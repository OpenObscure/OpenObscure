use regex::{Regex, RegexSet};
use serde_json::Value;

use crate::pii_types::PiiType;

/// A single detected PII match within a text body.
#[derive(Debug, Clone)]
pub struct PiiMatch {
    pub pii_type: PiiType,
    /// Byte offset start in the scanned string.
    pub start: usize,
    /// Byte offset end in the scanned string.
    pub end: usize,
    /// The raw matched text.
    pub raw_value: String,
    /// The JSON path where this match was found (e.g., "messages[0].content").
    pub json_path: Option<String>,
    /// Detection confidence (0.0–1.0). Regex/keyword = 1.0, NER/CRF = model score.
    pub confidence: f32,
}

/// Compiled PII scanner. Built once at startup, shared via Arc.
pub struct PiiScanner {
    regex_set: RegexSet,
    patterns: Vec<(PiiType, Regex)>,
}

impl PiiScanner {
    pub fn new() -> Self {
        let pattern_defs: Vec<(PiiType, &str)> = vec![
            // Credit cards: major brands, 13-19 digits, optional dashes/spaces
            (
                PiiType::CreditCard,
                r"\b(?:4[0-9]{3}|5[1-5][0-9]{2}|3[47][0-9]{2}|6(?:011|5[0-9]{2}))[- ]?[0-9]{4}[- ]?[0-9]{4}[- ]?[0-9]{1,7}\b",
            ),
            // SSN: XXX-XX-XXXX with dashes or spaces (require separators to reduce false positives)
            (
                PiiType::Ssn,
                r"\b[0-9]{3}[- ][0-9]{2}[- ][0-9]{4}\b",
            ),
            // US/international phone numbers — requires at least one separator or leading +
            // to avoid matching bare digit runs inside credit card numbers etc.
            (
                PiiType::PhoneNumber,
                r"(?:\+[0-9]{1,3}[-.\s]?\(?[0-9]{3}\)?[-.\s]?[0-9]{3}[-.\s]?[0-9]{4}|\(?[0-9]{3}\)[-.\s]?[0-9]{3}[-.\s]?[0-9]{4}|\b[0-9]{3}[-.\s][0-9]{3}[-.\s]?[0-9]{4}\b)",
            ),
            // Email addresses
            (
                PiiType::Email,
                r"\b[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}\b",
            ),
            // API keys: common provider prefixes
            (
                PiiType::ApiKey,
                r"\b(?:sk-ant-[a-zA-Z0-9_-]{20,}|sk-[a-zA-Z0-9]{20,}|AKIA[0-9A-Z]{16}|ghp_[a-zA-Z0-9]{36,}|gho_[a-zA-Z0-9]{36,}|xoxb-[0-9]+-[a-zA-Z0-9]+|xoxp-[0-9]+-[a-zA-Z0-9]+)\b",
            ),
        ];

        let regex_strings: Vec<&str> = pattern_defs.iter().map(|(_, pat)| *pat).collect();
        let regex_set = RegexSet::new(&regex_strings).expect("Invalid regex patterns in scanner");
        let patterns: Vec<(PiiType, Regex)> = pattern_defs
            .into_iter()
            .map(|(pii_type, pat)| (pii_type, Regex::new(pat).expect("Invalid regex")))
            .collect();

        Self {
            regex_set,
            patterns,
        }
    }

    /// Scan a text string for all PII matches. Returns matches sorted by start offset.
    pub fn scan_text(&self, text: &str) -> Vec<PiiMatch> {
        let matching_indices: Vec<usize> = self.regex_set.matches(text).into_iter().collect();
        if matching_indices.is_empty() {
            return Vec::new();
        }

        let mut matches = Vec::new();
        for &idx in &matching_indices {
            let (pii_type, regex) = &self.patterns[idx];
            for m in regex.find_iter(text) {
                let raw_value = m.as_str().to_string();

                // Post-validation filters
                if !self.validate_match(*pii_type, &raw_value) {
                    continue;
                }

                matches.push(PiiMatch {
                    pii_type: *pii_type,
                    start: m.start(),
                    end: m.end(),
                    raw_value,
                    json_path: None,
                    confidence: 1.0,
                });
            }
        }

        // Deduplicate overlapping matches (prefer longer match)
        matches.sort_by_key(|m| (m.start, std::cmp::Reverse(m.end)));
        let mut deduped = Vec::new();
        let mut last_end = 0;
        for m in matches {
            if m.start >= last_end {
                last_end = m.end;
                deduped.push(m);
            }
        }

        deduped
    }

    /// Scan a JSON body, traversing string values and tracking JSON paths.
    pub fn scan_json(&self, json: &Value, skip_fields: &[String]) -> Vec<PiiMatch> {
        let mut matches = Vec::new();
        self.scan_json_recursive(json, "", skip_fields, &mut matches);
        matches
    }

    fn scan_json_recursive(
        &self,
        value: &Value,
        path: &str,
        skip_fields: &[String],
        matches: &mut Vec<PiiMatch>,
    ) {
        match value {
            Value::String(s) => {
                let mut text_matches = self.scan_text(s);
                for m in &mut text_matches {
                    m.json_path = Some(path.to_string());
                }
                matches.extend(text_matches);
            }
            Value::Object(map) => {
                for (key, val) in map {
                    if skip_fields.contains(key) {
                        continue;
                    }
                    let child_path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{}.{}", path, key)
                    };
                    self.scan_json_recursive(val, &child_path, skip_fields, matches);
                }
            }
            Value::Array(arr) => {
                for (i, val) in arr.iter().enumerate() {
                    let child_path = format!("{}[{}]", path, i);
                    self.scan_json_recursive(val, &child_path, skip_fields, matches);
                }
            }
            _ => {} // Skip numbers, bools, nulls
        }
    }

    /// Post-validation for specific PII types.
    fn validate_match(&self, pii_type: PiiType, raw: &str) -> bool {
        match pii_type {
            PiiType::CreditCard => luhn_check(raw),
            PiiType::Ssn => validate_ssn(raw),
            _ => true,
        }
    }
}

/// Luhn algorithm check for credit card numbers.
fn luhn_check(raw: &str) -> bool {
    let digits: Vec<u32> = raw
        .chars()
        .filter(|c| c.is_ascii_digit())
        .filter_map(|c| c.to_digit(10))
        .collect();

    if digits.len() < 13 || digits.len() > 19 {
        return false;
    }

    let mut sum = 0u32;
    let mut double = false;
    for &d in digits.iter().rev() {
        let mut val = d;
        if double {
            val *= 2;
            if val > 9 {
                val -= 9;
            }
        }
        sum += val;
        double = !double;
    }
    sum % 10 == 0
}

/// Validate SSN ranges (reject 000, 666, 900-999 area numbers).
fn validate_ssn(raw: &str) -> bool {
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() != 9 {
        return false;
    }
    let area: u32 = digits[0..3].parse().unwrap_or(0);
    let group: u32 = digits[3..5].parse().unwrap_or(0);
    let serial: u32 = digits[5..9].parse().unwrap_or(0);

    // Invalid areas: 000, 666, 900-999
    if area == 0 || area == 666 || area >= 900 {
        return false;
    }
    if group == 0 || serial == 0 {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_credit_card_detection() {
        let scanner = PiiScanner::new();
        // Valid Visa test card (passes Luhn: 4111111111111111)
        let matches = scanner.scan_text("My card is 4111111111111111");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, PiiType::CreditCard);

        // With dashes (Luhn-valid)
        let matches = scanner.scan_text("Card: 4111-1111-1111-1111");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, PiiType::CreditCard);
    }

    #[test]
    fn test_credit_card_luhn_rejection() {
        let scanner = PiiScanner::new();
        // Invalid Luhn — increment last digit of valid card
        let matches = scanner.scan_text("Not a card: 4111-1111-1111-1112");
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_ssn_detection() {
        let scanner = PiiScanner::new();
        let matches = scanner.scan_text("SSN: 123-45-6789");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, PiiType::Ssn);
        assert_eq!(matches[0].raw_value, "123-45-6789");
    }

    #[test]
    fn test_ssn_invalid_area() {
        let scanner = PiiScanner::new();
        // Area 000 is invalid
        let matches = scanner.scan_text("SSN: 000-45-6789");
        assert_eq!(matches.len(), 0);
        // Area 666 is invalid
        let matches = scanner.scan_text("SSN: 666-45-6789");
        assert_eq!(matches.len(), 0);
        // Area 900+ is invalid
        let matches = scanner.scan_text("SSN: 900-45-6789");
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_phone_detection() {
        let scanner = PiiScanner::new();
        let matches = scanner.scan_text("Call me at (555) 123-4567");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, PiiType::PhoneNumber);

        let matches = scanner.scan_text("Phone: +1-555-123-4567");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_email_detection() {
        let scanner = PiiScanner::new();
        let matches = scanner.scan_text("Email john.doe@example.com for info");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, PiiType::Email);
        assert_eq!(matches[0].raw_value, "john.doe@example.com");
    }

    #[test]
    fn test_api_key_detection() {
        let scanner = PiiScanner::new();
        let matches = scanner.scan_text("Key: sk-ant-api03-abcdefghijklmnopqrstuvwx");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, PiiType::ApiKey);
    }

    #[test]
    fn test_multiple_pii_in_text() {
        let scanner = PiiScanner::new();
        let text = "SSN: 123-45-6789, email: test@example.com";
        let matches = scanner.scan_text(text);
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn test_json_scanning() {
        let scanner = PiiScanner::new();
        let json: Value = serde_json::json!({
            "model": "claude-sonnet-4-20250514",
            "messages": [
                {
                    "role": "user",
                    "content": "My SSN is 123-45-6789"
                }
            ]
        });

        let skip_fields = vec!["model".to_string()];
        let matches = scanner.scan_json(&json, &skip_fields);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, PiiType::Ssn);
        assert_eq!(
            matches[0].json_path.as_deref(),
            Some("messages[0].content")
        );
    }

    #[test]
    fn test_skip_fields() {
        let scanner = PiiScanner::new();
        let json: Value = serde_json::json!({
            "model": "123-45-6789",
            "temperature": "123-45-6789",
            "content": "123-45-6789"
        });

        let skip_fields = vec!["model".to_string(), "temperature".to_string()];
        let matches = scanner.scan_json(&json, &skip_fields);
        // Only the "content" field should be scanned
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].json_path.as_deref(), Some("content"));
    }

    #[test]
    fn test_no_false_positives_on_clean_text() {
        let scanner = PiiScanner::new();
        let matches = scanner.scan_text("Hello, how are you today? The weather is nice.");
        assert_eq!(matches.len(), 0);
    }
}

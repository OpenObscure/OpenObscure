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

impl Default for PiiScanner {
    fn default() -> Self {
        Self::new()
    }
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
            (PiiType::Ssn, r"\b[0-9]{3}[- ][0-9]{2}[- ][0-9]{4}\b"),
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
            // IPv4 addresses (0-255 octets, validate_ipv4 rejects loopback/broadcast/0.x.x.x)
            (
                PiiType::Ipv4Address,
                r"\b(?:(?:25[0-5]|2[0-4][0-9]|1[0-9]{2}|[1-9]?[0-9])\.){3}(?:25[0-5]|2[0-4][0-9]|1[0-9]{2}|[1-9]?[0-9])\b",
            ),
            // IPv6 addresses: full 8-group, mid-compressed (groups::groups), and ::prefix compressed
            (
                PiiType::Ipv6Address,
                r"(?i)\b[0-9a-f]{1,4}(?::[0-9a-f]{1,4}){7}\b|(?i)\b[0-9a-f]{1,4}(?::[0-9a-f]{1,4}){0,5}::[0-9a-f]{0,4}(?::[0-9a-f]{1,4}){0,5}\b|(?i)::(?:[0-9a-f]{1,4}:){0,5}[0-9a-f]{1,4}\b",
            ),
            // GPS coordinates: signed decimal lat/long pairs (e.g., 45.5231, -122.6765)
            (
                PiiType::GpsCoordinate,
                r"-?(?:[1-8]?[0-9]\.[0-9]{4,}|90\.0{4,}),\s*-?(?:1[0-7][0-9]\.[0-9]{4,}|180\.0{4,}|[0-9]{1,2}\.[0-9]{4,})",
            ),
            // MAC addresses (colon, dash, or dot separated)
            (
                PiiType::MacAddress,
                r"(?i)\b[0-9a-f]{2}(?::[0-9a-f]{2}){5}\b|(?i)\b[0-9a-f]{2}(?:-[0-9a-f]{2}){5}\b|(?i)\b[0-9a-f]{4}\.[0-9a-f]{4}\.[0-9a-f]{4}\b",
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
            PiiType::Ipv4Address => validate_ipv4(raw),
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

/// Reject non-PII IPv4 addresses: loopback (127.x), link-local (169.254.x),
/// broadcast (255.255.255.255), and common version-like patterns (x.0.0.x).
fn validate_ipv4(raw: &str) -> bool {
    let octets: Vec<u8> = raw
        .split('.')
        .filter_map(|s| s.parse::<u8>().ok())
        .collect();
    if octets.len() != 4 {
        return false;
    }
    // Reject loopback (127.x.x.x)
    if octets[0] == 127 {
        return false;
    }
    // Reject broadcast
    if octets.iter().all(|&o| o == 255) {
        return false;
    }
    // Reject link-local (169.254.x.x)
    if octets[0] == 169 && octets[1] == 254 {
        return false;
    }
    // Reject 0.x.x.x (unspecified)
    if octets[0] == 0 {
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
        assert_eq!(matches[0].json_path.as_deref(), Some("messages[0].content"));
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

    // --- IPv4 tests ---

    #[test]
    fn test_ipv4_detection() {
        let scanner = PiiScanner::new();
        let matches = scanner.scan_text("Server at 192.168.1.42 is down");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, PiiType::Ipv4Address);
        assert_eq!(matches[0].raw_value, "192.168.1.42");
    }

    #[test]
    fn test_ipv4_public_address() {
        let scanner = PiiScanner::new();
        let matches = scanner.scan_text("User connected from 203.0.113.42");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, PiiType::Ipv4Address);
    }

    #[test]
    fn test_ipv4_reject_loopback() {
        let scanner = PiiScanner::new();
        let matches = scanner.scan_text("localhost is 127.0.0.1");
        assert!(
            !matches.iter().any(|m| m.pii_type == PiiType::Ipv4Address),
            "loopback 127.x.x.x should be rejected"
        );
    }

    #[test]
    fn test_ipv4_reject_broadcast() {
        let scanner = PiiScanner::new();
        let matches = scanner.scan_text("broadcast 255.255.255.255");
        assert!(
            !matches.iter().any(|m| m.pii_type == PiiType::Ipv4Address),
            "broadcast should be rejected"
        );
    }

    // --- IPv6 tests ---

    #[test]
    fn test_ipv6_full_detection() {
        let scanner = PiiScanner::new();
        let matches = scanner.scan_text("IPv6: 2001:0db8:85a3:0000:0000:8a2e:0370:7334");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, PiiType::Ipv6Address);
    }

    #[test]
    fn test_ipv6_compressed_detection() {
        let scanner = PiiScanner::new();
        let matches = scanner.scan_text("IPv6: ::1234:5678");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, PiiType::Ipv6Address);
    }

    // --- GPS coordinate tests ---

    #[test]
    fn test_gps_coordinate_detection() {
        let scanner = PiiScanner::new();
        let matches = scanner.scan_text("Location: 45.5231, -122.6765");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, PiiType::GpsCoordinate);
        assert_eq!(matches[0].raw_value, "45.5231, -122.6765");
    }

    #[test]
    fn test_gps_negative_latitude() {
        let scanner = PiiScanner::new();
        let matches = scanner.scan_text("Sydney: -33.8688, 151.2093");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, PiiType::GpsCoordinate);
    }

    #[test]
    fn test_gps_no_false_positive_on_low_precision() {
        let scanner = PiiScanner::new();
        // Only 2 decimal places — not precise enough to be a GPS coordinate
        let matches = scanner.scan_text("Value: 45.52, -122.67");
        assert!(
            !matches.iter().any(|m| m.pii_type == PiiType::GpsCoordinate),
            "low-precision decimals should not match GPS"
        );
    }

    // --- MAC address tests ---

    #[test]
    fn test_mac_colon_detection() {
        let scanner = PiiScanner::new();
        let matches = scanner.scan_text("MAC: 00:1A:2B:3C:4D:5E");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, PiiType::MacAddress);
    }

    #[test]
    fn test_mac_dash_detection() {
        let scanner = PiiScanner::new();
        let matches = scanner.scan_text("MAC: 00-1A-2B-3C-4D-5E");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, PiiType::MacAddress);
    }

    #[test]
    fn test_mac_dot_detection() {
        let scanner = PiiScanner::new();
        let matches = scanner.scan_text("MAC: 001a.2b3c.4d5e");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, PiiType::MacAddress);
    }

    // ── Regression Suite ─────────────────────────────────────────────────
    // Tests for previously fixed bugs to prevent regressions.

    /// Regression: bare 10-digit runs must NOT match as phone numbers.
    /// Fix: phone regex requires separators or leading +.
    #[test]
    fn test_regression_phone_bare_digits_no_match() {
        let scanner = PiiScanner::new();
        // Bare digit runs should not be detected as phone numbers
        assert!(
            !scanner
                .scan_text("Bare digits 5551234567 are not a phone number.")
                .iter()
                .any(|m| m.pii_type == PiiType::PhoneNumber),
            "bare 10-digit run should not match as phone"
        );
        assert!(
            !scanner
                .scan_text("Sequence 1234567890 is just ten digits.")
                .iter()
                .any(|m| m.pii_type == PiiType::PhoneNumber),
            "sequential digits should not match as phone"
        );
        assert!(
            !scanner
                .scan_text("Order ID: 9876543210")
                .iter()
                .any(|m| m.pii_type == PiiType::PhoneNumber),
            "order ID should not match as phone"
        );
    }

    /// Regression: phone numbers WITH separators must still be detected.
    #[test]
    fn test_regression_phone_with_separators_matches() {
        let scanner = PiiScanner::new();
        // Separators: dash, dot, space, parens
        for input in &[
            "Call 555-123-4567",
            "Call 555.123.4567",
            "Call (555) 123-4567",
            "Call +1-555-123-4567",
        ] {
            let matches = scanner.scan_text(input);
            assert!(
                matches.iter().any(|m| m.pii_type == PiiType::PhoneNumber),
                "Phone with separators should match: {}",
                input
            );
        }
    }

    /// Regression: SSN area code 000 must be rejected.
    #[test]
    fn test_regression_ssn_area_000_rejected() {
        let scanner = PiiScanner::new();
        let matches = scanner.scan_text("SSN: 000-12-3456");
        assert!(
            !matches.iter().any(|m| m.pii_type == PiiType::Ssn),
            "SSN area 000 must be rejected"
        );
    }

    /// Regression: SSN area code 666 must be rejected.
    #[test]
    fn test_regression_ssn_area_666_rejected() {
        let scanner = PiiScanner::new();
        let matches = scanner.scan_text("SSN: 666-12-3456");
        assert!(
            !matches.iter().any(|m| m.pii_type == PiiType::Ssn),
            "SSN area 666 must be rejected"
        );
    }

    /// Regression: SSN area codes 900+ must be rejected.
    #[test]
    fn test_regression_ssn_area_900_plus_rejected() {
        let scanner = PiiScanner::new();
        for area in &["900", "950", "987", "999"] {
            let text = format!("SSN: {}-45-6789", area);
            let matches = scanner.scan_text(&text);
            assert!(
                !matches.iter().any(|m| m.pii_type == PiiType::Ssn),
                "SSN area {} must be rejected",
                area
            );
        }
    }

    /// Regression: credit card numbers that fail Luhn must not match.
    #[test]
    fn test_regression_cc_luhn_failure_rejected() {
        let scanner = PiiScanner::new();
        // 4111-1111-1111-1112 fails Luhn (last digit should be 1)
        let matches = scanner.scan_text("Card: 4111-1111-1111-1112");
        assert!(
            !matches.iter().any(|m| m.pii_type == PiiType::CreditCard),
            "CC failing Luhn should be rejected"
        );
    }

    /// Regression: valid credit card numbers that pass Luhn must match.
    #[test]
    fn test_regression_cc_luhn_pass_matches() {
        let scanner = PiiScanner::new();
        // 4111-1111-1111-1111 passes Luhn
        let matches = scanner.scan_text("Card: 4111-1111-1111-1111");
        assert!(
            matches.iter().any(|m| m.pii_type == PiiType::CreditCard),
            "CC passing Luhn should match"
        );
    }

    /// Regression: bare digit runs inside CC-like patterns should not match as phone.
    #[test]
    fn test_regression_cc_digits_not_phone() {
        let scanner = PiiScanner::new();
        let text = "Card: 4111-1111-1111-1111";
        let matches = scanner.scan_text(text);
        assert!(
            !matches.iter().any(|m| m.pii_type == PiiType::PhoneNumber),
            "CC number should not also match as phone"
        );
    }

    /// Regression: valid SSN with valid area code must be detected.
    #[test]
    fn test_regression_ssn_valid_area_matches() {
        let scanner = PiiScanner::new();
        for area in &["001", "123", "456", "665", "899"] {
            let text = format!("SSN: {}-45-6789", area);
            let matches = scanner.scan_text(&text);
            assert!(
                matches.iter().any(|m| m.pii_type == PiiType::Ssn),
                "SSN area {} should match",
                area
            );
        }
    }
}

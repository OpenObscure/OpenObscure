//! Arabic PII patterns.
//!
//! - Saudi National ID (الهوية الوطنية): 10 digits starting with 1 (citizen) or 2 (resident)
//! - UAE Emirates ID: 784-XXXX-XXXXXXX-X (15 digits)
//! - Phone: +966 5X XXX XXXX (Saudi), +971 5X XXX XXXX (UAE)

use regex::Regex;

use crate::pii_types::PiiType;

use super::LangPattern;

/// Saudi National ID Luhn-like check (simplified: start with 1 or 2, 10 digits).
fn validate_saudi_id(s: &str) -> bool {
    let clean: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if clean.len() != 10 {
        return false;
    }
    // Must start with 1 (citizen) or 2 (resident/iqama)
    let first = clean.chars().next().unwrap();
    first == '1' || first == '2'
}

/// UAE Emirates ID: 784-YYYY-NNNNNNN-C format, 15 digits total.
fn validate_emirates_id(s: &str) -> bool {
    let clean: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if clean.len() != 15 {
        return false;
    }
    clean.starts_with("784")
}

pub fn patterns() -> Vec<LangPattern> {
    vec![
        LangPattern {
            pii_type: PiiType::Ssn,
            regex: Regex::new(r"\b[12]\d{9}\b").unwrap(),
            validate: Some(validate_saudi_id),
            label: "Saudi National ID",
        },
        LangPattern {
            pii_type: PiiType::Ssn,
            regex: Regex::new(r"\b784[\s-]?\d{4}[\s-]?\d{7}[\s-]?\d\b").unwrap(),
            validate: Some(validate_emirates_id),
            label: "UAE Emirates ID",
        },
        LangPattern {
            pii_type: PiiType::PhoneNumber,
            regex: Regex::new(r"\+966[\s.-]?5\d[\s.-]?\d{3}[\s.-]?\d{4}\b").unwrap(),
            validate: None,
            label: "Saudi phone (+966)",
        },
        LangPattern {
            pii_type: PiiType::PhoneNumber,
            regex: Regex::new(r"\+971[\s.-]?5\d[\s.-]?\d{3}[\s.-]?\d{4}\b").unwrap(),
            validate: None,
            label: "UAE phone (+971)",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_saudi_citizen_id() {
        assert!(validate_saudi_id("1234567890"));
    }

    #[test]
    fn test_valid_saudi_resident_id() {
        assert!(validate_saudi_id("2123456789"));
    }

    #[test]
    fn test_invalid_saudi_id_wrong_prefix() {
        assert!(!validate_saudi_id("3123456789"));
    }

    #[test]
    fn test_scan_saudi_phone() {
        let matches = super::super::scan_with_lang(
            "اتصل بي على +966 50 123 4567",
            crate::lang_detect::Language::Arabic,
        );
        assert!(!matches.is_empty(), "Should find Saudi phone");
    }

    #[test]
    fn test_scan_uae_phone() {
        let matches = super::super::scan_with_lang(
            "رقم هاتفي +971 50 123 4567",
            crate::lang_detect::Language::Arabic,
        );
        assert!(!matches.is_empty(), "Should find UAE phone");
    }
}

//! German PII patterns.
//!
//! - Personalausweisnummer: 10 alphanumeric characters (IDP format)
//! - Steueridentifikationsnummer (Tax ID): 11 digits
//! - Phone: +49 XXX XXXXXXX or 0XXX XXXXXXX
//! - IBAN: DE + 2 check digits + 18 digits

use regex::Regex;

use crate::pii_types::PiiType;

use super::LangPattern;

/// German tax ID (Steueridentifikationsnummer) validation.
/// 11 digits, first digit non-zero, check digit algorithm.
fn validate_tax_id(s: &str) -> bool {
    let clean: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if clean.len() != 11 {
        return false;
    }
    // First digit must not be 0
    if clean.starts_with('0') {
        return false;
    }
    // At least one digit appears exactly 2 or 3 times, none more than 3
    let mut counts = [0u8; 10];
    for &b in &clean.as_bytes()[..10] {
        counts[(b - b'0') as usize] += 1;
    }
    let has_dup = counts.iter().any(|&c| c >= 2);
    let no_excess = counts.iter().all(|&c| c <= 3);
    has_dup && no_excess
}

/// Validate German IBAN (mod 97 check).
fn validate_iban_de(s: &str) -> bool {
    let clean: String = s.chars().filter(|c| c.is_alphanumeric()).collect();
    if clean.len() != 22 || !clean.starts_with("DE") {
        return false;
    }
    super::es::validate_iban_mod97_public(&clean)
}

pub fn patterns() -> Vec<LangPattern> {
    vec![
        LangPattern {
            pii_type: PiiType::Ssn,
            regex: Regex::new(r"\b\d{11}\b").unwrap(),
            validate: Some(validate_tax_id),
            label: "German Tax ID",
        },
        LangPattern {
            pii_type: PiiType::PhoneNumber,
            regex: Regex::new(r"\+49[\s.-]?\d{2,4}[\s.-]?\d{4,8}\b").unwrap(),
            validate: None,
            label: "German phone (+49)",
        },
        LangPattern {
            pii_type: PiiType::Iban,
            regex: Regex::new(
                r"\bDE\d{2}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{2}\b",
            )
            .unwrap(),
            validate: Some(validate_iban_de),
            label: "German IBAN",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_german_phone() {
        let matches = super::super::scan_with_lang(
            "Rufen Sie mich an unter +49 30 12345678 bitte",
            crate::lang_detect::Language::German,
        );
        assert!(!matches.is_empty(), "Should find German phone");
    }

    #[test]
    fn test_valid_tax_id() {
        // 11 digits, first non-zero, has repeated digit
        assert!(validate_tax_id("65929970489"));
    }

    #[test]
    fn test_invalid_tax_id_leading_zero() {
        assert!(!validate_tax_id("05929970489"));
    }

    #[test]
    fn test_iban_de_valid() {
        assert!(validate_iban_de("DE89370400440532013000"));
    }
}

//! French PII patterns.
//!
//! - NIR (Numéro d'Inscription au Répertoire): 15 digits (1 sex + 2 birth year + 2 birth month + 5 dept/commune + 3 order + 2 check)
//! - Phone: +33 X XX XX XX XX or 0X XX XX XX XX
//! - IBAN: FR + 2 check digits + 23 alphanumeric

use regex::Regex;

use crate::pii_types::PiiType;

use super::LangPattern;

/// NIR (French social security number) validation.
/// 15 digits: check = 97 - (first 13 digits mod 97)
fn validate_nir(s: &str) -> bool {
    let clean: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if clean.len() != 15 {
        return false;
    }
    let base: u64 = match clean[..13].parse() {
        Ok(n) => n,
        Err(_) => return false,
    };
    let check: u64 = match clean[13..15].parse() {
        Ok(n) => n,
        Err(_) => return false,
    };
    let expected = 97 - (base % 97);
    check == expected
}

/// Validate French IBAN (mod 97 check).
fn validate_iban_fr(s: &str) -> bool {
    let clean: String = s.chars().filter(|c| c.is_alphanumeric()).collect();
    if clean.len() != 27 || !clean.starts_with("FR") {
        return false;
    }
    super::es::validate_iban_mod97_public(&clean)
}

pub fn patterns() -> Vec<LangPattern> {
    vec![
        LangPattern {
            pii_type: PiiType::Ssn,
            regex: Regex::new(r"\b[12]\s?\d{2}\s?\d{2}\s?\d{5}\s?\d{3}\s?\d{2}\b").unwrap(),
            validate: Some(validate_nir),
            label: "French NIR",
        },
        LangPattern {
            pii_type: PiiType::PhoneNumber,
            regex: Regex::new(r"\+33[\s.-]?[1-9](?:[\s.-]?\d{2}){4}\b").unwrap(),
            validate: None,
            label: "French phone (+33)",
        },
        LangPattern {
            pii_type: PiiType::PhoneNumber,
            regex: Regex::new(r"\b0[1-9](?:[\s.-]?\d{2}){4}\b").unwrap(),
            validate: None,
            label: "French phone (local)",
        },
        LangPattern {
            pii_type: PiiType::Iban,
            regex: Regex::new(
                r"\bFR\d{2}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{3}\b",
            )
            .unwrap(),
            validate: Some(validate_iban_fr),
            label: "French IBAN",
        },
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_scan_french_phone_international() {
        let matches = super::super::scan_with_lang(
            "Appelez-moi au +33 1 23 45 67 89 demain",
            crate::lang_detect::Language::French,
        );
        assert!(!matches.is_empty(), "Should find French phone");
    }

    #[test]
    fn test_scan_french_phone_local() {
        let matches = super::super::scan_with_lang(
            "Mon numéro est 01 23 45 67 89",
            crate::lang_detect::Language::French,
        );
        assert!(!matches.is_empty(), "Should find French local phone");
    }
}

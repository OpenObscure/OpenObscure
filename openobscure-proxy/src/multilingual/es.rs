//! Spanish PII patterns.
//!
//! - DNI (Documento Nacional de Identidad): 8 digits + check letter
//! - NIE (Número de Identidad de Extranjero): X/Y/Z + 7 digits + check letter
//! - Phone: +34 XXX XXX XXX or 9XX XXX XXX
//! - IBAN: ES + 2 check digits + 20 digits

use regex::Regex;

use crate::pii_types::PiiType;

use super::LangPattern;

/// DNI check letter validation.
/// letter = "TRWAGMYFPDXBNJZSQVHLCKE"[number % 23]
fn validate_dni(s: &str) -> bool {
    let s = s.replace('-', "");
    if s.len() != 9 {
        return false;
    }
    let digits = &s[..8];
    let letter = s.chars().last().unwrap().to_ascii_uppercase();
    let num: u64 = match digits.parse() {
        Ok(n) => n,
        Err(_) => return false,
    };
    let table = b"TRWAGMYFPDXBNJZSQVHLCKE";
    let expected = table[(num % 23) as usize] as char;
    letter == expected
}

/// NIE check letter validation (same algorithm, X→0, Y→1, Z→2 prefix).
fn validate_nie(s: &str) -> bool {
    let s = s.replace('-', "");
    if s.len() != 9 {
        return false;
    }
    let prefix = match s.chars().next().unwrap().to_ascii_uppercase() {
        'X' => '0',
        'Y' => '1',
        'Z' => '2',
        _ => return false,
    };
    let digits = format!("{}{}", prefix, &s[1..8]);
    let letter = s.chars().last().unwrap().to_ascii_uppercase();
    let num: u64 = match digits.parse() {
        Ok(n) => n,
        Err(_) => return false,
    };
    let table = b"TRWAGMYFPDXBNJZSQVHLCKE";
    let expected = table[(num % 23) as usize] as char;
    letter == expected
}

/// IBAN ES check digit validation (mod 97).
fn validate_iban_es(s: &str) -> bool {
    let clean: String = s.chars().filter(|c| c.is_alphanumeric()).collect();
    if clean.len() != 24 || !clean.starts_with("ES") {
        return false;
    }
    validate_iban_mod97(&clean)
}

/// Public IBAN mod-97 validation, reusable by other language modules.
pub fn validate_iban_mod97_public(iban: &str) -> bool {
    validate_iban_mod97(iban)
}

fn validate_iban_mod97(iban: &str) -> bool {
    // Move first 4 chars to end, convert letters to digits (A=10..Z=35)
    let rearranged = format!("{}{}", &iban[4..], &iban[..4]);
    let mut num_str = String::with_capacity(rearranged.len() * 2);
    for ch in rearranged.chars() {
        if ch.is_ascii_digit() {
            num_str.push(ch);
        } else {
            let val = ch.to_ascii_uppercase() as u32 - 'A' as u32 + 10;
            num_str.push_str(&val.to_string());
        }
    }
    // Compute mod 97 using chunked arithmetic (avoid u128)
    let mut remainder: u64 = 0;
    for chunk in num_str.as_bytes().chunks(9) {
        let s: String = std::str::from_utf8(chunk).unwrap().to_string();
        let combined = format!("{}{}", remainder, s);
        remainder = combined.parse::<u64>().unwrap_or(0) % 97;
    }
    remainder == 1
}

pub fn patterns() -> Vec<LangPattern> {
    vec![
        LangPattern {
            pii_type: PiiType::Ssn, // Reuse SSN type for national IDs
            regex: Regex::new(r"\b[0-9]{8}-?[A-Za-z]\b").unwrap(),
            validate: Some(validate_dni),
            label: "Spanish DNI",
        },
        LangPattern {
            pii_type: PiiType::Ssn,
            regex: Regex::new(r"\b[XYZxyz]-?[0-9]{7}-?[A-Za-z]\b").unwrap(),
            validate: Some(validate_nie),
            label: "Spanish NIE",
        },
        LangPattern {
            pii_type: PiiType::PhoneNumber,
            regex: Regex::new(r"\+34[\s.-]?[0-9]{3}[\s.-]?[0-9]{3}[\s.-]?[0-9]{3}\b").unwrap(),
            validate: None,
            label: "Spanish phone (+34)",
        },
        LangPattern {
            pii_type: PiiType::Iban,
            regex: Regex::new(
                r"\bES\d{2}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{4}\b",
            )
            .unwrap(),
            validate: Some(validate_iban_es),
            label: "Spanish IBAN",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_dni() {
        assert!(validate_dni("12345678Z"));
        assert!(validate_dni("00000000T"));
    }

    #[test]
    fn test_invalid_dni_letter() {
        assert!(!validate_dni("12345678A")); // Wrong letter
    }

    #[test]
    fn test_valid_nie() {
        // X0000000T
        assert!(validate_nie("X0000000T"));
    }

    #[test]
    fn test_scan_dni() {
        let matches = super::super::scan_with_lang(
            "Mi DNI es 12345678Z y vivo en Madrid",
            crate::lang_detect::Language::Spanish,
        );
        assert!(!matches.is_empty(), "Should find DNI");
    }

    #[test]
    fn test_scan_spanish_phone() {
        let matches = super::super::scan_with_lang(
            "Llámame al +34 612 345 678 por favor",
            crate::lang_detect::Language::Spanish,
        );
        assert!(!matches.is_empty(), "Should find Spanish phone");
    }

    #[test]
    fn test_iban_es_valid() {
        // ES91 2100 0418 4502 0005 1332 is a well-known test IBAN
        assert!(validate_iban_es("ES9121000418450200051332"));
    }
}

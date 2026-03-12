//! Korean PII patterns.
//!
//! - RRN (Resident Registration Number, 주민등록번호): 13 digits (YYMMDD-GXXXXXX)
//!   6 birthdate + 1 gender/century + 6 serial/check
//! - Phone: +82 10-XXXX-XXXX or 010-XXXX-XXXX

use regex::Regex;

use crate::pii_types::PiiType;

use super::LangPattern;

/// Korean RRN check digit validation.
/// 13 digits: weighted sum mod 11, check = (11 - sum%11) mod 10.
fn validate_rrn(s: &str) -> bool {
    let clean: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if clean.len() != 13 {
        return false;
    }
    let digits: Vec<u32> = clean.chars().map(|c| c as u32 - '0' as u32).collect();
    let weights = [2, 3, 4, 5, 6, 7, 8, 9, 2, 3, 4, 5];
    let sum: u32 = digits[..12]
        .iter()
        .zip(weights.iter())
        .map(|(&d, &w)| d * w)
        .sum();
    let check = (11 - sum % 11) % 10;
    check == digits[12]
}

pub fn patterns() -> Vec<LangPattern> {
    vec![
        LangPattern {
            pii_type: PiiType::Ssn,
            regex: Regex::new(r"\d{6}[\s-]?[1-4]\d{6}").unwrap(),
            validate: Some(validate_rrn),
            label: "Korean RRN",
        },
        LangPattern {
            pii_type: PiiType::PhoneNumber,
            regex: Regex::new(r"\+82[\s.-]?10[\s.-]?\d{4}[\s.-]?\d{4}").unwrap(),
            validate: None,
            label: "Korean phone (+82)",
        },
        LangPattern {
            pii_type: PiiType::PhoneNumber,
            regex: Regex::new(r"010[\s.-]?\d{4}[\s.-]?\d{4}").unwrap(),
            validate: None,
            label: "Korean mobile phone",
        },
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_scan_korean_phone() {
        let matches = super::super::scan_with_lang(
            "전화번호는 +82 10 1234 5678입니다",
            crate::lang_detect::Language::Korean,
        );
        assert!(!matches.is_empty(), "Should find Korean phone");
    }

    #[test]
    fn test_scan_korean_mobile() {
        let matches = super::super::scan_with_lang(
            "핸드폰 번호: 010-1234-5678",
            crate::lang_detect::Language::Korean,
        );
        assert!(!matches.is_empty(), "Should find Korean mobile");
    }
}

//! Chinese PII patterns.
//!
//! - Citizen ID (居民身份证号): 18 digits (6 region + 8 birthdate + 3 seq + 1 check)
//! - Phone: +86 1XX XXXX XXXX (11 digits, starts with 1)

use regex::Regex;

use crate::pii_types::PiiType;

use super::LangPattern;

/// Chinese Citizen ID (18-digit) check digit validation.
/// Last character is check digit: weighted sum mod 11, mapped to 0-9 or X.
fn validate_citizen_id(s: &str) -> bool {
    let clean: String = s.chars().filter(|c| c.is_alphanumeric()).collect();
    if clean.len() != 18 {
        return false;
    }
    let weights = [7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2];
    let check_chars = ['1', '0', 'X', '9', '8', '7', '6', '5', '4', '3', '2'];

    let digits: Vec<u32> = clean[..17].chars().map(|c| c as u32 - '0' as u32).collect();
    if digits.len() != 17 || digits.iter().any(|&d| d > 9) {
        return false;
    }

    let sum: u32 = digits
        .iter()
        .zip(weights.iter())
        .map(|(&d, &w)| d * w as u32)
        .sum();
    let expected = check_chars[(sum % 11) as usize];
    let actual = clean.chars().last().unwrap().to_ascii_uppercase();
    actual == expected
}

pub fn patterns() -> Vec<LangPattern> {
    vec![
        LangPattern {
            pii_type: PiiType::Ssn,
            regex: Regex::new(
                r"[1-9]\d{5}(?:19|20)\d{2}(?:0[1-9]|1[0-2])(?:0[1-9]|[12]\d|3[01])\d{3}[\dXx]",
            )
            .unwrap(),
            validate: Some(validate_citizen_id),
            label: "Chinese Citizen ID",
        },
        LangPattern {
            pii_type: PiiType::PhoneNumber,
            regex: Regex::new(r"\+86[\s.-]?1[3-9]\d[\s.-]?\d{4}[\s.-]?\d{4}").unwrap(),
            validate: None,
            label: "Chinese phone (+86)",
        },
        LangPattern {
            pii_type: PiiType::PhoneNumber,
            regex: Regex::new(r"1[3-9]\d[\s.-]?\d{4}[\s.-]?\d{4}").unwrap(),
            validate: None,
            label: "Chinese mobile phone",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_chinese_phone() {
        let matches = super::super::scan_with_lang(
            "请拨打+86 138 0013 8000联系我",
            crate::lang_detect::Language::Chinese,
        );
        assert!(!matches.is_empty(), "Should find Chinese phone");
    }

    #[test]
    fn test_citizen_id_format() {
        // 11010519491231002X is a well-known test ID
        assert!(validate_citizen_id("11010519491231002X"));
    }
}

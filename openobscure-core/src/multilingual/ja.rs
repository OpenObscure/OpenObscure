//! Japanese PII patterns.
//!
//! - My Number (マイナンバー): 12 digits with check digit
//! - Phone: +81 XX-XXXX-XXXX or 0X0-XXXX-XXXX (mobile)
//! - Passport: 2 letters + 7 digits

use regex::Regex;

use crate::pii_types::PiiType;

use super::LangPattern;

/// My Number (個人番号) check digit validation.
/// 12 digits, last digit is check.
fn validate_my_number(s: &str) -> bool {
    let digits: Vec<u32> = s
        .chars()
        .filter(|c| c.is_ascii_digit())
        .map(|c| c as u32 - '0' as u32)
        .collect();
    if digits.len() != 12 {
        return false;
    }
    // Weights: positions 1-11 use Q_n = (n+1) for n<=6, else (n-5) for n>6
    // where position is counted from right (check digit = position 0)
    let mut sum: u32 = 0;
    for (i, &d) in digits.iter().enumerate().take(11) {
        let pos = 11 - i; // position from right (11 down to 1)
        let q = if pos <= 6 { pos + 1 } else { pos - 5 };
        sum += d * q as u32;
    }
    let remainder = sum % 11;
    let check = if remainder <= 1 { 0 } else { 11 - remainder };
    check == digits[11]
}

pub fn patterns() -> Vec<LangPattern> {
    vec![
        LangPattern {
            pii_type: PiiType::Ssn,
            regex: Regex::new(r"\d{4}[\s-]?\d{4}[\s-]?\d{4}").unwrap(),
            validate: Some(validate_my_number),
            label: "Japanese My Number",
        },
        LangPattern {
            pii_type: PiiType::PhoneNumber,
            regex: Regex::new(r"\+81[\s.-]?\d{1,2}[\s.-]?\d{4}[\s.-]?\d{4}").unwrap(),
            validate: None,
            label: "Japanese phone (+81)",
        },
        LangPattern {
            pii_type: PiiType::PhoneNumber,
            regex: Regex::new(r"0[789]0[\s.-]?\d{4}[\s.-]?\d{4}").unwrap(),
            validate: None,
            label: "Japanese mobile phone",
        },
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_scan_japanese_phone() {
        let matches = super::super::scan_with_lang(
            "電話番号は+81 90 1234 5678です",
            crate::lang_detect::Language::Japanese,
        );
        assert!(!matches.is_empty(), "Should find Japanese phone");
    }

    #[test]
    fn test_scan_japanese_mobile() {
        let matches = super::super::scan_with_lang(
            "携帯番号は090-1234-5678です",
            crate::lang_detect::Language::Japanese,
        );
        assert!(!matches.is_empty(), "Should find Japanese mobile");
    }
}

//! Portuguese/Brazilian PII patterns.
//!
//! - CPF (Cadastro de Pessoas Físicas): 11 digits (XXX.XXX.XXX-XX) with check digits
//! - CNPJ (Cadastro Nacional da Pessoa Jurídica): 14 digits (XX.XXX.XXX/XXXX-XX)
//! - Phone: +55 XX XXXXX-XXXX (Brazil mobile) or +351 XXX XXX XXX (Portugal)

use regex::Regex;

use crate::pii_types::PiiType;

use super::LangPattern;

/// CPF check digit validation (Brazilian individual tax ID).
fn validate_cpf(s: &str) -> bool {
    let digits: Vec<u32> = s
        .chars()
        .filter(|c| c.is_ascii_digit())
        .map(|c| c as u32 - '0' as u32)
        .collect();
    if digits.len() != 11 {
        return false;
    }
    // Reject all-same digits (e.g., 111.111.111-11)
    if digits.iter().all(|&d| d == digits[0]) {
        return false;
    }
    // First check digit
    let sum1: u32 = digits[..9]
        .iter()
        .enumerate()
        .map(|(i, &d)| d * (10 - i as u32))
        .sum();
    let d1 = (sum1 * 10 % 11) % 10;
    if d1 != digits[9] {
        return false;
    }
    // Second check digit
    let sum2: u32 = digits[..10]
        .iter()
        .enumerate()
        .map(|(i, &d)| d * (11 - i as u32))
        .sum();
    let d2 = (sum2 * 10 % 11) % 10;
    d2 == digits[10]
}

/// CNPJ check digit validation (Brazilian company tax ID).
fn validate_cnpj(s: &str) -> bool {
    let digits: Vec<u32> = s
        .chars()
        .filter(|c| c.is_ascii_digit())
        .map(|c| c as u32 - '0' as u32)
        .collect();
    if digits.len() != 14 {
        return false;
    }
    // First check digit
    let weights1 = [5, 4, 3, 2, 9, 8, 7, 6, 5, 4, 3, 2];
    let sum1: u32 = digits[..12]
        .iter()
        .zip(weights1.iter())
        .map(|(&d, &w)| d * w)
        .sum();
    let d1 = if sum1 % 11 < 2 { 0 } else { 11 - sum1 % 11 };
    if d1 != digits[12] {
        return false;
    }
    // Second check digit
    let weights2 = [6, 5, 4, 3, 2, 9, 8, 7, 6, 5, 4, 3, 2];
    let sum2: u32 = digits[..13]
        .iter()
        .zip(weights2.iter())
        .map(|(&d, &w)| d * w)
        .sum();
    let d2 = if sum2 % 11 < 2 { 0 } else { 11 - sum2 % 11 };
    d2 == digits[13]
}

pub fn patterns() -> Vec<LangPattern> {
    vec![
        LangPattern {
            pii_type: PiiType::Ssn,
            regex: Regex::new(r"\b\d{3}\.?\d{3}\.?\d{3}-?\d{2}\b").unwrap(),
            validate: Some(validate_cpf),
            label: "Brazilian CPF",
        },
        LangPattern {
            pii_type: PiiType::Ssn,
            regex: Regex::new(r"\b\d{2}\.?\d{3}\.?\d{3}/?\d{4}-?\d{2}\b").unwrap(),
            validate: Some(validate_cnpj),
            label: "Brazilian CNPJ",
        },
        LangPattern {
            pii_type: PiiType::PhoneNumber,
            regex: Regex::new(r"\+55[\s.-]?\d{2}[\s.-]?\d{4,5}[\s.-]?\d{4}\b").unwrap(),
            validate: None,
            label: "Brazilian phone (+55)",
        },
        LangPattern {
            pii_type: PiiType::PhoneNumber,
            regex: Regex::new(r"\+351[\s.-]?\d{3}[\s.-]?\d{3}[\s.-]?\d{3}\b").unwrap(),
            validate: None,
            label: "Portuguese phone (+351)",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_cpf() {
        // 529.982.247-25 is a valid CPF
        assert!(validate_cpf("52998224725"));
    }

    #[test]
    fn test_invalid_cpf_all_same() {
        assert!(!validate_cpf("11111111111"));
    }

    #[test]
    fn test_valid_cnpj() {
        // 11.222.333/0001-81 is a valid CNPJ
        assert!(validate_cnpj("11222333000181"));
    }

    #[test]
    fn test_scan_brazilian_phone() {
        let matches = super::super::scan_with_lang(
            "Me ligue no +55 11 98765-4321 por favor",
            crate::lang_detect::Language::Portuguese,
        );
        assert!(!matches.is_empty(), "Should find Brazilian phone");
    }

    #[test]
    fn test_scan_cpf() {
        let matches = super::super::scan_with_lang(
            "Meu CPF é 529.982.247-25",
            crate::lang_detect::Language::Portuguese,
        );
        assert!(!matches.is_empty(), "Should find CPF");
    }
}

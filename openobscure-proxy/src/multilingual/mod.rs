//! Multilingual PII pattern registry.
//!
//! Each language module registers patterns for country-specific PII types
//! (national IDs, tax numbers, phone formats, IBANs, etc.) and validation
//! functions (check digits, modular arithmetic, Luhn variants).
//!
//! The registry is queried by `HybridScanner` after language detection.

pub mod ar;
pub mod de;
pub mod es;
pub mod fr;
pub mod ja;
pub mod ko;
pub mod pt;
pub mod zh;

use regex::Regex;

use crate::lang_detect::Language;
use crate::pii_types::PiiType;
use crate::scanner::PiiMatch;

/// A language-specific PII pattern with optional validation.
pub struct LangPattern {
    pub pii_type: PiiType,
    pub regex: Regex,
    pub validate: Option<fn(&str) -> bool>,
    pub label: &'static str,
}

/// Determine which languages to scan based on detection result.
///
/// If detection returned a non-English language, scan that language plus any
/// closely related languages (e.g., Spanish ↔ Portuguese) since `whatlang`
/// can confuse them on short texts. Validation functions prevent false positives.
///
/// Returns empty if detection returned English or None.
pub fn languages_to_scan(detection: Option<&crate::lang_detect::DetectionResult>) -> Vec<Language> {
    let detected = match detection {
        Some(d) => d.language,
        None => return vec![],
    };
    if detected == Language::English {
        return vec![];
    }

    let mut langs = vec![detected];
    // Add confusable languages
    match detected {
        Language::Spanish => langs.push(Language::Portuguese),
        Language::Portuguese => langs.push(Language::Spanish),
        Language::French => {
            langs.push(Language::Spanish);
            langs.push(Language::Portuguese);
        }
        _ => {}
    }
    langs
}

/// Get all PII patterns for a given language.
///
/// Returns language-specific patterns only — universal patterns (credit card,
/// email, IP, etc.) are handled by the base `PiiScanner` regardless of language.
pub fn patterns_for(lang: Language) -> Vec<LangPattern> {
    match lang {
        Language::English => vec![], // English patterns are in the base scanner
        Language::Spanish => es::patterns(),
        Language::French => fr::patterns(),
        Language::German => de::patterns(),
        Language::Portuguese => pt::patterns(),
        Language::Japanese => ja::patterns(),
        Language::Chinese => zh::patterns(),
        Language::Korean => ko::patterns(),
        Language::Arabic => ar::patterns(),
    }
}

/// Scan text using language-specific patterns.
///
/// Called by `HybridScanner` after language detection. Returns matches
/// from patterns not covered by the base regex scanner.
pub fn scan_with_lang(text: &str, lang: Language) -> Vec<PiiMatch> {
    let patterns = patterns_for(lang);
    let mut matches = Vec::new();

    for pat in &patterns {
        for m in pat.regex.find_iter(text) {
            let raw = m.as_str().to_string();

            // Digit-boundary check: reject partial matches inside longer digit runs.
            // Rust regex `\b` doesn't fire between digit and CJK/Hangul/Hiragana
            // (all are Unicode \w), so we check explicitly.
            if raw.starts_with(|c: char| c.is_ascii_digit()) {
                if let Some(prev) = text[..m.start()].chars().last() {
                    if prev.is_ascii_digit() {
                        continue;
                    }
                }
            }
            if raw.ends_with(|c: char| c.is_ascii_digit()) {
                if let Some(next) = text[m.end()..].chars().next() {
                    if next.is_ascii_digit() {
                        continue;
                    }
                }
            }

            let valid = pat.validate.map_or(true, |f| f(&raw));
            if valid {
                matches.push(PiiMatch {
                    pii_type: pat.pii_type,
                    start: m.start(),
                    end: m.end(),
                    raw_value: raw,
                    json_path: None,
                    confidence: 1.0,
                });
            }
        }
    }

    // Sort by start offset, remove overlaps (keep longer match)
    matches.sort_by_key(|m| (m.start, std::cmp::Reverse(m.end)));
    dedup_overlapping(&mut matches);
    matches
}

/// Remove overlapping matches, keeping the longer one.
fn dedup_overlapping(matches: &mut Vec<PiiMatch>) {
    if matches.len() <= 1 {
        return;
    }
    let mut keep = vec![true; matches.len()];
    for i in 1..matches.len() {
        if matches[i].start < matches[i - 1].end && keep[i - 1] {
            // Overlap — keep the longer match
            if (matches[i].end - matches[i].start) > (matches[i - 1].end - matches[i - 1].start) {
                keep[i - 1] = false;
            } else {
                keep[i] = false;
            }
        }
    }
    let mut idx = 0;
    matches.retain(|_| {
        let k = keep[idx];
        idx += 1;
        k
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_english_returns_empty() {
        let patterns = patterns_for(Language::English);
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_all_languages_have_patterns() {
        for lang in Language::all() {
            if *lang == Language::English {
                continue;
            }
            let patterns = patterns_for(*lang);
            assert!(!patterns.is_empty(), "{} should have patterns", lang.code());
        }
    }

    #[test]
    fn test_scan_with_lang_no_match() {
        let matches = scan_with_lang("Hello world, no PII here", Language::Spanish);
        assert!(matches.is_empty());
    }
}

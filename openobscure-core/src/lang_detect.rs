//! Language detection for multilingual PII scanning.
//!
//! Uses `whatlang` for lightweight (~0 model files) language identification.
//! Detects the dominant language in a text block, returning a `Language` enum
//! that maps to per-language PII pattern modules.

use whatlang::{detect, Lang};

/// Supported languages for PII detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    English,
    Spanish,
    French,
    German,
    Portuguese,
    Japanese,
    Chinese,
    Korean,
    Arabic,
}

impl Language {
    /// ISO 639-1 two-letter code.
    pub fn code(&self) -> &'static str {
        match self {
            Language::English => "en",
            Language::Spanish => "es",
            Language::French => "fr",
            Language::German => "de",
            Language::Portuguese => "pt",
            Language::Japanese => "ja",
            Language::Chinese => "zh",
            Language::Korean => "ko",
            Language::Arabic => "ar",
        }
    }

    /// All supported languages.
    pub fn all() -> &'static [Language] {
        &[
            Language::English,
            Language::Spanish,
            Language::French,
            Language::German,
            Language::Portuguese,
            Language::Japanese,
            Language::Chinese,
            Language::Korean,
            Language::Arabic,
        ]
    }
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.code())
    }
}

/// Detection result with language and confidence.
#[derive(Debug, Clone)]
pub struct DetectionResult {
    pub language: Language,
    pub confidence: f64,
}

/// Detect the dominant language of a text block.
///
/// Returns `None` if:
/// - Text is too short for reliable detection
/// - Detected language is not in our supported set
/// - Confidence is below the minimum threshold (0.5)
pub fn detect_language(text: &str) -> Option<DetectionResult> {
    // whatlang needs ~20 chars for reliable detection
    if text.len() < 20 {
        return None;
    }

    let info = detect(text)?;
    let confidence = info.confidence();
    // 0.15 is intentionally permissive: PII-heavy text (SSNs, IBANs, phone numbers)
    // dilutes the language signal and pushes whatlang confidence below typical thresholds.
    // False positives from loose detection are safe because every language-specific
    // pattern has its own check-digit or structural validation (mod-97 IBAN, Luhn, etc.).
    if confidence < 0.15 {
        return None;
    }

    let language = match info.lang() {
        Lang::Eng => Language::English,
        Lang::Spa => Language::Spanish,
        Lang::Fra => Language::French,
        Lang::Deu => Language::German,
        Lang::Por => Language::Portuguese,
        Lang::Jpn => Language::Japanese,
        Lang::Cmn => Language::Chinese,
        Lang::Kor => Language::Korean,
        Lang::Ara => Language::Arabic,
        _ => return None,
    };

    Some(DetectionResult {
        language,
        confidence,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_english() {
        let result = detect_language(
            "The quick brown fox jumps over the lazy dog near the river. \
             This is a longer sentence to help with language detection accuracy and confidence.",
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap().language, Language::English);
    }

    #[test]
    fn test_detect_spanish() {
        let result = detect_language(
            "El rápido zorro marrón salta sobre el perro perezoso. \
             Esta es una oración más larga para ayudar con la detección del idioma.",
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap().language, Language::Spanish);
    }

    #[test]
    fn test_detect_french() {
        let result = detect_language(
            "Le renard brun rapide saute par-dessus le chien paresseux. \
             Ceci est une phrase plus longue pour aider à la détection de la langue.",
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap().language, Language::French);
    }

    #[test]
    fn test_detect_german() {
        let result = detect_language(
            "Der schnelle braune Fuchs springt über den faulen Hund in der Nähe des Flusses. \
             Dies ist ein längerer Satz um die Spracherkennung zu verbessern.",
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap().language, Language::German);
    }

    #[test]
    fn test_detect_portuguese() {
        let result = detect_language(
            "A rápida raposa marrom pula sobre o cachorro preguiçoso perto do rio. \
             Esta é uma frase mais longa para ajudar na detecção do idioma.",
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap().language, Language::Portuguese);
    }

    #[test]
    fn test_detect_japanese() {
        let result = detect_language("速い茶色のキツネが怠惰な犬の上を飛び越えます");
        assert!(result.is_some());
        assert_eq!(result.unwrap().language, Language::Japanese);
    }

    #[test]
    fn test_detect_chinese() {
        let result = detect_language("快速的棕色狐狸跳过了懒惰的狗在河边");
        assert!(result.is_some());
        assert_eq!(result.unwrap().language, Language::Chinese);
    }

    #[test]
    fn test_detect_korean() {
        let result = detect_language("빠른 갈색 여우가 게으른 개를 뛰어넘습니다");
        assert!(result.is_some());
        assert_eq!(result.unwrap().language, Language::Korean);
    }

    #[test]
    fn test_detect_arabic() {
        let result = detect_language("الثعلب البني السريع يقفز فوق الكلب الكسول");
        assert!(result.is_some());
        assert_eq!(result.unwrap().language, Language::Arabic);
    }

    #[test]
    fn test_short_text_returns_none() {
        assert!(detect_language("hi").is_none());
    }

    #[test]
    fn test_language_code() {
        assert_eq!(Language::English.code(), "en");
        assert_eq!(Language::Spanish.code(), "es");
        assert_eq!(Language::Japanese.code(), "ja");
    }

    #[test]
    fn test_all_languages() {
        assert_eq!(Language::all().len(), 9);
    }

    #[test]
    fn test_detect_texts_with_embedded_digits() {
        // Texts with phone numbers/IDs have low whatlang confidence due to digits.
        // Threshold 0.15 should still detect the correct language.
        let texts = [
            (
                Language::Spanish,
                "Llámame al teléfono +34 612 345 678 para confirmar la reserva en Barcelona.",
            ),
            (
                Language::French,
                "Appelez-moi au numéro 06 12 34 56 78 si vous avez des questions.",
            ),
            (
                Language::Portuguese,
                "Por favor, ligue para +55 11 91234-5678 para agendar a consulta.",
            ),
            (
                Language::Chinese,
                "我的身份证号码是11010519491231002X，请核实个人信息是否正确。",
            ),
        ];
        for (expected_lang, text) in texts {
            let result = detect_language(text);
            assert!(result.is_some(), "Should detect language for: {:.50}", text);
            assert_eq!(
                result.unwrap().language,
                expected_lang,
                "Wrong language for: {:.50}",
                text
            );
        }
    }
}

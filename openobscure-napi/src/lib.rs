//! OpenObscure NAPI — Native Node.js addon for PII scanning.
//!
//! Wraps the Rust HybridScanner (regex + keywords + NER TinyBERT + ensemble
//! voting) as a Node.js class via napi-rs, enabling in-process PII detection
//! from TypeScript without requiring a running proxy.
//!
//! Usage from JavaScript:
//! ```js
//! const { OpenObscureScanner } = require('@openobscure/scanner-napi');
//!
//! // Regex + keywords only (no models needed)
//! const scanner = new OpenObscureScanner();
//! const result = scanner.scanText('My SSN is 123-45-6789');
//! // → { matches: [{ start: 10, end: 21, piiType: 'ssn', ... }], timingUs: 42 }
//!
//! // With NER (requires model files)
//! const scannerNer = new OpenObscureScanner('path/to/models/ner');
//! const result2 = scannerNer.scanText('Call John Smith at 555-0123');
//! // → { matches: [person("John Smith"), phone("555-0123")], ... }
//! ```

use std::path::Path;
use std::sync::Mutex;

use napi_derive::napi;

use openobscure_core::hybrid_scanner::HybridScanner;
use openobscure_core::ner_scanner::{NerPool, NerScanner};
use openobscure_core::persuasion_dict::PersuasionDict;

/// Minimum per-token confidence for a NER span to be reported.
/// 0.5 matches the L0 proxy default so that the NAPI addon and the gateway
/// produce identical results when both are available.
const NER_CONFIDENCE_THRESHOLD: f32 = 0.5;

/// A single PII match returned by `scanText()`.
#[napi(object)]
pub struct ScanMatch {
    /// Byte offset start in the input string.
    pub start: u32,
    /// Byte offset end in the input string.
    pub end: u32,
    /// PII type identifier (e.g. "ssn", "email", "person", "location").
    pub pii_type: String,
    /// Detection confidence (0.0–1.0). Regex/keyword = 1.0, NER = model score.
    pub confidence: f64,
    /// The raw matched text.
    pub raw_value: String,
}

/// Result of a `scanText()` call.
#[napi(object)]
pub struct ScanResult {
    /// All PII matches found.
    pub matches: Vec<ScanMatch>,
    /// Total scan time in microseconds.
    pub timing_us: u32,
}

/// Native PII scanner wrapping the Rust HybridScanner.
///
/// Supports two modes:
/// - **Regex + keywords** (no model dir): detects 9 structured PII types
/// - **Full NER** (with model dir): adds person, location, organization,
///   health, child detection (14 total types)
#[napi]
pub struct OpenObscureScanner {
    inner: Mutex<HybridScanner>,
    ner_loaded: bool,
}

#[napi]
impl OpenObscureScanner {
    /// Create a new scanner.
    ///
    /// @param nerModelDir - Optional path to NER model directory containing
    ///   `model_int8.onnx`, `vocab.txt`, and `label_map.json`. If omitted or
    ///   models not found, falls back to regex + keywords only.
    #[napi(constructor)]
    pub fn new(ner_model_dir: Option<String>) -> napi::Result<Self> {
        let ner_scanner = if let Some(ref dir) = ner_model_dir {
            let path = Path::new(dir);
            if path.join("model_int8.onnx").exists() || path.join("model.onnx").exists() {
                match NerScanner::load(path, NER_CONFIDENCE_THRESHOLD) {
                    Ok(ner) => Some(ner),
                    Err(e) => {
                        // Log warning but don't fail — fall back to regex
                        eprintln!(
                            "Warning: NER model load failed (falling back to regex): {}",
                            e
                        );
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        let ner_loaded = ner_scanner.is_some();
        let ner_pool = ner_scanner.map(|ner| NerPool::new(vec![ner]));
        let scanner = HybridScanner::new(true, ner_pool, None);

        Ok(Self {
            inner: Mutex::new(scanner),
            ner_loaded,
        })
    }

    /// Scan text for PII and return all matches with timing.
    ///
    /// @param text - The text to scan for PII.
    /// @returns ScanResult with matches and timing.
    #[napi]
    pub fn scan_text(&self, text: String) -> napi::Result<ScanResult> {
        let scanner = self
            .inner
            .lock()
            .map_err(|e| napi::Error::from_reason(format!("Scanner lock poisoned: {}", e)))?;

        let (matches, timing) = scanner.scan_text_with_timing(&text);

        let scan_matches = matches
            .into_iter()
            .map(|m| ScanMatch {
                start: m.start as u32,
                end: m.end as u32,
                pii_type: m.pii_type.to_string(),
                confidence: m.confidence as f64,
                raw_value: m.raw_value,
            })
            .collect();

        Ok(ScanResult {
            matches: scan_matches,
            timing_us: timing.total_us as u32,
        })
    }

    /// Check whether the NER model is loaded.
    #[napi]
    pub fn has_ner(&self) -> bool {
        self.ner_loaded
    }
}

// ── Persuasion Scanner ───────────────────────────────────────────────

/// A single persuasion phrase match.
#[napi(object)]
pub struct PersuasionMatchJs {
    pub category: String,
    pub start: u32,
    pub end: u32,
    pub phrase: String,
}

/// Result of a persuasion scan.
#[napi(object)]
pub struct PersuasionScanResultJs {
    pub matches: Vec<PersuasionMatchJs>,
    pub timing_us: u32,
}

/// Scan text for persuasion/manipulation phrases using the Rust dictionary.
///
/// Returns all matches with category, offsets, and timing.
/// This is the R1 dictionary layer only (no R2 model inference).
#[napi]
pub fn scan_persuasion(text: String) -> PersuasionScanResultJs {
    scan_persuasion_inner(&text)
}

/// Inner implementation (testable without NAPI runtime).
fn scan_persuasion_inner(text: &str) -> PersuasionScanResultJs {
    let dict = PersuasionDict::new();
    let start = std::time::Instant::now();
    let matches = dict.scan_text(text);
    let timing_us = start.elapsed().as_micros() as u32;

    PersuasionScanResultJs {
        matches: matches
            .into_iter()
            .map(|m| PersuasionMatchJs {
                category: m.category.to_string(),
                start: m.start as u32,
                end: m.end as u32,
                phrase: m.phrase,
            })
            .collect(),
        timing_us,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_persuasion_detects_urgency() {
        let result = scan_persuasion_inner("Act now before it's too late!");
        assert!(
            result.matches.iter().any(|m| m.category == "Urgency"),
            "Should detect urgency, got: {:?}",
            result.matches.iter().map(|m| &m.category).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_scan_persuasion_clean_text() {
        let result = scan_persuasion_inner("The function returns a sorted list of integers.");
        assert!(
            result.matches.is_empty(),
            "Clean text should have no matches, got {}",
            result.matches.len()
        );
    }

    #[test]
    fn test_scan_persuasion_multiple_categories() {
        let result = scan_persuasion_inner(
            "Act now! Experts agree this exclusive offer is a smart choice.",
        );
        let categories: std::collections::HashSet<&str> =
            result.matches.iter().map(|m| m.category.as_str()).collect();
        assert!(
            categories.len() >= 3,
            "Expected >= 3 categories, got {:?}",
            categories
        );
    }

    #[test]
    fn test_scan_persuasion_offsets_valid() {
        let text = "Buy now and save money today!";
        let result = scan_persuasion_inner(text);
        for m in &result.matches {
            let slice = &text[m.start as usize..m.end as usize];
            assert_eq!(
                slice.to_lowercase(),
                m.phrase.to_lowercase(),
                "Offset [{},{}) should match phrase '{}'",
                m.start,
                m.end,
                m.phrase
            );
        }
    }

    #[test]
    fn test_scan_persuasion_timing_populated() {
        let result = scan_persuasion_inner("Act now!");
        // timing_us can be 0 on very fast machines, but shouldn't be wildly large
        assert!(result.timing_us < 1_000_000, "Timing should be < 1s");
    }

    #[test]
    fn test_scan_persuasion_empty_string() {
        let result = scan_persuasion_inner("");
        assert!(result.matches.is_empty());
    }
}

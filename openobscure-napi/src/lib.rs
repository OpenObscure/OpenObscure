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

use openobscure_proxy::hybrid_scanner::HybridScanner;
use openobscure_proxy::ner_scanner::NerScanner;

/// Default NER confidence threshold (matches proxy default).
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
        let scanner = HybridScanner::new(true, ner_scanner);

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

//! Mobile-facing API for OpenObscure.
//!
//! Provides a self-contained PII sanitization library that can be called from
//! Swift (iOS) or Kotlin (Android) via UniFFI-generated bindings. No HTTP server,
//! no sockets — just direct function calls.
//!
//! # Architecture
//!
//! On mobile, the host app (e.g. OpenClaw iOS/Android companion) passes text and
//! images through OpenObscure before sending them to the Gateway over WebSocket.
//! The FPE key is provided by the host app's native secure storage (iOS Keychain
//! or Android Keystore).

use crate::config::ImageConfig;
use crate::fpe_engine::{FpeEngine, FpeError, TweakGenerator};
use crate::hash_token::TokenGenerator;
use crate::hybrid_scanner::HybridScanner;
use crate::image_pipeline::ImageModelManager;
use crate::response_integrity::{ResponseIntegrityScanner, Sensitivity};

/// Configuration for the mobile library.
///
/// Deserialized from JSON passed by the host app. This avoids needing TOML
/// parsing or file system access on mobile.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct MobileConfig {
    /// Enable keyword dictionary for health/child term detection.
    #[serde(default = "default_true")]
    pub keywords_enabled: bool,

    /// Scanner mode: "regex", "crf", "ner", or "auto" (default).
    /// In "auto" mode with auto_detect enabled, the device profiler selects
    /// the best scanner the hardware can support.
    #[serde(default = "default_scanner_mode")]
    pub scanner_mode: String,

    /// Enable automatic hardware detection to select scanner features.
    /// When true (default), the device profiler determines which scanners
    /// to enable based on available hardware. When false, uses scanner_mode
    /// literally without hardware-based upgrades.
    #[serde(default = "default_true")]
    pub auto_detect: bool,

    /// Path to CRF model directory (if using CRF scanner).
    #[serde(default)]
    pub crf_model_dir: Option<String>,

    /// Path to NER model directory (model_int8.onnx + vocab.txt).
    /// Used for Full/Standard tiers (DistilBERT).
    #[serde(default)]
    pub ner_model_dir: Option<String>,

    /// Path to Lite-tier NER model directory (TinyBERT INT8).
    /// Falls back to `ner_model_dir` if not set.
    #[serde(default)]
    pub ner_model_dir_lite: Option<String>,

    /// Enable image processing pipeline.
    #[serde(default)]
    pub image_enabled: bool,

    /// Path to BlazeFace model directory (Lite tier).
    #[serde(default)]
    pub face_model_dir: Option<String>,

    /// Path to SCRFD model directory (Full/Standard tier).
    #[serde(default)]
    pub scrfd_model_dir: Option<String>,

    /// Path to OCR model directory.
    #[serde(default)]
    pub ocr_model_dir: Option<String>,

    /// Path to NSFW/NudeNet model directory.
    #[serde(default)]
    pub nsfw_model_dir: Option<String>,

    /// Maximum image dimension before resize.
    #[serde(default = "default_max_dimension")]
    pub max_dimension: u32,

    /// Enable response integrity (cognitive firewall) scanning.
    #[serde(default)]
    pub ri_enabled: bool,

    /// Response integrity sensitivity: "off", "low", "medium" (default), "high".
    #[serde(default = "default_ri_sensitivity")]
    pub ri_sensitivity: String,

    /// Path to R2 model directory (model_int8.onnx + vocab.txt).
    /// Optional — R1 dictionary scan works without any model files.
    #[serde(default)]
    pub ri_model_dir: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_scanner_mode() -> String {
    "auto".to_string()
}

fn default_max_dimension() -> u32 {
    960
}

fn default_ri_sensitivity() -> String {
    "medium".to_string()
}

impl Default for MobileConfig {
    fn default() -> Self {
        Self {
            keywords_enabled: true,
            scanner_mode: "auto".to_string(),
            auto_detect: true,
            crf_model_dir: None,
            ner_model_dir: None,
            ner_model_dir_lite: None,
            image_enabled: false,
            face_model_dir: None,
            scrfd_model_dir: None,
            ocr_model_dir: None,
            nsfw_model_dir: None,
            max_dimension: 960,
            ri_enabled: false,
            ri_sensitivity: "medium".to_string(),
            ri_model_dir: None,
        }
    }
}

/// Result of sanitizing text for PII.
#[derive(Debug, Clone)]
pub struct SanitizeResult {
    /// Text with PII replaced by FPE-encrypted or redacted values.
    pub sanitized_text: String,
    /// Number of PII matches found and processed.
    pub pii_count: u32,
    /// Categories of PII found (e.g. "credit_card", "email", "person").
    pub categories: Vec<String>,
    /// Opaque mapping data needed to restore original values in responses.
    /// Pass this back to `restore_text()`.
    pub mapping_json: String,
}

/// Statistics from a sanitize operation.
#[derive(Debug, Clone)]
pub struct MobileStats {
    /// Total PII matches found across all calls.
    pub total_pii_found: u64,
    /// Total images processed.
    pub total_images_processed: u64,
    /// Scanner mode in use ("regex", "crf", "ner").
    pub scanner_mode: String,
    /// Whether image pipeline is available.
    pub image_pipeline_available: bool,
    /// Device capability tier ("full", "standard", "lite", or "manual").
    pub device_tier: String,
}

/// Result of scanning a response for persuasion/manipulation (cognitive firewall).
#[derive(Debug, Clone)]
pub struct MobileRiReport {
    /// Severity tier: "Notice", "Warning", or "Caution".
    pub severity: String,
    /// Persuasion categories detected by R1 dictionary (e.g. "Urgency", "Fear").
    pub categories: Vec<String>,
    /// Matched phrases from R1 dictionary scan.
    pub flags: Vec<String>,
    /// Article 5 categories detected by R2 classifier (if model loaded).
    pub r2_categories: Vec<String>,
    /// Scan duration in microseconds.
    pub scan_time_us: u64,
}

/// Errors from mobile library operations.
#[derive(Debug, thiserror::Error)]
pub enum MobileError {
    #[error("Invalid FPE key: expected 64 hex characters (32 bytes)")]
    InvalidKey,
    #[error("FPE engine error: {0}")]
    Fpe(#[from] FpeError),
    #[error("Invalid config JSON: {0}")]
    InvalidConfig(String),
    #[error("Image processing error: {0}")]
    ImageError(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
}

/// The main mobile API handle. Thread-safe and reusable across calls.
pub struct OpenObscureMobile {
    scanner: HybridScanner,
    fpe: FpeEngine,
    image_manager: Option<ImageModelManager>,
    ri_scanner: Option<ResponseIntegrityScanner>,
    stats: std::sync::Mutex<InternalStats>,
}

struct InternalStats {
    total_pii_found: u64,
    total_images_processed: u64,
    scanner_mode: String,
    device_tier: String,
}

impl OpenObscureMobile {
    /// Create a new mobile OpenObscure instance.
    ///
    /// # Arguments
    /// * `config` - Mobile configuration
    /// * `fpe_key` - 32-byte AES-256 key for Format-Preserving Encryption
    ///
    /// When `auto_detect` is true (default) and `scanner_mode` is "auto",
    /// the device profiler detects hardware capabilities and enables the
    /// best scanner the device can support (NER on 8GB+ devices, CRF on
    /// 4-8GB, regex-only on <4GB).
    pub fn new(config: MobileConfig, fpe_key: [u8; 32]) -> Result<Self, MobileError> {
        let fpe = FpeEngine::new(&fpe_key)?;

        // Determine scanner and tier via auto-detection or explicit mode
        let (scanner, effective_mode, device_tier, budget) = if config.auto_detect
            && config.scanner_mode == "auto"
        {
            let profile = crate::device_profile::detect(true);
            let tier = crate::device_profile::tier_for_profile(&profile);
            let budget = crate::device_profile::budget_for_tier(tier, &profile);

            let (mut scan, mode) = Self::build_scanner_from_budget(&config, &budget);
            let effective_bonus = if budget.ensemble_enabled { 0.15 } else { 0.0 };
            scan.set_confidence_params(0.5, effective_bonus);
            (scan, mode, tier.to_string(), Some(budget))
        } else {
            // Explicit mode — honor scanner_mode literally, but still use
            // device tier for NER model selection (TinyBERT vs DistilBERT).
            let profile = crate::device_profile::detect(true);
            let tier = crate::device_profile::tier_for_profile(&profile);
            let budget = crate::device_profile::budget_for_tier(tier, &profile);

            let scanner = match config.scanner_mode.as_str() {
                "crf" => {
                    if let Some(ref dir) = config.crf_model_dir {
                        match crate::crf_scanner::CrfScanner::load(std::path::Path::new(dir), 0.5) {
                            Ok(crf) => {
                                HybridScanner::with_crf(config.keywords_enabled, Some(crf), None)
                            }
                            Err(_) => HybridScanner::new(config.keywords_enabled, None, None),
                        }
                    } else {
                        HybridScanner::new(config.keywords_enabled, None, None)
                    }
                }
                "ner" => {
                    // Select model dir based on device tier
                    let model_dir = match budget.ner_model.as_str() {
                        "tinybert" => config
                            .ner_model_dir_lite
                            .as_ref()
                            .or(config.ner_model_dir.as_ref()),
                        _ => config.ner_model_dir.as_ref(),
                    };
                    if let Some(dir) = model_dir {
                        match crate::ner_scanner::NerScanner::load(std::path::Path::new(dir), 0.60)
                        {
                            Ok(ner) => {
                                let pool = crate::ner_scanner::NerPool::new(vec![ner]);
                                HybridScanner::new(config.keywords_enabled, Some(pool), None)
                            }
                            Err(_) => HybridScanner::new(config.keywords_enabled, None, None),
                        }
                    } else {
                        HybridScanner::new(config.keywords_enabled, None, None)
                    }
                }
                _ => {
                    // "regex" or unknown — regex+keywords only
                    HybridScanner::new(config.keywords_enabled, None, None)
                }
            };
            (
                scanner,
                config.scanner_mode.clone(),
                tier.to_string(),
                Some(budget),
            )
        };

        // Build image pipeline if enabled and budget allows
        let image_pipeline_allowed = budget
            .as_ref()
            .map(|b| b.image_pipeline_enabled)
            .unwrap_or(true); // manual mode: no budget gating
        let image_manager = if config.image_enabled && image_pipeline_allowed {
            let idle_timeout = budget
                .as_ref()
                .map(|b| b.model_idle_timeout_secs)
                .unwrap_or(300);
            let ocr_tier = budget
                .as_ref()
                .map(|b| b.ocr_tier.clone())
                .unwrap_or_else(|| "detect_and_fill".to_string());
            let screen_guard = budget
                .as_ref()
                .map(|b| b.screen_guard_enabled)
                .unwrap_or(false);
            let nsfw_enabled = budget.as_ref().map(|b| b.nsfw_enabled).unwrap_or(false);
            let face_model_name = budget
                .as_ref()
                .map(|b| b.face_model.clone())
                .unwrap_or_else(|| "blazeface".to_string());
            let img_config = ImageConfig {
                enabled: true,
                face_detection: config.face_model_dir.is_some() || config.scrfd_model_dir.is_some(),
                ocr_enabled: config.ocr_model_dir.is_some(),
                ocr_tier,
                max_dimension: config.max_dimension,
                model_idle_timeout_secs: idle_timeout,
                face_model: face_model_name,
                face_model_dir: config.face_model_dir,
                face_model_dir_scrfd: config.scrfd_model_dir,
                face_model_dir_ultralight: None,
                ocr_model_dir: config.ocr_model_dir,
                screen_guard,
                exif_strip: true,
                nsfw_detection: nsfw_enabled,
                nsfw_model_dir: if nsfw_enabled {
                    config.nsfw_model_dir
                } else {
                    None
                },
                nsfw_threshold: 0.45,
                nsfw_classifier_enabled: false, // Classifier not yet bundled for mobile
                nsfw_classifier_model_dir: None,
                nsfw_classifier_threshold: 0.75,
                url_fetch_enabled: false, // Mobile doesn't fetch URLs
                url_max_bytes: 0,
                url_timeout_secs: 0,
                url_allow_localhost_http: false,
            };
            Some(ImageModelManager::new(img_config))
        } else {
            None
        };

        // Build response integrity scanner (cognitive firewall) if enabled and budget allows
        let ri_allowed = budget.as_ref().map(|b| b.ri_enabled).unwrap_or(true);
        let ri_scanner = if config.ri_enabled && ri_allowed {
            let sensitivity: Sensitivity =
                config.ri_sensitivity.parse().unwrap_or(Sensitivity::Medium);
            let scanner = ResponseIntegrityScanner::new(sensitivity);
            // Optionally load R2 model for deeper classification
            if let Some(ref dir) = config.ri_model_dir {
                let _ = scanner.load_r2(std::path::Path::new(dir), 0.5, 0.9);
            }
            Some(scanner)
        } else {
            None
        };

        Ok(Self {
            scanner,
            fpe,
            image_manager,
            ri_scanner,
            stats: std::sync::Mutex::new(InternalStats {
                total_pii_found: 0,
                total_images_processed: 0,
                scanner_mode: effective_mode,
                device_tier,
            }),
        })
    }

    /// Build a scanner based on the device profiler's feature budget.
    fn build_scanner_from_budget(
        config: &MobileConfig,
        budget: &crate::device_profile::FeatureBudget,
    ) -> (HybridScanner, String) {
        if budget.ner_enabled {
            // Select model dir based on budget tier: tinybert uses lite path
            let model_dir = match budget.ner_model.as_str() {
                "tinybert" => config
                    .ner_model_dir_lite
                    .as_ref()
                    .or(config.ner_model_dir.as_ref()),
                _ => config.ner_model_dir.as_ref(),
            };
            if let Some(dir) = model_dir {
                if let Ok(ner) =
                    crate::ner_scanner::NerScanner::load(std::path::Path::new(dir), 0.60)
                {
                    let pool = crate::ner_scanner::NerPool::new(vec![ner]);
                    return (
                        HybridScanner::new(config.keywords_enabled, Some(pool), None),
                        "ner".to_string(),
                    );
                }
            }
        }
        if budget.crf_enabled {
            if let Some(ref dir) = config.crf_model_dir {
                if let Ok(crf) =
                    crate::crf_scanner::CrfScanner::load(std::path::Path::new(dir), 0.5)
                {
                    return (
                        HybridScanner::with_crf(config.keywords_enabled, Some(crf), None),
                        "crf".to_string(),
                    );
                }
            }
        }
        (
            HybridScanner::new(config.keywords_enabled, None, None),
            "regex".to_string(),
        )
    }

    /// Scan text for PII and encrypt matches with FF1 FPE.
    ///
    /// Returns a `SanitizeResult` containing the sanitized text and metadata.
    /// The `mapping_json` field should be saved and passed to `restore_text()`
    /// when processing the corresponding response.
    pub fn sanitize_text(&self, text: &str) -> Result<SanitizeResult, MobileError> {
        let matches = self.scanner.scan_text(text);

        if matches.is_empty() {
            return Ok(SanitizeResult {
                sanitized_text: text.to_string(),
                pii_count: 0,
                categories: Vec::new(),
                mapping_json: "{}".to_string(),
            });
        }

        // Collect unique categories
        let mut categories: Vec<String> = matches
            .iter()
            .map(|m| m.pii_type.to_string())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        categories.sort();

        let pii_count = matches.len() as u32;
        let request_id = uuid::Uuid::new_v4();

        // Build replacements sorted by position (reverse order for safe replacement)
        let mut replacements: Vec<(usize, usize, String)> = Vec::new();
        let mut mapping_data: Vec<(String, String)> = Vec::new();
        let mut token_gen = TokenGenerator::new(request_id);

        for m in &matches {
            if m.pii_type.is_fpe_eligible() {
                let tweak = TweakGenerator::generate(&request_id, "mobile");
                match self.fpe.encrypt_match(m, &tweak) {
                    Ok(result) => {
                        mapping_data.push((result.encrypted.clone(), m.raw_value.clone()));
                        replacements.push((m.start, m.end, result.encrypted));
                    }
                    Err(_) => {
                        // FPE failed (e.g. domain too small) — fall back to hash token
                        let token = token_gen.generate(m.pii_type, &m.raw_value);
                        mapping_data.push((token.clone(), m.raw_value.clone()));
                        replacements.push((m.start, m.end, token));
                    }
                }
            } else {
                // Non-FPE types get hash-based token (e.g., PER_a7f2)
                let token = token_gen.generate(m.pii_type, &m.raw_value);
                mapping_data.push((token.clone(), m.raw_value.clone()));
                replacements.push((m.start, m.end, token));
            }
        }

        // Apply replacements in reverse order to preserve byte offsets
        let mut result = text.to_string();
        replacements.sort_by(|a, b| b.0.cmp(&a.0));
        for (start, end, replacement) in &replacements {
            if *start <= result.len() && *end <= result.len() {
                result.replace_range(*start..*end, replacement);
            }
        }

        // Serialize mappings for later restore
        let mapping_json = serde_json::to_string(&mapping_data)
            .map_err(|e| MobileError::Serialization(e.to_string()))?;

        // Update stats
        if let Ok(mut stats) = self.stats.lock() {
            stats.total_pii_found += pii_count as u64;
        }

        Ok(SanitizeResult {
            sanitized_text: result,
            pii_count,
            categories,
            mapping_json,
        })
    }

    /// Restore original PII values in a response text using saved mappings.
    ///
    /// The `mapping_json` should be the value from a previous `sanitize_text()` call.
    pub fn restore_text(&self, text: &str, mapping_json: &str) -> String {
        let mappings: Vec<(String, String)> = match serde_json::from_str(mapping_json) {
            Ok(m) => m,
            Err(_) => return text.to_string(),
        };

        let mut result = text.to_string();
        // Sort by ciphertext length descending to avoid partial matches
        let mut sorted_mappings = mappings;
        sorted_mappings.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        for (ciphertext, plaintext) in &sorted_mappings {
            result = result.replace(ciphertext, plaintext);
        }
        result
    }

    /// Process an image for visual PII (face redaction, OCR text redaction, EXIF strip).
    ///
    /// Returns the sanitized image bytes in the same format as input (JPEG/PNG).
    pub fn sanitize_image(&self, image_bytes: &[u8]) -> Result<Vec<u8>, MobileError> {
        let manager = self
            .image_manager
            .as_ref()
            .ok_or_else(|| MobileError::ImageError("Image pipeline not enabled".to_string()))?;

        let img = crate::image_pipeline::decode_image(image_bytes)
            .map_err(|e| MobileError::ImageError(e.to_string()))?;

        let max_dim = manager.config().max_dimension;
        let img = crate::image_pipeline::resize_if_needed(img, max_dim);

        let (result_img, _stats, _meta) = manager
            .process_image(img, None, None)
            .map_err(|e| MobileError::ImageError(e.to_string()))?;

        // Update stats
        if let Ok(mut s) = self.stats.lock() {
            s.total_images_processed += 1;
        }

        // Encode back to JPEG
        let mut buf = std::io::Cursor::new(Vec::new());
        result_img
            .write_to(&mut buf, image::ImageFormat::Jpeg)
            .map_err(|e| MobileError::ImageError(e.to_string()))?;

        Ok(buf.into_inner())
    }

    /// Get current statistics for diagnostics.
    pub fn stats(&self) -> MobileStats {
        let s = self.stats.lock().unwrap();
        MobileStats {
            total_pii_found: s.total_pii_found,
            total_images_processed: s.total_images_processed,
            scanner_mode: s.scanner_mode.clone(),
            image_pipeline_available: self.image_manager.is_some(),
            device_tier: s.device_tier.clone(),
        }
    }

    /// Scan a transcript (from platform speech API) for PII.
    ///
    /// Mobile apps transcribe audio using iOS `SFSpeechRecognizer` or Android
    /// `SpeechRecognizer`, then pass the transcript here for PII detection.
    /// Returns a `SanitizeResult` with the sanitized transcript and mappings.
    pub fn sanitize_audio_transcript(
        &self,
        transcript: &str,
    ) -> Result<SanitizeResult, MobileError> {
        self.sanitize_text(transcript)
    }

    /// Check if a transcript contains PII without encrypting.
    ///
    /// Returns the number of PII matches found. Useful for deciding whether
    /// to strip an entire audio block vs. pass it through.
    pub fn check_audio_pii(&self, transcript: &str) -> u32 {
        self.scanner.scan_text(transcript).len() as u32
    }

    /// Scan a response for persuasion and manipulation techniques (cognitive firewall).
    ///
    /// Returns `Some(MobileRiReport)` if manipulation is detected, `None` if clean.
    /// The cognitive firewall uses a two-tier cascade:
    /// - R1: Dictionary scan (~250 phrases, <1ms) — always available
    /// - R2: TinyBERT classifier (~30ms) — requires `ri_model_dir` config
    pub fn scan_response(&self, text: &str) -> Option<MobileRiReport> {
        let scanner = self.ri_scanner.as_ref()?;
        let report = scanner.scan(text)?;
        Some(MobileRiReport {
            severity: report.severity.to_string(),
            categories: report.categories.iter().map(|c| c.to_string()).collect(),
            flags: report.flags.iter().map(|f| f.phrase.clone()).collect(),
            r2_categories: report.r2_categories,
            scan_time_us: report.scan_time_us,
        })
    }

    /// Whether the cognitive firewall is enabled and available.
    pub fn ri_available(&self) -> bool {
        self.ri_scanner.is_some()
    }

    /// Release all loaded image models immediately.
    ///
    /// Call this from iOS `applicationDidReceiveMemoryWarning` or
    /// Android `ComponentCallbacks2.onTrimMemory(TRIM_MEMORY_RUNNING_LOW)`.
    /// Models will be reloaded on-demand when the next image is processed.
    pub fn release_models(&self) {
        if let Some(ref manager) = self.image_manager {
            manager.force_evict();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_key() -> [u8; 32] {
        [0x42u8; 32]
    }

    #[test]
    fn test_mobile_sanitize_no_pii() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let result = mobile.sanitize_text("Hello world, no PII here").unwrap();
        assert_eq!(result.pii_count, 0);
        assert_eq!(result.sanitized_text, "Hello world, no PII here");
        assert!(result.categories.is_empty());
    }

    #[test]
    fn test_mobile_sanitize_credit_card() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let result = mobile
            .sanitize_text("My card is 4111-1111-1111-1111")
            .unwrap();
        assert!(result.pii_count >= 1);
        assert!(!result.sanitized_text.contains("4111-1111-1111-1111"));
        assert!(result.categories.contains(&"credit_card".to_string()));
    }

    #[test]
    fn test_mobile_sanitize_email() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let result = mobile
            .sanitize_text("Contact me at johnathan.doe@example.com please")
            .unwrap();
        assert!(result.pii_count >= 1);
        assert!(!result.sanitized_text.contains("johnathan.doe@example.com"));
    }

    #[test]
    fn test_mobile_sanitize_ssn() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let result = mobile.sanitize_text("SSN: 123-45-6789").unwrap();
        assert!(result.pii_count >= 1);
        assert!(!result.sanitized_text.contains("123-45-6789"));
    }

    #[test]
    fn test_mobile_restore_text() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let sanitized = mobile
            .sanitize_text("My card is 4111-1111-1111-1111")
            .unwrap();
        assert!(sanitized.pii_count >= 1);

        // Simulate a response that echoes back the full sanitized text
        let response = sanitized.sanitized_text.clone();
        let restored = mobile.restore_text(&response, &sanitized.mapping_json);
        // Should restore back to original credit card number
        assert!(
            restored.contains("4111-1111-1111-1111"),
            "Expected restored text to contain original card number, got: {}",
            restored
        );
    }

    #[test]
    fn test_mobile_stats() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let _ = mobile.sanitize_text("Card: 4111-1111-1111-1111");
        let stats = mobile.stats();
        assert!(stats.total_pii_found >= 1);
        assert!(!stats.image_pipeline_available);
    }

    #[test]
    fn test_mobile_config_from_json() {
        let json = r#"{"keywords_enabled": true, "scanner_mode": "regex"}"#;
        let config: MobileConfig = serde_json::from_str(json).unwrap();
        assert!(config.keywords_enabled);
        assert_eq!(config.scanner_mode, "regex");
    }

    #[test]
    fn test_mobile_default_config() {
        let config = MobileConfig::default();
        assert!(config.keywords_enabled);
        assert_eq!(config.scanner_mode, "auto");
        assert!(config.auto_detect);
        assert!(config.ner_model_dir.is_none());
        assert!(!config.image_enabled);
        assert_eq!(config.max_dimension, 960);
    }

    #[test]
    fn test_mobile_invalid_key() {
        // Key must be 32 bytes — test that engine creation works
        let result = OpenObscureMobile::new(MobileConfig::default(), [0u8; 32]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_mobile_image_not_enabled() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let result = mobile.sanitize_image(&[0xFF, 0xD8, 0xFF]); // JPEG header
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not enabled"));
    }

    #[test]
    fn test_mobile_restore_empty_mapping() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let result = mobile.restore_text("Hello world", "{}");
        assert_eq!(result, "Hello world");
    }

    #[test]
    fn test_mobile_restore_invalid_json() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let result = mobile.restore_text("Hello world", "not json");
        assert_eq!(result, "Hello world");
    }

    #[test]
    fn test_mobile_multiple_pii() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let result = mobile
            .sanitize_text("Card 4111-1111-1111-1111 and SSN 123-45-6789")
            .unwrap();
        assert!(result.pii_count >= 2);
        assert!(!result.sanitized_text.contains("4111"));
        assert!(!result.sanitized_text.contains("123-45-6789"));
    }

    #[test]
    fn test_mobile_auto_detect_default_true() {
        let config = MobileConfig::default();
        assert!(config.auto_detect);
    }

    #[test]
    fn test_mobile_auto_detect_false_stays_regex() {
        let config = MobileConfig {
            auto_detect: false,
            scanner_mode: "auto".to_string(),
            ..MobileConfig::default()
        };
        let mobile = OpenObscureMobile::new(config, make_test_key()).unwrap();
        let stats = mobile.stats();
        // With auto_detect=false and scanner_mode="auto", falls to the `_` branch → regex
        assert_eq!(stats.scanner_mode, "auto");
        // Device tier is always detected now (for model selection)
        assert!(
            stats.device_tier == "full"
                || stats.device_tier == "standard"
                || stats.device_tier == "lite"
        );
    }

    #[test]
    fn test_mobile_config_deserialize_auto_detect() {
        let json = r#"{"auto_detect": false, "scanner_mode": "regex"}"#;
        let config: MobileConfig = serde_json::from_str(json).unwrap();
        assert!(!config.auto_detect);
        assert_eq!(config.scanner_mode, "regex");
    }

    #[test]
    fn test_mobile_config_deserialize_ner_model_dir() {
        let json = r#"{"ner_model_dir": "/models/ner"}"#;
        let config: MobileConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.ner_model_dir.unwrap(), "/models/ner");
    }

    #[test]
    fn test_mobile_stats_includes_device_tier() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let stats = mobile.stats();
        // On auto-detect, device_tier should be a real tier (full/standard/lite)
        assert!(!stats.device_tier.is_empty());
        assert!(
            stats.device_tier == "full"
                || stats.device_tier == "standard"
                || stats.device_tier == "lite"
        );
    }

    #[test]
    fn test_mobile_explicit_ner_mode_no_model() {
        let config = MobileConfig {
            scanner_mode: "ner".to_string(),
            auto_detect: false,
            // No ner_model_dir → falls back to regex
            ..MobileConfig::default()
        };
        let mobile = OpenObscureMobile::new(config, make_test_key()).unwrap();
        let stats = mobile.stats();
        assert_eq!(stats.scanner_mode, "ner");
        // Even explicit mode now computes device tier for model selection
        assert!(
            stats.device_tier == "full"
                || stats.device_tier == "standard"
                || stats.device_tier == "lite"
        );
    }

    #[test]
    fn test_mobile_sanitize_audio_transcript_with_pii() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let result = mobile
            .sanitize_audio_transcript("my social security number is 123-45-6789")
            .unwrap();
        assert!(result.pii_count >= 1);
        assert!(!result.sanitized_text.contains("123-45-6789"));
    }

    #[test]
    fn test_mobile_sanitize_audio_transcript_clean() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let result = mobile
            .sanitize_audio_transcript("the weather today is sunny and warm")
            .unwrap();
        assert_eq!(result.pii_count, 0);
        assert_eq!(result.sanitized_text, "the weather today is sunny and warm");
    }

    #[test]
    fn test_mobile_check_audio_pii_found() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let count = mobile.check_audio_pii("call me at 555-867-5309 please");
        assert!(count >= 1, "Should detect phone number, got {}", count);
    }

    #[test]
    fn test_mobile_check_audio_pii_clean() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let count = mobile.check_audio_pii("no personal information here");
        assert_eq!(count, 0);
    }

    #[test]
    fn test_mobile_audio_transcript_multiple_pii() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let result = mobile
            .sanitize_audio_transcript("my card is 4111-1111-1111-1111 and my ssn is 123-45-6789")
            .unwrap();
        assert!(
            result.pii_count >= 2,
            "Expected >= 2 PII, got {}",
            result.pii_count
        );
        assert!(!result.sanitized_text.contains("4111"));
        assert!(!result.sanitized_text.contains("123-45-6789"));
    }

    #[test]
    fn test_mobile_audio_transcript_restore_roundtrip() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let sanitized = mobile
            .sanitize_audio_transcript("my email is johnathan.doe@example.com")
            .unwrap();
        assert!(sanitized.pii_count >= 1);
        let restored = mobile.restore_text(&sanitized.sanitized_text, &sanitized.mapping_json);
        assert!(
            restored.contains("johnathan.doe@example.com"),
            "Roundtrip should restore original email, got: {}",
            restored
        );
    }

    #[test]
    fn test_mobile_audio_transcript_empty_string() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let result = mobile.sanitize_audio_transcript("").unwrap();
        assert_eq!(result.pii_count, 0);
        assert_eq!(result.sanitized_text, "");
    }

    #[test]
    fn test_mobile_check_audio_pii_exact_count() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        // Two distinct PII items
        let count = mobile.check_audio_pii("call 555-867-5309 or email johnathan.doe@example.com");
        assert!(count >= 2, "Expected >= 2 PII items, got {}", count);
    }

    #[test]
    fn test_mobile_audio_transcript_unicode_text() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let result = mobile
            .sanitize_audio_transcript("私のメールは johnathan.doe@example.com です")
            .unwrap();
        assert!(result.pii_count >= 1);
        assert!(!result.sanitized_text.contains("johnathan.doe@example.com"));
    }

    // --- Cognitive firewall (response integrity) tests ---

    fn make_ri_config() -> MobileConfig {
        MobileConfig {
            ri_enabled: true,
            ri_sensitivity: "medium".to_string(),
            ..MobileConfig::default()
        }
    }

    #[test]
    fn test_mobile_ri_default_disabled() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        assert!(!mobile.ri_available());
        // scan_response returns None when RI is disabled
        assert!(mobile
            .scan_response("Act now or lose your account!")
            .is_none());
    }

    #[test]
    fn test_mobile_ri_enabled_r1_only() {
        let mobile = OpenObscureMobile::new(make_ri_config(), make_test_key()).unwrap();
        assert!(mobile.ri_available());
    }

    #[test]
    fn test_mobile_ri_detects_urgency() {
        let mobile = OpenObscureMobile::new(make_ri_config(), make_test_key()).unwrap();
        let report = mobile.scan_response("Act now before this limited time offer expires!");
        assert!(report.is_some(), "Should detect urgency phrases");
        let report = report.unwrap();
        assert!(
            report.categories.iter().any(|c| c == "Urgency"),
            "Expected Urgency category, got: {:?}",
            report.categories
        );
        assert!(!report.flags.is_empty());
        assert!(report.scan_time_us > 0);
    }

    #[test]
    fn test_mobile_ri_clean_response() {
        let mobile = OpenObscureMobile::new(make_ri_config(), make_test_key()).unwrap();
        let report =
            mobile.scan_response("The weather is nice today. Here is the code you requested.");
        assert!(report.is_none(), "Clean text should not trigger RI");
    }

    #[test]
    fn test_mobile_ri_severity_in_report() {
        let mobile = OpenObscureMobile::new(make_ri_config(), make_test_key()).unwrap();
        // Multiple persuasion categories should produce a report with a severity tier
        let report = mobile.scan_response(
            "Act now! This is a limited time offer. Everyone is doing it. Don't miss out or you'll regret it!"
        );
        if let Some(r) = report {
            assert!(
                r.severity == "Notice" || r.severity == "Warning" || r.severity == "Caution",
                "Severity should be a valid tier, got: {}",
                r.severity
            );
        }
    }

    #[test]
    fn test_mobile_ri_sensitivity_off() {
        let config = MobileConfig {
            ri_enabled: true,
            ri_sensitivity: "off".to_string(),
            ..MobileConfig::default()
        };
        let mobile = OpenObscureMobile::new(config, make_test_key()).unwrap();
        assert!(mobile.ri_available());
        // sensitivity=off means scan always returns None
        let report = mobile.scan_response("Act now! Limited time offer!");
        assert!(
            report.is_none(),
            "sensitivity=off should suppress all reports"
        );
    }

    #[test]
    fn test_mobile_ri_r2_model_not_found() {
        let config = MobileConfig {
            ri_enabled: true,
            ri_sensitivity: "high".to_string(),
            ri_model_dir: Some("/nonexistent/path/to/ri".to_string()),
            ..MobileConfig::default()
        };
        // Should not fail — graceful degradation to R1-only
        let mobile = OpenObscureMobile::new(config, make_test_key()).unwrap();
        assert!(mobile.ri_available());
    }

    #[test]
    fn test_mobile_ri_config_deserialize() {
        let json =
            r#"{"ri_enabled": true, "ri_sensitivity": "high", "ri_model_dir": "/models/ri"}"#;
        let config: MobileConfig = serde_json::from_str(json).unwrap();
        assert!(config.ri_enabled);
        assert_eq!(config.ri_sensitivity, "high");
        assert_eq!(config.ri_model_dir.unwrap(), "/models/ri");
    }
}

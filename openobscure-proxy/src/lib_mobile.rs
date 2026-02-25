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
use crate::hybrid_scanner::HybridScanner;
use crate::image_pipeline::ImageModelManager;

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
    /// Required for NER to activate when the device profiler enables it.
    #[serde(default)]
    pub ner_model_dir: Option<String>,

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

impl Default for MobileConfig {
    fn default() -> Self {
        Self {
            keywords_enabled: true,
            scanner_mode: "auto".to_string(),
            auto_detect: true,
            crf_model_dir: None,
            ner_model_dir: None,
            image_enabled: false,
            face_model_dir: None,
            scrfd_model_dir: None,
            ocr_model_dir: None,
            nsfw_model_dir: None,
            max_dimension: 960,
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
            // Explicit mode — honor scanner_mode literally
            let scanner = match config.scanner_mode.as_str() {
                "crf" => {
                    if let Some(ref dir) = config.crf_model_dir {
                        match crate::crf_scanner::CrfScanner::load(std::path::Path::new(dir), 0.5) {
                            Ok(crf) => HybridScanner::with_crf(config.keywords_enabled, Some(crf)),
                            Err(_) => HybridScanner::new(config.keywords_enabled, None),
                        }
                    } else {
                        HybridScanner::new(config.keywords_enabled, None)
                    }
                }
                "ner" => {
                    if let Some(ref dir) = config.ner_model_dir {
                        match crate::ner_scanner::NerScanner::load(std::path::Path::new(dir), 0.85)
                        {
                            Ok(ner) => HybridScanner::new(config.keywords_enabled, Some(ner)),
                            Err(_) => HybridScanner::new(config.keywords_enabled, None),
                        }
                    } else {
                        HybridScanner::new(config.keywords_enabled, None)
                    }
                }
                _ => {
                    // "regex" or unknown — regex+keywords only
                    HybridScanner::new(config.keywords_enabled, None)
                }
            };
            (
                scanner,
                config.scanner_mode.clone(),
                "manual".to_string(),
                None,
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
            };
            Some(ImageModelManager::new(img_config))
        } else {
            None
        };

        Ok(Self {
            scanner,
            fpe,
            image_manager,
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
            if let Some(ref dir) = config.ner_model_dir {
                if let Ok(ner) =
                    crate::ner_scanner::NerScanner::load(std::path::Path::new(dir), 0.85)
                {
                    return (
                        HybridScanner::new(config.keywords_enabled, Some(ner)),
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
                        HybridScanner::with_crf(config.keywords_enabled, Some(crf)),
                        "crf".to_string(),
                    );
                }
            }
        }
        (
            HybridScanner::new(config.keywords_enabled, None),
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

        for m in &matches {
            if m.pii_type.is_fpe_eligible() {
                let tweak = TweakGenerator::generate(&request_id, "mobile");
                match self.fpe.encrypt_match(m, &tweak) {
                    Ok(result) => {
                        mapping_data.push((result.encrypted.clone(), m.raw_value.clone()));
                        replacements.push((m.start, m.end, result.encrypted));
                    }
                    Err(_) => {
                        // FPE failed (e.g. domain too small) — fall back to redaction
                        let label = format!("[{}]", m.pii_type);
                        mapping_data.push((label.clone(), m.raw_value.clone()));
                        replacements.push((m.start, m.end, label));
                    }
                }
            } else {
                // Non-FPE types get redacted with label
                let label = format!("[{}]", m.pii_type);
                mapping_data.push((label.clone(), m.raw_value.clone()));
                replacements.push((m.start, m.end, label));
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
            .process_image(img, None)
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
        assert_eq!(stats.device_tier, "manual");
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
        assert_eq!(stats.device_tier, "manual");
    }
}

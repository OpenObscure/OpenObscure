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

/// Global debug log buffer for mobile diagnostics.
/// Swift/Kotlin can retrieve this via `get_debug_log()` UniFFI function.
static DEBUG_LOG: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Append a debug message to the global log buffer.
pub(crate) fn debug_log(msg: String) {
    if let Ok(mut log) = DEBUG_LOG.lock() {
        log.push(msg);
    }
}

/// Drain and return all buffered debug messages.
pub fn drain_debug_log() -> String {
    if let Ok(mut log) = DEBUG_LOG.lock() {
        let result = log.join("\n");
        log.clear();
        result
    } else {
        String::new()
    }
}
use crate::hash_token::TokenGenerator;
use crate::hybrid_scanner::HybridScanner;
use crate::image_pipeline::ImageModelManager;
use crate::name_gazetteer::NameGazetteer;
use crate::response_integrity::{R2Role, ResponseIntegrityScanner, Sensitivity};

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
    /// Defaults to true — the device budget gates actual activation.
    #[serde(default = "default_true")]
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

    /// Path to NSFW holistic classifier model directory (Phase 0b cascade).
    #[serde(default)]
    pub nsfw_classifier_model_dir: Option<String>,

    /// Maximum image dimension before resize.
    #[serde(default = "default_max_dimension")]
    pub max_dimension: u32,

    /// Enable response integrity (cognitive firewall) scanning.
    /// Defaults to true — the device budget gates actual activation.
    #[serde(default = "default_true")]
    pub ri_enabled: bool,

    /// Response integrity sensitivity: "off", "low", "medium" (default), "high".
    #[serde(default = "default_ri_sensitivity")]
    pub ri_sensitivity: String,

    /// Path to R2 model directory (model_int8.onnx + vocab.txt).
    /// Optional — R1 dictionary scan works without any model files.
    #[serde(default)]
    pub ri_model_dir: Option<String>,

    /// Base directory containing model subdirectories.
    /// When set, individual `*_model_dir` fields are auto-resolved from standard
    /// subdirectory names (ner/, ner_lite/, crf/, scrfd/, blazeface/, ocr/, nsfw/, ri/).
    /// Individual `*_model_dir` fields override the auto-resolved path if both are set.
    #[serde(default)]
    pub models_base_dir: Option<String>,

    /// Enable name gazetteer for person name detection.
    /// Uses embedded name lists (no model files needed). Default: true.
    #[serde(default = "default_true")]
    pub gazetteer_enabled: bool,

    /// Number of NER model instances in the pool. Default: 1.
    /// Higher values allow concurrent inference but use more memory.
    #[serde(default = "default_ner_pool_size")]
    pub ner_pool_size: usize,
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

fn default_ner_pool_size() -> usize {
    1
}

fn default_ri_sensitivity() -> String {
    "medium".to_string()
}

impl MobileConfig {
    /// Verify that all expected model subdirectories exist under `models_base_dir`.
    /// Logs missing models to the debug buffer so Swift/Kotlin can surface them.
    fn verify_models(&self) {
        let base = match self.models_base_dir.as_ref() {
            Some(b) => std::path::PathBuf::from(b),
            None => {
                debug_log("models_base_dir not set — skipping model verification".to_string());
                return;
            }
        };

        // All subdirectory names that resolve_model_dirs() looks for
        let expected = [
            ("ner", "DistilBERT NER (Full/Standard)"),
            ("ner_lite", "TinyBERT NER (Lite/fallback)"),
            ("scrfd", "SCRFD face detection (Full/Standard)"),
            ("blazeface", "BlazeFace face detection (Lite/fallback)"),
            ("ocr", "PaddleOCR text detection"),
            ("nsfw", "NudeNet NSFW detection"),
            ("nsfw_classifier", "NSFW classifier"),
            ("ri", "Response Integrity model"),
        ];

        let mut missing = Vec::new();
        let mut present = Vec::new();
        for (name, desc) in &expected {
            if base.join(name).is_dir() {
                present.push(*name);
            } else {
                missing.push(format!("{} ({})", name, desc));
            }
        }

        debug_log(format!(
            "model_verify: present=[{}] missing=[{}]",
            present.join(", "),
            if missing.is_empty() {
                "none".to_string()
            } else {
                missing.join(", ")
            }
        ));
    }

    /// Resolve individual `*_model_dir` fields from `models_base_dir`.
    ///
    /// For each model directory field that is `None`, checks if the standard
    /// subdirectory exists under `models_base_dir` and sets it. Explicit
    /// per-model paths always take priority.
    fn resolve_model_dirs(&mut self) {
        let base = match self.models_base_dir.as_ref() {
            Some(b) => std::path::PathBuf::from(b),
            None => return,
        };

        let resolve = |field: &mut Option<String>, subdir: &str| {
            if field.is_none() {
                let path = base.join(subdir);
                if path.is_dir() {
                    *field = Some(path.to_string_lossy().into_owned());
                }
            }
        };

        resolve(&mut self.ner_model_dir, "ner");
        resolve(&mut self.ner_model_dir_lite, "ner_lite");
        resolve(&mut self.crf_model_dir, "crf");
        resolve(&mut self.scrfd_model_dir, "scrfd");
        resolve(&mut self.face_model_dir, "blazeface");
        resolve(&mut self.ocr_model_dir, "ocr");
        resolve(&mut self.nsfw_model_dir, "nsfw");
        resolve(&mut self.nsfw_classifier_model_dir, "nsfw_classifier");
        resolve(&mut self.ri_model_dir, "ri");
    }
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
            image_enabled: true,
            face_model_dir: None,
            scrfd_model_dir: None,
            ocr_model_dir: None,
            nsfw_model_dir: None,
            nsfw_classifier_model_dir: None,
            max_dimension: 960,
            ri_enabled: true,
            ri_sensitivity: "medium".to_string(),
            ri_model_dir: None,
            models_base_dir: None,
            gazetteer_enabled: true,
            ner_pool_size: 1,
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

/// Manages FPE key rotation with a 30-second overlap window.
/// During the overlap, the previous key is retained so that in-flight
/// responses encrypted with the old key can still be restored.
struct MobileKeyManager {
    current: std::sync::RwLock<FpeEngine>,
    previous: std::sync::Mutex<Option<(FpeEngine, std::time::Instant)>>,
}

impl MobileKeyManager {
    fn new(engine: FpeEngine) -> Self {
        Self {
            current: std::sync::RwLock::new(engine),
            previous: std::sync::Mutex::new(None),
        }
    }

    /// Rotate to a new FPE key. The old engine is retained for 30 seconds.
    fn rotate(&self, new_key: &[u8; 32]) -> Result<(), MobileError> {
        let new_engine = FpeEngine::new(new_key)?;
        let mut current = self.current.write().unwrap();
        let old = std::mem::replace(&mut *current, new_engine);
        let mut prev = self.previous.lock().unwrap();
        *prev = Some((old, std::time::Instant::now()));
        Ok(())
    }

    /// Access the current engine for encryption.
    fn current(&self) -> std::sync::RwLockReadGuard<'_, FpeEngine> {
        self.current.read().unwrap()
    }

    /// Access the previous engine if still within the 30-second overlap window.
    #[allow(dead_code)]
    fn previous(&self) -> Option<FpeEngine> {
        // Note: we cannot return a reference through the Mutex, so we check
        // and return None if expired. The previous engine is only used for
        // FPE decryption during overlap, which restore_text doesn't need
        // (it uses stored mappings). This is provided for future use.
        let mut prev = self.previous.lock().unwrap();
        if let Some((_, retired_at)) = prev.as_ref() {
            if retired_at.elapsed() >= std::time::Duration::from_secs(30) {
                *prev = None;
            }
        }
        None // Previous engine access not needed for current mobile restore pattern
    }
}

/// The main mobile API handle. Thread-safe and reusable across calls.
pub struct OpenObscureMobile {
    scanner: HybridScanner,
    key_manager: MobileKeyManager,
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
    pub fn new(mut config: MobileConfig, fpe_key: [u8; 32]) -> Result<Self, MobileError> {
        // Resolve model paths from base directory before anything else
        config.verify_models();
        config.resolve_model_dirs();

        let fpe = FpeEngine::new(&fpe_key)?;
        let key_manager = MobileKeyManager::new(fpe);

        // Compute device budget first — all feature gating flows through it
        let profile = crate::device_profile::detect(true);
        let tier = crate::device_profile::tier_for_profile(&profile);
        let budget = crate::device_profile::budget_for_tier(tier, &profile);

        // Gate features by budget ∩ config (both must agree)
        let gazetteer_allowed = config.gazetteer_enabled && budget.gazetteer_enabled;
        let keywords_allowed = config.keywords_enabled && budget.keywords_enabled;
        let effective_pool_size = config.ner_pool_size.min(budget.ner_pool_size);

        // Load name gazetteer if allowed by budget and config
        let gazetteer = if gazetteer_allowed {
            Some(NameGazetteer::new())
        } else {
            None
        };

        // Determine scanner and tier via auto-detection or explicit mode
        let (scanner, effective_mode, device_tier) = if config.auto_detect
            && config.scanner_mode == "auto"
        {
            let (mut scan, mode) = Self::build_scanner_from_budget(
                &config,
                &budget,
                gazetteer,
                keywords_allowed,
                effective_pool_size,
            );
            let effective_bonus = if budget.ensemble_enabled { 0.15 } else { 0.0 };
            scan.set_confidence_params(0.5, effective_bonus);
            (scan, mode, tier.to_string())
        } else {
            // Explicit mode — honor scanner_mode literally, but still use
            // device tier for NER model selection (TinyBERT vs DistilBERT).
            let scanner = match config.scanner_mode.as_str() {
                "crf" => {
                    if let Some(ref dir) = config.crf_model_dir {
                        match crate::crf_scanner::CrfScanner::load(std::path::Path::new(dir), 0.5) {
                            Ok(crf) => {
                                HybridScanner::with_crf(keywords_allowed, Some(crf), gazetteer)
                            }
                            Err(_) => HybridScanner::new(keywords_allowed, None, gazetteer),
                        }
                    } else {
                        HybridScanner::new(keywords_allowed, None, gazetteer)
                    }
                }
                "ner" => {
                    // Select model dir based on device tier, with fallback
                    let model_dir = match budget.ner_model.as_str() {
                        "tinybert" => config
                            .ner_model_dir_lite
                            .as_ref()
                            .or(config.ner_model_dir.as_ref()),
                        _ => config
                            .ner_model_dir
                            .as_ref()
                            .or(config.ner_model_dir_lite.as_ref()),
                    };
                    let pool = model_dir.and_then(|d| Self::load_ner_pool(d, effective_pool_size));
                    HybridScanner::new(keywords_allowed, pool, gazetteer)
                }
                _ => {
                    // "regex" or unknown — regex+keywords only
                    HybridScanner::new(keywords_allowed, None, gazetteer)
                }
            };
            (scanner, config.scanner_mode.clone(), tier.to_string())
        };

        // Build image pipeline if enabled and budget allows
        let image_manager = if config.image_enabled && budget.image_pipeline_enabled {
            let idle_timeout = budget.model_idle_timeout_secs;
            let ocr_tier = budget.ocr_tier.clone();
            let screen_guard = budget.screen_guard_enabled;
            let nsfw_enabled = budget.nsfw_enabled;
            let face_model_name = budget.face_model.clone();
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
                nsfw_classifier_enabled: nsfw_enabled && config.nsfw_classifier_model_dir.is_some(),
                nsfw_classifier_model_dir: if nsfw_enabled {
                    config.nsfw_classifier_model_dir
                } else {
                    None
                },
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
        let ri_scanner = if config.ri_enabled && budget.ri_enabled {
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
            key_manager,
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

    /// Load NER pool with `config.ner_pool_size` instances.
    fn load_ner_pool(dir: &str, pool_size: usize) -> Option<crate::ner_scanner::NerPool> {
        let path = std::path::Path::new(dir);
        let mut scanners = Vec::with_capacity(pool_size.max(1));
        for _ in 0..pool_size.max(1) {
            match crate::ner_scanner::NerScanner::load(path, 0.60) {
                Ok(s) => scanners.push(s),
                Err(e) => {
                    debug_log(format!("NER model load FAILED dir={} error={}", dir, e));
                    break;
                }
            }
        }
        if scanners.is_empty() {
            None
        } else {
            Some(crate::ner_scanner::NerPool::new(scanners))
        }
    }

    /// Build a scanner based on the device profiler's feature budget.
    fn build_scanner_from_budget(
        config: &MobileConfig,
        budget: &crate::device_profile::FeatureBudget,
        gazetteer: Option<NameGazetteer>,
        keywords_allowed: bool,
        pool_size: usize,
    ) -> (HybridScanner, String) {
        if budget.ner_enabled {
            // Select model dir based on budget tier.
            // Prefer the budget-recommended model, but fall back to whatever is available.
            // tinybert → ner_lite/ (fallback: ner/)
            // distilbert → ner/ (fallback: ner_lite/)
            let model_dir = match budget.ner_model.as_str() {
                "tinybert" => config
                    .ner_model_dir_lite
                    .as_ref()
                    .or(config.ner_model_dir.as_ref()),
                _ => config
                    .ner_model_dir
                    .as_ref()
                    .or(config.ner_model_dir_lite.as_ref()),
            };
            if let Some(pool) = model_dir.and_then(|d| Self::load_ner_pool(d, pool_size)) {
                return (
                    HybridScanner::new(keywords_allowed, Some(pool), gazetteer),
                    "ner".to_string(),
                );
            }
        }
        if budget.crf_enabled {
            if let Some(ref dir) = config.crf_model_dir {
                if let Ok(crf) =
                    crate::crf_scanner::CrfScanner::load(std::path::Path::new(dir), 0.5)
                {
                    return (
                        HybridScanner::with_crf(keywords_allowed, Some(crf), gazetteer),
                        "crf".to_string(),
                    );
                }
            }
        }
        (
            HybridScanner::new(keywords_allowed, None, gazetteer),
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
        // Mapping tuples: (ciphertext, plaintext, pii_type_str)
        let mut mapping_data: Vec<(String, String, String)> = Vec::new();
        let mut token_gen = TokenGenerator::new(request_id);
        let fpe = self.key_manager.current();

        for m in &matches {
            let type_str = m.pii_type.to_string();
            if m.pii_type.is_fpe_eligible() {
                let tweak = TweakGenerator::generate(&request_id, &format!("m:{}", m.start));
                match fpe.encrypt_match(m, &tweak) {
                    Ok(result) => {
                        mapping_data.push((
                            result.encrypted.clone(),
                            m.raw_value.clone(),
                            type_str,
                        ));
                        replacements.push((m.start, m.end, result.encrypted));
                    }
                    Err(_) => {
                        // FPE failed (e.g. domain too small) — fall back to hash token
                        let token = token_gen.generate(m.pii_type, &m.raw_value);
                        mapping_data.push((token.clone(), m.raw_value.clone(), type_str));
                        replacements.push((m.start, m.end, token));
                    }
                }
            } else {
                // Non-FPE types get hash-based token (e.g., PER_a7f2)
                let token = token_gen.generate(m.pii_type, &m.raw_value);
                mapping_data.push((token.clone(), m.raw_value.clone(), type_str));
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
    /// Mirrors the gateway's `decrypt_response` logic:
    /// 1. Normalize markdown escapes (`\_` → `_`) — LLMs escape underscores in tokens
    /// 2. Normalize unicode dashes (en-dash, em-dash, etc.) to ASCII hyphens
    /// 3. Exact match pass (fast path)
    /// 4. Fuzzy regex match for numeric PII types (handles LLM reformatting of separators)
    pub fn restore_text(&self, text: &str, mapping_json: &str) -> String {
        // Accept both 2-tuple (legacy) and 3-tuple (with PII type) formats
        let mappings: Vec<(String, String, Option<String>)> =
            if let Ok(m) = serde_json::from_str::<Vec<(String, String, String)>>(mapping_json) {
                m.into_iter().map(|(c, p, t)| (c, p, Some(t))).collect()
            } else if let Ok(m) = serde_json::from_str::<Vec<(String, String)>>(mapping_json) {
                m.into_iter().map(|(c, p)| (c, p, None)).collect()
            } else {
                return text.to_string();
            };

        // Normalize: markdown escape + unicode dashes (same as gateway)
        let result = text.replace("\\_", "_");
        let mut result = crate::mapping::normalize_unicode_dashes(&result);

        // Sort by ciphertext length descending to avoid partial matches
        let mut sorted = mappings;
        sorted.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

        // Phase 1: Exact match (fast path)
        let mut unmatched_numeric: Vec<&(String, String, Option<String>)> = Vec::new();
        for entry in &sorted {
            if result.contains(&entry.0) {
                result = result.replace(&entry.0, &entry.1);
            } else if let Some(ref t) = entry.2 {
                if matches!(t.as_str(), "credit_card" | "ssn" | "phone" | "iban") {
                    unmatched_numeric.push(entry);
                }
            }
        }

        // Phase 2: Fuzzy regex match for unmatched numeric PII (handles LLM reformatting)
        for (ciphertext, plaintext, type_str) in &unmatched_numeric {
            let pii_type = match type_str.as_deref() {
                Some("phone") => crate::pii_types::PiiType::PhoneNumber,
                Some("ssn") => crate::pii_types::PiiType::Ssn,
                Some("credit_card") => crate::pii_types::PiiType::CreditCard,
                Some("iban") => crate::pii_types::PiiType::Iban,
                _ => continue,
            };
            if let Some(re) = crate::mapping::build_fpe_fuzzy_regex(ciphertext, &pii_type) {
                result = re
                    .replace_all(&result, regex::NoExpand(plaintext))
                    .to_string();
            }
        }

        result
    }

    /// Process an image for visual PII (face redaction, OCR text redaction, EXIF strip).
    ///
    /// EXIF metadata is always stripped (decode → re-encode), even when the full
    /// image pipeline is disabled. Face/OCR/NSFW detection requires models but
    /// EXIF stripping does not.
    ///
    /// Returns the sanitized image bytes in JPEG format.
    pub fn sanitize_image(&self, image_bytes: &[u8]) -> Result<Vec<u8>, MobileError> {
        // Always decode first — this strips all metadata (EXIF, IPTC, XMP)
        // because the `image` crate only loads pixel data.
        let img = crate::image_pipeline::decode_image(image_bytes)
            .map_err(|e| MobileError::ImageError(e.to_string()))?;

        let result_img = if let Some(ref manager) = self.image_manager {
            // Full pipeline: screenshot detection → resize → face/OCR/NSFW
            let screen_result = if manager.config().screen_guard {
                Some(crate::screen_guard::detect_screenshot(image_bytes, &img))
            } else {
                None
            };

            let max_dim = manager.config().max_dimension;
            let img = crate::image_pipeline::resize_if_needed(img, max_dim);

            let (result_img, _stats, _meta) = manager
                .process_image(img, screen_result.as_ref(), Some(&self.scanner))
                .map_err(|e| MobileError::ImageError(e.to_string()))?;
            result_img
        } else {
            // No image pipeline — still resize and strip EXIF via decode → re-encode
            crate::image_pipeline::resize_if_needed(img, 960)
        };

        // Update stats
        if let Ok(mut s) = self.stats.lock() {
            s.total_images_processed += 1;
        }

        // Encode back to JPEG (metadata-free)
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
        // Suppress R2-only discoveries (no R1 confirmation) — high false-positive risk
        if report.r2_role == R2Role::Discover {
            return None;
        }
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

    /// Rotate the FPE key. The previous key is retained for 30 seconds
    /// so that in-flight responses can still be restored using saved mappings.
    ///
    /// Call this when the host app's secure storage provides a new key
    /// (e.g., periodic rotation or user-initiated key change).
    pub fn rotate_key(&self, new_key: &[u8; 32]) -> Result<(), MobileError> {
        self.key_manager.rotate(new_key)
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
    fn test_mobile_sanitize_api_key() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let result = mobile
            .sanitize_text("Here's my ABC key: sk-ant-api03-abc123def456ghi789jkl012mn")
            .unwrap();
        eprintln!("api_key sanitized: {}", result.sanitized_text);
        assert!(result.pii_count >= 1, "Expected API key to be detected");
        assert!(
            !result
                .sanitized_text
                .contains("sk-ant-api03-abc123def456ghi789jkl012mn"),
            "API key should be sanitized, got: {}",
            result.sanitized_text
        );
        assert!(result.categories.contains(&"api_key".to_string()));
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
        assert!(stats.image_pipeline_available);
    }

    #[test]
    fn test_mobile_sanitize_ipv4() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let result = mobile.sanitize_text("Server IP is 203.0.113.42").unwrap();
        assert!(result.pii_count >= 1, "Expected IP to be detected");
        assert!(
            !result.sanitized_text.contains("203.0.113.42"),
            "IP should be sanitized, got: {}",
            result.sanitized_text
        );
        assert!(result.categories.contains(&"ipv4_address".to_string()));
    }

    #[test]
    fn test_mobile_sanitize_mac_address() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let result = mobile.sanitize_text("MAC: aa:bb:cc:dd:ee:ff").unwrap();
        assert!(result.pii_count >= 1, "Expected MAC to be detected");
        assert!(
            !result.sanitized_text.contains("aa:bb:cc:dd:ee:ff"),
            "MAC should be sanitized, got: {}",
            result.sanitized_text
        );
        assert!(result.categories.contains(&"mac_address".to_string()));
    }

    #[test]
    fn test_mobile_sanitize_gps() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let result = mobile
            .sanitize_text("Location: 45.52310, -122.67650")
            .unwrap();
        assert!(result.pii_count >= 1, "Expected GPS to be detected");
        assert!(
            !result.sanitized_text.contains("45.52310"),
            "GPS should be sanitized, got: {}",
            result.sanitized_text
        );
        assert!(result.categories.contains(&"gps_coordinate".to_string()));
    }

    #[test]
    fn test_mobile_sanitize_ipv4_roundtrip() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let result = mobile.sanitize_text("Server IP is 203.0.113.42").unwrap();
        assert!(result.pii_count >= 1);
        let restored = mobile.restore_text(&result.sanitized_text, &result.mapping_json);
        assert!(
            restored.contains("203.0.113.42"),
            "Expected restored text to contain original IP, got: {}",
            restored
        );
    }

    #[test]
    fn test_mobile_ri_scan_detects_persuasion() {
        let mobile = OpenObscureMobile::new(make_ri_config(), make_test_key()).unwrap();
        assert!(mobile.ri_available());
        let report = mobile.scan_response(
            "You must act now! This is a limited time offer. Don't miss out or you'll regret it forever. Trust me, I'm an expert.",
        );
        assert!(
            report.is_some(),
            "Expected RI scanner to detect persuasion phrases"
        );
        let r = report.unwrap();
        assert!(
            !r.categories.is_empty(),
            "Expected at least one persuasion category"
        );
    }

    #[test]
    fn test_mobile_ri_scan_short_phrase() {
        let mobile = OpenObscureMobile::new(make_ri_config(), make_test_key()).unwrap();
        assert!(mobile.ri_available());
        let report = mobile.scan_response("You must act now! Don't miss out!");
        eprintln!("Short phrase RI result: {:?}", report);
        // Even a short phrase with "act now" + "don't miss out" should be detected
        assert!(
            report.is_some(),
            "Expected short persuasion phrase to be detected"
        );
    }

    #[test]
    fn test_mobile_ri_scan_clean_response() {
        let mobile = OpenObscureMobile::new(make_ri_config(), make_test_key()).unwrap();
        let report =
            mobile.scan_response("The weather today is partly cloudy with a high of 72 degrees.");
        assert!(
            report.is_none(),
            "Expected clean response to return None, got: {:?}",
            report
        );
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
        assert!(config.image_enabled);
        assert!(config.ri_enabled);
        assert!(config.models_base_dir.is_none());
        assert_eq!(config.max_dimension, 960);
    }

    #[test]
    fn test_mobile_invalid_key() {
        // Key must be 32 bytes — test that engine creation works
        let result = OpenObscureMobile::new(MobileConfig::default(), [0u8; 32]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_mobile_image_not_enabled_still_strips_exif() {
        let config = MobileConfig {
            image_enabled: false,
            ..MobileConfig::default()
        };
        let mobile = OpenObscureMobile::new(config, make_test_key()).unwrap();
        // Invalid bytes still produce a decode error (not "not enabled")
        let result = mobile.sanitize_image(&[0xFF, 0xD8, 0xFF]);
        assert!(result.is_err());
        // Error should be a decode error, not "not enabled"
        let err_msg = result.unwrap_err().to_string();
        assert!(
            !err_msg.contains("not enabled"),
            "sanitize_image should attempt decode even without image pipeline: {err_msg}"
        );
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
    fn test_mobile_models_base_dir_enables_face_detection() {
        // Simulates the exact config Enchanted/RikkaHub use
        let models_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models");
        let config_json = serde_json::json!({
            "scanner_mode": "regex",
            "models_base_dir": models_dir.to_str().unwrap()
        })
        .to_string();
        let config: MobileConfig = serde_json::from_str(&config_json).unwrap();
        assert!(config.image_enabled, "image_enabled should default to true");
        assert!(
            config.scrfd_model_dir.is_none(),
            "before resolve, scrfd_model_dir should be None"
        );

        let mobile = OpenObscureMobile::new(config, make_test_key()).unwrap();
        let stats = mobile.stats();
        assert!(
            stats.image_pipeline_available,
            "image pipeline should be available"
        );

        // Load the face test image and verify sanitize_image succeeds.
        // Skip if the file is a Git LFS pointer (CI without LFS pull).
        let face_img_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("test/data/input/Visual_PII/Faces/face_single_frontal_05.jpg");
        if face_img_path.exists() {
            let img_bytes = std::fs::read(&face_img_path).unwrap();
            // LFS pointer files start with "version https://git-lfs"
            if img_bytes.starts_with(b"version ") {
                eprintln!("Skipping face image test: LFS pointer (not fetched)");
                return;
            }
            let result = mobile.sanitize_image(&img_bytes);
            assert!(
                result.is_ok(),
                "sanitize_image should succeed: {:?}",
                result.err()
            );
            let sanitized = result.unwrap();
            // Sanitized image should be different from original (face redaction applied)
            assert_ne!(sanitized.len(), 0);
            assert_ne!(
                sanitized, img_bytes,
                "sanitized image should differ from original"
            );
        }
    }

    #[test]
    fn test_mobile_nsfw_detection_via_models_base_dir() {
        let models_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models");
        let config_json = serde_json::json!({
            "scanner_mode": "regex",
            "models_base_dir": models_dir.to_str().unwrap()
        })
        .to_string();
        let mobile =
            OpenObscureMobile::new(serde_json::from_str(&config_json).unwrap(), make_test_key())
                .unwrap();

        let nsfw_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("test/data/input/Visual_PII/NSFW/semi_nu_pic2.jpg");
        if nsfw_path.exists() {
            let img_bytes = std::fs::read(&nsfw_path).unwrap();
            // Skip if LFS pointer (CI without LFS pull)
            if img_bytes.starts_with(b"version ") {
                eprintln!("Skipping NSFW image test: LFS pointer (not fetched)");
                return;
            }
            let result = mobile.sanitize_image(&img_bytes).unwrap();
            // NSFW should solid-fill the entire image — result should differ from input
            assert_ne!(result, img_bytes, "NSFW image should be redacted");
            eprintln!(
                "NSFW result: {} bytes (original {} bytes)",
                result.len(),
                img_bytes.len()
            );
        }
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
    fn test_mobile_config_defaults_budget_driven() {
        // All feature flags should default to true — the budget gates activation
        let config = MobileConfig::default();
        assert!(config.image_enabled, "image_enabled should default to true");
        assert!(config.ri_enabled, "ri_enabled should default to true");
        assert!(
            config.keywords_enabled,
            "keywords_enabled should default to true"
        );
        assert!(config.auto_detect, "auto_detect should default to true");
        assert_eq!(config.scanner_mode, "auto");
    }

    #[test]
    fn test_mobile_config_deserialize_defaults_true() {
        // Minimal config — all feature flags should be true by default
        let json = r#"{}"#;
        let config: MobileConfig = serde_json::from_str(json).unwrap();
        assert!(config.image_enabled);
        assert!(config.ri_enabled);
        assert!(config.keywords_enabled);
        assert!(config.auto_detect);
        assert!(config.models_base_dir.is_none());
    }

    #[test]
    fn test_mobile_config_models_base_dir_resolves() {
        let tmp = std::env::temp_dir().join("oo_test_base_dir");
        let _ = std::fs::remove_dir_all(&tmp);
        // Create standard subdirectories
        std::fs::create_dir_all(tmp.join("ner")).unwrap();
        std::fs::create_dir_all(tmp.join("ocr")).unwrap();
        std::fs::create_dir_all(tmp.join("scrfd")).unwrap();

        let mut config = MobileConfig {
            models_base_dir: Some(tmp.to_string_lossy().into_owned()),
            ..MobileConfig::default()
        };
        config.resolve_model_dirs();

        assert_eq!(
            config.ner_model_dir.as_deref(),
            Some(tmp.join("ner").to_str().unwrap())
        );
        assert_eq!(
            config.ocr_model_dir.as_deref(),
            Some(tmp.join("ocr").to_str().unwrap())
        );
        assert_eq!(
            config.scrfd_model_dir.as_deref(),
            Some(tmp.join("scrfd").to_str().unwrap())
        );
        // Subdirs that don't exist should remain None
        assert!(config.crf_model_dir.is_none());
        assert!(config.nsfw_model_dir.is_none());
        assert!(config.face_model_dir.is_none());
        assert!(config.ri_model_dir.is_none());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_mobile_config_explicit_dir_overrides_base() {
        let tmp = std::env::temp_dir().join("oo_test_override");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("ner")).unwrap();

        let mut config = MobileConfig {
            models_base_dir: Some(tmp.to_string_lossy().into_owned()),
            ner_model_dir: Some("/explicit/ner".to_string()),
            ..MobileConfig::default()
        };
        config.resolve_model_dirs();

        // Explicit path takes priority over base dir resolution
        assert_eq!(config.ner_model_dir.as_deref(), Some("/explicit/ner"));

        let _ = std::fs::remove_dir_all(&tmp);
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
    fn test_mobile_ri_default_enabled() {
        // RI defaults to enabled — budget gates activation
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        assert!(mobile.ri_available());
        // R1 dictionary scanner works without models
        let result = mobile.scan_response("Act now or lose your account!");
        assert!(result.is_some(), "R1 should detect persuasion phrases");
    }

    #[test]
    fn test_mobile_ri_explicitly_disabled() {
        let config = MobileConfig {
            ri_enabled: false,
            ..MobileConfig::default()
        };
        let mobile = OpenObscureMobile::new(config, make_test_key()).unwrap();
        assert!(!mobile.ri_available());
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
    fn test_mobile_ri_with_regex_scanner_mode() {
        // Simulate exact Enchanted config
        let json = r#"{"scanner_mode": "regex", "models_base_dir": "/nonexistent"}"#;
        let config: MobileConfig = serde_json::from_str(json).unwrap();
        assert!(
            config.ri_enabled,
            "ri_enabled should default to true even with scanner_mode=regex"
        );
        let mobile = OpenObscureMobile::new(config, make_test_key()).unwrap();
        assert!(
            mobile.ri_available(),
            "RI should be available with scanner_mode=regex"
        );
        let report = mobile.scan_response("You must act now! Don't miss out!");
        eprintln!("regex mode RI result: {:?}", report);
        assert!(
            report.is_some(),
            "Should detect persuasion with scanner_mode=regex"
        );
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

    #[test]
    fn test_mobile_restore_markdown_escaped_tokens() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let sanitized = mobile
            .sanitize_text("Patient has hypertension and diabetes")
            .unwrap();
        assert!(sanitized.pii_count >= 2);

        // Simulate LLM escaping underscores in hash tokens: HLT_xxxx → HLT\_xxxx
        let escaped = sanitized.sanitized_text.replace("_", "\\_");
        let restored = mobile.restore_text(&escaped, &sanitized.mapping_json);
        assert!(
            restored.contains("hypertension"),
            "Markdown-escaped token should be restored, got: {}",
            restored
        );
        assert!(
            restored.contains("diabetes"),
            "Markdown-escaped token should be restored, got: {}",
            restored
        );
    }

    #[test]
    fn test_mobile_restore_unicode_dashes() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let sanitized = mobile.sanitize_text("SSN: 123-45-6789").unwrap();
        assert!(sanitized.pii_count >= 1);

        // Replace ASCII hyphens with en-dashes in the ciphertext (LLM behavior)
        let with_endash = sanitized.sanitized_text.replace('-', "\u{2013}");
        let restored = mobile.restore_text(&with_endash, &sanitized.mapping_json);
        assert!(
            restored.contains("123-45-6789"),
            "Unicode dash normalization should enable restore, got: {}",
            restored
        );
    }

    #[test]
    fn test_mobile_restore_fuzzy_phone_reformatting() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        let sanitized = mobile.sanitize_text("Call (305) 555-0188").unwrap();
        assert!(sanitized.pii_count >= 1);

        // Extract the FPE-encrypted phone digits, reformat with dots (LLM behavior)
        let ct = &sanitized.sanitized_text;
        let digits: String = ct.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.len() >= 10 {
            let reformatted = format!(
                "Call {}.{}.{}",
                &digits[digits.len() - 10..digits.len() - 7],
                &digits[digits.len() - 7..digits.len() - 4],
                &digits[digits.len() - 4..]
            );
            let restored = mobile.restore_text(&reformatted, &sanitized.mapping_json);
            assert!(
                restored.contains("(305) 555-0188")
                    || restored.contains("305-555-0188")
                    || restored.contains("305") && restored.contains("0188"),
                "Fuzzy match should restore reformatted phone, got: {}",
                restored
            );
        }
    }

    #[test]
    fn test_mobile_restore_legacy_2tuple_mapping() {
        let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
        // Legacy 2-tuple format (no PII type) — should still work for exact matches
        let legacy_mapping = r#"[["HLT_test","hypertension"],["PER_abcd","John Smith"]]"#;
        let response = "Patient HLT_test treated by PER_abcd";
        let restored = mobile.restore_text(response, legacy_mapping);
        assert_eq!(restored, "Patient hypertension treated by John Smith");
    }
}

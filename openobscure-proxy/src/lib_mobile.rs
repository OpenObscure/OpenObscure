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

    /// Scanner mode: "regex", "crf", or "auto" (default).
    /// NER is not recommended on mobile due to RAM constraints.
    #[serde(default = "default_scanner_mode")]
    pub scanner_mode: String,

    /// Path to CRF model directory (if using CRF scanner).
    #[serde(default)]
    pub crf_model_dir: Option<String>,

    /// Enable image processing pipeline.
    #[serde(default)]
    pub image_enabled: bool,

    /// Path to face detection model directory.
    #[serde(default)]
    pub face_model_dir: Option<String>,

    /// Path to OCR model directory.
    #[serde(default)]
    pub ocr_model_dir: Option<String>,

    /// Maximum image dimension before resize.
    #[serde(default = "default_max_dimension")]
    pub max_dimension: u32,

    /// Data Protection Officer email (for breach notification drafts).
    #[serde(default)]
    pub dpo_email: Option<String>,

    /// Supervisory authority / DPA contact email.
    #[serde(default)]
    pub dpa_contact: Option<String>,
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
            crf_model_dir: None,
            image_enabled: false,
            face_model_dir: None,
            ocr_model_dir: None,
            max_dimension: 960,
            dpo_email: None,
            dpa_contact: None,
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
    #[error("Governance error: {0}")]
    Governance(String),
    #[error("Governance not enabled")]
    GovernanceNotEnabled,
    #[error("Crypto error: {0}")]
    CryptoError(String),
}

/// Result of a file access check.
#[derive(Debug, Clone)]
pub struct FileCheckResultMobile {
    pub allowed: bool,
    pub reason: Option<String>,
}

/// Result of a privacy command.
#[derive(Debug, Clone)]
pub struct PrivacyCommandResultMobile {
    pub text: String,
    pub success: bool,
}

/// Result of retention enforcement.
#[derive(Debug, Clone)]
pub struct EnforceResultMobile {
    pub promoted: u32,
    pub pruned: u32,
}

/// Retention summary for diagnostics.
#[derive(Debug, Clone)]
pub struct RetentionSummaryMobile {
    pub hot: u32,
    pub warm: u32,
    pub cold: u32,
    pub expired: u32,
    pub total: u32,
}

/// Consent record exposed to mobile API.
#[derive(Debug, Clone)]
pub struct ConsentRecordMobile {
    pub id: i64,
    pub consent_type: String,
    pub granted: bool,
    pub version: i64,
}

/// Breach assessment result for mobile API.
#[derive(Debug, Clone)]
pub struct BreachAssessmentMobile {
    pub risk_level: String,
    pub anomaly_count: u32,
    pub recommendation: String,
    pub anomalies_json: String,
}

/// The main mobile API handle. Thread-safe and reusable across calls.
pub struct OpenObscureMobile {
    scanner: HybridScanner,
    fpe: FpeEngine,
    image_manager: Option<ImageModelManager>,
    #[cfg(feature = "governance")]
    governance: Option<crate::governance::GovernanceEngine>,
    stats: std::sync::Mutex<InternalStats>,
}

struct InternalStats {
    total_pii_found: u64,
    total_images_processed: u64,
    scanner_mode: String,
}

impl OpenObscureMobile {
    /// Create a new mobile OpenObscure instance.
    ///
    /// # Arguments
    /// * `config` - Mobile configuration
    /// * `fpe_key` - 32-byte AES-256 key for Format-Preserving Encryption
    pub fn new(config: MobileConfig, fpe_key: [u8; 32]) -> Result<Self, MobileError> {
        let fpe = FpeEngine::new(&fpe_key)?;

        // Build scanner based on config
        let scanner_mode = config.scanner_mode.clone();
        let scanner = match config.scanner_mode.as_str() {
            "crf" => {
                if let Some(ref dir) = config.crf_model_dir {
                    match crate::crf_scanner::CrfScanner::load(&std::path::Path::new(dir), 0.5) {
                        Ok(crf) => HybridScanner::with_crf(config.keywords_enabled, Some(crf)),
                        Err(_) => HybridScanner::new(config.keywords_enabled, None),
                    }
                } else {
                    HybridScanner::new(config.keywords_enabled, None)
                }
            }
            _ => {
                // "auto" or "regex" — on mobile, default to regex+keywords only
                // to stay within RAM budget. CRF can be enabled explicitly.
                HybridScanner::new(config.keywords_enabled, None)
            }
        };

        // Build image pipeline if enabled and model paths provided
        let image_manager = if config.image_enabled {
            let img_config = ImageConfig {
                enabled: true,
                face_detection: config.face_model_dir.is_some(),
                ocr_enabled: config.ocr_model_dir.is_some(),
                ocr_tier: "detect_and_blur".to_string(),
                max_dimension: config.max_dimension,
                face_blur_sigma: 25.0,
                text_blur_sigma: 20.0,
                model_idle_timeout_secs: 300,
                face_model_dir: config.face_model_dir,
                ocr_model_dir: config.ocr_model_dir,
                screen_guard: false,
                exif_strip: true,
                nsfw_detection: false,
                nsfw_model_dir: None,
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
            #[cfg(feature = "governance")]
            governance: None,
            stats: std::sync::Mutex::new(InternalStats {
                total_pii_found: 0,
                total_images_processed: 0,
                scanner_mode,
            }),
        })
    }

    /// Create a new mobile OpenObscure instance with governance enabled.
    ///
    /// # Arguments
    /// * `config` - Mobile configuration
    /// * `fpe_key` - 32-byte AES-256 key for Format-Preserving Encryption
    /// * `db_path` - Path to SQLite database for consent/retention storage
    /// * `extra_deny_patterns` - Additional file deny patterns (merged with defaults)
    #[cfg(feature = "governance")]
    pub fn new_with_governance(
        config: MobileConfig,
        fpe_key: [u8; 32],
        db_path: &str,
        extra_deny_patterns: &[String],
    ) -> Result<Self, MobileError> {
        let mut instance = Self::new(config, fpe_key)?;
        let file_guard_config = crate::governance::FileGuardConfig {
            extra_deny: extra_deny_patterns.to_vec(),
            allow: Vec::new(),
        };
        let engine = crate::governance::GovernanceEngine::new(db_path, Some(file_guard_config))
            .map_err(Self::gov_err)?;
        instance.governance = Some(engine);
        Ok(instance)
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

    /// Process an image for visual PII (face blur, OCR text blur, EXIF strip).
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
            .process_image(img)
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

    // ── Governance Methods ──

    #[cfg(feature = "governance")]
    fn gov_err(e: crate::governance::GovernanceError) -> MobileError {
        MobileError::Governance(e.to_string())
    }

    #[cfg(feature = "governance")]
    fn gov_engine(&self) -> Result<&crate::governance::GovernanceEngine, MobileError> {
        self.governance
            .as_ref()
            .ok_or(MobileError::GovernanceNotEnabled)
    }

    /// Check if consent is active for a given type.
    #[cfg(feature = "governance")]
    pub fn check_consent(&self, user_id: &str, consent_type: &str) -> Result<bool, MobileError> {
        let engine = self.gov_engine()?;
        let ct = crate::governance::ConsentType::from_str(consent_type).map_err(Self::gov_err)?;
        engine
            .consent_store()
            .has_active_consent(user_id, ct)
            .map_err(Self::gov_err)
    }

    /// Grant consent for a specific type.
    #[cfg(feature = "governance")]
    pub fn grant_consent(
        &self,
        user_id: &str,
        consent_type: &str,
        purpose: Option<&str>,
    ) -> Result<ConsentRecordMobile, MobileError> {
        let engine = self.gov_engine()?;
        let ct = crate::governance::ConsentType::from_str(consent_type).map_err(Self::gov_err)?;
        let record = engine
            .consent_store()
            .grant_consent(user_id, ct, purpose, None)
            .map_err(Self::gov_err)?;
        Ok(ConsentRecordMobile {
            id: record.id,
            consent_type: record.consent_type,
            granted: record.granted,
            version: record.version,
        })
    }

    /// Revoke consent for a specific type.
    #[cfg(feature = "governance")]
    pub fn revoke_consent(&self, user_id: &str, consent_type: &str) -> Result<bool, MobileError> {
        let engine = self.gov_engine()?;
        let ct = crate::governance::ConsentType::from_str(consent_type).map_err(Self::gov_err)?;
        engine
            .consent_store()
            .revoke_consent(user_id, ct)
            .map_err(Self::gov_err)
    }

    /// Check if a file path is safe to access.
    #[cfg(feature = "governance")]
    pub fn check_file_access(&self, path: &str) -> Result<FileCheckResultMobile, MobileError> {
        let engine = self.gov_engine()?;
        let result = engine.file_guard().check_access(path);
        Ok(FileCheckResultMobile {
            allowed: result.allowed,
            reason: result.reason,
        })
    }

    /// Execute a /privacy command. Args are the space-separated tokens after "/privacy".
    #[cfg(feature = "governance")]
    pub fn privacy_command(
        &self,
        user_id: &str,
        args: &[String],
    ) -> Result<PrivacyCommandResultMobile, MobileError> {
        let engine = self.gov_engine()?;
        let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let result = crate::governance::handle_privacy_command(engine, user_id, &args_refs);
        Ok(PrivacyCommandResultMobile {
            text: result.text,
            success: result.success,
        })
    }

    /// Enforce retention policy (promote tiers, prune expired).
    #[cfg(feature = "governance")]
    pub fn enforce_retention(&self) -> Result<EnforceResultMobile, MobileError> {
        let engine = self.gov_engine()?;
        let result = engine.retention().enforce(None).map_err(Self::gov_err)?;
        Ok(EnforceResultMobile {
            promoted: result.promoted,
            pruned: result.pruned,
        })
    }

    /// Get retention tier summary.
    #[cfg(feature = "governance")]
    pub fn retention_summary(&self) -> Result<RetentionSummaryMobile, MobileError> {
        let engine = self.gov_engine()?;
        let summary = engine.retention().get_summary().map_err(Self::gov_err)?;
        Ok(RetentionSummaryMobile {
            hot: summary.hot,
            warm: summary.warm,
            cold: summary.cold,
            expired: summary.expired,
            total: summary.total,
        })
    }

    /// Export all user data as JSON (for DSAR access request).
    #[cfg(feature = "governance")]
    pub fn export_user_data(&self, user_id: &str) -> Result<String, MobileError> {
        let engine = self.gov_engine()?;
        let data = engine
            .consent_store()
            .export_user_data(user_id)
            .map_err(Self::gov_err)?;
        serde_json::to_string_pretty(&data).map_err(|e| MobileError::Serialization(e.to_string()))
    }

    // ── Breach Detection & Compliance Methods ──

    /// Assess processing log entries for anomalous PII activity.
    ///
    /// Queries the governance SQLite processing log, buckets by hour,
    /// and flags outliers above `threshold` standard deviations (default: 3.0).
    #[cfg(feature = "governance")]
    pub fn assess_breach(
        &self,
        threshold: Option<f64>,
    ) -> Result<BreachAssessmentMobile, MobileError> {
        let engine = self.gov_engine()?;
        let entries = engine
            .consent_store()
            .get_all_processing_log(None)
            .map_err(Self::gov_err)?;
        let audit = crate::governance::processing_log_to_audit_entries(&entries);
        let assessment = crate::breach_detect::assess_breach(&audit, threshold.unwrap_or(3.0));
        let anomalies_json = serde_json::to_string(
            &assessment
                .anomalies
                .iter()
                .map(|a| {
                    serde_json::json!({
                        "hour": a.hour,
                        "pii_count": a.pii_count,
                        "expected_mean": a.expected_mean,
                        "sigma_deviation": a.sigma_deviation,
                        "pii_types": a.pii_types,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".to_string());
        Ok(BreachAssessmentMobile {
            risk_level: format!("{:?}", assessment.risk_level).to_lowercase(),
            anomaly_count: assessment.anomalies.len() as u32,
            recommendation: assessment.recommendation,
            anomalies_json,
        })
    }

    /// Generate a GDPR Art. 33 breach notification draft (Markdown).
    #[cfg(feature = "governance")]
    pub fn generate_breach_report(&self) -> Result<String, MobileError> {
        let engine = self.gov_engine()?;
        let entries = engine
            .consent_store()
            .get_all_processing_log(None)
            .map_err(Self::gov_err)?;
        let audit = crate::governance::processing_log_to_audit_entries(&entries);
        let assessment = crate::breach_detect::assess_breach(&audit, 3.0);
        let config = crate::config::ComplianceConfig {
            organization_name: None,
            dpo_email: None,
            dpa_contact: None,
            reports_dir: None,
            retention_days: 365,
        };
        Ok(crate::breach_detect::generate_art33_notification(
            &assessment,
            &config,
        ))
    }

    /// Export audit entries in SIEM format (CEF or LEEF).
    #[cfg(feature = "governance")]
    pub fn export_audit_entries(
        &self,
        format: &str,
        limit: Option<u32>,
    ) -> Result<String, MobileError> {
        let engine = self.gov_engine()?;
        let entries = engine
            .consent_store()
            .get_all_processing_log(limit.map(|l| l as usize))
            .map_err(Self::gov_err)?;
        let audit = crate::governance::processing_log_to_audit_entries(&entries);
        let lines: Vec<String> = audit
            .iter()
            .map(|entry| match format.to_lowercase().as_str() {
                "leef" => crate::compliance::format_leef_line(entry),
                _ => crate::compliance::format_cef_line(entry),
            })
            .collect();
        Ok(lines.join("\n"))
    }

    /// Get a compliance summary from the processing log.
    #[cfg(feature = "governance")]
    pub fn compliance_summary(&self) -> Result<String, MobileError> {
        let engine = self.gov_engine()?;
        let entries = engine
            .consent_store()
            .get_all_processing_log(None)
            .map_err(Self::gov_err)?;
        let audit = crate::governance::processing_log_to_audit_entries(&entries);

        let total_entries = audit.len();
        let total_pii: u64 = audit.iter().filter_map(|e| e.pii_total).sum();

        // Aggregate PII types
        let mut type_counts: std::collections::HashMap<String, u64> =
            std::collections::HashMap::new();
        for entry in &audit {
            if let Some(ref breakdown) = entry.pii_breakdown {
                for pair in breakdown.split(", ") {
                    if let Some((pii_type, count_str)) = pair.split_once('=') {
                        let count: u64 = count_str.parse().unwrap_or(1);
                        *type_counts.entry(pii_type.to_string()).or_insert(0) += count;
                    }
                }
            }
        }

        let mut summary = format!(
            "Compliance Summary\n==================\nTotal processing events: {}\nTotal PII detections: {}\n",
            total_entries, total_pii
        );
        if !type_counts.is_empty() {
            summary.push_str("\nPII Type Breakdown:\n");
            let mut types: Vec<_> = type_counts.into_iter().collect();
            types.sort_by(|a, b| b.1.cmp(&a.1));
            for (pii_type, count) in &types {
                summary.push_str(&format!("  {}: {}\n", pii_type, count));
            }
        }
        Ok(summary)
    }

    // ── Encrypted Storage Methods ──

    /// Encrypt data with AES-256-GCM using a passphrase-derived key (Argon2id).
    ///
    /// Returns an opaque blob: `[4-byte params_len LE][KdfParams JSON][nonce+ciphertext]`.
    /// The host app stores this blob as-is and passes it back to `decrypt_data()`.
    #[cfg(feature = "crypto")]
    pub fn encrypt_data(&self, plaintext: &[u8], passphrase: &str) -> Result<Vec<u8>, MobileError> {
        let params = openobscure_crypto::KdfParams::new();
        let key = openobscure_crypto::derive_key(passphrase, &params)
            .map_err(|e| MobileError::CryptoError(e.to_string()))?;
        let ciphertext = openobscure_crypto::encrypt(&key, plaintext)
            .map_err(|e| MobileError::CryptoError(e.to_string()))?;

        let params_json =
            serde_json::to_vec(&params).map_err(|e| MobileError::Serialization(e.to_string()))?;
        let params_len = (params_json.len() as u32).to_le_bytes();

        let mut blob = Vec::with_capacity(4 + params_json.len() + ciphertext.len());
        blob.extend_from_slice(&params_len);
        blob.extend_from_slice(&params_json);
        blob.extend_from_slice(&ciphertext);
        Ok(blob)
    }

    /// Decrypt data previously encrypted with `encrypt_data()`.
    ///
    /// Reads KdfParams from the blob header, derives the same key from the passphrase,
    /// then decrypts the AES-256-GCM ciphertext.
    #[cfg(feature = "crypto")]
    pub fn decrypt_data(&self, data: &[u8], passphrase: &str) -> Result<Vec<u8>, MobileError> {
        if data.len() < 4 {
            return Err(MobileError::CryptoError("Data too short".to_string()));
        }
        let params_len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        if data.len() < 4 + params_len {
            return Err(MobileError::CryptoError("Truncated KDF params".to_string()));
        }
        let params: openobscure_crypto::KdfParams =
            serde_json::from_slice(&data[4..4 + params_len])
                .map_err(|e| MobileError::CryptoError(format!("Invalid KDF params: {}", e)))?;
        let key = openobscure_crypto::derive_key(passphrase, &params)
            .map_err(|e| MobileError::CryptoError(e.to_string()))?;
        openobscure_crypto::decrypt(&key, &data[4 + params_len..])
            .map_err(|e| MobileError::CryptoError(e.to_string()))
    }

    /// Get current statistics for diagnostics.
    pub fn stats(&self) -> MobileStats {
        let s = self.stats.lock().unwrap();
        MobileStats {
            total_pii_found: s.total_pii_found,
            total_images_processed: s.total_images_processed,
            scanner_mode: s.scanner_mode.clone(),
            image_pipeline_available: self.image_manager.is_some(),
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

    // ── Governance Integration Tests ──

    #[cfg(feature = "governance")]
    mod governance_tests {
        use super::*;

        fn make_governance_mobile() -> OpenObscureMobile {
            OpenObscureMobile::new_with_governance(
                MobileConfig::default(),
                make_test_key(),
                ":memory:",
                &[],
            )
            .unwrap()
        }

        #[test]
        fn test_mobile_consent_grant_revoke() {
            let mobile = make_governance_mobile();
            let record = mobile
                .grant_consent("user1", "processing", Some("Test"))
                .unwrap();
            assert!(record.granted);
            assert_eq!(record.version, 1);

            assert!(mobile.check_consent("user1", "processing").unwrap());

            let revoked = mobile.revoke_consent("user1", "processing").unwrap();
            assert!(revoked);
            assert!(!mobile.check_consent("user1", "processing").unwrap());
        }

        #[test]
        fn test_mobile_file_guard_deny_env() {
            let mobile = make_governance_mobile();
            let result = mobile.check_file_access("/project/.env").unwrap();
            assert!(!result.allowed);
            assert!(result.reason.is_some());
        }

        #[test]
        fn test_mobile_file_guard_allow_normal() {
            let mobile = make_governance_mobile();
            let result = mobile.check_file_access("/project/src/main.rs").unwrap();
            assert!(result.allowed);
            assert!(result.reason.is_none());
        }

        #[test]
        fn test_mobile_privacy_command_status() {
            let mobile = make_governance_mobile();
            let result = mobile
                .privacy_command("user1", &["status".to_string()])
                .unwrap();
            assert!(result.success);
            assert!(result.text.contains("Privacy Status"));
        }

        #[test]
        fn test_mobile_privacy_command_consent() {
            let mobile = make_governance_mobile();
            let result = mobile
                .privacy_command(
                    "user1",
                    &[
                        "consent".to_string(),
                        "grant".to_string(),
                        "processing".to_string(),
                    ],
                )
                .unwrap();
            assert!(result.success);
            assert!(result.text.contains("Consent granted"));
        }

        #[test]
        fn test_mobile_retention_enforce() {
            let mobile = make_governance_mobile();
            let result = mobile.enforce_retention().unwrap();
            assert_eq!(result.promoted, 0);
            assert_eq!(result.pruned, 0);

            let summary = mobile.retention_summary().unwrap();
            assert_eq!(summary.total, 0);
        }

        #[test]
        fn test_mobile_export_user_data() {
            let mobile = make_governance_mobile();
            mobile.grant_consent("user1", "processing", None).unwrap();
            let json = mobile.export_user_data("user1").unwrap();
            assert!(json.contains("user1"));
            assert!(json.contains("processing"));
        }

        #[test]
        fn test_mobile_privacy_command_delete() {
            let mobile = make_governance_mobile();
            mobile.grant_consent("user1", "storage", None).unwrap();
            let result = mobile
                .privacy_command("user1", &["delete".to_string()])
                .unwrap();
            assert!(result.success);
            assert!(result.text.contains("erasure complete"));
        }

        #[test]
        fn test_mobile_governance_disabled() {
            let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
            let result = mobile.check_consent("user1", "processing");
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("not enabled"));
        }

        #[test]
        fn test_mobile_privacy_command_help() {
            let mobile = make_governance_mobile();
            let result = mobile.privacy_command("user1", &[]).unwrap();
            assert!(result.success);
            assert!(result.text.contains("Privacy Commands"));
        }

        // ── Breach Detection & Compliance Tests ──

        #[test]
        fn test_mobile_assess_breach_empty() {
            let mobile = make_governance_mobile();
            let result = mobile.assess_breach(None).unwrap();
            assert_eq!(result.risk_level, "low");
            assert_eq!(result.anomaly_count, 0);
        }

        #[test]
        fn test_mobile_assess_breach_with_data() {
            use crate::governance::ProcessingAction;
            let mobile = make_governance_mobile();
            let engine = mobile.gov_engine().unwrap();
            // Log some processing entries
            for _ in 0..5 {
                engine
                    .consent_store()
                    .log_processing(
                        "user1",
                        ProcessingAction::Scan,
                        Some(&["email", "ssn"]),
                        Some("proxy"),
                        None,
                    )
                    .unwrap();
            }
            let result = mobile.assess_breach(Some(3.0)).unwrap();
            // With only a few entries in the same hour, should be low risk
            assert_eq!(result.risk_level, "low");
            assert!(result.anomalies_json.starts_with('['));
        }

        #[test]
        fn test_mobile_generate_breach_report() {
            let mobile = make_governance_mobile();
            let report = mobile.generate_breach_report().unwrap();
            // Art. 33 notification always produces a markdown doc
            assert!(
                report.contains("Breach")
                    || report.contains("breach")
                    || report.contains("Notification")
            );
        }

        #[test]
        fn test_mobile_export_audit_cef() {
            use crate::governance::ProcessingAction;
            let mobile = make_governance_mobile();
            let engine = mobile.gov_engine().unwrap();
            engine
                .consent_store()
                .log_processing(
                    "user1",
                    ProcessingAction::Encrypt,
                    Some(&["credit_card"]),
                    Some("proxy"),
                    None,
                )
                .unwrap();
            let cef = mobile.export_audit_entries("cef", Some(10)).unwrap();
            assert!(cef.contains("CEF:"));
        }

        #[test]
        fn test_mobile_export_audit_leef() {
            use crate::governance::ProcessingAction;
            let mobile = make_governance_mobile();
            let engine = mobile.gov_engine().unwrap();
            engine
                .consent_store()
                .log_processing(
                    "user1",
                    ProcessingAction::Redact,
                    Some(&["phone"]),
                    Some("scanner"),
                    None,
                )
                .unwrap();
            let leef = mobile.export_audit_entries("leef", None).unwrap();
            assert!(leef.contains("LEEF:"));
        }

        #[test]
        fn test_mobile_compliance_summary() {
            use crate::governance::ProcessingAction;
            let mobile = make_governance_mobile();
            let engine = mobile.gov_engine().unwrap();
            engine
                .consent_store()
                .log_processing(
                    "user1",
                    ProcessingAction::Scan,
                    Some(&["email", "ssn"]),
                    Some("proxy"),
                    None,
                )
                .unwrap();
            engine
                .consent_store()
                .log_processing(
                    "user2",
                    ProcessingAction::Encrypt,
                    Some(&["credit_card"]),
                    Some("proxy"),
                    None,
                )
                .unwrap();
            let summary = mobile.compliance_summary().unwrap();
            assert!(summary.contains("Compliance Summary"));
            assert!(summary.contains("Total processing events: 2"));
            assert!(summary.contains("Total PII detections: 3"));
        }
    }

    // ── Encrypted Storage Tests ──

    #[cfg(feature = "crypto")]
    mod crypto_tests {
        use super::*;

        #[test]
        fn test_mobile_encrypt_decrypt_roundtrip() {
            let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
            let plaintext = b"Sensitive user data to protect";
            let blob = mobile.encrypt_data(plaintext, "my-passphrase").unwrap();
            assert_ne!(&blob, plaintext);
            let decrypted = mobile.decrypt_data(&blob, "my-passphrase").unwrap();
            assert_eq!(decrypted, plaintext);
        }

        #[test]
        fn test_mobile_encrypt_wrong_passphrase() {
            let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
            let blob = mobile.encrypt_data(b"secret", "correct-pass").unwrap();
            let result = mobile.decrypt_data(&blob, "wrong-pass");
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("Crypto error"));
        }

        #[test]
        fn test_mobile_encrypt_empty_data() {
            let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
            let blob = mobile.encrypt_data(b"", "passphrase").unwrap();
            let decrypted = mobile.decrypt_data(&blob, "passphrase").unwrap();
            assert!(decrypted.is_empty());
        }

        #[test]
        fn test_mobile_decrypt_truncated_data() {
            let mobile = OpenObscureMobile::new(MobileConfig::default(), make_test_key()).unwrap();
            // Too short — less than 4 bytes
            let result = mobile.decrypt_data(&[0, 1], "pass");
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("too short"));

            // Header says 100 bytes of params but only 10 total
            let result = mobile.decrypt_data(&[100, 0, 0, 0, 0, 0, 0, 0, 0, 0], "pass");
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("Truncated"));
        }
    }
}

//! UniFFI bindings for OpenObscure mobile library.
//!
//! These bindings are compiled only when the `mobile` feature is enabled.
//! UniFFI generates idiomatic Swift and Kotlin wrappers automatically.
//!
//! # Usage
//!
//! Swift (iOS):
//! ```swift
//! let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: "aa".repeated(32))
//! let result = sanitizeText(handle: handle, text: "Card 4111-1111-1111-1111")
//! print(result.sanitizedText) // PII encrypted
//! ```
//!
//! Kotlin (Android):
//! ```kotlin
//! val handle = createOpenobscure(configJson = "{}", fpeKeyHex = "aa".repeat(32))
//! val result = sanitizeText(handle = handle, text = "Card 4111-1111-1111-1111")
//! println(result.sanitizedText) // PII encrypted
//! ```

#[cfg(feature = "mobile")]
use std::sync::Arc;

#[cfg(feature = "mobile")]
use crate::lib_mobile::{MobileConfig, MobileError, OpenObscureMobile};

// Re-export for UniFFI scaffolding
#[cfg(feature = "mobile")]
uniffi::setup_scaffolding!();

/// Create a new OpenObscure mobile instance.
///
/// # Arguments
/// * `config_json` - JSON string with mobile configuration. Pass `"{}"` for defaults.
/// * `fpe_key_hex` - 64-character hex string encoding the 32-byte FPE key.
///
/// # Returns
/// An opaque handle to use with other functions.
#[cfg(feature = "mobile")]
#[uniffi::export]
pub fn create_openobscure(
    config_json: String,
    fpe_key_hex: String,
) -> Result<Arc<OpenObscureMobile>, MobileBindingError> {
    let config: MobileConfig = serde_json::from_str(&config_json)
        .map_err(|e| MobileBindingError::Config(e.to_string()))?;

    let key_bytes = hex::decode(fpe_key_hex.trim())
        .map_err(|e| MobileBindingError::InvalidKey(format!("Bad hex: {}", e)))?;
    if key_bytes.len() != 32 {
        return Err(MobileBindingError::InvalidKey(format!(
            "Expected 32 bytes, got {}",
            key_bytes.len()
        )));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&key_bytes);

    let mobile = OpenObscureMobile::new(config, key)
        .map_err(|e| MobileBindingError::Init(e.to_string()))?;

    Ok(Arc::new(mobile))
}

/// Scan text for PII and encrypt matches with FF1 FPE.
///
/// Returns a result with sanitized text and mapping data for later restoration.
#[cfg(feature = "mobile")]
#[uniffi::export]
pub fn sanitize_text(
    handle: &Arc<OpenObscureMobile>,
    text: String,
) -> Result<SanitizeResultFFI, MobileBindingError> {
    let result = handle
        .sanitize_text(&text)
        .map_err(|e| MobileBindingError::Processing(e.to_string()))?;

    Ok(SanitizeResultFFI {
        sanitized_text: result.sanitized_text,
        pii_count: result.pii_count,
        categories: result.categories,
        mapping_json: result.mapping_json,
    })
}

/// Restore original PII values in response text using saved mappings.
#[cfg(feature = "mobile")]
#[uniffi::export]
pub fn restore_text(
    handle: &Arc<OpenObscureMobile>,
    text: String,
    mapping_json: String,
) -> String {
    handle.restore_text(&text, &mapping_json)
}

/// Process an image for visual PII (face blur, OCR text blur, EXIF strip).
///
/// Returns sanitized image bytes (JPEG format).
#[cfg(feature = "mobile")]
#[uniffi::export]
pub fn sanitize_image(
    handle: &Arc<OpenObscureMobile>,
    image_bytes: Vec<u8>,
) -> Result<Vec<u8>, MobileBindingError> {
    handle
        .sanitize_image(&image_bytes)
        .map_err(|e| MobileBindingError::Processing(e.to_string()))
}

/// Get current statistics for diagnostics.
#[cfg(feature = "mobile")]
#[uniffi::export]
pub fn get_stats(handle: &Arc<OpenObscureMobile>) -> MobileStatsFFI {
    let stats = handle.stats();
    MobileStatsFFI {
        total_pii_found: stats.total_pii_found,
        total_images_processed: stats.total_images_processed,
        scanner_mode: stats.scanner_mode,
        image_pipeline_available: stats.image_pipeline_available,
    }
}

// ---- Governance FFI exports (requires mobile-full feature) ----

/// Create a new OpenObscure mobile instance with governance enabled.
///
/// # Arguments
/// * `config_json` - JSON string with mobile configuration
/// * `fpe_key_hex` - 64-character hex string encoding the 32-byte FPE key
/// * `db_path` - Path to SQLite database for consent/retention storage
/// * `extra_deny_patterns` - Additional file deny regex patterns
#[cfg(all(feature = "mobile", feature = "governance"))]
#[uniffi::export]
pub fn create_openobscure_with_governance(
    config_json: String,
    fpe_key_hex: String,
    db_path: String,
    extra_deny_patterns: Vec<String>,
) -> Result<Arc<OpenObscureMobile>, MobileBindingError> {
    let config: MobileConfig = serde_json::from_str(&config_json)
        .map_err(|e| MobileBindingError::Config(e.to_string()))?;

    let key_bytes = hex::decode(fpe_key_hex.trim())
        .map_err(|e| MobileBindingError::InvalidKey(format!("Bad hex: {}", e)))?;
    if key_bytes.len() != 32 {
        return Err(MobileBindingError::InvalidKey(format!(
            "Expected 32 bytes, got {}",
            key_bytes.len()
        )));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&key_bytes);

    let mobile = OpenObscureMobile::new_with_governance(config, key, &db_path, &extra_deny_patterns)
        .map_err(|e| MobileBindingError::Init(e.to_string()))?;

    Ok(Arc::new(mobile))
}

/// Check if consent is active for a given type.
#[cfg(all(feature = "mobile", feature = "governance"))]
#[uniffi::export]
pub fn check_consent(
    handle: &Arc<OpenObscureMobile>,
    user_id: String,
    consent_type: String,
) -> Result<bool, MobileBindingError> {
    handle.check_consent(&user_id, &consent_type)
        .map_err(|e| MobileBindingError::Processing(e.to_string()))
}

/// Grant consent for a specific type.
#[cfg(all(feature = "mobile", feature = "governance"))]
#[uniffi::export]
pub fn grant_consent(
    handle: &Arc<OpenObscureMobile>,
    user_id: String,
    consent_type: String,
    purpose: Option<String>,
) -> Result<ConsentRecordFFI, MobileBindingError> {
    let record = handle.grant_consent(&user_id, &consent_type, purpose.as_deref())
        .map_err(|e| MobileBindingError::Processing(e.to_string()))?;
    Ok(ConsentRecordFFI {
        id: record.id,
        consent_type: record.consent_type,
        granted: record.granted,
        version: record.version,
    })
}

/// Revoke consent for a specific type.
#[cfg(all(feature = "mobile", feature = "governance"))]
#[uniffi::export]
pub fn revoke_consent(
    handle: &Arc<OpenObscureMobile>,
    user_id: String,
    consent_type: String,
) -> Result<bool, MobileBindingError> {
    handle.revoke_consent(&user_id, &consent_type)
        .map_err(|e| MobileBindingError::Processing(e.to_string()))
}

/// Check if a file path is safe to access.
#[cfg(all(feature = "mobile", feature = "governance"))]
#[uniffi::export]
pub fn check_file_access(
    handle: &Arc<OpenObscureMobile>,
    path: String,
) -> Result<FileCheckResultFFI, MobileBindingError> {
    let result = handle.check_file_access(&path)
        .map_err(|e| MobileBindingError::Processing(e.to_string()))?;
    Ok(FileCheckResultFFI {
        allowed: result.allowed,
        reason: result.reason,
    })
}

/// Execute a /privacy command. Args are the tokens after "/privacy".
#[cfg(all(feature = "mobile", feature = "governance"))]
#[uniffi::export]
pub fn privacy_command(
    handle: &Arc<OpenObscureMobile>,
    user_id: String,
    args: Vec<String>,
) -> Result<PrivacyCommandResultFFI, MobileBindingError> {
    let result = handle.privacy_command(&user_id, &args)
        .map_err(|e| MobileBindingError::Processing(e.to_string()))?;
    Ok(PrivacyCommandResultFFI {
        text: result.text,
        success: result.success,
    })
}

/// Enforce retention policy (promote tiers, prune expired).
#[cfg(all(feature = "mobile", feature = "governance"))]
#[uniffi::export]
pub fn enforce_retention(
    handle: &Arc<OpenObscureMobile>,
) -> Result<EnforceResultFFI, MobileBindingError> {
    let result = handle.enforce_retention()
        .map_err(|e| MobileBindingError::Processing(e.to_string()))?;
    Ok(EnforceResultFFI {
        promoted: result.promoted,
        pruned: result.pruned,
    })
}

/// Get retention tier summary.
#[cfg(all(feature = "mobile", feature = "governance"))]
#[uniffi::export]
pub fn retention_summary(
    handle: &Arc<OpenObscureMobile>,
) -> Result<RetentionSummaryFFI, MobileBindingError> {
    let summary = handle.retention_summary()
        .map_err(|e| MobileBindingError::Processing(e.to_string()))?;
    Ok(RetentionSummaryFFI {
        hot: summary.hot,
        warm: summary.warm,
        cold: summary.cold,
        expired: summary.expired,
        total: summary.total,
    })
}

/// Export all user data as JSON (for DSAR access request).
#[cfg(all(feature = "mobile", feature = "governance"))]
#[uniffi::export]
pub fn export_user_data(
    handle: &Arc<OpenObscureMobile>,
    user_id: String,
) -> Result<String, MobileBindingError> {
    handle.export_user_data(&user_id)
        .map_err(|e| MobileBindingError::Processing(e.to_string()))
}

// ---- Breach Detection & Compliance FFI exports ----

/// Assess processing log for anomalous PII activity (breach detection).
///
/// Returns a risk assessment with anomaly count and recommendations.
/// `threshold` controls sensitivity (default: 3.0 sigma).
#[cfg(all(feature = "mobile", feature = "governance"))]
#[uniffi::export]
pub fn assess_breach(
    handle: &Arc<OpenObscureMobile>,
    threshold: Option<f64>,
) -> Result<BreachAssessmentFFI, MobileBindingError> {
    let result = handle.assess_breach(threshold)
        .map_err(|e| MobileBindingError::Processing(e.to_string()))?;
    Ok(BreachAssessmentFFI {
        risk_level: result.risk_level,
        anomaly_count: result.anomaly_count,
        recommendation: result.recommendation,
        anomalies_json: result.anomalies_json,
    })
}

/// Generate a GDPR Art. 33 breach notification draft (Markdown).
#[cfg(all(feature = "mobile", feature = "governance"))]
#[uniffi::export]
pub fn generate_breach_report(
    handle: &Arc<OpenObscureMobile>,
) -> Result<String, MobileBindingError> {
    handle.generate_breach_report()
        .map_err(|e| MobileBindingError::Processing(e.to_string()))
}

/// Export audit entries in SIEM format ("cef" or "leef").
#[cfg(all(feature = "mobile", feature = "governance"))]
#[uniffi::export]
pub fn export_audit_entries(
    handle: &Arc<OpenObscureMobile>,
    format: String,
    limit: Option<u32>,
) -> Result<String, MobileBindingError> {
    handle.export_audit_entries(&format, limit)
        .map_err(|e| MobileBindingError::Processing(e.to_string()))
}

/// Get a compliance summary from the processing log.
#[cfg(all(feature = "mobile", feature = "governance"))]
#[uniffi::export]
pub fn compliance_summary(
    handle: &Arc<OpenObscureMobile>,
) -> Result<String, MobileBindingError> {
    handle.compliance_summary()
        .map_err(|e| MobileBindingError::Processing(e.to_string()))
}

// ---- Encrypted Storage FFI exports (requires mobile + crypto) ----

/// Encrypt data with AES-256-GCM using a passphrase-derived key (Argon2id).
///
/// Returns an opaque blob that the host app stores and passes back to `decrypt_data()`.
#[cfg(all(feature = "mobile", feature = "crypto"))]
#[uniffi::export]
pub fn encrypt_data(
    handle: &Arc<OpenObscureMobile>,
    plaintext: Vec<u8>,
    passphrase: String,
) -> Result<Vec<u8>, MobileBindingError> {
    handle.encrypt_data(&plaintext, &passphrase)
        .map_err(|e| MobileBindingError::Processing(e.to_string()))
}

/// Decrypt data previously encrypted with `encrypt_data()`.
#[cfg(all(feature = "mobile", feature = "crypto"))]
#[uniffi::export]
pub fn decrypt_data(
    handle: &Arc<OpenObscureMobile>,
    data: Vec<u8>,
    passphrase: String,
) -> Result<Vec<u8>, MobileBindingError> {
    handle.decrypt_data(&data, &passphrase)
        .map_err(|e| MobileBindingError::Processing(e.to_string()))
}

// ---- FFI-safe types for UniFFI ----

/// Result of sanitizing text, exposed to Swift/Kotlin via UniFFI.
#[cfg(feature = "mobile")]
#[derive(uniffi::Record)]
pub struct SanitizeResultFFI {
    pub sanitized_text: String,
    pub pii_count: u32,
    pub categories: Vec<String>,
    pub mapping_json: String,
}

/// Statistics exposed to Swift/Kotlin via UniFFI.
#[cfg(feature = "mobile")]
#[derive(uniffi::Record)]
pub struct MobileStatsFFI {
    pub total_pii_found: u64,
    pub total_images_processed: u64,
    pub scanner_mode: String,
    pub image_pipeline_available: bool,
}

/// Consent record exposed to Swift/Kotlin via UniFFI.
#[cfg(all(feature = "mobile", feature = "governance"))]
#[derive(uniffi::Record)]
pub struct ConsentRecordFFI {
    pub id: i64,
    pub consent_type: String,
    pub granted: bool,
    pub version: i64,
}

/// File access check result exposed to Swift/Kotlin via UniFFI.
#[cfg(all(feature = "mobile", feature = "governance"))]
#[derive(uniffi::Record)]
pub struct FileCheckResultFFI {
    pub allowed: bool,
    pub reason: Option<String>,
}

/// Privacy command result exposed to Swift/Kotlin via UniFFI.
#[cfg(all(feature = "mobile", feature = "governance"))]
#[derive(uniffi::Record)]
pub struct PrivacyCommandResultFFI {
    pub text: String,
    pub success: bool,
}

/// Retention enforcement result exposed to Swift/Kotlin via UniFFI.
#[cfg(all(feature = "mobile", feature = "governance"))]
#[derive(uniffi::Record)]
pub struct EnforceResultFFI {
    pub promoted: u32,
    pub pruned: u32,
}

/// Retention summary exposed to Swift/Kotlin via UniFFI.
#[cfg(all(feature = "mobile", feature = "governance"))]
#[derive(uniffi::Record)]
pub struct RetentionSummaryFFI {
    pub hot: u32,
    pub warm: u32,
    pub cold: u32,
    pub expired: u32,
    pub total: u32,
}

/// Breach assessment result exposed to Swift/Kotlin via UniFFI.
#[cfg(all(feature = "mobile", feature = "governance"))]
#[derive(uniffi::Record)]
pub struct BreachAssessmentFFI {
    pub risk_level: String,
    pub anomaly_count: u32,
    pub recommendation: String,
    pub anomalies_json: String,
}

/// Error type exposed to Swift/Kotlin via UniFFI.
#[cfg(feature = "mobile")]
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum MobileBindingError {
    #[error("Invalid configuration: {0}")]
    Config(String),
    #[error("Invalid FPE key: {0}")]
    InvalidKey(String),
    #[error("Initialization failed: {0}")]
    Init(String),
    #[error("Processing error: {0}")]
    Processing(String),
}

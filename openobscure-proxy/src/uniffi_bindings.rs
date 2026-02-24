//! UniFFI bindings for OpenObscure mobile library.
//!
//! These bindings are compiled only when the `mobile` feature is enabled.
//! UniFFI generates idiomatic Swift and Kotlin wrappers automatically.
//!
//! # Architecture
//!
//! `OpenObscureHandle` is a thin UniFFI-visible wrapper around `OpenObscureMobile`.
//! The derive lives here (not on `OpenObscureMobile` itself) because this module
//! is only compiled in the lib crate where `uniffi::setup_scaffolding!()` runs.
//! The binary crate (main.rs) never compiles this module, avoiding the
//! `UniFfiTag not found` error.
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
use crate::lib_mobile::{MobileConfig, OpenObscureMobile};

/// Opaque handle exposed to Swift/Kotlin via UniFFI.
///
/// Wraps `OpenObscureMobile` so the `uniffi::Object` derive lives in the lib
/// crate (where `UniFfiTag` exists) rather than on the shared struct.
#[cfg(feature = "mobile")]
#[derive(uniffi::Object)]
pub struct OpenObscureHandle {
    inner: OpenObscureMobile,
}

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
) -> Result<Arc<OpenObscureHandle>, MobileBindingError> {
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

    let mobile =
        OpenObscureMobile::new(config, key).map_err(|e| MobileBindingError::Init(e.to_string()))?;

    Ok(Arc::new(OpenObscureHandle { inner: mobile }))
}

/// Scan text for PII and encrypt matches with FF1 FPE.
///
/// Returns a result with sanitized text and mapping data for later restoration.
#[cfg(feature = "mobile")]
#[uniffi::export]
pub fn sanitize_text(
    handle: &Arc<OpenObscureHandle>,
    text: String,
) -> Result<SanitizeResultFFI, MobileBindingError> {
    let result = handle
        .inner
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
pub fn restore_text(handle: &Arc<OpenObscureHandle>, text: String, mapping_json: String) -> String {
    handle.inner.restore_text(&text, &mapping_json)
}

/// Process an image for visual PII (face redaction, OCR text redaction, EXIF strip).
///
/// Returns sanitized image bytes (JPEG format).
#[cfg(feature = "mobile")]
#[uniffi::export]
pub fn sanitize_image(
    handle: &Arc<OpenObscureHandle>,
    image_bytes: Vec<u8>,
) -> Result<Vec<u8>, MobileBindingError> {
    handle
        .inner
        .sanitize_image(&image_bytes)
        .map_err(|e| MobileBindingError::Processing(e.to_string()))
}

/// Get current statistics for diagnostics.
#[cfg(feature = "mobile")]
#[uniffi::export]
pub fn get_stats(handle: &Arc<OpenObscureHandle>) -> MobileStatsFFI {
    let stats = handle.inner.stats();
    MobileStatsFFI {
        total_pii_found: stats.total_pii_found,
        total_images_processed: stats.total_images_processed,
        scanner_mode: stats.scanner_mode,
        image_pipeline_available: stats.image_pipeline_available,
        device_tier: stats.device_tier,
    }
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
    /// Device capability tier ("full", "standard", "lite", or "manual").
    pub device_tier: String,
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

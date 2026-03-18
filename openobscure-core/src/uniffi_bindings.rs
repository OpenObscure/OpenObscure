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
/// `uniffi::Object` requires the derive to live in the crate that calls
/// `uniffi::setup_scaffolding!()` (this lib crate, via `UniFfiTag`). We therefore
/// wrap `OpenObscureMobile` here rather than deriving `uniffi::Object` on the
/// struct itself, keeping the heavy implementation out of the FFI layer.
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

/// Sanitize a full conversation history in one call.
///
/// User and system messages are sanitized; assistant messages pass through
/// unchanged (they already contain FPE tokens from DB). The same plaintext
/// value gets the same token across turns — pass `existingMappingJson` from
/// the previous call's `mappingJson` output (or `"[]"` for the first call).
///
/// Use this instead of calling `sanitize_text()` per user message.
#[cfg(feature = "mobile")]
#[uniffi::export]
pub fn sanitize_messages(
    handle: &Arc<OpenObscureHandle>,
    messages: Vec<ChatMessageFfi>,
    existing_mapping_json: String,
) -> Result<SanitizeMessagesResultFfi, MobileBindingError> {
    let input: Vec<crate::lib_mobile::ChatMessage> = messages
        .into_iter()
        .map(|m| crate::lib_mobile::ChatMessage {
            role: m.role,
            content: m.content,
        })
        .collect();

    let result = handle
        .inner
        .sanitize_messages(&input, &existing_mapping_json)
        .map_err(|e| MobileBindingError::Processing(e.to_string()))?;

    Ok(SanitizeMessagesResultFfi {
        messages: result
            .messages
            .into_iter()
            .map(|m| ChatMessageFfi {
                role: m.role,
                content: m.content,
            })
            .collect(),
        pii_count: result.pii_count,
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

/// Scan a transcript (from platform speech API) for PII and encrypt matches.
///
/// Used by iOS `SFSpeechRecognizer` and Android `SpeechRecognizer` voice pipelines.
/// The mobile app transcribes audio locally, then calls this to detect/encrypt PII.
#[cfg(feature = "mobile")]
#[uniffi::export]
pub fn sanitize_audio_transcript(
    handle: &Arc<OpenObscureHandle>,
    transcript: String,
) -> Result<SanitizeResultFFI, MobileBindingError> {
    let result = handle
        .inner
        .sanitize_audio_transcript(&transcript)
        .map_err(|e| MobileBindingError::Processing(e.to_string()))?;

    Ok(SanitizeResultFFI {
        sanitized_text: result.sanitized_text,
        pii_count: result.pii_count,
        categories: result.categories,
        mapping_json: result.mapping_json,
    })
}

/// Check if a transcript contains PII without encrypting.
///
/// Returns the count of PII matches. Use to decide whether to strip an audio block.
#[cfg(feature = "mobile")]
#[uniffi::export]
pub fn check_audio_pii(handle: &Arc<OpenObscureHandle>, transcript: String) -> u32 {
    handle.inner.check_audio_pii(&transcript)
}

/// Scan a response for persuasion and manipulation techniques (cognitive firewall).
///
/// Returns `Some(RiReportFFI)` if manipulation is detected, `None` if clean or disabled.
#[cfg(feature = "mobile")]
#[uniffi::export]
pub fn scan_response(
    handle: &Arc<OpenObscureHandle>,
    response_text: String,
) -> Option<RiReportFFI> {
    let report = handle.inner.scan_response(&response_text)?;
    Some(RiReportFFI {
        severity: report.severity,
        categories: report.categories,
        flags: report.flags,
        r2_categories: report.r2_categories,
        scan_time_us: report.scan_time_us,
    })
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

/// Get buffered debug log messages from the Rust layer.
///
/// Returns all accumulated messages since last call, then clears the buffer.
/// Use this for diagnostics — call after `createOpenobscure` or `sanitizeImage`
/// and print the result in Swift/Kotlin.
#[cfg(feature = "mobile")]
#[uniffi::export]
pub fn get_debug_log() -> String {
    crate::lib_mobile::drain_debug_log()
}

// ---- FFI-safe types for UniFFI ----

/// A single chat message for multi-message sanitization.
#[cfg(feature = "mobile")]
#[derive(uniffi::Record)]
pub struct ChatMessageFfi {
    pub role: String,
    pub content: String,
}

/// Result of sanitizing a full conversation history.
#[cfg(feature = "mobile")]
#[derive(uniffi::Record)]
pub struct SanitizeMessagesResultFfi {
    pub messages: Vec<ChatMessageFfi>,
    pub pii_count: u32,
    pub mapping_json: String,
}

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
    /// Device capability tier: "full", "standard", or "lite".
    pub device_tier: String,
}

/// Response integrity report exposed to Swift/Kotlin via UniFFI.
#[cfg(feature = "mobile")]
#[derive(uniffi::Record)]
pub struct RiReportFFI {
    /// Severity tier: "Notice", "Warning", or "Caution".
    pub severity: String,
    /// Persuasion categories detected (e.g. "Urgency", "Fear", "Authority").
    pub categories: Vec<String>,
    /// Matched phrases from R1 dictionary scan.
    pub flags: Vec<String>,
    /// Article 5 categories detected by R2 classifier (if model loaded).
    pub r2_categories: Vec<String>,
    /// Scan duration in microseconds.
    pub scan_time_us: u64,
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

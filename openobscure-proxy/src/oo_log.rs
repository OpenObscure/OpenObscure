//! OpenObscure Unified Logging API — `oo_log!` macros.
//!
//! Every module calls these macros instead of `tracing::*` directly.
//! Benefits:
//! - Auto-injects `oo_module` field for per-module filtering
//! - All events route through the PII scrub subscriber layer
//! - `oo_audit!` events are tagged for the GDPR audit log layer
//! - Module name constants prevent typos
//!
//! Usage:
//!   oo_info!(modules::SCANNER, "PII encrypted", pii_total = 3);
//!   oo_warn!(modules::HEALTH, "Proxy degraded", error = %e);
//!   oo_audit!(modules::PROXY, "grant", user_id = "u123");

/// Module name constants — use these instead of string literals.
pub mod modules {
    pub const PROXY: &str = "proxy";
    pub const SCANNER: &str = "scanner";
    pub const HYBRID: &str = "hybrid_scanner";
    pub const FPE: &str = "fpe";
    pub const VAULT: &str = "vault";
    pub const HEALTH: &str = "health";
    pub const CONFIG: &str = "config";
    pub const NER: &str = "ner";
    pub const CRF: &str = "crf";
    pub const BODY: &str = "body";
    pub const SERVER: &str = "server";
    pub const MAPPING: &str = "mapping";
    pub const IMAGE: &str = "image_pipeline";
    pub const FACE: &str = "face_detector";
    pub const OCR: &str = "ocr_engine";
    pub const SCREEN: &str = "screen_guard";
    pub const DEVICE: &str = "device_profile";
    #[allow(dead_code)]
    pub const WATCHDOG: &str = "watchdog";
}

// Macro design: the message is the SECOND argument (for readability at call sites),
// but tracing requires it LAST. The "with fields" arm captures remaining tokens as
// raw `tt` to pass through tracing's `%`/`?` sigils unchanged, then appends the
// message at the end.

/// Log at ERROR level with module tag.
#[macro_export]
macro_rules! oo_error {
    ($module:expr, $msg:expr, $($rest:tt)+) => {
        tracing::error!(oo_module = $module, $($rest)+, $msg)
    };
    ($module:expr, $msg:expr) => {
        tracing::error!(oo_module = $module, $msg)
    };
}

/// Log at WARN level with module tag.
#[macro_export]
macro_rules! oo_warn {
    ($module:expr, $msg:expr, $($rest:tt)+) => {
        tracing::warn!(oo_module = $module, $($rest)+, $msg)
    };
    ($module:expr, $msg:expr) => {
        tracing::warn!(oo_module = $module, $msg)
    };
}

/// Log at INFO level with module tag.
#[macro_export]
macro_rules! oo_info {
    ($module:expr, $msg:expr, $($rest:tt)+) => {
        tracing::info!(oo_module = $module, $($rest)+, $msg)
    };
    ($module:expr, $msg:expr) => {
        tracing::info!(oo_module = $module, $msg)
    };
}

/// Log at DEBUG level with module tag.
#[macro_export]
macro_rules! oo_debug {
    ($module:expr, $msg:expr, $($rest:tt)+) => {
        tracing::debug!(oo_module = $module, $($rest)+, $msg)
    };
    ($module:expr, $msg:expr) => {
        tracing::debug!(oo_module = $module, $msg)
    };
}

/// Log at TRACE level with module tag.
#[macro_export]
macro_rules! oo_trace {
    ($module:expr, $msg:expr, $($rest:tt)+) => {
        tracing::trace!(oo_module = $module, $($rest)+, $msg)
    };
    ($module:expr, $msg:expr) => {
        tracing::trace!(oo_module = $module, $msg)
    };
}

/// GDPR audit log entry — tagged with `oo_audit = true` so only
/// the audit log Layer captures it.
#[macro_export]
macro_rules! oo_audit {
    ($module:expr, $op:expr, $($rest:tt)+) => {
        tracing::info!(oo_module = $module, oo_audit = true, operation = $op, $($rest)+, "audit")
    };
    ($module:expr, $op:expr) => {
        tracing::info!(oo_module = $module, oo_audit = true, operation = $op, "audit")
    };
}

#[cfg(test)]
mod tests {
    use super::modules;

    #[test]
    fn test_oo_info_no_fields() {
        oo_info!(modules::CONFIG, "Config loaded");
    }

    #[test]
    fn test_oo_info_with_fields() {
        oo_info!(
            modules::SCANNER,
            "PII encrypted",
            pii_total = 3,
            breakdown = "ssn=1"
        );
    }

    #[test]
    fn test_oo_warn_with_display_field() {
        let err = "connection refused";
        oo_warn!(modules::HEALTH, "Proxy degraded", error = %err, failures = 2);
    }

    #[test]
    fn test_oo_error_with_debug_field() {
        let details = vec!["a", "b"];
        oo_error!(modules::PROXY, "Request failed", details = ?details);
    }

    #[test]
    fn test_oo_debug_no_fields() {
        oo_debug!(modules::FPE, "FF1 encrypt cycle");
    }

    #[test]
    fn test_oo_audit_with_fields() {
        oo_audit!(modules::VAULT, "encrypt", transcript_id = "t456");
    }

    #[test]
    fn test_oo_audit_multiple_fields() {
        oo_audit!(
            modules::SCANNER,
            "scan",
            pii_count = 3,
            types = "ssn=1, email=2"
        );
    }

    #[test]
    fn test_oo_audit_no_fields() {
        oo_audit!(modules::HEALTH, "check");
    }

    #[test]
    fn test_all_modules_exist() {
        let _mods = [
            modules::PROXY,
            modules::SCANNER,
            modules::HYBRID,
            modules::FPE,
            modules::VAULT,
            modules::HEALTH,
            modules::CONFIG,
            modules::NER,
            modules::CRF,
            modules::BODY,
            modules::SERVER,
            modules::MAPPING,
            modules::IMAGE,
            modules::FACE,
            modules::OCR,
            modules::SCREEN,
            modules::DEVICE,
            modules::WATCHDOG,
        ];
    }
}

use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::path::Path;

/// Controls proxy behavior when body processing (JSON parse, FPE) fails.
/// - Open: forward original body unmodified (default — never block AI functionality)
/// - Closed: reject the request with 502 (strict privacy — no unscanned traffic)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FailMode {
    #[default]
    Open,
    Closed,
}

impl fmt::Display for FailMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FailMode::Open => write!(f, "open"),
            FailMode::Closed => write!(f, "closed"),
        }
    }
}

impl<'de> Deserialize<'de> for FailMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.to_lowercase().as_str() {
            "open" => Ok(FailMode::Open),
            "closed" => Ok(FailMode::Closed),
            other => Err(serde::de::Error::custom(format!(
                "invalid fail_mode '{}': expected 'open' or 'closed'",
                other
            ))),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub fpe: FpeConfig,
    #[serde(default)]
    pub scanner: ScannerConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub image: ImageConfig,
    #[serde(default)]
    pub voice: VoiceConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ProxyConfig {
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_timeout")]
    pub request_timeout_secs: u64,
    #[serde(default = "default_max_body")]
    pub max_body_bytes: usize,
    /// "open" (default): forward original body on processing errors.
    /// "closed": reject request with 502 if body processing fails.
    #[serde(default)]
    pub fail_mode: FailMode,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            listen_addr: default_listen_addr(),
            port: default_port(),
            request_timeout_secs: default_timeout(),
            max_body_bytes: default_max_body(),
            fail_mode: FailMode::default(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct ProviderConfig {
    pub upstream_url: String,
    pub route_prefix: String,
    #[serde(default)]
    pub strip_headers: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct FpeConfig {
    #[serde(default = "default_keychain_service")]
    pub keychain_service: String,
    #[serde(default = "default_keychain_user")]
    pub keychain_user: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub type_overrides: HashMap<String, bool>,
}

impl Default for FpeConfig {
    fn default() -> Self {
        Self {
            keychain_service: default_keychain_service(),
            keychain_user: default_keychain_user(),
            enabled: true,
            type_overrides: HashMap::new(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct ScannerConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub custom_patterns: HashMap<String, CustomPattern>,
    #[serde(default = "default_skip_fields")]
    pub skip_fields: Vec<String>,
    /// Enable health/child keyword dictionary scanning (Phase 2).
    #[serde(default = "default_true")]
    pub keywords_enabled: bool,
    /// Enable TinyBERT NER model scanning (Phase 2).
    #[serde(default = "default_true")]
    pub ner_enabled: bool,
    /// NER confidence threshold (0.0–1.0). Matches below this are logged but not reported.
    #[serde(default = "default_ner_confidence")]
    pub ner_confidence_threshold: f32,
    /// Path to the NER model directory (containing model_int8.onnx + vocab.txt).
    #[serde(default)]
    pub ner_model_dir: Option<String>,
    /// Scanner mode: "auto" (default), "ner", "crf", "regex".
    /// - auto: use NER if model available + RAM ≥ threshold, else CRF if available, else regex-only
    /// - ner: force NER (fail to regex-only if model unavailable)
    /// - crf: force CRF (fail to regex-only if model unavailable)
    /// - regex: regex + keywords only, no semantic scanning
    #[serde(default = "default_scanner_mode")]
    pub scanner_mode: String,
    /// Path to the CRF model directory (containing crf_model.json).
    #[serde(default)]
    pub crf_model_dir: Option<String>,
    /// RAM threshold in MB for auto NER→CRF fallback (default 200).
    ///
    /// In "auto" scanner mode, the device profiler's tier system now takes
    /// precedence. This field is retained for backward compatibility.
    #[serde(default = "default_ram_threshold")]
    pub ram_threshold_mb: u64,
    /// Skip PII scanning inside markdown code fences and inline code (default: true).
    #[serde(default = "default_true")]
    pub respect_code_fences: bool,
    /// Minimum confidence threshold for ensemble voting (0.0–1.0, default 0.5).
    #[serde(default = "default_min_confidence")]
    pub min_confidence: f32,
    /// Confidence bonus when ≥2 scanners agree on a type at an overlapping span (default 0.15).
    #[serde(default = "default_agreement_bonus")]
    pub agreement_bonus: f32,
}

impl Default for ScannerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            custom_patterns: HashMap::new(),
            skip_fields: default_skip_fields(),
            keywords_enabled: true,
            ner_enabled: true,
            ner_confidence_threshold: default_ner_confidence(),
            ner_model_dir: None,
            scanner_mode: default_scanner_mode(),
            crf_model_dir: None,
            ram_threshold_mb: default_ram_threshold(),
            respect_code_fences: true,
            min_confidence: default_min_confidence(),
            agreement_bonus: default_agreement_bonus(),
        }
    }
}

fn default_min_confidence() -> f32 {
    0.5
}

fn default_agreement_bonus() -> f32 {
    0.15
}

#[derive(Debug, Deserialize, Clone)]
pub struct CustomPattern {
    pub regex: String,
    pub radix: u32,
    pub alphabet: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LoggingConfig {
    /// Log level: trace, debug, info, warn, error (default: "info").
    #[serde(default = "default_log_level")]
    pub level: String,
    /// Emit logs as JSON (default: false — human-readable format).
    #[serde(default)]
    pub json_output: bool,
    /// Path to log file. If unset, logs go to stderr only.
    #[serde(default)]
    pub file_path: Option<String>,
    /// Max log file size in bytes before rotation (default: 10MB).
    #[serde(default = "default_log_max_size")]
    pub max_file_size: u64,
    /// Number of rotated log files to keep (default: 3).
    #[serde(default = "default_log_max_files")]
    pub max_files: u32,
    /// Path to GDPR audit log (append-only JSONL). If unset, audit events go to main log only.
    #[serde(default)]
    pub audit_log_path: Option<String>,
    /// Enable PII scrubbing in log output (default: true).
    #[serde(default = "default_true")]
    pub pii_scrub: bool,
    /// Enable mmap crash buffer for post-mortem debugging (default: false).
    #[serde(default)]
    pub crash_buffer: bool,
    /// Crash buffer size in bytes (default: 2MB).
    #[serde(default = "default_crash_buffer_size")]
    pub crash_buffer_size: usize,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            json_output: false,
            file_path: None,
            max_file_size: default_log_max_size(),
            max_files: default_log_max_files(),
            audit_log_path: None,
            pii_scrub: true,
            crash_buffer: false,
            crash_buffer_size: default_crash_buffer_size(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct ImageConfig {
    /// Enable image processing pipeline (default: true).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Enable face detection and blurring (default: true).
    #[serde(default = "default_true")]
    pub face_detection: bool,
    /// Enable OCR text detection (default: true).
    #[serde(default = "default_true")]
    pub ocr_enabled: bool,
    /// OCR processing tier: "detect_and_blur" (default) or "full_recognition".
    #[serde(default = "default_ocr_tier")]
    pub ocr_tier: String,
    /// Maximum image dimension in pixels before resize (default: 960).
    #[serde(default = "default_max_dimension")]
    pub max_dimension: u32,
    /// Gaussian blur sigma for face regions (default: 25.0).
    #[serde(default = "default_face_blur_sigma")]
    pub face_blur_sigma: f32,
    /// Gaussian blur sigma for text regions (default: 15.0).
    #[serde(default = "default_text_blur_sigma")]
    pub text_blur_sigma: f32,
    /// Seconds before idle face/OCR models are evicted (default: 300).
    #[serde(default = "default_model_idle_timeout")]
    pub model_idle_timeout_secs: u64,
    /// Face detection model: "scrfd" (Full/Standard) or "blazeface" (Lite).
    #[serde(default = "default_face_model")]
    pub face_model: String,
    /// Path to BlazeFace model directory.
    #[serde(default)]
    pub face_model_dir: Option<String>,
    /// Path to SCRFD model directory.
    #[serde(default)]
    pub face_model_dir_scrfd: Option<String>,
    /// Path to PaddleOCR model directory.
    #[serde(default)]
    pub ocr_model_dir: Option<String>,
    /// Enable screenshot detection heuristics (default: true).
    #[serde(default = "default_true")]
    pub screen_guard: bool,
    /// Strip EXIF metadata from images (default: true).
    #[serde(default = "default_true")]
    pub exif_strip: bool,
    /// Enable NSFW/nudity detection (default: true).
    #[serde(default = "default_true")]
    pub nsfw_detection: bool,
    /// Path to NudeNet ONNX model directory.
    #[serde(default)]
    pub nsfw_model_dir: Option<String>,
    /// NSFW detection confidence threshold (default: 0.45).
    #[serde(default = "default_nsfw_threshold")]
    pub nsfw_threshold: f32,
}

impl Default for ImageConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            face_detection: true,
            ocr_enabled: true,
            ocr_tier: default_ocr_tier(),
            max_dimension: default_max_dimension(),
            face_blur_sigma: default_face_blur_sigma(),
            text_blur_sigma: default_text_blur_sigma(),
            model_idle_timeout_secs: default_model_idle_timeout(),
            face_model: default_face_model(),
            face_model_dir: None,
            face_model_dir_scrfd: None,
            ocr_model_dir: None,
            screen_guard: true,
            exif_strip: true,
            nsfw_detection: true,
            nsfw_model_dir: None,
            nsfw_threshold: default_nsfw_threshold(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct VoiceConfig {
    /// Enable voice PII detection pipeline (default: false).
    /// When enabled and KWS models are available, audio blocks are scanned
    /// for PII keywords. Blocks with PII are stripped; clean audio passes through.
    /// When KWS models are unavailable, audio passes through unscanned.
    #[serde(default)]
    pub enabled: bool,

    /// Directory containing KWS Zipformer ONNX models (encoder/decoder/joiner + tokens.txt).
    #[serde(default = "default_kws_model_dir")]
    pub kws_model_dir: String,

    /// Path to tokenized PII keywords file.
    #[serde(default = "default_kws_keywords_file")]
    pub kws_keywords_file: String,

    /// KWS detection threshold (0-1). Lower = more sensitive. Default: 0.1.
    #[serde(default = "default_kws_threshold")]
    pub kws_threshold: f32,

    /// KWS keyword boosting score. Higher = easier to trigger. Default: 3.0.
    #[serde(default = "default_kws_score")]
    pub kws_score: f32,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            kws_model_dir: default_kws_model_dir(),
            kws_keywords_file: default_kws_keywords_file(),
            kws_threshold: default_kws_threshold(),
            kws_score: default_kws_score(),
        }
    }
}

fn default_kws_model_dir() -> String {
    "models/kws".to_string()
}
fn default_kws_keywords_file() -> String {
    "models/kws/keywords.txt".to_string()
}
fn default_kws_threshold() -> f32 {
    0.1
}
fn default_kws_score() -> f32 {
    3.0
}

fn default_face_model() -> String {
    "blazeface".to_string()
}
fn default_ocr_tier() -> String {
    "detect_and_blur".to_string()
}
fn default_max_dimension() -> u32 {
    960
}
fn default_face_blur_sigma() -> f32 {
    25.0
}
fn default_text_blur_sigma() -> f32 {
    20.0
}
fn default_nsfw_threshold() -> f32 {
    0.45
}
fn default_model_idle_timeout() -> u64 {
    300
}

fn default_listen_addr() -> String {
    "127.0.0.1".to_string()
}
fn default_port() -> u16 {
    18790
}
fn default_timeout() -> u64 {
    120
}
fn default_max_body() -> usize {
    16 * 1024 * 1024 // 16MB
}
fn default_keychain_service() -> String {
    "openobscure".to_string()
}
fn default_keychain_user() -> String {
    "fpe-master-key".to_string()
}
fn default_true() -> bool {
    true
}
fn default_ner_confidence() -> f32 {
    0.85
}
fn default_scanner_mode() -> String {
    "auto".to_string()
}
fn default_ram_threshold() -> u64 {
    200
}
fn default_log_level() -> String {
    "info".to_string()
}
fn default_log_max_size() -> u64 {
    10 * 1024 * 1024 // 10MB
}
fn default_log_max_files() -> u32 {
    3
}
fn default_crash_buffer_size() -> usize {
    2 * 1024 * 1024 // 2MB
}
fn default_skip_fields() -> Vec<String> {
    vec![
        "model".to_string(),
        "stream".to_string(),
        "temperature".to_string(),
        "max_tokens".to_string(),
        "top_p".to_string(),
        "top_k".to_string(),
    ]
}

impl AppConfig {
    /// Parse config from a TOML string (for testing and embedded use).
    pub fn from_toml(toml_str: &str) -> anyhow::Result<Self> {
        let config: Self =
            toml::from_str(toml_str).map_err(|e| anyhow::anyhow!("Failed to parse TOML: {}", e))?;
        config.validate()?;
        Ok(config)
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read config file {}: {}", path.display(), e))?;
        let config: Self = toml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse config file: {}", e))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.proxy.port == 0 {
            anyhow::bail!("Proxy port must be non-zero");
        }
        if self.proxy.max_body_bytes == 0 {
            anyhow::bail!("Max body bytes must be non-zero");
        }
        for (name, provider) in &self.providers {
            if provider.upstream_url.is_empty() {
                anyhow::bail!("Provider '{}' has empty upstream_url", name);
            }
            if provider.route_prefix.is_empty() {
                anyhow::bail!("Provider '{}' has empty route_prefix", name);
            }
            if !provider.route_prefix.starts_with('/') {
                anyhow::bail!("Provider '{}' route_prefix must start with '/'", name);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_CONFIG: &str = r#"
[proxy]
"#;

    const FULL_CONFIG: &str = r#"
[proxy]
listen_addr = "0.0.0.0"
port = 9090
request_timeout_secs = 60
max_body_bytes = 8388608
fail_mode = "closed"

[providers.anthropic]
upstream_url = "https://api.anthropic.com"
route_prefix = "/anthropic"
strip_headers = ["x-internal"]

[providers.openai]
upstream_url = "https://api.openai.com"
route_prefix = "/openai"

[fpe]
keychain_service = "my-service"
keychain_user = "my-user"
enabled = false

[scanner]
enabled = true
keywords_enabled = false
ner_enabled = false
scanner_mode = "regex"
min_confidence = 0.7
agreement_bonus = 0.2

[logging]
level = "debug"
json_output = true
pii_scrub = false
crash_buffer = true
crash_buffer_size = 4194304

[image]
enabled = false
face_detection = false
ocr_enabled = false
nsfw_detection = false
"#;

    // --- Defaults ---

    #[test]
    fn test_minimal_config_defaults() {
        let config = AppConfig::from_toml(MINIMAL_CONFIG).unwrap();
        assert_eq!(config.proxy.listen_addr, "127.0.0.1");
        assert_eq!(config.proxy.port, 18790);
        assert_eq!(config.proxy.request_timeout_secs, 120);
        assert_eq!(config.proxy.max_body_bytes, 16 * 1024 * 1024);
        assert_eq!(config.proxy.fail_mode, FailMode::Open);
    }

    #[test]
    fn test_fpe_defaults() {
        let config = AppConfig::from_toml(MINIMAL_CONFIG).unwrap();
        assert_eq!(config.fpe.keychain_service, "openobscure");
        assert_eq!(config.fpe.keychain_user, "fpe-master-key");
        assert!(config.fpe.enabled);
        assert!(config.fpe.type_overrides.is_empty());
    }

    #[test]
    fn test_scanner_defaults() {
        let config = AppConfig::from_toml(MINIMAL_CONFIG).unwrap();
        assert!(config.scanner.enabled);
        assert!(config.scanner.keywords_enabled);
        assert!(config.scanner.ner_enabled);
        assert_eq!(config.scanner.scanner_mode, "auto");
        assert_eq!(config.scanner.ner_confidence_threshold, 0.85);
        assert_eq!(config.scanner.ram_threshold_mb, 200);
        assert!(config.scanner.respect_code_fences);
        assert_eq!(config.scanner.min_confidence, 0.5);
        assert_eq!(config.scanner.agreement_bonus, 0.15);
        assert_eq!(config.scanner.skip_fields.len(), 6);
        assert!(config.scanner.skip_fields.contains(&"model".to_string()));
    }

    #[test]
    fn test_logging_defaults() {
        let config = AppConfig::from_toml(MINIMAL_CONFIG).unwrap();
        assert_eq!(config.logging.level, "info");
        assert!(!config.logging.json_output);
        assert!(config.logging.file_path.is_none());
        assert_eq!(config.logging.max_file_size, 10 * 1024 * 1024);
        assert_eq!(config.logging.max_files, 3);
        assert!(config.logging.audit_log_path.is_none());
        assert!(config.logging.pii_scrub);
        assert!(!config.logging.crash_buffer);
        assert_eq!(config.logging.crash_buffer_size, 2 * 1024 * 1024);
    }

    #[test]
    fn test_image_defaults() {
        let config = AppConfig::from_toml(MINIMAL_CONFIG).unwrap();
        assert!(config.image.enabled);
        assert!(config.image.face_detection);
        assert!(config.image.ocr_enabled);
        assert_eq!(config.image.ocr_tier, "detect_and_blur");
        assert_eq!(config.image.max_dimension, 960);
        assert_eq!(config.image.face_blur_sigma, 25.0);
        assert_eq!(config.image.text_blur_sigma, 20.0);
        assert_eq!(config.image.model_idle_timeout_secs, 300);
        assert_eq!(config.image.face_model, "blazeface");
        assert!(config.image.screen_guard);
        assert!(config.image.exif_strip);
        assert!(config.image.nsfw_detection);
        assert_eq!(config.image.nsfw_threshold, 0.45);
    }

    // --- Full config with overrides ---

    #[test]
    fn test_full_config_overrides() {
        let config = AppConfig::from_toml(FULL_CONFIG).unwrap();
        assert_eq!(config.proxy.listen_addr, "0.0.0.0");
        assert_eq!(config.proxy.port, 9090);
        assert_eq!(config.proxy.request_timeout_secs, 60);
        assert_eq!(config.proxy.max_body_bytes, 8388608);
        assert_eq!(config.proxy.fail_mode, FailMode::Closed);
    }

    #[test]
    fn test_full_config_providers() {
        let config = AppConfig::from_toml(FULL_CONFIG).unwrap();
        assert_eq!(config.providers.len(), 2);

        let anthropic = &config.providers["anthropic"];
        assert_eq!(anthropic.upstream_url, "https://api.anthropic.com");
        assert_eq!(anthropic.route_prefix, "/anthropic");
        assert_eq!(anthropic.strip_headers, vec!["x-internal"]);

        let openai = &config.providers["openai"];
        assert_eq!(openai.upstream_url, "https://api.openai.com");
        assert_eq!(openai.route_prefix, "/openai");
        assert!(openai.strip_headers.is_empty());
    }

    #[test]
    fn test_full_config_scanner_overrides() {
        let config = AppConfig::from_toml(FULL_CONFIG).unwrap();
        assert!(!config.scanner.keywords_enabled);
        assert!(!config.scanner.ner_enabled);
        assert_eq!(config.scanner.scanner_mode, "regex");
        assert_eq!(config.scanner.min_confidence, 0.7);
        assert_eq!(config.scanner.agreement_bonus, 0.2);
    }

    #[test]
    fn test_full_config_logging_overrides() {
        let config = AppConfig::from_toml(FULL_CONFIG).unwrap();
        assert_eq!(config.logging.level, "debug");
        assert!(config.logging.json_output);
        assert!(!config.logging.pii_scrub);
        assert!(config.logging.crash_buffer);
        assert_eq!(config.logging.crash_buffer_size, 4194304);
    }

    #[test]
    fn test_full_config_image_disabled() {
        let config = AppConfig::from_toml(FULL_CONFIG).unwrap();
        assert!(!config.image.enabled);
        assert!(!config.image.face_detection);
        assert!(!config.image.ocr_enabled);
        assert!(!config.image.nsfw_detection);
    }

    // --- FailMode ---

    #[test]
    fn test_fail_mode_open() {
        let config = AppConfig::from_toml(
            r#"
[proxy]
fail_mode = "open"
"#,
        )
        .unwrap();
        assert_eq!(config.proxy.fail_mode, FailMode::Open);
    }

    #[test]
    fn test_fail_mode_closed() {
        let config = AppConfig::from_toml(
            r#"
[proxy]
fail_mode = "closed"
"#,
        )
        .unwrap();
        assert_eq!(config.proxy.fail_mode, FailMode::Closed);
    }

    #[test]
    fn test_fail_mode_case_insensitive() {
        let config = AppConfig::from_toml(
            r#"
[proxy]
fail_mode = "CLOSED"
"#,
        )
        .unwrap();
        assert_eq!(config.proxy.fail_mode, FailMode::Closed);
    }

    #[test]
    fn test_fail_mode_invalid() {
        let result = AppConfig::from_toml(
            r#"
[proxy]
fail_mode = "unknown"
"#,
        );
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("invalid fail_mode"));
    }

    #[test]
    fn test_fail_mode_display() {
        assert_eq!(FailMode::Open.to_string(), "open");
        assert_eq!(FailMode::Closed.to_string(), "closed");
    }

    // --- Validation ---

    #[test]
    fn test_validate_port_zero() {
        let result = AppConfig::from_toml(
            r#"
[proxy]
port = 0
"#,
        );
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("port must be non-zero"));
    }

    #[test]
    fn test_validate_max_body_zero() {
        let result = AppConfig::from_toml(
            r#"
[proxy]
max_body_bytes = 0
"#,
        );
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Max body bytes must be non-zero"));
    }

    #[test]
    fn test_validate_provider_empty_url() {
        let result = AppConfig::from_toml(
            r#"
[proxy]

[providers.bad]
upstream_url = ""
route_prefix = "/bad"
"#,
        );
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("empty upstream_url"));
    }

    #[test]
    fn test_validate_provider_empty_prefix() {
        let result = AppConfig::from_toml(
            r#"
[proxy]

[providers.bad]
upstream_url = "https://example.com"
route_prefix = ""
"#,
        );
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("empty route_prefix"));
    }

    #[test]
    fn test_validate_provider_prefix_no_slash() {
        let result = AppConfig::from_toml(
            r#"
[proxy]

[providers.bad]
upstream_url = "https://example.com"
route_prefix = "no-slash"
"#,
        );
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must start with '/'"));
    }

    // --- Invalid TOML ---

    #[test]
    fn test_invalid_toml_syntax() {
        let result = AppConfig::from_toml("this is not valid toml {{{}}}");
        assert!(result.is_err());
    }

    #[test]
    fn test_wrong_type_port_string() {
        let result = AppConfig::from_toml(
            r#"
[proxy]
port = "not a number"
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_wrong_type_enabled_string() {
        let result = AppConfig::from_toml(
            r#"
[proxy]
[scanner]
enabled = "yes"
"#,
        );
        assert!(result.is_err());
    }

    // --- File loading ---

    #[test]
    fn test_load_nonexistent_file() {
        let result = AppConfig::load(Path::new("/tmp/nonexistent_openobscure_config.toml"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Failed to read"));
    }

    #[test]
    fn test_load_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_config.toml");
        std::fs::write(
            &path,
            r#"
[proxy]
port = 12345

[providers.test]
upstream_url = "https://test.com"
route_prefix = "/test"
"#,
        )
        .unwrap();
        let config = AppConfig::load(&path).unwrap();
        assert_eq!(config.proxy.port, 12345);
        assert_eq!(config.providers["test"].upstream_url, "https://test.com");
    }

    // --- No providers is valid ---

    #[test]
    fn test_no_providers_valid() {
        let config = AppConfig::from_toml(
            r#"
[proxy]
"#,
        )
        .unwrap();
        assert!(config.providers.is_empty());
    }

    // --- Custom patterns ---

    #[test]
    fn test_custom_patterns() {
        let config = AppConfig::from_toml(
            r#"
[proxy]

[scanner.custom_patterns.passport]
regex = "[A-Z]{2}\\d{7}"
radix = 36
"#,
        )
        .unwrap();
        assert!(config.scanner.custom_patterns.contains_key("passport"));
        let p = &config.scanner.custom_patterns["passport"];
        assert_eq!(p.radix, 36);
        assert!(p.alphabet.is_none());
    }
}

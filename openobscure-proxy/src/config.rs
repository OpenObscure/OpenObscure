use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::path::Path;

/// Controls proxy behavior when body processing (JSON parse, FPE) fails.
/// - Open: forward original body unmodified (default — never block AI functionality)
/// - Closed: reject the request with 502 (strict privacy — no unscanned traffic)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailMode {
    Open,
    Closed,
}

impl Default for FailMode {
    fn default() -> Self {
        FailMode::Open
    }
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
    pub compliance: ComplianceConfig,
    #[serde(default)]
    pub cross_border: CrossBorderConfig,
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
    /// When false (default), auth headers from the host agent pass through to upstream
    /// unchanged — no duplicate API key management required.
    /// When true, OpenObscure injects/replaces the auth header with a key from its
    /// own vault (secondary/override key for advanced setups).
    #[serde(default)]
    pub override_auth: bool,
    /// Name of the vault entry to use when override_auth is true.
    /// Defaults to the provider name (e.g., "anthropic").
    pub vault_key_name: Option<String>,
    /// Which HTTP header carries the API key for this provider.
    /// Only used when override_auth is true.
    /// Examples: "x-api-key" (Anthropic), "authorization" (OpenAI), "api-key" (Azure).
    /// Defaults to "authorization".
    pub auth_header_name: Option<String>,
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
    /// Seconds to keep the previous key active after rotation (default: 30).
    #[serde(default = "default_key_overlap_secs")]
    pub key_overlap_secs: u64,
}

impl Default for FpeConfig {
    fn default() -> Self {
        Self {
            keychain_service: default_keychain_service(),
            keychain_user: default_keychain_user(),
            enabled: true,
            type_overrides: HashMap::new(),
            key_overlap_secs: default_key_overlap_secs(),
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
    /// Path to BlazeFace model directory.
    #[serde(default)]
    pub face_model_dir: Option<String>,
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
            face_model_dir: None,
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
pub struct ComplianceConfig {
    /// Organization name for compliance reports.
    #[serde(default)]
    pub organization_name: Option<String>,
    /// Data Protection Officer email.
    #[serde(default)]
    pub dpo_email: Option<String>,
    /// Supervisory authority / DPA contact email.
    #[serde(default)]
    pub dpa_contact: Option<String>,
    /// Directory for generated compliance reports (default: ~/.openobscure/reports).
    #[serde(default)]
    pub reports_dir: Option<String>,
    /// Audit log retention in days (default: 365).
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
}

impl Default for ComplianceConfig {
    fn default() -> Self {
        Self {
            organization_name: None,
            dpo_email: None,
            dpa_contact: None,
            reports_dir: None,
            retention_days: default_retention_days(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct CrossBorderConfig {
    /// Enable cross-border jurisdiction classification (default: false).
    #[serde(default)]
    pub enabled: bool,
    /// Policy mode: "log" (default), "warn", "block".
    #[serde(default = "default_cross_border_mode")]
    pub mode: String,
    /// Allowed jurisdictions (e.g. ["EU", "US", "UK"]). Empty = all allowed.
    #[serde(default)]
    pub allowed_jurisdictions: Vec<String>,
    /// Blocked jurisdictions. Overrides allowed list.
    #[serde(default)]
    pub blocked_jurisdictions: Vec<String>,
}

impl Default for CrossBorderConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: default_cross_border_mode(),
            allowed_jurisdictions: Vec::new(),
            blocked_jurisdictions: Vec::new(),
        }
    }
}

fn default_retention_days() -> u32 {
    365
}
fn default_cross_border_mode() -> String {
    "log".to_string()
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
fn default_key_overlap_secs() -> u64 {
    30
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

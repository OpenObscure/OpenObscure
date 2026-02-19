// Many modules export pub items for the library API, benchmarks, and tests
// that are not used directly by the server binary.
#![allow(dead_code)]

#[macro_use]
mod oo_log;
mod body;
mod config;
mod crash_buffer;
mod crf_scanner;
mod detection_meta;
mod detection_validators;
mod device_profile;
mod error;
mod face_detector;
mod fpe_engine;
mod health;
mod hybrid_scanner;
mod image_blur;
mod image_detect;
mod image_pipeline;
mod key_manager;
mod keyword_dict;
mod lib_mobile;
mod mapping;
mod ner_scanner;
mod nsfw_detector;
mod ocr_engine;
mod pii_scrub_layer;
mod pii_types;
mod proxy;
mod scanner;
mod screen_guard;
mod server;
mod vault;
mod wordpiece;

#[cfg(test)]
mod integration_tests;

use std::sync::Arc;

use axum::body::Body;
use clap::{Parser, Subcommand};
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use rand::RngCore;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

use crate::config::{AppConfig, LoggingConfig};
use crate::health::HealthStats;
use crate::hybrid_scanner::HybridScanner;
use crate::image_pipeline::ImageModelManager;
use crate::key_manager::KeyManager;
use crate::mapping::MappingStore;
use crate::proxy::AppState;
use crate::vault::Vault;

#[derive(Parser, Debug)]
#[command(
    name = "openobscure-proxy",
    version,
    about = "OpenObscure PII Privacy Proxy — FF1 format-preserving encryption for LLM API calls"
)]
struct Cli {
    /// Path to TOML config file
    #[arg(
        short,
        long,
        default_value = "config/openobscure.toml",
        env = "OPENOBSCURE_CONFIG"
    )]
    config: std::path::PathBuf,

    /// Override listen port
    #[arg(short, long, env = "OPENOBSCURE_PORT")]
    port: Option<u16>,

    /// Override log level (trace, debug, info, warn, error)
    #[arg(long, env = "OPENOBSCURE_LOG")]
    log_level: Option<String>,

    /// Initialize FPE key in OS keychain (first-run setup)
    #[arg(long)]
    init_key: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start the PII proxy server (default when no subcommand given)
    Serve,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Install panic hook for crash marker (before anything else)
    health::install_panic_hook();

    let cli = Cli::parse();

    // Load config first so we can configure tracing from it
    let mut config = AppConfig::load(&cli.config)?;
    if let Some(port) = cli.port {
        config.proxy.port = port;
    }

    // Initialize tracing subscriber from config + CLI overrides
    let _guards = init_tracing(&cli, &config.logging);

    // Handle --init-key (works regardless of subcommand)
    if cli.init_key {
        let vault = Vault::new(&config.fpe.keychain_service);
        if vault.fpe_key_exists() {
            oo_warn!(
                crate::oo_log::modules::VAULT,
                "FPE key already exists in keychain. Delete it first to regenerate."
            );
            return Ok(());
        }
        vault
            .init_fpe_key()
            .map_err(|e| anyhow::anyhow!("Failed to initialize FPE key: {}", e))?;
        oo_info!(
            crate::oo_log::modules::VAULT,
            "FPE master key generated and stored in OS keychain"
        );
        return Ok(());
    }

    // Dispatch on subcommand
    match cli.command {
        None | Some(Commands::Serve) => {
            // Default: start the PII proxy server
            run_serve(config).await
        }
    }
}

/// Start the PII proxy server (default behavior).
async fn run_serve(config: AppConfig) -> anyhow::Result<()> {
    // Check for crash marker from previous run
    health::check_crash_marker();

    // Detect device hardware and select feature budget
    let profile = device_profile::detect(false);
    let tier = device_profile::tier_for_profile(&profile);
    let budget = device_profile::budget_for_tier(tier, &profile);

    oo_info!(
        crate::oo_log::modules::DEVICE,
        "Device profile detected",
        total_ram_mb = profile.total_ram_mb,
        available_ram_mb = profile.available_ram_mb.unwrap_or(0),
        cpu_cores = profile.cpu_cores,
        tier = %tier,
        max_ram_mb = budget.max_ram_mb,
        ner = budget.ner_enabled,
        ensemble = budget.ensemble_enabled,
        image = budget.image_pipeline_enabled
    );

    oo_info!(
        crate::oo_log::modules::CONFIG,
        "Configuration loaded",
        providers = config.providers.len()
    );

    // Initialize vault and key manager
    let vault = Arc::new(Vault::new(&config.fpe.keychain_service));
    let key_manager = KeyManager::new(Arc::clone(&vault))
        .map_err(|e| anyhow::anyhow!("Failed to initialize KeyManager: {}", e))?;

    oo_info!(
        crate::oo_log::modules::FPE,
        "FPE engine initialized (FF1, AES-256)"
    );

    // Build HTTPS client for upstream connections
    let https_connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .expect("Failed to load native TLS root certificates")
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .build();

    let http_client: Client<_, Body> = Client::builder(TokioExecutor::new()).build(https_connector);

    // Build hybrid scanner (regex + keywords + semantic backend)
    let scanner = build_scanner(&config, &budget);
    oo_info!(
        crate::oo_log::modules::SCANNER,
        "Hybrid scanner initialized",
        keywords = scanner.keywords_enabled(),
        semantic = scanner.semantic_backend_name()
    );

    // Initialize image model manager if image processing is enabled and budget allows
    let image_models = if config.image.enabled && budget.image_pipeline_enabled {
        let mut img_config = config.image.clone();
        img_config.model_idle_timeout_secs = budget.model_idle_timeout_secs;
        let models = Arc::new(ImageModelManager::new(img_config));
        oo_info!(crate::oo_log::modules::IMAGE, "Image pipeline enabled",
            face_detection = config.image.face_detection,
            ocr_enabled = config.image.ocr_enabled,
            ocr_tier = %config.image.ocr_tier,
            max_dimension = config.image.max_dimension,
            idle_timeout_secs = budget.model_idle_timeout_secs);

        // Spawn model eviction task (checks every 60s)
        let evict_models = Arc::clone(&models);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                evict_models.evict_if_idle();
            }
        });

        Some(models)
    } else if config.image.enabled && !budget.image_pipeline_enabled {
        oo_info!(crate::oo_log::modules::IMAGE, "Image pipeline disabled by device budget",
            tier = %tier, max_ram_mb = budget.max_ram_mb);
        None
    } else {
        oo_info!(crate::oo_log::modules::IMAGE, "Image pipeline disabled");
        None
    };

    // Build application state
    let state = AppState {
        config: Arc::new(config),
        scanner: Arc::new(scanner),
        key_manager: Arc::new(key_manager),
        mapping_store: MappingStore::new(300), // 5 minute TTL
        http_client,
        vault,
        health: HealthStats::new(),
        image_models,
    };

    // Resolve auth token for health endpoint
    let auth_token = resolve_auth_token();

    // Build budget summary for health endpoint
    let budget_summary = health::FeatureBudgetSummary {
        tier: tier.to_string(),
        max_ram_mb: budget.max_ram_mb,
        ner_enabled: budget.ner_enabled,
        crf_enabled: budget.crf_enabled,
        ensemble_enabled: budget.ensemble_enabled,
        image_pipeline_enabled: budget.image_pipeline_enabled,
    };

    // Start server
    server::run(state, auth_token, tier.to_string(), budget_summary).await
}

/// Build the hybrid scanner based on config scanner_mode and available resources.
///
/// Modes:
/// Initialize the tracing subscriber from config + CLI overrides.
///
/// Returns `WorkerGuard`s that must be held for the lifetime of the program —
/// dropping them flushes any non-blocking writer buffers.
fn init_tracing(
    cli: &Cli,
    log_cfg: &LoggingConfig,
) -> Vec<tracing_appender::non_blocking::WorkerGuard> {
    let mut guards = Vec::new();

    // CLI --log-level overrides config
    let log_level = cli.log_level.as_deref().unwrap_or(&log_cfg.level);
    let filter = EnvFilter::try_new(log_level).unwrap_or_else(|_| EnvFilter::new("info"));

    use crate::pii_scrub_layer::PiiScrubMakeWriter;

    // Stderr layer: 4 combinations (json × pii_scrub). Only one is Some at a time.
    let (stderr_js, stderr_jp, stderr_ps, stderr_pp) =
        match (log_cfg.json_output, log_cfg.pii_scrub) {
            (true, true) => (
                Some(
                    tracing_subscriber::fmt::layer()
                        .json()
                        .with_target(false)
                        .with_writer(PiiScrubMakeWriter::new(std::io::stderr)),
                ),
                None,
                None,
                None,
            ),
            (true, false) => (
                None,
                Some(
                    tracing_subscriber::fmt::layer()
                        .json()
                        .with_target(false)
                        .with_writer(std::io::stderr),
                ),
                None,
                None,
            ),
            (false, true) => (
                None,
                None,
                Some(
                    tracing_subscriber::fmt::layer()
                        .with_target(false)
                        .with_writer(PiiScrubMakeWriter::new(std::io::stderr)),
                ),
                None,
            ),
            (false, false) => (
                None,
                None,
                None,
                Some(
                    tracing_subscriber::fmt::layer()
                        .with_target(false)
                        .with_writer(std::io::stderr),
                ),
            ),
        };

    // File layer: optional, daily rotation, always JSON. Two variants for pii_scrub on/off.
    let (file_scrub, file_plain) = if let Some(ref file_path) = log_cfg.file_path {
        let path = std::path::Path::new(file_path);
        let dir = path.parent().unwrap_or(std::path::Path::new("."));
        let prefix = path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("openobscure.log");

        let file_appender = tracing_appender::rolling::daily(dir, prefix);
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
        guards.push(guard);

        if log_cfg.pii_scrub {
            (
                Some(
                    tracing_subscriber::fmt::layer()
                        .json()
                        .with_target(false)
                        .with_writer(PiiScrubMakeWriter::new(non_blocking)),
                ),
                None,
            )
        } else {
            (
                None,
                Some(
                    tracing_subscriber::fmt::layer()
                        .json()
                        .with_target(false)
                        .with_writer(non_blocking),
                ),
            )
        }
    } else {
        (None, None)
    };

    // Audit log layer: optional, append-only JSONL for GDPR audit events.
    // NOT PII-scrubbed — audit logs record what was processed (with redacted labels).
    let audit_layer = if let Some(ref audit_path) = log_cfg.audit_log_path {
        let path = std::path::Path::new(audit_path);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            Ok(file) => {
                let (non_blocking, guard) = tracing_appender::non_blocking(file);
                guards.push(guard);
                Some(
                    tracing_subscriber::fmt::layer()
                        .json()
                        .with_target(false)
                        .with_writer(non_blocking)
                        .with_filter(tracing_subscriber::filter::FilterFn::new(|metadata| {
                            metadata.fields().field("oo_audit").is_some()
                        })),
                )
            }
            Err(e) => {
                eprintln!(
                    "[OpenObscure] Failed to open audit log {}: {}",
                    audit_path, e
                );
                None
            }
        }
    } else {
        None
    };

    // Crash buffer layer: optional mmap ring buffer for post-mortem debugging.
    // Separate layer with plain-text format — survives SIGKILL/OOM.
    let crash_layer = if log_cfg.crash_buffer {
        let crash_path = resolve_crash_buffer_path();
        match crash_buffer::CrashBuffer::open(&crash_path, log_cfg.crash_buffer_size) {
            Ok(buf) => {
                let buf = Arc::new(buf);
                eprintln!(
                    "[OpenObscure] Crash buffer enabled ({}KB at {})",
                    log_cfg.crash_buffer_size / 1024,
                    crash_path.display()
                );
                Some(
                    tracing_subscriber::fmt::layer()
                        .with_ansi(false)
                        .with_target(false)
                        .with_writer(crash_buffer::CrashBufferMakeWriter::new(std::io::sink, buf)),
                )
            }
            Err(e) => {
                eprintln!("[OpenObscure] Failed to open crash buffer: {}", e);
                None
            }
        }
    } else {
        None
    };

    // Platform-specific OS log layer (macOS: OSLog, Linux: journald)
    let platform_layer = init_platform_log_layer();

    tracing_subscriber::registry()
        .with(filter)
        .with(stderr_js)
        .with(stderr_jp)
        .with(stderr_ps)
        .with(stderr_pp)
        .with(file_scrub)
        .with(file_plain)
        .with(audit_layer)
        .with(crash_layer)
        .with(platform_layer)
        .init();

    guards
}

/// Initialize platform-specific log layer.
///
/// - macOS: tracing-oslog (sends to unified logging / Console.app)
/// - Linux: tracing-journald (sends to systemd journal)
/// - Windows/Other: None (uses file + stderr logging only)
#[cfg(target_os = "macos")]
fn init_platform_log_layer() -> Option<tracing_oslog::OsLogger> {
    let layer = tracing_oslog::OsLogger::new("com.openobscure.proxy", "default");
    eprintln!("[OpenObscure] macOS unified logging enabled (com.openobscure.proxy)");
    Some(layer)
}

#[cfg(target_os = "linux")]
fn init_platform_log_layer() -> Option<tracing_journald::Layer> {
    match tracing_journald::layer() {
        Ok(layer) => {
            eprintln!("[OpenObscure] journald logging enabled");
            Some(layer)
        }
        Err(e) => {
            eprintln!("[OpenObscure] Failed to init journald: {}", e);
            None
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn init_platform_log_layer() -> Option<tracing_subscriber::layer::Identity> {
    None
}

/// Resolve the crash buffer file path (~/.openobscure/crash.buf).
fn resolve_crash_buffer_path() -> std::path::PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    home.join(".openobscure").join("crash.buf")
}

/// Build the hybrid scanner based on config scanner_mode and available resources.
///
/// Modes:
/// - "auto": try NER if model available + enough RAM, else CRF if available, else regex-only
/// - "ner": force NER (fall back to regex+keywords if model unavailable)
/// - "crf": force CRF (fall back to regex+keywords if model unavailable)
/// - "regex": regex + keywords only, no semantic scanner
fn build_scanner(config: &AppConfig, budget: &device_profile::FeatureBudget) -> HybridScanner {
    let kw = config.scanner.keywords_enabled;
    let threshold = config.scanner.ner_confidence_threshold;
    let code_fences = config.scanner.respect_code_fences;

    let mut scanner = match config.scanner.scanner_mode.as_str() {
        "regex" => {
            oo_info!(
                crate::oo_log::modules::SCANNER,
                "Scanner mode: regex-only (no semantic backend)"
            );
            HybridScanner::new(kw, None)
        }
        "ner" => {
            let ner = try_load_ner(config, threshold);
            if ner.is_none() {
                oo_warn!(
                    crate::oo_log::modules::SCANNER,
                    "Scanner mode 'ner' requested but model unavailable, using regex+keywords"
                );
            }
            HybridScanner::new(kw, ner)
        }
        "crf" => {
            let crf = try_load_crf(config, threshold);
            if crf.is_none() {
                oo_warn!(
                    crate::oo_log::modules::SCANNER,
                    "Scanner mode 'crf' requested but model unavailable, using regex+keywords"
                );
            }
            HybridScanner::with_crf(kw, crf)
        }
        _ => {
            // Auto: use device profile budget to decide NER vs CRF
            oo_info!(
                crate::oo_log::modules::SCANNER,
                "Auto scanner selection via device profiler",
                tier = %budget.tier,
                budget_ner = budget.ner_enabled,
                budget_crf = budget.crf_enabled,
                budget_ensemble = budget.ensemble_enabled
            );

            if budget.ner_enabled {
                if let Some(ner) = try_load_ner(config, threshold) {
                    HybridScanner::new(kw, Some(ner))
                } else if budget.crf_enabled {
                    oo_info!(
                        crate::oo_log::modules::SCANNER,
                        "NER model unavailable, falling back to CRF"
                    );
                    if let Some(crf) = try_load_crf(config, threshold) {
                        HybridScanner::with_crf(kw, Some(crf))
                    } else {
                        oo_info!(
                            crate::oo_log::modules::SCANNER,
                            "No semantic model available, using regex+keywords only"
                        );
                        HybridScanner::new(kw, None)
                    }
                } else {
                    oo_info!(
                        crate::oo_log::modules::SCANNER,
                        "NER unavailable, no CRF in budget, using regex+keywords only"
                    );
                    HybridScanner::new(kw, None)
                }
            } else if budget.crf_enabled {
                if let Some(crf) = try_load_crf(config, threshold) {
                    HybridScanner::with_crf(kw, Some(crf))
                } else {
                    oo_info!(
                        crate::oo_log::modules::SCANNER,
                        "CRF model unavailable, using regex+keywords only"
                    );
                    HybridScanner::new(kw, None)
                }
            } else {
                oo_info!(
                    crate::oo_log::modules::SCANNER,
                    "Device budget: regex+keywords only"
                );
                HybridScanner::new(kw, None)
            }
        }
    };
    scanner.set_respect_code_fences(code_fences);
    scanner.set_confidence_params(
        config.scanner.min_confidence,
        config.scanner.agreement_bonus,
    );
    scanner
}

fn try_load_ner(config: &AppConfig, threshold: f32) -> Option<ner_scanner::NerScanner> {
    let model_dir = config.scanner.ner_model_dir.as_ref()?;
    let model_path = std::path::Path::new(model_dir);
    match ner_scanner::NerScanner::load(model_path, threshold) {
        Ok(ner) => {
            oo_info!(crate::oo_log::modules::NER, "NER scanner loaded", model_dir = %model_dir);
            Some(ner)
        }
        Err(e) => {
            oo_warn!(crate::oo_log::modules::NER, "NER scanner failed to load", error = %e);
            None
        }
    }
}

fn try_load_crf(config: &AppConfig, threshold: f32) -> Option<crf_scanner::CrfScanner> {
    let model_dir = config.scanner.crf_model_dir.as_ref()?;
    let model_path = std::path::Path::new(model_dir);
    match crf_scanner::CrfScanner::load(model_path, threshold) {
        Ok(crf) => {
            oo_info!(crate::oo_log::modules::CRF, "CRF scanner loaded", model_dir = %model_dir);
            Some(crf)
        }
        Err(e) => {
            oo_warn!(crate::oo_log::modules::CRF, "CRF scanner failed to load", error = %e);
            None
        }
    }
}

/// Resolve the auth token for the health endpoint.
///
/// Resolution order:
/// 1. `OPENOBSCURE_AUTH_TOKEN` env var
/// 2. `~/.openobscure/.auth-token` file
/// 3. Generate random 32-byte hex token and write to file (0600 on Unix)
fn resolve_auth_token() -> Option<String> {
    // 1. Check env var
    if let Ok(token) = std::env::var("OPENOBSCURE_AUTH_TOKEN") {
        let token = token.trim().to_string();
        if !token.is_empty() {
            oo_info!(
                crate::oo_log::modules::SERVER,
                "Auth token loaded from OPENOBSCURE_AUTH_TOKEN env var"
            );
            return Some(token);
        }
    }

    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));

    // 2. Try reading from file
    if let Some(ref home) = home {
        let token_path = std::path::Path::new(home).join(".openobscure/.auth-token");
        if let Ok(token) = std::fs::read_to_string(&token_path) {
            let token = token.trim().to_string();
            if !token.is_empty() {
                oo_info!(crate::oo_log::modules::SERVER, "Auth token loaded from file", path = %token_path.display());
                return Some(token);
            }
        }
    }

    // 3. Generate new token and write to file
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let token = hex::encode(bytes);

    if let Some(ref home) = home {
        let dir = std::path::Path::new(home).join(".openobscure");
        let _ = std::fs::create_dir_all(&dir);
        let token_path = dir.join(".auth-token");
        match std::fs::write(&token_path, &token) {
            Ok(()) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(
                        &token_path,
                        std::fs::Permissions::from_mode(0o600),
                    );
                }
                oo_info!(crate::oo_log::modules::SERVER, "Auth token generated and saved", path = %token_path.display());
            }
            Err(e) => {
                oo_warn!(crate::oo_log::modules::SERVER, "Failed to write auth token file", error = %e);
            }
        }
    }

    Some(token)
}

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
mod hash_token;
mod health;
mod hybrid_scanner;
mod image_detect;
mod image_fetch;
mod image_pipeline;
mod image_redact;
mod inspect;
mod key_manager;
mod keyword_dict;
mod lang_detect;

mod audio_decode;
mod kws_engine;
mod lib_mobile;
mod mapping;
mod multilingual;
mod name_gazetteer;
mod ner_endpoint;
mod ner_scanner;
mod nsfw_classifier;
mod nsfw_detector;
mod ocr_engine;
mod ort_ep;
mod passthrough;
mod persuasion_dict;
mod pii_scrub_layer;
mod pii_types;
mod proxy;
mod response_format;
mod response_integrity;
mod ri_model;
mod scanner;
mod screen_guard;
mod server;
mod sse_accumulator;
mod vault;
mod voice_detect;
mod voice_pipeline;
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
use crate::fpe_engine::FpeEngine;
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

    /// Run in foreground — disables managed service for this session
    #[arg(long)]
    foreground: bool,

    /// Inspect mode: log incoming/outgoing data to console, save image/audio files
    #[arg(long)]
    inspect: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start the PII proxy server (default when no subcommand given)
    Serve,
    /// Rotate the FPE encryption key (generates new key, keeps old for overlap window)
    KeyRotate,
    /// Lightweight passthrough proxy (no PII scanning, forwards to upstream directly)
    Passthrough,
    /// Manage the OpenObscure system service (launchd on macOS, systemd on Linux)
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
}

#[derive(Subcommand, Debug)]
enum ServiceAction {
    /// Install the service (copies plist/unit file, creates directories)
    Install,
    /// Uninstall the service (unloads and removes plist/unit file)
    Uninstall,
    /// Start the managed service (enables auto-restart)
    Start,
    /// Stop the managed service (disables auto-restart)
    Stop {
        /// Start passthrough proxy after stopping (keeps agent working without PII scanning)
        #[arg(long)]
        passthrough: bool,
    },
    /// Show service status
    Status,
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

    // Suppress verbose ONNX Runtime internal logs (GraphTransformer, allocator, etc.)
    // Route only Warning+ ORT messages through tracing.
    ort::init()
        .with_logger(std::sync::Arc::new(
            |level: ort::logging::LogLevel,
             _category: &str,
             _id: &str,
             _code_location: &str,
             message: &str| {
                match level {
                    ort::logging::LogLevel::Warning => {
                        // Suppress noisy CoreML EP capability and node-assignment warnings
                        if message.contains("CoreMLExecutionProvider")
                            || message.contains("not assigned to the preferred execution providers")
                        {
                            // silenced — these are expected on CoreML partial support
                        } else {
                            tracing::warn!(target: "ort", "{}", message);
                        }
                    }
                    ort::logging::LogLevel::Error | ort::logging::LogLevel::Fatal => {
                        tracing::error!(target: "ort", "{}", message);
                    }
                    // Suppress Verbose and Info
                    _ => {}
                }
            },
        ))
        .commit();

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
        return Ok(());
    }

    // Dispatch on subcommand
    match cli.command {
        None | Some(Commands::Serve) => {
            if cli.foreground {
                run_serve_foreground(config, cli.inspect).await
            } else {
                run_serve(config, cli.inspect).await
            }
        }
        Some(Commands::KeyRotate) => run_key_rotate(config).await,
        Some(Commands::Passthrough) => run_passthrough(config).await,
        Some(Commands::Service { action }) => run_service(action).await,
    }
}

/// Start the PII proxy server (default behavior).
async fn run_serve(config: AppConfig, inspect: bool) -> anyhow::Result<()> {
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
        image = budget.image_pipeline_enabled,
        onnx_ep = ort_ep::ep_name()
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

    // Install rustls CryptoProvider before building HTTPS connector
    let _ = rustls::crypto::ring::default_provider().install_default();

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
        img_config.ocr_tier = budget.ocr_tier.clone();
        img_config.nsfw_detection = config.image.nsfw_detection && budget.nsfw_enabled;
        img_config.screen_guard = config.image.screen_guard && budget.screen_guard_enabled;
        img_config.face_model = budget.face_model.clone();
        let effective_nsfw = img_config.nsfw_detection;
        let effective_screen_guard = img_config.screen_guard;
        let models = Arc::new(ImageModelManager::new(img_config));
        oo_info!(crate::oo_log::modules::IMAGE, "Image pipeline enabled",
            face_detection = config.image.face_detection,
            ocr_enabled = config.image.ocr_enabled,
            ocr_tier = %budget.ocr_tier,
            nsfw = effective_nsfw,
            screen_guard = effective_screen_guard,
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

    // Voice pipeline: KWS-based PII detection (config AND budget gated)
    let kws_engine = if config.voice.enabled && budget.voice_enabled {
        match kws_engine::KwsEngine::new(&config.voice) {
            Ok(engine) => {
                oo_info!(
                    crate::oo_log::modules::VOICE,
                    "Voice KWS engine loaded — PII keyword detection active"
                );
                Some(Arc::new(engine))
            }
            Err(e) => {
                oo_warn!(crate::oo_log::modules::VOICE,
                    "KWS models not found, audio will pass through unscanned",
                    error = %e);
                None
            }
        }
    } else if config.voice.enabled && !budget.voice_enabled {
        oo_info!(crate::oo_log::modules::VOICE, "Voice pipeline disabled by device budget",
            tier = %tier, max_ram_mb = budget.max_ram_mb);
        None
    } else {
        oo_info!(crate::oo_log::modules::VOICE, "Voice pipeline disabled");
        None
    };

    // Response integrity scanner (cognitive firewall, config AND budget gated)
    let ri_scanner = if config.response_integrity.enabled && budget.ri_enabled {
        let sensitivity: response_integrity::Sensitivity =
            config.response_integrity.sensitivity.parse().unwrap();
        if sensitivity == response_integrity::Sensitivity::Off {
            oo_info!(
                crate::oo_log::modules::RESPONSE_INTEGRITY,
                "Response integrity enabled but sensitivity=off, scanner inactive"
            );
            None
        } else {
            let scanner = response_integrity::ResponseIntegrityScanner::with_r2(
                sensitivity,
                None, // R2 model loaded below if configured
                config.response_integrity.ri_sample_rate,
            );

            // Load R2 model if configured
            if let Some(ref model_dir) = config.response_integrity.ri_model_dir {
                let model_path = std::path::Path::new(model_dir);
                match scanner.load_r2(
                    model_path,
                    config.response_integrity.ri_threshold,
                    config.response_integrity.ri_early_exit_threshold,
                ) {
                    Ok(true) => {
                        oo_info!(crate::oo_log::modules::RESPONSE_INTEGRITY,
                            "R2 model loaded successfully",
                            model_dir = %model_dir,
                            threshold = config.response_integrity.ri_threshold,
                            early_exit = config.response_integrity.ri_early_exit_threshold);
                    }
                    Ok(false) => {
                        oo_info!(crate::oo_log::modules::RESPONSE_INTEGRITY,
                            "R2 model not found, running R1-only mode",
                            model_dir = %model_dir);
                    }
                    Err(e) => {
                        oo_warn!(crate::oo_log::modules::RESPONSE_INTEGRITY,
                            "R2 model loading failed, running R1-only mode",
                            error = %e);
                    }
                }
            }

            oo_info!(crate::oo_log::modules::RESPONSE_INTEGRITY,
                "Response integrity scanner enabled",
                sensitivity = %config.response_integrity.sensitivity,
                r2_available = scanner.has_r2(),
                phrases = scanner.dict_count());
            Some(Arc::new(scanner))
        }
    } else if config.response_integrity.enabled && !budget.ri_enabled {
        oo_info!(crate::oo_log::modules::RESPONSE_INTEGRITY,
            "Response integrity disabled by device budget",
            tier = %tier, max_ram_mb = budget.max_ram_mb);
        None
    } else {
        None
    };

    // Open request journal for crash recovery
    let request_journal = {
        let journal_path = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".openobscure")
            .join("request_journal.buf");
        match crate::crash_buffer::RequestJournal::open(&journal_path) {
            Ok(journal) => {
                // Check for incomplete entries from a previous crash
                let incomplete = journal.read_incomplete();
                if !incomplete.is_empty() {
                    for entry in &incomplete {
                        oo_warn!(
                            crate::oo_log::modules::PROXY,
                            "Incomplete journaled request from previous run (possible crash during FPE forward)",
                            request_id = %entry.request_id,
                            timestamp = entry.timestamp,
                            mapping_count = entry.mapping_count
                        );
                    }
                    oo_warn!(
                        crate::oo_log::modules::PROXY,
                        "Found incomplete journaled requests",
                        count = incomplete.len()
                    );
                }
                Some(Arc::new(journal))
            }
            Err(e) => {
                oo_warn!(
                    crate::oo_log::modules::PROXY,
                    "Failed to open request journal — crash recovery disabled",
                    error = %e
                );
                None
            }
        }
    };

    // Build application state
    let state = AppState {
        config: Arc::new(config),
        scanner: Arc::new(scanner),
        key_manager: Arc::new(key_manager),
        mapping_store: MappingStore::new(300), // 5 minute TTL
        http_client,
        vault,
        health: {
            let stats = HealthStats::new();
            // Load persisted stats from previous runs
            let stats_path = stats_file_path();
            if stats_path.exists() {
                match stats.load_from_file(&stats_path) {
                    Ok(()) => oo_info!(
                        crate::oo_log::modules::HEALTH,
                        "Restored stats from previous session",
                        path = %stats_path.display()
                    ),
                    Err(e) => oo_warn!(
                        crate::oo_log::modules::HEALTH,
                        "Failed to restore stats, starting fresh",
                        error = %e
                    ),
                }
            }
            stats
        },
        image_models,
        kws_engine,
        response_integrity: ri_scanner,
        device_tier: tier,
        inspect,
        request_journal,
    };

    if inspect {
        eprintln!();
        eprintln!("  ┌────────────────────────────────────────┐");
        eprintln!("  │         INSPECT MODE ACTIVE             │");
        eprintln!("  │  Console: incoming + redacted text      │");
        eprintln!("  │  Files:   ~/.openobscure/inspect/       │");
        eprintln!("  └────────────────────────────────────────┘");
        eprintln!();
        if let Some(dir) = crate::inspect::inspect_dir() {
            let _ = std::fs::create_dir_all(&dir);
        }
    }

    // Spawn periodic stats flush (every 60s)
    let flush_health = state.health.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        let path = stats_file_path();
        loop {
            interval.tick().await;
            if let Err(e) = flush_health.save_to_file(&path) {
                oo_warn!(
                    crate::oo_log::modules::HEALTH,
                    "Failed to persist stats",
                    error = %e
                );
            }
        }
    });

    // Resolve auth token for health endpoint
    let auth_token = resolve_auth_token();

    // Build budget summary for health endpoint
    let budget_summary = health::FeatureBudgetSummary {
        tier: tier.to_string(),
        max_ram_mb: budget.max_ram_mb,
        ner_enabled: budget.ner_enabled,
        ner_model: budget.ner_model.clone(),
        crf_enabled: budget.crf_enabled,
        ensemble_enabled: budget.ensemble_enabled,
        image_pipeline_enabled: budget.image_pipeline_enabled,
        ocr_tier: budget.ocr_tier.clone(),
        nsfw_enabled: budget.nsfw_enabled,
        screen_guard_enabled: budget.screen_guard_enabled,
        face_model: budget.face_model.clone(),
        voice_enabled: budget.voice_enabled,
        ri_enabled: budget.ri_enabled,
    };

    // Start server
    server::run(state, auth_token, tier.to_string(), budget_summary).await
}

/// Rotate the FPE encryption key.
///
/// Generates a new random 32-byte key, stores it in the vault, and logs the
/// version change. The old key remains in the proxy's overlap window for
/// in-flight requests when the proxy is running.
async fn run_key_rotate(config: AppConfig) -> anyhow::Result<()> {
    let vault = Vault::new(&config.fpe.keychain_service);

    // Verify current key exists first
    if !vault.fpe_key_exists() {
        anyhow::bail!("No FPE key found. Run with --init-key first to create the initial key.");
    }

    // Read old key to verify access
    let old_key = vault
        .get_fpe_key()
        .map_err(|e| anyhow::anyhow!("Failed to read current key: {}", e))?;
    let old_engine =
        FpeEngine::new(&old_key).map_err(|e| anyhow::anyhow!("Current key is invalid: {}", e))?;
    drop(old_engine);

    // Generate and store new key
    vault
        .init_fpe_key()
        .map_err(|e| anyhow::anyhow!("Failed to store new key: {}", e))?;

    // Verify new key works
    let new_key = vault
        .get_fpe_key()
        .map_err(|e| anyhow::anyhow!("Failed to read new key: {}", e))?;
    let _new_engine =
        FpeEngine::new(&new_key).map_err(|e| anyhow::anyhow!("New key is invalid: {}", e))?;

    oo_info!(
        crate::oo_log::modules::FPE,
        "FPE key rotated successfully. Restart the proxy to use the new key."
    );
    eprintln!("[OpenObscure] FPE key rotated. Restart the proxy to pick up the new key.");
    eprintln!("[OpenObscure] In-flight requests using the old key will continue during the overlap window.");

    Ok(())
}

/// Start the PII proxy in foreground mode.
///
/// If the managed service is loaded, unloads it first to prevent port conflicts.
/// On exit, re-loads the managed service if it was previously active.
async fn run_serve_foreground(config: AppConfig, inspect: bool) -> anyhow::Result<()> {
    let was_loaded = service_is_loaded();
    if was_loaded {
        oo_warn!(
            crate::oo_log::modules::SERVER,
            "Foreground mode: unloading managed service to prevent port conflict"
        );
        let _ = service_unload();
        // Brief pause to let launchd release the port
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    let result = run_serve(config, inspect).await;

    if was_loaded {
        oo_info!(
            crate::oo_log::modules::SERVER,
            "Foreground mode exiting: re-loading managed service"
        );
        let _ = service_load();
    }

    result
}

/// Start the lightweight passthrough proxy (no PII scanning).
async fn run_passthrough(config: AppConfig) -> anyhow::Result<()> {
    passthrough::run(config).await
}

/// Handle service management commands.
async fn run_service(action: ServiceAction) -> anyhow::Result<()> {
    match action {
        ServiceAction::Install => service_install(),
        ServiceAction::Uninstall => service_uninstall(),
        ServiceAction::Start => {
            service_load()?;
            eprintln!("[OpenObscure] Service started (auto-restart enabled)");
            Ok(())
        }
        ServiceAction::Stop { passthrough } => {
            service_unload()?;
            eprintln!("[OpenObscure] Service stopped (auto-restart disabled)");
            if passthrough {
                eprintln!("[OpenObscure] Starting passthrough proxy...");
                // Load config for passthrough
                let config_path = service_config_path();
                let config = AppConfig::load(&config_path)?;
                passthrough::run(config).await?;
            }
            Ok(())
        }
        ServiceAction::Status => service_status(),
    }
}

// ── Service management helpers ──────────────────────────────────────────

const LAUNCHD_LABEL: &str = "com.openobscure.proxy";

fn plist_path() -> std::path::PathBuf {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    home.join("Library/LaunchAgents/com.openobscure.proxy.plist")
}

fn service_config_path() -> std::path::PathBuf {
    // Check if installed config exists, otherwise use default
    let installed = std::path::PathBuf::from("/usr/local/etc/openobscure/openobscure.toml");
    if installed.exists() {
        installed
    } else {
        std::path::PathBuf::from("config/openobscure.toml")
    }
}

fn service_is_loaded() -> bool {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("launchctl")
            .args(["list", LAUNCHD_LABEL])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("systemctl")
            .args(["--user", "is-active", "--quiet", "openobscure-proxy"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

fn service_load() -> anyhow::Result<()> {
    let path = plist_path();
    if !path.exists() {
        anyhow::bail!(
            "Service not installed. Run 'openobscure-proxy service install' first.\n  \
             Expected plist at: {}",
            path.display()
        );
    }

    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("launchctl")
            .args(["load", &path.to_string_lossy()])
            .status()?;
        if !status.success() {
            anyhow::bail!("launchctl load failed (exit {})", status);
        }
    }

    #[cfg(target_os = "linux")]
    {
        let status = std::process::Command::new("systemctl")
            .args(["--user", "start", "openobscure-proxy"])
            .status()?;
        if !status.success() {
            anyhow::bail!("systemctl start failed (exit {})", status);
        }
    }

    Ok(())
}

fn service_unload() -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let path = plist_path();
        let status = std::process::Command::new("launchctl")
            .args(["unload", &path.to_string_lossy()])
            .status()?;
        if !status.success() {
            anyhow::bail!("launchctl unload failed (exit {})", status);
        }
    }

    #[cfg(target_os = "linux")]
    {
        let status = std::process::Command::new("systemctl")
            .args(["--user", "stop", "openobscure-proxy"])
            .status()?;
        if !status.success() {
            anyhow::bail!("systemctl stop failed (exit {})", status);
        }
    }

    Ok(())
}

fn service_install() -> anyhow::Result<()> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let binary_path = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("Cannot determine binary path: {}", e))?;

    #[cfg(target_os = "macos")]
    {
        let plist = plist_path();

        // Create log directories
        let log_dir = std::path::Path::new("/usr/local/var/log/openobscure");
        let data_dir = std::path::Path::new("/usr/local/var/openobscure");
        let _ = std::fs::create_dir_all(log_dir);
        let _ = std::fs::create_dir_all(data_dir);

        // Ensure LaunchAgents directory exists
        if let Some(parent) = plist.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        // Determine config path
        let config_path = service_config_path();

        // Generate plist with correct binary path
        let plist_content = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>

    <key>ProgramArguments</key>
    <array>
        <string>{binary}</string>
        <string>serve</string>
    </array>

    <key>KeepAlive</key>
    <true/>

    <key>RunAtLoad</key>
    <true/>

    <key>StandardOutPath</key>
    <string>/usr/local/var/log/openobscure/stdout.log</string>

    <key>StandardErrorPath</key>
    <string>/usr/local/var/log/openobscure/stderr.log</string>

    <key>EnvironmentVariables</key>
    <dict>
        <key>OPENOBSCURE_CONFIG</key>
        <string>{config}</string>
    </dict>

    <key>WorkingDirectory</key>
    <string>/usr/local/var/openobscure</string>

    <key>ThrottleInterval</key>
    <integer>5</integer>
</dict>
</plist>"#,
            label = LAUNCHD_LABEL,
            binary = binary_path.display(),
            config = config_path.display()
        );

        std::fs::write(&plist, &plist_content)?;
        eprintln!("[OpenObscure] Service installed:");
        eprintln!("  Plist: {}", plist.display());
        eprintln!("  Binary: {}", binary_path.display());
        eprintln!("  Config: {}", config_path.display());
        eprintln!("  Logs: /usr/local/var/log/openobscure/");
        eprintln!();
        eprintln!("Run 'openobscure-proxy service start' to enable auto-restart.");
    }

    #[cfg(target_os = "linux")]
    {
        let unit_dir = dirs_next::config_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("~/.config"))
            .join("systemd/user");
        let _ = std::fs::create_dir_all(&unit_dir);
        let unit_path = unit_dir.join("openobscure-proxy.service");

        let config_path = service_config_path();

        let unit_content = format!(
            r#"[Unit]
Description=OpenObscure PII Privacy Proxy
After=network.target

[Service]
Type=simple
ExecStart={binary} serve
Restart=on-failure
RestartSec=5
Environment=OPENOBSCURE_CONFIG={config}

[Install]
WantedBy=default.target
"#,
            binary = binary_path.display(),
            config = config_path.display()
        );

        std::fs::write(&unit_path, &unit_content)?;

        let _ = std::process::Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();

        eprintln!("[OpenObscure] Service installed at {}", unit_path.display());
        eprintln!("Run 'openobscure-proxy service start' to enable.");
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        eprintln!("[OpenObscure] Service management is only supported on macOS and Linux.");
    }

    Ok(())
}

fn service_uninstall() -> anyhow::Result<()> {
    // Unload first if loaded
    if service_is_loaded() {
        let _ = service_unload();
    }

    #[cfg(target_os = "macos")]
    {
        let plist = plist_path();
        if plist.exists() {
            std::fs::remove_file(&plist)?;
            eprintln!(
                "[OpenObscure] Service uninstalled (removed {})",
                plist.display()
            );
        } else {
            eprintln!("[OpenObscure] Service not installed (no plist found)");
        }
    }

    #[cfg(target_os = "linux")]
    {
        let unit_path = dirs_next::config_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("~/.config"))
            .join("systemd/user/openobscure-proxy.service");
        if unit_path.exists() {
            std::fs::remove_file(&unit_path)?;
            let _ = std::process::Command::new("systemctl")
                .args(["--user", "daemon-reload"])
                .status();
            eprintln!(
                "[OpenObscure] Service uninstalled (removed {})",
                unit_path.display()
            );
        } else {
            eprintln!("[OpenObscure] Service not installed");
        }
    }

    Ok(())
}

fn service_status() -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let plist = plist_path();
        if !plist.exists() {
            eprintln!("[OpenObscure] Service not installed");
            eprintln!("  Run 'openobscure-proxy service install' to set up.");
            return Ok(());
        }

        let output = std::process::Command::new("launchctl")
            .args(["list", LAUNCHD_LABEL])
            .output()?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            eprintln!("[OpenObscure] Service is loaded (auto-restart enabled)");
            // Parse PID from launchctl output
            for line in stdout.lines() {
                let line = line.trim();
                if line.starts_with("\"PID\"") || line.contains("PID") {
                    eprintln!("  {}", line);
                }
            }
            // Also try to get the PID directly
            let list_output = std::process::Command::new("launchctl")
                .args(["list"])
                .output()?;
            let list_stdout = String::from_utf8_lossy(&list_output.stdout);
            for line in list_stdout.lines() {
                if line.contains(LAUNCHD_LABEL) {
                    eprintln!("  {}", line.trim());
                }
            }
        } else {
            eprintln!("[OpenObscure] Service is installed but not loaded");
            eprintln!("  Run 'openobscure-proxy service start' to enable.");
        }
        eprintln!("  Plist: {}", plist.display());
    }

    #[cfg(target_os = "linux")]
    {
        let output = std::process::Command::new("systemctl")
            .args(["--user", "status", "openobscure-proxy"])
            .output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        eprintln!("{}", stdout);
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        eprintln!("[OpenObscure] Service management is only supported on macOS and Linux.");
    }

    Ok(())
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

    // CLI --log-level overrides config. Suppress noisy ORT/CoreML session logs.
    let log_level = cli.log_level.as_deref().unwrap_or(&log_cfg.level);
    let filter_str = format!("{log_level},ort=error,session=error");
    let filter = EnvFilter::try_new(&filter_str).unwrap_or_else(|_| EnvFilter::new("info"));

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

    // Load name gazetteer if enabled
    let gazetteer = if config.scanner.gazetteer_enabled {
        let gaz = name_gazetteer::NameGazetteer::new();
        oo_info!(
            crate::oo_log::modules::SCANNER,
            "Name gazetteer loaded",
            first_names = gaz.first_name_count(),
            surnames = gaz.surname_count()
        );
        Some(gaz)
    } else {
        oo_info!(
            crate::oo_log::modules::SCANNER,
            "Name gazetteer disabled by config"
        );
        None
    };

    let mut scanner = match config.scanner.scanner_mode.as_str() {
        "regex" => {
            oo_info!(
                crate::oo_log::modules::SCANNER,
                "Scanner mode: regex-only (no semantic backend)"
            );
            HybridScanner::new(kw, None, gazetteer)
        }
        "ner" => {
            let ner = try_load_ner_pool(config, budget, threshold);
            if ner.is_none() {
                oo_warn!(
                    crate::oo_log::modules::SCANNER,
                    "Scanner mode 'ner' requested but model unavailable, using regex+keywords"
                );
            }
            HybridScanner::new(kw, ner, gazetteer)
        }
        "crf" => {
            let crf = try_load_crf(config, threshold);
            if crf.is_none() {
                oo_warn!(
                    crate::oo_log::modules::SCANNER,
                    "Scanner mode 'crf' requested but model unavailable, using regex+keywords"
                );
            }
            HybridScanner::with_crf(kw, crf, gazetteer)
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
                if let Some(ner) = try_load_ner_pool(config, budget, threshold) {
                    HybridScanner::new(kw, Some(ner), gazetteer)
                } else if budget.crf_enabled {
                    oo_info!(
                        crate::oo_log::modules::SCANNER,
                        "NER model unavailable, falling back to CRF"
                    );
                    if let Some(crf) = try_load_crf(config, threshold) {
                        HybridScanner::with_crf(kw, Some(crf), gazetteer)
                    } else {
                        oo_info!(
                            crate::oo_log::modules::SCANNER,
                            "No semantic model available, using regex+keywords only"
                        );
                        HybridScanner::new(kw, None, gazetteer)
                    }
                } else {
                    oo_info!(
                        crate::oo_log::modules::SCANNER,
                        "NER unavailable, no CRF in budget, using regex+keywords only"
                    );
                    HybridScanner::new(kw, None, gazetteer)
                }
            } else if budget.crf_enabled {
                if let Some(crf) = try_load_crf(config, threshold) {
                    HybridScanner::with_crf(kw, Some(crf), gazetteer)
                } else {
                    oo_info!(
                        crate::oo_log::modules::SCANNER,
                        "CRF model unavailable, using regex+keywords only"
                    );
                    HybridScanner::new(kw, None, gazetteer)
                }
            } else {
                oo_info!(
                    crate::oo_log::modules::SCANNER,
                    "Device budget: regex+keywords only"
                );
                HybridScanner::new(kw, None, gazetteer)
            }
        }
    };
    scanner.set_respect_code_fences(code_fences);
    let effective_bonus = if budget.ensemble_enabled {
        config.scanner.agreement_bonus
    } else {
        0.0
    };
    scanner.set_confidence_params(config.scanner.min_confidence, effective_bonus);
    scanner
}

fn try_load_ner_pool(
    config: &AppConfig,
    budget: &device_profile::FeatureBudget,
    threshold: f32,
) -> Option<ner_scanner::NerPool> {
    // Config override takes precedence over budget
    let model_name = config
        .scanner
        .ner_model
        .as_deref()
        .unwrap_or(budget.ner_model.as_str());

    let model_dir = match model_name {
        "tinybert" => config
            .scanner
            .ner_model_dir_lite
            .as_ref()
            .or(config.scanner.ner_model_dir.as_ref())?,
        _ => config.scanner.ner_model_dir.as_ref()?,
    };

    let pool_size = config.scanner.ner_pool_size;
    let model_path = std::path::Path::new(model_dir);
    let mut scanners = Vec::with_capacity(pool_size);

    for i in 0..pool_size {
        match ner_scanner::NerScanner::load(model_path, threshold) {
            Ok(s) => scanners.push(s),
            Err(e) => {
                if i == 0 {
                    oo_warn!(
                        crate::oo_log::modules::NER,
                        "NER scanner failed to load",
                        error = %e,
                        variant = %model_name
                    );
                    return None;
                }
                oo_warn!(
                    crate::oo_log::modules::NER,
                    "NER pool: partial load",
                    loaded = i,
                    requested = pool_size
                );
                break;
            }
        }
    }

    oo_info!(
        crate::oo_log::modules::NER,
        "NER pool ready",
        variant = %model_name,
        sessions = scanners.len(),
        model_dir = %model_dir
    );
    Some(ner_scanner::NerPool::new(scanners))
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

/// Path to the persisted health stats file (~/.openobscure/stats.json).
fn stats_file_path() -> std::path::PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let dir = home.join(".openobscure");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("stats.json")
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

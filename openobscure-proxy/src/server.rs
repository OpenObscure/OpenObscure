use std::net::SocketAddr;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;
use tower::ServiceBuilder;
use tower_http::trace::TraceLayer;

use crate::health::{FeatureBudgetSummary, HealthState, ReadinessState};
use crate::ner_endpoint::NerState;
use crate::proxy::AppState;

pub async fn run(
    state: AppState,
    auth_token: Option<String>,
    device_tier: String,
    feature_budget: FeatureBudgetSummary,
) -> anyhow::Result<()> {
    let config = state.config.clone();

    // Readiness state: starts Cold, transitions to Warming → Ready
    let readiness = Arc::new(AtomicU8::new(ReadinessState::Cold as u8));

    // Resolve key version for health endpoint (read once at startup,
    // updated if rotation occurs at runtime via the shared AtomicU32).
    let key_version = Arc::new(std::sync::atomic::AtomicU32::new(
        state.key_manager.current_version().await,
    ));

    let health_state = HealthState {
        stats: state.health.clone(),
        auth_token: auth_token.clone(),
        key_version,
        device_tier,
        feature_budget,
        readiness: readiness.clone(),
    };

    let ner_state = NerState {
        scanner: state.scanner.clone(),
        auth_token,
    };

    let app = Router::new()
        // Health endpoint on its own state (no proxy overhead)
        .route(
            "/_openobscure/health",
            get(crate::health::health_handler).with_state(health_state),
        )
        // NER scanning endpoint for L1 plugin
        .route(
            "/_openobscure/ner",
            post(crate::ner_endpoint::ner_handler).with_state(ner_state),
        )
        // All other routes go through the proxy handler
        .fallback(crate::proxy::proxy_handler)
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(tower_http::timeout::TimeoutLayer::with_status_code(
                    axum::http::StatusCode::GATEWAY_TIMEOUT,
                    std::time::Duration::from_secs(config.proxy.request_timeout_secs),
                )),
        )
        .with_state(state.clone());

    let addr = SocketAddr::new(
        config
            .proxy
            .listen_addr
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid listen address: {}", e))?,
        config.proxy.port,
    );

    oo_info!(crate::oo_log::modules::SERVER, "OpenObscure proxy starting", addr = %addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;

    // Start background eviction task
    let mapping_store = state.mapping_store.clone();
    tokio::spawn(async move {
        mapping_store.eviction_loop().await;
    });

    // Pre-warm NER model in background
    if config.proxy.enable_prewarm {
        let scanner = state.scanner.clone();
        let warm_readiness = readiness.clone();
        warm_readiness.store(ReadinessState::Warming as u8, Ordering::Relaxed);

        tokio::spawn(async move {
            let duration = tokio::task::spawn_blocking(move || scanner.warm())
                .await
                .unwrap_or_default();
            warm_readiness.store(ReadinessState::Ready as u8, Ordering::Relaxed);
            oo_info!(
                crate::oo_log::modules::SCANNER,
                "Model pre-warm complete",
                warm_ms = duration.as_millis()
            );
        });
    } else {
        readiness.store(ReadinessState::Ready as u8, Ordering::Relaxed);
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    oo_info!(
        crate::oo_log::modules::SERVER,
        "OpenObscure proxy shut down gracefully"
    );
    Ok(())
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install Ctrl+C handler");
    oo_info!(crate::oo_log::modules::SERVER, "Shutdown signal received");
}

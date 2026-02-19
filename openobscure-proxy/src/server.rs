use std::net::SocketAddr;

use axum::routing::get;
use axum::Router;
use tower::ServiceBuilder;
use tower_http::trace::TraceLayer;

use crate::health::{FeatureBudgetSummary, HealthState};
use crate::proxy::AppState;

pub async fn run(
    state: AppState,
    auth_token: Option<String>,
    device_tier: String,
    feature_budget: FeatureBudgetSummary,
) -> anyhow::Result<()> {
    let config = state.config.clone();

    // Resolve key version for health endpoint (read once at startup,
    // updated if rotation occurs at runtime via the shared AtomicU32).
    let key_version = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(
        state.key_manager.current_version().await,
    ));

    let health_state = HealthState {
        stats: state.health.clone(),
        auth_token,
        key_version,
        device_tier,
        feature_budget,
    };

    let app = Router::new()
        // Health endpoint on its own state (no proxy overhead)
        .route(
            "/_openobscure/health",
            get(crate::health::health_handler).with_state(health_state),
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

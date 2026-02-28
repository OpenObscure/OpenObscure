//! Lightweight passthrough proxy — forwards requests to upstream providers
//! without any PII scanning, FPE encryption, or model loading.
//!
//! Used as a graceful fallback when the full proxy is intentionally stopped
//! but the agent should keep working (without privacy protection).

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Method, Request, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use bytes::Bytes;
use http_body_util::BodyExt;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;

use crate::config::AppConfig;

/// Shared state for the passthrough proxy — just config + HTTP client.
#[derive(Clone)]
pub struct PassthroughState {
    pub config: Arc<AppConfig>,
    pub http_client: Client<
        hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
        Body,
    >,
}

/// Start the passthrough proxy server.
pub async fn run(config: AppConfig) -> anyhow::Result<()> {
    let addr = SocketAddr::new(
        config
            .proxy
            .listen_addr
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid listen address: {}", e))?,
        config.proxy.port,
    );

    // Install rustls CryptoProvider
    let _ = rustls::crypto::ring::default_provider().install_default();

    let https_connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .expect("Failed to load native TLS root certificates")
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .build();

    let http_client: Client<_, Body> = Client::builder(TokioExecutor::new()).build(https_connector);

    let state = PassthroughState {
        config: Arc::new(config),
        http_client,
    };

    let app = Router::new()
        .route("/_openobscure/health", get(passthrough_health))
        .fallback(passthrough_handler)
        .with_state(state);

    oo_warn!(
        crate::oo_log::modules::SERVER,
        "PASSTHROUGH MODE — PII protection is DISABLED. Forwarding requests directly to upstream."
    );
    oo_info!(crate::oo_log::modules::SERVER, "Passthrough proxy starting", addr = %addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;

    axum::serve(listener, app)
        .with_graceful_shutdown(passthrough_shutdown_signal())
        .await?;

    oo_info!(
        crate::oo_log::modules::SERVER,
        "Passthrough proxy shut down"
    );
    Ok(())
}

/// Health endpoint for passthrough mode — returns status "passthrough" so the
/// L1 heartbeat can detect this mode and use regex-only redaction.
async fn passthrough_health() -> impl IntoResponse {
    axum::Json(serde_json::json!({
        "status": "passthrough",
        "ready": true,
        "mode": "passthrough",
        "pii_protection": false
    }))
}

/// Forward requests to upstream providers without any scanning.
async fn passthrough_handler(
    State(state): State<PassthroughState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, StatusCode> {
    // Resolve provider by path prefix (reuse same logic as full proxy)
    let (provider_name, provider) =
        crate::proxy::resolve_provider(&state.config, &uri).ok_or(StatusCode::NOT_FOUND)?;

    // Strip route_prefix from path to get upstream path
    let path = uri.path();
    let upstream_path = path.strip_prefix(&provider.route_prefix).unwrap_or(path);

    let upstream_uri = format!(
        "{}{}{}",
        provider.upstream_url.trim_end_matches('/'),
        upstream_path,
        uri.query().map(|q| format!("?{}", q)).unwrap_or_default()
    );

    oo_info!(
        crate::oo_log::modules::PROXY,
        "Passthrough forward",
        provider = %provider_name,
        upstream = %upstream_uri
    );

    // Build upstream request — forward all headers except hop-by-hop
    let mut req_builder = Request::builder().method(method).uri(&upstream_uri);

    if let Some(h) = req_builder.headers_mut() {
        for (name, value) in &headers {
            // Skip hop-by-hop headers
            if matches!(
                name.as_str(),
                "host" | "connection" | "transfer-encoding" | "upgrade" | "proxy-connection"
            ) {
                continue;
            }
            // Skip headers configured for stripping
            if provider
                .strip_headers
                .iter()
                .any(|s| s.eq_ignore_ascii_case(name.as_str()))
            {
                continue;
            }
            h.insert(name.clone(), value.clone());
        }
    }

    let request = req_builder
        .body(Body::from(body))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Forward to upstream
    let upstream_response = state.http_client.request(request).await.map_err(|e| {
        oo_warn!(
            crate::oo_log::modules::PROXY,
            "Passthrough upstream error",
            error = %e
        );
        StatusCode::BAD_GATEWAY
    })?;

    // Forward response back — stream body directly without buffering
    let (parts, body) = upstream_response.into_parts();
    let body = body.map_err(|e| std::io::Error::other(e.to_string()));

    Ok(Response::from_parts(parts, Body::new(body)))
}

/// Shutdown signal for passthrough mode (same as full proxy).
async fn passthrough_shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {},
            _ = sigterm.recv() => {},
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.expect("failed to install Ctrl+C handler");
    }

    oo_info!(
        crate::oo_log::modules::SERVER,
        "Passthrough shutdown signal received"
    );
}

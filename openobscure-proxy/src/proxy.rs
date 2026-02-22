use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, HeaderValue, Method, Request, Response, StatusCode, Uri},
};
use bytes::Bytes;
use http_body_util::BodyExt;
use hyper_util::client::legacy::Client;

use crate::config::{AppConfig, FailMode, ProviderConfig};
use crate::health::HealthStats;
use crate::hybrid_scanner::HybridScanner;
use crate::image_pipeline::ImageModelManager;
use crate::key_manager::KeyManager;
use crate::kws_engine::KwsEngine;
use crate::mapping::MappingStore;
use crate::pii_types::PiiType;
use crate::vault::Vault;

type HttpsConnector =
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>;

/// Headers that must not be forwarded to upstream (hop-by-hop per RFC 7230).
const HOP_BY_HOP_HEADERS: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
    "host",
];

/// Shared application state passed to all handlers.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub scanner: Arc<HybridScanner>,
    pub key_manager: Arc<KeyManager>,
    pub mapping_store: MappingStore,
    pub http_client: Client<HttpsConnector, Body>,
    pub vault: Arc<Vault>,
    pub health: HealthStats,
    pub image_models: Option<Arc<ImageModelManager>>,
    pub kws_engine: Option<Arc<KwsEngine>>,
}

/// The main proxy handler. All requests flow through here.
pub async fn proxy_handler(
    State(state): State<AppState>,
    req: Request<Body>,
) -> Result<Response<Body>, StatusCode> {
    let request_start = std::time::Instant::now();
    let request_id = uuid::Uuid::new_v4();
    let method = req.method().clone();
    let uri = req.uri().clone();
    let original_headers = req.headers().clone();

    state.health.record_request();

    oo_info!(crate::oo_log::modules::PROXY, "Incoming request",
        request_id = %request_id,
        method = %method,
        uri = %uri);

    // 1. Resolve provider by path prefix
    let (provider_name, provider) = resolve_provider(&state.config, &uri).ok_or_else(|| {
        oo_warn!(crate::oo_log::modules::PROXY, "No provider matched", path = %uri.path());
        StatusCode::NOT_FOUND
    })?;

    oo_debug!(crate::oo_log::modules::PROXY, "Matched provider", request_id = %request_id, provider = %provider_name);

    // 2. Buffer the request body
    let body_bytes = buffer_body(req, state.config.proxy.max_body_bytes).await?;

    // 3. Check if body is scannable (JSON content type or missing content type with body)
    let should_scan = !body_bytes.is_empty()
        && state.config.scanner.enabled
        && state.config.fpe.enabled
        && is_json_content(&original_headers);

    if !body_bytes.is_empty() && !is_json_content(&original_headers) {
        oo_debug!(crate::oo_log::modules::PROXY, "Non-JSON body, passing through without scanning",
            request_id = %request_id,
            content_type = ?original_headers.get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap_or("<invalid>")));
    }

    // 4. Scan and encrypt PII (only for JSON bodies)
    let versioned_engine = state.key_manager.current().await;
    let (modified_body, mappings) = if should_scan {
        let scan_start = std::time::Instant::now();
        match crate::body::process_request_body(
            &body_bytes,
            &request_id,
            &state.scanner,
            &versioned_engine.engine,
            versioned_engine.version,
            &state.config.scanner.skip_fields,
            state.image_models.as_ref(),
            state.kws_engine.as_ref(),
        ) {
            Ok((body, mappings, image_stats)) => {
                state.health.scan_latency.record(scan_start.elapsed());
                if !mappings.is_empty() {
                    let count = mappings.by_ciphertext.len() as u64;
                    state.health.record_pii_matches(count);
                    log_pii_stats(&request_id, &mappings);
                }
                // Record image processing stats
                for is in &image_stats {
                    state.health.record_images_processed(1);
                    state.health.record_faces_blurred(is.faces_blurred as u64);
                    state
                        .health
                        .record_text_regions(is.text_regions_found as u64);
                    if is.nsfw_detected {
                        state.health.record_nsfw_blocked(1);
                    }
                    if is.is_screenshot {
                        state.health.record_screenshots_detected(1);
                    }
                }
                (body, mappings)
            }
            Err(e) => {
                state.health.scan_latency.record(scan_start.elapsed());
                match state.config.proxy.fail_mode {
                    FailMode::Open => {
                        oo_warn!(crate::oo_log::modules::PROXY, "Body processing failed (fail-open), forwarding original",
                            request_id = %request_id,
                            error = %e);
                        (
                            body_bytes.clone(),
                            crate::mapping::RequestMappings::new(request_id),
                        )
                    }
                    FailMode::Closed => {
                        oo_error!(crate::oo_log::modules::PROXY, "Body processing failed (fail-closed), rejecting request",
                            request_id = %request_id,
                            error = %e);
                        return Err(StatusCode::BAD_GATEWAY);
                    }
                }
            }
        }
    } else {
        (body_bytes, crate::mapping::RequestMappings::new(request_id))
    };

    // 4. Store mappings for response decryption
    let has_mappings = !mappings.is_empty();
    if has_mappings {
        state.mapping_store.insert(mappings.clone()).await;
    }

    // 5. Build upstream request (passthrough original headers)
    let upstream_req = build_upstream_request(
        method,
        &uri,
        provider,
        &provider_name,
        &original_headers,
        &modified_body,
        &state.vault,
    )
    .map_err(|e| {
        oo_error!(crate::oo_log::modules::PROXY, "Failed to build upstream request", request_id = %request_id, error = %e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // 6. Forward to upstream
    let upstream_resp = state
        .http_client
        .request(upstream_req)
        .await
        .map_err(|e| {
            oo_error!(crate::oo_log::modules::PROXY, "Upstream request failed", request_id = %request_id, error = %e);
            StatusCode::BAD_GATEWAY
        })?;

    // 7. Check if response is SSE (text/event-stream) for streaming path
    let (parts, resp_body) = upstream_resp.into_parts();
    let is_sse = is_event_stream(&parts.headers);

    if is_sse && has_mappings {
        // 7a. SSE streaming path: stream through with per-chunk decryption
        oo_debug!(crate::oo_log::modules::PROXY, "SSE response detected, streaming with per-chunk decryption",
            request_id = %request_id);

        let mappings = state.mapping_store.get(&request_id).await;
        let mapping_store = state.mapping_store.clone();
        let req_id = request_id;

        // Build streaming body that decrypts each chunk
        let stream =
            futures_util::stream::unfold((resp_body, mappings), move |(mut body, mappings)| {
                let mapping_store = mapping_store.clone();
                let req_id = req_id;
                async move {
                    match body.frame().await {
                        Some(Ok(frame)) => {
                            if let Ok(data) = frame.into_data() {
                                let chunk = if let Some(ref m) = mappings {
                                    crate::body::process_response_body(&data, m)
                                } else {
                                    data
                                };
                                Some((Ok::<_, std::convert::Infallible>(chunk), (body, mappings)))
                            } else {
                                // Trailers or other frame types — yield empty bytes
                                Some((Ok(Bytes::new()), (body, mappings)))
                            }
                        }
                        Some(Err(_)) => {
                            // Stream error — clean up mappings and end
                            mapping_store.remove(&req_id).await;
                            None
                        }
                        None => {
                            // Stream complete — clean up mappings
                            mapping_store.remove(&req_id).await;
                            None
                        }
                    }
                }
            });

        let stream_body = Body::from_stream(stream);
        let mut response = Response::new(stream_body);
        *response.status_mut() = parts.status;
        *response.version_mut() = parts.version;
        // Forward all headers except content-length (streaming has no fixed length)
        for (key, value) in &parts.headers {
            if key == "content-length" {
                continue;
            }
            response.headers_mut().insert(key.clone(), value.clone());
        }

        state.health.request_latency.record(request_start.elapsed());
        oo_info!(crate::oo_log::modules::PROXY, "SSE response streaming",
            request_id = %request_id,
            status = %response.status());

        return Ok(response);
    }

    // 7b. Non-SSE path: buffer and process response (existing behavior)
    let resp_bytes = resp_body
        .collect()
        .await
        .map_err(|e| {
            oo_error!(crate::oo_log::modules::PROXY, "Failed to read upstream response", request_id = %request_id, error = %e);
            StatusCode::BAD_GATEWAY
        })?
        .to_bytes();

    // 8. Decrypt FPE values in response
    let final_body = if has_mappings {
        if let Some(mappings) = state.mapping_store.get(&request_id).await {
            let decrypted = crate::body::process_response_body(&resp_bytes, &mappings);
            state.mapping_store.remove(&request_id).await;
            decrypted
        } else {
            resp_bytes
        }
    } else {
        resp_bytes
    };

    // 9. Rebuild response with correct content-length
    let mut response = Response::new(Body::from(final_body.clone()));
    *response.status_mut() = parts.status;
    *response.version_mut() = parts.version;
    // Copy response headers, replacing content-length and removing transfer-encoding
    // (we buffer the full body so chunked encoding no longer applies)
    for (key, value) in &parts.headers {
        if key == "content-length" || key == "transfer-encoding" {
            continue;
        }
        response.headers_mut().insert(key.clone(), value.clone());
    }
    response.headers_mut().insert(
        "content-length",
        HeaderValue::from_str(&final_body.len().to_string()).unwrap(),
    );

    state.health.request_latency.record(request_start.elapsed());
    oo_info!(crate::oo_log::modules::PROXY, "Response sent",
        request_id = %request_id,
        status = %response.status());

    Ok(response)
}

/// Find the provider config that matches the request URI path prefix.
fn resolve_provider<'a>(config: &'a AppConfig, uri: &Uri) -> Option<(String, &'a ProviderConfig)> {
    let path = uri.path();
    // Try longest prefix first for specificity
    let mut candidates: Vec<_> = config
        .providers
        .iter()
        .filter(|(_, p)| path.starts_with(&p.route_prefix))
        .collect();
    candidates.sort_by(|a, b| b.1.route_prefix.len().cmp(&a.1.route_prefix.len()));
    candidates
        .into_iter()
        .next()
        .map(|(name, provider)| (name.clone(), provider))
}

/// Build the upstream HTTP request.
///
/// Auth strategy (passthrough):
/// Forward all original headers from the host agent (except hop-by-hop and strip_headers).
/// Users share one set of API keys with the host agent — no duplicate key management.
fn build_upstream_request(
    method: Method,
    original_uri: &Uri,
    provider: &ProviderConfig,
    _provider_name: &str,
    original_headers: &HeaderMap,
    body: &Bytes,
    _vault: &Vault,
) -> Result<Request<Body>, String> {
    // Strip the route prefix from the path
    let original_path = original_uri.path();
    let upstream_path = original_path
        .strip_prefix(&provider.route_prefix)
        .unwrap_or(original_path);

    // Build upstream URI
    let upstream_uri = format!(
        "{}{}{}",
        provider.upstream_url.trim_end_matches('/'),
        upstream_path,
        original_uri
            .query()
            .map(|q| format!("?{}", q))
            .unwrap_or_default()
    );

    let uri: Uri = upstream_uri
        .parse()
        .map_err(|e| format!("Invalid upstream URI: {}", e))?;

    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::from(body.clone()))
        .map_err(|e| format!("Failed to build request: {}", e))?;

    // --- Header forwarding (passthrough-first) ---
    // Forward all original headers from the host agent, skipping hop-by-hop
    // and any provider-specific strip_headers.
    let headers = req.headers_mut();
    for (key, value) in original_headers.iter() {
        let key_str = key.as_str().to_lowercase();

        // Skip hop-by-hop headers
        if HOP_BY_HOP_HEADERS.contains(&key_str.as_str()) {
            continue;
        }

        // Skip provider-specific stripped headers
        if provider
            .strip_headers
            .iter()
            .any(|h| h.eq_ignore_ascii_case(&key_str))
        {
            continue;
        }

        headers.insert(key.clone(), value.clone());
    }

    // Always set correct content-length for the (possibly modified) body
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&body.len().to_string())
            .map_err(|e| format!("Invalid content-length: {}", e))?,
    );

    // If no content-type was forwarded and we have a body, default to JSON
    if !body.is_empty() && !headers.contains_key(header::CONTENT_TYPE) {
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }

    Ok(req)
}

/// Check if the request Content-Type indicates JSON.
/// Returns true for `application/json`, any `+json` suffix, or missing Content-Type
/// (missing Content-Type with a body is treated as potentially JSON for scanning).
fn is_json_content(headers: &HeaderMap) -> bool {
    match headers.get(header::CONTENT_TYPE) {
        None => true, // No Content-Type with body → scan optimistically
        Some(val) => {
            let ct = val.to_str().unwrap_or("");
            let ct_lower = ct.to_lowercase();
            ct_lower.contains("application/json") || ct_lower.contains("+json")
        }
    }
}

/// Check if the response Content-Type indicates Server-Sent Events.
fn is_event_stream(headers: &hyper::HeaderMap) -> bool {
    match headers.get(hyper::header::CONTENT_TYPE) {
        Some(val) => {
            let ct = val.to_str().unwrap_or("");
            ct.to_lowercase().contains("text/event-stream")
        }
        None => false,
    }
}

/// Log per-PII-type match counts without logging actual PII values.
fn log_pii_stats(request_id: &uuid::Uuid, mappings: &crate::mapping::RequestMappings) {
    let mut counts: std::collections::HashMap<PiiType, usize> = std::collections::HashMap::new();
    for mapping in mappings.by_ciphertext.values() {
        *counts.entry(mapping.pii_type).or_insert(0) += 1;
    }

    let total: usize = counts.values().sum();
    let breakdown: Vec<String> = counts
        .iter()
        .map(|(pii_type, count)| format!("{}={}", pii_type, count))
        .collect();

    oo_info!(crate::oo_log::modules::PROXY, "PII encrypted in request",
        request_id = %request_id,
        pii_total = total,
        pii_breakdown = %breakdown.join(", "));
}

/// Check if request body is JSON content (public for testing).
#[cfg(test)]
pub fn is_json_content_pub(headers: &HeaderMap) -> bool {
    is_json_content(headers)
}

/// Buffer the request body up to the size limit.
async fn buffer_body(req: Request<Body>, max_bytes: usize) -> Result<Bytes, StatusCode> {
    let body = req.into_body();
    let collected = body.collect().await.map_err(|e| {
        oo_error!(crate::oo_log::modules::PROXY, "Failed to read request body", error = %e);
        StatusCode::BAD_REQUEST
    })?;
    let bytes = collected.to_bytes();
    if bytes.len() > max_bytes {
        oo_warn!(
            crate::oo_log::modules::PROXY,
            "Request body exceeds size limit",
            size = bytes.len(),
            limit = max_bytes
        );
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppConfig, ProviderConfig};
    use axum::http::{HeaderMap, HeaderValue, Method, Uri};
    use std::collections::HashMap;

    fn test_config_with_providers() -> AppConfig {
        let mut providers = HashMap::new();
        providers.insert(
            "anthropic".to_string(),
            ProviderConfig {
                upstream_url: "https://api.anthropic.com".to_string(),
                route_prefix: "/anthropic".to_string(),
                strip_headers: vec!["x-internal".to_string()],
            },
        );
        providers.insert(
            "openai".to_string(),
            ProviderConfig {
                upstream_url: "https://api.openai.com".to_string(),
                route_prefix: "/openai".to_string(),
                strip_headers: vec![],
            },
        );
        providers.insert(
            "openai_v2".to_string(),
            ProviderConfig {
                upstream_url: "https://api.openai.com/v2".to_string(),
                route_prefix: "/openai/v2".to_string(),
                strip_headers: vec![],
            },
        );
        AppConfig {
            proxy: crate::config::ProxyConfig::default(),
            providers,
            fpe: crate::config::FpeConfig::default(),
            scanner: crate::config::ScannerConfig::default(),
            logging: crate::config::LoggingConfig::default(),
            image: crate::config::ImageConfig::default(),
            voice: crate::config::VoiceConfig::default(),
        }
    }

    // --- resolve_provider ---

    #[test]
    fn test_resolve_provider_matches_prefix() {
        let config = test_config_with_providers();
        let uri: Uri = "/anthropic/v1/messages".parse().unwrap();
        let result = resolve_provider(&config, &uri);
        assert!(result.is_some());
        let (name, provider) = result.unwrap();
        assert_eq!(name, "anthropic");
        assert_eq!(provider.upstream_url, "https://api.anthropic.com");
    }

    #[test]
    fn test_resolve_provider_longest_prefix_wins() {
        let config = test_config_with_providers();
        let uri: Uri = "/openai/v2/chat".parse().unwrap();
        let result = resolve_provider(&config, &uri);
        assert!(result.is_some());
        let (name, _) = result.unwrap();
        assert_eq!(name, "openai_v2");
    }

    #[test]
    fn test_resolve_provider_short_prefix() {
        let config = test_config_with_providers();
        let uri: Uri = "/openai/v1/completions".parse().unwrap();
        let result = resolve_provider(&config, &uri);
        assert!(result.is_some());
        let (name, _) = result.unwrap();
        assert_eq!(name, "openai");
    }

    #[test]
    fn test_resolve_provider_no_match() {
        let config = test_config_with_providers();
        let uri: Uri = "/unknown/api".parse().unwrap();
        let result = resolve_provider(&config, &uri);
        assert!(result.is_none());
    }

    #[test]
    fn test_resolve_provider_root_path_no_match() {
        let config = test_config_with_providers();
        let uri: Uri = "/".parse().unwrap();
        assert!(resolve_provider(&config, &uri).is_none());
    }

    #[test]
    fn test_resolve_provider_empty_providers() {
        let mut config = test_config_with_providers();
        config.providers.clear();
        let uri: Uri = "/anthropic/v1/messages".parse().unwrap();
        assert!(resolve_provider(&config, &uri).is_none());
    }

    // --- is_json_content ---

    #[test]
    fn test_is_json_application_json() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        assert!(is_json_content(&headers));
    }

    #[test]
    fn test_is_json_with_charset() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
        assert!(is_json_content(&headers));
    }

    #[test]
    fn test_is_json_plus_json_suffix() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/vnd.api+json"),
        );
        assert!(is_json_content(&headers));
    }

    #[test]
    fn test_is_json_missing_content_type() {
        let headers = HeaderMap::new();
        assert!(is_json_content(&headers));
    }

    #[test]
    fn test_is_not_json_text_plain() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        assert!(!is_json_content(&headers));
    }

    #[test]
    fn test_is_not_json_multipart() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("multipart/form-data"),
        );
        assert!(!is_json_content(&headers));
    }

    #[test]
    fn test_is_not_json_octet_stream() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
        assert!(!is_json_content(&headers));
    }

    // --- is_event_stream ---

    #[test]
    fn test_is_event_stream_true() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        assert!(is_event_stream(&headers));
    }

    #[test]
    fn test_is_event_stream_false() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        assert!(!is_event_stream(&headers));
    }

    #[test]
    fn test_is_event_stream_missing() {
        let headers = HeaderMap::new();
        assert!(!is_event_stream(&headers));
    }

    // --- build_upstream_request ---

    #[test]
    fn test_build_upstream_strips_prefix() {
        let provider = ProviderConfig {
            upstream_url: "https://api.anthropic.com".to_string(),
            route_prefix: "/anthropic".to_string(),
            strip_headers: vec![],
        };
        let uri: Uri = "/anthropic/v1/messages".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let body = Bytes::from(r#"{"test":true}"#);
        let vault = Vault::new("test");

        let req = build_upstream_request(
            Method::POST,
            &uri,
            &provider,
            "anthropic",
            &headers,
            &body,
            &vault,
        )
        .unwrap();

        assert_eq!(
            req.uri().to_string(),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(req.method(), Method::POST);
    }

    #[test]
    fn test_build_upstream_preserves_query() {
        let provider = ProviderConfig {
            upstream_url: "https://api.example.com".to_string(),
            route_prefix: "/api".to_string(),
            strip_headers: vec![],
        };
        let uri: Uri = "/api/search?q=test&limit=10".parse().unwrap();
        let headers = HeaderMap::new();
        let body = Bytes::new();
        let vault = Vault::new("test");

        let req =
            build_upstream_request(Method::GET, &uri, &provider, "api", &headers, &body, &vault)
                .unwrap();

        assert!(req.uri().to_string().contains("?q=test&limit=10"));
    }

    #[test]
    fn test_build_upstream_strips_hop_by_hop() {
        let provider = ProviderConfig {
            upstream_url: "https://api.example.com".to_string(),
            route_prefix: "/api".to_string(),
            strip_headers: vec![],
        };
        let uri: Uri = "/api/endpoint".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("connection", HeaderValue::from_static("keep-alive"));
        headers.insert("keep-alive", HeaderValue::from_static("timeout=5"));
        headers.insert("transfer-encoding", HeaderValue::from_static("chunked"));
        headers.insert("authorization", HeaderValue::from_static("Bearer sk-test"));
        let body = Bytes::from("{}");
        let vault = Vault::new("test");

        let req = build_upstream_request(
            Method::POST,
            &uri,
            &provider,
            "api",
            &headers,
            &body,
            &vault,
        )
        .unwrap();

        // Hop-by-hop headers should be stripped
        assert!(!req.headers().contains_key("connection"));
        assert!(!req.headers().contains_key("keep-alive"));
        assert!(!req.headers().contains_key("transfer-encoding"));
        // Auth header should be forwarded (passthrough-first)
        assert!(req.headers().contains_key("authorization"));
    }

    #[test]
    fn test_build_upstream_strips_provider_headers() {
        let provider = ProviderConfig {
            upstream_url: "https://api.example.com".to_string(),
            route_prefix: "/api".to_string(),
            strip_headers: vec!["x-internal".to_string(), "x-debug".to_string()],
        };
        let uri: Uri = "/api/endpoint".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-internal", HeaderValue::from_static("secret"));
        headers.insert("x-debug", HeaderValue::from_static("true"));
        headers.insert("x-custom", HeaderValue::from_static("keep-me"));
        let body = Bytes::from("{}");
        let vault = Vault::new("test");

        let req = build_upstream_request(
            Method::POST,
            &uri,
            &provider,
            "api",
            &headers,
            &body,
            &vault,
        )
        .unwrap();

        assert!(!req.headers().contains_key("x-internal"));
        assert!(!req.headers().contains_key("x-debug"));
        assert!(req.headers().contains_key("x-custom"));
    }

    #[test]
    fn test_build_upstream_sets_content_length() {
        let provider = ProviderConfig {
            upstream_url: "https://api.example.com".to_string(),
            route_prefix: "/api".to_string(),
            strip_headers: vec![],
        };
        let uri: Uri = "/api/endpoint".parse().unwrap();
        let headers = HeaderMap::new();
        let body = Bytes::from(r#"{"messages":[]}"#);
        let vault = Vault::new("test");

        let req = build_upstream_request(
            Method::POST,
            &uri,
            &provider,
            "api",
            &headers,
            &body,
            &vault,
        )
        .unwrap();

        let cl = req
            .headers()
            .get("content-length")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(cl, "15");
    }

    #[test]
    fn test_build_upstream_defaults_json_content_type() {
        let provider = ProviderConfig {
            upstream_url: "https://api.example.com".to_string(),
            route_prefix: "/api".to_string(),
            strip_headers: vec![],
        };
        let uri: Uri = "/api/endpoint".parse().unwrap();
        let headers = HeaderMap::new(); // No content-type
        let body = Bytes::from("{}");
        let vault = Vault::new("test");

        let req = build_upstream_request(
            Method::POST,
            &uri,
            &provider,
            "api",
            &headers,
            &body,
            &vault,
        )
        .unwrap();

        assert_eq!(
            req.headers().get("content-type").unwrap().to_str().unwrap(),
            "application/json"
        );
    }

    // --- buffer_body ---

    #[tokio::test]
    async fn test_buffer_body_within_limit() {
        let body = Body::from("hello");
        let req = Request::builder().body(body).unwrap();
        let result = buffer_body(req, 1024).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Bytes::from("hello"));
    }

    #[tokio::test]
    async fn test_buffer_body_exceeds_limit() {
        let body = Body::from("a]".repeat(1000));
        let req = Request::builder().body(body).unwrap();
        let result = buffer_body(req, 100).await;
        assert_eq!(result.unwrap_err(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn test_buffer_body_empty() {
        let body = Body::empty();
        let req = Request::builder().body(body).unwrap();
        let result = buffer_body(req, 1024).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }
}

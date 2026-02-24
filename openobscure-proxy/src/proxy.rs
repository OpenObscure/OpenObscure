use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, HeaderValue, Method, Request, Response, StatusCode, Uri},
};
use bytes::Bytes;
use futures_util::StreamExt;
use http_body_util::{BodyExt, BodyStream};
use hyper_util::client::legacy::Client;

use crate::config::{AppConfig, FailMode, ProviderConfig};
use crate::health::HealthStats;
use crate::hybrid_scanner::HybridScanner;
use crate::image_pipeline::ImageModelManager;
use crate::key_manager::KeyManager;
use crate::kws_engine::KwsEngine;
use crate::mapping::MappingStore;
use crate::pii_types::PiiType;
use crate::response_integrity::ResponseIntegrityScanner;
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
    pub response_integrity: Option<Arc<ResponseIntegrityScanner>>,
    pub device_tier: crate::device_profile::CapabilityTier,
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

    // 2. Buffer the request body (tier-aware limit)
    let effective_limit = state.config.proxy.body_limit_for_tier(state.device_tier);
    let body_bytes = buffer_body(req, effective_limit).await?;

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
                    for _ in 0..is.onnx_panics {
                        state.health.record_onnx_panic();
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
        // 7a. SSE streaming path: stream through with cross-frame accumulation
        oo_debug!(crate::oo_log::modules::PROXY, "SSE response detected, streaming with accumulation buffer",
            request_id = %request_id);

        let mappings = state.mapping_store.get(&request_id).await;
        let mapping_store = state.mapping_store.clone();
        let req_id = request_id;
        let sse_buffer_size = state.config.proxy.sse_buffer_size;
        let sse_flush_timeout =
            std::time::Duration::from_millis(state.config.proxy.sse_flush_timeout_ms);
        let accumulator = crate::sse_accumulator::SseAccumulator::new(sse_buffer_size);

        // Build streaming body that decrypts each chunk with cross-frame accumulation.
        // `done` flag prevents polling after stream end (flush-then-end pattern).
        let stream = futures_util::stream::unfold(
            (resp_body, mappings, accumulator, false),
            move |(mut body, mappings, mut accumulator, done)| {
                let mapping_store = mapping_store.clone();
                let req_id = req_id;
                async move {
                    if done {
                        return None;
                    }

                    match tokio::time::timeout(sse_flush_timeout, body.frame()).await {
                        Ok(Some(Ok(frame))) => {
                            if let Ok(data) = frame.into_data() {
                                let chunk = if let Some(ref m) = mappings {
                                    accumulator.feed(&data, m)
                                } else {
                                    data
                                };
                                Some((
                                    Ok::<_, std::convert::Infallible>(chunk),
                                    (body, mappings, accumulator, false),
                                ))
                            } else {
                                // Trailers or other frame types — yield empty bytes
                                Some((Ok(Bytes::new()), (body, mappings, accumulator, false)))
                            }
                        }
                        Ok(Some(Err(_))) => {
                            // Stream error — flush accumulator and end
                            let flush = if let Some(ref m) = mappings {
                                accumulator.flush(m)
                            } else {
                                Bytes::new()
                            };
                            mapping_store.remove(&req_id).await;
                            if !flush.is_empty() {
                                Some((Ok(flush), (body, mappings, accumulator, true)))
                            } else {
                                None
                            }
                        }
                        Ok(None) => {
                            // Stream complete — flush remaining buffer
                            let flush = if let Some(ref m) = mappings {
                                accumulator.flush(m)
                            } else {
                                Bytes::new()
                            };
                            mapping_store.remove(&req_id).await;
                            if !flush.is_empty() {
                                Some((Ok(flush), (body, mappings, accumulator, true)))
                            } else {
                                None
                            }
                        }
                        Err(_timeout) => {
                            // Flush timeout — emit whatever is in the buffer
                            let flush = if let Some(ref m) = mappings {
                                accumulator.flush(m)
                            } else {
                                Bytes::new()
                            };
                            Some((Ok(flush), (body, mappings, accumulator, false)))
                        }
                    }
                }
            },
        );

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

    // 8b. Response integrity scan (cognitive firewall)
    // No content-type check here: extract_response_text returns None for non-JSON (fail-open).
    let final_body = if let Some(ref ri_scanner) = state.response_integrity {
        scan_response_integrity(&final_body, ri_scanner, &request_id, &state)
    } else {
        final_body
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

/// Extract text content from an LLM JSON response body.
/// Supports Anthropic (`content[].text`) and OpenAI (`choices[].message.content`) formats.
fn extract_response_text(body: &Bytes) -> Option<String> {
    let json: serde_json::Value = serde_json::from_slice(body).ok()?;

    // Anthropic format: { "content": [{ "type": "text", "text": "..." }] }
    if let Some(content_arr) = json.get("content").and_then(|c| c.as_array()) {
        let texts: Vec<&str> = content_arr
            .iter()
            .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
            .collect();
        if !texts.is_empty() {
            return Some(texts.join(" "));
        }
    }

    // OpenAI format: { "choices": [{ "message": { "content": "..." } }] }
    if let Some(choices) = json.get("choices").and_then(|c| c.as_array()) {
        let texts: Vec<&str> = choices
            .iter()
            .filter_map(|choice| {
                choice
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_str())
            })
            .collect();
        if !texts.is_empty() {
            return Some(texts.join(" "));
        }
    }

    None
}

/// Inject a warning label into the first text content of an LLM JSON response.
/// Returns modified body bytes, or None on parse/format error.
fn inject_warning_label(body: &Bytes, label: &str) -> Option<Bytes> {
    let mut json: serde_json::Value = serde_json::from_slice(body).ok()?;

    // Anthropic format
    if let Some(content_arr) = json.get_mut("content").and_then(|c| c.as_array_mut()) {
        for block in content_arr.iter_mut() {
            if let Some(text) = block
                .get_mut("text")
                .and_then(|t| t.as_str().map(String::from))
            {
                block["text"] = serde_json::Value::String(format!("{}{}", label, text));
                let serialized = serde_json::to_vec(&json).ok()?;
                return Some(Bytes::from(serialized));
            }
        }
    }

    // OpenAI format
    if let Some(choices) = json.get_mut("choices").and_then(|c| c.as_array_mut()) {
        for choice in choices.iter_mut() {
            if let Some(content) = choice
                .get_mut("message")
                .and_then(|m| m.get_mut("content"))
                .and_then(|c| c.as_str().map(String::from))
            {
                choice["message"]["content"] =
                    serde_json::Value::String(format!("{}{}", label, content));
                let serialized = serde_json::to_vec(&json).ok()?;
                return Some(Bytes::from(serialized));
            }
        }
    }

    None
}

/// Orchestrate response integrity scanning. Fail-open: returns original body on any error.
fn scan_response_integrity(
    body: &Bytes,
    scanner: &ResponseIntegrityScanner,
    request_id: &uuid::Uuid,
    state: &AppState,
) -> Bytes {
    // Extract text from response JSON
    let text = match extract_response_text(body) {
        Some(t) => t,
        None => return body.clone(), // Not parseable or no text content → pass through
    };

    state.health.record_ri_scan();

    // Scan for persuasion techniques
    let report = match scanner.scan(&text) {
        Some(r) => r,
        None => return body.clone(), // Clean or filtered → pass through
    };

    let flag_count = report.flags.len() as u64;
    state.health.record_ri_flags(flag_count);

    let category_names: Vec<String> = {
        let mut names: Vec<String> = report.categories.iter().map(|c| c.to_string()).collect();
        names.sort();
        names
    };

    oo_info!(crate::oo_log::modules::RESPONSE_INTEGRITY, "Persuasion techniques detected",
        request_id = %request_id,
        severity = %report.severity,
        flags = flag_count,
        categories = %category_names.join(", "),
        scan_time_us = report.scan_time_us);

    // If log_only mode, don't modify the response
    if state.config.response_integrity.log_only {
        return body.clone();
    }

    // Inject warning label
    let label = ResponseIntegrityScanner::format_warning_label(&report);
    match inject_warning_label(body, &label) {
        Some(modified) => modified,
        None => body.clone(), // Fail-open: return original on injection error
    }
}

/// Check if request body is JSON content (public for testing).
#[cfg(test)]
pub fn is_json_content_pub(headers: &HeaderMap) -> bool {
    is_json_content(headers)
}

/// Buffer the request body with streaming size enforcement.
///
/// Rejects as soon as accumulated bytes exceed the limit, without
/// buffering the full body first.
async fn buffer_body(req: Request<Body>, max_bytes: usize) -> Result<Bytes, StatusCode> {
    let body = req.into_body();
    let mut collected = Vec::new();
    let mut stream = BodyStream::new(body);

    while let Some(frame_result) = stream.next().await {
        let frame = frame_result.map_err(|e| {
            oo_error!(crate::oo_log::modules::PROXY, "Failed to read request body", error = %e);
            StatusCode::BAD_REQUEST
        })?;
        if let Ok(data) = frame.into_data() {
            collected.extend_from_slice(&data);
            if collected.len() > max_bytes {
                oo_warn!(
                    crate::oo_log::modules::PROXY,
                    "Request body exceeds size limit",
                    accumulated = collected.len(),
                    limit = max_bytes
                );
                return Err(StatusCode::PAYLOAD_TOO_LARGE);
            }
        }
    }
    Ok(Bytes::from(collected))
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
            response_integrity: crate::config::ResponseIntegrityConfig::default(),
        }
    }

    // --- extract_response_text ---

    #[test]
    fn test_extract_anthropic_response() {
        let body = serde_json::json!({
            "content": [{"type": "text", "text": "Hello, how can I help you?"}],
            "model": "claude-3-5-sonnet"
        });
        let bytes = Bytes::from(serde_json::to_vec(&body).unwrap());
        let text = extract_response_text(&bytes);
        assert_eq!(text.unwrap(), "Hello, how can I help you?");
    }

    #[test]
    fn test_extract_openai_response() {
        let body = serde_json::json!({
            "choices": [{"message": {"content": "Hello there!"}, "index": 0}]
        });
        let bytes = Bytes::from(serde_json::to_vec(&body).unwrap());
        let text = extract_response_text(&bytes);
        assert_eq!(text.unwrap(), "Hello there!");
    }

    #[test]
    fn test_extract_non_json_returns_none() {
        let bytes = Bytes::from("not json at all");
        assert!(extract_response_text(&bytes).is_none());
    }

    #[test]
    fn test_extract_no_text_content() {
        let body = serde_json::json!({"id": "msg_123", "type": "message"});
        let bytes = Bytes::from(serde_json::to_vec(&body).unwrap());
        assert!(extract_response_text(&bytes).is_none());
    }

    // --- inject_warning_label ---

    #[test]
    fn test_inject_label_anthropic() {
        let body = serde_json::json!({
            "content": [{"type": "text", "text": "Buy now!"}]
        });
        let bytes = Bytes::from(serde_json::to_vec(&body).unwrap());
        let result = inject_warning_label(&bytes, "WARNING: ");
        assert!(result.is_some());
        let modified: serde_json::Value = serde_json::from_slice(&result.unwrap()).unwrap();
        let text = modified["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("WARNING: "));
        assert!(text.contains("Buy now!"));
    }

    #[test]
    fn test_inject_label_openai() {
        let body = serde_json::json!({
            "choices": [{"message": {"content": "Act now!"}, "index": 0}]
        });
        let bytes = Bytes::from(serde_json::to_vec(&body).unwrap());
        let result = inject_warning_label(&bytes, "CAUTION: ");
        assert!(result.is_some());
        let modified: serde_json::Value = serde_json::from_slice(&result.unwrap()).unwrap();
        let text = modified["choices"][0]["message"]["content"]
            .as_str()
            .unwrap();
        assert!(text.starts_with("CAUTION: "));
        assert!(text.contains("Act now!"));
    }

    #[test]
    fn test_inject_label_invalid_json() {
        let bytes = Bytes::from("not json");
        assert!(inject_warning_label(&bytes, "WARNING: ").is_none());
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

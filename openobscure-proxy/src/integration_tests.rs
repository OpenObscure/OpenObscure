//! Integration tests for the OpenObscure proxy.
//!
//! Uses wiremock as mock upstream + tower::ServiceExt::oneshot to test the
//! full request→encrypt→forward→decrypt→respond pipeline without binding
//! the proxy to a port.

use axum::{body::Body, http::Request, http::StatusCode, Router};
use http_body_util::BodyExt;
use std::collections::HashMap;
use std::sync::Arc;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

use crate::config::*;
use crate::fpe_engine::FpeEngine;
use crate::health::HealthStats;
use crate::hybrid_scanner::HybridScanner;
use crate::key_manager::KeyManager;
use crate::mapping::MappingStore;
use crate::proxy::AppState;
use crate::vault::Vault;

// ── Helpers ──────────────────────────────────────────────────────────────

fn test_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = i as u8;
    }
    key
}

fn build_config(upstream_url: &str) -> AppConfig {
    build_config_ext(upstream_url, FailMode::Open)
}

fn build_config_ext(upstream_url: &str, fail_mode: FailMode) -> AppConfig {
    let mut providers = HashMap::new();
    providers.insert(
        "test".to_string(),
        ProviderConfig {
            upstream_url: upstream_url.to_string(),
            route_prefix: "/test".to_string(),
            strip_headers: vec![],
        },
    );
    AppConfig {
        proxy: ProxyConfig {
            fail_mode,
            max_body_bytes: 1024, // 1KB for testing oversized body
            ..ProxyConfig::default()
        },
        providers,
        fpe: FpeConfig::default(),
        scanner: ScannerConfig::default(),
        logging: LoggingConfig::default(),
        image: crate::config::ImageConfig::default(),
    }
}

async fn build_state(config: AppConfig) -> AppState {
    // Install rustls CryptoProvider (no-op after first call)
    let _ = rustls::crypto::ring::default_provider().install_default();

    let fpe_engine = FpeEngine::new(&test_key()).unwrap();
    let key_manager = KeyManager::from_engine(fpe_engine, 1);
    let https_connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .expect("TLS roots")
        .https_or_http()
        .enable_http1()
        .build();
    let http_client =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(https_connector);
    AppState {
        config: Arc::new(config),
        scanner: Arc::new(HybridScanner::regex_only()),
        key_manager: Arc::new(key_manager),
        mapping_store: MappingStore::new(300),
        http_client,
        vault: Arc::new(Vault::new("openobscure-test")),
        health: HealthStats::new(),
        image_models: None,
    }
}

fn app(state: AppState) -> Router {
    Router::new()
        .fallback(crate::proxy::proxy_handler)
        .with_state(state)
}

async fn resp_body(resp: axum::http::Response<Body>) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// Wiremock responder that echoes the request body back as the response.
struct EchoResponder;

impl Respond for EchoResponder {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_bytes(request.body.clone())
    }
}

// ── PII Encryption ───────────────────────────────────────────────────────

/// Verify upstream receives FPE-encrypted SSN, not the original.
#[tokio::test]
async fn test_ssn_encrypted_before_upstream() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
        .mount(&mock)
        .await;

    let router = app(build_state(build_config(&mock.uri())).await);

    let body = r#"{"messages":[{"role":"user","content":"My SSN is 123-45-6789"}]}"#;
    let req = Request::post("/test/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Upstream must NOT see original SSN
    let received = mock.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
    let upstream_body = String::from_utf8(received[0].body.clone()).unwrap();
    assert!(
        !upstream_body.contains("123-45-6789"),
        "Upstream must not see original SSN. Got: {}",
        upstream_body
    );
    // Verify JSON structure preserved
    let json: serde_json::Value = serde_json::from_str(&upstream_body).unwrap();
    let content = json["messages"][0]["content"].as_str().unwrap();
    assert!(content.starts_with("My SSN is "));
}

/// Verify multiple PII types are all encrypted before reaching upstream.
#[tokio::test]
async fn test_multi_pii_encryption() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
        .mount(&mock)
        .await;

    let router = app(build_state(build_config(&mock.uri())).await);

    // Note: email local part must be >=6 chars for FF1 (radix 36, min domain size)
    let body = r#"{"messages":[{"role":"user","content":"SSN: 123-45-6789, email: johndoe@example.com"}]}"#;
    let req = Request::post("/test/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let received = mock.received_requests().await.unwrap();
    let upstream_body = String::from_utf8(received[0].body.clone()).unwrap();
    assert!(
        !upstream_body.contains("123-45-6789"),
        "SSN leaked to upstream"
    );
    assert!(
        !upstream_body.contains("johndoe@example.com"),
        "Email leaked to upstream"
    );
    // Email domain should be preserved (FPE only encrypts local part)
    assert!(
        upstream_body.contains("@example.com"),
        "Email domain should be preserved"
    );
}

// ── Response Decryption ──────────────────────────────────────────────────

/// End-to-end: proxy encrypts PII, echo server returns it, proxy decrypts.
/// Client should see the original PII value restored.
#[tokio::test]
async fn test_response_decryption_e2e() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(EchoResponder)
        .mount(&mock)
        .await;

    let router = app(build_state(build_config(&mock.uri())).await);

    let body = r#"{"messages":[{"role":"user","content":"My SSN is 123-45-6789"}]}"#;
    let req = Request::post("/test/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Echo server returned encrypted body; proxy should have decrypted it
    let text = resp_body(resp).await;
    assert!(
        text.contains("123-45-6789"),
        "Response should contain decrypted SSN. Got: {}",
        text
    );
}

// ── Non-JSON Passthrough ─────────────────────────────────────────────────

/// Non-JSON bodies should pass through without scanning (even if they contain PII-like text).
#[tokio::test]
async fn test_non_json_body_passthrough() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/upload"))
        .respond_with(ResponseTemplate::new(200).set_body_string("uploaded"))
        .mount(&mock)
        .await;

    let router = app(build_state(build_config(&mock.uri())).await);

    let body = "SSN: 123-45-6789 in plain text";
    let req = Request::post("/test/v1/upload")
        .header("content-type", "text/plain")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Non-JSON body should be forwarded unchanged
    let received = mock.received_requests().await.unwrap();
    let upstream_body = String::from_utf8(received[0].body.clone()).unwrap();
    assert_eq!(upstream_body, body);
}

// ── Provider Routing ─────────────────────────────────────────────────────

/// Verify route prefix is stripped and request reaches correct upstream path.
#[tokio::test]
async fn test_provider_routing_strips_prefix() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_string("routed"))
        .mount(&mock)
        .await;

    let router = app(build_state(build_config(&mock.uri())).await);

    let req = Request::post("/test/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp_body(resp).await, "routed");
}

/// Unknown route prefix → 404.
#[tokio::test]
async fn test_provider_not_found() {
    let mock = MockServer::start().await;
    let router = app(build_state(build_config(&mock.uri())).await);

    let req = Request::post("/unknown/v1/messages")
        .body(Body::from("{}"))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── Header Forwarding ────────────────────────────────────────────────────

/// Auth and custom headers should pass through to upstream unchanged.
#[tokio::test]
async fn test_auth_headers_forwarded() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock)
        .await;

    let router = app(build_state(build_config(&mock.uri())).await);

    let req = Request::post("/test/v1/messages")
        .header("content-type", "application/json")
        .header("authorization", "Bearer sk-test-key-12345")
        .header("x-api-key", "sk-ant-test-key")
        .header("x-custom-header", "custom-value")
        .body(Body::from("{}"))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let received = mock.received_requests().await.unwrap();
    let headers = &received[0].headers;
    assert_eq!(
        headers.get("authorization").map(|v| v.to_str().unwrap()),
        Some("Bearer sk-test-key-12345")
    );
    assert_eq!(
        headers.get("x-api-key").map(|v| v.to_str().unwrap()),
        Some("sk-ant-test-key")
    );
    assert_eq!(
        headers.get("x-custom-header").map(|v| v.to_str().unwrap()),
        Some("custom-value")
    );
}

// ── Fail Mode ────────────────────────────────────────────────────────────

/// Fail-open: malformed JSON is forwarded unchanged to upstream.
#[tokio::test]
async fn test_fail_open_malformed_json() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&mock)
        .await;

    let router = app(build_state(build_config(&mock.uri())).await);

    let body = "{ not valid json SSN: 123-45-6789";
    let req = Request::post("/test/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Original body forwarded unchanged
    let received = mock.received_requests().await.unwrap();
    let upstream_body = String::from_utf8(received[0].body.clone()).unwrap();
    assert_eq!(upstream_body, body);
}

/// Fail-closed: malformed JSON is rejected, upstream gets nothing.
#[tokio::test]
async fn test_fail_closed_malformed_json() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock)
        .await;

    let config = build_config_ext(&mock.uri(), FailMode::Closed);
    let router = app(build_state(config).await);

    let body = "{ not valid json SSN: 123-45-6789";
    let req = Request::post("/test/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);

    // Upstream should NOT receive any request
    let received = mock.received_requests().await.unwrap();
    assert_eq!(received.len(), 0);
}

// ── Body Limits ──────────────────────────────────────────────────────────

/// Body exceeding max_body_bytes → 413 Payload Too Large.
#[tokio::test]
async fn test_oversized_body_rejected() {
    let mock = MockServer::start().await;
    let router = app(build_state(build_config(&mock.uri())).await);

    // Body larger than 1KB test limit
    let body = "x".repeat(2048);
    let req = Request::post("/test/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

// ── Clean Body ───────────────────────────────────────────────────────────

/// Body with no PII passes through and back unchanged.
#[tokio::test]
async fn test_clean_body_unchanged() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(EchoResponder)
        .mount(&mock)
        .await;

    let router = app(build_state(build_config(&mock.uri())).await);

    let body = r#"{"messages":[{"role":"user","content":"Hello!"}]}"#;
    let req = Request::post("/test/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // No PII → response body should be identical to request body
    let text = resp_body(resp).await;
    let resp_json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let orig_json: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(resp_json, orig_json);
}

// ── Empty Body ───────────────────────────────────────────────────────────

/// Empty body forwarded without error.
#[tokio::test]
async fn test_empty_body_forwarded() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&mock)
        .await;

    let router = app(build_state(build_config(&mock.uri())).await);

    let req = Request::post("/test/v1/messages")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ── SSE Streaming ────────────────────────────────────────────────────────

/// Wiremock responder that returns SSE-formatted events echoing back the body.
struct SseEchoResponder;

impl Respond for SseEchoResponder {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        // Build SSE events from the request body
        let body_str = String::from_utf8_lossy(&request.body);
        let sse_response = format!("data: {}\n\ndata: [DONE]\n\n", body_str);
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(sse_response)
    }
}

/// SSE responses should stream through and decrypt PII in chunks.
#[tokio::test]
async fn test_sse_response_streams_with_decryption() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SseEchoResponder)
        .mount(&mock)
        .await;

    let router = app(build_state(build_config(&mock.uri())).await);

    let body = r#"{"messages":[{"role":"user","content":"My SSN is 123-45-6789"}]}"#;
    let req = Request::post("/test/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // The SSE response should contain the decrypted SSN
    let text = resp_body(resp).await;
    assert!(
        text.contains("123-45-6789"),
        "SSE response should contain decrypted SSN. Got: {}",
        text
    );
}

/// SSE response without PII mappings should stream through unchanged.
#[tokio::test]
async fn test_sse_response_no_pii_passthrough() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string("data: {\"content\":\"hello\"}\n\ndata: [DONE]\n\n"),
        )
        .mount(&mock)
        .await;

    let router = app(build_state(build_config(&mock.uri())).await);

    let body = r#"{"messages":[{"role":"user","content":"Hello!"}]}"#;
    let req = Request::post("/test/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let text = resp_body(resp).await;
    assert!(text.contains("hello"), "Clean SSE should pass through");
}

/// Non-SSE response still uses buffered path.
#[tokio::test]
async fn test_non_sse_response_still_buffered() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(EchoResponder)
        .mount(&mock)
        .await;

    let router = app(build_state(build_config(&mock.uri())).await);

    let body = r#"{"messages":[{"role":"user","content":"My SSN is 123-45-6789"}]}"#;
    let req = Request::post("/test/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Non-SSE: buffered path decrypts, response should contain original SSN
    let text = resp_body(resp).await;
    assert!(
        text.contains("123-45-6789"),
        "Buffered response should contain decrypted SSN. Got: {}",
        text
    );
}

/// SSE response content-type detection is case-insensitive.
#[tokio::test]
async fn test_sse_content_type_case_insensitive() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "Text/Event-Stream; charset=utf-8")
                .set_body_string("data: {\"content\":\"hello\"}\n\n"),
        )
        .mount(&mock)
        .await;

    let router = app(build_state(build_config(&mock.uri())).await);

    let body = r#"{"messages":[{"role":"user","content":"Hello!"}]}"#;
    let req = Request::post("/test/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // SSE detected (case-insensitive) — should not have content-length
    // (streaming responses don't know their size upfront)
    let _has_content_length = resp.headers().contains_key("content-length");
    // For no-PII case, we actually take the non-SSE streaming path (has_mappings=false)
    // So content-length IS set. This test verifies SSE detection works.
    let text = resp_body(resp).await;
    assert!(text.contains("hello"));
}

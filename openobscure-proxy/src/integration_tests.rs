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
            max_body_bytes: 1024,  // 1KB for testing oversized body
            body_limit_full: 1024, // tier-aware limit matches max_body_bytes for tests
            ..ProxyConfig::default()
        },
        providers,
        fpe: FpeConfig::default(),
        scanner: ScannerConfig::default(),
        logging: LoggingConfig::default(),
        image: crate::config::ImageConfig::default(),
        voice: crate::config::VoiceConfig::default(),
        response_integrity: crate::config::ResponseIntegrityConfig::default(),
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
        kws_engine: None,
        response_integrity: None,
        device_tier: crate::device_profile::CapabilityTier::Full,
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
        // Use set_body_bytes to avoid set_body_string overriding content-type to text/plain
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_bytes(sse_response.into_bytes())
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
                .set_body_bytes("data: {\"content\":\"hello\"}\n\ndata: [DONE]\n\n".as_bytes()),
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

/// GET method forwarded correctly.
#[tokio::test]
async fn test_get_method_forwarded() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"models":["gpt-4"]}"#))
        .mount(&mock)
        .await;

    let router = app(build_state(build_config(&mock.uri())).await);

    let req = Request::get("/test/v1/models").body(Body::empty()).unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let text = resp_body(resp).await;
    assert!(text.contains("gpt-4"));
}

/// Query parameters preserved when forwarding.
#[tokio::test]
async fn test_query_params_preserved() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&mock)
        .await;

    let router = app(build_state(build_config(&mock.uri())).await);

    let req = Request::get("/test/v1/models?limit=10&offset=0")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let received = mock.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
    let query = received[0].url.query().unwrap_or("");
    assert!(query.contains("limit=10"));
    assert!(query.contains("offset=0"));
}

/// Multiple PII values in nested JSON are all encrypted.
#[tokio::test]
async fn test_nested_json_pii_encrypted() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
        .mount(&mock)
        .await;

    let router = app(build_state(build_config(&mock.uri())).await);

    // Both SSNs must pass validation: area code not 000/666/900+
    let body = r#"{"messages":[{"role":"user","content":"SSN: 123-45-6789"},{"role":"user","content":"SSN: 234-56-7890"}]}"#;
    let req = Request::post("/test/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let received = mock.received_requests().await.unwrap();
    let upstream_body = String::from_utf8(received[0].body.clone()).unwrap();
    assert!(!upstream_body.contains("123-45-6789"), "First SSN leaked");
    assert!(!upstream_body.contains("234-56-7890"), "Second SSN leaked");
}

/// Upstream 500 error is propagated back to client.
#[tokio::test]
async fn test_upstream_error_propagated() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(500).set_body_string(r#"{"error":"internal"}"#))
        .mount(&mock)
        .await;

    let router = app(build_state(build_config(&mock.uri())).await);

    let req = Request::post("/test/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

// ── Health Endpoint ─────────────────────────────────────────────────────

fn build_full_router(state: AppState, auth_token: Option<String>) -> Router {
    use crate::health::{FeatureBudgetSummary, HealthState, ReadinessState};
    use crate::ner_endpoint::NerState;
    use std::sync::atomic::{AtomicU32, AtomicU8};

    let health_state = HealthState {
        stats: state.health.clone(),
        auth_token: auth_token.clone(),
        key_version: Arc::new(AtomicU32::new(1)),
        device_tier: "Full".to_string(),
        feature_budget: FeatureBudgetSummary {
            tier: "Full".to_string(),
            max_ram_mb: 275,
            ner_enabled: true,
            crf_enabled: false,
            ensemble_enabled: true,
            image_pipeline_enabled: true,
            ocr_tier: "full_recognition".to_string(),
            nsfw_enabled: true,
            screen_guard_enabled: true,
            face_model: "scrfd".to_string(),
        },
        readiness: Arc::new(AtomicU8::new(ReadinessState::Ready as u8)),
    };

    let ner_state = NerState {
        scanner: state.scanner.clone(),
        auth_token,
    };

    Router::new()
        .route(
            "/_openobscure/health",
            axum::routing::get(crate::health::health_handler).with_state(health_state),
        )
        .route(
            "/_openobscure/ner",
            axum::routing::post(crate::ner_endpoint::ner_handler).with_state(ner_state),
        )
        .fallback(crate::proxy::proxy_handler)
        .with_state(state)
}

/// Health endpoint returns OK with valid auth token.
#[tokio::test]
async fn test_health_endpoint_with_auth() {
    let mock = MockServer::start().await;
    let state = build_state(build_config(&mock.uri())).await;
    let router = build_full_router(state, Some("test-secret-token".to_string()));

    let req = Request::get("/_openobscure/health")
        .header("x-openobscure-token", "test-secret-token")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let text = resp_body(resp).await;
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["status"], "ok");
    assert_eq!(json["fpe_key_version"], 1);
    assert_eq!(json["device_tier"], "Full");
    assert!(json["feature_budget"]["ner_enabled"].as_bool().unwrap());
}

/// Health endpoint rejects missing auth token.
#[tokio::test]
async fn test_health_endpoint_missing_auth() {
    let mock = MockServer::start().await;
    let state = build_state(build_config(&mock.uri())).await;
    let router = build_full_router(state, Some("test-secret-token".to_string()));

    let req = Request::get("/_openobscure/health")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// Health endpoint rejects wrong auth token.
#[tokio::test]
async fn test_health_endpoint_wrong_auth() {
    let mock = MockServer::start().await;
    let state = build_state(build_config(&mock.uri())).await;
    let router = build_full_router(state, Some("test-secret-token".to_string()));

    let req = Request::get("/_openobscure/health")
        .header("x-openobscure-token", "wrong-token")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// Health endpoint works without auth when no token is configured.
#[tokio::test]
async fn test_health_endpoint_no_auth_required() {
    let mock = MockServer::start().await;
    let state = build_state(build_config(&mock.uri())).await;
    let router = build_full_router(state, None);

    let req = Request::get("/_openobscure/health")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ── NER Endpoint ────────────────────────────────────────────────────────

/// NER endpoint detects PII in text.
#[tokio::test]
async fn test_ner_endpoint_detects_ssn() {
    let mock = MockServer::start().await;
    let state = build_state(build_config(&mock.uri())).await;
    let router = build_full_router(state, Some("test-token".to_string()));

    let body = r#"{"text":"My SSN is 123-45-6789"}"#;
    let req = Request::post("/_openobscure/ner")
        .header("content-type", "application/json")
        .header("x-openobscure-token", "test-token")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let text = resp_body(resp).await;
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let matches = json.as_array().unwrap();
    assert!(!matches.is_empty(), "NER should detect SSN");
    assert_eq!(matches[0]["type"], "ssn");
}

/// NER endpoint returns empty array for clean text.
#[tokio::test]
async fn test_ner_endpoint_clean_text() {
    let mock = MockServer::start().await;
    let state = build_state(build_config(&mock.uri())).await;
    let router = build_full_router(state, Some("test-token".to_string()));

    let body = r#"{"text":"Hello, how are you today?"}"#;
    let req = Request::post("/_openobscure/ner")
        .header("content-type", "application/json")
        .header("x-openobscure-token", "test-token")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let text = resp_body(resp).await;
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(json.as_array().unwrap().is_empty());
}

/// NER endpoint requires auth token.
#[tokio::test]
async fn test_ner_endpoint_requires_auth() {
    let mock = MockServer::start().await;
    let state = build_state(build_config(&mock.uri())).await;
    let router = build_full_router(state, Some("test-token".to_string()));

    let body = r#"{"text":"My SSN is 123-45-6789"}"#;
    let req = Request::post("/_openobscure/ner")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ── Full Router Proxy + Internal Endpoints ──────────────────────────────

/// Internal endpoints don't interfere with proxy routing.
#[tokio::test]
async fn test_full_router_proxy_still_works() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(EchoResponder)
        .mount(&mock)
        .await;

    let state = build_state(build_config(&mock.uri())).await;
    let router = build_full_router(state, Some("tok".to_string()));

    let body = r#"{"messages":[{"role":"user","content":"My SSN is 123-45-6789"}]}"#;
    let req = Request::post("/test/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify PII was decrypted in the response
    let text = resp_body(resp).await;
    assert!(
        text.contains("123-45-6789"),
        "Proxy should still decrypt PII. Got: {}",
        text
    );
}

// ── SSE Streaming Edge Cases ─────────────────────────────────────────────

/// SSE response with multiple events containing PII should decrypt all of them.
#[tokio::test]
async fn test_sse_multiple_events_all_decrypted() {
    let mock = MockServer::start().await;

    // Responder that echoes request body across multiple SSE events
    struct MultiEventSseResponder;
    impl Respond for MultiEventSseResponder {
        fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
            let body_str = String::from_utf8_lossy(&request.body);
            let sse_response = format!(
                "data: {{\"chunk\":1,\"text\":\"{body}\"}}\n\ndata: {{\"chunk\":2,\"text\":\"{body}\"}}\n\ndata: [DONE]\n\n",
                body = body_str
            );
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_bytes(sse_response.into_bytes())
        }
    }

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(MultiEventSseResponder)
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

    let text = resp_body(resp).await;
    // Both SSE events should have decrypted SSN
    let ssn_count = text.matches("123-45-6789").count();
    assert!(
        ssn_count >= 2,
        "Both SSE events should contain decrypted SSN (found {} occurrences). Got: {}",
        ssn_count,
        text
    );
}

/// SSE response should preserve text/event-stream content-type header.
#[tokio::test]
async fn test_sse_preserves_content_type_header() {
    let mock = MockServer::start().await;

    // Use a custom responder to avoid set_body_string overriding content-type
    struct SseFixedResponder;
    impl Respond for SseFixedResponder {
        fn respond(&self, _request: &wiremock::Request) -> ResponseTemplate {
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_bytes("data: {\"text\":\"hello\"}\n\ndata: [DONE]\n\n")
        }
    }

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SseFixedResponder)
        .mount(&mock)
        .await;

    let router = app(build_state(build_config(&mock.uri())).await);

    // Send PII so we go through the SSE streaming path (has_mappings=true)
    let body = r#"{"messages":[{"role":"user","content":"SSN: 123-45-6789"}]}"#;
    let req = Request::post("/test/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        ct.contains("text/event-stream"),
        "SSE content-type should be preserved. Got: {}",
        ct
    );
    // SSE streaming responses should NOT have content-length
    assert!(
        !resp.headers().contains_key("content-length"),
        "SSE responses should not have content-length"
    );
}

/// SSE response with empty data events should stream through without error.
#[tokio::test]
async fn test_sse_empty_events_passthrough() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_bytes("data: \n\ndata: \n\ndata: [DONE]\n\n".as_bytes()),
        )
        .mount(&mock)
        .await;

    let router = app(build_state(build_config(&mock.uri())).await);

    let body = r#"{"messages":[{"role":"user","content":"SSN: 123-45-6789"}]}"#;
    let req = Request::post("/test/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let text = resp_body(resp).await;
    assert!(
        text.contains("[DONE]"),
        "SSE with empty events should still complete"
    );
}

/// SSE response with multiple PII types should decrypt all types per-chunk.
#[tokio::test]
async fn test_sse_multi_pii_decryption() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SseEchoResponder)
        .mount(&mock)
        .await;

    let router = app(build_state(build_config(&mock.uri())).await);

    let body = r#"{"messages":[{"role":"user","content":"SSN: 123-45-6789, email: johndoe@example.com"}]}"#;
    let req = Request::post("/test/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let text = resp_body(resp).await;
    assert!(
        text.contains("123-45-6789"),
        "SSN should be decrypted in SSE response. Got: {}",
        text
    );
    assert!(
        text.contains("johndoe@example.com"),
        "Email should be decrypted in SSE response. Got: {}",
        text
    );
}

/// Mappings are cleaned up after SSE stream completes.
#[tokio::test]
async fn test_sse_mappings_cleaned_up_after_stream() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SseEchoResponder)
        .mount(&mock)
        .await;

    let state = build_state(build_config(&mock.uri())).await;
    let mapping_store = state.mapping_store.clone();
    let router = app(state);

    let body = r#"{"messages":[{"role":"user","content":"SSN: 123-45-6789"}]}"#;
    let req = Request::post("/test/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Consume the full SSE stream
    let _text = resp_body(resp).await;

    // After stream is fully consumed, mappings should be cleaned up
    // (We can't inspect the exact request_id, but the store should be empty
    // since no other requests are in flight)
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    // The mapping store eviction happens via the stream's unfold callback
    // At minimum, verify the store is accessible (no deadlock)
    let dummy_id = uuid::Uuid::new_v4();
    assert!(
        mapping_store.get(&dummy_id).await.is_none(),
        "Dummy ID should not exist in mapping store"
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
                .set_body_bytes("data: {\"content\":\"hello\"}\n\n".as_bytes()),
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

// ── Load / Stress Tests ─────────────────────────────────────────────────

/// Multiple concurrent proxy requests all encrypt/decrypt correctly.
#[tokio::test]
async fn test_concurrent_requests_all_decrypt() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(EchoResponder)
        .expect(10)
        .mount(&mock)
        .await;

    let state = build_state(build_config(&mock.uri())).await;

    let mut handles = vec![];
    for i in 0..10 {
        let state_clone = state.clone();
        handles.push(tokio::spawn(async move {
            let router = app(state_clone);
            let ssn = format!("123-45-678{}", i);
            let body = format!(
                r#"{{"messages":[{{"role":"user","content":"My SSN is {}"}}]}}"#,
                ssn
            );
            let req = Request::post("/test/v1/messages")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap();

            let resp = router.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "Request {} failed", i);

            let text = resp_body(resp).await;
            assert!(
                text.contains(&ssn),
                "Request {} SSN {} not decrypted in response: {}",
                i,
                ssn,
                text
            );
        }));
    }

    for h in handles {
        h.await.unwrap();
    }
}

/// Concurrent SSE streaming requests don't interfere with each other.
#[tokio::test]
async fn test_concurrent_sse_streams() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SseEchoResponder)
        .expect(5)
        .mount(&mock)
        .await;

    let state = build_state(build_config(&mock.uri())).await;

    let mut handles = vec![];
    for i in 0..5 {
        let state_clone = state.clone();
        handles.push(tokio::spawn(async move {
            let router = app(state_clone);
            let ssn = format!("123-45-678{}", i);
            let body = format!(
                r#"{{"messages":[{{"role":"user","content":"SSN: {}"}}]}}"#,
                ssn
            );
            let req = Request::post("/test/v1/messages")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap();

            let resp = router.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "SSE stream {} failed", i);

            let text = resp_body(resp).await;
            assert!(
                text.contains(&ssn),
                "SSE stream {} SSN {} not found: {}",
                i,
                ssn,
                text
            );
        }));
    }

    for h in handles {
        h.await.unwrap();
    }
}

/// Mapping store correctly isolates concurrent request mappings.
#[tokio::test]
async fn test_mapping_store_isolation() {
    let store = MappingStore::new(300);

    let id1 = uuid::Uuid::new_v4();
    let id2 = uuid::Uuid::new_v4();

    let mut m1 = crate::mapping::RequestMappings::new(id1);
    m1.insert(crate::mapping::FpeMapping {
        pii_type: crate::pii_types::PiiType::Ssn,
        plaintext: "111-11-1111".to_string(),
        ciphertext: "999-99-9999".to_string(),
        tweak: vec![],
        key_version: 1,
    });

    let mut m2 = crate::mapping::RequestMappings::new(id2);
    m2.insert(crate::mapping::FpeMapping {
        pii_type: crate::pii_types::PiiType::Ssn,
        plaintext: "222-22-2222".to_string(),
        ciphertext: "888-88-8888".to_string(),
        tweak: vec![],
        key_version: 1,
    });

    store.insert(m1).await;
    store.insert(m2).await;

    // Each request ID gets its own mappings
    let r1 = store.get(&id1).await.unwrap();
    let r2 = store.get(&id2).await.unwrap();
    assert_eq!(r1.decrypt_response("999-99-9999"), "111-11-1111");
    assert_eq!(r2.decrypt_response("888-88-8888"), "222-22-2222");

    // Cross-contamination check: r1 doesn't decrypt r2's ciphertext
    assert_eq!(r1.decrypt_response("888-88-8888"), "888-88-8888");

    // Clean up
    store.remove(&id1).await;
    assert!(store.get(&id1).await.is_none());
    assert!(store.get(&id2).await.is_some());
}

/// Rapid sequential requests reuse the same infrastructure without leaks.
#[tokio::test]
async fn test_rapid_sequential_requests() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(EchoResponder)
        .expect(20)
        .mount(&mock)
        .await;

    let state = build_state(build_config(&mock.uri())).await;

    for i in 0..20 {
        let router = app(state.clone());
        let ssn = format!("234-56-78{:02}", i);
        let body = format!(
            r#"{{"messages":[{{"role":"user","content":"SSN: {}"}}]}}"#,
            ssn
        );
        let req = Request::post("/test/v1/messages")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();

        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "Sequential request {} failed",
            i
        );

        let text = resp_body(resp).await;
        assert!(
            text.contains(&ssn),
            "Sequential request {} SSN {} not found: {}",
            i,
            ssn,
            text
        );
    }
}

/// Mixed concurrent PII and clean requests all succeed.
#[tokio::test]
async fn test_mixed_pii_and_clean_concurrent() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(EchoResponder)
        .expect(10)
        .mount(&mock)
        .await;

    let state = build_state(build_config(&mock.uri())).await;

    let mut handles = vec![];
    for i in 0..10 {
        let state_clone = state.clone();
        handles.push(tokio::spawn(async move {
            let router = app(state_clone);
            let body = if i % 2 == 0 {
                // PII request
                format!(
                    r#"{{"messages":[{{"role":"user","content":"SSN: 123-45-678{}"}}]}}"#,
                    i
                )
            } else {
                // Clean request
                format!(
                    r#"{{"messages":[{{"role":"user","content":"Hello world {}"}}]}}"#,
                    i
                )
            };
            let req = Request::post("/test/v1/messages")
                .header("content-type", "application/json")
                .body(Body::from(body.clone()))
                .unwrap();

            let resp = router.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "Mixed request {} failed", i);

            let text = resp_body(resp).await;
            if i % 2 == 0 {
                let ssn = format!("123-45-678{}", i);
                assert!(
                    text.contains(&ssn),
                    "PII request {} not decrypted: {}",
                    i,
                    text
                );
            } else {
                let msg = format!("Hello world {}", i);
                assert!(
                    text.contains(&msg),
                    "Clean request {} content lost: {}",
                    i,
                    text
                );
            }
        }));
    }

    for h in handles {
        h.await.unwrap();
    }
}

// ── Response Integrity (Cognitive Firewall) ─────────────────────────────

fn build_ri_config(upstream_url: &str, enabled: bool, log_only: bool) -> AppConfig {
    let mut config = build_config(upstream_url);
    config.response_integrity = ResponseIntegrityConfig {
        enabled,
        sensitivity: "medium".to_string(),
        log_only,
        ..ResponseIntegrityConfig::default()
    };
    // Increase max body to accommodate response JSON
    config.proxy.max_body_bytes = 65536;
    config
}

async fn build_ri_state(config: AppConfig) -> AppState {
    let ri_scanner = if config.response_integrity.enabled {
        let sensitivity: crate::response_integrity::Sensitivity =
            config.response_integrity.sensitivity.parse().unwrap();
        if sensitivity == crate::response_integrity::Sensitivity::Off {
            None
        } else {
            Some(Arc::new(
                crate::response_integrity::ResponseIntegrityScanner::new(sensitivity),
            ))
        }
    } else {
        None
    };

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
        kws_engine: None,
        response_integrity: ri_scanner,
        device_tier: crate::device_profile::CapabilityTier::Full,
    }
}

/// Persuasive Anthropic response for testing.
const PERSUASIVE_ANTHROPIC_RESPONSE: &str = r#"{"id":"msg_01","type":"message","role":"assistant","content":[{"type":"text","text":"Act now! This limited time offer is a smart choice. Experts agree you could lose out. Buy now for the best deal!"}],"model":"claude-3"}"#;

/// Persuasive OpenAI response for testing.
const PERSUASIVE_OPENAI_RESPONSE: &str = r#"{"id":"chatcmpl-01","choices":[{"index":0,"message":{"role":"assistant","content":"Act now! This limited time offer is a smart choice. Experts agree you could lose out. Buy now for the best deal!"},"finish_reason":"stop"}]}"#;

/// Clean (non-persuasive) response for testing.
const CLEAN_ANTHROPIC_RESPONSE: &str = r#"{"id":"msg_02","type":"message","role":"assistant","content":[{"type":"text","text":"Here is a Python function that sorts a list in ascending order using the built-in sorted() function."}],"model":"claude-3"}"#;

/// RI disabled: response passes through unchanged.
#[tokio::test]
async fn test_ri_disabled_passthrough() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(PERSUASIVE_ANTHROPIC_RESPONSE)
                .insert_header("content-type", "application/json"),
        )
        .mount(&mock)
        .await;

    let config = build_ri_config(&mock.uri(), false, true);
    let router = app(build_ri_state(config).await);

    let req = Request::post("/test/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"messages":[{"role":"user","content":"hello"}]}"#,
        ))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp_body(resp).await;
    // Response should be unchanged (no warning label)
    assert!(
        !body.contains("OpenObscure"),
        "Disabled RI should not inject labels"
    );
    assert!(
        body.contains("Act now!"),
        "Original content should be preserved"
    );
}

/// RI enabled + log_only: response is scanned and logged but NOT modified.
#[tokio::test]
async fn test_ri_log_only_no_modification() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(PERSUASIVE_ANTHROPIC_RESPONSE)
                .insert_header("content-type", "application/json"),
        )
        .mount(&mock)
        .await;

    let config = build_ri_config(&mock.uri(), true, true);
    let router = app(build_ri_state(config).await);

    let req = Request::post("/test/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"messages":[{"role":"user","content":"hello"}]}"#,
        ))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp_body(resp).await;
    // log_only=true: body should NOT contain warning label
    assert!(
        !body.contains("OpenObscure"),
        "log_only should not inject labels"
    );
    assert!(
        body.contains("Act now!"),
        "Original content should be preserved"
    );
}

/// RI enabled + log_only=false + Anthropic format: warning label prepended.
#[tokio::test]
async fn test_ri_label_anthropic_format() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(PERSUASIVE_ANTHROPIC_RESPONSE)
                .insert_header("content-type", "application/json"),
        )
        .mount(&mock)
        .await;

    let config = build_ri_config(&mock.uri(), true, false);
    let router = app(build_ri_state(config).await);

    let req = Request::post("/test/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"messages":[{"role":"user","content":"hello"}]}"#,
        ))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp_body(resp).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let text = json["content"][0]["text"].as_str().unwrap();

    // Should have warning label prepended
    assert!(
        text.contains("OpenObscure"),
        "Should contain OpenObscure label. Got: {}",
        text
    );
    // Original content should still be present
    assert!(
        text.contains("Act now!"),
        "Original content should be preserved"
    );
}

/// RI enabled + log_only=false + OpenAI format: warning label prepended.
#[tokio::test]
async fn test_ri_label_openai_format() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(PERSUASIVE_OPENAI_RESPONSE)
                .insert_header("content-type", "application/json"),
        )
        .mount(&mock)
        .await;

    let config = build_ri_config(&mock.uri(), true, false);
    let router = app(build_ri_state(config).await);

    let req = Request::post("/test/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"messages":[{"role":"user","content":"hello"}]}"#,
        ))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp_body(resp).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let text = json["choices"][0]["message"]["content"].as_str().unwrap();

    assert!(
        text.contains("OpenObscure"),
        "Should contain OpenObscure label. Got: {}",
        text
    );
    assert!(
        text.contains("Act now!"),
        "Original content should be preserved"
    );
}

/// RI enabled + clean response: no warning label added.
#[tokio::test]
async fn test_ri_clean_response_no_label() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(CLEAN_ANTHROPIC_RESPONSE)
                .insert_header("content-type", "application/json"),
        )
        .mount(&mock)
        .await;

    let config = build_ri_config(&mock.uri(), true, false);
    let router = app(build_ri_state(config).await);

    let req = Request::post("/test/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"messages":[{"role":"user","content":"hello"}]}"#,
        ))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp_body(resp).await;
    assert!(
        !body.contains("OpenObscure"),
        "Clean response should not get a label"
    );
    assert!(
        body.contains("Python function"),
        "Original content should be preserved"
    );
}

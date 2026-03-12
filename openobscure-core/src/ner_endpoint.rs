//! POST /_openobscure/ner — semantic PII scanning endpoint for L1 plugin.
//!
//! Accepts `{"text": "..."}` and returns detected PII spans with type and
//! confidence. Protected by the same auth token as the health endpoint.
//!
//! The L1 plugin calls this endpoint synchronously to augment its regex-only
//! redaction with NER/CRF semantic scanning from L0.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Json;
use serde::{Deserialize, Serialize};

use crate::hybrid_scanner::HybridScanner;

/// Shared state for the NER endpoint.
#[derive(Clone)]
pub struct NerState {
    pub scanner: Arc<HybridScanner>,
    pub auth_token: Option<String>,
}

/// Request body for NER scanning.
#[derive(Deserialize)]
pub struct NerRequest {
    pub text: String,
}

/// A single PII span detected by the scanner.
#[derive(Serialize, Deserialize)]
pub struct NerMatch {
    pub start: usize,
    pub end: usize,
    #[serde(rename = "type")]
    pub pii_type: String,
    pub confidence: f32,
}

/// POST /_openobscure/ner — scan text for PII using the hybrid scanner.
pub async fn ner_handler(
    State(state): State<NerState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Vec<NerMatch>>, StatusCode> {
    // Validate auth token if configured (same as health endpoint)
    if let Some(ref expected) = state.auth_token {
        let provided = headers
            .get("x-openobscure-token")
            .and_then(|v| v.to_str().ok());
        match provided {
            Some(token) if token == expected => {}
            _ => return Err(StatusCode::UNAUTHORIZED),
        }
    }

    // Parse JSON body
    let request: NerRequest = serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;

    // Limit input size to prevent abuse (64KB should be plenty for tool results)
    if request.text.len() > 65_536 {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }

    // Scan text through the hybrid scanner (regex + keyword + NER/CRF)
    let matches = state.scanner.scan_text(&request.text);

    // Convert to response format
    let result: Vec<NerMatch> = matches
        .into_iter()
        .map(|m| NerMatch {
            start: m.start,
            end: m.end,
            pii_type: m.pii_type.to_string(),
            confidence: m.confidence,
        })
        .collect();

    Ok(Json(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request, Router};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn ner_app(auth_token: Option<String>) -> Router {
        let scanner = Arc::new(HybridScanner::regex_only());
        let state = NerState {
            scanner,
            auth_token,
        };
        Router::new().route(
            "/_openobscure/ner",
            axum::routing::post(ner_handler).with_state(state),
        )
    }

    #[tokio::test]
    async fn test_ner_detects_email() {
        let app = ner_app(None);
        let body = serde_json::json!({"text": "Contact john@example.com for details"});
        let req = Request::post("/_openobscure/ner")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let matches: Vec<NerMatch> = serde_json::from_slice(&body).unwrap();
        assert!(!matches.is_empty());
        assert!(matches.iter().any(|m| m.pii_type == "email"));
    }

    #[tokio::test]
    async fn test_ner_detects_ssn() {
        let app = ner_app(None);
        let body = serde_json::json!({"text": "SSN: 123-45-6789"});
        let req = Request::post("/_openobscure/ner")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let matches: Vec<NerMatch> = serde_json::from_slice(&body).unwrap();
        assert!(matches.iter().any(|m| m.pii_type == "ssn"));
    }

    #[tokio::test]
    async fn test_ner_empty_text() {
        let app = ner_app(None);
        let body = serde_json::json!({"text": ""});
        let req = Request::post("/_openobscure/ner")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let matches: Vec<NerMatch> = serde_json::from_slice(&body).unwrap();
        assert!(matches.is_empty());
    }

    #[tokio::test]
    async fn test_ner_no_pii() {
        let app = ner_app(None);
        let body = serde_json::json!({"text": "The weather is nice today."});
        let req = Request::post("/_openobscure/ner")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let matches: Vec<NerMatch> = serde_json::from_slice(&body).unwrap();
        assert!(matches.is_empty());
    }

    #[tokio::test]
    async fn test_ner_auth_required() {
        let app = ner_app(Some("secret-token".to_string()));
        let body = serde_json::json!({"text": "test"});
        let req = Request::post("/_openobscure/ner")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_ner_auth_valid() {
        let app = ner_app(Some("secret-token".to_string()));
        let body = serde_json::json!({"text": "Email: a@b.com"});
        let req = Request::post("/_openobscure/ner")
            .header("content-type", "application/json")
            .header("x-openobscure-token", "secret-token")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_ner_auth_wrong_token() {
        let app = ner_app(Some("secret-token".to_string()));
        let body = serde_json::json!({"text": "test"});
        let req = Request::post("/_openobscure/ner")
            .header("content-type", "application/json")
            .header("x-openobscure-token", "wrong-token")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_ner_invalid_json() {
        let app = ner_app(None);
        let req = Request::post("/_openobscure/ner")
            .header("content-type", "application/json")
            .body(Body::from("not json"))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_ner_multiple_pii() {
        let app = ner_app(None);
        let body = serde_json::json!({
            "text": "SSN: 123-45-6789, email: test@example.com, phone: (555) 123-4567"
        });
        let req = Request::post("/_openobscure/ner")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let matches: Vec<NerMatch> = serde_json::from_slice(&body).unwrap();
        assert!(matches.len() >= 3);
    }

    #[tokio::test]
    async fn test_ner_match_offsets_valid() {
        let app = ner_app(None);
        let text = "My email is bob@test.org";
        let body = serde_json::json!({"text": text});
        let req = Request::post("/_openobscure/ner")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let matches: Vec<NerMatch> = serde_json::from_slice(&body).unwrap();

        for m in &matches {
            assert!(m.start < m.end, "start must be less than end");
            assert!(m.end <= text.len(), "end must be within text bounds");
            assert!(
                m.confidence > 0.0 && m.confidence <= 1.0,
                "confidence in (0,1]"
            );
        }
    }

    #[tokio::test]
    async fn test_ner_payload_too_large() {
        let app = ner_app(None);
        // Build a text just over 64KB
        let big_text = "a".repeat(65_537);
        let body = serde_json::json!({"text": big_text});
        let req = Request::post("/_openobscure/ner")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn test_ner_exactly_64kb_allowed() {
        let app = ner_app(None);
        let text = "a".repeat(65_536);
        let body = serde_json::json!({"text": text});
        let req = Request::post("/_openobscure/ner")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_ner_missing_text_field() {
        let app = ner_app(None);
        let body = serde_json::json!({"data": "not the right field"});
        let req = Request::post("/_openobscure/ner")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_ner_no_auth_when_not_configured() {
        let app = ner_app(None);
        let body = serde_json::json!({"text": "SSN: 123-45-6789"});
        let req = Request::post("/_openobscure/ner")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let matches: Vec<NerMatch> = serde_json::from_slice(&body).unwrap();
        assert!(matches.iter().any(|m| m.pii_type == "ssn"));
    }
}

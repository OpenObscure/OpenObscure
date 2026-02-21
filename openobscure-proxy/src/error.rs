use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("Configuration error: {0}")]
    Config(#[from] anyhow::Error),

    #[error("FPE error: {0}")]
    Fpe(#[from] crate::fpe_engine::FpeError),

    #[error("Vault error: {0}")]
    Vault(#[from] crate::vault::VaultError),

    #[error("Body processing error: {0}")]
    Body(String),

    #[error("Upstream error: {0}")]
    Upstream(String),

    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
}

impl IntoResponse for ProxyError {
    fn into_response(self) -> Response {
        oo_error!(crate::oo_log::modules::PROXY, "Proxy error", error = %self);

        let status = match &self {
            ProxyError::Config(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ProxyError::Fpe(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ProxyError::Vault(_) => StatusCode::SERVICE_UNAVAILABLE,
            ProxyError::Body(_) => StatusCode::BAD_REQUEST,
            ProxyError::Upstream(_) => StatusCode::BAD_GATEWAY,
            ProxyError::Json(_) => StatusCode::BAD_REQUEST,
        };

        (status, "OpenObscure proxy error").into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    // --- Display / Error messages ---

    #[test]
    fn test_config_error_display() {
        let err = ProxyError::Config(anyhow::anyhow!("missing field 'port'"));
        assert_eq!(err.to_string(), "Configuration error: missing field 'port'");
    }

    #[test]
    fn test_body_error_display() {
        let err = ProxyError::Body("invalid UTF-8".to_string());
        assert_eq!(err.to_string(), "Body processing error: invalid UTF-8");
    }

    #[test]
    fn test_upstream_error_display() {
        let err = ProxyError::Upstream("connection refused".to_string());
        assert_eq!(err.to_string(), "Upstream error: connection refused");
    }

    #[test]
    fn test_json_error_display() {
        let json_err = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        let err = ProxyError::Json(json_err);
        assert!(err.to_string().starts_with("JSON parse error:"));
    }

    #[test]
    fn test_vault_error_display() {
        let vault_err = crate::vault::VaultError::InvalidKeyLength(16);
        let err = ProxyError::Vault(vault_err);
        assert_eq!(
            err.to_string(),
            "Vault error: FPE key has invalid length: expected 32 bytes, got 16"
        );
    }

    // --- IntoResponse status code mapping ---

    #[test]
    fn test_config_error_status() {
        let err = ProxyError::Config(anyhow::anyhow!("bad config"));
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_vault_error_status() {
        let err = ProxyError::Vault(crate::vault::VaultError::KeyNotFound("no key".to_string()));
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn test_body_error_status() {
        let err = ProxyError::Body("bad body".to_string());
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_upstream_error_status() {
        let err = ProxyError::Upstream("timeout".to_string());
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn test_json_error_status() {
        let json_err = serde_json::from_str::<serde_json::Value>("{bad}").unwrap_err();
        let err = ProxyError::Json(json_err);
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // --- From conversions ---

    #[test]
    fn test_from_anyhow() {
        let anyhow_err = anyhow::anyhow!("test error");
        let err: ProxyError = anyhow_err.into();
        assert!(matches!(err, ProxyError::Config(_)));
    }

    #[test]
    fn test_from_serde_json() {
        let json_err = serde_json::from_str::<serde_json::Value>("oops").unwrap_err();
        let err: ProxyError = json_err.into();
        assert!(matches!(err, ProxyError::Json(_)));
    }

    #[test]
    fn test_from_vault_error() {
        let vault_err = crate::vault::VaultError::InvalidKeyLength(0);
        let err: ProxyError = vault_err.into();
        assert!(matches!(err, ProxyError::Vault(_)));
    }

    #[test]
    fn test_error_is_debug() {
        let err = ProxyError::Body("test".to_string());
        let debug = format!("{:?}", err);
        assert!(debug.contains("Body"));
    }
}

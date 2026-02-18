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

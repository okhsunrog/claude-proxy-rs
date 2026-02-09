use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("Invalid API key")]
    InvalidApiKey,

    #[error("No authentication configured")]
    NoAuthConfigured,

    #[error("Rate limit exceeded: {0}")]
    RateLimitExceeded(String),

    #[error("Anthropic API error: {0}")]
    AnthropicApiError(String),

    #[error("Failed to parse response: {0}")]
    ParseError(String),

    #[error("OAuth error: {0}")]
    OAuthError(String),

    #[error("Network error: {0}")]
    NetworkError(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Missing required header: {0}")]
    MissingHeader(String),

    #[error("Database error: {0}")]
    DatabaseError(String),
}

impl ProxyError {
    /// Convert error to OpenAI-compatible error response
    pub fn to_openai_response(&self) -> Response {
        let (status, message) = match self {
            ProxyError::InvalidApiKey
            | ProxyError::MissingHeader(_)
            | ProxyError::NoAuthConfigured => (StatusCode::UNAUTHORIZED, self.to_string()),
            ProxyError::RateLimitExceeded(_) => (StatusCode::TOO_MANY_REQUESTS, self.to_string()),
            ProxyError::OAuthError(_) | ProxyError::IoError(_) | ProxyError::DatabaseError(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, self.to_string())
            }
            ProxyError::NetworkError(_)
            | ProxyError::AnthropicApiError(_)
            | ProxyError::ParseError(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
        };

        (status, Json(json!({ "error": message }))).into_response()
    }

    /// Convert error to Anthropic-compatible error response
    pub fn to_anthropic_response(&self) -> Response {
        let (status, error_type, message) = match self {
            ProxyError::InvalidApiKey
            | ProxyError::MissingHeader(_)
            | ProxyError::NoAuthConfigured => (
                StatusCode::UNAUTHORIZED,
                "authentication_error",
                self.to_string(),
            ),
            ProxyError::RateLimitExceeded(_) => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limit_error",
                self.to_string(),
            ),
            ProxyError::OAuthError(_) | ProxyError::IoError(_) | ProxyError::DatabaseError(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                self.to_string(),
            ),
            ProxyError::NetworkError(_)
            | ProxyError::AnthropicApiError(_)
            | ProxyError::ParseError(_) => (StatusCode::BAD_GATEWAY, "api_error", self.to_string()),
        };

        (
            status,
            Json(json!({
                "type": "error",
                "error": {
                    "type": error_type,
                    "message": message
                }
            })),
        )
            .into_response()
    }
}

impl IntoResponse for ProxyError {
    fn into_response(self) -> Response {
        // Default to Anthropic format
        self.to_anthropic_response()
    }
}

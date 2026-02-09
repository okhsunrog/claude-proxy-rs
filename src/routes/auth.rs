use axum::http::{HeaderMap, header};
use reqwest::{Client, RequestBuilder};
use std::sync::Arc;

use crate::AppState;
use crate::auth::ClientKey;
use crate::constants::{ANTHROPIC_VERSION, OAUTH_BETA_HEADER, USER_AGENT};
use crate::error::ProxyError;

/// Result of successful authentication containing the client key and OAuth token
pub struct AuthResult {
    pub client_key: ClientKey,
    pub token: String,
}

/// Extract API key from Authorization: Bearer header (OpenAI style)
fn extract_bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
}

/// Extract API key from either x-api-key or Authorization header (Anthropic style)
fn extract_api_key(headers: &HeaderMap) -> Option<&str> {
    // Try x-api-key header first (standard Anthropic)
    if let Some(key) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        return Some(key);
    }

    // Try Authorization: Bearer header (alternative)
    extract_bearer_token(headers)
}

/// Get OAuth token, refreshing if needed
async fn get_oauth_token(state: &AppState) -> Result<String, ProxyError> {
    match state.oauth.refresh_if_needed().await {
        Ok(Some(token)) => Ok(token),
        Ok(None) => Err(ProxyError::NoAuthConfigured),
        Err(e) => Err(ProxyError::OAuthError(e)),
    }
}

/// Full authentication flow for OpenAI-compatible endpoint
pub async fn authenticate_openai(
    headers: &HeaderMap,
    state: &Arc<AppState>,
) -> Result<AuthResult, ProxyError> {
    let key = extract_bearer_token(headers)
        .ok_or_else(|| ProxyError::MissingHeader("Authorization".to_string()))?;

    let client_key = state
        .client_keys
        .validate(key)
        .await
        .ok_or(ProxyError::InvalidApiKey)?;

    // Check rate limits before proceeding
    if let Err(msg) = state.client_keys.check_limits(&client_key.id).await {
        return Err(ProxyError::RateLimitExceeded(msg));
    }

    let _ = state.client_keys.update_last_used(&client_key.id).await;

    let token = get_oauth_token(state).await?;

    Ok(AuthResult { client_key, token })
}

/// Full authentication flow for Anthropic native endpoint
pub async fn authenticate_anthropic(
    headers: &HeaderMap,
    state: &Arc<AppState>,
) -> Result<AuthResult, ProxyError> {
    let key = extract_api_key(headers)
        .ok_or_else(|| ProxyError::MissingHeader("x-api-key or Authorization".to_string()))?;

    let client_key = state
        .client_keys
        .validate(key)
        .await
        .ok_or(ProxyError::InvalidApiKey)?;

    // Check rate limits before proceeding
    if let Err(msg) = state.client_keys.check_limits(&client_key.id).await {
        return Err(ProxyError::RateLimitExceeded(msg));
    }

    let _ = state.client_keys.update_last_used(&client_key.id).await;

    let token = get_oauth_token(state).await?;

    Ok(AuthResult { client_key, token })
}

/// Build a request to the Anthropic API with OAuth headers
pub fn build_anthropic_request(
    client: &Client,
    url: &str,
    token: &str,
    extra_betas: Option<&[String]>,
    stream: bool,
) -> RequestBuilder {
    // Merge base betas with extra betas from request body
    let beta_header = if let Some(extras) = extra_betas {
        if extras.is_empty() {
            OAUTH_BETA_HEADER.to_string()
        } else {
            let existing: std::collections::HashSet<&str> = OAUTH_BETA_HEADER.split(',').collect();
            let mut result = OAUTH_BETA_HEADER.to_string();
            for beta in extras {
                let beta = beta.trim();
                if !beta.is_empty() && !existing.contains(beta) {
                    result.push(',');
                    result.push_str(beta);
                }
            }
            result
        }
    } else {
        OAUTH_BETA_HEADER.to_string()
    };

    let accept = if stream {
        "text/event-stream"
    } else {
        "application/json"
    };

    client
        .post(url)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", token))
        .header("anthropic-beta", beta_header)
        .header("user-agent", USER_AGENT)
        // Additional headers matching CLIProxyAPI behavior
        .header("anthropic-dangerous-direct-browser-access", "true")
        .header("x-app", "cli")
        .header("x-stainless-helper-method", "stream")
        .header("x-stainless-retry-count", "0")
        .header("x-stainless-runtime-version", "v24.3.0")
        .header("x-stainless-package-version", "0.55.1")
        .header("x-stainless-runtime", "node")
        .header("x-stainless-lang", "js")
        .header("x-stainless-arch", "x86_64")
        .header("x-stainless-os", "Linux")
        .header("x-stainless-timeout", "60")
        .header("connection", "keep-alive")
        .header("accept", accept)
}

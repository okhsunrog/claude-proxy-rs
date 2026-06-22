use axum::http::{HeaderMap, header};
use reqwest::{Client, RequestBuilder};
use std::collections::HashSet;
use std::sync::Arc;
use tracing::warn;

use crate::AppState;
use crate::auth::ClientKey;
use crate::constants::{ANTHROPIC_VERSION, INFERENCE_USER_AGENT, OAUTH_BETA_HEADER};
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

/// Build a non-sensitive fingerprint of a presented API key for logging.
/// Shows only a short prefix and the length so a mistyped/stale key can be
/// recognized without leaking the secret into the logs.
fn key_fingerprint(key: &str) -> String {
    let prefix: String = key.chars().take(12).collect();
    format!("{prefix}…(len={})", key.len())
}

/// Get OAuth token, refreshing if needed
async fn get_oauth_token(state: &AppState) -> Result<String, ProxyError> {
    match state.oauth.refresh_if_needed().await {
        Ok(Some(token)) => Ok(token),
        Ok(None) => Err(ProxyError::NoAuthConfigured),
        Err(e) => Err(ProxyError::OAuthError(e)),
    }
}

/// Shared authentication logic: validate key, check limits, get OAuth token
async fn authenticate_key(
    key: &str,
    state: &Arc<AppState>,
    model: &str,
) -> Result<AuthResult, ProxyError> {
    let client_key = match state.client_keys.validate(key).await? {
        Some(ck) => ck,
        None => {
            warn!(
                key_prefix = %key_fingerprint(key),
                "auth rejected: no enabled key matches the presented API key"
            );
            return Err(ProxyError::InvalidApiKey);
        }
    };

    // Get window resets for limit checks. Pure read from the usage cache —
    // no HTTP I/O. The cache is kept fresh by `patch_from_headers` on every
    // /v1/messages response and by the opportunistic refresh triggered by
    // the admin UI poll.
    let window_resets = state.usage_cache.snapshot().await.window_state();

    // Check global limits (cost-based, derived from per-model aggregation)
    if let Err(msg) = state
        .client_keys
        .check_limits(&client_key.id, &window_resets)
        .await
    {
        warn!(
            key = %client_key.name,
            key_id = %client_key.id,
            "auth rejected: global rate limit exceeded: {msg}"
        );
        return Err(ProxyError::RateLimitExceeded(msg));
    }

    // Check model exists and is enabled
    if !state.models.is_valid(model).await? {
        warn!(
            key = %client_key.name,
            %model,
            "auth rejected: unknown or disabled model"
        );
        return Err(ProxyError::InvalidModel(model.to_string()));
    }

    // Check model access whitelist
    if !state
        .client_keys
        .is_model_allowed(&client_key.id, model)
        .await?
    {
        warn!(
            key = %client_key.name,
            %model,
            "auth rejected: model not in key's allowed-models whitelist"
        );
        return Err(ProxyError::ModelNotAllowed(model.to_string()));
    }

    // Check per-model limits (cost-based, from request_log)
    if let Err(msg) = state
        .client_keys
        .check_model_limits(&client_key.id, model, &window_resets)
        .await
    {
        warn!(
            key = %client_key.name,
            %model,
            "auth rejected: per-model rate limit exceeded: {msg}"
        );
        return Err(ProxyError::RateLimitExceeded(msg));
    }

    // Block keys without extra-usage permission when subscription limits are
    // exhausted. Reads from the usage cache (populated from /v1/messages
    // response headers in near real time); no per-request HTTP call.
    if !client_key.allow_extra_usage && state.usage_cache.is_over_subscription_limit().await {
        warn!(
            key = %client_key.name,
            "auth rejected: subscription limits exhausted (extra usage not allowed for this key)"
        );
        return Err(ProxyError::RateLimitExceeded(
            "Subscription limits exhausted (extra usage not allowed for this key)".into(),
        ));
    }

    if let Err(e) = state.client_keys.update_last_used(&client_key.id).await {
        warn!("Failed to update last_used for key {}: {e}", client_key.id);
    }

    let token = get_oauth_token(state).await?;

    Ok(AuthResult { client_key, token })
}

/// Full authentication flow for OpenAI-compatible endpoint
pub async fn authenticate_openai(
    headers: &HeaderMap,
    state: &Arc<AppState>,
    model: &str,
) -> Result<AuthResult, ProxyError> {
    let key = extract_bearer_token(headers)
        .ok_or_else(|| ProxyError::MissingHeader("Authorization".to_string()))?;
    authenticate_key(key, state, model).await
}

/// Full authentication flow for Anthropic native endpoint
pub async fn authenticate_anthropic(
    headers: &HeaderMap,
    state: &Arc<AppState>,
    model: &str,
) -> Result<AuthResult, ProxyError> {
    let key = extract_api_key(headers)
        .ok_or_else(|| ProxyError::MissingHeader("x-api-key or Authorization".to_string()))?;
    authenticate_key(key, state, model).await
}

/// Parse client-supplied beta flags from the inbound `anthropic-beta` header.
///
/// Native Claude Code (and the Anthropic SDK) send beta flags in this header,
/// not in a body `betas` field. They must be forwarded upstream or Anthropic
/// rejects newer features and tool types (e.g. `advisor_*`) with a 400.
pub fn extract_client_betas(headers: &HeaderMap) -> Vec<String> {
    headers
        .get("anthropic-beta")
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            s.split(',')
                .map(|b| b.trim().to_string())
                .filter(|b| !b.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Merge the base OAuth betas with caller-supplied extras, preserving order and
/// de-duplicating both against the base set and within the extras themselves.
fn build_beta_header(extras: &[String]) -> String {
    let mut seen: HashSet<&str> = OAUTH_BETA_HEADER.split(',').collect();
    let mut result = OAUTH_BETA_HEADER.to_string();
    for beta in extras {
        let beta = beta.trim();
        if !beta.is_empty() && seen.insert(beta) {
            result.push(',');
            result.push_str(beta);
        }
    }
    result
}

/// Build a request to the Anthropic API with OAuth headers.
///
/// Headers mirror the Claude Code 2.1.178 CLI exactly (captured from live
/// traffic) so the upstream request is indistinguishable from the real client.
pub fn build_anthropic_request(
    client: &Client,
    url: &str,
    token: &str,
    extra_betas: Option<&[String]>,
    session_id: &str,
) -> RequestBuilder {
    let beta_header = build_beta_header(extra_betas.unwrap_or(&[]));

    client
        .post(url)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", token))
        .header("anthropic-beta", beta_header)
        .header("user-agent", INFERENCE_USER_AGENT)
        .header("anthropic-dangerous-direct-browser-access", "true")
        .header("x-app", "cli")
        .header("X-Claude-Code-Session-Id", session_id)
        .header("x-stainless-retry-count", "0")
        .header("x-stainless-runtime-version", "v24.3.0")
        .header("x-stainless-package-version", "0.94.0")
        .header("x-stainless-runtime", "node")
        .header("x-stainless-lang", "js")
        .header("x-stainless-arch", "x64")
        .header("x-stainless-os", "Linux")
        .header("x-stainless-timeout", "600")
        .header("connection", "keep-alive")
        .header("accept", "application/json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with_beta(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("anthropic-beta", value.parse().unwrap());
        h
    }

    #[test]
    fn extract_client_betas_splits_and_trims() {
        let h = headers_with_beta("advisor-2026-03-01, fine-grained-tool-streaming-2025-05-14 ,");
        assert_eq!(
            extract_client_betas(&h),
            vec![
                "advisor-2026-03-01".to_string(),
                "fine-grained-tool-streaming-2025-05-14".to_string(),
            ]
        );
    }

    #[test]
    fn extract_client_betas_empty_when_absent() {
        assert!(extract_client_betas(&HeaderMap::new()).is_empty());
    }

    #[test]
    fn build_beta_header_appends_new_betas() {
        let header = build_beta_header(&["advisor-2026-03-01".to_string()]);
        assert!(header.starts_with(OAUTH_BETA_HEADER));
        assert!(header.ends_with(",advisor-2026-03-01"));
    }

    #[test]
    fn build_beta_header_dedups_against_base_and_within_extras() {
        // A beta already in the base set, plus a duplicate extra, are not repeated.
        let base_first = OAUTH_BETA_HEADER.split(',').next().unwrap().to_string();
        let header = build_beta_header(&[
            base_first,
            "advisor-2026-03-01".to_string(),
            "advisor-2026-03-01".to_string(),
        ]);
        assert_eq!(header.matches("advisor-2026-03-01").count(), 1);
        assert_eq!(header, format!("{OAUTH_BETA_HEADER},advisor-2026-03-01"));
    }

    #[test]
    fn build_beta_header_no_extras_is_base() {
        assert_eq!(build_beta_header(&[]), OAUTH_BETA_HEADER);
    }
}

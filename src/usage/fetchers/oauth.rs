//! Fetcher for Anthropic's OAuth `/api/oauth/usage` endpoint.
//!
//! This is the "official" path but it's aggressively rate-limited for
//! third-party OAuth clients — see the doc on
//! [`super::web_session::fetch`] for why we prefer the web-session path
//! when available.

use super::super::error::FetchError;
use super::super::types::SubscriptionUsageResponse;
use crate::AppState;
use crate::constants::{ANTHROPIC_USAGE_URL, ANTHROPIC_VERSION, OAUTH_USAGE_BETA, USER_AGENT};

/// Fetch the full usage snapshot via the OAuth `/api/oauth/usage` endpoint.
/// Returns [`FetchError::NotConfigured`] when no OAuth token is stored.
pub async fn fetch(state: &AppState) -> Result<SubscriptionUsageResponse, FetchError> {
    let token = state
        .oauth
        .refresh_if_needed()
        .await
        .map_err(|e| FetchError::Internal(format!("oauth refresh: {e}")))?
        .ok_or(FetchError::NotConfigured)?;

    let resp = state
        .http_client
        .get(ANTHROPIC_USAGE_URL)
        .header("authorization", format!("Bearer {token}"))
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("anthropic-beta", OAUTH_USAGE_BETA)
        .header("content-type", "application/json")
        .header("user-agent", USER_AGENT)
        .header("accept", "application/json")
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| FetchError::Network(e.to_string()))?;

    let status = resp.status();

    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(FetchError::Upstream {
            status: status.as_u16(),
            body,
        });
    }

    let body = resp
        .text()
        .await
        .map_err(|e| FetchError::Network(format!("read body: {e}")))?;

    serde_json::from_str::<SubscriptionUsageResponse>(&body)
        .map_err(|e| FetchError::Parse(e.to_string()))
}

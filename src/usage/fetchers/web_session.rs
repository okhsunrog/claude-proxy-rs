//! Fetcher for the claude.ai web-session endpoint.
//!
//! See [`super::super::cache::UsageCache`] and the crate-level
//! [`fetch_usage_via_web_session`] doc for the full rationale. In short:
//! Anthropic's OAuth `/api/oauth/usage` endpoint is aggressively rate-limited
//! for third-party clients, but `claude.ai/api/organizations/{uuid}/usage`
//! is not, because it's the endpoint the web UI polls. This fetcher
//! impersonates the browser request using a manually-scraped session cookie.

use tracing::{info, warn};

use super::super::error::FetchError;
use super::super::types::SubscriptionUsageResponse;
use crate::AppState;
use crate::auth::storage::Auth;

pub const WEB_SESSION_PROVIDER: &str = "anthropic_web";

/// Fetch the full usage snapshot via the user's manually-configured
/// claude.ai web session. Returns [`FetchError::NotConfigured`] when no
/// web session is stored, so the fetcher chain falls through to the next
/// candidate without logging a warning.
///
/// ## Session key rotation
///
/// claude.ai responds with a fresh `Set-Cookie: sessionKey=...` on every
/// successful request (sliding expiry — a month out from the last touch).
/// We parse it and `update_web_session_key` the stored value so, as long
/// as the proxy hits this endpoint at least once a month, the session key
/// auto-rotates and never expires. If the user changes their claude.ai
/// password or revokes the session, subsequent requests will start
/// returning 401/403, we'll bubble that up as [`FetchError::Upstream`],
/// and the user has to re-scrape the cookie from DevTools.
pub async fn fetch(state: &AppState) -> Result<SubscriptionUsageResponse, FetchError> {
    let auth = state
        .auth_store
        .get(WEB_SESSION_PROVIDER)
        .await
        .ok_or(FetchError::NotConfigured)?;

    let (session_key, org_uuid, device_id, anonymous_id) = match auth {
        Auth::WebSession {
            session_key,
            org_uuid,
            device_id,
            anonymous_id,
        } => (session_key, org_uuid, device_id, anonymous_id),
        _ => {
            return Err(FetchError::Internal(
                "stored credential under anthropic_web is not a WebSession".into(),
            ));
        }
    };

    let url = format!("https://claude.ai/api/organizations/{org_uuid}/usage");
    let cookie = format!("sessionKey={session_key}");

    let resp = state
        .http_client
        .get(&url)
        .header("cookie", &cookie)
        .header("accept", "*/*")
        .header("accept-language", "en-US,en;q=0.9")
        .header("anthropic-client-platform", "web_claude_ai")
        .header("anthropic-client-version", "1.0.0")
        .header("anthropic-device-id", &device_id)
        .header("anthropic-anonymous-id", &anonymous_id)
        .header("content-type", "application/json")
        .header("referer", "https://claude.ai/settings/usage")
        .header(
            "user-agent",
            "Mozilla/5.0 (X11; Linux x86_64; rv:149.0) Gecko/20100101 Firefox/149.0",
        )
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| FetchError::Network(e.to_string()))?;

    let status = resp.status();

    // Extract the rotated sessionKey from Set-Cookie before consuming the body.
    let rotated_key = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(parse_session_key_from_set_cookie);

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

    // Rotate the stored session key if the server sent a new one.
    if let Some(new_key) = rotated_key
        && new_key != session_key
    {
        match state
            .auth_store
            .update_web_session_key(WEB_SESSION_PROVIDER, &new_key)
            .await
        {
            Ok(()) => info!("rotated web sessionKey from claude.ai Set-Cookie"),
            Err(e) => warn!("failed to rotate web session key: {e}"),
        }
    }

    serde_json::from_str::<SubscriptionUsageResponse>(&body)
        .map_err(|e| FetchError::Parse(e.to_string()))
}

/// Parse a `sessionKey=...; ...` Set-Cookie value and return the cookie value.
/// Returns `None` for any cookie whose name is not exactly `sessionKey`.
fn parse_session_key_from_set_cookie(raw: &str) -> Option<String> {
    let trimmed = raw.trim_start();
    let rest = trimmed.strip_prefix("sessionKey=")?;
    let end = rest.find(';').unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

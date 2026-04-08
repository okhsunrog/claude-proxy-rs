use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::storage::Auth;
use crate::constants::{
    ANTHROPIC_PROFILE_URL, ANTHROPIC_USAGE_URL, ANTHROPIC_VERSION, OAUTH_USAGE_BETA, USER_AGENT,
};

pub const WEB_SESSION_PROVIDER: &str = "anthropic_web";

/// Cached subscription state: window reset times (epoch ms) and utilization percentages.
/// Used to sync per-key rate-limit windows and enforce extra-usage restrictions.
#[derive(Debug, Clone, Default)]
pub struct SubscriptionState {
    pub five_hour_reset_at: Option<u64>,
    pub seven_day_reset_at: Option<u64>,
    /// 5-hour utilization percentage (0.0–100.0+)
    pub five_hour_utilization: Option<f64>,
    /// 7-day utilization percentage (0.0–100.0+)
    pub seven_day_utilization: Option<f64>,
}

// --- API response types ---

#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct UsageLimit {
    pub utilization: Option<f64>,
    pub resets_at: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ExtraUsage {
    pub is_enabled: bool,
    pub monthly_limit: Option<f64>,
    pub used_credits: Option<f64>,
    pub utilization: Option<f64>,
}

#[derive(Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct SubscriptionUsageResponse {
    pub five_hour: Option<UsageLimit>,
    pub seven_day: Option<UsageLimit>,
    pub seven_day_oauth_apps: Option<UsageLimit>,
    pub seven_day_opus: Option<UsageLimit>,
    pub seven_day_sonnet: Option<UsageLimit>,
    pub extra_usage: Option<ExtraUsage>,
    /// True when this response is a fallback (stale cache or derived from
    /// `/v1/messages` response headers) because Anthropic's usage endpoint
    /// returned an error or was unreachable.
    #[serde(default)]
    pub is_stale: bool,
    /// Human-readable description of why the fetch failed. Present only when
    /// the latest fetch attempt failed; None on success or when we have never
    /// tried yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_error: Option<String>,
}

// --- Helpers ---

pub fn timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// Fetch plan name from Anthropic profile endpoint.
/// Returns None on any error (non-critical).
pub async fn fetch_plan_name(state: &AppState) -> Option<String> {
    let token = state.oauth.refresh_if_needed().await.ok()??;
    let resp = state
        .http_client
        .get(ANTHROPIC_PROFILE_URL)
        .header("authorization", format!("Bearer {token}"))
        .header("anthropic-beta", OAUTH_USAGE_BETA)
        .header("content-type", "application/json")
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .ok()?;
    let body: serde_json::Value = resp.json().await.ok()?;
    let account = body.get("account")?;
    if account.get("has_claude_max")?.as_bool() == Some(true) {
        return Some("Max".into());
    }
    if account.get("has_claude_pro")?.as_bool() == Some(true) {
        return Some("Pro".into());
    }
    None
}

/// Fetch subscription window reset times from Anthropic.
/// Returns Default on any error (non-critical).
async fn fetch_window_resets(state: &AppState) -> SubscriptionState {
    let token = match state.oauth.refresh_if_needed().await {
        Ok(Some(t)) => t,
        _ => return SubscriptionState::default(),
    };

    let resp = match state
        .http_client
        .get(ANTHROPIC_USAGE_URL)
        .header("authorization", format!("Bearer {token}"))
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("anthropic-beta", OAUTH_USAGE_BETA)
        .header("content-type", "application/json")
        .header("user-agent", USER_AGENT)
        .header("accept", "application/json")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r,
        _ => return SubscriptionState::default(),
    };

    let usage: SubscriptionUsageResponse = match resp.json().await {
        Ok(u) => u,
        Err(_) => return SubscriptionState::default(),
    };

    extract_subscription_state(&usage)
}

/// Parse subscription usage response into cached state (reset times + utilization).
pub fn extract_subscription_state(usage: &SubscriptionUsageResponse) -> SubscriptionState {
    let parse_reset = |s: &str| -> Option<u64> {
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.timestamp_millis() as u64)
    };

    let five_hour_reset_at = usage
        .five_hour
        .as_ref()
        .and_then(|u| u.resets_at.as_deref())
        .and_then(parse_reset);

    let seven_day_reset_at = usage
        .seven_day
        .as_ref()
        .and_then(|u| u.resets_at.as_deref())
        .and_then(parse_reset);

    let five_hour_utilization = usage.five_hour.as_ref().and_then(|u| u.utilization);
    let seven_day_utilization = usage.seven_day.as_ref().and_then(|u| u.utilization);

    SubscriptionState {
        five_hour_reset_at,
        seven_day_reset_at,
        five_hour_utilization,
        seven_day_utilization,
    }
}

/// Get cached window resets, refreshing inline if stale (window boundary crossed).
pub async fn get_or_refresh_window_resets(state: &AppState) -> SubscriptionState {
    let now = timestamp_millis();
    let cached = state.window_resets.read().await.clone();

    // Refresh if cache is empty or any cached reset time has passed
    let needs_refresh = cached.five_hour_reset_at.is_none()
        || cached.five_hour_reset_at.is_some_and(|t| now >= t)
        || cached.seven_day_reset_at.is_some_and(|t| now >= t);

    if needs_refresh {
        let fresh = fetch_window_resets(state).await;
        if fresh.five_hour_reset_at.is_some() {
            info!(
                "Fetched subscription window resets: 5h={:?}, 7d={:?}",
                fresh.five_hour_reset_at, fresh.seven_day_reset_at
            );
            *state.window_resets.write().await = fresh.clone();
            return fresh;
        }
    }
    cached
}

/// Update the cached window resets from Anthropic rate-limit response headers.
///
/// Anthropic returns these headers on every successful inference response:
///   anthropic-ratelimit-unified-5h-utilization  (0.0–1.0 fraction)
///   anthropic-ratelimit-unified-5h-reset         (unix epoch seconds)
///   anthropic-ratelimit-unified-7d-utilization
///   anthropic-ratelimit-unified-7d-reset
///
/// This matches how Claude Code keeps its utilization state continuously fresh
/// without needing to poll the usage API on every request.
pub async fn update_window_resets_from_headers(
    headers: &reqwest::header::HeaderMap,
    state: &AppState,
) {
    let get_f64 = |name: &str| -> Option<f64> { headers.get(name)?.to_str().ok()?.parse().ok() };
    let get_u64 = |name: &str| -> Option<u64> { headers.get(name)?.to_str().ok()?.parse().ok() };

    // Headers send reset as epoch seconds; we store epoch ms.
    // Utilization is a 0-1 fraction; we store as 0-100 percentage.
    let five_hour_reset_at = get_u64("anthropic-ratelimit-unified-5h-reset").map(|s| s * 1000);
    let seven_day_reset_at = get_u64("anthropic-ratelimit-unified-7d-reset").map(|s| s * 1000);
    let five_hour_utilization =
        get_f64("anthropic-ratelimit-unified-5h-utilization").map(|u| u * 100.0);
    let seven_day_utilization =
        get_f64("anthropic-ratelimit-unified-7d-utilization").map(|u| u * 100.0);

    // Only update if at least the 5h reset is present — otherwise headers weren't included.
    if five_hour_reset_at.is_none() {
        return;
    }

    let fresh = SubscriptionState {
        five_hour_reset_at,
        seven_day_reset_at,
        five_hour_utilization,
        seven_day_utilization,
    };

    info!(
        "Updated subscription state from response headers: 5h_reset={:?} 7d_reset={:?} 5h_util={:?} 7d_util={:?}",
        fresh.five_hour_reset_at,
        fresh.seven_day_reset_at,
        fresh.five_hour_utilization,
        fresh.seven_day_utilization,
    );

    *state.window_resets.write().await = fresh;
}

/// Always fetch fresh subscription state from Anthropic and update cache.
/// Used for pre-request extra-usage checks where stale data is not acceptable.
pub async fn fetch_fresh_subscription_state(state: &AppState) -> SubscriptionState {
    let fresh = fetch_window_resets(state).await;
    if fresh.five_hour_reset_at.is_some() {
        *state.window_resets.write().await = fresh.clone();
    }
    fresh
}

/// Fetch subscription usage via a claude.ai web session instead of the
/// OAuth `/api/oauth/usage` endpoint.
///
/// ## Why this exists
///
/// Anthropic's OAuth usage endpoint (`api.anthropic.com/api/oauth/usage`,
/// beta header `oauth-2025-04-20`) is **aggressively rate-limited** for
/// third-party OAuth clients. In practice it returns `429 Too Many Requests`
/// after a handful of calls per hour with a `rate_limit_error` body and a
/// useless `retry-after: 0` header, and stays 429 for extended windows.
/// Neither refreshing the access token nor waiting for `retry-after` helps;
/// the limit appears to be keyed to the OAuth session/account.
///
/// Claude Code itself mostly avoids this by reading the
/// `anthropic-ratelimit-unified-5h/7d-*` headers that come back on every
/// `/v1/messages` response (see [`update_window_resets_from_headers`]). That
/// covers 5h/7d utilization, but **not** `extra_usage`, `seven_day_sonnet`,
/// or `seven_day_opus` — those only come from the dedicated usage endpoint.
///
/// Meanwhile, the web UI at `claude.ai/settings/usage` calls a completely
/// different endpoint — `claude.ai/api/organizations/{uuid}/usage` — that
/// has the **same response shape** but is **not rate-limited**, because it's
/// intended to be polled from the browser. It authenticates with a
/// `sessionKey` cookie (scraped manually from DevTools, see the admin UI
/// form) and requires a bunch of `anthropic-client-*` and
/// `anthropic-device-id` / `anthropic-anonymous-id` headers to pass
/// server-side fingerprinting.
///
/// This function makes the proxy impersonate that browser call so we can
/// keep `extra_usage` / `seven_day_sonnet` / `seven_day_opus` fresh without
/// hitting the OAuth rate limit. It's an optional, manually-configured
/// fallback in front of the OAuth path (see `get_subscription_usage`).
///
/// ## Session key rotation
///
/// claude.ai responds with a fresh `Set-Cookie: sessionKey=...` on every
/// successful request (sliding expiry — a month out from the last touch).
/// We parse it and `update_web_session_key` the stored value so, as long
/// as the proxy hits this endpoint at least once a month, the session key
/// auto-rotates and never expires. If the user changes their claude.ai
/// password or manually revokes the session, this path starts returning
/// 401/403, the caller in `get_subscription_usage` logs the error and
/// falls back to OAuth, and the user has to re-scrape the cookie from
/// DevTools one more time.
pub async fn fetch_usage_via_web_session(
    state: &AppState,
) -> Result<SubscriptionUsageResponse, String> {
    let auth = state
        .auth_store
        .get(WEB_SESSION_PROVIDER)
        .await
        .ok_or_else(|| "no web session configured".to_string())?;

    let (session_key, org_uuid, device_id, anonymous_id) = match auth {
        Auth::WebSession {
            session_key,
            org_uuid,
            device_id,
            anonymous_id,
        } => (session_key, org_uuid, device_id, anonymous_id),
        _ => return Err("stored credential is not a web session".into()),
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
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("network error: {e}"))?;

    let status = resp.status();

    // Extract rotated sessionKey from Set-Cookie before consuming the body.
    let rotated_key = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(extract_session_key_from_set_cookie);

    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("claude.ai returned {status}: {body}"));
    }

    let body = resp.text().await.map_err(|e| format!("read body: {e}"))?;

    // Rotate stored session key if the server sent a new one.
    if let Some(new_key) = rotated_key
        && new_key != session_key
    {
        if let Err(e) = state
            .auth_store
            .update_web_session_key(WEB_SESSION_PROVIDER, &new_key)
            .await
        {
            warn!("failed to rotate web session key: {e}");
        } else {
            info!("rotated web sessionKey from claude.ai Set-Cookie");
        }
    }

    serde_json::from_str::<SubscriptionUsageResponse>(&body)
        .map_err(|e| format!("parse response: {e}"))
}

/// Parse a `sessionKey=...; ...` Set-Cookie value and return the value.
fn extract_session_key_from_set_cookie(raw: &str) -> Option<String> {
    let trimmed = raw.trim_start();
    let rest = trimmed.strip_prefix("sessionKey=")?;
    let end = rest.find(';').unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

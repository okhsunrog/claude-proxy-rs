use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::info;
use utoipa::ToSchema;

use crate::AppState;
use crate::constants::{
    ANTHROPIC_PROFILE_URL, ANTHROPIC_USAGE_URL, ANTHROPIC_VERSION, OAUTH_USAGE_BETA, USER_AGENT,
};

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

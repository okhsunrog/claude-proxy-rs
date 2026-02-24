use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::info;
use utoipa::ToSchema;

use crate::AppState;
use crate::constants::{
    ANTHROPIC_PROFILE_URL, ANTHROPIC_USAGE_URL, ANTHROPIC_VERSION, OAUTH_BETA_HEADER, USER_AGENT,
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

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct UsageLimit {
    pub utilization: Option<f64>,
    pub resets_at: Option<String>,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ExtraUsage {
    pub is_enabled: bool,
    pub monthly_limit: Option<f64>,
    pub used_credits: Option<f64>,
    pub utilization: Option<f64>,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct SubscriptionUsageResponse {
    pub five_hour: Option<UsageLimit>,
    pub seven_day: Option<UsageLimit>,
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
        .header("anthropic-beta", OAUTH_BETA_HEADER)
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

/// Always fetch fresh subscription state from Anthropic and update cache.
/// Used for pre-request extra-usage checks where stale data is not acceptable.
pub async fn fetch_fresh_subscription_state(state: &AppState) -> SubscriptionState {
    let fresh = fetch_window_resets(state).await;
    if fresh.five_hour_reset_at.is_some() {
        *state.window_resets.write().await = fresh.clone();
    }
    fresh
}

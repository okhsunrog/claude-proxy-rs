//! Thin helpers for the Anthropic OAuth profile endpoint and a shared
//! millisecond timestamp helper.
//!
//! All subscription *usage* state has moved to the [`crate::usage`]
//! module. This file is kept only for callers that still want
//! `fetch_plan_name` (used by `/oauth/status`) and `timestamp_millis`
//! (used by rate-limit bookkeeping and request logging).

use std::time::{SystemTime, UNIX_EPOCH};

use crate::AppState;
use crate::constants::{ANTHROPIC_PROFILE_URL, OAUTH_USAGE_BETA};

/// Current time since the Unix epoch, in milliseconds.
pub fn timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Fetch the human-readable plan name (`"Pro"` / `"Max"`) from Anthropic's
/// profile endpoint. Returns `None` on any error (non-critical).
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

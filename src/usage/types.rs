//! Data types for subscription usage: the on-wire response shape from
//! Anthropic, plus the cache metadata we layer on top of it.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct UsageLimit {
    pub utilization: Option<f64>,
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ExtraUsage {
    pub is_enabled: bool,
    pub monthly_limit: Option<f64>,
    pub used_credits: Option<f64>,
    pub utilization: Option<f64>,
}

/// The full subscription usage response as returned by either the OAuth
/// `api.anthropic.com/api/oauth/usage` endpoint or the web-session
/// `claude.ai/api/organizations/{uuid}/usage` endpoint. Both have identical
/// shape.
///
/// `is_stale`, `source`, and the two freshness timestamps are injected by
/// the proxy's [`UsageCache`] on read — they are not part of Anthropic's
/// response.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct SubscriptionUsageResponse {
    pub five_hour: Option<UsageLimit>,
    pub seven_day: Option<UsageLimit>,
    pub seven_day_oauth_apps: Option<UsageLimit>,
    pub seven_day_opus: Option<UsageLimit>,
    pub seven_day_sonnet: Option<UsageLimit>,
    pub extra_usage: Option<ExtraUsage>,
    /// True when this response was served from cache after a failed refresh
    /// attempt, or assembled from `/v1/messages` response headers because no
    /// full fetch has succeeded yet.
    #[serde(default)]
    pub is_stale: bool,
    /// Error from the most recent fetch attempt. Present iff the latest
    /// refresh failed. `None` on success or before the first attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_error: Option<String>,
    /// Which fetcher produced the current snapshot, or [`UsageSource::None`]
    /// if no full fetch has succeeded yet.
    #[serde(default)]
    pub source: UsageSource,
    /// Epoch-ms timestamp of the last successful full HTTP fetch. Tracks the
    /// freshness of extras (`extra_usage`, `seven_day_sonnet`, etc.) that
    /// cannot be derived from `/v1/messages` headers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_fetched_at: Option<u64>,
    /// Epoch-ms timestamp of the most recent 5h/7d utilization update, from
    /// either a full fetch or a `/v1/messages` response header patch.
    /// Usually much fresher than `full_fetched_at` under active inference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub util_updated_at: Option<u64>,
}

/// A subset of [`SubscriptionUsageResponse`] — window reset timestamps and
/// 5h/7d utilization percentages — used by per-key rate-limit bookkeeping
/// in `auth/rate_limits.rs`. Derived from a [`CachedUsage`] snapshot.
#[derive(Debug, Clone, Default)]
pub struct SubscriptionState {
    pub five_hour_reset_at: Option<u64>,
    pub seven_day_reset_at: Option<u64>,
    pub five_hour_utilization: Option<f64>,
    pub seven_day_utilization: Option<f64>,
}

/// Where the most recent successful full fetch came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
    /// No successful full fetch has happened yet. `snapshot` may still carry
    /// 5h/7d values patched in from inference response headers.
    #[default]
    None,
    /// Fetched via claude.ai web-session cookie.
    WebSession,
    /// Fetched via OAuth `/api/oauth/usage`.
    OAuthApi,
}

/// The mutable state owned by [`UsageCache`]. Everything related to "current
/// subscription usage" lives here; there is no other source of truth.
#[derive(Debug, Clone, Default)]
pub struct CachedUsage {
    /// The most recent full snapshot we've successfully fetched, with 5h/7d
    /// utilization potentially patched from later `/v1/messages` headers.
    pub snapshot: Option<SubscriptionUsageResponse>,
    /// Epoch-ms of the last successful full HTTP fetch (web session or OAuth).
    pub full_fetched_at: Option<u64>,
    /// Epoch-ms of the last util update, from a full fetch or header patch.
    pub util_updated_at: Option<u64>,
    /// The fetcher that produced the snapshot currently in `snapshot`.
    pub source: UsageSource,
    /// Error from the most recent fetch attempt. `None` on success.
    pub last_error: Option<String>,
}

impl CachedUsage {
    /// Project the cache down to the window-reset view used by per-key rate
    /// limit bookkeeping. Handles RFC3339 parsing of the `resets_at` strings.
    pub fn window_state(&self) -> SubscriptionState {
        let Some(snapshot) = &self.snapshot else {
            return SubscriptionState::default();
        };
        let parse_reset = |s: &str| -> Option<u64> {
            chrono::DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|dt| dt.timestamp_millis() as u64)
        };
        SubscriptionState {
            five_hour_reset_at: snapshot
                .five_hour
                .as_ref()
                .and_then(|u| u.resets_at.as_deref())
                .and_then(parse_reset),
            seven_day_reset_at: snapshot
                .seven_day
                .as_ref()
                .and_then(|u| u.resets_at.as_deref())
                .and_then(parse_reset),
            five_hour_utilization: snapshot.five_hour.as_ref().and_then(|u| u.utilization),
            seven_day_utilization: snapshot.seven_day.as_ref().and_then(|u| u.utilization),
        }
    }

    /// Has the user blown through their 5h or 7d subscription window? Used
    /// to block inference for keys that don't have `allow_extra_usage`.
    /// Pure read, no I/O.
    pub fn is_over_subscription_limit(&self) -> bool {
        let state = self.window_state();
        state.five_hour_utilization.is_some_and(|u| u >= 100.0)
            || state.seven_day_utilization.is_some_and(|u| u >= 100.0)
    }

    /// Build the public response the admin UI gets back. Clones the snapshot
    /// (if any) and attaches cache metadata.
    pub fn to_response(&self) -> SubscriptionUsageResponse {
        let mut out = self.snapshot.clone().unwrap_or_default();
        out.is_stale = matches!(self.source, UsageSource::None);
        out.upstream_error = self.last_error.clone();
        out.source = self.source;
        out.full_fetched_at = self.full_fetched_at;
        out.util_updated_at = self.util_updated_at;
        out
    }
}

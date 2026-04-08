//! The single source of truth for subscription usage state.
//!
//! `UsageCache` owns all in-memory caching, fetcher dispatch, and header
//! patching for Claude subscription data. Everything else in the crate
//! that needs to know "how much of the subscription is used right now"
//! reads it through the public methods on this struct.
//!
//! ## Freshness model
//!
//! Two independent timestamps drive refresh decisions:
//!
//! - **`util_updated_at`** — when 5h/7d utilization was last updated,
//!   either via a full fetch or via [`patch_from_headers`] on a
//!   `/v1/messages` response. Under active inference this is nearly
//!   always within seconds.
//! - **`full_fetched_at`** — when the last *full* HTTP fetch succeeded.
//!   Tracks the freshness of extras (`extra_usage`, `seven_day_sonnet`,
//!   etc.) that cannot be derived from headers.
//!
//! [`get_or_refresh`] triggers an HTTP fetch if *either* timestamp is
//! older than its respective threshold:
//!
//! - `UTIL_MAX_AGE_MS` (60 s) — forces a fetch when headers aren't
//!   arriving (e.g. the user is talking to Claude through another client
//!   and only using this proxy for monitoring).
//! - `FULL_MAX_AGE_MS` (5 min) — forces a fetch to refresh the extras.
//!
//! Under active inference, `patch_from_headers` keeps `util_updated_at`
//! fresh for free, so only the 5-minute `full_fetched_at` threshold fires
//! and the proxy hits Anthropic at most once per 5 minutes. With no
//! inference traffic, the 60-second `util_updated_at` threshold fires and
//! we fall into the 1-per-minute cadence. The transition is automatic —
//! no modes, no state machine.

use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::info;

use super::fetchers;
use super::headers::HeaderPatch;
use super::types::{CachedUsage, SubscriptionUsageResponse, UsageLimit};
use crate::AppState;

/// Maximum age of `util_updated_at` before [`get_or_refresh`] will trigger
/// a fetch. Short enough to give near-realtime updates when the user is
/// driving Claude from somewhere other than this proxy.
const UTIL_MAX_AGE_MS: u64 = 60 * 1000;

/// Maximum age of `full_fetched_at` before [`get_or_refresh`] will trigger
/// a fetch. Bounds the staleness of extras (`extra_usage`,
/// `seven_day_sonnet`, etc.) that aren't derivable from headers.
const FULL_MAX_AGE_MS: u64 = 5 * 60 * 1000;

pub struct UsageCache {
    state: RwLock<CachedUsage>,
}

impl Default for UsageCache {
    fn default() -> Self {
        Self::new()
    }
}

impl UsageCache {
    pub fn new() -> Self {
        Self {
            state: RwLock::new(CachedUsage::default()),
        }
    }

    // ----- reads -----

    /// O(1) read. Returns the current in-memory state with no I/O.
    /// Suitable for rate-limit checks and any hot path.
    pub async fn snapshot(&self) -> CachedUsage {
        self.state.read().await.clone()
    }

    /// Read with opportunistic refresh. Triggers a full HTTP fetch iff one
    /// of the two freshness thresholds has expired; otherwise returns the
    /// cached state directly. This is the normal path for the admin UI.
    pub async fn get_or_refresh(&self, state: &AppState) -> CachedUsage {
        if self.needs_refresh().await {
            self.do_fetch_and_store(state).await;
        }
        self.state.read().await.clone()
    }

    /// Force a fetch right now, regardless of freshness. Used when the
    /// caller has a reason to believe the cached data is wrong — e.g.
    /// after saving new web-session credentials, or when the admin UI's
    /// manual "refresh" button is pressed.
    pub async fn force_refresh(&self, state: &AppState) -> CachedUsage {
        self.do_fetch_and_store(state).await;
        self.state.read().await.clone()
    }

    /// Pure read of the "is the subscription blown" flag. Used in the
    /// per-request auth path for keys without `allow_extra_usage`. No I/O.
    pub async fn is_over_subscription_limit(&self) -> bool {
        self.state.read().await.is_over_subscription_limit()
    }

    // ----- writes -----

    /// Patch 5h/7d utilization and reset times from a `/v1/messages`
    /// response header map. Updates `util_updated_at` but **not**
    /// `full_fetched_at` — extras are untouched.
    ///
    /// Creates `snapshot.five_hour` / `snapshot.seven_day` if they didn't
    /// exist yet, so the first `/v1/messages` response is enough to
    /// populate partial usage data even if no full fetch has succeeded.
    pub async fn patch_from_headers(&self, headers: &reqwest::header::HeaderMap) {
        let patch = super::headers::parse(headers);
        if !patch.is_present() {
            return;
        }
        let now = now_ms();
        let mut cache = self.state.write().await;

        let snapshot = cache
            .snapshot
            .get_or_insert_with(SubscriptionUsageResponse::default);
        apply_header_patch(snapshot, &patch);
        cache.util_updated_at = Some(now);

        info!(
            "patched usage from /v1/messages headers: 5h_util={:?} 7d_util={:?}",
            patch.five_hour_utilization, patch.seven_day_utilization,
        );
    }

    /// Clear everything. Called on OAuth logout or any other event that
    /// invalidates the stored identity.
    pub async fn invalidate(&self) {
        let mut cache = self.state.write().await;
        *cache = CachedUsage::default();
    }

    // ----- query helpers -----

    /// Build the admin-facing response from the current state. One-liner
    /// around `CachedUsage::to_response`.
    pub async fn to_response(&self) -> SubscriptionUsageResponse {
        self.state.read().await.to_response()
    }

    // ----- internals -----

    async fn needs_refresh(&self) -> bool {
        let cache = self.state.read().await;
        let now = now_ms();
        let full_stale = match cache.full_fetched_at {
            None => true,
            Some(t) => now.saturating_sub(t) >= FULL_MAX_AGE_MS,
        };
        let util_stale = match cache.util_updated_at {
            None => true,
            Some(t) => now.saturating_sub(t) >= UTIL_MAX_AGE_MS,
        };
        full_stale || util_stale
    }

    async fn do_fetch_and_store(&self, state: &AppState) {
        match fetchers::do_fetch(state).await {
            Ok((resp, source)) => {
                let now = now_ms();
                let mut cache = self.state.write().await;
                cache.snapshot = Some(resp);
                cache.full_fetched_at = Some(now);
                cache.util_updated_at = Some(now);
                cache.source = source;
                cache.last_error = None;
                info!("fetched subscription usage via {:?}", source);
            }
            Err(e) => {
                let mut cache = self.state.write().await;
                cache.last_error = Some(e.to_string());
                // Do not touch `snapshot`/`full_fetched_at`/`util_updated_at` —
                // any prior successful data remains valid, just marked with
                // `last_error` so the admin UI can surface it.
            }
        }
    }
}

/// Apply a `HeaderPatch` to a snapshot in place, creating the `five_hour`
/// and `seven_day` sub-structures if they didn't exist yet.
fn apply_header_patch(snapshot: &mut SubscriptionUsageResponse, patch: &HeaderPatch) {
    let empty_limit = || UsageLimit {
        utilization: None,
        resets_at: None,
    };
    if patch.five_hour_utilization.is_some() || patch.five_hour_reset_at_ms.is_some() {
        let five = snapshot.five_hour.get_or_insert_with(empty_limit);
        if let Some(u) = patch.five_hour_utilization {
            five.utilization = Some(u);
        }
        if let Some(reset_ms) = patch.five_hour_reset_at_ms {
            five.resets_at = iso_from_epoch_ms(reset_ms);
        }
    }
    if patch.seven_day_utilization.is_some() || patch.seven_day_reset_at_ms.is_some() {
        let seven = snapshot.seven_day.get_or_insert_with(empty_limit);
        if let Some(u) = patch.seven_day_utilization {
            seven.utilization = Some(u);
        }
        if let Some(reset_ms) = patch.seven_day_reset_at_ms {
            seven.resets_at = iso_from_epoch_ms(reset_ms);
        }
    }
}

fn iso_from_epoch_ms(ms: u64) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(ms as i64).map(|dt| dt.to_rfc3339())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

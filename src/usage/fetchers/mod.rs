//! Fetcher chain for subscription usage.
//!
//! There are two possible sources: a manually-configured claude.ai web
//! session (preferred, not rate-limited), and the OAuth
//! `/api/oauth/usage` endpoint (always available when an OAuth token is
//! present, but aggressively rate-limited). [`do_fetch`] tries them in
//! that order and returns the first successful result along with a
//! [`UsageSource`] tag identifying which path won.

use tracing::warn;

use super::error::FetchError;
use super::types::{SubscriptionUsageResponse, UsageSource};
use crate::AppState;

pub mod oauth;
pub mod web_session;

pub use web_session::WEB_SESSION_PROVIDER;

/// Run the fetcher chain: web session → OAuth.
///
/// Returns `Ok((response, source))` on the first success. A
/// [`FetchError::NotConfigured`] from a fetcher is silent (the chain just
/// moves to the next candidate); any other error is logged with `warn!`
/// before falling through. If **all** fetchers fail, returns the error
/// from the last one tried.
pub async fn do_fetch(
    state: &AppState,
) -> Result<(SubscriptionUsageResponse, UsageSource), FetchError> {
    // 1. Web session — not rate-limited, preferred when configured.
    match web_session::fetch(state).await {
        Ok(resp) => return Ok((resp, UsageSource::WebSession)),
        Err(FetchError::NotConfigured) => {
            // fall through silently
        }
        Err(e) => {
            warn!("web session usage fetch failed, falling back to OAuth: {e}");
        }
    }

    // 2. OAuth /api/oauth/usage — rate-limited but always available.
    match oauth::fetch(state).await {
        Ok(resp) => Ok((resp, UsageSource::OAuthApi)),
        Err(e) => {
            warn!("OAuth usage fetch failed: {e}");
            Err(e)
        }
    }
}

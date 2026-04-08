//! Subscription usage tracking.
//!
//! This module owns all state and logic related to "how much of the user's
//! Claude subscription is currently used". It replaces the older pair of
//! `cached_usage` + `window_resets` RwLocks with a single [`UsageCache`]
//! that unifies full-fetch snapshots and per-request header patches under
//! one consistent API.
//!
//! See the doc on [`cache::UsageCache`] for the freshness model and the
//! adaptive refresh strategy. See [`fetchers::do_fetch`] for the fetcher
//! chain (web session → OAuth).

mod cache;
mod error;
mod fetchers;
mod headers;
mod types;

pub use cache::UsageCache;
pub use fetchers::WEB_SESSION_PROVIDER;
pub use types::{SubscriptionState, SubscriptionUsageResponse};

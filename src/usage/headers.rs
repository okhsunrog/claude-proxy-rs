//! Parse `/v1/messages` response headers to extract 5h/7d utilization and
//! reset times. This lets us keep utilization continuously fresh without
//! polling the usage API, mirroring how Claude Code itself does it.
//!
//! Anthropic emits four headers on every successful inference response:
//!
//! - `anthropic-ratelimit-unified-5h-utilization` (0.0–1.0 fraction)
//! - `anthropic-ratelimit-unified-5h-reset` (unix epoch seconds)
//! - `anthropic-ratelimit-unified-7d-utilization`
//! - `anthropic-ratelimit-unified-7d-reset`

use reqwest::header::HeaderMap;

/// Values extracted from a single `/v1/messages` response. All fields are
/// independent — the 7d pair may be present without the 5h pair or vice
/// versa. Callers should only apply the update if `is_present()` returns
/// `true`.
#[derive(Debug, Default, Clone)]
pub struct HeaderPatch {
    pub five_hour_reset_at_ms: Option<u64>,
    pub seven_day_reset_at_ms: Option<u64>,
    /// 0–100 percentage (we convert from the header's 0.0–1.0 fraction).
    pub five_hour_utilization: Option<f64>,
    pub seven_day_utilization: Option<f64>,
}

impl HeaderPatch {
    pub fn is_present(&self) -> bool {
        self.five_hour_reset_at_ms.is_some()
            || self.seven_day_reset_at_ms.is_some()
            || self.five_hour_utilization.is_some()
            || self.seven_day_utilization.is_some()
    }
}

pub fn parse(headers: &HeaderMap) -> HeaderPatch {
    let get_f64 = |name: &str| -> Option<f64> { headers.get(name)?.to_str().ok()?.parse().ok() };
    let get_u64 = |name: &str| -> Option<u64> { headers.get(name)?.to_str().ok()?.parse().ok() };

    HeaderPatch {
        // Headers send reset as epoch seconds; we store epoch ms.
        five_hour_reset_at_ms: get_u64("anthropic-ratelimit-unified-5h-reset").map(|s| s * 1000),
        seven_day_reset_at_ms: get_u64("anthropic-ratelimit-unified-7d-reset").map(|s| s * 1000),
        // Fraction → percentage.
        five_hour_utilization: get_f64("anthropic-ratelimit-unified-5h-utilization")
            .map(|u| u * 100.0),
        seven_day_utilization: get_f64("anthropic-ratelimit-unified-7d-utilization")
            .map(|u| u * 100.0),
    }
}

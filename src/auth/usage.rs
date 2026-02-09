//! Token usage tracking and weighted calculation.
//!
//! This module provides a centralized way to track and calculate token usage
//! with appropriate weights for different token types.

use serde::Deserialize;
use serde_json::Value;

use crate::transforms::AnthropicUsage;

/// Weight applied to cache read tokens (they cost 0.1x of regular input)
const CACHE_READ_WEIGHT: f64 = 0.1;

/// Represents token usage from an Anthropic API response.
///
/// This struct collects all token types and provides methods for
/// calculating weighted totals for usage limiting.
#[derive(Debug, Clone, Default)]
pub struct TokenUsageReport {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
}

impl TokenUsageReport {
    /// Create a new empty usage report.
    pub fn new() -> Self {
        Self::default()
    }

    /// Calculate the weighted total tokens for usage limiting.
    ///
    /// Weights:
    /// - Input tokens: 1.0x
    /// - Output tokens: 1.0x
    /// - Cache creation tokens: 1.0x
    /// - Cache read tokens: 0.1x (since they cost 0.1x of regular input)
    pub fn weighted_total(&self) -> u64 {
        let total = self.input_tokens as f64
            + self.output_tokens as f64
            + self.cache_creation_tokens as f64
            + (self.cache_read_tokens as f64 * CACHE_READ_WEIGHT);
        total.round() as u64
    }

    /// Add another usage report to this one (useful for accumulating in streams).
    pub fn add(&mut self, other: &TokenUsageReport) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_creation_tokens += other.cache_creation_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
    }

    /// Parse from a JSON value (Anthropic's usage object format).
    pub fn from_json(value: &Value) -> Self {
        Self {
            input_tokens: value
                .get("input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            output_tokens: value
                .get("output_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            cache_creation_tokens: value
                .get("cache_creation_input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            cache_read_tokens: value
                .get("cache_read_input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
        }
    }

    /// Parse from the typed AnthropicUsage struct (from transforms module).
    pub fn from_anthropic_usage(usage: &AnthropicUsage) -> Self {
        Self {
            input_tokens: usage.input_tokens as u64,
            output_tokens: usage.output_tokens as u64,
            cache_creation_tokens: usage.cache_creation_input_tokens.unwrap_or(0) as u64,
            cache_read_tokens: usage.cache_read_input_tokens.unwrap_or(0) as u64,
        }
    }

    /// Parse from streaming usage data.
    pub fn from_stream_usage(usage: &StreamUsageData) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_creation_tokens: usage.cache_creation_input_tokens,
            cache_read_tokens: usage.cache_read_input_tokens,
        }
    }
}

/// Usage data from streaming events.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct StreamUsageData {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_weighted_total_no_cache() {
        let report = TokenUsageReport {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
        };
        assert_eq!(report.weighted_total(), 150);
    }

    #[test]
    fn test_weighted_total_with_cache() {
        let report = TokenUsageReport {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_tokens: 20,
            cache_read_tokens: 1000, // Should count as 100
        };
        // 100 + 50 + 20 + (1000 * 0.1) = 270
        assert_eq!(report.weighted_total(), 270);
    }

    #[test]
    fn test_add_reports() {
        let mut report1 = TokenUsageReport {
            input_tokens: 100,
            output_tokens: 0,
            cache_creation_tokens: 10,
            cache_read_tokens: 50,
        };
        let report2 = TokenUsageReport {
            input_tokens: 0,
            output_tokens: 200,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
        };
        report1.add(&report2);
        assert_eq!(report1.input_tokens, 100);
        assert_eq!(report1.output_tokens, 200);
        assert_eq!(report1.cache_creation_tokens, 10);
        assert_eq!(report1.cache_read_tokens, 50);
    }

    #[test]
    fn test_from_json() {
        let json = serde_json::json!({
            "input_tokens": 100,
            "output_tokens": 50,
            "cache_creation_input_tokens": 20,
            "cache_read_input_tokens": 30
        });
        let report = TokenUsageReport::from_json(&json);
        assert_eq!(report.input_tokens, 100);
        assert_eq!(report.output_tokens, 50);
        assert_eq!(report.cache_creation_tokens, 20);
        assert_eq!(report.cache_read_tokens, 30);
    }
}

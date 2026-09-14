//! Token usage tracking and cost calculation.
//!
//! This module provides utility functions for working with llm-relay's `Usage` type
//! in the context of proxy-specific rate limiting and cost tracking.

use llm_relay::Usage;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;

/// Parse usage from a JSON value (Anthropic's usage object format).
pub fn usage_from_json(value: &Value) -> Usage {
    Usage {
        input_tokens: value
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        output_tokens: value
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cache_creation_input_tokens: value
            .get("cache_creation_input_tokens")
            .and_then(|v| v.as_u64()),
        cache_read_input_tokens: value
            .get("cache_read_input_tokens")
            .and_then(|v| v.as_u64()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_usage_from_json() {
        let json = json!({
            "input_tokens": 100,
            "output_tokens": 50,
            "cache_creation_input_tokens": 20,
            "cache_read_input_tokens": 30
        });
        let usage = usage_from_json(&json);
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_creation_input_tokens, Some(20));
        assert_eq!(usage.cache_read_input_tokens, Some(30));
    }
}

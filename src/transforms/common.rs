//! Shared utilities for request transformations.

use rand::Rng;
use uuid::Uuid;

// Re-export cache control from llm-relay
pub use llm_relay::convert::cache_control::ensure_cache_control;

/// Generate a fake user ID in Claude Code format.
/// Format: user_[64-hex-chars]_account__session_[UUID-v4]
pub fn generate_fake_user_id() -> String {
    let mut rng = rand::rng();
    let hex_bytes: [u8; 32] = rng.random();
    let hex_part: String = hex_bytes.iter().map(|b| format!("{:02x}", b)).collect();
    let uuid_part = Uuid::new_v4().to_string();
    format!("user_{}_account__session_{}", hex_part, uuid_part)
}

/// Check if a user ID matches Claude Code format.
/// Format: user_[64-hex]_account__session_[uuid-v4]
pub fn is_valid_user_id(user_id: &str) -> bool {
    let parts: Vec<&str> = user_id.split("_account__session_").collect();
    if parts.len() != 2 {
        return false;
    }

    // Check hex part
    let hex_part = parts[0].strip_prefix("user_");
    let valid_hex =
        hex_part.is_some_and(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()));

    // Check UUID part (basic validation)
    let valid_uuid = parts[1].len() == 36 && parts[1].matches('-').count() == 4;

    valid_hex && valid_uuid
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_generate_fake_user_id_format() {
        let id = generate_fake_user_id();
        assert!(id.starts_with("user_"));
        assert!(id.contains("_account__session_"));
        assert!(is_valid_user_id(&id));
    }

    #[test]
    fn test_is_valid_user_id() {
        // Valid format
        let valid = "user_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef_account__session_12345678-1234-1234-1234-123456789012";
        assert!(is_valid_user_id(valid));

        // Invalid formats
        assert!(!is_valid_user_id("invalid"));
        assert!(!is_valid_user_id("user_short_account__session_uuid"));
        assert!(!is_valid_user_id(""));
    }

    #[test]
    fn test_ensure_cache_control_full() {
        let body = json!({
            "system": [
                {"type": "text", "text": "System prompt"}
            ],
            "tools": [
                {"name": "tool1"},
                {"name": "tool2"}
            ],
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "Q1"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "A1"}]},
                {"role": "user", "content": [{"type": "text", "text": "Q2"}]}
            ]
        });
        let result = ensure_cache_control(body);

        // Tools: last tool should have cache_control
        assert_eq!(result["tools"][1]["cache_control"]["type"], "ephemeral");

        // System: last element should have cache_control
        assert_eq!(result["system"][0]["cache_control"]["type"], "ephemeral");

        // Messages: second-to-last user (index 0) should have cache_control
        assert_eq!(
            result["messages"][0]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
    }
}

//! Shared utilities for request transformations.

use rand::Rng;
use serde_json::{Value, json};
use uuid::Uuid;

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

/// Maximum number of cache_control blocks allowed by Anthropic API.
const MAX_CACHE_CONTROL_BLOCKS: usize = 4;

/// Count existing cache_control blocks in an Anthropic request body.
/// Anthropic allows a maximum of 4 cache_control blocks per request.
fn count_cache_control_blocks(body: &Value) -> usize {
    let mut count = 0;

    // Count in system
    if let Some(Value::Array(arr)) = body.get("system") {
        for item in arr {
            if item.get("cache_control").is_some() {
                count += 1;
            }
        }
    }

    // Count in tools
    if let Some(Value::Array(tools)) = body.get("tools") {
        for tool in tools {
            if tool.get("cache_control").is_some() {
                count += 1;
            }
        }
    }

    // Count in messages
    if let Some(Value::Array(messages)) = body.get("messages") {
        for msg in messages {
            if let Some(Value::Array(content)) = msg.get("content") {
                for block in content {
                    if block.get("cache_control").is_some() {
                        count += 1;
                    }
                }
            }
        }
    }

    count
}

/// Inject cache_control breakpoints for optimal prompt caching.
/// Per Anthropic docs, caching order is: tools -> system -> messages.
/// Up to 4 breakpoints allowed, each can reduce cost by 90% on cached tokens.
/// See: https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching
///
/// Respects the 4-block limit by counting existing blocks first.
pub fn ensure_cache_control(mut body: Value) -> Value {
    let existing = count_cache_control_blocks(&body);
    if existing >= MAX_CACHE_CONTROL_BLOCKS {
        return body; // Already at or over limit
    }

    let mut remaining = MAX_CACHE_CONTROL_BLOCKS - existing;

    // 1. Inject into last tool (caches all tool definitions)
    if remaining > 0 {
        let before = count_cache_control_blocks(&body);
        body = inject_tools_cache_control(body);
        let after = count_cache_control_blocks(&body);
        remaining -= after - before;
    }

    // 2. Inject into last system element
    if remaining > 0 {
        let before = count_cache_control_blocks(&body);
        body = inject_system_cache_control(body);
        let after = count_cache_control_blocks(&body);
        remaining -= after - before;
    }

    // 3. Inject into second-to-last user turn (multi-turn caching)
    if remaining > 0 {
        body = inject_messages_cache_control(body);
    }

    body
}

/// Inject cache_control into the last tool in the tools array.
/// Per Anthropic docs: "The cache_control parameter on the last tool definition caches all tool definitions."
/// Only adds cache_control if NO tool already has it.
fn inject_tools_cache_control(mut body: Value) -> Value {
    let tools = match body.get_mut("tools").and_then(|t| t.as_array_mut()) {
        Some(arr) if !arr.is_empty() => arr,
        _ => return body,
    };

    // Skip if any tool already has cache_control
    if tools.iter().any(|t| t.get("cache_control").is_some()) {
        return body;
    }

    // Add to last tool
    if let Some(last) = tools.last_mut()
        && let Some(obj) = last.as_object_mut()
    {
        obj.insert("cache_control".to_string(), json!({"type": "ephemeral"}));
    }

    body
}

/// Inject cache_control into the last element of the system prompt.
/// Converts string system prompts to array format if needed.
/// Only adds cache_control if NO system element already has it.
fn inject_system_cache_control(mut body: Value) -> Value {
    let system = match body.get_mut("system") {
        Some(s) => s,
        None => return body,
    };

    match system {
        Value::Array(arr) if !arr.is_empty() => {
            // Skip if any element has cache_control
            if arr.iter().any(|s| s.get("cache_control").is_some()) {
                return body;
            }
            // Add to last element
            if let Some(last) = arr.last_mut()
                && let Some(obj) = last.as_object_mut()
            {
                obj.insert("cache_control".to_string(), json!({"type": "ephemeral"}));
            }
        }
        Value::String(text) => {
            // Convert to array format with cache_control
            let text = text.clone();
            *system = json!([{
                "type": "text",
                "text": text,
                "cache_control": {"type": "ephemeral"}
            }]);
        }
        _ => {}
    }

    body
}

/// Inject cache_control into the second-to-last user turn for multi-turn caching.
/// Per Anthropic docs: "Place cache_control on the second-to-last User message to let the model reuse the earlier cache."
/// Only adds cache_control if:
/// - There are at least 2 user turns in the conversation
/// - No message content already has cache_control
fn inject_messages_cache_control(mut body: Value) -> Value {
    let messages = match body.get_mut("messages").and_then(|m| m.as_array_mut()) {
        Some(arr) => arr,
        None => return body,
    };

    // Check if any message content already has cache_control
    let has_cache = messages.iter().any(|msg| {
        msg.get("content")
            .and_then(|c| c.as_array())
            .is_some_and(|arr| arr.iter().any(|b| b.get("cache_control").is_some()))
    });
    if has_cache {
        return body;
    }

    // Find user message indices
    let user_indices: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.get("role").and_then(|r| r.as_str()) == Some("user"))
        .map(|(i, _)| i)
        .collect();

    // Need at least 2 user turns
    if user_indices.len() < 2 {
        return body;
    }

    // Get second-to-last user message
    let target_idx = user_indices[user_indices.len() - 2];

    if let Some(msg) = messages.get_mut(target_idx)
        && let Some(content) = msg.get_mut("content")
    {
        match content {
            Value::Array(arr) if !arr.is_empty() => {
                // Add to last content block
                if let Some(last) = arr.last_mut()
                    && let Some(obj) = last.as_object_mut()
                {
                    obj.insert("cache_control".to_string(), json!({"type": "ephemeral"}));
                }
            }
            Value::String(text) => {
                // Convert to array format
                let text = text.clone();
                *content = json!([{
                    "type": "text",
                    "text": text,
                    "cache_control": {"type": "ephemeral"}
                }]);
            }
            _ => {}
        }
    }

    body
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_count_cache_control_blocks() {
        let body = serde_json::json!({
            "system": [
                {"type": "text", "text": "hello", "cache_control": {"type": "ephemeral"}},
                {"type": "text", "text": "world"}
            ],
            "tools": [
                {"name": "tool1", "cache_control": {"type": "ephemeral"}},
                {"name": "tool2"}
            ],
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "hi", "cache_control": {"type": "ephemeral"}}
                ]}
            ]
        });
        assert_eq!(count_cache_control_blocks(&body), 3);
    }

    #[test]
    fn test_inject_tools_cache_control() {
        let body = json!({
            "tools": [
                {"name": "tool1", "description": "First tool"},
                {"name": "tool2", "description": "Second tool"}
            ]
        });
        let result = inject_tools_cache_control(body);

        // Should add cache_control to last tool only
        assert!(result["tools"][0].get("cache_control").is_none());
        assert_eq!(result["tools"][1]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn test_inject_tools_cache_control_skips_if_exists() {
        let body = json!({
            "tools": [
                {"name": "tool1", "cache_control": {"type": "ephemeral"}},
                {"name": "tool2"}
            ]
        });
        let result = inject_tools_cache_control(body);

        // Should not modify - already has cache_control
        assert!(result["tools"][1].get("cache_control").is_none());
    }

    #[test]
    fn test_inject_system_cache_control_array() {
        let body = json!({
            "system": [
                {"type": "text", "text": "First"},
                {"type": "text", "text": "Second"}
            ]
        });
        let result = inject_system_cache_control(body);

        // Should add cache_control to last element only
        assert!(result["system"][0].get("cache_control").is_none());
        assert_eq!(result["system"][1]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn test_inject_system_cache_control_string() {
        let body = json!({
            "system": "Hello world"
        });
        let result = inject_system_cache_control(body);

        // Should convert to array with cache_control
        assert!(result["system"].is_array());
        assert_eq!(result["system"][0]["text"], "Hello world");
        assert_eq!(result["system"][0]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn test_inject_messages_cache_control() {
        let body = json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "First question"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "First answer"}]},
                {"role": "user", "content": [{"type": "text", "text": "Second question"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "Second answer"}]},
                {"role": "user", "content": [{"type": "text", "text": "Third question"}]}
            ]
        });
        let result = inject_messages_cache_control(body);

        // Should add cache_control to second-to-last user turn (index 2)
        assert!(
            result["messages"][0]["content"][0]
                .get("cache_control")
                .is_none()
        );
        assert_eq!(
            result["messages"][2]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        assert!(
            result["messages"][4]["content"][0]
                .get("cache_control")
                .is_none()
        );
    }

    #[test]
    fn test_inject_messages_cache_control_insufficient_turns() {
        let body = json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "Only one user turn"}]}
            ]
        });
        let result = inject_messages_cache_control(body.clone());

        // Should not modify - need at least 2 user turns
        assert_eq!(result, body);
    }

    #[test]
    fn test_inject_messages_cache_control_string_content() {
        let body = json!({
            "messages": [
                {"role": "user", "content": "First question"},
                {"role": "assistant", "content": "Answer"},
                {"role": "user", "content": "Second question"}
            ]
        });
        let result = inject_messages_cache_control(body);

        // Should convert string content to array with cache_control
        assert!(result["messages"][0]["content"].is_array());
        assert_eq!(
            result["messages"][0]["content"][0]["text"],
            "First question"
        );
        assert_eq!(
            result["messages"][0]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
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

    #[test]
    fn test_ensure_cache_control_respects_4_block_limit() {
        // Request already has 3 cache_control blocks in system
        let body = json!({
            "system": [
                {"type": "text", "text": "1", "cache_control": {"type": "ephemeral"}},
                {"type": "text", "text": "2", "cache_control": {"type": "ephemeral"}},
                {"type": "text", "text": "3", "cache_control": {"type": "ephemeral"}}
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

        // Should only add 1 more (to tools), respecting the 4-block limit
        let total = count_cache_control_blocks(&result);
        assert_eq!(total, 4, "Should not exceed 4 cache_control blocks");

        // Tools should have cache_control (was added)
        assert_eq!(result["tools"][1]["cache_control"]["type"], "ephemeral");

        // Messages should NOT have cache_control (would exceed limit)
        assert!(
            result["messages"][0]["content"][0]
                .get("cache_control")
                .is_none()
        );
    }

    #[test]
    fn test_ensure_cache_control_skips_when_at_limit() {
        // Request already has 4 cache_control blocks
        let body = json!({
            "system": [
                {"type": "text", "text": "1", "cache_control": {"type": "ephemeral"}},
                {"type": "text", "text": "2", "cache_control": {"type": "ephemeral"}}
            ],
            "tools": [
                {"name": "tool1", "cache_control": {"type": "ephemeral"}},
                {"name": "tool2", "cache_control": {"type": "ephemeral"}}
            ],
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "Q1"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "A1"}]},
                {"role": "user", "content": [{"type": "text", "text": "Q2"}]}
            ]
        });
        let result = ensure_cache_control(body);

        // Should not add anything - already at limit
        let total = count_cache_control_blocks(&result);
        assert_eq!(total, 4);

        // Messages should NOT have cache_control added
        assert!(
            result["messages"][0]["content"][0]
                .get("cache_control")
                .is_none()
        );
    }
}

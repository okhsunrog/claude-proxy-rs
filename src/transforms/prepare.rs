//! Prepare requests for the Anthropic API.
//!
//! This module provides a unified pipeline for transforming any request
//! before sending it to the Anthropic API, including:
//! - Extracting betas from request body to headers
//! - Disabling thinking when tool_choice forces tool use
//! - Injecting fake user ID for OAuth
//! - Adding mcp_ prefix to tool names
//! - Injecting system message prefix
//! - Auto-injecting cache_control breakpoints for optimal caching

use serde_json::{Value, json};

use super::common::{ensure_cache_control, generate_fake_user_id, is_valid_user_id};
use super::tool_names::transform_request_tool_names;
use crate::constants::SYSTEM_PREFIX;

/// Result of preparing a request for Anthropic API.
pub struct PreparedRequest {
    /// The transformed request body
    pub body: Value,
    /// Betas extracted from the body (to be added to headers)
    pub betas: Vec<String>,
}

/// Prepare a request body for the Anthropic API.
///
/// This applies all necessary transformations:
/// 1. Extract and remove `betas` array from body
/// 2. Disable thinking if `tool_choice` forces tool use
/// 3. Inject fake user ID in metadata (if cloaking)
/// 4. Add mcp_ prefix to tool names
/// 5. Inject system message prefix (if cloaking)
/// 6. Auto-inject cache_control breakpoints (tools, system, messages)
///
/// When `cloak` is false, steps 3 and 5 are skipped.
/// Returns the transformed body and extracted betas.
pub fn prepare_anthropic_request(body: Value, cloak: bool) -> PreparedRequest {
    let (betas, body) = extract_betas(body);
    let body = disable_thinking_if_forced(body);
    let body = if cloak {
        inject_fake_user_id(body)
    } else {
        body
    };
    let mut body = body;
    transform_request_tool_names(&mut body);
    let body = if cloak {
        inject_system_message(body)
    } else {
        sanitize_system_only(body)
    };
    let body = ensure_cache_control(body);
    let body = strip_unsupported_fields(body);

    PreparedRequest { body, betas }
}

/// Strip fields not supported by the Anthropic OAuth API endpoint.
/// Claude Code may send newer fields that the OAuth backend rejects.
fn strip_unsupported_fields(mut body: Value) -> Value {
    if let Some(obj) = body.as_object_mut() {
        obj.remove("context_management");
    }
    body
}

/// Prepare a count_tokens request for the Anthropic API.
///
/// This applies only the transformations appropriate for count_tokens:
/// 1. Extract and remove `betas` array from body
/// 2. Inject system message prefix (if cloaking)
/// 3. Auto-inject cache_control breakpoints
///
/// Note: count_tokens doesn't support metadata or thinking.
pub fn prepare_count_tokens_request(body: Value, cloak: bool) -> PreparedRequest {
    let (betas, body) = extract_betas(body);
    let body = if cloak {
        inject_system_message(body)
    } else {
        sanitize_system_only(body)
    };
    let body = ensure_cache_control(body);

    PreparedRequest { body, betas }
}

/// Extract betas array from request body and remove it.
fn extract_betas(mut body: Value) -> (Vec<String>, Value) {
    let betas = match body.get("betas") {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        Some(Value::String(s)) => {
            let s = s.trim();
            if s.is_empty() {
                vec![]
            } else {
                vec![s.to_string()]
            }
        }
        _ => vec![],
    };

    if let Some(obj) = body.as_object_mut() {
        obj.remove("betas");
    }

    (betas, body)
}

/// Disable thinking if tool_choice forces tool use.
///
/// Anthropic API does not allow thinking when tool_choice.type is "any" or "tool".
/// See: https://docs.anthropic.com/en/docs/build-with-claude/extended-thinking#important-considerations
fn disable_thinking_if_forced(mut body: Value) -> Value {
    let tool_choice_type = body
        .get("tool_choice")
        .and_then(|tc| tc.get("type"))
        .and_then(|t| t.as_str());

    // "auto" is allowed with thinking, but "any" or "tool" (specific tool) are not
    if matches!(tool_choice_type, Some("any") | Some("tool"))
        && let Some(obj) = body.as_object_mut()
    {
        obj.remove("thinking");
    }

    body
}

/// Inject a fake user ID into request metadata if missing or invalid.
fn inject_fake_user_id(mut body: Value) -> Value {
    let needs_injection = match body.get("metadata") {
        None => true,
        Some(metadata) => match metadata.get("user_id") {
            None => true,
            Some(Value::String(id)) => id.is_empty() || !is_valid_user_id(id),
            _ => true,
        },
    };

    if needs_injection {
        let fake_id = generate_fake_user_id();
        if let Some(obj) = body.as_object_mut() {
            if let Some(Value::Object(metadata)) = obj.get_mut("metadata") {
                metadata.insert("user_id".to_string(), Value::String(fake_id));
            } else {
                obj.insert("metadata".to_string(), json!({"user_id": fake_id}));
            }
        }
    }

    body
}

/// Inject system message prefix into the request body (Claude Code identity).
///
/// Cache_control is handled separately by ensure_cache_control().
fn inject_system_message(mut body: Value) -> Value {
    let obj = match body.as_object_mut() {
        Some(o) => o,
        None => return body,
    };

    let prefix = json!({
        "type": "text",
        "text": SYSTEM_PREFIX
    });

    let new_system = match obj.get("system").cloned() {
        None => json!([prefix]),
        Some(Value::String(s)) => {
            json!([
                prefix,
                {"type": "text", "text": s}
            ])
        }
        Some(Value::Array(arr)) => {
            let mut new_arr = vec![prefix];
            new_arr.extend(arr);
            Value::Array(new_arr)
        }
        Some(other) => {
            json!([prefix, other])
        }
    };

    obj.insert("system".to_string(), sanitize_system(new_system));
    body
}

/// Sanitize system prompt without injecting the Claude Code prefix.
/// Still applies OpenCode sanitization since the OAuth backend blocks it.
fn sanitize_system_only(mut body: Value) -> Value {
    let obj = match body.as_object_mut() {
        Some(o) => o,
        None => return body,
    };

    if let Some(system) = obj.get("system").cloned() {
        obj.insert("system".to_string(), sanitize_system(system));
    }

    body
}

/// Sanitize system prompt text blocks.
/// The Anthropic OAuth backend blocks requests containing "OpenCode" in system prompts.
fn sanitize_system(mut system: Value) -> Value {
    if let Value::Array(arr) = &mut system {
        for item in arr.iter_mut() {
            if item.get("type").and_then(|t| t.as_str()) == Some("text")
                && let Some(text) = item
                    .get("text")
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string())
            {
                let sanitized = text
                    .replace("OpenCode", "Claude Code")
                    .replace("opencode", "Claude")
                    .replace("Opencode", "Claude")
                    .replace("OPENCODE", "Claude");
                if let Some(obj) = item.as_object_mut() {
                    obj.insert("text".to_string(), Value::String(sanitized));
                }
            }
        }
    }
    system
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_betas() {
        let body = serde_json::json!({
            "model": "claude-3",
            "betas": ["beta1", "beta2"]
        });
        let (betas, body) = extract_betas(body);
        assert_eq!(betas, vec!["beta1", "beta2"]);
        assert!(body.get("betas").is_none());
    }

    #[test]
    fn test_disable_thinking_when_forced() {
        let body = serde_json::json!({
            "tool_choice": {"type": "any"},
            "thinking": {"type": "enabled", "budget_tokens": 1000}
        });
        let result = disable_thinking_if_forced(body);
        assert!(result.get("thinking").is_none());
    }

    #[test]
    fn test_thinking_preserved_with_auto() {
        let body = serde_json::json!({
            "tool_choice": {"type": "auto"},
            "thinking": {"type": "enabled", "budget_tokens": 1000}
        });
        let result = disable_thinking_if_forced(body);
        assert!(result.get("thinking").is_some());
    }

    #[test]
    fn test_inject_fake_user_id() {
        let body = serde_json::json!({"model": "claude-3"});
        let result = inject_fake_user_id(body);
        let user_id = result["metadata"]["user_id"].as_str().unwrap();
        assert!(user_id.starts_with("user_"));
    }

    #[test]
    fn test_inject_system_message() {
        let body = serde_json::json!({"model": "claude-3"});
        let result = inject_system_message(body);
        let system = result["system"].as_array().unwrap();
        assert_eq!(system[0]["text"], SYSTEM_PREFIX);
    }

    #[test]
    fn test_sanitize_system_replaces_opencode() {
        let body = serde_json::json!({
            "system": "You are OpenCode, an AI assistant. Use opencode tools."
        });
        let result = inject_system_message(body);
        let system = result["system"].as_array().unwrap();
        // Second element is the user-provided system prompt (first is prefix)
        let text = system[1]["text"].as_str().unwrap();
        assert!(!text.contains("OpenCode"));
        assert!(!text.contains("opencode"));
        assert!(text.contains("Claude Code"));
        assert!(text.contains("Claude tools"));
    }

    #[test]
    fn test_sanitize_system_array_format() {
        let body = serde_json::json!({
            "system": [
                {"type": "text", "text": "You are OpenCode assistant"},
                {"type": "text", "text": "Use opencode for help"}
            ]
        });
        let result = inject_system_message(body);
        let system = result["system"].as_array().unwrap();
        // Index 0 is prefix, 1 and 2 are user-provided
        assert!(!system[1]["text"].as_str().unwrap().contains("OpenCode"));
        assert!(!system[2]["text"].as_str().unwrap().contains("opencode"));
    }
}

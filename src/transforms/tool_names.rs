//! Tool name transformations for OAuth requests.
//!
//! OAuth requests require tool names to have an `mcp_` prefix.
//! This module handles adding/stripping that prefix from requests and responses.

use serde_json::Value;

/// Add `mcp_` prefix to a tool name if not already present.
pub fn add_mcp_prefix(name: &str) -> String {
    if name.starts_with("mcp_") {
        name.to_string()
    } else {
        format!("mcp_{}", name)
    }
}

/// Strip `mcp_` prefix from a tool name if present.
pub fn strip_mcp_prefix(name: &str) -> String {
    name.strip_prefix("mcp_").unwrap_or(name).to_string()
}

/// Transform tool names in a request body for OAuth (add mcp_ prefix).
///
/// This transforms:
/// - Tool definitions in the `tools` array (skipping built-in tools with `type` field)
/// - `tool_choice.name` when type is "tool"
/// - `tool_use` blocks in messages
pub fn transform_request_tool_names(body: &mut Value) {
    let obj = match body.as_object_mut() {
        Some(o) => o,
        None => return,
    };

    // Transform tools array
    // Skip built-in tools (web_search, code_execution, etc.) which have a "type" field
    if let Some(Value::Array(tools)) = obj.get_mut("tools") {
        for tool in tools.iter_mut() {
            // Skip built-in tools that have a type field (e.g., web_search, code_execution)
            if tool
                .get("type")
                .and_then(|t| t.as_str())
                .is_some_and(|t| !t.is_empty())
            {
                continue;
            }

            if let Some(name) = tool
                .get("name")
                .and_then(|n| n.as_str())
                .map(|s| s.to_string())
                && let Some(obj) = tool.as_object_mut()
            {
                obj.insert("name".to_string(), Value::String(add_mcp_prefix(&name)));
            }
        }
    }

    // Transform tool_choice.name when type is "tool" (specific tool forced)
    if obj
        .get("tool_choice")
        .and_then(|tc| tc.get("type"))
        .and_then(|t| t.as_str())
        == Some("tool")
        && let Some(Value::Object(tool_choice)) = obj.get_mut("tool_choice")
        && let Some(name) = tool_choice
            .get("name")
            .and_then(|n| n.as_str())
            .map(|s| s.to_string())
        && !name.is_empty()
        && !name.starts_with("mcp_")
    {
        tool_choice.insert("name".to_string(), Value::String(add_mcp_prefix(&name)));
    }

    // Transform tool_use in messages
    if let Some(Value::Array(messages)) = obj.get_mut("messages") {
        for msg in messages.iter_mut() {
            if let Some(Value::Array(content)) = msg.get_mut("content") {
                for block in content.iter_mut() {
                    if block.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                        && let Some(name) = block
                            .get("name")
                            .and_then(|n| n.as_str())
                            .map(|s| s.to_string())
                        && let Some(obj) = block.as_object_mut()
                    {
                        obj.insert("name".to_string(), Value::String(add_mcp_prefix(&name)));
                    }
                }
            }
        }
    }
}

/// Transform tool names in a response body (strip mcp_ prefix).
///
/// This transforms `tool_use` blocks in the response content array.
pub fn transform_response_tool_names(body: &mut Value) {
    if let Some(Value::Array(content)) = body.get_mut("content") {
        for block in content.iter_mut() {
            if block.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                && let Some(name) = block
                    .get("name")
                    .and_then(|n| n.as_str())
                    .map(|s| s.to_string())
                && let Some(obj) = block.as_object_mut()
            {
                obj.insert("name".to_string(), Value::String(strip_mcp_prefix(&name)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_mcp_prefix() {
        assert_eq!(add_mcp_prefix("tool"), "mcp_tool");
        assert_eq!(add_mcp_prefix("mcp_tool"), "mcp_tool");
    }

    #[test]
    fn test_strip_mcp_prefix() {
        assert_eq!(strip_mcp_prefix("mcp_tool"), "tool");
        assert_eq!(strip_mcp_prefix("tool"), "tool");
    }

    #[test]
    fn test_transform_request_skips_builtin_tools() {
        let mut body = serde_json::json!({
            "tools": [
                {"name": "my_tool", "description": "custom tool"},
                {"type": "web_search", "name": "web_search"}
            ]
        });
        transform_request_tool_names(&mut body);

        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools[0]["name"], "mcp_my_tool");
        assert_eq!(tools[1]["name"], "web_search"); // Not prefixed
    }

    #[test]
    fn test_transform_tool_choice_name() {
        let mut body = serde_json::json!({
            "tool_choice": {"type": "tool", "name": "my_tool"}
        });
        transform_request_tool_names(&mut body);
        assert_eq!(body["tool_choice"]["name"], "mcp_my_tool");
    }
}

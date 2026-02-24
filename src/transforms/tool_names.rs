//! Tool name transformations for OAuth requests.
//!
//! OAuth requests require tool names to have an `mcp_` prefix.
//! Re-exports from llm-relay.

pub use llm_relay::convert::tool_names::*;

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

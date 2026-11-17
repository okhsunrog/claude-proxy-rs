use std::collections::{HashMap, HashSet};

use super::tool_adapters::ArgumentAdapter;
use crate::compatibility::ClientProfile;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;

#[derive(Clone, Debug, Default)]
pub struct ToolNameMap {
    aliases: Vec<ToolNameAlias>,
}

#[derive(Clone, Debug)]
struct ToolNameAlias {
    upstream: String,
    client: String,
    adapter: ArgumentAdapter,
}

impl ToolNameMap {
    pub fn forward_request(&self, body: &mut Value) {
        if let Some(choice) = body.get_mut("tool_choice") {
            self.forward_block(choice);
        }
        if let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) {
            for message in messages {
                if let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) {
                    for block in content {
                        if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                            self.forward_block(block);
                        }
                        rewrite_references(block, &|name| {
                            self.aliases
                                .iter()
                                .find(|a| a.client == name)
                                .map(|a| a.upstream.clone())
                        });
                    }
                }
            }
        }
    }

    fn forward_block(&self, block: &mut Value) {
        let Some(alias) = block
            .get("name")
            .and_then(Value::as_str)
            .and_then(|n| self.aliases.iter().find(|a| a.client == n))
        else {
            return;
        };
        set_name(block, &alias.upstream);
        if let Some(input) = block.get_mut("input") {
            alias.adapter.forward(input);
        }
    }

    pub fn estimated_bytes(&self) -> usize {
        self.aliases
            .iter()
            .map(|a| {
                a.upstream.len()
                    + a.client.len()
                    + a.adapter.estimated_bytes()
                    + std::mem::size_of::<ToolNameAlias>()
            })
            .sum()
    }
    pub fn restore(&self, upstream_name: &str) -> String {
        self.aliases
            .iter()
            .find(|alias| alias.upstream == upstream_name)
            .map(|alias| alias.client.clone())
            .unwrap_or_else(|| restore_unaliased(upstream_name))
    }

    pub fn adapter(&self, upstream: &str) -> Option<&ArgumentAdapter> {
        self.aliases
            .iter()
            .find(|a| a.upstream == upstream)
            .map(|a| &a.adapter)
    }

    fn insert(&mut self, upstream: &str, client: &str, adapter: ArgumentAdapter) {
        if self
            .aliases
            .iter()
            .any(|alias| alias.upstream == upstream && alias.client == client)
        {
            return;
        }
        self.aliases.push(ToolNameAlias {
            upstream: upstream.to_string(),
            client: client.to_string(),
            adapter,
        });
    }
}

/// Rewrite client tool names to Claude Code-compatible names for the cloaked
/// OAuth upstream, returning a map to reverse them on the response.
///
/// Known Claude Code built-ins and third-party aliases map to their canonical
/// `mcp_*` name. Genuine MCP tools (`mcp__server__tool`) and Anthropic typed
/// tools (those carrying a `type` field) are left untouched. Everything else —
/// unrecognized tools and alias collisions — is wrapped into the `mcp__`
/// namespace, which Anthropic bills as native MCP usage. Nothing is rejected.
///
/// The rename is decided once from the `tools` definitions, then applied by
/// lookup to `tool_choice` and to `tool_use` blocks in message history, so all
/// three always agree (a collision-wrapped name must match everywhere).
pub fn normalize_tools(body: &mut Value, profile: ClientProfile) -> ToolNameMap {
    let mut map = ToolNameMap::default();
    let mut forward: HashMap<String, (String, ArgumentAdapter)> = HashMap::new();
    let mut assigned: HashSet<String> = body
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .filter(|n| n.starts_with("mcp__"))
        .map(str::to_owned)
        .collect();
    let mut wrapped: Vec<String> = Vec::new();

    if let Some(Value::Array(tools)) = body.get_mut("tools") {
        for tool in tools.iter_mut() {
            // Anthropic typed/server tools (web_search, advisor, …) carry a
            // `type` field and must reach the API under their real name.
            if tool
                .get("type")
                .and_then(|t| t.as_str())
                .is_some_and(|t| !t.is_empty() && t != "custom")
            {
                continue;
            }
            let Some(name) = tool
                .get("name")
                .and_then(|n| n.as_str())
                .map(str::to_string)
            else {
                continue;
            };
            let (upstream, adapter) = plan_upstream(&name, tool, profile, &assigned, &mut wrapped);
            if let Some(schema) = tool.get_mut("input_schema") {
                adapter.schema(schema);
            }
            assigned.insert(upstream.clone());
            if !name.starts_with("mcp__") {
                let client = name.clone();
                forward.insert(client.clone(), (upstream.clone(), adapter.clone()));
                map.insert(&upstream, &client, adapter);
                set_name(tool, &upstream);
            }
        }
    }

    if body
        .get("tool_choice")
        .and_then(|tc| tc.get("type"))
        .and_then(|t| t.as_str())
        == Some("tool")
        && let Some(tool_choice) = body.get_mut("tool_choice")
    {
        apply_forward(tool_choice, &forward);
    }

    if let Some(Value::Array(messages)) = body.get_mut("messages") {
        for msg in messages.iter_mut() {
            let Some(Value::Array(content)) = msg.get_mut("content") else {
                continue;
            };
            for block in content.iter_mut() {
                if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    apply_forward(block, &forward);
                }
            }
        }
    }

    if !wrapped.is_empty() {
        tracing::debug!(
            count = wrapped.len(),
            tools = %wrapped.join(", "),
            "wrapped unrecognized tool names into mcp__ namespace"
        );
    }

    if let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) {
        for message in messages {
            if let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) {
                for block in content {
                    rewrite_references(block, &|name| {
                        forward.get(name).map(|(name, _)| name.clone())
                    });
                }
            }
        }
    }

    map
}

pub fn restore_response_tool_names(body: &mut Value, map: &ToolNameMap) {
    if let Some(Value::Array(content)) = body.get_mut("content") {
        for block in content.iter_mut() {
            if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                restore_name_field(block, map);
            }
            rewrite_references(block, &|name| {
                map.aliases
                    .iter()
                    .find(|a| a.upstream == name)
                    .map(|a| a.client.clone())
            });
        }
    }
}

// Only traverse protocol content containers. Tool inputs and arbitrary JSON in
// text/results can contain identically named keys and are not protocol blocks.
fn rewrite_references(block: &mut Value, rename: &impl Fn(&str) -> Option<String>) {
    match block.get("type").and_then(Value::as_str) {
        Some("tool_reference") => {
            if let Some(name) = block
                .get("tool_name")
                .and_then(Value::as_str)
                .and_then(rename)
                && let Some(object) = block.as_object_mut()
            {
                object.insert("tool_name".into(), Value::String(name));
            }
        }
        Some("tool_result" | "tool_search_tool_result") => {
            if let Some(content) = block.get_mut("content") {
                if let Some(items) = content.as_array_mut() {
                    for item in items {
                        rewrite_references(item, rename);
                    }
                } else if content.is_object() {
                    rewrite_references(content, rename);
                }
            }
        }
        Some("tool_search_tool_search_result") => {
            if let Some(items) = block
                .get_mut("tool_references")
                .and_then(Value::as_array_mut)
            {
                for item in items {
                    rewrite_references(item, rename);
                }
            }
        }
        _ => {}
    }
}

/// Decide the upstream name for a client tool definition.
fn plan_upstream(
    name: &str,
    tool: &Value,
    profile: ClientProfile,
    assigned: &HashSet<String>,
    wrapped: &mut Vec<String>,
) -> (String, ArgumentAdapter) {
    // Genuine MCP tools already use the `mcp__server__tool` shape Anthropic
    // accepts as native MCP usage — leave them exactly as-is.
    if name.starts_with("mcp__") {
        return (name.to_string(), ArgumentAdapter::default());
    }

    // Known built-in / third-party alias → canonical name, unless that
    // canonical name is already taken this request (e.g. grep + glob both map
    // to mcp_WebSearch); in that case fall through to wrapping.
    if let Some((canonical, adapter)) = ArgumentAdapter::select(name, tool, profile)
        && !assigned.contains(canonical)
    {
        return (canonical.to_owned(), adapter);
    }

    // Unrecognized tool, or an alias collision: wrap into the mcp__ namespace.
    let client = name.to_owned();
    wrapped.push(client.clone());
    (wrap_unique(&client, assigned), ArgumentAdapter::default())
}

/// Wrap a client tool name into a unique `mcp__`-prefixed name.
fn wrap_unique(client: &str, assigned: &HashSet<String>) -> String {
    let base = truncate(&format!("mcp__{}", sanitize_tool_name(client)), 64);
    if !assigned.contains(&base) {
        return base;
    }
    let stem = truncate(&base, 60);
    (2..)
        .map(|i| format!("{stem}_{i}"))
        .find(|candidate| !assigned.contains(candidate))
        .expect("a free suffixed name always exists")
}

/// Apply a planned rename to a `name` field by forward lookup. Names not in the
/// map (genuine MCP tools, typed tools, tools absent from the definitions) are
/// left untouched.
fn apply_forward(value: &mut Value, forward: &HashMap<String, (String, ArgumentAdapter)>) {
    let Some(name) = value
        .get("name")
        .and_then(|n| n.as_str())
        .map(str::to_string)
    else {
        return;
    };
    if let Some((upstream, adapter)) = forward.get(&name) {
        set_name(value, upstream);
        if let Some(input) = value.get_mut("input") {
            adapter.forward(input);
        }
    }
}

fn set_name(value: &mut Value, name: &str) {
    if let Some(obj) = value.as_object_mut() {
        obj.insert("name".to_string(), Value::String(name.to_string()));
    }
}

/// Keep only characters Anthropic accepts in tool names (`[A-Za-z0-9_-]`).
fn sanitize_tool_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Truncate to at most `max` bytes. Safe for sanitized names, which are ASCII.
fn truncate(name: &str, max: usize) -> String {
    if name.len() <= max {
        name.to_string()
    } else {
        name.chars().take(max).collect()
    }
}

fn restore_name_field(value: &mut Value, map: &ToolNameMap) {
    let Some(name) = value
        .get("name")
        .and_then(|n| n.as_str())
        .map(str::to_string)
    else {
        return;
    };

    let client_name = map.restore(&name);
    if let Some(adapter) = map.adapter(&name)
        && let Some(input) = value.get_mut("input")
    {
        adapter.reverse(input);
    }
    tracing::info!(tool = %client_name, "tool_use");
    if let Some(obj) = value.as_object_mut() {
        obj.insert("name".to_string(), Value::String(client_name));
    }
}

fn strip_mcp_prefix(name: &str) -> String {
    name.strip_prefix("mcp_").unwrap_or(name).to_string()
}

/// Restore a tool name that has no explicit alias entry.
///
/// Genuine MCP server tools are named `mcp__<server>__<tool>` (double
/// underscore) and must be returned verbatim — the client registered them
/// under that exact name, so stripping the prefix yields a name it can't
/// match (`mcp__flashprobe__list_ports` -> `_flashprobe__list_ports`).
///
/// Only the single-underscore `mcp_<Builtin>` prefix that the request pipeline
/// adds to Claude Code's built-in tools should be stripped back off. This
/// matters on the cloak=false path (native Claude Code), where the alias map
/// is empty and every response name falls through to this fallback.
fn restore_unaliased(name: &str) -> String {
    if name.starts_with("mcp__") {
        name.to_string()
    } else {
        strip_mcp_prefix(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn read_tool(name: &str) -> Value {
        json!({"name":name,"input_schema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}})
    }

    #[test]
    fn schema_history_choice_and_arguments_round_trip() {
        let result =
            json!({"type":"tool_result","tool_use_id":"id","is_error":true,"content":"missing"});
        let mut body = json!({"tools":[read_tool("read_file")],"tool_choice":{"type":"tool","name":"read_file"},"messages":[{"role":"assistant","content":[{"type":"tool_use","id":"id","name":"read_file","input":{"path":"file.txt"}}]},{"role":"user","content":[result.clone()]}]});
        let map = normalize_tools(&mut body, ClientProfile::OpenAi);
        assert_eq!(body["tools"][0]["name"], "mcp_Read");
        assert_eq!(
            body["tools"][0]["input_schema"]["required"],
            json!(["file_path"])
        );
        assert_eq!(
            body["messages"][0]["content"][0]["input"],
            json!({"file_path":"file.txt"})
        );
        assert_eq!(body["tool_choice"]["name"], "mcp_Read");
        assert_eq!(body["messages"][1]["content"][0], result);
        let mut response = json!({"content":[{"type":"tool_use","id":"next","name":"mcp_Read","input":{"file_path":"other.txt"}}]});
        restore_response_tool_names(&mut response, &map);
        assert_eq!(response["content"][0]["name"], "read_file");
        assert_eq!(response["content"][0]["input"], json!({"path":"other.txt"}));
    }

    #[test]
    fn incompatible_contracts_remain_mcp() {
        for name in [
            "list_files",
            "grep",
            "switch_mode",
            "remove",
            "patch",
            "new_task",
        ] {
            let original = read_tool(name);
            let mut body = json!({"tools":[original.clone()]});
            let map = normalize_tools(&mut body, ClientProfile::Roo);
            assert_eq!(body["tools"][0]["name"], format!("mcp__{name}"));
            assert_eq!(body["tools"][0]["input_schema"], original["input_schema"]);
            assert_eq!(map.restore(&format!("mcp__{name}")), name);
        }
        for profile in [ClientProfile::Generic, ClientProfile::OpenCode] {
            let name = if profile == ClientProfile::OpenCode {
                "read"
            } else {
                "read_file"
            };
            let mut body = json!({"tools":[read_tool(name)]});
            normalize_tools(&mut body, profile);
            assert_eq!(body["tools"][0]["name"], format!("mcp__{name}"));
        }
        let mut tool = read_tool("read_file");
        tool["input_schema"]["properties"]["mode"] =
            json!({"type":"string","enum":["slice","indentation"]});
        let mut body = json!({"tools":[tool.clone()]});
        normalize_tools(&mut body, ClientProfile::Roo);
        assert_eq!(body["tools"][0]["name"], "mcp__read_file");
        assert_eq!(body["tools"][0]["input_schema"], tool["input_schema"]);
    }

    #[test]
    fn reserves_mcp_names_and_preserves_client_prefixes() {
        let mut body = json!({"tools":[read_tool("foo"),read_tool("mcp__foo"),read_tool("mcp_custom"),{"type":"web_search_20250305","name":"web_search"}]});
        let map = normalize_tools(&mut body, ClientProfile::Generic);
        assert_eq!(body["tools"][0]["name"], "mcp__foo_2");
        assert_eq!(body["tools"][1]["name"], "mcp__foo");
        assert_eq!(map.restore("mcp__mcp_custom"), "mcp_custom");
        assert_eq!(body["tools"][3]["name"], "web_search");
    }

    #[test]
    fn collision_retains_second_contract() {
        let second = read_tool("read_file");
        let mut body = json!({"tools":[read_tool("Read"),second.clone()]});
        normalize_tools(&mut body, ClientProfile::OpenAi);
        assert_eq!(body["tools"][0]["name"], "mcp_Read");
        assert_eq!(body["tools"][1]["name"], "mcp__read_file");
        assert_eq!(body["tools"][1]["input_schema"], second["input_schema"]);
    }

    #[test]
    fn explicit_custom_type_is_not_a_server_tool() {
        let mut tool = read_tool("read_file");
        tool["type"] = json!("custom");
        let mut body = json!({"tools":[tool,{"name":"web_search","type":"web_search_20250305"}]});
        let map = normalize_tools(&mut body, ClientProfile::OpenAi);
        assert_eq!(body["tools"][0]["name"], "mcp_Read");
        assert_eq!(map.restore("mcp_Read"), "read_file");
        assert_eq!(body["tools"][1]["name"], "web_search");
    }

    #[test]
    fn search_references_round_trip_without_rewriting_tool_input() {
        let server = json!({"type":"tool_search_tool_result","content":{"type":"tool_search_tool_search_result","tool_references":[{"type":"tool_reference","tool_name":"read_file"}]}});
        let custom = json!({"type":"tool_result","content":[{"type":"tool_reference","tool_name":"read_file"}]});
        let opaque = json!({"type":"tool_use","name":"read_file","input":{"path":"p","payload":{"type":"tool_reference","tool_name":"read_file"}}});
        let mut tool = read_tool("read_file");
        tool["defer_loading"] = json!(true);
        let mut body = json!({"tools":[tool],"messages":[{"role":"assistant","content":[server,custom,opaque.clone()]}]});
        let map = normalize_tools(&mut body, ClientProfile::OpenAi);
        assert_eq!(
            body["messages"][0]["content"][0]["content"]["tool_references"][0]["tool_name"],
            "mcp_Read"
        );
        assert_eq!(
            body["messages"][0]["content"][1]["content"][0]["tool_name"],
            "mcp_Read"
        );
        assert_eq!(
            body["messages"][0]["content"][2]["input"]["payload"],
            opaque["input"]["payload"]
        );
        assert_eq!(body["tools"][0]["defer_loading"], true);
        let mut response = json!({"content":body["messages"][0]["content"].clone()});
        restore_response_tool_names(&mut response, &map);
        assert_eq!(
            response["content"][0]["content"]["tool_references"][0]["tool_name"],
            "read_file"
        );
        assert_eq!(
            response["content"][1]["content"][0]["tool_name"],
            "read_file"
        );
        assert_eq!(response["content"][2], opaque);
    }
}

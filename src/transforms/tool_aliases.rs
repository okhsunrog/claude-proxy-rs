use std::collections::{HashMap, HashSet};

use serde_json::Value;
#[cfg(test)]
use serde_json::json;

const CLAUDE_CODE_TOOLS: &[&str] = &[
    "mcp_Agent",
    "mcp_AskUserQuestion",
    "mcp_Bash",
    "mcp_CronCreate",
    "mcp_CronDelete",
    "mcp_CronList",
    "mcp_Edit",
    "mcp_EnterPlanMode",
    "mcp_EnterWorktree",
    "mcp_ExitPlanMode",
    "mcp_ExitWorktree",
    "mcp_LSP",
    "mcp_Monitor",
    "mcp_NotebookEdit",
    "mcp_PushNotification",
    "mcp_Read",
    "mcp_ScheduleWakeup",
    "mcp_ShareOnboardingGuide",
    "mcp_Skill",
    "mcp_TaskCreate",
    "mcp_TaskGet",
    "mcp_TaskList",
    "mcp_TaskOutput",
    "mcp_TaskStop",
    "mcp_TaskUpdate",
    "mcp_WebFetch",
    "mcp_WebSearch",
    "mcp_Write",
];

#[derive(Clone, Debug, Default)]
pub struct ToolNameMap {
    aliases: Vec<ToolNameAlias>,
}

#[derive(Clone, Debug)]
struct ToolNameAlias {
    upstream: String,
    client: String,
}

impl ToolNameMap {
    pub fn restore(&self, upstream_name: &str) -> String {
        self.aliases
            .iter()
            .find(|alias| alias.upstream == upstream_name)
            .map(|alias| alias.client.clone())
            .unwrap_or_else(|| restore_unaliased(upstream_name))
    }

    fn insert(&mut self, upstream: &str, client: &str) {
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
pub fn normalize_claude_code_tool_names(body: &mut Value) -> ToolNameMap {
    let mut map = ToolNameMap::default();
    let mut forward: HashMap<String, String> = HashMap::new();
    let mut assigned: HashSet<String> = HashSet::new();
    let mut wrapped: Vec<String> = Vec::new();

    if let Some(Value::Array(tools)) = body.get_mut("tools") {
        for tool in tools.iter_mut() {
            // Anthropic typed/server tools (web_search, advisor, …) carry a
            // `type` field and must reach the API under their real name.
            if tool
                .get("type")
                .and_then(|t| t.as_str())
                .is_some_and(|t| !t.is_empty())
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
            let upstream = plan_upstream(&name, &assigned, &mut wrapped);
            assigned.insert(upstream.clone());
            if upstream != name {
                let client = strip_mcp_prefix(&name);
                forward.insert(client.clone(), upstream.clone());
                map.insert(&upstream, &client);
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

    map
}

pub fn restore_response_tool_names(body: &mut Value, map: &ToolNameMap) {
    if let Some(Value::Array(content)) = body.get_mut("content") {
        for block in content.iter_mut() {
            if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                restore_name_field(block, map);
            }
        }
    }
}

/// Decide the upstream name for a client tool definition.
fn plan_upstream(name: &str, assigned: &HashSet<String>, wrapped: &mut Vec<String>) -> String {
    // Genuine MCP tools already use the `mcp__server__tool` shape Anthropic
    // accepts as native MCP usage — leave them exactly as-is.
    if name.starts_with("mcp__") {
        return name.to_string();
    }

    // Known built-in / third-party alias → canonical name, unless that
    // canonical name is already taken this request (e.g. grep + glob both map
    // to mcp_WebSearch); in that case fall through to wrapping.
    if let Some(canonical) = normalize_tool_name(name)
        && !assigned.contains(&canonical)
    {
        return canonical;
    }

    // Unrecognized tool, or an alias collision: wrap into the mcp__ namespace.
    let client = strip_mcp_prefix(name);
    wrapped.push(client.clone());
    wrap_unique(&client, assigned)
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
fn apply_forward(value: &mut Value, forward: &HashMap<String, String>) {
    let Some(name) = value
        .get("name")
        .and_then(|n| n.as_str())
        .map(str::to_string)
    else {
        return;
    };
    if let Some(upstream) = forward.get(&strip_mcp_prefix(&name)) {
        set_name(value, upstream);
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
    tracing::info!(tool = %client_name, "tool_use");
    if let Some(obj) = value.as_object_mut() {
        obj.insert("name".to_string(), Value::String(client_name));
    }
}

fn normalize_tool_name(name: &str) -> Option<String> {
    if CLAUDE_CODE_TOOLS.contains(&name) {
        return Some(name.to_string());
    }

    let base = strip_mcp_prefix(name);
    let lower = base.to_ascii_lowercase();
    let normalized = match lower.as_str() {
        "agent" | "task" | "new_task" => "mcp_Agent",
        "askuserquestion" | "ask_user_question" | "ask_followup_question" => "mcp_AskUserQuestion",
        "bash" | "shell" | "terminal" | "run_command" | "execute" => "mcp_Bash",
        "edit" | "remove" | "delete" => "mcp_Edit",
        "list_files" => "mcp_EnterWorktree",
        "switch_mode" => "mcp_EnterPlanMode",
        "lsp" | "multi_patch" | "multipatch" | "codebase_search" => "mcp_LSP",
        "notebookedit" | "notebook_edit" | "patch" => "mcp_NotebookEdit",
        "read" | "read_file" => "mcp_Read",
        "skill" => "mcp_Skill",
        "todowrite" | "todo_write" => "mcp_TaskCreate",
        "todoget" | "todo_get" => "mcp_TaskGet",
        "todoread" | "todo_read" | "todo_list" | "task_list" => "mcp_TaskList",
        "taskoutput" | "task_output" | "attempt_completion" => "mcp_TaskOutput",
        "taskstop" | "task_stop" | "undo" => "mcp_TaskStop",
        "taskupdate" | "task_update" | "update_todo_list" => "mcp_TaskUpdate",
        "fetch" | "web_fetch" | "webfetch" | "http" | "http_request" => "mcp_WebFetch",
        "fs_search" | "search" | "web_search" | "websearch" | "grep" | "glob" | "search_files" => {
            "mcp_WebSearch"
        }
        "write" | "write_file" => "mcp_Write",
        _ => return None,
    };

    Some(normalized.to_string())
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

    #[test]
    fn normalizes_common_tool_aliases() {
        let mut body = json!({
            "tools": [
                {"name": "mcp_shell"},
                {"name": "mcp_fs_search"},
                {"name": "mcp_Read"},
                {"name": "mcp_patch"},
                {"name": "mcp_multi_patch"}
            ],
            "tool_choice": {"type": "tool", "name": "mcp_shell"},
            "messages": [{
                "role": "assistant",
                "content": [{"type": "tool_use", "name": "mcp_fs_search"}]
            }]
        });

        let map = normalize_claude_code_tool_names(&mut body);

        assert_eq!(body["tools"][0]["name"], "mcp_Bash");
        assert_eq!(body["tools"][1]["name"], "mcp_WebSearch");
        assert_eq!(body["tools"][2]["name"], "mcp_Read");
        assert_eq!(body["tools"][3]["name"], "mcp_NotebookEdit");
        assert_eq!(body["tools"][4]["name"], "mcp_LSP");
        assert_eq!(body["tool_choice"]["name"], "mcp_Bash");
        assert_eq!(body["messages"][0]["content"][0]["name"], "mcp_WebSearch");
        assert_eq!(map.restore("mcp_Bash"), "shell");
        assert_eq!(map.restore("mcp_WebSearch"), "fs_search");
        // mcp_Read is already canonical (no alias entry); restored via fallback.
        assert_eq!(map.restore("mcp_Read"), "Read");
        assert_eq!(map.restore("mcp_NotebookEdit"), "patch");
        assert_eq!(map.restore("mcp_LSP"), "multi_patch");
    }

    #[test]
    fn wraps_unknown_tool_names() {
        // OpenCode-style flat MCP names and bare builtins it doesn't share with
        // Claude Code get wrapped into the mcp__ namespace instead of a 400.
        let mut body = json!({
            "tools": [
                {"name": "chrome-devtools_click"},
                {"name": "telegram_send_message"},
                {"name": "question"}
            ]
        });

        let map = normalize_claude_code_tool_names(&mut body);

        assert_eq!(body["tools"][0]["name"], "mcp__chrome-devtools_click");
        assert_eq!(body["tools"][1]["name"], "mcp__telegram_send_message");
        assert_eq!(body["tools"][2]["name"], "mcp__question");
        assert_eq!(
            map.restore("mcp__chrome-devtools_click"),
            "chrome-devtools_click"
        );
        assert_eq!(
            map.restore("mcp__telegram_send_message"),
            "telegram_send_message"
        );
        assert_eq!(map.restore("mcp__question"), "question");
    }

    #[test]
    fn wraps_colliding_aliases() {
        // grep and glob both alias to mcp_WebSearch — the first wins the
        // canonical name, the second is wrapped uniquely (no more 400).
        let mut body = json!({
            "tools": [{"name": "grep"}, {"name": "glob"}]
        });

        let map = normalize_claude_code_tool_names(&mut body);

        assert_eq!(body["tools"][0]["name"], "mcp_WebSearch");
        assert_eq!(body["tools"][1]["name"], "mcp__glob");
        assert_eq!(map.restore("mcp_WebSearch"), "grep");
        assert_eq!(map.restore("mcp__glob"), "glob");
    }

    #[test]
    fn history_tool_use_matches_definition() {
        // A collision-wrapped name must be applied identically in history.
        let mut body = json!({
            "tools": [{"name": "grep"}, {"name": "glob"}],
            "messages": [{
                "role": "assistant",
                "content": [{"type": "tool_use", "name": "glob"}]
            }]
        });

        normalize_claude_code_tool_names(&mut body);

        assert_eq!(body["messages"][0]["content"][0]["name"], "mcp__glob");
    }

    #[test]
    fn leaves_typed_and_genuine_mcp_tools_untouched() {
        let mut body = json!({
            "tools": [
                {"type": "web_search_20250305", "name": "web_search"},
                {"name": "mcp__flashprobe__list_ports"},
                {"name": "chrome-devtools_click"}
            ]
        });

        let map = normalize_claude_code_tool_names(&mut body);

        assert_eq!(body["tools"][0]["name"], "web_search");
        assert_eq!(body["tools"][1]["name"], "mcp__flashprobe__list_ports");
        assert_eq!(body["tools"][2]["name"], "mcp__chrome-devtools_click");
        // Untouched tools have no alias entry and restore verbatim.
        assert_eq!(map.restore("web_search"), "web_search");
        assert_eq!(
            map.restore("mcp__flashprobe__list_ports"),
            "mcp__flashprobe__list_ports"
        );
    }

    #[test]
    fn normalizes_roo_code_aliases() {
        let mut body = json!({
            "tools": [
                {"name": "ask_followup_question"},
                {"name": "attempt_completion"},
                {"name": "codebase_search"},
                {"name": "list_files"},
                {"name": "new_task"},
                {"name": "read_file"},
                {"name": "skill"},
                {"name": "search_files"},
                {"name": "switch_mode"},
                {"name": "update_todo_list"}
            ]
        });

        let map = normalize_claude_code_tool_names(&mut body);
        let names: Vec<&str> = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();

        assert_eq!(
            names,
            vec![
                "mcp_AskUserQuestion",
                "mcp_TaskOutput",
                "mcp_LSP",
                "mcp_EnterWorktree",
                "mcp_Agent",
                "mcp_Read",
                "mcp_Skill",
                "mcp_WebSearch",
                "mcp_EnterPlanMode",
                "mcp_TaskUpdate",
            ]
        );
        assert_eq!(map.restore("mcp_AskUserQuestion"), "ask_followup_question");
        assert_eq!(map.restore("mcp_TaskOutput"), "attempt_completion");
        assert_eq!(map.restore("mcp_EnterWorktree"), "list_files");
        assert_eq!(map.restore("mcp_EnterPlanMode"), "switch_mode");
    }

    #[test]
    fn restores_response_tool_names() {
        let mut body = json!({
            "tools": [{"name": "mcp_shell"}]
        });
        let map = normalize_claude_code_tool_names(&mut body);
        let mut response = json!({
            "content": [{"type": "tool_use", "name": "mcp_Bash"}]
        });

        restore_response_tool_names(&mut response, &map);

        assert_eq!(response["content"][0]["name"], "shell");
    }

    #[test]
    fn restore_preserves_genuine_mcp_tool_names() {
        // Native Claude Code (cloak=false) builds no alias map, so every
        // response tool name falls through to the unaliased fallback. Genuine
        // MCP tools (double-underscore) must round-trip verbatim; built-ins
        // prefixed by the request pipeline still strip back to their bare name.
        let map = ToolNameMap::default();
        assert_eq!(
            map.restore("mcp__flashprobe__list_ports"),
            "mcp__flashprobe__list_ports"
        );
        assert_eq!(
            map.restore("mcp__stm32-data__list_chips"),
            "mcp__stm32-data__list_chips"
        );
        assert_eq!(map.restore("mcp_Read"), "Read");
        assert_eq!(map.restore("mcp_Bash"), "Bash");
    }
}

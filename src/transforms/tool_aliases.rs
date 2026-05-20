use serde_json::Value;
use std::collections::HashSet;

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

#[derive(Debug)]
pub struct UnsupportedToolNames {
    names: Vec<String>,
}

impl UnsupportedToolNames {
    pub fn names(&self) -> &[String] {
        &self.names
    }
}

impl ToolNameMap {
    pub fn restore(&self, upstream_name: &str) -> String {
        self.aliases
            .iter()
            .find(|alias| alias.upstream == upstream_name)
            .map(|alias| alias.client.clone())
            .unwrap_or_else(|| strip_mcp_prefix(upstream_name))
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

pub fn normalize_claude_code_tool_names(
    body: &mut Value,
) -> Result<ToolNameMap, UnsupportedToolNames> {
    let mut map = ToolNameMap::default();
    let mut unsupported = Vec::new();

    normalize_tool_definitions(body, &mut map, &mut unsupported);
    normalize_tool_choice(body, &mut map, &mut unsupported);
    normalize_message_tool_uses(body, &mut map, &mut unsupported);

    unsupported.sort();
    unsupported.dedup();
    if unsupported.is_empty() {
        Ok(map)
    } else {
        Err(UnsupportedToolNames { names: unsupported })
    }
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

fn normalize_tool_definitions(
    body: &mut Value,
    map: &mut ToolNameMap,
    unsupported: &mut Vec<String>,
) {
    let Some(Value::Array(tools)) = body.get_mut("tools") else {
        return;
    };

    let mut seen = HashSet::new();
    for tool in tools.iter_mut() {
        let Some(normalized) = normalize_name_field(tool, map, unsupported) else {
            continue;
        };
        if !seen.insert(normalized.clone()) {
            unsupported.push(format!("duplicate alias target {normalized}"));
        }
    }
}

fn normalize_tool_choice(body: &mut Value, map: &mut ToolNameMap, unsupported: &mut Vec<String>) {
    if body
        .get("tool_choice")
        .and_then(|tc| tc.get("type"))
        .and_then(|t| t.as_str())
        == Some("tool")
        && let Some(tool_choice) = body.get_mut("tool_choice")
    {
        normalize_name_field(tool_choice, map, unsupported);
    }
}

fn normalize_message_tool_uses(
    body: &mut Value,
    map: &mut ToolNameMap,
    unsupported: &mut Vec<String>,
) {
    let Some(Value::Array(messages)) = body.get_mut("messages") else {
        return;
    };

    for msg in messages.iter_mut() {
        let Some(Value::Array(content)) = msg.get_mut("content") else {
            continue;
        };
        for block in content.iter_mut() {
            if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                normalize_name_field(block, map, unsupported);
            }
        }
    }
}

fn normalize_name_field(
    value: &mut Value,
    map: &mut ToolNameMap,
    unsupported: &mut Vec<String>,
) -> Option<String> {
    let name = value
        .get("name")
        .and_then(|n| n.as_str())
        .map(str::to_string)?;

    let Some(normalized) = normalize_tool_name(&name) else {
        unsupported.push(strip_mcp_prefix(&name));
        return None;
    };

    if normalized != name {
        map.insert(&normalized, &strip_mcp_prefix(&name));
        if let Some(obj) = value.as_object_mut() {
            obj.insert("name".to_string(), Value::String(normalized.clone()));
        }
    }

    Some(normalized)
}

fn restore_name_field(value: &mut Value, map: &ToolNameMap) {
    let Some(name) = value
        .get("name")
        .and_then(|n| n.as_str())
        .map(str::to_string)
    else {
        return;
    };

    if let Some(obj) = value.as_object_mut() {
        obj.insert("name".to_string(), Value::String(map.restore(&name)));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_common_tool_aliases() {
        let mut body = serde_json::json!({
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

        let map = normalize_claude_code_tool_names(&mut body).unwrap();

        assert_eq!(body["tools"][0]["name"], "mcp_Bash");
        assert_eq!(body["tools"][1]["name"], "mcp_WebSearch");
        assert_eq!(body["tools"][2]["name"], "mcp_Read");
        assert_eq!(body["tools"][3]["name"], "mcp_NotebookEdit");
        assert_eq!(body["tools"][4]["name"], "mcp_LSP");
        assert_eq!(body["tool_choice"]["name"], "mcp_Bash");
        assert_eq!(body["messages"][0]["content"][0]["name"], "mcp_WebSearch");
        assert_eq!(map.restore("mcp_Bash"), "shell");
        assert_eq!(map.restore("mcp_WebSearch"), "fs_search");
        assert_eq!(map.restore("mcp_Read"), "Read");
        assert_eq!(map.restore("mcp_NotebookEdit"), "patch");
        assert_eq!(map.restore("mcp_LSP"), "multi_patch");
    }

    #[test]
    fn rejects_unknown_tool_names() {
        let mut body = serde_json::json!({
            "tools": [{"name": "mcp_test"}]
        });

        let err = normalize_claude_code_tool_names(&mut body).unwrap_err();

        assert_eq!(err.names(), &["test".to_string()]);
    }

    #[test]
    fn normalizes_roo_code_aliases() {
        let mut body = serde_json::json!({
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

        let map = normalize_claude_code_tool_names(&mut body).unwrap();
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
    fn rejects_colliding_tool_aliases() {
        let mut body = serde_json::json!({
            "tools": [{"name": "mcp_edit"}, {"name": "mcp_remove"}]
        });

        let err = normalize_claude_code_tool_names(&mut body).unwrap_err();

        assert_eq!(
            err.names(),
            &["duplicate alias target mcp_Edit".to_string()]
        );
    }

    #[test]
    fn restores_response_tool_names() {
        let mut body = serde_json::json!({
            "tools": [{"name": "mcp_shell"}]
        });
        let map = normalize_claude_code_tool_names(&mut body).unwrap();
        let mut response = serde_json::json!({
            "content": [{"type": "tool_use", "name": "mcp_Bash"}]
        });

        restore_response_tool_names(&mut response, &map);

        assert_eq!(response["content"][0]["name"], "shell");
    }
}

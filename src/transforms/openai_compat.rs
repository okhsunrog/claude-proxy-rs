//! OpenAI-compatible API format conversion.
//!
//! This module converts between OpenAI chat completion format and
//! Anthropic messages format.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

use super::tool_names::strip_mcp_prefix;
use crate::constants::{DEFAULT_MAX_OUTPUT, OPUS_4_6_MAX_OUTPUT};

const DEFAULT_MODEL: &str = "claude-sonnet-4-5";
const DEFAULT_MAX_TOKENS: u32 = 16000;

// ============================================================================
// OpenAI Request Types
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct OpenAIChatRequest {
    pub model: Option<String>,
    pub messages: Vec<OpenAIMessage>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub stream: Option<bool>,
    pub top_p: Option<f32>,
    pub tools: Option<Vec<Value>>,
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIMessage {
    pub role: String,
    pub content: OpenAIContent,
    pub tool_calls: Option<Vec<OpenAIToolCall>>,
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum OpenAIContent {
    Text(String),
    Parts(Vec<OpenAIContentPart>),
    Null,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum OpenAIContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrl },
}

#[derive(Debug, Deserialize)]
pub struct ImageUrl {
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIToolCall {
    pub id: String,
    pub function: OpenAIFunction,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIFunction {
    pub name: String,
    pub arguments: String,
}

// ============================================================================
// Anthropic Response Types (for deserializing responses)
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct AnthropicResponse {
    #[allow(dead_code)]
    pub id: String,
    pub model: String,
    pub content: Vec<AnthropicResponseContent>,
    pub stop_reason: Option<String>,
    pub usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum AnthropicResponseContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
}

#[derive(Debug, Deserialize)]
pub struct AnthropicUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u32>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u32>,
}

// ============================================================================
// OpenAI Response Types (for serializing responses)
// ============================================================================

#[derive(Debug, Serialize)]
pub struct OpenAIChatResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<OpenAIChoice>,
    pub usage: OpenAIUsage,
}

#[derive(Debug, Serialize)]
pub struct OpenAIChoice {
    pub index: u32,
    pub message: OpenAIResponseMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct OpenAIResponseMessage {
    pub role: String,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OpenAIResponseToolCall>>,
}

#[derive(Debug, Serialize)]
pub struct OpenAIResponseToolCall {
    pub id: String,
    pub r#type: String,
    pub function: OpenAIResponseFunction,
}

#[derive(Debug, Serialize)]
pub struct OpenAIResponseFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Serialize)]
pub struct OpenAIUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u32>,
}

// ============================================================================
// Transform Functions
// ============================================================================

/// Transform an OpenAI chat request to Anthropic format.
///
/// Returns a JSON Value that can be further processed by `prepare_anthropic_request()`.
/// This function handles:
/// - Message format conversion
/// - Tool format conversion (OpenAI function → Anthropic tool)
/// - Model suffix parsing for thinking configuration
/// - reasoning_effort conversion to thinking config
///
/// Note: This does NOT add mcp_ prefix, system injection, or user ID.
/// Those are handled by `prepare_anthropic_request()`.
pub fn transform_openai_request(req: OpenAIChatRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    let mut system_parts: Vec<String> = Vec::new();

    for msg in req.messages {
        match msg.role.as_str() {
            "system" => {
                let text = extract_text_content(&msg.content);
                if !text.is_empty() {
                    system_parts.push(text);
                }
            }
            "user" | "assistant" => {
                let content = convert_message_content(&msg.content, &msg.tool_calls);
                messages.push(serde_json::json!({
                    "role": msg.role,
                    "content": content
                }));
            }
            "tool" => {
                // Tool results must be in a user message for Anthropic API
                // Always create a new user message (matches CLIProxyAPI behavior)
                if let Some(tool_call_id) = msg.tool_call_id {
                    let result_text = extract_text_content(&msg.content);
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": tool_call_id,
                            "content": result_text
                        }]
                    }));
                }
            }
            _ => {}
        }
    }

    // Build system (will be processed by prepare_anthropic_request for prefix injection)
    let system = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n\n"))
    };

    // Convert tools to Anthropic format (without mcp_ prefix - that's done by prepare)
    let tools: Option<Vec<Value>> = req.tools.map(|tools| {
        tools
            .into_iter()
            .map(convert_openai_tool_to_anthropic)
            .collect()
    });

    // Parse model suffix for thinking config
    let raw_model = req.model.unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let (base_model, suffix_effort) = parse_model_suffix(&raw_model);

    // Convert reasoning_effort or suffix to thinking config
    // reasoning_effort takes priority over suffix
    let thinking_config_result = if let Some(effort) = req.reasoning_effort.as_ref() {
        Some(build_thinking_config(&base_model, effort))
    } else {
        suffix_effort
            .as_ref()
            .map(|effort| build_thinking_config(&base_model, effort))
    };

    // Extract thinking and output_config from result
    let (thinking, output_config) = match thinking_config_result {
        Some(config) => (config.thinking, config.output_config),
        None => (None, None),
    };

    // Determine appropriate max_tokens based on model capabilities
    let model_max_output = if is_opus_4_6(&base_model) {
        OPUS_4_6_MAX_OUTPUT // 128K for Opus 4.6
    } else {
        DEFAULT_MAX_OUTPUT // 64K for other Claude 4 models
    };

    let mut max_tokens = req.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);

    // For manual thinking (older models): ensure max_tokens > budget_tokens
    if let Some(ref t) = thinking
        && let Some(budget) = t.get("budget_tokens").and_then(|b| b.as_u64())
        && max_tokens as u64 <= budget
    {
        max_tokens = (budget as u32 + 1000).min(model_max_output);
    }

    // For Opus 4.6 with adaptive thinking, use higher default for thinking headroom
    if is_opus_4_6(&base_model) && thinking.is_some() && max_tokens < 32000 {
        max_tokens = 32000;
    }

    // Cap at model's max output
    max_tokens = max_tokens.min(model_max_output);

    // Build the request
    let mut request = serde_json::json!({
        "model": base_model,
        "max_tokens": max_tokens,
        "messages": messages
    });

    if let Some(s) = system {
        request["system"] = Value::String(s);
    }
    if let Some(t) = req.temperature {
        request["temperature"] = serde_json::json!(t);
    }
    if let Some(p) = req.top_p {
        request["top_p"] = serde_json::json!(p);
    }
    if let Some(s) = req.stream {
        request["stream"] = serde_json::json!(s);
    }
    if let Some(t) = tools {
        request["tools"] = Value::Array(t);
    }
    if let Some(t) = thinking {
        request["thinking"] = t;
    }
    if let Some(oc) = output_config {
        request["output_config"] = oc;
    }

    request
}

/// Transform an Anthropic response to OpenAI format.
pub fn transform_openai_response(resp: AnthropicResponse) -> OpenAIChatResponse {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut text_content = String::new();
    let mut thinking_content = String::new();
    let mut tool_calls: Vec<OpenAIResponseToolCall> = Vec::new();

    for content in resp.content {
        match content {
            AnthropicResponseContent::Text { text } => {
                text_content.push_str(&text);
            }
            AnthropicResponseContent::Thinking { thinking } => {
                thinking_content.push_str(&thinking);
            }
            AnthropicResponseContent::ToolUse { id, name, input } => {
                let name = strip_mcp_prefix(&name);
                tool_calls.push(OpenAIResponseToolCall {
                    id,
                    r#type: "function".to_string(),
                    function: OpenAIResponseFunction {
                        name,
                        arguments: serde_json::to_string(&input).unwrap_or_default(),
                    },
                });
            }
        }
    }

    let finish_reason = resp.stop_reason.map(|r| match r.as_str() {
        "end_turn" => "stop".to_string(),
        "tool_use" => "tool_calls".to_string(),
        "max_tokens" => "length".to_string(),
        other => other.to_string(),
    });

    OpenAIChatResponse {
        id: format!("chatcmpl-{}", now),
        object: "chat.completion".to_string(),
        created: now,
        model: resp.model,
        choices: vec![OpenAIChoice {
            index: 0,
            message: OpenAIResponseMessage {
                role: "assistant".to_string(),
                content: if text_content.is_empty() {
                    None
                } else {
                    Some(text_content)
                },
                reasoning_content: if thinking_content.is_empty() {
                    None
                } else {
                    Some(thinking_content)
                },
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls)
                },
            },
            finish_reason,
        }],
        usage: OpenAIUsage {
            prompt_tokens: resp.usage.input_tokens,
            completion_tokens: resp.usage.output_tokens,
            total_tokens: resp.usage.input_tokens + resp.usage.output_tokens,
            cache_creation_input_tokens: resp.usage.cache_creation_input_tokens,
            cache_read_input_tokens: resp.usage.cache_read_input_tokens,
        },
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

fn extract_text_content(content: &OpenAIContent) -> String {
    match content {
        OpenAIContent::Text(t) => t.clone(),
        OpenAIContent::Parts(parts) => parts
            .iter()
            .filter_map(|p| match p {
                OpenAIContentPart::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        OpenAIContent::Null => String::new(),
    }
}

fn convert_message_content(
    content: &OpenAIContent,
    tool_calls: &Option<Vec<OpenAIToolCall>>,
) -> Vec<Value> {
    let mut result = Vec::new();

    match content {
        OpenAIContent::Text(text) => {
            if !text.is_empty() {
                result.push(serde_json::json!({"type": "text", "text": text}));
            }
        }
        OpenAIContent::Parts(parts) => {
            for part in parts {
                match part {
                    OpenAIContentPart::Text { text } => {
                        if !text.is_empty() {
                            result.push(serde_json::json!({"type": "text", "text": text}));
                        }
                    }
                    OpenAIContentPart::ImageUrl { image_url } => {
                        if let Some((media_type, data)) = extract_base64_image(&image_url.url) {
                            result.push(serde_json::json!({
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": media_type,
                                    "data": data
                                }
                            }));
                        }
                    }
                }
            }
        }
        OpenAIContent::Null => {}
    }

    // Tool calls (without mcp_ prefix - that's handled by prepare_anthropic_request)
    if let Some(calls) = tool_calls {
        for call in calls {
            let input: Value = serde_json::from_str(&call.function.arguments)
                .unwrap_or(Value::Object(Default::default()));
            result.push(serde_json::json!({
                "type": "tool_use",
                "id": call.id,
                "name": call.function.name,
                "input": input
            }));
        }
    }

    // Ensure non-empty content
    if result.is_empty() {
        result.push(serde_json::json!({"type": "text", "text": ""}));
    }

    result
}

fn convert_openai_tool_to_anthropic(tool: Value) -> Value {
    // OpenAI format: {"type": "function", "function": {"name": "...", "description": "...", "parameters": {...}}}
    // Anthropic format: {"name": "...", "description": "...", "input_schema": {...}}

    if let Some(func) = tool.get("function") {
        let name = func.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let description = func.get("description").cloned().unwrap_or(Value::Null);
        let parameters = func
            .get("parameters")
            .cloned()
            .unwrap_or(Value::Object(Default::default()));

        let mut claude_tool = serde_json::Map::new();
        claude_tool.insert("name".to_string(), Value::String(name.to_string()));
        if !description.is_null() {
            claude_tool.insert("description".to_string(), description);
        }
        claude_tool.insert("input_schema".to_string(), parameters);

        Value::Object(claude_tool)
    } else {
        // Already in Anthropic format or unknown format
        tool
    }
}

fn extract_base64_image(url: &str) -> Option<(String, String)> {
    if url.starts_with("data:") {
        let parts: Vec<&str> = url.splitn(2, ',').collect();
        if parts.len() == 2 {
            let header = parts[0];
            let data = parts[1];
            if let Some(media_type) = header.strip_prefix("data:") {
                let media_type = media_type.split(';').next().unwrap_or("image/png");
                return Some((media_type.to_string(), data.to_string()));
            }
        }
    }
    None
}

/// Check if model is Opus 4.6 (uses adaptive thinking)
fn is_opus_4_6(model: &str) -> bool {
    let lower = model.to_lowercase();
    lower.starts_with("claude-opus-4-6") || lower.contains("opus-4-6")
}

/// Parse model suffix for thinking configuration.
/// Returns (base_model, effort_string)
fn parse_model_suffix(model: &str) -> (String, Option<String>) {
    let Some(open_paren) = model.rfind('(') else {
        return (model.to_string(), None);
    };

    if !model.ends_with(')') {
        return (model.to_string(), None);
    }

    let base_model = &model[..open_paren];
    let suffix = &model[open_paren + 1..model.len() - 1];

    // Validate suffix is a known effort level or a number
    let is_valid = matches!(
        suffix.to_lowercase().as_str(),
        "none"
            | "off"
            | "disabled"
            | "low"
            | "minimal"
            | "medium"
            | "med"
            | "high"
            | "xhigh"
            | "max"
            | "auto"
    ) || suffix.parse::<u32>().is_ok();

    if is_valid {
        (base_model.to_string(), Some(suffix.to_string()))
    } else {
        (model.to_string(), None)
    }
}

/// Thinking configuration result
struct ThinkingConfig {
    thinking: Option<Value>,
    output_config: Option<Value>,
}

/// Build thinking configuration based on model and effort level.
/// For Opus 4.6: uses adaptive thinking with effort parameter
/// For older models: uses manual thinking with budget_tokens
fn build_thinking_config(model: &str, effort: &str) -> ThinkingConfig {
    let effort_lower = effort.to_lowercase();

    // Check for disabled
    if matches!(effort_lower.as_str(), "none" | "off" | "disabled") {
        return ThinkingConfig {
            thinking: None,
            output_config: None,
        };
    }

    if is_opus_4_6(model) {
        // Opus 4.6: Use adaptive thinking with effort parameter
        let effort_level = match effort_lower.as_str() {
            "low" | "minimal" => "low",
            "medium" | "med" | "auto" => "medium",
            "high" => "high",
            "xhigh" | "max" => "max",
            // Numeric values: map to reasonable effort levels
            _ => {
                if let Ok(n) = effort.parse::<u32>() {
                    match n {
                        0 => {
                            return ThinkingConfig {
                                thinking: None,
                                output_config: None,
                            };
                        }
                        1..=2048 => "low",
                        2049..=16384 => "medium",
                        16385..=49152 => "high",
                        _ => "max",
                    }
                } else {
                    "high" // default
                }
            }
        };

        ThinkingConfig {
            thinking: Some(serde_json::json!({"type": "adaptive"})),
            output_config: Some(serde_json::json!({"effort": effort_level})),
        }
    } else {
        // Older models: Use manual thinking with budget_tokens
        let budget_tokens = match effort_lower.as_str() {
            "low" | "minimal" => 1024,
            "medium" | "med" => 8192,
            "high" => 32000,
            "xhigh" | "max" => 64000,
            "auto" => 16000,
            _ => effort.parse::<u32>().unwrap_or(8192),
        };

        ThinkingConfig {
            thinking: Some(serde_json::json!({
                "type": "enabled",
                "budget_tokens": budget_tokens
            })),
            output_config: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_model_suffix() {
        assert_eq!(
            parse_model_suffix("claude-sonnet-4-5"),
            ("claude-sonnet-4-5".to_string(), None)
        );
        assert_eq!(
            parse_model_suffix("claude-sonnet-4-5(medium)"),
            ("claude-sonnet-4-5".to_string(), Some("medium".to_string()))
        );
        assert_eq!(
            parse_model_suffix("claude-sonnet-4-5(1000)"),
            ("claude-sonnet-4-5".to_string(), Some("1000".to_string()))
        );
        assert_eq!(
            parse_model_suffix("claude-opus-4-6(high)"),
            ("claude-opus-4-6".to_string(), Some("high".to_string()))
        );
    }

    #[test]
    fn test_is_opus_4_6() {
        assert!(is_opus_4_6("claude-opus-4-6"));
        assert!(is_opus_4_6("claude-opus-4-6-20260101"));
        assert!(!is_opus_4_6("claude-opus-4-5"));
        assert!(!is_opus_4_6("claude-sonnet-4-5"));
    }

    #[test]
    fn test_build_thinking_config_opus_4_6() {
        // Opus 4.6 should use adaptive thinking
        let config = build_thinking_config("claude-opus-4-6", "high");
        assert_eq!(config.thinking.unwrap()["type"], "adaptive");
        assert_eq!(config.output_config.unwrap()["effort"], "high");

        let config = build_thinking_config("claude-opus-4-6", "max");
        assert_eq!(config.output_config.unwrap()["effort"], "max");

        // Numeric effort should map to levels
        let config = build_thinking_config("claude-opus-4-6", "32000");
        assert_eq!(config.output_config.unwrap()["effort"], "high");

        let config = build_thinking_config("claude-opus-4-6", "65000");
        assert_eq!(config.output_config.unwrap()["effort"], "max");
    }

    #[test]
    fn test_build_thinking_config_older_models() {
        // Older models should use manual thinking with budget_tokens
        let config = build_thinking_config("claude-sonnet-4-5", "high");
        assert_eq!(config.thinking.as_ref().unwrap()["type"], "enabled");
        assert_eq!(config.thinking.unwrap()["budget_tokens"], 32000);
        assert!(config.output_config.is_none());

        let config = build_thinking_config("claude-sonnet-4-5", "8192");
        assert_eq!(config.thinking.unwrap()["budget_tokens"], 8192);
    }

    #[test]
    fn test_build_thinking_config_disabled() {
        let config = build_thinking_config("claude-opus-4-6", "none");
        assert!(config.thinking.is_none());

        let config = build_thinking_config("claude-sonnet-4-5", "off");
        assert!(config.thinking.is_none());
    }

    #[test]
    fn test_convert_openai_tool() {
        let openai_tool = serde_json::json!({
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Get weather",
                "parameters": {"type": "object"}
            }
        });
        let result = convert_openai_tool_to_anthropic(openai_tool);
        assert_eq!(result["name"], "get_weather");
        assert_eq!(result["description"], "Get weather");
    }
}

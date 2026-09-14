//! OpenAI-compatible API format conversion.
//!
//! This module converts between OpenAI chat completion format and
//! Anthropic messages format. Types and core conversion logic come from
//! llm-relay; this module adds proxy-specific concerns (model suffix parsing,
//! thinking config, max_tokens caps, mcp_ prefix stripping).

use super::tool_aliases::ToolNameMap;
use llm_relay::anthropic::thinking::{
    build_thinking_for_model, build_thinking_params_json, parse_model_suffix,
    supports_adaptive_thinking,
};
use llm_relay::protocol::{Protocol, translate_request, translate_response};
use llm_relay::wire::anthropic::MessagesResponse;
#[cfg(test)]
use llm_relay::{EffortLevel, ThinkingConfig};
use serde_json::{Value, json};

use crate::constants::{DEFAULT_MAX_OUTPUT, OPUS_4_6_MAX_OUTPUT};

const DEFAULT_MODEL: &str = "claude-sonnet-4-5";
const DEFAULT_MAX_TOKENS: u32 = 16000;

// ============================================================================
// Transform Functions
// ============================================================================

/// Transform an OpenAI chat request to Anthropic format.
///
/// Returns a JSON Value that can be further processed by `prepare_anthropic_request()`.
/// This function handles:
/// - Message format conversion (via llm-relay)
/// - Tool format conversion (via llm-relay)
/// - Model suffix parsing for thinking configuration
/// - reasoning_effort conversion to thinking config
/// - max_tokens adjustment for thinking headroom
///
/// Note: This does NOT add mcp_ prefix, system injection, or user ID.
/// Those are handled by `prepare_anthropic_request()`.
pub fn transform_openai_request(mut req: Value) -> Result<Value, String> {
    // Save proxy-specific fields before consuming
    let stream = req.get("stream").and_then(Value::as_bool);
    let top_p = req.get("top_p").cloned();
    let reasoning_effort = req
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let raw_model = req
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_MODEL)
        .to_owned();
    set_field(&mut req, "model", json!(raw_model));
    // Parse model suffix for thinking config (e.g., "claude-sonnet-4-5(medium)")
    let (base_model, suffix_effort) = parse_model_suffix(&raw_model);

    // Core conversion: messages, system, tools, model, temperature, max_tokens
    let translated = translate_request(Protocol::ChatCompletions, Protocol::Messages, &req)?
        .enforce(llm_relay::Policy::Compatible)?;
    for diagnostic in &translated.diagnostics {
        tracing::debug!(field=%diagnostic.field, reason=%diagnostic.reason, "Applied protocol approximation");
    }
    let mut request = translated.body;

    // Override model with the base model (without suffix)
    set_field(&mut request, "model", Value::String(base_model.clone()));

    // Preserve transport controls.
    if let Some(s) = stream {
        set_field(&mut request, "stream", json!(s));
    }
    if let Some(p) = top_p {
        set_field(&mut request, "top_p", json!(p));
    }

    // Convert reasoning_effort or suffix to thinking config
    // reasoning_effort takes priority over suffix
    let thinking_config = if let Some(effort) = reasoning_effort.as_ref() {
        build_thinking_for_model(&base_model, effort)
    } else {
        suffix_effort
            .as_ref()
            .and_then(|effort| build_thinking_for_model(&base_model, effort))
    };

    // Set thinking and output_config on the request
    if let Some(ref config) = thinking_config {
        let (thinking_json, output_config_json) = build_thinking_params_json(Some(config));
        if let Some(v) = thinking_json {
            set_field(&mut request, "thinking", v);
        }
        if let Some(v) = output_config_json
            && let Some(fields) = v.as_object()
        {
            let mut output = request.get("output_config").cloned().unwrap_or(json!({}));
            if let Some(existing) = output.as_object_mut() {
                existing.extend(fields.clone());
            }
            set_field(&mut request, "output_config", output);
        }
    }

    // Determine appropriate max_tokens based on model capabilities
    let is_opus = {
        let lower = base_model.to_lowercase();
        lower.starts_with("claude-opus-4-6") || lower.contains("opus-4-6")
    };

    let model_max_output = if is_opus {
        OPUS_4_6_MAX_OUTPUT // 128K for Opus 4.6
    } else {
        DEFAULT_MAX_OUTPUT // 64K for other Claude 4 models
    };

    let mut max_tokens = request
        .get("max_tokens")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .unwrap_or(DEFAULT_MAX_TOKENS);

    // For manual thinking (older models): ensure max_tokens > budget_tokens
    if let Some(t) = request.get("thinking")
        && let Some(budget) = t.get("budget_tokens").and_then(|b| b.as_u64())
        && max_tokens as u64 <= budget
    {
        max_tokens = (budget as u32 + 1000).min(model_max_output);
    }

    // For models with adaptive thinking, use higher default for thinking headroom
    if supports_adaptive_thinking(&base_model) && thinking_config.is_some() && max_tokens < 32000 {
        max_tokens = 32000;
    }

    // Cap at model's max output
    max_tokens = max_tokens.min(model_max_output);
    set_field(&mut request, "max_tokens", json!(max_tokens));

    Ok(request)
}

fn set_field(request: &mut Value, key: &str, value: Value) {
    if let Some(object) = request.as_object_mut() {
        object.insert(key.to_string(), value);
    }
}

/// Transform an Anthropic response to OpenAI format.
///
/// Uses llm-relay's core conversion and restores client-visible tool names.
pub fn transform_openai_response(
    resp: MessagesResponse,
    tool_names: &ToolNameMap,
) -> Result<Value, String> {
    let raw = serde_json::to_value(resp).map_err(|e| e.to_string())?;
    let mut response = translate_response(Protocol::Messages, Protocol::ChatCompletions, &raw)?;
    if let Some(choices) = response.get_mut("choices").and_then(Value::as_array_mut) {
        for choice in choices {
            if let Some(calls) = choice
                .pointer_mut("/message/tool_calls")
                .and_then(Value::as_array_mut)
            {
                for call in calls {
                    if let Some(name) = call.pointer_mut("/function/name")
                        && let Some(original) = name.as_str()
                    {
                        *name = Value::String(tool_names.restore(original));
                    }
                }
            }
        }
    }
    Ok(response)
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
    fn test_supports_adaptive_thinking() {
        assert!(supports_adaptive_thinking("claude-opus-4-6"));
        assert!(supports_adaptive_thinking("claude-opus-4-6-20260101"));
        assert!(supports_adaptive_thinking("claude-sonnet-4-6"));
        assert!(!supports_adaptive_thinking("claude-opus-4-5"));
        assert!(!supports_adaptive_thinking("claude-sonnet-4-5"));
    }

    #[test]
    fn test_build_thinking_for_model_opus_4_6() {
        // Opus 4.6 should use adaptive thinking
        let config = build_thinking_for_model("claude-opus-4-6", "high").unwrap();
        assert!(matches!(
            config,
            ThinkingConfig::Adaptive {
                effort: EffortLevel::High
            }
        ));

        let config = build_thinking_for_model("claude-opus-4-6", "max").unwrap();
        assert!(matches!(
            config,
            ThinkingConfig::Adaptive {
                effort: EffortLevel::Max
            }
        ));

        // Numeric effort should map to levels
        let config = build_thinking_for_model("claude-opus-4-6", "32000").unwrap();
        assert!(matches!(
            config,
            ThinkingConfig::Adaptive {
                effort: EffortLevel::High
            }
        ));

        let config = build_thinking_for_model("claude-opus-4-6", "65000").unwrap();
        assert!(matches!(
            config,
            ThinkingConfig::Adaptive {
                effort: EffortLevel::Max
            }
        ));
    }

    #[test]
    fn test_build_thinking_for_model_older_models() {
        // Older models should use manual thinking with budget_tokens
        let config = build_thinking_for_model("claude-sonnet-4-5", "high").unwrap();
        assert!(matches!(
            config,
            ThinkingConfig::Enabled {
                budget_tokens: 32000
            }
        ));

        let config = build_thinking_for_model("claude-sonnet-4-5", "8192").unwrap();
        assert!(matches!(
            config,
            ThinkingConfig::Enabled {
                budget_tokens: 8192
            }
        ));
    }

    #[test]
    fn test_build_thinking_for_model_disabled() {
        let config = build_thinking_for_model("claude-opus-4-6", "none");
        assert!(config.is_none());

        let config = build_thinking_for_model("claude-sonnet-4-5", "off");
        assert!(config.is_none());
    }

    #[test]
    fn test_convert_openai_tool() {
        let openai_tool = json!({
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Get weather",
                "parameters": {"type": "object"}
            }
        });
        let converted=translate_request(Protocol::ChatCompletions,Protocol::Messages,&json!({"model":"test","messages":[{"role":"user","content":"hi"}],"tools":[openai_tool]})).unwrap();
        let result = &converted.body["tools"][0];
        assert_eq!(result["name"], "get_weather");
        assert_eq!(result["description"], "Get weather");
    }
}

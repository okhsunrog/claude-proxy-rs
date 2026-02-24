//! SSE stream transformations.
//!
//! This module provides:
//! - `stream_anthropic_to_openai_with_usage`: Convert Anthropic SSE to OpenAI SSE format with usage tracking
//! - `stream_strip_mcp_prefix_with_usage`: Strip mcp_ prefix from native Anthropic SSE with usage tracking
//!
//! Both functions include keep-alive pings to prevent connection timeouts
//! during long-running requests (e.g., extended thinking).

use async_stream::stream;
use bytes::Bytes;
use futures_util::Stream;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::interval;
use tracing::warn;

use llm_relay::Usage;
use llm_relay::convert::tool_names::strip_mcp_prefix;

use crate::AppState;
use crate::auth::usage::{add_usage, usage_from_json};

/// Keep-alive interval for SSE streams (prevents proxy/load balancer timeouts).
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(15);

/// SSE keep-alive comment (ignored by clients but keeps connection alive).
const KEEP_ALIVE_COMMENT: &str = ": keep-alive\n\n";

/// Map Anthropic stop reason to OpenAI finish reason.
fn map_stop_reason(reason: &str) -> &str {
    match reason {
        "end_turn" => "stop",
        "tool_use" => "tool_calls",
        "max_tokens" => "length",
        other => other,
    }
}

// ============================================================================
// Anthropic SSE Event Types
// ============================================================================

#[derive(Debug, Deserialize)]
struct StreamEvent {
    #[serde(rename = "type")]
    event_type: String,
    delta: Option<Delta>,
    content_block: Option<ContentBlock>,
    #[allow(dead_code)]
    index: Option<u32>,
    message: Option<MessageInfo>,
    usage: Option<StreamUsage>,
}

#[derive(Debug, Deserialize)]
struct Delta {
    #[serde(rename = "type")]
    #[allow(dead_code)]
    delta_type: Option<String>,
    text: Option<String>,
    thinking: Option<String>,
    partial_json: Option<String>,
    stop_reason: Option<String>,
    #[allow(dead_code)]
    usage: Option<StreamUsage>,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    id: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MessageInfo {
    #[allow(dead_code)]
    model: Option<String>,
    usage: Option<StreamUsage>,
}

/// Alias for usage data from streaming events.
type StreamUsage = Usage;

// ============================================================================
// Stream Transformations
// ============================================================================

/// Transform Anthropic SSE stream to OpenAI SSE format with usage tracking.
///
/// This converts Anthropic's streaming events to OpenAI's chat.completion.chunk format,
/// including stripping the mcp_ prefix from tool names.
/// Records token usage to the client keys store after the stream ends.
///
/// Includes keep-alive pings every 15 seconds to prevent connection timeouts.
pub fn stream_anthropic_to_openai_with_usage(
    body: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
    model: String,
    state: Arc<AppState>,
    key_id: String,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send {
    stream! {
        use futures_util::StreamExt;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let mut buffer = String::new();
        let mut current_tool_call_id: Option<String> = None;
        let mut tool_call_index: u32 = 0;
        let mut usage_report = Usage::default();

        let mut body = std::pin::pin!(body);
        let mut keep_alive = interval(KEEP_ALIVE_INTERVAL);
        keep_alive.reset(); // Don't fire immediately

        loop {
            tokio::select! {
                biased; // Prefer data over keep-alive when both ready

                // Data chunk received
                chunk_opt = body.next() => {
                    let Some(chunk_result) = chunk_opt else {
                        break; // Stream ended
                    };

                    let chunk = match chunk_result {
                        Ok(c) => c,
                        Err(e) => {
                            yield Err(std::io::Error::other(e));
                            return;
                        }
                    };

                    let text = match std::str::from_utf8(&chunk) {
                        Ok(t) => t,
                        Err(_) => continue,
                    };

                    buffer.push_str(text);

                    while let Some(newline_pos) = buffer.find('\n') {
                        let line = buffer[..newline_pos].trim().to_string();
                        buffer = buffer[newline_pos + 1..].to_string();

                        if line.is_empty() || !line.starts_with("data: ") {
                            continue;
                        }

                        let data = &line[6..];

                        if data == "[DONE]" {
                            continue;
                        }

                        let event: StreamEvent = match serde_json::from_str(data) {
                            Ok(e) => e,
                            Err(_) => continue,
                        };

                        // Capture usage from message_start event (input + cache tokens)
                        if event.event_type == "message_start"
                            && let Some(msg) = &event.message
                            && let Some(usage) = &msg.usage
                        {
                            add_usage(&mut usage_report, usage);
                        }

                        // Capture usage from message_delta event (output tokens)
                        if event.event_type == "message_delta"
                            && let Some(usage) = &event.usage
                        {
                            add_usage(&mut usage_report, usage);
                        }

                        match event.event_type.as_str() {
                            "content_block_start" => {
                                if let Some(block) = &event.content_block
                                    && block.block_type == "tool_use"
                                {
                                    current_tool_call_id = block.id.clone();
                                    let name = block.name.as_ref().map(|n| strip_mcp_prefix(n));

                                    let chunk = serde_json::json!({
                                        "id": format!("chatcmpl-{}", now),
                                        "object": "chat.completion.chunk",
                                        "created": now,
                                        "model": &model,
                                        "choices": [{
                                            "index": 0,
                                            "delta": {
                                                "tool_calls": [{
                                                    "index": tool_call_index,
                                                    "id": current_tool_call_id,
                                                    "type": "function",
                                                    "function": {
                                                        "name": name,
                                                        "arguments": ""
                                                    }
                                                }]
                                            },
                                            "finish_reason": Value::Null
                                        }]
                                    });

                                    let sse = format!("data: {}\n\n", chunk);
                                    yield Ok(Bytes::from(sse));
                                }
                            }
                            "content_block_delta" => {
                                if let Some(delta) = &event.delta {
                                    // Handle thinking content
                                    if let Some(thinking) = &delta.thinking {
                                        let chunk = serde_json::json!({
                                            "id": format!("chatcmpl-{}", now),
                                            "object": "chat.completion.chunk",
                                            "created": now,
                                            "model": &model,
                                            "choices": [{
                                                "index": 0,
                                                "delta": {
                                                    "reasoning_content": thinking
                                                },
                                                "finish_reason": Value::Null
                                            }]
                                        });

                                        let sse = format!("data: {}\n\n", chunk);
                                        yield Ok(Bytes::from(sse));
                                    }

                                    // Handle regular text content
                                    if let Some(text) = &delta.text {
                                        let chunk = serde_json::json!({
                                            "id": format!("chatcmpl-{}", now),
                                            "object": "chat.completion.chunk",
                                            "created": now,
                                            "model": &model,
                                            "choices": [{
                                                "index": 0,
                                                "delta": {
                                                    "content": text
                                                },
                                                "finish_reason": Value::Null
                                            }]
                                        });

                                        let sse = format!("data: {}\n\n", chunk);
                                        yield Ok(Bytes::from(sse));
                                    }

                                    // Handle tool call arguments
                                    if let Some(partial_json) = &delta.partial_json {
                                        let chunk = serde_json::json!({
                                            "id": format!("chatcmpl-{}", now),
                                            "object": "chat.completion.chunk",
                                            "created": now,
                                            "model": &model,
                                            "choices": [{
                                                "index": 0,
                                                "delta": {
                                                    "tool_calls": [{
                                                        "index": tool_call_index,
                                                        "function": {
                                                            "arguments": partial_json
                                                        }
                                                    }]
                                                },
                                                "finish_reason": Value::Null
                                            }]
                                        });

                                        let sse = format!("data: {}\n\n", chunk);
                                        yield Ok(Bytes::from(sse));
                                    }
                                }
                            }
                            "content_block_stop" => {
                                if current_tool_call_id.is_some() {
                                    tool_call_index += 1;
                                    current_tool_call_id = None;
                                }
                            }
                            "message_delta" => {
                                if let Some(delta) = &event.delta
                                    && let Some(stop_reason) = &delta.stop_reason
                                {
                                    let finish_reason = map_stop_reason(stop_reason);

                                    let chunk = serde_json::json!({
                                        "id": format!("chatcmpl-{}", now),
                                        "object": "chat.completion.chunk",
                                        "created": now,
                                        "model": &model,
                                        "choices": [{
                                            "index": 0,
                                            "delta": {},
                                            "finish_reason": finish_reason
                                        }]
                                    });

                                    let sse = format!("data: {}\n\n", chunk);
                                    yield Ok(Bytes::from(sse));
                                }
                            }
                            "message_stop" => {
                                yield Ok(Bytes::from("data: [DONE]\n\n"));
                            }
                            _ => {}
                        }
                    }
                }

                // Keep-alive timer fired
                _ = keep_alive.tick() => {
                    yield Ok(Bytes::from(KEEP_ALIVE_COMMENT));
                }
            }
        }

        // Record usage after stream ends (per-model; global is derived via aggregation)
        let window_resets = crate::subscription::get_or_refresh_window_resets(&state).await;
        if let Err(e) = state.client_keys.record_model_usage(&key_id, &model, &usage_report, &window_resets).await {
            warn!("Failed to record streaming model usage for key {key_id}/{model}: {e}");
        }
    }
}

/// Strip mcp_ prefix from tool names in native Anthropic SSE stream with usage tracking.
///
/// This is used for the Anthropic-native endpoint to return clean tool names
/// to clients while preserving the SSE format.
/// Records token usage to the client keys store after the stream ends.
///
/// Includes keep-alive pings every 15 seconds to prevent connection timeouts.
pub fn stream_strip_mcp_prefix_with_usage(
    body: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
    state: Arc<AppState>,
    key_id: String,
    model: String,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send {
    use futures_util::StreamExt;

    stream! {
        let mut body = std::pin::pin!(body);
        let mut buffer = String::new();
        let mut keep_alive = interval(KEEP_ALIVE_INTERVAL);
        keep_alive.reset(); // Don't fire immediately
        let mut usage_report = Usage::default();

        loop {
            tokio::select! {
                biased; // Prefer data over keep-alive when both ready

                // Data chunk received
                chunk_opt = body.next() => {
                    let Some(chunk_result) = chunk_opt else {
                        break; // Stream ended
                    };

                    let chunk = match chunk_result {
                        Ok(c) => c,
                        Err(e) => {
                            yield Err(std::io::Error::other(e));
                            return;
                        }
                    };

                    let text = match std::str::from_utf8(&chunk) {
                        Ok(t) => t,
                        Err(_) => {
                            // Not valid UTF-8, pass through
                            yield Ok(chunk);
                            continue;
                        }
                    };

                    buffer.push_str(text);

                    // Process complete lines
                    let mut output = String::new();
                    while let Some(newline_pos) = buffer.find('\n') {
                        let line = &buffer[..=newline_pos];

                        // Try to extract usage from data lines
                        if line.starts_with("data: ") {
                            let data = &line[6..line.len()].trim();
                            if let Ok(event) = serde_json::from_str::<Value>(data) {
                                // Capture usage from message_start event
                                if event.get("type").and_then(|t| t.as_str()) == Some("message_start")
                                    && let Some(usage) = event
                                        .get("message")
                                        .and_then(|m| m.get("usage"))
                                {
                                    add_usage(&mut usage_report, &usage_from_json(usage));
                                }

                                // Capture usage from message_delta event
                                if event.get("type").and_then(|t| t.as_str()) == Some("message_delta")
                                    && let Some(usage) = event.get("usage")
                                {
                                    add_usage(&mut usage_report, &usage_from_json(usage));
                                }
                            }
                        }

                        // Check if this is an SSE data line with content_block_start
                        if line.starts_with("data: ") && line.contains("content_block_start") {
                            // Try to parse and transform the JSON
                            let data = &line[6..line.len()].trim();
                            if let Ok(mut event) = serde_json::from_str::<Value>(data) {
                                // Check for tool_use content block and strip mcp_ prefix
                                if let Some(content_block) = event.get_mut("content_block")
                                    && content_block.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                                    && let Some(name) = content_block.get("name").and_then(|n| n.as_str()).map(|s| s.to_string())
                                    && let Some(obj) = content_block.as_object_mut()
                                {
                                    obj.insert("name".to_string(), Value::String(strip_mcp_prefix(&name)));
                                }
                                output.push_str("data: ");
                                output.push_str(&serde_json::to_string(&event).unwrap_or_else(|_| data.to_string()));
                                output.push('\n');
                            } else {
                                output.push_str(line);
                            }
                        } else {
                            output.push_str(line);
                        }

                        buffer = buffer[newline_pos + 1..].to_string();
                    }

                    if !output.is_empty() {
                        yield Ok(Bytes::from(output));
                    }
                }

                // Keep-alive timer fired
                _ = keep_alive.tick() => {
                    yield Ok(Bytes::from(KEEP_ALIVE_COMMENT));
                }
            }
        }

        // Flush remaining buffer
        if !buffer.is_empty() {
            yield Ok(Bytes::from(buffer));
        }

        // Record usage after stream ends (per-model; global is derived via aggregation)
        let window_resets = crate::subscription::get_or_refresh_window_resets(&state).await;
        if let Err(e) = state.client_keys.record_model_usage(&key_id, &model, &usage_report, &window_resets).await {
            warn!("Failed to record streaming model usage for key {key_id}/{model}: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::usage::add_usage;
    use llm_relay::Usage;
    use llm_relay::convert::tool_names::strip_mcp_prefix;

    #[test]
    fn test_map_stop_reason() {
        assert_eq!(map_stop_reason("end_turn"), "stop");
        assert_eq!(map_stop_reason("tool_use"), "tool_calls");
        assert_eq!(map_stop_reason("max_tokens"), "length");
        assert_eq!(map_stop_reason("unknown"), "unknown");
    }

    #[test]
    fn test_parse_message_start_event() {
        let data = r#"{"type":"message_start","message":{"model":"claude-sonnet-4-5-20250514","usage":{"input_tokens":100,"output_tokens":0,"cache_read_input_tokens":50}}}"#;
        let event: StreamEvent = serde_json::from_str(data).unwrap();
        assert_eq!(event.event_type, "message_start");
        let msg = event.message.unwrap();
        let usage = msg.usage.unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.cache_read_input_tokens, Some(50));
    }

    #[test]
    fn test_parse_content_block_start_text() {
        let data =
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#;
        let event: StreamEvent = serde_json::from_str(data).unwrap();
        assert_eq!(event.event_type, "content_block_start");
        let block = event.content_block.unwrap();
        assert_eq!(block.block_type, "text");
        assert!(block.name.is_none());
    }

    #[test]
    fn test_parse_content_block_start_tool_use() {
        let data = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_123","name":"mcp_read_file"}}"#;
        let event: StreamEvent = serde_json::from_str(data).unwrap();
        let block = event.content_block.unwrap();
        assert_eq!(block.block_type, "tool_use");
        assert_eq!(block.id.as_deref(), Some("toolu_123"));
        assert_eq!(block.name.as_deref(), Some("mcp_read_file"));
    }

    #[test]
    fn test_parse_content_block_delta_text() {
        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        let event: StreamEvent = serde_json::from_str(data).unwrap();
        let delta = event.delta.unwrap();
        assert_eq!(delta.text.as_deref(), Some("Hello"));
        assert!(delta.thinking.is_none());
        assert!(delta.partial_json.is_none());
    }

    #[test]
    fn test_parse_content_block_delta_thinking() {
        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Let me think..."}}"#;
        let event: StreamEvent = serde_json::from_str(data).unwrap();
        let delta = event.delta.unwrap();
        assert_eq!(delta.thinking.as_deref(), Some("Let me think..."));
        assert!(delta.text.is_none());
    }

    #[test]
    fn test_parse_content_block_delta_partial_json() {
        let data = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"src/"}}"#;
        let event: StreamEvent = serde_json::from_str(data).unwrap();
        let delta = event.delta.unwrap();
        assert_eq!(delta.partial_json.as_deref(), Some("{\"path\":\"src/"));
    }

    #[test]
    fn test_parse_message_delta_with_stop_reason() {
        let data = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":0,"output_tokens":42}}"#;
        let event: StreamEvent = serde_json::from_str(data).unwrap();
        assert_eq!(event.event_type, "message_delta");
        let delta = event.delta.unwrap();
        assert_eq!(delta.stop_reason.as_deref(), Some("end_turn"));
        let usage = event.usage.unwrap();
        assert_eq!(usage.output_tokens, 42);
    }

    #[test]
    fn test_parse_message_stop() {
        let data = r#"{"type":"message_stop"}"#;
        let event: StreamEvent = serde_json::from_str(data).unwrap();
        assert_eq!(event.event_type, "message_stop");
    }

    #[test]
    fn test_usage_accumulation_from_stream() {
        let mut usage_report = Usage::default();

        // message_start with input tokens
        let start_data = r#"{"type":"message_start","message":{"model":"claude-sonnet-4-5-20250514","usage":{"input_tokens":150,"output_tokens":0,"cache_read_input_tokens":80,"cache_creation_input_tokens":20}}}"#;
        let start_event: StreamEvent = serde_json::from_str(start_data).unwrap();
        if let Some(msg) = &start_event.message
            && let Some(usage) = &msg.usage
        {
            add_usage(&mut usage_report, usage);
        }

        // message_delta with output tokens
        let delta_data = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":0,"output_tokens":75}}"#;
        let delta_event: StreamEvent = serde_json::from_str(delta_data).unwrap();
        if let Some(usage) = &delta_event.usage {
            add_usage(&mut usage_report, usage);
        }

        assert_eq!(usage_report.input_tokens, 150);
        assert_eq!(usage_report.output_tokens, 75);
        assert_eq!(usage_report.cache_read_input_tokens, Some(80));
        assert_eq!(usage_report.cache_creation_input_tokens, Some(20));
    }

    #[test]
    fn test_mcp_prefix_stripping_in_tool_name() {
        let data = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_abc","name":"mcp_read_file"}}"#;
        let event: StreamEvent = serde_json::from_str(data).unwrap();
        let block = event.content_block.unwrap();
        let stripped = block.name.as_ref().map(|n| strip_mcp_prefix(n));
        assert_eq!(stripped.as_deref(), Some("read_file"));
    }

    #[test]
    fn test_mcp_prefix_not_stripped_when_absent() {
        let data = r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_abc","name":"my_tool"}}"#;
        let event: StreamEvent = serde_json::from_str(data).unwrap();
        let block = event.content_block.unwrap();
        let stripped = block.name.as_ref().map(|n| strip_mcp_prefix(n));
        assert_eq!(stripped.as_deref(), Some("my_tool"));
    }

    #[test]
    fn test_sse_data_line_extraction() {
        let line = "data: {\"type\":\"message_stop\"}";
        assert!(line.starts_with("data: "));
        let data = &line[6..];
        let event: StreamEvent = serde_json::from_str(data).unwrap();
        assert_eq!(event.event_type, "message_stop");
    }

    #[test]
    fn test_non_data_lines_skipped() {
        assert!(!"event: message_start".starts_with("data: "));
        assert!(!"".starts_with("data: "));
        assert!(!"id: 123".starts_with("data: "));
        assert!(!": comment".starts_with("data: "));
    }

    #[test]
    fn test_done_sentinel() {
        let line = "data: [DONE]";
        let data = &line[6..];
        assert_eq!(data, "[DONE]");
    }
}

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

use super::tool_names::strip_mcp_prefix;
use crate::auth::{ClientKeysStore, StreamUsageData, TokenUsageReport};

/// Keep-alive interval for SSE streams (prevents proxy/load balancer timeouts).
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(15);

/// SSE keep-alive comment (ignored by clients but keeps connection alive).
const KEEP_ALIVE_COMMENT: &str = ": keep-alive\n\n";

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

/// Alias for usage data from streaming events (uses centralized type)
type StreamUsage = StreamUsageData;

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
    client_keys: Arc<ClientKeysStore>,
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
        let mut usage_report = TokenUsageReport::new();

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
                            usage_report.add(&TokenUsageReport::from_stream_usage(usage));
                        }

                        // Capture usage from message_delta event (output tokens)
                        if event.event_type == "message_delta"
                            && let Some(usage) = &event.usage
                        {
                            usage_report.add(&TokenUsageReport::from_stream_usage(usage));
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
                                    let finish_reason = match stop_reason.as_str() {
                                        "end_turn" => "stop",
                                        "tool_use" => "tool_calls",
                                        "max_tokens" => "length",
                                        other => other,
                                    };

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

        // Record usage after stream ends
        let weighted_total = usage_report.weighted_total();
        if weighted_total > 0 {
            let _ = client_keys.record_usage(&key_id, weighted_total).await;
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
    client_keys: Arc<ClientKeysStore>,
    key_id: String,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send {
    use futures_util::StreamExt;

    stream! {
        let mut body = std::pin::pin!(body);
        let mut buffer = String::new();
        let mut keep_alive = interval(KEEP_ALIVE_INTERVAL);
        keep_alive.reset(); // Don't fire immediately
        let mut usage_report = TokenUsageReport::new();

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
                                    usage_report.add(&TokenUsageReport::from_json(usage));
                                }

                                // Capture usage from message_delta event
                                if event.get("type").and_then(|t| t.as_str()) == Some("message_delta")
                                    && let Some(usage) = event.get("usage")
                                {
                                    usage_report.add(&TokenUsageReport::from_json(usage));
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

        // Record usage after stream ends
        let weighted_total = usage_report.weighted_total();
        if weighted_total > 0 {
            let _ = client_keys.record_usage(&key_id, weighted_total).await;
        }
    }
}

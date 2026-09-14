//! SSE stream transformations.
//!
//! This module provides:
//! - `stream_anthropic_to_openai_with_usage`: Convert Anthropic SSE to OpenAI SSE format with usage tracking
//! - `stream_restore_native_tool_names_with_usage`: Restore native Anthropic SSE tool names with usage tracking
//!
//! Both functions include keep-alive pings to prevent connection timeouts
//! during long-running requests (e.g., extended thinking).

use async_stream::stream;
use bytes::Bytes;
use eventsource_stream::Eventsource;
use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::{Value, from_str, json};
use std::io::Error as IoError;
use std::pin::pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::{select, time::interval};
use tracing::warn;

use llm_relay::Usage;
#[cfg(test)]
use llm_relay::anthropic::tool_names::strip_mcp_prefix;

use crate::AppState;
use crate::transforms::tool_aliases::ToolNameMap;

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

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
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
    #[allow(dead_code)]
    message: Option<MessageInfo>,
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
/// Records the last reported usage on completion, error, or response cancellation.
///
/// Includes keep-alive pings every 15 seconds to prevent connection timeouts.
pub fn stream_anthropic_to_openai_with_usage(
    body: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
    model: String,
    state: Arc<AppState>,
    key_id: String,
    tool_name_map: ToolNameMap,
) -> impl Stream<Item = Result<Bytes, IoError>> + Send {
    let events = tracked_events(body, recorder(state, key_id, model.clone()));
    openai_events(events, model, tool_name_map)
}

fn openai_events(
    events: impl Stream<Item = Result<Value, IoError>> + Send,
    model: String,
    tool_name_map: ToolNameMap,
) -> impl Stream<Item = Result<Bytes, IoError>> + Send {
    stream! {
        let now = now_secs();
        let mut current_tool_call_id: Option<String> = None;
        let mut tool_call_index: u32 = 0;
        let mut events = pin!(events);
        let mut keep_alive = interval(KEEP_ALIVE_INTERVAL);
        keep_alive.reset();
        loop {
            select! {
                biased;
                next = events.next() => {
                    let Some(next) = next else { break; };
                    let value = match next { Ok(value) => value, Err(e) => { yield Err(e); return; } };
                    if value.get("type").and_then(Value::as_str) == Some("error") {
                        yield Ok(Bytes::from(format!("data: {}\n\n", json!({"error": value.get("error")}))));
                        return;
                    }
                    let event: StreamEvent = match serde_json::from_value(value) {
                        Ok(event) => event,
                        Err(e) => { yield Err(IoError::other(e)); return; }
                    };
                        match event.event_type.as_str() {
                            "content_block_start" => {
                                if let Some(block) = &event.content_block
                                    && block.block_type == "tool_use"
                                {
                                    current_tool_call_id = block.id.clone();
                                    let name = block.name.as_ref().map(|n| tool_name_map.restore(n));

                                    let chunk = json!({
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
                                        let chunk = json!({
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
                                        let chunk = json!({
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
                                        let chunk = json!({
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
                            "content_block_stop" if current_tool_call_id.is_some() => {
                                    tool_call_index += 1;
                                    current_tool_call_id = None;
                            }
                            "message_delta" => {
                                if let Some(delta) = &event.delta
                                    && let Some(stop_reason) = &delta.stop_reason
                                {
                                    let finish_reason = map_stop_reason(stop_reason);

                                    let chunk = json!({
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
                                return;
                            }
                            _ => {}
                        }

                }
                _ = keep_alive.tick() => { yield Ok(Bytes::from(KEEP_ALIVE_COMMENT)); }
            }
        }
    }
}

pub fn stream_restore_native_tool_names_with_usage(
    body: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
    state: Arc<AppState>,
    key_id: String,
    model: String,
    tool_name_map: ToolNameMap,
) -> impl Stream<Item = Result<Bytes, IoError>> + Send {
    native_events(
        tracked_events(body, recorder(state, key_id, model)),
        tool_name_map,
    )
}

fn native_events(
    events: impl Stream<Item = Result<Value, IoError>> + Send,
    tool_name_map: ToolNameMap,
) -> impl Stream<Item = Result<Bytes, IoError>> + Send {
    stream! {
        let mut events = pin!(events);
        let mut keep_alive = interval(KEEP_ALIVE_INTERVAL);
        keep_alive.reset();
        loop {
            select! {
                biased;
                next = events.next() => {
                    let Some(next) = next else { break; };
                    let mut event = match next { Ok(event) => event, Err(e) => { yield Err(e); return; } };
                    if event.get("type").and_then(Value::as_str) == Some("content_block_start")
                        && let Some(block) = event.get_mut("content_block")
                        && block.get("type").and_then(Value::as_str) == Some("tool_use")
                        && let Some(name) = block.get("name").and_then(Value::as_str).map(|name| tool_name_map.restore(name))
                        && let Some(block) = block.as_object_mut()
                    {
                        block.insert("name".into(), json!(name));
                    }
                    let kind = event.get("type").and_then(Value::as_str).unwrap_or("message");
                    let terminal = matches!(kind, "message_stop" | "error");
                    yield Ok(Bytes::from(format!("event: {kind}\ndata: {event}\n\n")));
                    if terminal { return; }
                }
                _ = keep_alive.tick() => { yield Ok(Bytes::from(KEEP_ALIVE_COMMENT)); }
            }
        }
    }
}

// Owns the last reported cumulative usage even when the response body is dropped.
// The callback is taken before invocation, so all exit paths record at most once.
struct UsageFinalizer<F: FnOnce(Usage)> {
    usage: Usage,
    finish: Option<F>,
}

impl<F: FnOnce(Usage)> UsageFinalizer<F> {
    fn finish(&mut self) {
        if let Some(finish) = self.finish.take() {
            finish(self.usage.clone());
        }
    }

    fn observe(&mut self, event: &Value) {
        let usage = match event.get("type").and_then(Value::as_str) {
            Some("message_start") => event.pointer("/message/usage").unwrap_or(&Value::Null),
            Some("message_delta") => event.get("usage").unwrap_or(&Value::Null),
            _ => return,
        };
        // Delta usage fields are cumulative; missing fields retain their last value.
        if let Some(n) = usage.get("input_tokens").and_then(Value::as_u64) {
            self.usage.input_tokens = n;
        }
        if let Some(n) = usage.get("output_tokens").and_then(Value::as_u64) {
            self.usage.output_tokens = n;
        }
        if let Some(n) = usage.get("cache_read_input_tokens").and_then(Value::as_u64) {
            self.usage.cache_read_input_tokens = Some(n);
        }
        if let Some(n) = usage
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64)
        {
            self.usage.cache_creation_input_tokens = Some(n);
        }
    }
}

impl<F: FnOnce(Usage)> Drop for UsageFinalizer<F> {
    fn drop(&mut self) {
        self.finish();
    }
}

fn recorder(state: Arc<AppState>, key_id: String, model: String) -> impl FnOnce(Usage) + Send {
    move |usage| {
        // Detached from response cancellation, but bounded by the application's runtime.
        tokio::spawn(async move {
            let resets = state.usage_cache.snapshot().await.window_state();
            if let Err(e) = state
                .client_keys
                .record_model_usage(&key_id, &model, &usage, &resets)
                .await
            {
                warn!("Failed to record streaming usage for {key_id}/{model}: {e}");
            }
        });
    }
}

fn tracked_events<E: std::fmt::Display + Send + 'static>(
    body: impl Stream<Item = Result<Bytes, E>> + Send,
    finish: impl FnOnce(Usage) + Send,
) -> impl Stream<Item = Result<Value, IoError>> + Send {
    let finalizer = UsageFinalizer {
        usage: Usage::default(),
        finish: Some(finish),
    };
    stream! {
        let mut usage = finalizer;
        let mut events = pin!(body.eventsource());
        while let Some(event) = events.next().await {
            let event = match event {
                Ok(event) => event,
                Err(e) => { usage.finish(); yield Err(IoError::other(e.to_string())); return; }
            };
            let value: Value = match from_str(&event.data) {
                Ok(value) => value,
                Err(e) => { usage.finish(); yield Err(IoError::other(e)); return; }
            };
            let terminal = matches!(value.get("type").and_then(Value::as_str), Some("message_stop" | "error"));
            usage.observe(&value);
            if terminal { usage.finish(); }
            yield Ok(value);
            if terminal { return; }
        }
        usage.finish();
        yield Err(IoError::new(std::io::ErrorKind::UnexpectedEof, "Upstream stream ended before message_stop"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let event: StreamEvent = from_str(data).unwrap();
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
        let event: StreamEvent = from_str(data).unwrap();
        assert_eq!(event.event_type, "content_block_start");
        let block = event.content_block.unwrap();
        assert_eq!(block.block_type, "text");
        assert!(block.name.is_none());
    }

    #[test]
    fn test_parse_content_block_start_tool_use() {
        let data = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_123","name":"mcp_read_file"}}"#;
        let event: StreamEvent = from_str(data).unwrap();
        let block = event.content_block.unwrap();
        assert_eq!(block.block_type, "tool_use");
        assert_eq!(block.id.as_deref(), Some("toolu_123"));
        assert_eq!(block.name.as_deref(), Some("mcp_read_file"));
    }

    #[test]
    fn test_parse_content_block_delta_text() {
        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        let event: StreamEvent = from_str(data).unwrap();
        let delta = event.delta.unwrap();
        assert_eq!(delta.text.as_deref(), Some("Hello"));
        assert!(delta.thinking.is_none());
        assert!(delta.partial_json.is_none());
    }

    #[test]
    fn test_parse_content_block_delta_thinking() {
        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Let me think..."}}"#;
        let event: StreamEvent = from_str(data).unwrap();
        let delta = event.delta.unwrap();
        assert_eq!(delta.thinking.as_deref(), Some("Let me think..."));
        assert!(delta.text.is_none());
    }

    #[test]
    fn test_parse_content_block_delta_partial_json() {
        let data = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"src/"}}"#;
        let event: StreamEvent = from_str(data).unwrap();
        let delta = event.delta.unwrap();
        assert_eq!(delta.partial_json.as_deref(), Some("{\"path\":\"src/"));
    }

    #[test]
    fn test_parse_message_delta_with_stop_reason() {
        let data = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":0,"output_tokens":42}}"#;
        let event: StreamEvent = from_str(data).unwrap();
        assert_eq!(event.event_type, "message_delta");
        let delta = event.delta.unwrap();
        assert_eq!(delta.stop_reason.as_deref(), Some("end_turn"));
        let usage = event.usage.unwrap();
        assert_eq!(usage.output_tokens, 42);
    }

    #[test]
    fn test_parse_message_stop() {
        let data = r#"{"type":"message_stop"}"#;
        let event: StreamEvent = from_str(data).unwrap();
        assert_eq!(event.event_type, "message_stop");
    }

    #[test]
    fn test_mcp_prefix_stripping_in_tool_name() {
        let data = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_abc","name":"mcp_read_file"}}"#;
        let event: StreamEvent = from_str(data).unwrap();
        let block = event.content_block.unwrap();
        let stripped = block.name.as_ref().map(|n| strip_mcp_prefix(n));
        assert_eq!(stripped.as_deref(), Some("read_file"));
    }

    #[test]
    fn test_mcp_prefix_not_stripped_when_absent() {
        let data = r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_abc","name":"my_tool"}}"#;
        let event: StreamEvent = from_str(data).unwrap();
        let block = event.content_block.unwrap();
        let stripped = block.name.as_ref().map(|n| strip_mcp_prefix(n));
        assert_eq!(stripped.as_deref(), Some("my_tool"));
    }

    #[test]
    fn test_sse_data_line_extraction() {
        let line = "data: {\"type\":\"message_stop\"}";
        assert!(line.starts_with("data: "));
        let data = line.strip_prefix("data: ").unwrap();
        let event: StreamEvent = from_str(data).unwrap();
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
        let data = line.strip_prefix("data: ").unwrap();
        assert_eq!(data, "[DONE]");
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    use futures_util::stream;
    use std::sync::Mutex;

    fn input(chunks: Vec<Bytes>) -> impl Stream<Item = Result<Bytes, IoError>> {
        stream::iter(chunks.into_iter().map(Ok))
    }

    fn observed() -> (Arc<Mutex<Vec<Usage>>>, impl FnOnce(Usage) + Send) {
        let reports = Arc::new(Mutex::new(Vec::new()));
        let copy = reports.clone();
        (reports, move |usage| copy.lock().unwrap().push(usage))
    }

    const FIXTURE: &str = concat!(
        ": heartbeat\r\n\r\n",
        "event: message_start\r\ndata:{\"type\":\"message_start\",\r\ndata: \"message\":{\"usage\":{\"input_tokens\":12,\"output_tokens\":1}}}\r\n\r\n",
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Привет 🌍\"}}\n\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":7}}\n\n",
        "data: {\"type\":\"message_stop\"}\n\n"
    );

    #[tokio::test]
    async fn both_formats_preserve_every_network_split() {
        for native in [false, true] {
            let mut expected = None;
            for split in 0..=FIXTURE.len() {
                let (a, b) = FIXTURE.as_bytes().split_at(split);
                let (reports, finish) = observed();
                let events = tracked_events(
                    input(vec![Bytes::copy_from_slice(a), Bytes::copy_from_slice(b)]),
                    finish,
                );
                let output: Vec<_> = if native {
                    native_events(events, ToolNameMap::default())
                        .collect()
                        .await
                } else {
                    openai_events(events, "model".into(), ToolNameMap::default())
                        .collect()
                        .await
                };
                let mut text = String::new();
                for chunk in output {
                    text.push_str(std::str::from_utf8(&chunk.unwrap()).unwrap());
                }
                // Generated timestamps are irrelevant to the framing regression.
                let content = text.contains("Привет 🌍");
                assert!(content, "split {split}, native={native}: {text}");
                let reports = reports.lock().unwrap();
                assert_eq!(reports.len(), 1);
                assert_eq!(reports[0].input_tokens, 12);
                assert_eq!(reports[0].output_tokens, 7);
                let count = text.matches("Привет 🌍").count();
                if let Some(expected) = expected {
                    assert_eq!(count, expected);
                }
                expected = Some(count);
            }
        }
    }

    #[tokio::test]
    async fn cancellation_records_observed_usage_once() {
        let (reports, finish) = observed();
        let body = input(vec![Bytes::from_static(
            b"data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":12}}}\n\n",
        )])
        .chain(stream::pending());
        let mut events = Box::pin(tracked_events(body, finish));
        assert!(events.next().await.unwrap().is_ok());
        drop(events);
        let reports = reports.lock().unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].input_tokens, 12);
    }

    #[tokio::test]
    async fn truncated_and_failed_streams_are_errors_and_record_once() {
        for transport_error in [false, true] {
            let (reports, finish) = observed();
            let mut chunks = vec![Ok(Bytes::from_static(b"data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":12}}}\n\n"))];
            if transport_error {
                chunks.push(Err(IoError::other("broken connection")));
            }
            let output: Vec<_> = tracked_events(stream::iter(chunks), finish).collect().await;
            assert!(output.last().unwrap().is_err());
            assert_eq!(reports.lock().unwrap().len(), 1);
            assert_eq!(reports.lock().unwrap()[0].input_tokens, 12);
        }
    }

    #[tokio::test]
    async fn upstream_error_is_forwarded_without_success_marker() {
        for native in [false, true] {
            let (reports, finish) = observed();
            let events = tracked_events(input(vec![Bytes::from_static(b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"busy\"}}\n\n")]), finish);
            let chunks: Vec<_> = if native {
                native_events(events, ToolNameMap::default())
                    .collect()
                    .await
            } else {
                openai_events(events, "model".into(), ToolNameMap::default())
                    .collect()
                    .await
            };
            let text = chunks
                .into_iter()
                .map(|c| String::from_utf8(c.unwrap().to_vec()).unwrap())
                .collect::<String>();
            assert!(text.contains("overloaded_error"));
            assert!(!text.contains("[DONE]"));
            assert!(!text.contains("message_stop"));
            assert_eq!(reports.lock().unwrap().len(), 1);
        }
    }
    #[tokio::test]
    async fn tool_alias_round_trip_matches_both_response_formats() {
        let prepared = crate::transforms::prepare_anthropic_request(
            json!({
                "model": "claude-sonnet-4-6",
                "tools": [{"name": "read_file", "input_schema": {"type": "object"}}],
                "tool_choice": {"type": "tool", "name": "read_file"},
                "messages": [{"role": "assistant", "content": [{"type": "tool_use", "id": "call", "name": "read_file", "input": {}}]}]
            }),
            true,
        );
        let upstream_name = prepared.body["tools"][0]["name"].as_str().unwrap();
        assert_eq!(prepared.body["tool_choice"]["name"], upstream_name);
        assert_eq!(
            prepared.body["messages"][0]["content"][0]["name"],
            upstream_name
        );
        let event = json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "call", "name": upstream_name}});
        for native in [false, true] {
            let events = stream::iter(vec![Ok(event.clone()), Ok(json!({"type":"message_stop"}))]);
            let chunks: Vec<_> = if native {
                native_events(events, prepared.tool_name_map.clone())
                    .collect()
                    .await
            } else {
                openai_events(events, "model".into(), prepared.tool_name_map.clone())
                    .collect()
                    .await
            };
            let text = chunks
                .into_iter()
                .map(|c| String::from_utf8(c.unwrap().to_vec()).unwrap())
                .collect::<String>();
            assert!(text.contains("\"name\":\"read_file\""), "{text}");
        }
        let response: llm_relay::wire::anthropic::MessagesResponse = serde_json::from_value(json!({
            "id": "msg", "type": "message", "role": "assistant", "model": "model",
            "content": [{"type":"tool_use", "id":"call", "name":upstream_name, "input":{}}], "stop_reason":"tool_use"
        })).unwrap();
        let response =
            crate::transforms::transform_openai_response(response, &prepared.tool_name_map)
                .unwrap();
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(
            value["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "read_file"
        );
    }
    #[tokio::test]
    async fn dropping_either_response_format_finalizes_usage() {
        for native in [false, true] {
            let (reports, finish) = observed();
            let prefix = concat!(
                "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":12}}}\n\n",
                "data: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"hello\"}}\n\n"
            );
            let events = tracked_events(
                input(vec![Bytes::from_static(prefix.as_bytes())]).chain(stream::pending()),
                finish,
            );
            if native {
                let mut response = Box::pin(native_events(events, ToolNameMap::default()));
                assert!(response.next().await.unwrap().is_ok());
                drop(response);
            } else {
                let mut response = Box::pin(openai_events(
                    events,
                    "model".into(),
                    ToolNameMap::default(),
                ));
                assert!(response.next().await.unwrap().is_ok());
                drop(response);
            }
            assert_eq!(reports.lock().unwrap().len(), 1);
            assert_eq!(reports.lock().unwrap()[0].input_tokens, 12);
        }
    }

    #[tokio::test]
    async fn malformed_event_fails_instead_of_silently_disappearing() {
        let (reports, finish) = observed();
        let output: Vec<_> = tracked_events(
            input(vec![Bytes::from_static(b"data: {broken}\n\n")]),
            finish,
        )
        .collect()
        .await;
        assert_eq!(output.len(), 1);
        assert!(output[0].is_err());
        assert_eq!(reports.lock().unwrap().len(), 1);
    }
}

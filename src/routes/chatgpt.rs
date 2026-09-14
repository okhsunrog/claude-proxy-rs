//! Single-account GPT inference over the subscription Responses endpoint.
// JSON reads return Null for missing fields; writes below target validated or constructed objects.
#![allow(clippy::indexing_slicing)]
use crate::{AppState, error::ProxyError, transforms::responses as compat};
use axum::{
    Json,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use eventsource_stream::Eventsource;
use futures_util::{Stream, StreamExt};
use llm_relay::protocol::events::Decoded;
use llm_relay::protocol::{Decoder, Encoder, Event, Policy, Protocol};
use serde_json::{Value, json};
use std::{io, pin::pin, sync::Arc, time::Duration};

const URL: &str = "https://chatgpt.com/backend-api/codex/responses";
pub fn is_model(model: &str) -> bool {
    model.starts_with("gpt-")
        || model.starts_with("codex-")
        || ["o1", "o3", "o4"]
            .iter()
            .any(|p| model == *p || model.starts_with(&format!("{p}-")))
}
fn error(protocol: Protocol, status: StatusCode, message: impl Into<String>) -> Response {
    (
        status,
        Json(if protocol == Protocol::Messages {
            json!({"type":"error","error":{"type":"api_error","message":message.into()}})
        } else {
            json!({"error":{"type":"upstream_error","message":message.into()}})
        }),
    )
        .into_response()
}

pub async fn responses(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    execute(state, headers, body, Protocol::Responses).await
}
pub async fn chat_completions(state: Arc<AppState>, headers: HeaderMap, body: Value) -> Response {
    execute(state, headers, body, Protocol::ChatCompletions).await
}
pub async fn messages(state: Arc<AppState>, headers: HeaderMap, body: Value) -> Response {
    execute(state, headers, body, Protocol::Messages).await
}
async fn execute(
    state: Arc<AppState>,
    headers: HeaderMap,
    body: Value,
    protocol: Protocol,
) -> Response {
    let Some(model) = body
        .get("model")
        .and_then(Value::as_str)
        .filter(|m| is_model(m))
    else {
        return error(
            protocol,
            StatusCode::BAD_REQUEST,
            "Specify a registered GPT model",
        );
    };
    let model = model.to_owned();
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));
    let Some(key) = (if protocol == Protocol::Messages {
        headers
            .get("x-api-key")
            .and_then(|h| h.to_str().ok())
            .or(bearer)
    } else {
        bearer
    }) else {
        return if protocol == Protocol::Messages {
            ProxyError::InvalidApiKey.to_anthropic_response()
        } else {
            ProxyError::InvalidApiKey.to_openai_response()
        };
    };
    let client_key = match super::auth::validate_inference_key(key, &state, &model, true).await {
        Ok(k) => k,
        Err(e) => {
            return if protocol == Protocol::Messages {
                e.to_anthropic_response()
            } else {
                e.to_openai_response()
            };
        }
    };
    let streaming = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let include_usage = body
        .pointer("/stream_options/include_usage")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let capture = crate::capture::Capture::begin(
        &state.capture,
        match protocol {
            Protocol::Messages => "anthropic",
            Protocol::ChatCompletions => "openai",
            Protocol::Responses => "responses",
        },
        match protocol {
            Protocol::Messages => "/v1/messages",
            Protocol::ChatCompletions => "/v1/chat/completions",
            Protocol::Responses => "/v1/responses",
        },
        &model,
        streaming,
        &headers,
        &body,
    )
    .await;
    let policy = match headers
        .get("x-proxy-compatibility")
        .and_then(|h| h.to_str().ok())
    {
        Some("strict") => Policy::Strict,
        Some("compatible") => Policy::Compatible,
        None if protocol == Protocol::Responses => Policy::Strict,
        None => Policy::Compatible,
        Some(_) => {
            return error(
                protocol,
                StatusCode::BAD_REQUEST,
                "x-proxy-compatibility must be strict or compatible",
            );
        }
    };
    let translated = match compat::prepare_request(protocol, &body, policy) {
        Ok(t) => t,
        Err(e) => return error(protocol, StatusCode::BAD_REQUEST, e),
    };
    let warning = translated
        .diagnostics
        .iter()
        .map(|d| d.field.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    if !warning.is_empty() {
        tracing::info!(fields=%warning,"Applied explicit compatibility policy");
    }
    let mut prepared = translated.body;
    if protocol == Protocol::Messages {
        prepared["include"] = json!(["reasoning.encrypted_content"]);
    }
    // Keep cache routing separate for independently authorized client keys.
    let cache_key = prepared
        .get("prompt_cache_key")
        .and_then(Value::as_str)
        .unwrap_or("");
    prepared["prompt_cache_key"] = json!(format!("{}:{cache_key}", client_key.id));
    if let Some(capture) = &capture {
        capture.write_prepared(&prepared, &[], false).await;
    }
    let (mut token, mut account) = match state.chatgpt.credentials(None).await {
        Ok(c) => c,
        Err(e) => return error(protocol, StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let mut response = None;
    for attempt in 0..2 {
        let mut request = state
            .http_client
            .post(URL)
            .bearer_auth(&token)
            .header("originator", "codex_cli_rs")
            .header(header::USER_AGENT, "codex_cli_rs/0.114.0")
            .header(header::ACCEPT, "text/event-stream")
            .header("session_id", uuid::Uuid::new_v4().to_string())
            .timeout(Duration::from_secs(600))
            .json(&prepared);
        if let Some(account) = &account {
            request = request.header("ChatGPT-Account-Id", account);
        }
        let upstream = match request.send().await {
            Ok(r) => r,
            Err(_error) => {
                return error(
                    protocol,
                    StatusCode::BAD_GATEWAY,
                    "Cannot reach ChatGPT inference service",
                );
            }
        };
        if upstream.status() == StatusCode::UNAUTHORIZED && attempt == 0 {
            (token, account) = match state.chatgpt.credentials(Some(&token)).await {
                Ok(c) => c,
                Err(e) => return error(protocol, StatusCode::SERVICE_UNAVAILABLE, e),
            };
            continue;
        }
        if !upstream.status().is_success() {
            let status = upstream.status();
            // Do not expose arbitrary upstream bodies or credential diagnostics.
            let message = match status {
                StatusCode::BAD_REQUEST => "ChatGPT rejected the model or request parameters",
                StatusCode::TOO_MANY_REQUESTS => "ChatGPT subscription rate limit reached",
                _ => "ChatGPT inference request failed",
            };
            return error(
                protocol,
                if status.is_server_error() || status == StatusCode::UNAUTHORIZED {
                    StatusCode::BAD_GATEWAY
                } else {
                    status
                },
                message,
            );
        }
        response = Some(upstream);
        break;
    }
    let Some(response) = response else {
        return error(
            protocol,
            StatusCode::BAD_GATEWAY,
            "ChatGPT authentication failed",
        );
    };
    if let Some(capture) = &capture {
        capture
            .write_upstream_response(response.status(), response.headers())
            .await;
    }
    let body_stream = crate::capture::capture_byte_stream(
        response.bytes_stream(),
        capture.as_ref().map(|c| c.upstream_stream_path()),
    );
    let key_id = client_key.id;
    let tracked_model = model.clone();
    let events = tracked_events(body_stream, move |usage| {
        tokio::spawn(async move {
            let resets = state.usage_cache.snapshot().await.window_state();
            if let Err(e) = state
                .client_keys
                .record_model_usage(&key_id, &tracked_model, &usage, &resets)
                .await
            {
                tracing::warn!("Cannot record GPT usage: {e}");
            }
        });
    });
    if streaming {
        let stream = wire_stream(events, Encoder::new(protocol, model, include_usage));
        let mut response = (
            [
                (header::CONTENT_TYPE, "text/event-stream"),
                (header::CACHE_CONTROL, "no-cache"),
                (header::HeaderName::from_static("x-accel-buffering"), "no"),
            ],
            Body::from_stream(stream),
        )
            .into_response();
        attach_warning(&mut response, &warning);
        return response;
    }
    let mut events = pin!(events);
    while let Some(event) = events.next().await {
        match event {
            Ok(Decoded {
                event: Event::Finish(completion),
                ..
            }) => {
                let mut response = match completion.render(protocol) {
                    Ok(body) => Json(body).into_response(),
                    Err(e) => error(protocol, StatusCode::BAD_GATEWAY, e),
                };
                attach_warning(&mut response, &warning);
                return response;
            }
            Ok(_) => {}
            Err(e) => return error(protocol, StatusCode::BAD_GATEWAY, e.to_string()),
        }
    }
    error(
        protocol,
        StatusCode::BAD_GATEWAY,
        "ChatGPT stream ended without a response",
    )
}

struct UsageGuard<F: FnOnce(llm_relay::Usage)> {
    usage: Option<llm_relay::Usage>,
    finish: Option<F>,
}
impl<F: FnOnce(llm_relay::Usage)> Drop for UsageGuard<F> {
    fn drop(&mut self) {
        if let (Some(usage), Some(finish)) = (self.usage.take(), self.finish.take()) {
            finish(usage);
        }
    }
}
fn attach_warning(response: &mut Response, warning: &str) {
    if !warning.is_empty()
        && let Ok(value) = warning.parse()
    {
        response
            .headers_mut()
            .insert("x-proxy-compatibility-warnings", value);
    }
}
fn tracked_events<E: std::fmt::Display + Send + 'static>(
    body: impl Stream<Item = Result<Bytes, E>> + Send,
    finish: impl FnOnce(llm_relay::Usage) + Send,
) -> impl Stream<Item = Result<Decoded, io::Error>> + Send {
    let guard = UsageGuard {
        usage: None,
        finish: Some(finish),
    };
    async_stream::stream! {
        let mut guard=guard;
        let mut decoder=Decoder::default();
        let mut events=pin!(body.eventsource());
        loop {
            let next=tokio::time::timeout(Duration::from_secs(90),events.next()).await;
            let event=match next {
                Ok(Some(Ok(e)))=>e,
                Ok(None)=>{yield Err(io::Error::other("ChatGPT stream ended before completion"));return;},
                Ok(Some(Err(_error)))=>{yield Err(io::Error::other("Invalid or interrupted ChatGPT event stream"));return;},
                Err(_error)=>{yield Err(io::Error::other("ChatGPT stream timed out"));return;},
            };
            let raw:Value=match serde_json::from_str(&event.data) { Ok(v)=>v,Err(_error)=>{yield Err(io::Error::other("Invalid ChatGPT event JSON"));return;} };
            let decoded=decoder.decode(raw);
            guard.usage=decoder.usage().cloned();
            match decoded {
                Ok(decoded)=> {let terminal=matches!(decoded.event,Event::Finish(_));yield Ok(decoded);if terminal{return;}},
                Err(e)=>{yield Err(io::Error::other(e));return;},
            }
        }
    }
}

fn wire_stream(
    events: impl Stream<Item = Result<Decoded, io::Error>> + Send,
    mut encoder: Encoder,
) -> impl Stream<Item = Result<Bytes, io::Error>> + Send {
    async_stream::stream! {
        let mut events=pin!(events);
        let mut keepalive=tokio::time::interval(Duration::from_secs(15));keepalive.tick().await;
        loop {
            tokio::select! { biased;
                next=events.next()=>match next {
                    Some(Ok(decoded))=> {
                        let terminal=matches!(decoded.event,Event::Finish(_));
                        match encoder.encode(&decoded) {
                            Ok(frames)=>for frame in frames {yield Ok(Bytes::from(frame.sse()));},
                            Err(e)=>{yield Ok(Bytes::from(encoder.error(&e).sse()));return;},
                        }
                        if terminal {if encoder.needs_done_sentinel(){yield Ok(Bytes::from_static(b"data: [DONE]\n\n"));}return;}
                    },
                    Some(Err(e))=>{yield Ok(Bytes::from(encoder.error(&e.to_string()).sse()));return;},
                    None=>return,
                },
                _=keepalive.tick()=>yield Ok(Bytes::from_static(b": keep-alive\n\n")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    #[tokio::test]
    async fn fragmented_utf8_and_terminal_usage_survive_cancellation() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let callback = seen.clone();
        let data = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Привет\",\"output_index\":0,\"content_index\":0}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"output\":[],\"usage\":{\"input_tokens\":100,\"input_tokens_details\":{\"cached_tokens\":80},\"output_tokens\":2}}}\n\n";
        let chunks: Vec<Result<Bytes, io::Error>> = data
            .as_bytes()
            .iter()
            .map(|b| Ok(Bytes::copy_from_slice(&[*b])))
            .collect();
        let mut stream = Box::pin(tracked_events(
            futures_util::stream::iter(chunks),
            move |u| callback.lock().unwrap().push(u),
        ));
        assert_eq!(
            stream.next().await.unwrap().unwrap().native["delta"],
            "Привет"
        );
        assert_eq!(
            stream.next().await.unwrap().unwrap().native["type"],
            "response.completed"
        );
        drop(stream);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen.first().unwrap().input_tokens, 20);
    }
    #[tokio::test]
    async fn truncated_stream_is_an_error_without_success_sentinel() {
        let body=futures_util::stream::iter([Ok::<_,io::Error>(Bytes::from_static(b"data: {\"type\":\"response.created\",\"response\":{\"id\":\"r\",\"created_at\":1}}\n\n"))]);
        let chunks: Vec<_> = wire_stream(
            tracked_events(body, |_| {}),
            Encoder::new(Protocol::ChatCompletions, "gpt-test".into(), false),
        )
        .collect()
        .await;
        let text = chunks
            .into_iter()
            .map(|c| String::from_utf8(c.unwrap().to_vec()).unwrap())
            .collect::<String>();
        assert!(text.contains("event: error"));
        assert!(!text.contains("[DONE]"));
    }
    #[tokio::test]
    async fn failed_generation_records_reported_usage_once() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let callback = seen.clone();
        let body=futures_util::stream::iter([Ok::<_,io::Error>(Bytes::from_static(b"data: {\"type\":\"response.failed\",\"response\":{\"usage\":{\"input_tokens\":3,\"output_tokens\":1}}}\n\n"))]);
        let events: Vec<_> = tracked_events(body, move |u| callback.lock().unwrap().push(u))
            .collect()
            .await;
        assert!(events.first().unwrap().is_err());
        assert_eq!(seen.lock().unwrap().len(), 1);
    }
}

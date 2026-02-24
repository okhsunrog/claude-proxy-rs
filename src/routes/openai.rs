use axum::{
    Json,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::sync::Arc;
use tracing::warn;

use llm_relay::MessagesResponse;
use llm_relay::types::openai::InboundChatRequest;

use crate::AppState;
use crate::constants::ANTHROPIC_API_URL;
use crate::error::ProxyError;
use crate::transforms::{
    prepare_anthropic_request, stream_anthropic_to_openai_with_usage, transform_openai_request,
    transform_openai_response,
};

use super::auth::{authenticate_openai, build_anthropic_request};

pub async fn list_models(State(state): State<Arc<AppState>>) -> Response {
    let model_ids = match state.models.list_enabled_ids().await {
        Ok(ids) => ids,
        Err(e) => {
            return ProxyError::DatabaseError(e.to_string()).to_openai_response();
        }
    };
    let models: Vec<Value> = model_ids
        .iter()
        .map(|id| {
            json!({
                "id": id,
                "object": "model",
                "owned_by": "anthropic"
            })
        })
        .collect();

    Json(json!({
        "object": "list",
        "data": models
    }))
    .into_response()
}

pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<InboundChatRequest>,
) -> Response {
    // Extract model before auth so we can validate it
    let model_name = body
        .model
        .as_deref()
        .unwrap_or("claude-sonnet-4-5")
        .to_string();

    // Parse model suffix (e.g., "claude-sonnet-4-5(high)" -> base model)
    let base_model = model_name
        .find('(')
        .map(|i| &model_name[..i])
        .unwrap_or(&model_name);

    let auth = match authenticate_openai(&headers, &state, base_model).await {
        Ok(a) => a,
        Err(err) => return err.to_openai_response(),
    };

    let cloak = state.should_cloak(headers.get("user-agent").and_then(|v| v.to_str().ok()));

    let stream = body.stream.unwrap_or(false);
    let anthropic_value = transform_openai_request(body);
    let model = anthropic_value
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    let prepared = prepare_anthropic_request(anthropic_value, cloak);

    let req_builder = build_anthropic_request(
        &state.http_client,
        ANTHROPIC_API_URL,
        &auth.token,
        Some(&prepared.betas),
        stream,
    );

    let response: reqwest::Response = match req_builder.json(&prepared.body).send().await {
        Ok(r) => r,
        Err(e) => {
            return ProxyError::AnthropicApiError(format!("Failed to contact Anthropic: {}", e))
                .to_openai_response();
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let text: String = response.text().await.unwrap_or_default();
        return (
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            Json(json!({ "error": text })),
        )
            .into_response();
    }

    if stream {
        let body_stream = response.bytes_stream();
        let key_id = auth.client_key.id.clone();
        let sse_stream =
            stream_anthropic_to_openai_with_usage(body_stream, model, state.clone(), key_id);

        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .header(header::CONNECTION, "keep-alive")
            .body(Body::from_stream(sse_stream))
            .unwrap()
    } else {
        let anthropic_response = match response.json::<MessagesResponse>().await {
            Ok(r) => r,
            Err(e) => {
                return ProxyError::ParseError(format!("Failed to parse response: {}", e))
                    .to_openai_response();
            }
        };

        // Record token usage (per-model; global is derived via aggregation)
        let usage_report = anthropic_response.usage.clone().unwrap_or_default();
        let window_resets = crate::subscription::get_or_refresh_window_resets(&state).await;

        if let Err(e) = state
            .client_keys
            .record_model_usage(&auth.client_key.id, &model, &usage_report, &window_resets)
            .await
        {
            warn!(
                "Failed to record model usage for key {}/{model}: {e}",
                auth.client_key.id
            );
        }

        let openai_response = transform_openai_response(anthropic_response);
        Json(openai_response).into_response()
    }
}

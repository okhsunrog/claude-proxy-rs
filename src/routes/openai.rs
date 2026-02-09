use axum::{
    Json,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::sync::Arc;

use crate::AppState;
use crate::auth::TokenUsageReport;
use crate::constants::{ANTHROPIC_API_URL, MODELS};
use crate::error::ProxyError;
use crate::transforms::{
    AnthropicResponse, OpenAIChatRequest, prepare_anthropic_request,
    stream_anthropic_to_openai_with_usage, transform_openai_request, transform_openai_response,
};

use super::auth::{authenticate_openai, build_anthropic_request};

pub async fn list_models() -> Json<Value> {
    let models: Vec<Value> = MODELS
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
}

pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<OpenAIChatRequest>,
) -> Response {
    let auth = match authenticate_openai(&headers, &state).await {
        Ok(a) => a,
        Err(err) => return err.to_openai_response(),
    };

    let stream = body.stream.unwrap_or(false);
    let anthropic_value = transform_openai_request(body);
    let model = anthropic_value
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    let prepared = prepare_anthropic_request(anthropic_value);

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
        let client_keys = Arc::clone(&state.client_keys);
        let key_id = auth.client_key.id.clone();
        let sse_stream =
            stream_anthropic_to_openai_with_usage(body_stream, model, client_keys, key_id);

        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .header(header::CONNECTION, "keep-alive")
            .body(Body::from_stream(sse_stream))
            .unwrap()
    } else {
        let anthropic_response = match response.json::<AnthropicResponse>().await {
            Ok(r) => r,
            Err(e) => {
                return ProxyError::ParseError(format!("Failed to parse response: {}", e))
                    .to_openai_response();
            }
        };

        // Record token usage
        let usage_report = TokenUsageReport::from_anthropic_usage(&anthropic_response.usage);
        let _ = state
            .client_keys
            .record_usage(&auth.client_key.id, usage_report.weighted_total())
            .await;

        let openai_response = transform_openai_response(anthropic_response);
        Json(openai_response).into_response()
    }
}

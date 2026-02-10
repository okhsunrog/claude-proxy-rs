use axum::{
    Json,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::Value;
use std::sync::Arc;

use crate::AppState;
use crate::auth::TokenUsageReport;
use crate::constants::{ANTHROPIC_API_URL, ANTHROPIC_COUNT_TOKENS_URL};
use crate::error::ProxyError;
use crate::transforms::tool_names::transform_response_tool_names;
use crate::transforms::{
    prepare_anthropic_request, prepare_count_tokens_request, stream_strip_mcp_prefix_with_usage,
};

use super::auth::{authenticate_anthropic, build_anthropic_request};

pub async fn messages(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let model = body
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("claude-sonnet-4-5");

    let auth = match authenticate_anthropic(&headers, &state, model).await {
        Ok(a) => a,
        Err(err) => return err.to_anthropic_response(),
    };

    let cloak = state.should_cloak(headers.get("user-agent").and_then(|v| v.to_str().ok()));
    let model = model.to_string();

    let stream = body
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Apply all transformations via unified pipeline
    let prepared = prepare_anthropic_request(body, cloak);

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
                .to_anthropic_response();
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let text: String = response.text().await.unwrap_or_default();
        return (
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            text,
        )
            .into_response();
    }

    if stream {
        let body_stream = response.bytes_stream();
        let key_id = auth.client_key.id.clone();
        // Transform stream to strip mcp_ prefix from tool names and track usage
        let transformed_stream = stream_strip_mcp_prefix_with_usage(
            body_stream,
            state.clone(),
            key_id,
            model,
            auth.model_pricing,
        );

        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .header(header::CONNECTION, "keep-alive")
            .body(Body::from_stream(transformed_stream))
            .unwrap()
    } else {
        let mut json_response = match response.json::<Value>().await {
            Ok(r) => r,
            Err(e) => {
                return ProxyError::ParseError(format!("Failed to parse response: {}", e))
                    .to_anthropic_response();
            }
        };

        // Record token usage
        if let Some(usage) = json_response.get("usage") {
            let usage_report = TokenUsageReport::from_json(usage);
            let window_resets = crate::routes::admin::get_or_refresh_window_resets(&state).await;

            // Global usage (cost in microdollars)
            let cost = usage_report.cost_microdollars(&auth.model_pricing);
            if let Err(e) = state
                .client_keys
                .record_usage(&auth.client_key.id, cost, &window_resets)
                .await
            {
                tracing::warn!("Failed to record usage for key {}: {e}", auth.client_key.id);
            }

            // Per-model usage (raw tokens)
            if let Err(e) = state
                .client_keys
                .record_model_usage(&auth.client_key.id, &model, &usage_report, &window_resets)
                .await
            {
                tracing::warn!(
                    "Failed to record model usage for key {}/{model}: {e}",
                    auth.client_key.id
                );
            }
        }

        // Strip mcp_ prefix from tool names in response
        transform_response_tool_names(&mut json_response);
        Json(json_response).into_response()
    }
}

pub async fn count_tokens(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let model = body
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("claude-sonnet-4-5");

    let auth = match authenticate_anthropic(&headers, &state, model).await {
        Ok(a) => a,
        Err(err) => return err.to_anthropic_response(),
    };

    let cloak = state.should_cloak(headers.get("user-agent").and_then(|v| v.to_str().ok()));

    // Apply lighter transformations for count_tokens (no metadata/tools support)
    let prepared = prepare_count_tokens_request(body, cloak);

    let req_builder = build_anthropic_request(
        &state.http_client,
        ANTHROPIC_COUNT_TOKENS_URL,
        &auth.token,
        Some(&prepared.betas),
        false, // count_tokens is never streaming
    );

    let response: reqwest::Response = match req_builder.json(&prepared.body).send().await {
        Ok(r) => r,
        Err(e) => {
            return ProxyError::AnthropicApiError(format!("Failed to contact Anthropic: {}", e))
                .to_anthropic_response();
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let text: String = response.text().await.unwrap_or_default();
        return (
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            text,
        )
            .into_response();
    }

    let json_response: Value = match response.json::<Value>().await {
        Ok(r) => r,
        Err(e) => {
            return ProxyError::ParseError(format!("Failed to parse response: {}", e))
                .to_anthropic_response();
        }
    };

    Json(json_response).into_response()
}

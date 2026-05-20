use axum::{
    Json,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::Value;
use std::sync::Arc;
use tracing::{debug, info, warn};

use llm_relay::convert::tool_names::transform_response_tool_names;

use crate::AppState;
use crate::auth::usage::usage_from_json;
use crate::capture::{Capture, capture_byte_stream};
use crate::constants::{ANTHROPIC_API_URL, ANTHROPIC_COUNT_TOKENS_URL};
use crate::error::ProxyError;
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
    let capture = Capture::begin(
        &state.capture,
        "anthropic",
        "/v1/messages",
        &model,
        stream,
        &headers,
        &body,
    )
    .await;

    // Apply all transformations via unified pipeline
    let prepared = prepare_anthropic_request(body, cloak);
    if let Some(capture) = &capture {
        capture
            .write_prepared(&prepared.body, &prepared.betas, cloak)
            .await;
    }

    // Log outgoing request body keys for debugging
    if let Some(obj) = prepared.body.as_object() {
        let keys: Vec<&String> = obj.keys().collect();
        debug!(model = %model, stream = %stream, "Forwarding to Anthropic with body keys: {keys:?}");
    }

    let req_builder = build_anthropic_request(
        &state.http_client,
        ANTHROPIC_API_URL,
        &auth.token,
        Some(&prepared.betas),
        stream,
        &state.session_id,
    );

    let response: reqwest::Response = match req_builder.json(&prepared.body).send().await {
        Ok(r) => r,
        Err(e) => {
            return ProxyError::AnthropicApiError(format!("Failed to contact Anthropic: {}", e))
                .to_anthropic_response();
        }
    };

    // On 401, force-refresh the OAuth token and retry once. This handles server-side
    // token revocation (e.g. password change) without waiting for local expiry.
    let response = if response.status() == StatusCode::UNAUTHORIZED {
        info!("Anthropic returned 401, force-refreshing OAuth token and retrying");
        let new_token = match state.oauth.force_refresh().await {
            Ok(Some(t)) => t,
            Ok(None) => {
                return ProxyError::NoAuthConfigured.to_anthropic_response();
            }
            Err(e) => {
                return ProxyError::OAuthError(e).to_anthropic_response();
            }
        };
        let retry_builder = build_anthropic_request(
            &state.http_client,
            ANTHROPIC_API_URL,
            &new_token,
            Some(&prepared.betas),
            stream,
            &state.session_id,
        );
        match retry_builder.json(&prepared.body).send().await {
            Ok(r) => r,
            Err(e) => {
                return ProxyError::AnthropicApiError(format!(
                    "Failed to contact Anthropic on retry: {}",
                    e
                ))
                .to_anthropic_response();
            }
        }
    } else {
        response
    };

    if !response.status().is_success() {
        let status = response.status();
        if let Some(capture) = &capture {
            capture
                .write_upstream_response(status, response.headers())
                .await;
        }
        let text: String = response.text().await.unwrap_or_default();
        if let Some(capture) = &capture {
            capture.write_upstream_body(&text).await;
        }
        warn!(
            status = %status, model = %model,
            "Anthropic API error: {text}"
        );
        return (
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            text,
        )
            .into_response();
    }

    // Update window resets from rate-limit headers on every successful response.
    state
        .usage_cache
        .patch_from_headers(response.headers())
        .await;
    if let Some(capture) = &capture {
        capture
            .write_upstream_response(response.status(), response.headers())
            .await;
    }

    if stream {
        let body_stream = capture_byte_stream(
            response.bytes_stream(),
            capture.as_ref().map(|c| c.upstream_stream_path()),
        );
        let key_id = auth.client_key.id.clone();
        // Transform stream to strip mcp_ prefix from tool names and track usage
        let transformed_stream =
            stream_strip_mcp_prefix_with_usage(body_stream, state.clone(), key_id, model);

        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .header(header::CONNECTION, "keep-alive")
            .body(Body::from_stream(transformed_stream))
            .unwrap()
    } else {
        let text = match response.text().await {
            Ok(text) => text,
            Err(e) => {
                return ProxyError::ParseError(format!("Failed to read response: {}", e))
                    .to_anthropic_response();
            }
        };
        if let Some(capture) = &capture {
            capture.write_upstream_body(&text).await;
        }

        let mut json_response = match serde_json::from_str::<Value>(&text) {
            Ok(r) => r,
            Err(e) => {
                return ProxyError::ParseError(format!("Failed to parse response: {}", e))
                    .to_anthropic_response();
            }
        };

        // Record token usage (per-model; global is derived via aggregation)
        if let Some(usage) = json_response.get("usage") {
            let usage_report = usage_from_json(usage);
            let window_resets = state.usage_cache.snapshot().await.window_state();

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
    let capture = Capture::begin(
        &state.capture,
        "anthropic",
        "/v1/messages/count_tokens",
        model,
        false,
        &headers,
        &body,
    )
    .await;

    // Apply lighter transformations for count_tokens (no metadata/tools support)
    let prepared = prepare_count_tokens_request(body, cloak);
    if let Some(capture) = &capture {
        capture
            .write_prepared(&prepared.body, &prepared.betas, cloak)
            .await;
    }

    let req_builder = build_anthropic_request(
        &state.http_client,
        ANTHROPIC_COUNT_TOKENS_URL,
        &auth.token,
        Some(&prepared.betas),
        false, // count_tokens is never streaming
        &state.session_id,
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
        if let Some(capture) = &capture {
            capture
                .write_upstream_response(status, response.headers())
                .await;
        }
        let text: String = response.text().await.unwrap_or_default();
        if let Some(capture) = &capture {
            capture.write_upstream_body(&text).await;
        }
        return (
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            text,
        )
            .into_response();
    }

    if let Some(capture) = &capture {
        capture
            .write_upstream_response(response.status(), response.headers())
            .await;
    }
    let text = match response.text().await {
        Ok(text) => text,
        Err(e) => {
            return ProxyError::ParseError(format!("Failed to read response: {}", e))
                .to_anthropic_response();
        }
    };
    if let Some(capture) = &capture {
        capture.write_upstream_body(&text).await;
    }

    let json_response: Value = match serde_json::from_str(&text) {
        Ok(r) => r,
        Err(e) => {
            return ProxyError::ParseError(format!("Failed to parse response: {}", e))
                .to_anthropic_response();
        }
    };

    Json(json_response).into_response()
}

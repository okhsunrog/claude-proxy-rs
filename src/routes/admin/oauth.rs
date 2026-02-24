use axum::{Json, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::warn;
use utoipa::ToSchema;

use super::{ErrorResponse, SuccessResponse};
use crate::AppState;
use crate::constants::{ANTHROPIC_USAGE_URL, ANTHROPIC_VERSION, OAUTH_BETA_HEADER, USER_AGENT};
use crate::subscription::{SubscriptionUsageResponse, extract_subscription_state, fetch_plan_name};

// --- Types ---

#[derive(Serialize, ToSchema)]
pub struct OAuthUrlResponse {
    pub url: String,
}

#[derive(Serialize, ToSchema)]
pub struct OAuthStatusResponse {
    pub authenticated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct ExchangeCodeRequest {
    code: String,
}

// --- Handlers ---

/// Get OAuth connection status
#[utoipa::path(
    get,
    path = "/oauth/status",
    tag = "oauth",
    responses(
        (status = 200, body = OAuthStatusResponse),
    )
)]
pub async fn get_oauth_status(State(state): State<Arc<AppState>>) -> Json<OAuthStatusResponse> {
    let authenticated = state.auth_store.has("anthropic").await.unwrap_or(false);
    let plan = if authenticated {
        fetch_plan_name(&state).await
    } else {
        None
    };
    Json(OAuthStatusResponse {
        authenticated,
        plan,
    })
}

/// Start OAuth flow
#[utoipa::path(
    post,
    path = "/oauth/start-flow",
    tag = "oauth",
    responses(
        (status = 200, body = OAuthUrlResponse),
    )
)]
pub async fn start_oauth_flow(State(state): State<Arc<AppState>>) -> Json<OAuthUrlResponse> {
    let url = state.oauth.start_flow().await;
    Json(OAuthUrlResponse { url })
}

/// Exchange OAuth code
#[utoipa::path(
    post,
    path = "/oauth/exchange",
    tag = "oauth",
    request_body = ExchangeCodeRequest,
    responses(
        (status = 200, body = SuccessResponse),
        (status = 400, body = ErrorResponse),
    )
)]
pub async fn exchange_oauth_code(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ExchangeCodeRequest>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.oauth.exchange_code(&body.code).await {
        Ok(_) => Ok(Json(SuccessResponse { success: true })),
        Err(e) => Err((StatusCode::BAD_REQUEST, Json(ErrorResponse { error: e }))),
    }
}

/// Delete OAuth credentials
#[utoipa::path(
    delete,
    path = "/oauth",
    tag = "oauth",
    responses(
        (status = 200, body = SuccessResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn delete_oauth(
    State(state): State<Arc<AppState>>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.oauth.logout().await {
        Ok(_) => Ok(Json(SuccessResponse { success: true })),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

/// Get Claude subscription usage from Anthropic API
#[utoipa::path(
    get,
    path = "/oauth/usage",
    tag = "oauth",
    responses(
        (status = 200, body = SubscriptionUsageResponse),
        (status = 401, body = ErrorResponse),
        (status = 502, body = ErrorResponse),
    )
)]
pub async fn get_subscription_usage(
    State(state): State<Arc<AppState>>,
) -> Result<Json<SubscriptionUsageResponse>, (StatusCode, Json<ErrorResponse>)> {
    let token = state
        .oauth
        .refresh_if_needed()
        .await
        .map_err(|e| {
            (
                StatusCode::UNAUTHORIZED,
                Json(ErrorResponse {
                    error: format!("OAuth error: {e}"),
                }),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(ErrorResponse {
                    error: "Not authenticated".into(),
                }),
            )
        })?;

    let resp = state
        .http_client
        .get(ANTHROPIC_USAGE_URL)
        .header("authorization", format!("Bearer {token}"))
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("anthropic-beta", OAUTH_BETA_HEADER)
        .header("content-type", "application/json")
        .header("user-agent", USER_AGENT)
        .header("accept", "application/json")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| {
            warn!("Failed to contact Anthropic usage API: {e}");
            (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error: format!("Failed to contact Anthropic: {e}"),
                }),
            )
        })?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        warn!("Anthropic usage API returned {status}: {body}");
        return Err((
            StatusCode::BAD_GATEWAY,
            Json(ErrorResponse {
                error: format!("Anthropic returned {status}: {body}"),
            }),
        ));
    }

    let usage: SubscriptionUsageResponse = serde_json::from_str(&body).map_err(|e| {
        warn!("Failed to parse usage response: {e}");
        (
            StatusCode::BAD_GATEWAY,
            Json(ErrorResponse {
                error: format!("Failed to parse usage response: {e}"),
            }),
        )
    })?;

    // Update window resets cache as side effect
    let resets = extract_subscription_state(&usage);
    if resets.five_hour_reset_at.is_some() {
        *state.window_resets.write().await = resets;
    }

    Ok(Json(usage))
}

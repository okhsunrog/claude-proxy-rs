use axum::{Json, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use super::{ErrorResponse, SuccessResponse};
use crate::AppState;
use crate::subscription::fetch_plan_name;
use crate::usage::{SubscriptionUsageResponse, WEB_SESSION_PROVIDER};

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

#[derive(Deserialize, Serialize, ToSchema)]
pub struct WebSessionRequest {
    pub session_key: String,
    pub org_uuid: String,
    pub device_id: String,
    pub anonymous_id: String,
}

#[derive(Serialize, ToSchema)]
pub struct WebSessionStatusResponse {
    pub configured: bool,
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
        Ok(_) => {
            // Fresh OAuth session — invalidate any cached usage from a
            // previous identity and trigger a fetch under the new token.
            state.usage_cache.invalidate().await;
            state.usage_cache.force_refresh(&state).await;
            Ok(Json(SuccessResponse { success: true }))
        }
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
        Ok(_) => {
            state.usage_cache.invalidate().await;
            Ok(Json(SuccessResponse { success: true }))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

/// Get Claude subscription usage.
///
/// Thin wrapper around [`UsageCache::get_or_refresh`]: reads the current
/// cached state, triggers an opportunistic refresh if the data is stale,
/// and returns the snapshot with freshness metadata attached.
#[utoipa::path(
    get,
    path = "/oauth/usage",
    tag = "oauth",
    responses(
        (status = 200, body = SubscriptionUsageResponse),
    )
)]
pub async fn get_subscription_usage(
    State(state): State<Arc<AppState>>,
) -> Json<SubscriptionUsageResponse> {
    let cached = state.usage_cache.get_or_refresh(&state).await;
    Json(cached.to_response())
}

/// Get web session configuration status
#[utoipa::path(
    get,
    path = "/oauth/web-session",
    tag = "oauth",
    responses((status = 200, body = WebSessionStatusResponse))
)]
pub async fn get_web_session_status(
    State(state): State<Arc<AppState>>,
) -> Json<WebSessionStatusResponse> {
    let configured = state
        .auth_store
        .has(WEB_SESSION_PROVIDER)
        .await
        .unwrap_or(false);
    Json(WebSessionStatusResponse { configured })
}

/// Save claude.ai web session credentials (used to bypass rate limits on the
/// OAuth usage endpoint). The sessionKey is auto-rotated on every successful
/// request via the Set-Cookie header.
#[utoipa::path(
    post,
    path = "/oauth/web-session",
    tag = "oauth",
    request_body = WebSessionRequest,
    responses(
        (status = 200, body = SuccessResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn save_web_session(
    State(state): State<Arc<AppState>>,
    Json(body): Json<WebSessionRequest>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    use crate::auth::storage::Auth;

    state
        .auth_store
        .set(
            WEB_SESSION_PROVIDER,
            Auth::WebSession {
                session_key: body.session_key,
                org_uuid: body.org_uuid,
                device_id: body.device_id,
                anonymous_id: body.anonymous_id,
            },
        )
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    // Re-fetch immediately under the new credentials so the admin UI
    // shows validation feedback without waiting for the throttle.
    state.usage_cache.force_refresh(&state).await;

    Ok(Json(SuccessResponse { success: true }))
}

/// Delete claude.ai web session credentials
#[utoipa::path(
    delete,
    path = "/oauth/web-session",
    tag = "oauth",
    responses(
        (status = 200, body = SuccessResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn delete_web_session(
    State(state): State<Arc<AppState>>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    state
        .auth_store
        .remove(WEB_SESSION_PROVIDER)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    // Fall back to the OAuth path immediately so the admin UI sees a
    // fresh (or newly-errored) state without waiting for the throttle.
    state.usage_cache.force_refresh(&state).await;

    Ok(Json(SuccessResponse { success: true }))
}

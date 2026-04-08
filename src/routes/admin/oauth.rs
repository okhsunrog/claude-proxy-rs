use axum::{Json, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::warn;
use utoipa::ToSchema;

use super::{ErrorResponse, SuccessResponse};
use crate::AppState;
use crate::constants::{ANTHROPIC_USAGE_URL, ANTHROPIC_VERSION, OAUTH_USAGE_BETA, USER_AGENT};
use crate::subscription::{
    SubscriptionState, SubscriptionUsageResponse, UsageLimit, extract_subscription_state,
    fetch_plan_name,
};

/// Build a partial SubscriptionUsageResponse from cached window_resets state.
/// Used as fallback when Anthropic's usage API is rate-limited but we have
/// fresh data from inference response headers.
fn usage_from_window_resets(resets: &SubscriptionState) -> SubscriptionUsageResponse {
    let to_iso = |epoch_ms: u64| -> Option<String> {
        chrono::DateTime::from_timestamp_millis(epoch_ms as i64).map(|dt| dt.to_rfc3339())
    };

    SubscriptionUsageResponse {
        five_hour: resets.five_hour_reset_at.map(|ts| UsageLimit {
            utilization: resets.five_hour_utilization,
            resets_at: to_iso(ts),
        }),
        seven_day: resets.seven_day_reset_at.map(|ts| UsageLimit {
            utilization: resets.seven_day_utilization,
            resets_at: to_iso(ts),
        }),
        seven_day_oauth_apps: None,
        seven_day_opus: None,
        seven_day_sonnet: None,
        extra_usage: None,
        is_stale: true,
        upstream_error: None,
    }
}

/// Mark a cached usage response as stale and attach an upstream error.
fn stale_with_error(
    mut usage: SubscriptionUsageResponse,
    err: String,
) -> SubscriptionUsageResponse {
    usage.is_stale = true;
    usage.upstream_error = Some(err);
    usage
}

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
    use crate::subscription::timestamp_millis;

    const CACHE_TTL_MS: u64 = 5 * 60 * 1000; // 5 minutes

    // Return cached response if it's still fresh.
    // Invalidate when: TTL expired OR the 5-hour window has reset since we cached.
    {
        let cached = state.cached_usage.read().await;
        if let Some((ref usage, cached_at)) = *cached {
            let now = timestamp_millis();
            let resets = state.window_resets.read().await.clone();
            let window_reset_since_cache =
                resets.five_hour_reset_at.is_some_and(|r| r > cached_at && r <= now);
            if now - cached_at < CACHE_TTL_MS && !window_reset_since_cache {
                // Patch utilization from window_resets (updated from every
                // /v1/messages response headers) so the UI sees near-realtime
                // changes instead of the snapshot from the last usage fetch.
                let mut fresh = usage.clone();
                if let (Some(fh), Some(util)) =
                    (fresh.five_hour.as_mut(), resets.five_hour_utilization)
                {
                    fh.utilization = Some(util);
                }
                if let (Some(sd), Some(util)) =
                    (fresh.seven_day.as_mut(), resets.seven_day_utilization)
                {
                    sd.utilization = Some(util);
                }
                return Ok(Json(fresh));
            }
        }
    }

    // Try the claude.ai web session endpoint first — it has the same
    // response shape but is not aggressively rate-limited like the OAuth
    // one (which 429s constantly — see fetch_usage_via_web_session docs
    // for the full story). Fall back to the OAuth endpoint if no web
    // session is configured or the request fails.
    match crate::subscription::fetch_usage_via_web_session(&state).await {
        Ok(usage) => {
            tracing::info!("Fetched subscription usage via claude.ai web session");
            let resets = extract_subscription_state(&usage);
            if resets.five_hour_reset_at.is_some() {
                *state.window_resets.write().await = resets;
            }
            *state.cached_usage.write().await = Some((usage.clone(), timestamp_millis()));
            return Ok(Json(usage));
        }
        Err(e) if e == "no web session configured" => {
            // fall through to OAuth path silently
        }
        Err(e) => {
            warn!("Web session usage fetch failed, falling back to OAuth: {e}");
        }
    }

    let token = match state.oauth.refresh_if_needed().await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(ErrorResponse {
                    error: "Not authenticated".into(),
                }),
            ));
        }
        Err(e) => {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(ErrorResponse {
                    error: format!("OAuth error: {e}"),
                }),
            ));
        }
    };

    let resp = state
        .http_client
        .get(ANTHROPIC_USAGE_URL)
        .header("authorization", format!("Bearer {token}"))
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("anthropic-beta", OAUTH_USAGE_BETA)
        .header("content-type", "application/json")
        .header("user-agent", USER_AGENT)
        .header("accept", "application/json")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await;

    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            let err_msg = format!("network error contacting Anthropic usage API: {e}");
            warn!("{err_msg}");
            let cached = state.cached_usage.read().await;
            if let Some((ref usage, _)) = *cached {
                return Ok(Json(stale_with_error(usage.clone(), err_msg)));
            }
            let mut fallback = usage_from_window_resets(&*state.window_resets.read().await);
            fallback.upstream_error = Some(err_msg);
            return Ok(Json(fallback));
        }
    };

    let status = resp.status();

    // On rate limit or server error, return stale cache or window_resets fallback.
    if !status.is_success() {
        let headers_dump: Vec<String> = resp
            .headers()
            .iter()
            .map(|(k, v)| format!("{}={}", k, v.to_str().unwrap_or("<binary>")))
            .collect();
        let body = resp.text().await.unwrap_or_default();
        let err_msg = format!("Anthropic usage API returned {status}: {body}");
        warn!("{err_msg} | headers: [{}]", headers_dump.join(", "));
        let cached = state.cached_usage.read().await;
        if let Some((ref usage, _)) = *cached {
            return Ok(Json(stale_with_error(usage.clone(), err_msg)));
        }
        let mut fallback = usage_from_window_resets(&*state.window_resets.read().await);
        fallback.upstream_error = Some(err_msg);
        return Ok(Json(fallback));
    }

    let body = resp.text().await.unwrap_or_default();
    let usage: SubscriptionUsageResponse = match serde_json::from_str(&body) {
        Ok(u) => u,
        Err(e) => {
            let err_msg = format!("failed to parse usage response: {e}");
            warn!("{err_msg}");
            let cached = state.cached_usage.read().await;
            if let Some((ref usage, _)) = *cached {
                return Ok(Json(stale_with_error(usage.clone(), err_msg)));
            }
            let mut fallback = usage_from_window_resets(&*state.window_resets.read().await);
            fallback.upstream_error = Some(err_msg);
            return Ok(Json(fallback));
        }
    };

    // Update caches.
    let resets = extract_subscription_state(&usage);
    if resets.five_hour_reset_at.is_some() {
        *state.window_resets.write().await = resets;
    }
    *state.cached_usage.write().await = Some((usage.clone(), timestamp_millis()));

    Ok(Json(usage))
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
        .has(crate::subscription::WEB_SESSION_PROVIDER)
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
            crate::subscription::WEB_SESSION_PROVIDER,
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

    // Invalidate cached usage so the next GET /oauth/usage hits the new web endpoint.
    *state.cached_usage.write().await = None;

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
        .remove(crate::subscription::WEB_SESSION_PROVIDER)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;
    *state.cached_usage.write().await = None;
    Ok(Json(SuccessResponse { success: true }))
}

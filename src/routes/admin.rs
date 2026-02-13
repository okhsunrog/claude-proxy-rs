use axum::{
    Json, Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use subtle::ConstantTimeEq;
use utoipa::ToSchema;

use crate::auth::{ClientKey, Model, ModelUsageEntry, TokenLimits, TokenUsage, UsageResetType};
use crate::constants::{
    ANTHROPIC_PROFILE_URL, ANTHROPIC_USAGE_URL, ANTHROPIC_VERSION, OAUTH_BETA_HEADER, USER_AGENT,
};
use crate::parse_cookie;
use crate::{AppState, SESSION_TTL_SECS, SubscriptionState, now_secs};

// --- Response types ---

#[derive(Serialize, ToSchema)]
pub struct OAuthUrlResponse {
    pub url: String,
}

#[derive(Serialize, ToSchema)]
pub struct SuccessResponse {
    pub success: bool,
}

#[derive(Serialize, ToSchema)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Serialize, ToSchema)]
pub struct OAuthStatusResponse {
    pub authenticated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct UsageLimit {
    pub utilization: Option<f64>,
    pub resets_at: Option<String>,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ExtraUsage {
    pub is_enabled: bool,
    pub monthly_limit: Option<f64>,
    pub used_credits: Option<f64>,
    pub utilization: Option<f64>,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct SubscriptionUsageResponse {
    pub five_hour: Option<UsageLimit>,
    pub seven_day: Option<UsageLimit>,
    pub seven_day_opus: Option<UsageLimit>,
    pub seven_day_sonnet: Option<UsageLimit>,
    pub extra_usage: Option<ExtraUsage>,
}

#[derive(Serialize, ToSchema)]
pub struct CreateKeyResponse {
    pub key: String,
    pub id: String,
}

#[derive(Serialize, ToSchema)]
pub struct ListKeysResponse {
    pub keys: Vec<ClientKey>,
}

#[derive(Serialize, ToSchema)]
pub struct KeyUsageResponse {
    pub limits: TokenLimits,
    pub usage: TokenUsage,
}

// --- Auth types ---

#[derive(Deserialize, Serialize, ToSchema)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuthCheckResponse {
    pub authenticated: bool,
    pub auth_required: bool,
}

// --- Request types ---

#[derive(Deserialize, Serialize, ToSchema)]
pub struct ExchangeCodeRequest {
    code: String,
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct CreateKeyRequest {
    name: String,
}

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateLimitsRequest {
    #[serde(rename = "fiveHourLimit", alias = "hourlyLimit")]
    five_hour_limit: Option<u64>,
    weekly_limit: Option<u64>,
    total_limit: Option<u64>,
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct SetKeyEnabledRequest {
    enabled: bool,
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct SetAllowExtraUsageRequest {
    allow_extra_usage: bool,
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct ResetUsageRequest {
    /// Which counter to reset: "hourly", "weekly", "total", or "all"
    #[serde(rename = "type")]
    reset_type: String,
}

// --- Model types ---

#[derive(Serialize, ToSchema)]
pub struct ListModelsResponse {
    pub models: Vec<Model>,
}

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AddModelRequest {
    pub id: String,
    #[serde(default)]
    pub input_price: f64,
    #[serde(default)]
    pub output_price: f64,
    #[serde(default)]
    pub cache_read_price: f64,
    #[serde(default)]
    pub cache_write_price: f64,
}

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateModelRequest {
    pub enabled: Option<bool>,
    pub input_price: Option<f64>,
    pub output_price: Option<f64>,
    pub cache_read_price: Option<f64>,
    pub cache_write_price: Option<f64>,
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct ReorderModelsRequest {
    pub ids: Vec<String>,
}

// --- Per-key model types ---

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyModelsResponse {
    pub allow_all: bool,
    pub models: Vec<String>,
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct SetKeyModelsRequest {
    pub models: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct KeyModelUsageResponse {
    pub entries: Vec<ModelUsageEntry>,
}

// --- Static file serving ---

pub fn static_routes() -> Router<Arc<AppState>> {
    memory_serve::load!()
        .index_file(Some("/index.html"))
        .fallback(Some("/index.html"))
        .into_router()
}

// --- Auth handlers ---

/// Login with username/password, returns a session cookie
pub async fn login(State(state): State<Arc<AppState>>, Json(body): Json<LoginRequest>) -> Response {
    let (username, password) = &state.admin_credentials;

    let user_match = body.username.as_bytes().ct_eq(username.as_bytes());
    let pass_match = body.password.as_bytes().ct_eq(password.as_bytes());

    if user_match.into() && pass_match.into() {
        let token = format!(
            "{:032x}{:032x}",
            rand::random::<u128>(),
            rand::random::<u128>()
        );
        let expires_at = now_secs() + SESSION_TTL_SECS;
        crate::save_session(&token, expires_at).await;

        let secure_flag = if state.secure_cookies { "; Secure" } else { "" };
        let cookie = format!(
            "admin_session={}; HttpOnly; SameSite=Strict; Path=/admin; Max-Age={}{}",
            token, SESSION_TTL_SECS, secure_flag
        );

        (
            StatusCode::OK,
            [(header::SET_COOKIE, cookie)],
            Json(SuccessResponse { success: true }),
        )
            .into_response()
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "Invalid credentials".into(),
            }),
        )
            .into_response()
    }
}

/// Logout and clear session cookie
pub async fn logout(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Response {
    if let Some(cookie_header) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok())
        && let Some(token) = parse_cookie(cookie_header, "admin_session")
    {
        crate::remove_session(&token).await;
    }

    let secure_flag = if state.secure_cookies { "; Secure" } else { "" };
    let clear_cookie = format!(
        "admin_session=; HttpOnly; SameSite=Strict; Path=/admin; Max-Age=0{}",
        secure_flag
    );

    (
        StatusCode::OK,
        [(header::SET_COOKIE, clear_cookie)],
        Json(SuccessResponse { success: true }),
    )
        .into_response()
}

/// Check if the current request is authenticated
pub async fn auth_check(headers: axum::http::HeaderMap) -> Json<AuthCheckResponse> {
    let authenticated = if let Some(cookie_header) =
        headers.get(header::COOKIE).and_then(|v| v.to_str().ok())
        && let Some(token) = parse_cookie(cookie_header, "admin_session")
    {
        crate::validate_session(&token).await
    } else {
        false
    };

    Json(AuthCheckResponse {
        authenticated,
        auth_required: true,
    })
}

// --- Handlers ---

/// Fetch plan name from Anthropic profile endpoint.
/// Returns None on any error (non-critical).
async fn fetch_plan_name(state: &AppState) -> Option<String> {
    let token = state.oauth.refresh_if_needed().await.ok()??;
    let resp = state
        .http_client
        .get(ANTHROPIC_PROFILE_URL)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .ok()?;
    let body: serde_json::Value = resp.json().await.ok()?;
    let account = body.get("account")?;
    if account.get("has_claude_max")?.as_bool() == Some(true) {
        return Some("Max".into());
    }
    if account.get("has_claude_pro")?.as_bool() == Some(true) {
        return Some("Pro".into());
    }
    None
}

/// Fetch subscription window reset times from Anthropic.
/// Returns Default on any error (non-critical).
async fn fetch_window_resets(state: &AppState) -> SubscriptionState {
    let token = match state.oauth.refresh_if_needed().await {
        Ok(Some(t)) => t,
        _ => return SubscriptionState::default(),
    };

    let resp = match state
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
    {
        Ok(r) if r.status().is_success() => r,
        _ => return SubscriptionState::default(),
    };

    let usage: SubscriptionUsageResponse = match resp.json().await {
        Ok(u) => u,
        Err(_) => return SubscriptionState::default(),
    };

    extract_subscription_state(&usage)
}

/// Parse subscription usage response into cached state (reset times + utilization).
fn extract_subscription_state(usage: &SubscriptionUsageResponse) -> SubscriptionState {
    let parse_reset = |s: &str| -> Option<u64> {
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.timestamp_millis() as u64)
    };

    let five_hour_reset_at = usage
        .five_hour
        .as_ref()
        .and_then(|u| u.resets_at.as_deref())
        .and_then(parse_reset);

    let seven_day_reset_at = usage
        .seven_day
        .as_ref()
        .and_then(|u| u.resets_at.as_deref())
        .and_then(parse_reset);

    let five_hour_utilization = usage.five_hour.as_ref().and_then(|u| u.utilization);
    let seven_day_utilization = usage.seven_day.as_ref().and_then(|u| u.utilization);

    SubscriptionState {
        five_hour_reset_at,
        seven_day_reset_at,
        five_hour_utilization,
        seven_day_utilization,
    }
}

fn timestamp_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// Get cached window resets, refreshing inline if stale (window boundary crossed).
pub async fn get_or_refresh_window_resets(state: &AppState) -> SubscriptionState {
    let now = timestamp_millis();
    let cached = state.window_resets.read().await.clone();

    // Refresh if cache is empty or any cached reset time has passed
    let needs_refresh = cached.five_hour_reset_at.is_none()
        || cached.five_hour_reset_at.is_some_and(|t| now >= t)
        || cached.seven_day_reset_at.is_some_and(|t| now >= t);

    if needs_refresh {
        let fresh = fetch_window_resets(state).await;
        if fresh.five_hour_reset_at.is_some() {
            tracing::info!(
                "Fetched subscription window resets: 5h={:?}, 7d={:?}",
                fresh.five_hour_reset_at,
                fresh.seven_day_reset_at
            );
            *state.window_resets.write().await = fresh.clone();
            return fresh;
        }
    }
    cached
}

/// Always fetch fresh subscription state from Anthropic and update cache.
/// Used for pre-request extra-usage checks where stale data is not acceptable.
pub async fn fetch_fresh_subscription_state(state: &AppState) -> SubscriptionState {
    let fresh = fetch_window_resets(state).await;
    if fresh.five_hour_reset_at.is_some() {
        *state.window_resets.write().await = fresh.clone();
    }
    fresh
}

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
    let authenticated = state.auth_store.has("anthropic").await;
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
            tracing::warn!("Failed to contact Anthropic usage API: {e}");
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
        tracing::warn!("Anthropic usage API returned {status}: {body}");
        return Err((
            StatusCode::BAD_GATEWAY,
            Json(ErrorResponse {
                error: format!("Anthropic returned {status}: {body}"),
            }),
        ));
    }

    let usage: SubscriptionUsageResponse = serde_json::from_str(&body).map_err(|e| {
        tracing::warn!("Failed to parse usage response: {e}");
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

/// Maximum allowed length for key names
const MAX_KEY_NAME_LENGTH: usize = 100;

/// Validate a key name: must be non-empty, not too long, no control characters
fn validate_key_name(name: &str) -> Result<(), &'static str> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Key name cannot be empty");
    }
    if name.len() > MAX_KEY_NAME_LENGTH {
        return Err("Key name too long (max 100 characters)");
    }
    if name.chars().any(|c| c.is_control()) {
        return Err("Key name cannot contain control characters");
    }
    Ok(())
}

/// Create a new API key
#[utoipa::path(
    post,
    path = "/keys",
    tag = "keys",
    request_body = CreateKeyRequest,
    responses(
        (status = 200, body = CreateKeyResponse),
        (status = 400, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn create_key(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateKeyRequest>,
) -> Result<Json<CreateKeyResponse>, (StatusCode, Json<ErrorResponse>)> {
    let name = body.name.trim().to_string();

    if let Err(e) = validate_key_name(&name) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        ));
    }

    match state.client_keys.create(name).await {
        Ok(key) => Ok(Json(CreateKeyResponse {
            key: key.key,
            id: key.id,
        })),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

/// List all API keys
#[utoipa::path(
    get,
    path = "/keys/list",
    tag = "keys",
    responses(
        (status = 200, body = ListKeysResponse),
    )
)]
pub async fn list_keys(State(state): State<Arc<AppState>>) -> Json<ListKeysResponse> {
    let keys = state.client_keys.list().await;
    Json(ListKeysResponse { keys })
}

/// Delete an API key
#[utoipa::path(
    delete,
    path = "/keys/{id}",
    tag = "keys",
    params(("id" = String, Path, description = "Key ID")),
    responses(
        (status = 200, body = SuccessResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn delete_key(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.client_keys.delete(&id).await {
        Ok(true) => Ok(Json(SuccessResponse { success: true })),
        Ok(false) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Key not found".into(),
            }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

/// Toggle a key enabled/disabled
#[utoipa::path(
    put,
    path = "/keys/{id}/enabled",
    tag = "keys",
    params(("id" = String, Path, description = "Key ID")),
    request_body = SetKeyEnabledRequest,
    responses(
        (status = 200, body = SuccessResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn set_key_enabled(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<SetKeyEnabledRequest>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.client_keys.set_enabled(&id, body.enabled).await {
        Ok(true) => Ok(Json(SuccessResponse { success: true })),
        Ok(false) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Key not found".into(),
            }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

/// Toggle allow_extra_usage for a key
#[utoipa::path(
    put,
    path = "/keys/{id}/allow-extra-usage",
    tag = "keys",
    params(("id" = String, Path, description = "Key ID")),
    request_body = SetAllowExtraUsageRequest,
    responses(
        (status = 200, body = SuccessResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn set_allow_extra_usage(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<SetAllowExtraUsageRequest>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state
        .client_keys
        .set_allow_extra_usage(&id, body.allow_extra_usage)
        .await
    {
        Ok(true) => Ok(Json(SuccessResponse { success: true })),
        Ok(false) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Key not found".into(),
            }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

/// Get usage statistics for a key
#[utoipa::path(
    get,
    path = "/keys/{id}/usage",
    tag = "keys",
    params(("id" = String, Path, description = "Key ID")),
    responses(
        (status = 200, body = KeyUsageResponse),
        (status = 404, body = ErrorResponse),
    )
)]
pub async fn get_key_usage(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<KeyUsageResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.client_keys.get_usage(&id).await {
        Some((limits, usage)) => Ok(Json(KeyUsageResponse { limits, usage })),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Key not found".into(),
            }),
        )),
    }
}

/// Update limits for a key
#[utoipa::path(
    put,
    path = "/keys/{id}/limits",
    tag = "keys",
    params(("id" = String, Path, description = "Key ID")),
    request_body = UpdateLimitsRequest,
    responses(
        (status = 200, body = SuccessResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn update_key_limits(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateLimitsRequest>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    let limits = TokenLimits {
        five_hour_limit: body.five_hour_limit,
        weekly_limit: body.weekly_limit,
        total_limit: body.total_limit,
    };

    match state.client_keys.set_limits(&id, limits).await {
        Ok(true) => Ok(Json(SuccessResponse { success: true })),
        Ok(false) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Key not found".into(),
            }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

/// Reset usage counters for a key
#[utoipa::path(
    post,
    path = "/keys/{id}/usage/reset",
    tag = "keys",
    params(("id" = String, Path, description = "Key ID")),
    request_body = ResetUsageRequest,
    responses(
        (status = 200, body = SuccessResponse),
        (status = 400, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn reset_key_usage(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<ResetUsageRequest>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    let reset_type = match body.reset_type.to_lowercase().as_str() {
        "fivehour" | "hourly" => UsageResetType::FiveHour,
        "weekly" => UsageResetType::Weekly,
        "total" => UsageResetType::Total,
        "all" => UsageResetType::All,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "Invalid reset type. Use: fiveHour, weekly, total, or all".into(),
                }),
            ));
        }
    };

    match state.client_keys.reset_usage(&id, reset_type).await {
        Ok(true) => Ok(Json(SuccessResponse { success: true })),
        Ok(false) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Key not found".into(),
            }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

// ========================================================================
// Models management
// ========================================================================

/// List all models (admin sees enabled + disabled)
#[utoipa::path(
    get,
    path = "/models",
    tag = "models",
    responses(
        (status = 200, body = ListModelsResponse),
    )
)]
pub async fn list_models_admin(State(state): State<Arc<AppState>>) -> Json<ListModelsResponse> {
    let models = state.models.list().await;
    Json(ListModelsResponse { models })
}

/// Add a new model
#[utoipa::path(
    post,
    path = "/models",
    tag = "models",
    request_body = AddModelRequest,
    responses(
        (status = 200, body = SuccessResponse),
        (status = 400, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn add_model(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AddModelRequest>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    let id = body.id.trim();
    if id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Model ID cannot be empty".into(),
            }),
        ));
    }

    match state
        .models
        .add(
            id,
            body.input_price,
            body.output_price,
            body.cache_read_price,
            body.cache_write_price,
        )
        .await
    {
        Ok(_) => Ok(Json(SuccessResponse { success: true })),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

/// Delete a model
#[utoipa::path(
    delete,
    path = "/models/{id}",
    tag = "models",
    params(("id" = String, Path, description = "Model ID")),
    responses(
        (status = 200, body = SuccessResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn delete_model(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.models.remove(&id).await {
        Ok(true) => Ok(Json(SuccessResponse { success: true })),
        Ok(false) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Model not found".into(),
            }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

/// Update a model (prices, enabled)
#[utoipa::path(
    put,
    path = "/models/{id}",
    tag = "models",
    params(("id" = String, Path, description = "Model ID")),
    request_body = UpdateModelRequest,
    responses(
        (status = 200, body = SuccessResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn update_model(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateModelRequest>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state
        .models
        .update(
            &id,
            body.input_price,
            body.output_price,
            body.cache_read_price,
            body.cache_write_price,
            body.enabled,
        )
        .await
    {
        Ok(true) => Ok(Json(SuccessResponse { success: true })),
        Ok(false) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Model not found".into(),
            }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

/// Reorder models
#[utoipa::path(
    put,
    path = "/models/reorder",
    tag = "models",
    request_body = ReorderModelsRequest,
    responses(
        (status = 200, body = SuccessResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn reorder_models(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ReorderModelsRequest>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.models.reorder(body.ids).await {
        Ok(_) => Ok(Json(SuccessResponse { success: true })),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

// ========================================================================
// Per-key model access
// ========================================================================

/// Get allowed models for a key
#[utoipa::path(
    get,
    path = "/keys/{id}/models",
    tag = "keys",
    params(("id" = String, Path, description = "Key ID")),
    responses(
        (status = 200, body = KeyModelsResponse),
    )
)]
pub async fn get_key_models(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Json<KeyModelsResponse> {
    let models = state.client_keys.get_allowed_models(&id).await;
    let allow_all = models.is_empty();
    Json(KeyModelsResponse { allow_all, models })
}

/// Set allowed models for a key (empty = allow all)
#[utoipa::path(
    put,
    path = "/keys/{id}/models",
    tag = "keys",
    params(("id" = String, Path, description = "Key ID")),
    request_body = SetKeyModelsRequest,
    responses(
        (status = 200, body = SuccessResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn set_key_models(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<SetKeyModelsRequest>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.client_keys.set_allowed_models(&id, body.models).await {
        Ok(_) => Ok(Json(SuccessResponse { success: true })),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

// ========================================================================
// Per-key per-model usage
// ========================================================================

/// Get per-model usage for a key
#[utoipa::path(
    get,
    path = "/keys/{id}/model-usage",
    tag = "keys",
    params(("id" = String, Path, description = "Key ID")),
    responses(
        (status = 200, body = KeyModelUsageResponse),
    )
)]
pub async fn get_key_model_usage(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Json<KeyModelUsageResponse> {
    let entries = state.client_keys.get_model_usage(&id).await;
    Json(KeyModelUsageResponse { entries })
}

/// Set per-model limits for a key
#[utoipa::path(
    put,
    path = "/keys/{id}/model-usage/{model}/limits",
    tag = "keys",
    params(
        ("id" = String, Path, description = "Key ID"),
        ("model" = String, Path, description = "Model ID"),
    ),
    request_body = UpdateLimitsRequest,
    responses(
        (status = 200, body = SuccessResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn set_key_model_limits(
    State(state): State<Arc<AppState>>,
    Path((id, model)): Path<(String, String)>,
    Json(body): Json<UpdateLimitsRequest>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    let limits = TokenLimits {
        five_hour_limit: body.five_hour_limit,
        weekly_limit: body.weekly_limit,
        total_limit: body.total_limit,
    };

    match state
        .client_keys
        .set_model_limits(&id, &model, limits)
        .await
    {
        Ok(_) => Ok(Json(SuccessResponse { success: true })),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

/// Remove per-model limits for a key
#[utoipa::path(
    delete,
    path = "/keys/{id}/model-usage/{model}/limits",
    tag = "keys",
    params(
        ("id" = String, Path, description = "Key ID"),
        ("model" = String, Path, description = "Model ID"),
    ),
    responses(
        (status = 200, body = SuccessResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn remove_key_model_limits(
    State(state): State<Arc<AppState>>,
    Path((id, model)): Path<(String, String)>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.client_keys.remove_model_limits(&id, &model).await {
        Ok(true) => Ok(Json(SuccessResponse { success: true })),
        Ok(false) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Model usage entry not found".into(),
            }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

/// Reset per-model usage counters for a key
#[utoipa::path(
    post,
    path = "/keys/{id}/model-usage/{model}/reset",
    tag = "keys",
    params(
        ("id" = String, Path, description = "Key ID"),
        ("model" = String, Path, description = "Model ID"),
    ),
    request_body = ResetUsageRequest,
    responses(
        (status = 200, body = SuccessResponse),
        (status = 400, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn reset_key_model_usage(
    State(state): State<Arc<AppState>>,
    Path((id, model)): Path<(String, String)>,
    Json(body): Json<ResetUsageRequest>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    let reset_type = match body.reset_type.to_lowercase().as_str() {
        "fivehour" | "hourly" => UsageResetType::FiveHour,
        "weekly" => UsageResetType::Weekly,
        "total" => UsageResetType::Total,
        "all" => UsageResetType::All,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "Invalid reset type. Use: fiveHour, weekly, total, or all".into(),
                }),
            ));
        }
    };

    match state
        .client_keys
        .reset_model_usage(&id, &model, reset_type)
        .await
    {
        Ok(true) => Ok(Json(SuccessResponse { success: true })),
        Ok(false) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Model usage entry not found".into(),
            }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

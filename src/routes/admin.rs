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

use crate::AppState;
use crate::auth::{ClientKey, TokenLimits, TokenUsage, UsageResetType};
use crate::parse_cookie;

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
    hourly_limit: Option<u64>,
    weekly_limit: Option<u64>,
    total_limit: Option<u64>,
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct ResetUsageRequest {
    /// Which counter to reset: "hourly", "weekly", "total", or "all"
    #[serde(rename = "type")]
    reset_type: String,
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
        state.admin_sessions.lock().await.insert(token.clone());

        let cookie = format!(
            "admin_session={}; HttpOnly; SameSite=Strict; Path=/admin; Max-Age=86400",
            token
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
        state.admin_sessions.lock().await.remove(&token);
    }

    let clear_cookie = "admin_session=; HttpOnly; SameSite=Strict; Path=/admin; Max-Age=0";

    (
        StatusCode::OK,
        [(header::SET_COOKIE, clear_cookie)],
        Json(SuccessResponse { success: true }),
    )
        .into_response()
}

/// Check if the current request is authenticated
pub async fn auth_check(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Json<AuthCheckResponse> {
    let authenticated =
        if let Some(cookie_header) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()) {
            if let Some(token) = parse_cookie(cookie_header, "admin_session") {
                state.admin_sessions.lock().await.contains(&token)
            } else {
                false
            }
        } else {
            false
        };

    Json(AuthCheckResponse {
        authenticated,
        auth_required: true,
    })
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
    let authenticated = state.auth_store.has("anthropic").await;
    Json(OAuthStatusResponse { authenticated })
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
        hourly_limit: body.hourly_limit,
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
        "hourly" => UsageResetType::Hourly,
        "weekly" => UsageResetType::Weekly,
        "total" => UsageResetType::Total,
        "all" => UsageResetType::All,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "Invalid reset type. Use: hourly, weekly, total, or all".into(),
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

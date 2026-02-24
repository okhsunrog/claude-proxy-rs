use axum::{
    Json,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use subtle::ConstantTimeEq;
use utoipa::ToSchema;

use super::{ErrorResponse, SuccessResponse};
use crate::parse_cookie;
use crate::{AppState, SESSION_TTL_SECS, now_secs};

// --- Types ---

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

// --- Handlers ---

/// Login with username/password, returns a session cookie
pub async fn login(State(state): State<Arc<AppState>>, Json(body): Json<LoginRequest>) -> Response {
    let creds = &state.admin_credentials;

    let user_match = body.username.as_bytes().ct_eq(creds.username.as_bytes());
    let pass_match = body.password.as_bytes().ct_eq(creds.password.as_bytes());

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

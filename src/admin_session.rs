use axum::{
    extract::Request,
    extract::State,
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use tracing::warn;

use crate::{AppState, db};

/// Session TTL: 30 days (with sliding expiration on each request)
pub(crate) const SESSION_TTL_SECS: u64 = 30 * 24 * 3600;

pub struct AdminCredentials {
    pub username: String,
    pub password: String,
}

/// Save a session token to the database.
pub(crate) async fn save_session(token: &str, expires_at: u64) {
    if let Ok(conn) = db::get_conn().await
        && let Err(e) = sqlx::query!(
            "INSERT INTO admin_sessions (token, expires_at) VALUES ($1, $2) \
             ON CONFLICT (token) DO UPDATE SET expires_at = EXCLUDED.expires_at",
            token,
            expires_at as i64,
        )
        .execute(&conn)
        .await
    {
        warn!("Failed to save session: {e}");
    }
}

/// Validate a session token, returns true if valid and not expired.
/// Also extends the session (sliding expiration) if it's valid.
pub(crate) async fn validate_session(token: &str) -> bool {
    let Ok(conn) = db::get_conn().await else {
        return false;
    };
    let Ok(row) = sqlx::query!(
        "SELECT expires_at FROM admin_sessions WHERE token = $1",
        token
    )
    .fetch_optional(&conn)
    .await
    else {
        return false;
    };
    let Some(row) = row else {
        return false;
    };
    let expires_at = row.expires_at;
    let now = now_secs() as i64;
    if now >= expires_at {
        // Expired: clean it up.
        if let Err(e) = sqlx::query!("DELETE FROM admin_sessions WHERE token = $1", token)
            .execute(&conn)
            .await
        {
            warn!("Failed to delete expired admin session: {e}");
        }
        return false;
    }

    // Sliding expiration: renew if more than 1 day has passed since last renewal.
    let new_expires = now + SESSION_TTL_SECS as i64;
    if new_expires - expires_at > 24 * 3600
        && let Err(e) = sqlx::query!(
            "UPDATE admin_sessions SET expires_at = $1 WHERE token = $2",
            new_expires,
            token
        )
        .execute(&conn)
        .await
    {
        warn!("Failed to refresh admin session expiry: {e}");
    }
    true
}

/// Remove a session token from the database.
pub(crate) async fn remove_session(token: &str) {
    if let Ok(conn) = db::get_conn().await
        && let Err(e) = sqlx::query!("DELETE FROM admin_sessions WHERE token = $1", token)
            .execute(&conn)
            .await
    {
        warn!("Failed to remove admin session: {e}");
    }
}

pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn session_expires_at() -> u64 {
    now_secs() + SESSION_TTL_SECS
}

pub(crate) fn session_cookie(token: &str, secure: bool) -> String {
    let secure_flag = if secure { "; Secure" } else { "" };
    format!(
        "admin_session={token}; HttpOnly; SameSite=Strict; Path=/admin; Max-Age={SESSION_TTL_SECS}{secure_flag}"
    )
}

pub(crate) fn clear_session_cookie(secure: bool) -> String {
    let secure_flag = if secure { "; Secure" } else { "" };
    format!("admin_session=; HttpOnly; SameSite=Strict; Path=/admin; Max-Age=0{secure_flag}")
}

/// Parse a named cookie from the Cookie header.
pub(crate) fn parse_cookie(header: &str, name: &str) -> Option<String> {
    header.split(';').find_map(|cookie| {
        let (key, value) = cookie.trim().split_once('=')?;
        if key.trim() == name {
            Some(value.trim().to_string())
        } else {
            None
        }
    })
}

/// Middleware for admin routes authentication (session cookie or Basic Auth).
pub(crate) async fn admin_auth_middleware(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    if state.disable_auth {
        return next.run(request).await;
    }

    let creds = &state.admin_credentials;

    // Check for session cookie first.
    if let Some(cookie_header) = request
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        && let Some(token) = parse_cookie(cookie_header, "admin_session")
        && validate_session(&token).await
    {
        let mut response = next.run(request).await;
        // Refresh cookie Max-Age to keep browser cookie in sync with sliding expiration.
        let cookie = session_cookie(&token, state.secure_cookies);
        if let Ok(value) = cookie.parse() {
            response.headers_mut().insert(header::SET_COOKIE, value);
        }
        return response;
    }

    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let Some(auth_value) = auth_header else {
        return unauthorized_response();
    };

    let Some(encoded) = auth_value.strip_prefix("Basic ") else {
        return unauthorized_response();
    };

    let Ok(decoded) = STANDARD.decode(encoded) else {
        return unauthorized_response();
    };

    let Ok(credentials) = String::from_utf8(decoded) else {
        return unauthorized_response();
    };

    let Some((provided_user, provided_pass)) = credentials.split_once(':') else {
        return unauthorized_response();
    };

    let user_match = provided_user.as_bytes().ct_eq(creds.username.as_bytes());
    let pass_match = provided_pass.as_bytes().ct_eq(creds.password.as_bytes());

    if user_match.into() && pass_match.into() {
        next.run(request).await
    } else {
        unauthorized_response()
    }
}

fn unauthorized_response() -> Response {
    (StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
}

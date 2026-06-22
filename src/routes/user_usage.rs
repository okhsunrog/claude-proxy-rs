//! User-facing usage dashboard API.
//!
//! These routes are authenticated via the client's own `sk-proxy-*` Bearer token —
//! no admin session required. They expose only data belonging to that key.
//! Paths are `/usage/...` which, nested under `/admin`, become `/admin/usage/...`.

use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::AppState;
use crate::auth::ModelUsageEntry;
use crate::auth::client_keys::{TokenLimits, TokenUsage};
use crate::db;
use crate::usage::history::{
    HistoryPeriod, ModelBreakdownResponse, TimeseriesResponse, by_model, timeseries,
};

// ── auth helper ────────────────────────────────────────────────────────────────

async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(String, String), (StatusCode, Json<ErrorBody>)> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(ErrorBody {
                    error: "Missing Authorization header".into(),
                }),
            )
        })?;

    let token = auth.strip_prefix("Bearer ").ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(ErrorBody {
                error: "Authorization header must use Bearer scheme".into(),
            }),
        )
    })?;

    match state.client_keys.validate(token).await {
        Ok(Some(key)) => Ok((key.id, key.name)),
        Ok(None) => Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorBody {
                error: "Invalid or disabled API key".into(),
            }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody {
                error: e.to_string(),
            }),
        )),
    }
}

// ── response types ─────────────────────────────────────────────────────────────

#[derive(Serialize, ToSchema)]
pub struct ErrorBody {
    pub error: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserUsageResponse {
    pub key_name: String,
    pub limits: TokenLimits,
    pub usage: TokenUsage,
    pub model_entries: Vec<ModelUsageEntry>,
}

#[derive(Deserialize, ToSchema)]
pub struct PeriodQuery {
    /// Time period: "24h", "7d", or "30d"
    pub period: Option<String>,
}

// ── handlers ──────────────────────────────────────────────────────────────────

/// Get current usage and limits for the authenticated key
#[utoipa::path(
    get,
    path = "/usage/me",
    tag = "user-usage",
    security(("bearer_key" = [])),
    responses(
        (status = 200, body = UserUsageResponse),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 500, body = ErrorBody),
    )
)]
pub async fn get_my_usage(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<UserUsageResponse>, (StatusCode, Json<ErrorBody>)> {
    let (key_id, key_name) = authenticate(&state, &headers).await?;

    let (limits, usage) = state
        .client_keys
        .get_usage(&key_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorBody {
                    error: e.to_string(),
                }),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorBody {
                    error: "Key not found".into(),
                }),
            )
        })?;

    let model_entries = state
        .client_keys
        .get_model_usage(&key_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorBody {
                    error: e.to_string(),
                }),
            )
        })?;

    Ok(Json(UserUsageResponse {
        key_name,
        limits,
        usage,
        model_entries,
    }))
}

/// Get cost/token timeseries for the authenticated key
#[utoipa::path(
    get,
    path = "/usage/history/timeseries",
    tag = "user-usage",
    security(("bearer_key" = [])),
    params(("period" = Option<String>, Query, description = "Period: 24h, 7d, or 30d")),
    responses(
        (status = 200, body = TimeseriesResponse),
        (status = 401, body = ErrorBody),
        (status = 500, body = ErrorBody),
    )
)]
pub async fn get_my_timeseries(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PeriodQuery>,
) -> Result<Json<TimeseriesResponse>, (StatusCode, Json<ErrorBody>)> {
    let (key_id, _) = authenticate(&state, &headers).await?;

    let period = HistoryPeriod::parse(query.period.as_deref());

    let conn = db::get_conn().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(
        timeseries(&conn, &period, Some(key_id.as_str()))
            .await
            .unwrap_or_else(|_| period.empty_timeseries()),
    ))
}

/// Get per-model breakdown for the authenticated key
#[utoipa::path(
    get,
    path = "/usage/history/by-model",
    tag = "user-usage",
    security(("bearer_key" = [])),
    params(("period" = Option<String>, Query, description = "Period: 24h, 7d, or 30d")),
    responses(
        (status = 200, body = ModelBreakdownResponse),
        (status = 401, body = ErrorBody),
        (status = 500, body = ErrorBody),
    )
)]
pub async fn get_my_by_model(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PeriodQuery>,
) -> Result<Json<ModelBreakdownResponse>, (StatusCode, Json<ErrorBody>)> {
    let (key_id, _) = authenticate(&state, &headers).await?;

    let period = HistoryPeriod::parse(query.period.as_deref());

    let conn = db::get_conn().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(
        by_model(&conn, &period, Some(key_id.as_str()))
            .await
            .unwrap_or_else(|_| period.empty_models()),
    ))
}

// ── router ─────────────────────────────────────────────────────────────────────

/// Build the OpenApiRouter for user-facing usage endpoints (no admin auth).
pub fn user_usage_router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(routes!(get_my_usage))
        .routes(routes!(get_my_timeseries))
        .routes(routes!(get_my_by_model))
}

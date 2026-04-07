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
use crate::auth::client_keys::{TokenLimits, TokenUsage, get_u64};
use crate::subscription::timestamp_millis;

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

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TimeseriesPoint {
    pub timestamp: u64,
    pub request_count: u64,
    pub cost_microdollars: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TimeseriesResponse {
    pub period: String,
    pub granularity: String,
    pub points: Vec<TimeseriesPoint>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelBreakdown {
    pub model: String,
    pub request_count: u64,
    pub cost_microdollars: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelBreakdownResponse {
    pub period: String,
    pub models: Vec<ModelBreakdown>,
}

// ── period helper ──────────────────────────────────────────────────────────────

fn parse_period(period: &str) -> (u64, u64, &'static str) {
    match period {
        "7d" => (7 * 24 * 3600 * 1000, 6 * 3600 * 1000, "6h"),
        "30d" => (30 * 24 * 3600 * 1000, 24 * 3600 * 1000, "day"),
        _ => (24 * 3600 * 1000, 3600 * 1000, "hour"),
    }
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

    let period_str = query.period.as_deref().unwrap_or("24h");
    let (cutoff_ms, bucket_ms, granularity) = parse_period(period_str);
    let now = timestamp_millis();
    let cutoff = now.saturating_sub(cutoff_ms);

    let conn = crate::db::get_conn().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody {
                error: e.to_string(),
            }),
        )
    })?;

    let Ok(mut rows) = conn
        .query(
            "SELECT (created_at / ?1) * ?1 AS bucket, \
             COUNT(*), SUM(cost_microdollars), \
             SUM(input_tokens), SUM(output_tokens), \
             SUM(cache_read_tokens), SUM(cache_write_tokens) \
             FROM request_log WHERE key_id = ?3 AND created_at >= ?2 \
             GROUP BY bucket ORDER BY bucket",
            (bucket_ms as i64, cutoff as i64, key_id.as_str()),
        )
        .await
    else {
        return Ok(Json(TimeseriesResponse {
            period: period_str.to_string(),
            granularity: granularity.to_string(),
            points: Vec::new(),
        }));
    };

    let mut data_map = std::collections::HashMap::new();
    while let Ok(Some(row)) = rows.next().await {
        let ts = get_u64(&row, 0);
        data_map.insert(
            ts,
            TimeseriesPoint {
                timestamp: ts,
                request_count: get_u64(&row, 1),
                cost_microdollars: get_u64(&row, 2),
                input_tokens: get_u64(&row, 3),
                output_tokens: get_u64(&row, 4),
                cache_read_tokens: get_u64(&row, 5),
                cache_write_tokens: get_u64(&row, 6),
            },
        );
    }

    let bucket_start = (cutoff / bucket_ms) * bucket_ms;
    let bucket_end = (now / bucket_ms) * bucket_ms;
    let mut points = Vec::new();
    let mut ts = bucket_start;
    while ts <= bucket_end {
        points.push(data_map.remove(&ts).unwrap_or(TimeseriesPoint {
            timestamp: ts,
            request_count: 0,
            cost_microdollars: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        }));
        ts += bucket_ms;
    }

    Ok(Json(TimeseriesResponse {
        period: period_str.to_string(),
        granularity: granularity.to_string(),
        points,
    }))
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

    let period_str = query.period.as_deref().unwrap_or("24h");
    let (cutoff_ms, _, _) = parse_period(period_str);
    let now = timestamp_millis();
    let cutoff = now.saturating_sub(cutoff_ms);

    let conn = crate::db::get_conn().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody {
                error: e.to_string(),
            }),
        )
    })?;

    let Ok(mut rows) = conn
        .query(
            "SELECT model, COUNT(*), SUM(cost_microdollars), \
             SUM(input_tokens), SUM(output_tokens), \
             SUM(cache_read_tokens), SUM(cache_write_tokens) \
             FROM request_log WHERE key_id = ?2 AND created_at >= ?1 \
             GROUP BY model ORDER BY SUM(cost_microdollars) DESC",
            (cutoff as i64, key_id.as_str()),
        )
        .await
    else {
        return Ok(Json(ModelBreakdownResponse {
            period: period_str.to_string(),
            models: Vec::new(),
        }));
    };

    let mut models = Vec::new();
    while let Ok(Some(row)) = rows.next().await {
        let Ok(model) = row.get::<String>(0) else {
            continue;
        };
        models.push(ModelBreakdown {
            model,
            request_count: get_u64(&row, 1),
            cost_microdollars: get_u64(&row, 2),
            input_tokens: get_u64(&row, 3),
            output_tokens: get_u64(&row, 4),
            cache_read_tokens: get_u64(&row, 5),
            cache_write_tokens: get_u64(&row, 6),
        });
    }

    Ok(Json(ModelBreakdownResponse {
        period: period_str.to_string(),
        models,
    }))
}

// ── router ─────────────────────────────────────────────────────────────────────

/// Build the OpenApiRouter for user-facing usage endpoints (no admin auth).
pub fn user_usage_router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(routes!(get_my_usage))
        .routes(routes!(get_my_timeseries))
        .routes(routes!(get_my_by_model))
}

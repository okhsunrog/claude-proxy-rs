use axum::{Json, http::StatusCode};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{ErrorResponse, SuccessResponse};
use crate::auth::client_keys::i64_to_u64;
use crate::subscription::timestamp_millis;

// --- Types ---

#[derive(Deserialize, ToSchema)]
pub struct UsageHistoryQuery {
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

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyBreakdown {
    pub key_id: String,
    pub key_name: Option<String>,
    pub request_count: u64,
    pub cost_microdollars: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyBreakdownResponse {
    pub period: String,
    pub keys: Vec<KeyBreakdown>,
}

// --- Helpers ---

/// Parse period string into (cutoff_ms_ago, bucket_ms, granularity_label)
fn parse_period(period: &str) -> (u64, u64, &'static str) {
    match period {
        "7d" => (7 * 24 * 3600 * 1000, 6 * 3600 * 1000, "6h"),
        "30d" => (30 * 24 * 3600 * 1000, 24 * 3600 * 1000, "day"),
        _ => (24 * 3600 * 1000, 3600 * 1000, "hour"), // default: 24h
    }
}

// --- Handlers ---

#[utoipa::path(
    get,
    path = "/usage-history/timeseries",
    params(("period" = Option<String>, Query, description = "Period: 24h, 7d, or 30d")),
    responses(
        (status = 200, body = TimeseriesResponse),
    )
)]
pub async fn get_usage_history_timeseries(
    axum::extract::Query(query): axum::extract::Query<UsageHistoryQuery>,
) -> Json<TimeseriesResponse> {
    let period_str = query.period.as_deref().unwrap_or("24h");
    let (cutoff_ms, bucket_ms, granularity) = parse_period(period_str);
    let now = timestamp_millis();
    let cutoff = now.saturating_sub(cutoff_ms);

    let Ok(conn) = crate::db::get_conn().await else {
        return Json(TimeseriesResponse {
            period: period_str.to_string(),
            granularity: granularity.to_string(),
            points: Vec::new(),
        });
    };

    let Ok(rows) = sqlx::query!(
            "SELECT (created_at / $1) * $1 AS bucket, \
             COUNT(*) AS \"request_count!\", COALESCE(SUM(cost_microdollars), 0)::BIGINT AS \"cost_microdollars!\", \
             COALESCE(SUM(input_tokens), 0)::BIGINT AS \"input_tokens!\", COALESCE(SUM(output_tokens), 0)::BIGINT AS \"output_tokens!\", \
             COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS \"cache_read_tokens!\", COALESCE(SUM(cache_write_tokens), 0)::BIGINT AS \"cache_write_tokens!\" \
             FROM request_log WHERE created_at >= $2 \
             GROUP BY bucket ORDER BY bucket",
        bucket_ms as i64,
        cutoff as i64,
    )
    .fetch_all(&conn)
    .await
    else {
        return Json(TimeseriesResponse {
            period: period_str.to_string(),
            granularity: granularity.to_string(),
            points: Vec::new(),
        });
    };

    let mut data_map = std::collections::HashMap::new();
    for row in rows {
        let ts = i64_to_u64(row.bucket.unwrap_or(0));
        data_map.insert(
            ts,
            TimeseriesPoint {
                timestamp: ts,
                request_count: i64_to_u64(row.request_count),
                cost_microdollars: i64_to_u64(row.cost_microdollars),
                input_tokens: i64_to_u64(row.input_tokens),
                output_tokens: i64_to_u64(row.output_tokens),
                cache_read_tokens: i64_to_u64(row.cache_read_tokens),
                cache_write_tokens: i64_to_u64(row.cache_write_tokens),
            },
        );
    }

    // Fill empty buckets across the full time range
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

    Json(TimeseriesResponse {
        period: period_str.to_string(),
        granularity: granularity.to_string(),
        points,
    })
}

#[utoipa::path(
    get,
    path = "/usage-history/by-model",
    params(("period" = Option<String>, Query, description = "Period: 24h, 7d, or 30d")),
    responses(
        (status = 200, body = ModelBreakdownResponse),
    )
)]
pub async fn get_usage_history_by_model(
    axum::extract::Query(query): axum::extract::Query<UsageHistoryQuery>,
) -> Json<ModelBreakdownResponse> {
    let period_str = query.period.as_deref().unwrap_or("24h");
    let (cutoff_ms, _, _) = parse_period(period_str);
    let now = timestamp_millis();
    let cutoff = now.saturating_sub(cutoff_ms);

    let Ok(conn) = crate::db::get_conn().await else {
        return Json(ModelBreakdownResponse {
            period: period_str.to_string(),
            models: Vec::new(),
        });
    };

    let Ok(rows) = sqlx::query!(
        "SELECT model, COUNT(*) AS \"request_count!\", COALESCE(SUM(cost_microdollars), 0)::BIGINT AS \"cost_microdollars!\", \
             COALESCE(SUM(input_tokens), 0)::BIGINT AS \"input_tokens!\", COALESCE(SUM(output_tokens), 0)::BIGINT AS \"output_tokens!\", \
             COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS \"cache_read_tokens!\", COALESCE(SUM(cache_write_tokens), 0)::BIGINT AS \"cache_write_tokens!\" \
             FROM request_log WHERE created_at >= $1 \
             GROUP BY model ORDER BY SUM(cost_microdollars) DESC",
        cutoff as i64,
    )
    .fetch_all(&conn)
    .await
    else {
        return Json(ModelBreakdownResponse {
            period: period_str.to_string(),
            models: Vec::new(),
        });
    };

    let mut models = Vec::new();
    for row in rows {
        models.push(ModelBreakdown {
            model: row.model,
            request_count: i64_to_u64(row.request_count),
            cost_microdollars: i64_to_u64(row.cost_microdollars),
            input_tokens: i64_to_u64(row.input_tokens),
            output_tokens: i64_to_u64(row.output_tokens),
            cache_read_tokens: i64_to_u64(row.cache_read_tokens),
            cache_write_tokens: i64_to_u64(row.cache_write_tokens),
        });
    }

    Json(ModelBreakdownResponse {
        period: period_str.to_string(),
        models,
    })
}

#[utoipa::path(
    get,
    path = "/usage-history/by-key",
    params(("period" = Option<String>, Query, description = "Period: 24h, 7d, or 30d")),
    responses(
        (status = 200, body = KeyBreakdownResponse),
    )
)]
pub async fn get_usage_history_by_key(
    axum::extract::Query(query): axum::extract::Query<UsageHistoryQuery>,
) -> Json<KeyBreakdownResponse> {
    let period_str = query.period.as_deref().unwrap_or("24h");
    let (cutoff_ms, _, _) = parse_period(period_str);
    let now = timestamp_millis();
    let cutoff = now.saturating_sub(cutoff_ms);

    let Ok(conn) = crate::db::get_conn().await else {
        return Json(KeyBreakdownResponse {
            period: period_str.to_string(),
            keys: Vec::new(),
        });
    };

    let Ok(rows) = sqlx::query!(
        "SELECT r.key_id, k.name AS \"key_name?\", COUNT(*) AS \"request_count!\", COALESCE(SUM(r.cost_microdollars), 0)::BIGINT AS \"cost_microdollars!\", \
             COALESCE(SUM(r.input_tokens), 0)::BIGINT AS \"input_tokens!\", COALESCE(SUM(r.output_tokens), 0)::BIGINT AS \"output_tokens!\", \
             COALESCE(SUM(r.cache_read_tokens), 0)::BIGINT AS \"cache_read_tokens!\", COALESCE(SUM(r.cache_write_tokens), 0)::BIGINT AS \"cache_write_tokens!\" \
             FROM request_log r LEFT JOIN client_keys k ON r.key_id = k.id \
             WHERE r.created_at >= $1 \
             GROUP BY r.key_id, k.name ORDER BY SUM(r.cost_microdollars) DESC",
        cutoff as i64,
    )
    .fetch_all(&conn)
    .await
    else {
        return Json(KeyBreakdownResponse {
            period: period_str.to_string(),
            keys: Vec::new(),
        });
    };

    let mut keys = Vec::new();
    for row in rows {
        keys.push(KeyBreakdown {
            key_id: row.key_id,
            key_name: row.key_name,
            request_count: i64_to_u64(row.request_count),
            cost_microdollars: i64_to_u64(row.cost_microdollars),
            input_tokens: i64_to_u64(row.input_tokens),
            output_tokens: i64_to_u64(row.output_tokens),
            cache_read_tokens: i64_to_u64(row.cache_read_tokens),
            cache_write_tokens: i64_to_u64(row.cache_write_tokens),
        });
    }

    Json(KeyBreakdownResponse {
        period: period_str.to_string(),
        keys,
    })
}

#[utoipa::path(
    delete,
    path = "/usage-history",
    responses(
        (status = 200, body = SuccessResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn delete_usage_history()
-> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    let conn = crate::db::get_conn().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    sqlx::query!("DELETE FROM request_log")
        .execute(&conn)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to clear usage history: {e}"),
                }),
            )
        })?;

    Ok(Json(SuccessResponse { success: true }))
}

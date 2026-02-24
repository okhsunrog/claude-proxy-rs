use axum::{Json, http::StatusCode};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{ErrorResponse, SuccessResponse};
use crate::auth::client_keys::get_u64;
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

    let Ok(mut rows) = conn
        .query(
            "SELECT (created_at / ?1) * ?1 AS bucket, \
             COUNT(*), SUM(cost_microdollars), \
             SUM(input_tokens), SUM(output_tokens), \
             SUM(cache_read_tokens), SUM(cache_write_tokens) \
             FROM request_log WHERE created_at >= ?2 \
             GROUP BY bucket ORDER BY bucket",
            (bucket_ms as i64, cutoff as i64),
        )
        .await
    else {
        return Json(TimeseriesResponse {
            period: period_str.to_string(),
            granularity: granularity.to_string(),
            points: Vec::new(),
        });
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

    let Ok(mut rows) = conn
        .query(
            "SELECT model, COUNT(*), SUM(cost_microdollars), \
             SUM(input_tokens), SUM(output_tokens), \
             SUM(cache_read_tokens), SUM(cache_write_tokens) \
             FROM request_log WHERE created_at >= ? \
             GROUP BY model ORDER BY SUM(cost_microdollars) DESC",
            [cutoff as i64],
        )
        .await
    else {
        return Json(ModelBreakdownResponse {
            period: period_str.to_string(),
            models: Vec::new(),
        });
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

    let Ok(mut rows) = conn
        .query(
            "SELECT r.key_id, k.name, COUNT(*), SUM(r.cost_microdollars), \
             SUM(r.input_tokens), SUM(r.output_tokens), \
             SUM(r.cache_read_tokens), SUM(r.cache_write_tokens) \
             FROM request_log r LEFT JOIN client_keys k ON r.key_id = k.id \
             WHERE r.created_at >= ? \
             GROUP BY r.key_id ORDER BY SUM(r.cost_microdollars) DESC",
            [cutoff as i64],
        )
        .await
    else {
        return Json(KeyBreakdownResponse {
            period: period_str.to_string(),
            keys: Vec::new(),
        });
    };

    let mut keys = Vec::new();
    while let Ok(Some(row)) = rows.next().await {
        let Ok(key_id) = row.get::<String>(0) else {
            continue;
        };
        keys.push(KeyBreakdown {
            key_id,
            key_name: row.get::<Option<String>>(1).ok().flatten(),
            request_count: get_u64(&row, 2),
            cost_microdollars: get_u64(&row, 3),
            input_tokens: get_u64(&row, 4),
            output_tokens: get_u64(&row, 5),
            cache_read_tokens: get_u64(&row, 6),
            cache_write_tokens: get_u64(&row, 7),
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

    conn.execute("DELETE FROM request_log", ())
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

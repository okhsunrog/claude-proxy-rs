use axum::{Json, extract::Query, http::StatusCode};
use serde::Deserialize;
use utoipa::ToSchema;

use super::{ErrorResponse, SuccessResponse};
use crate::db;
use crate::usage::history::{
    HistoryPeriod, KeyBreakdownResponse, KeyModelBreakdownResponse, ModelBreakdownResponse,
    TimeseriesResponse, by_key, by_key_model, by_model, timeseries,
};

// --- Types ---

#[derive(Deserialize, ToSchema, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct UsageHistoryQuery {
    /// Time period: "24h", "7d", or "30d"
    pub period: Option<String>,
    /// Restrict the result to a single client key. Absent = all keys.
    pub key_id: Option<String>,
}

// --- Handlers ---

#[utoipa::path(
    get,
    path = "/usage-history/timeseries",
    params(UsageHistoryQuery),
    responses(
        (status = 200, body = TimeseriesResponse),
    )
)]
pub async fn get_usage_history_timeseries(
    Query(query): Query<UsageHistoryQuery>,
) -> Json<TimeseriesResponse> {
    let period = HistoryPeriod::parse(query.period.as_deref());

    let Ok(conn) = db::get_conn().await else {
        return Json(period.empty_timeseries());
    };

    Json(
        timeseries(&conn, &period, query.key_id.as_deref())
            .await
            .unwrap_or_else(|_| period.empty_timeseries()),
    )
}

#[utoipa::path(
    get,
    path = "/usage-history/by-model",
    params(UsageHistoryQuery),
    responses(
        (status = 200, body = ModelBreakdownResponse),
    )
)]
pub async fn get_usage_history_by_model(
    Query(query): Query<UsageHistoryQuery>,
) -> Json<ModelBreakdownResponse> {
    let period = HistoryPeriod::parse(query.period.as_deref());

    let Ok(conn) = db::get_conn().await else {
        return Json(period.empty_models());
    };

    Json(
        by_model(&conn, &period, query.key_id.as_deref())
            .await
            .unwrap_or_else(|_| period.empty_models()),
    )
}

#[utoipa::path(
    get,
    path = "/usage-history/by-key",
    params(UsageHistoryQuery),
    responses(
        (status = 200, body = KeyBreakdownResponse),
    )
)]
pub async fn get_usage_history_by_key(
    Query(query): Query<UsageHistoryQuery>,
) -> Json<KeyBreakdownResponse> {
    let period = HistoryPeriod::parse(query.period.as_deref());

    let Ok(conn) = db::get_conn().await else {
        return Json(period.empty_keys());
    };

    Json(
        by_key(&conn, &period, query.key_id.as_deref())
            .await
            .unwrap_or_else(|_| period.empty_keys()),
    )
}

#[utoipa::path(
    get,
    path = "/usage-history/by-key-model",
    params(UsageHistoryQuery),
    responses(
        (status = 200, body = KeyModelBreakdownResponse),
    )
)]
pub async fn get_usage_history_by_key_model(
    Query(query): Query<UsageHistoryQuery>,
) -> Json<KeyModelBreakdownResponse> {
    let period = HistoryPeriod::parse(query.period.as_deref());

    let Ok(conn) = db::get_conn().await else {
        return Json(period.empty_key_models());
    };

    Json(
        by_key_model(&conn, &period, query.key_id.as_deref())
            .await
            .unwrap_or_else(|_| period.empty_key_models()),
    )
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
    let conn = db::get_conn().await.map_err(|e| {
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

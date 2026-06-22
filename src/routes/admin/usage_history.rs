use axum::{Json, http::StatusCode};
use serde::Deserialize;
use utoipa::ToSchema;

use super::{ErrorResponse, SuccessResponse};
use crate::usage::history::{
    HistoryPeriod, KeyBreakdownResponse, ModelBreakdownResponse, TimeseriesResponse,
};

// --- Types ---

#[derive(Deserialize, ToSchema)]
pub struct UsageHistoryQuery {
    /// Time period: "24h", "7d", or "30d"
    pub period: Option<String>,
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
    let period = HistoryPeriod::parse(query.period.as_deref());

    let Ok(conn) = crate::db::get_conn().await else {
        return Json(period.empty_timeseries());
    };

    Json(
        crate::usage::history::timeseries(&conn, &period, None)
            .await
            .unwrap_or_else(|_| period.empty_timeseries()),
    )
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
    let period = HistoryPeriod::parse(query.period.as_deref());

    let Ok(conn) = crate::db::get_conn().await else {
        return Json(period.empty_models());
    };

    Json(
        crate::usage::history::by_model(&conn, &period, None)
            .await
            .unwrap_or_else(|_| period.empty_models()),
    )
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
    let period = HistoryPeriod::parse(query.period.as_deref());

    let Ok(conn) = crate::db::get_conn().await else {
        return Json(period.empty_keys());
    };

    Json(
        crate::usage::history::by_key(&conn, &period)
            .await
            .unwrap_or_else(|_| period.empty_keys()),
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

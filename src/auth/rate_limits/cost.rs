use llm_relay::Usage;
use tracing::warn;

use super::windows::WindowState;
use crate::auth::client_keys::i64_to_u64;
use crate::db::Connection;
use crate::error::{DbResultExt, ProxyError};

/// Aggregate usage cost from request_log for a key across all three windows.
/// Returns (five_hour_cost, weekly_cost, total_cost) in microdollars.
pub(super) async fn aggregate_usage_costs(
    conn: &Connection,
    key_id: &str,
    ws: &WindowState,
) -> Result<(u64, u64, u64), ProxyError> {
    let min_from = ws
        .five_hour_count_from
        .min(ws.weekly_count_from)
        .min(ws.total_count_from);

    let row = sqlx::query!(
        "SELECT \
         COALESCE(SUM(CASE WHEN created_at >= $1 THEN cost_microdollars ELSE 0 END), 0)::BIGINT AS \"five_hour!\", \
         COALESCE(SUM(CASE WHEN created_at >= $2 THEN cost_microdollars ELSE 0 END), 0)::BIGINT AS \"weekly!\", \
         COALESCE(SUM(CASE WHEN created_at >= $3 THEN cost_microdollars ELSE 0 END), 0)::BIGINT AS \"total!\" \
         FROM request_log WHERE key_id = $4 AND created_at >= $5",
        ws.five_hour_count_from as i64,
        ws.weekly_count_from as i64,
        ws.total_count_from as i64,
        key_id,
        min_from as i64,
    )
    .fetch_one(conn)
    .await
    .db_context("Failed to aggregate usage")?;

    Ok((
        i64_to_u64(row.five_hour),
        i64_to_u64(row.weekly),
        i64_to_u64(row.total),
    ))
}

/// Query the sum of cost_microdollars from request_log for a specific key+model
/// where created_at >= the given threshold.
pub(super) async fn query_model_cost(
    conn: &Connection,
    key_id: &str,
    model: &str,
    from: u64,
) -> Result<u64, ProxyError> {
    let cost = sqlx::query_scalar!(
        "SELECT COALESCE(SUM(cost_microdollars), 0)::BIGINT AS \"cost!\" FROM request_log WHERE key_id = $1 AND model = $2 AND created_at >= $3",
        key_id,
        model,
        from as i64,
    )
    .fetch_one(conn)
    .await
    .db_context("Failed to query model cost")?;

    Ok(i64_to_u64(cost))
}

/// Look up model pricing and compute cost in microdollars.
/// Returns 0 if model is not found in the models table.
pub(super) async fn compute_cost(conn: &Connection, model: &str, report: &Usage) -> u64 {
    let Ok(row) = sqlx::query!(
        "SELECT input_price, output_price, cache_read_price, cache_write_price FROM models WHERE id = $1",
        model,
    )
    .fetch_optional(conn)
    .await
    else {
        warn!("Failed to look up pricing for model {model}, recording cost as 0");
        return 0;
    };

    let Some(row) = row else {
        warn!("Model {model} not found in models table, recording cost as 0");
        return 0;
    };

    let cost = report.input_tokens as f64 * row.input_price
        + report.output_tokens as f64 * row.output_price
        + report.cache_read_input_tokens.unwrap_or(0) as f64 * row.cache_read_price
        + report.cache_creation_input_tokens.unwrap_or(0) as f64 * row.cache_write_price;

    #[expect(
        clippy::cast_sign_loss,
        reason = "cost inputs are non-negative token counts and configured non-negative prices"
    )]
    {
        cost.round() as u64
    }
}

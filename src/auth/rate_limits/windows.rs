use crate::auth::client_keys::i64_to_u64;
use crate::db::Connection;
use crate::error::{DbResultExt, ProxyError};
use crate::usage::SubscriptionState;

/// Window boundary state read from client_keys.
pub(super) struct WindowState {
    pub(super) five_hour_count_from: u64,
    pub(super) weekly_count_from: u64,
    pub(super) total_count_from: u64,
}

/// Check and update window boundaries. When a window has expired, advances
/// the count_from timestamp and updates the reset_at from subscription state.
/// No counter zeroing: request_log queries use count_from as the lower bound.
pub(super) async fn maybe_reset_expired_windows(
    conn: &Connection,
    key_id: &str,
    now: u64,
    window_resets: &SubscriptionState,
) -> Result<WindowState, ProxyError> {
    let five_hour_ms: u64 = 5 * 60 * 60 * 1000;
    let one_week_ms: u64 = 7 * 24 * 60 * 60 * 1000;

    let row = sqlx::query!(
        "SELECT five_hour_reset_at, weekly_reset_at, five_hour_count_from, weekly_count_from, total_count_from FROM client_keys WHERE id = $1",
        key_id,
    )
    .fetch_optional(conn)
    .await
    .db_context("Failed to read window state")?;

    let Some(row) = row else {
        return Ok(WindowState {
            five_hour_count_from: 0,
            weekly_count_from: 0,
            total_count_from: 0,
        });
    };

    let mut five_hour_reset_at = i64_to_u64(row.five_hour_reset_at);
    let mut weekly_reset_at = i64_to_u64(row.weekly_reset_at);
    let mut five_hour_count_from = i64_to_u64(row.five_hour_count_from);
    let mut weekly_count_from = i64_to_u64(row.weekly_count_from);
    let total_count_from = i64_to_u64(row.total_count_from);

    let reset_five_hour = five_hour_reset_at > 0 && now >= five_hour_reset_at;
    let reset_weekly = weekly_reset_at > 0 && now >= weekly_reset_at;

    if !reset_five_hour && !reset_weekly {
        let new_five_hour = window_resets
            .five_hour_reset_at
            .filter(|&t| t > now && t < five_hour_reset_at)
            .unwrap_or(five_hour_reset_at);
        let new_weekly = window_resets
            .seven_day_reset_at
            .filter(|&t| t > now && t < weekly_reset_at)
            .unwrap_or(weekly_reset_at);

        if new_five_hour != five_hour_reset_at || new_weekly != weekly_reset_at {
            let _ = sqlx::query!(
                "UPDATE client_keys SET five_hour_reset_at = $1, weekly_reset_at = $2 WHERE id = $3",
                new_five_hour as i64,
                new_weekly as i64,
                key_id,
            )
            .execute(conn)
            .await;
        }

        return Ok(WindowState {
            five_hour_count_from,
            weekly_count_from,
            total_count_from,
        });
    }

    if reset_five_hour {
        five_hour_count_from = five_hour_reset_at;
        five_hour_reset_at = window_resets
            .five_hour_reset_at
            .filter(|&t| t > now)
            .unwrap_or(now + five_hour_ms);
    }
    if reset_weekly {
        weekly_count_from = weekly_reset_at;
        weekly_reset_at = window_resets
            .seven_day_reset_at
            .filter(|&t| t > now)
            .unwrap_or(now + one_week_ms);
    }

    sqlx::query!(
        "UPDATE client_keys SET five_hour_reset_at = $1, weekly_reset_at = $2, five_hour_count_from = $3, weekly_count_from = $4 WHERE id = $5",
        five_hour_reset_at as i64,
        weekly_reset_at as i64,
        five_hour_count_from as i64,
        weekly_count_from as i64,
        key_id,
    )
    .execute(conn)
    .await
    .db_context("Failed to update window state")?;

    Ok(WindowState {
        five_hour_count_from,
        weekly_count_from,
        total_count_from,
    })
}

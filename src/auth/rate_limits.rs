use llm_relay::Usage;
use serde::{Deserialize, Serialize};
use tracing::warn;
use turso::Connection;
use utoipa::ToSchema;

use super::client_keys::{
    ClientKeysStore, TokenLimits, TokenUsage, UsageResetType, get_u64, opt_u64,
};
use crate::SubscriptionState;
use crate::db;
use crate::error::ProxyError;
use crate::subscription::timestamp_millis;

// ============================================================================
// Structs
// ============================================================================

/// 4-type token breakdown for display
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenBreakdown {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

/// Per-model usage entry with limits and token breakdowns
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsageEntry {
    pub model: String,
    pub limits: TokenLimits,
    #[serde(rename = "fiveHour")]
    pub five_hour: TokenBreakdown,
    pub weekly: TokenBreakdown,
    pub total: TokenBreakdown,
    #[serde(rename = "fiveHourResetAt")]
    pub five_hour_reset_at: u64,
    pub weekly_reset_at: u64,
}

// ============================================================================
// Centralized helpers
// ============================================================================

/// Window boundary state read from client_keys.
struct WindowState {
    five_hour_count_from: u64,
    weekly_count_from: u64,
    total_count_from: u64,
}

/// Check and update window boundaries. When a window has expired, advances
/// the count_from timestamp and updates the reset_at from subscription state.
/// No counter zeroing — request_log queries use count_from as the lower bound.
///
/// Returns the current window state.
async fn maybe_reset_expired_windows(
    conn: &Connection,
    key_id: &str,
    now: u64,
    window_resets: &SubscriptionState,
) -> Result<WindowState, ProxyError> {
    let five_hour_ms: u64 = 5 * 60 * 60 * 1000;
    let one_week_ms: u64 = 7 * 24 * 60 * 60 * 1000;

    let mut rows = conn
        .query(
            "SELECT five_hour_reset_at, weekly_reset_at, five_hour_count_from, weekly_count_from, total_count_from FROM client_keys WHERE id = ?",
            [key_id],
        )
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to read window state: {e}")))?;

    let Some(row) = rows
        .next()
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to read window row: {e}")))?
    else {
        return Ok(WindowState {
            five_hour_count_from: 0,
            weekly_count_from: 0,
            total_count_from: 0,
        });
    };

    let mut five_hour_reset_at = get_u64(&row, 0);
    let mut weekly_reset_at = get_u64(&row, 1);
    let mut five_hour_count_from = get_u64(&row, 2);
    let mut weekly_count_from = get_u64(&row, 3);
    let total_count_from = get_u64(&row, 4);

    let reset_five_hour = five_hour_reset_at > 0 && now >= five_hour_reset_at;
    let reset_weekly = weekly_reset_at > 0 && now >= weekly_reset_at;

    if !reset_five_hour && !reset_weekly {
        // Re-sync: adopt subscription timestamps if they're earlier (without changing count_from)
        let new_five_hour = window_resets
            .five_hour_reset_at
            .filter(|&t| t > now && t < five_hour_reset_at)
            .unwrap_or(five_hour_reset_at);
        let new_weekly = window_resets
            .seven_day_reset_at
            .filter(|&t| t > now && t < weekly_reset_at)
            .unwrap_or(weekly_reset_at);

        if new_five_hour != five_hour_reset_at || new_weekly != weekly_reset_at {
            let _ = conn
                .execute(
                    "UPDATE client_keys SET five_hour_reset_at = ?, weekly_reset_at = ? WHERE id = ?",
                    (new_five_hour as i64, new_weekly as i64, key_id),
                )
                .await;
        }

        return Ok(WindowState {
            five_hour_count_from,
            weekly_count_from,
            total_count_from,
        });
    }

    // Window(s) expired: advance count_from to old reset_at (new window starts there)
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

    // Update client_keys with new count_from + reset_at values
    conn.execute(
        "UPDATE client_keys SET five_hour_reset_at = ?, weekly_reset_at = ?, five_hour_count_from = ?, weekly_count_from = ? WHERE id = ?",
        (
            five_hour_reset_at as i64,
            weekly_reset_at as i64,
            five_hour_count_from as i64,
            weekly_count_from as i64,
            key_id,
        ),
    )
    .await
    .map_err(|e| ProxyError::DatabaseError(format!("Failed to update window state: {e}")))?;

    Ok(WindowState {
        five_hour_count_from,
        weekly_count_from,
        total_count_from,
    })
}

/// Aggregate usage cost from request_log for a key across all three windows.
/// Returns (five_hour_cost, weekly_cost, total_cost) in microdollars.
async fn aggregate_usage_costs(
    conn: &Connection,
    key_id: &str,
    ws: &WindowState,
) -> Result<(u64, u64, u64), ProxyError> {
    let min_from = ws
        .five_hour_count_from
        .min(ws.weekly_count_from)
        .min(ws.total_count_from);

    let mut rows = conn
        .query(
            "SELECT \
             COALESCE(SUM(CASE WHEN created_at >= ?1 THEN cost_microdollars ELSE 0 END), 0), \
             COALESCE(SUM(CASE WHEN created_at >= ?2 THEN cost_microdollars ELSE 0 END), 0), \
             COALESCE(SUM(CASE WHEN created_at >= ?3 THEN cost_microdollars ELSE 0 END), 0) \
             FROM request_log WHERE key_id = ?4 AND created_at >= ?5",
            (
                ws.five_hour_count_from as i64,
                ws.weekly_count_from as i64,
                ws.total_count_from as i64,
                key_id,
                min_from as i64,
            ),
        )
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to aggregate usage: {e}")))?;

    let Some(row) = rows
        .next()
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to read aggregate row: {e}")))?
    else {
        return Ok((0, 0, 0));
    };

    let five_hour = row.get::<i64>(0).unwrap_or(0) as u64;
    let weekly = row.get::<i64>(1).unwrap_or(0) as u64;
    let total = row.get::<i64>(2).unwrap_or(0) as u64;

    Ok((five_hour, weekly, total))
}

/// Query the sum of cost_microdollars from request_log for a specific key+model
/// where created_at >= the given threshold.
async fn query_model_cost(
    conn: &Connection,
    key_id: &str,
    model: &str,
    from: u64,
) -> Result<u64, ProxyError> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(SUM(cost_microdollars), 0) FROM request_log WHERE key_id = ? AND model = ? AND created_at >= ?",
            (key_id, model, from as i64),
        )
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to query model cost: {e}")))?;

    let cost = rows
        .next()
        .await
        .ok()
        .flatten()
        .and_then(|r| r.get::<i64>(0).ok())
        .unwrap_or(0) as u64;

    Ok(cost)
}

/// Look up model pricing and compute cost in microdollars.
/// Returns 0 if model is not found in the models table.
async fn compute_cost(conn: &Connection, model: &str, report: &Usage) -> u64 {
    let Ok(mut rows) = conn
        .query(
            "SELECT input_price, output_price, cache_read_price, cache_write_price FROM models WHERE id = ?",
            [model],
        )
        .await
    else {
        warn!("Failed to look up pricing for model {model}, recording cost as 0");
        return 0;
    };

    let Some(row) = rows.next().await.ok().flatten() else {
        warn!("Model {model} not found in models table, recording cost as 0");
        return 0;
    };

    let input_price: f64 = row.get(0).unwrap_or(0.0);
    let output_price: f64 = row.get(1).unwrap_or(0.0);
    let cache_read_price: f64 = row.get(2).unwrap_or(0.0);
    let cache_write_price: f64 = row.get(3).unwrap_or(0.0);

    let cost = report.input_tokens as f64 * input_price
        + report.output_tokens as f64 * output_price
        + report.cache_read_input_tokens.unwrap_or(0) as f64 * cache_read_price
        + report.cache_creation_input_tokens.unwrap_or(0) as f64 * cache_write_price;

    cost.round() as u64
}

// ============================================================================
// Rate limiting, usage tracking, and model access methods on ClientKeysStore
// ============================================================================

impl ClientKeysStore {
    /// Check if a key's usage is within limits.
    /// Derives global usage from request_log aggregation.
    pub async fn check_limits(
        &self,
        id: &str,
        window_resets: &SubscriptionState,
    ) -> Result<(), String> {
        let now = timestamp_millis();
        let conn = db::get_conn().await.map_err(|e| e.to_string())?;

        // Update window boundaries
        let ws = maybe_reset_expired_windows(&conn, id, now, window_resets)
            .await
            .map_err(|e| e.to_string())?;

        // Read limits
        let mut rows = conn
            .query(
                "SELECT five_hour_limit, weekly_limit, total_limit FROM client_keys WHERE id = ?",
                [id],
            )
            .await
            .map_err(|e| format!("DB error: {e}"))?;

        let row = rows
            .next()
            .await
            .map_err(|e| format!("DB error: {e}"))?
            .ok_or_else(|| "Key not found".to_string())?;

        let five_hour_limit = opt_u64(&row, 0);
        let weekly_limit = opt_u64(&row, 1);
        let total_limit = opt_u64(&row, 2);

        // Skip aggregation if no limits are set
        if five_hour_limit.is_none() && weekly_limit.is_none() && total_limit.is_none() {
            return Ok(());
        }

        // Aggregate usage from request_log
        let (five_hour_cost, weekly_cost, total_cost) = aggregate_usage_costs(&conn, id, &ws)
            .await
            .map_err(|e| e.to_string())?;

        if let Some(limit) = five_hour_limit
            && five_hour_cost >= limit
        {
            return Err(format!(
                "5-hour token limit exceeded ({}/{})",
                five_hour_cost, limit
            ));
        }

        if let Some(limit) = weekly_limit
            && weekly_cost >= limit
        {
            return Err(format!(
                "Weekly token limit exceeded ({}/{})",
                weekly_cost, limit
            ));
        }

        if let Some(limit) = total_limit
            && total_cost >= limit
        {
            return Err(format!(
                "Total token limit exceeded ({}/{})",
                total_cost, limit
            ));
        }

        Ok(())
    }

    /// Record usage by inserting into request_log.
    /// Window boundaries are updated via maybe_reset_expired_windows.
    pub async fn record_model_usage(
        &self,
        key_id: &str,
        model: &str,
        report: &Usage,
        window_resets: &SubscriptionState,
    ) -> Result<(), ProxyError> {
        let now = timestamp_millis();
        let conn = db::get_conn().await?;

        // Update window boundaries
        maybe_reset_expired_windows(&conn, key_id, now, window_resets).await?;

        // Initialize reset timestamps if not yet set
        let mut rows = conn
            .query(
                "SELECT five_hour_reset_at, weekly_reset_at FROM client_keys WHERE id = ?",
                [key_id],
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to read timestamps: {e}")))?;

        if let Some(row) = rows
            .next()
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to read timestamp row: {e}")))?
        {
            let five_hour_reset_at = get_u64(&row, 0);
            let weekly_reset_at = get_u64(&row, 1);

            let mut needs_init = false;
            let new_five_hour = if five_hour_reset_at == 0 {
                needs_init = true;
                window_resets
                    .five_hour_reset_at
                    .filter(|&t| t > now)
                    .unwrap_or(0)
            } else {
                five_hour_reset_at
            };
            let new_weekly = if weekly_reset_at == 0 {
                needs_init = true;
                window_resets
                    .seven_day_reset_at
                    .filter(|&t| t > now)
                    .unwrap_or(0)
            } else {
                weekly_reset_at
            };

            if needs_init {
                let _ = conn
                    .execute(
                        "UPDATE client_keys SET five_hour_reset_at = ?, weekly_reset_at = ? WHERE id = ?",
                        (new_five_hour as i64, new_weekly as i64, key_id),
                    )
                    .await;
            }
        }

        // Compute cost using model pricing
        let cost = compute_cost(&conn, model, report).await;

        // Single INSERT into request_log
        conn.execute(
            "INSERT INTO request_log (key_id, model, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, cost_microdollars, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            (
                key_id,
                model,
                report.input_tokens as i64,
                report.output_tokens as i64,
                report.cache_read_input_tokens.unwrap_or(0) as i64,
                report.cache_creation_input_tokens.unwrap_or(0) as i64,
                cost as i64,
                now as i64,
            ),
        )
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to insert request log: {e}")))?;

        Ok(())
    }

    /// Get usage statistics for a key (derived from request_log aggregation)
    pub async fn get_usage(
        &self,
        id: &str,
    ) -> Result<Option<(TokenLimits, TokenUsage)>, ProxyError> {
        let now = timestamp_millis();
        let Some(key) = self.get(id).await? else {
            return Ok(None);
        };
        let conn = db::get_conn().await?;

        // Read count_from values
        let mut rows = conn
            .query(
                "SELECT five_hour_count_from, weekly_count_from, total_count_from FROM client_keys WHERE id = ?",
                [id],
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to read count_from: {e}")))?;
        let Some(count_from_row) = rows.next().await.map_err(|e| {
            ProxyError::DatabaseError(format!("Failed to read count_from row: {e}"))
        })?
        else {
            return Ok(None);
        };
        let five_hour_count_from = get_u64(&count_from_row, 0);
        let weekly_count_from = get_u64(&count_from_row, 1);
        let total_count_from = get_u64(&count_from_row, 2);

        let mut five_hour_reset_at = key.usage.five_hour_reset_at;
        let mut weekly_reset_at = key.usage.weekly_reset_at;

        // Show 0 if windows have expired (read-only view)
        let five_hour_expired = five_hour_reset_at > 0 && now >= five_hour_reset_at;
        let weekly_expired = weekly_reset_at > 0 && now >= weekly_reset_at;

        if five_hour_expired {
            five_hour_reset_at = 0;
        }
        if weekly_expired {
            weekly_reset_at = 0;
        }

        let ws = WindowState {
            five_hour_count_from,
            weekly_count_from,
            total_count_from,
        };

        let (five_hour_cost, weekly_cost, total_cost) =
            aggregate_usage_costs(&conn, id, &ws).await?;

        Ok(Some((
            key.limits,
            TokenUsage {
                five_hour_tokens: if five_hour_expired { 0 } else { five_hour_cost },
                five_hour_reset_at,
                weekly_tokens: if weekly_expired { 0 } else { weekly_cost },
                weekly_reset_at,
                total_tokens: total_cost,
            },
        )))
    }

    /// Reset usage for a key by advancing count_from timestamps.
    /// Historical data in request_log is preserved.
    pub async fn reset_usage(
        &self,
        id: &str,
        reset_type: UsageResetType,
    ) -> Result<bool, ProxyError> {
        let now = timestamp_millis();
        let conn = db::get_conn().await?;

        let sql = match reset_type {
            UsageResetType::FiveHour => {
                "UPDATE client_keys SET five_hour_count_from = ?, five_hour_reset_at = 0 WHERE id = ?"
            }
            UsageResetType::Weekly => {
                "UPDATE client_keys SET weekly_count_from = ?, weekly_reset_at = 0 WHERE id = ?"
            }
            UsageResetType::Total => "UPDATE client_keys SET total_count_from = ? WHERE id = ?",
            UsageResetType::All => {
                "UPDATE client_keys SET five_hour_count_from = ?, weekly_count_from = ?, total_count_from = ?, five_hour_reset_at = 0, weekly_reset_at = 0 WHERE id = ?"
            }
        };

        let affected = match reset_type {
            UsageResetType::FiveHour | UsageResetType::Weekly | UsageResetType::Total => conn
                .execute(sql, (now as i64, id))
                .await
                .map_err(|e| ProxyError::DatabaseError(format!("Failed to reset usage: {e}")))?,
            UsageResetType::All => conn
                .execute(sql, (now as i64, now as i64, now as i64, id))
                .await
                .map_err(|e| ProxyError::DatabaseError(format!("Failed to reset usage: {e}")))?,
        };

        Ok(affected > 0)
    }

    // ========================================================================
    // Per-key model access (key_allowed_models table)
    // ========================================================================

    /// Get allowed models for a key. Empty vec means "all models allowed".
    pub async fn get_allowed_models(&self, key_id: &str) -> Result<Vec<String>, ProxyError> {
        let conn = db::get_conn().await?;
        let mut rows = conn
            .query(
                "SELECT model FROM key_allowed_models WHERE key_id = ?",
                [key_id],
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to get allowed models: {e}")))?;
        let mut models = Vec::new();
        while let Ok(Some(row)) = rows.next().await {
            if let Ok(model) = row.get::<String>(0) {
                models.push(model);
            }
        }
        Ok(models)
    }

    /// Set allowed models for a key. Empty vec = allow all.
    pub async fn set_allowed_models(
        &self,
        key_id: &str,
        models: Vec<String>,
    ) -> Result<(), ProxyError> {
        let conn = db::get_conn().await?;
        conn.execute("DELETE FROM key_allowed_models WHERE key_id = ?", [key_id])
            .await
            .map_err(|e| {
                ProxyError::DatabaseError(format!("Failed to clear allowed models: {e}"))
            })?;

        for model in &models {
            conn.execute(
                "INSERT INTO key_allowed_models (key_id, model) VALUES (?, ?)",
                (key_id, model.as_str()),
            )
            .await
            .map_err(|e| {
                ProxyError::DatabaseError(format!("Failed to insert allowed model: {e}"))
            })?;
        }
        Ok(())
    }

    /// Check if a specific model is allowed for a key.
    /// If no rows exist in key_allowed_models for this key, all models are allowed.
    pub async fn is_model_allowed(&self, key_id: &str, model: &str) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;
        // Count total allowed models for this key
        let mut rows = conn
            .query(
                "SELECT COUNT(*) FROM key_allowed_models WHERE key_id = ?",
                [key_id],
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to check model access: {e}")))?;
        let total: i64 = rows
            .next()
            .await
            .ok()
            .flatten()
            .and_then(|r| r.get::<i64>(0).ok())
            .unwrap_or(0);

        if total == 0 {
            return Ok(true); // No whitelist = allow all
        }

        // Check if this specific model is in the whitelist
        let mut rows = conn
            .query(
                "SELECT COUNT(*) FROM key_allowed_models WHERE key_id = ? AND model = ?",
                (key_id, model),
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to check model access: {e}")))?;
        let count = rows
            .next()
            .await
            .ok()
            .flatten()
            .and_then(|r| r.get::<i64>(0).ok())
            .unwrap_or(0);
        Ok(count > 0)
    }

    // ========================================================================
    // Per-key per-model usage tracking (key_model_limits table + request_log)
    // ========================================================================

    /// Check per-model limits for a key. Returns Ok(()) if no limits set.
    /// Computes cost from request_log aggregation.
    pub async fn check_model_limits(
        &self,
        key_id: &str,
        model: &str,
        window_resets: &SubscriptionState,
    ) -> Result<(), String> {
        let now = timestamp_millis();
        let conn = db::get_conn().await.map_err(|e| e.to_string())?;

        // Update window boundaries
        let ws = maybe_reset_expired_windows(&conn, key_id, now, window_resets)
            .await
            .map_err(|e| e.to_string())?;

        let mut rows = conn
            .query(
                "SELECT five_hour_limit, weekly_limit, total_limit, count_from FROM key_model_limits WHERE key_id = ? AND model = ?",
                (key_id, model),
            )
            .await
            .map_err(|e| format!("DB error: {e}"))?;

        let Some(row) = rows.next().await.map_err(|e| format!("DB error: {e}"))? else {
            return Ok(()); // No row = no limits
        };

        let five_hour_limit = opt_u64(&row, 0);
        let weekly_limit = opt_u64(&row, 1);
        let total_limit = opt_u64(&row, 2);
        let model_count_from = get_u64(&row, 3);

        // Apply per-model count_from as a floor for all windows
        let five_hour_from = ws.five_hour_count_from.max(model_count_from);
        let weekly_from = ws.weekly_count_from.max(model_count_from);
        let total_from = ws.total_count_from.max(model_count_from);

        if let Some(limit) = five_hour_limit {
            let cost = query_model_cost(&conn, key_id, model, five_hour_from)
                .await
                .map_err(|e| e.to_string())?;
            if cost >= limit {
                return Err(format!(
                    "5-hour model limit exceeded for {model} (${:.2}/${:.2})",
                    cost as f64 / 1_000_000.0,
                    limit as f64 / 1_000_000.0
                ));
            }
        }

        if let Some(limit) = weekly_limit {
            let cost = query_model_cost(&conn, key_id, model, weekly_from)
                .await
                .map_err(|e| e.to_string())?;
            if cost >= limit {
                return Err(format!(
                    "Weekly model limit exceeded for {model} (${:.2}/${:.2})",
                    cost as f64 / 1_000_000.0,
                    limit as f64 / 1_000_000.0
                ));
            }
        }

        if let Some(limit) = total_limit {
            let cost = query_model_cost(&conn, key_id, model, total_from)
                .await
                .map_err(|e| e.to_string())?;
            if cost >= limit {
                return Err(format!(
                    "Total model limit exceeded for {model} (${:.2}/${:.2})",
                    cost as f64 / 1_000_000.0,
                    limit as f64 / 1_000_000.0
                ));
            }
        }

        Ok(())
    }

    /// Get per-model usage entries for a key (from request_log + key_model_limits)
    pub async fn get_model_usage(&self, key_id: &str) -> Result<Vec<ModelUsageEntry>, ProxyError> {
        let now = timestamp_millis();
        let conn = db::get_conn().await?;

        // Read window state from client_keys
        let mut ts_rows = conn
            .query(
                "SELECT five_hour_reset_at, weekly_reset_at, five_hour_count_from, weekly_count_from, total_count_from FROM client_keys WHERE id = ?",
                [key_id],
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to read window state: {e}")))?;
        let Some(ts_row) = ts_rows
            .next()
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to read window row: {e}")))?
        else {
            return Ok(Vec::new());
        };
        let five_hour_reset_at = get_u64(&ts_row, 0);
        let weekly_reset_at = get_u64(&ts_row, 1);
        let five_hour_count_from = get_u64(&ts_row, 2);
        let weekly_count_from = get_u64(&ts_row, 3);
        let total_count_from = get_u64(&ts_row, 4);

        let five_hour_expired = five_hour_reset_at > 0 && now >= five_hour_reset_at;
        let weekly_expired = weekly_reset_at > 0 && now >= weekly_reset_at;

        let effective_five_hour = if five_hour_expired {
            now
        } else {
            five_hour_count_from
        };
        let effective_weekly = if weekly_expired {
            now
        } else {
            weekly_count_from
        };

        // Read per-model limits and count_from
        let mut limit_rows = conn
            .query(
                "SELECT model, five_hour_limit, weekly_limit, total_limit, count_from FROM key_model_limits WHERE key_id = ?",
                [key_id],
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to read model limits: {e}")))?;

        let mut model_limits: Vec<(String, TokenLimits, u64)> = Vec::new();
        while let Ok(Some(row)) = limit_rows.next().await {
            let Ok(model) = row.get::<String>(0) else {
                continue;
            };
            model_limits.push((
                model,
                TokenLimits {
                    five_hour_limit: opt_u64(&row, 1),
                    weekly_limit: opt_u64(&row, 2),
                    total_limit: opt_u64(&row, 3),
                },
                get_u64(&row, 4),
            ));
        }

        // Get the minimum count_from across all windows for the broad query
        let min_from = effective_five_hour
            .min(effective_weekly)
            .min(total_count_from);

        // Query aggregated usage from request_log grouped by model
        let mut usage_rows = conn
            .query(
                "SELECT model, \
                     SUM(CASE WHEN created_at >= ?1 THEN input_tokens ELSE 0 END), \
                     SUM(CASE WHEN created_at >= ?1 THEN output_tokens ELSE 0 END), \
                     SUM(CASE WHEN created_at >= ?1 THEN cache_read_tokens ELSE 0 END), \
                     SUM(CASE WHEN created_at >= ?1 THEN cache_write_tokens ELSE 0 END), \
                     SUM(CASE WHEN created_at >= ?2 THEN input_tokens ELSE 0 END), \
                     SUM(CASE WHEN created_at >= ?2 THEN output_tokens ELSE 0 END), \
                     SUM(CASE WHEN created_at >= ?2 THEN cache_read_tokens ELSE 0 END), \
                     SUM(CASE WHEN created_at >= ?2 THEN cache_write_tokens ELSE 0 END), \
                     SUM(CASE WHEN created_at >= ?3 THEN input_tokens ELSE 0 END), \
                     SUM(CASE WHEN created_at >= ?3 THEN output_tokens ELSE 0 END), \
                     SUM(CASE WHEN created_at >= ?3 THEN cache_read_tokens ELSE 0 END), \
                     SUM(CASE WHEN created_at >= ?3 THEN cache_write_tokens ELSE 0 END) \
                     FROM request_log WHERE key_id = ?4 AND created_at >= ?5 GROUP BY model",
                (
                    effective_five_hour as i64,
                    effective_weekly as i64,
                    total_count_from as i64,
                    key_id,
                    min_from as i64,
                ),
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to query model usage: {e}")))?;

        // Collect usage data from request_log
        let mut usage_map: std::collections::HashMap<
            String,
            (TokenBreakdown, TokenBreakdown, TokenBreakdown),
        > = std::collections::HashMap::new();
        while let Ok(Some(row)) = usage_rows.next().await {
            let Ok(model) = row.get::<String>(0) else {
                continue;
            };
            usage_map.insert(
                model,
                (
                    TokenBreakdown {
                        input: get_u64(&row, 1),
                        output: get_u64(&row, 2),
                        cache_read: get_u64(&row, 3),
                        cache_write: get_u64(&row, 4),
                    },
                    TokenBreakdown {
                        input: get_u64(&row, 5),
                        output: get_u64(&row, 6),
                        cache_read: get_u64(&row, 7),
                        cache_write: get_u64(&row, 8),
                    },
                    TokenBreakdown {
                        input: get_u64(&row, 9),
                        output: get_u64(&row, 10),
                        cache_read: get_u64(&row, 11),
                        cache_write: get_u64(&row, 12),
                    },
                ),
            );
        }

        // Merge: models from limits + models from usage
        let mut all_models: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (model, _, _) in &model_limits {
            all_models.insert(model.clone());
        }
        for model in usage_map.keys() {
            all_models.insert(model.clone());
        }

        let mut entries = Vec::new();
        for model in all_models {
            let limits = model_limits
                .iter()
                .find(|(m, _, _)| m == &model)
                .map(|(_, l, _)| l.clone())
                .unwrap_or_default();

            let (five_hour, weekly, total) = usage_map.remove(&model).unwrap_or_default();

            entries.push(ModelUsageEntry {
                model,
                limits,
                five_hour,
                weekly,
                total,
                five_hour_reset_at: if five_hour_expired {
                    0
                } else {
                    five_hour_reset_at
                },
                weekly_reset_at: if weekly_expired { 0 } else { weekly_reset_at },
            });
        }
        Ok(entries)
    }

    /// Set per-model limits for a key (UPSERT into key_model_limits)
    pub async fn set_model_limits(
        &self,
        key_id: &str,
        model: &str,
        limits: TokenLimits,
    ) -> Result<(), ProxyError> {
        let conn = db::get_conn().await?;

        // Check if row exists
        let mut rows = conn
            .query(
                "SELECT COUNT(*) FROM key_model_limits WHERE key_id = ? AND model = ?",
                (key_id, model),
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to check model limits: {e}")))?;

        let exists: i64 = rows
            .next()
            .await
            .ok()
            .flatten()
            .and_then(|r| r.get::<i64>(0).ok())
            .unwrap_or(0);

        let h = limits.five_hour_limit.map(|v| v as i64);
        let w = limits.weekly_limit.map(|v| v as i64);
        let t = limits.total_limit.map(|v| v as i64);

        if exists > 0 {
            conn.execute(
                "UPDATE key_model_limits SET five_hour_limit = ?, weekly_limit = ?, total_limit = ? WHERE key_id = ? AND model = ?",
                (h, w, t, key_id, model),
            )
            .await
            .map_err(|e| {
                ProxyError::DatabaseError(format!("Failed to update model limits: {e}"))
            })?;
        } else {
            conn.execute(
                "INSERT INTO key_model_limits (key_id, model, five_hour_limit, weekly_limit, total_limit) VALUES (?, ?, ?, ?, ?)",
                (key_id, model, h, w, t),
            )
            .await
            .map_err(|e| {
                ProxyError::DatabaseError(format!("Failed to insert model limits: {e}"))
            })?;
        }
        Ok(())
    }

    /// Remove per-model limits for a key
    pub async fn remove_model_limits(&self, key_id: &str, model: &str) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;
        let affected = conn
            .execute(
                "DELETE FROM key_model_limits WHERE key_id = ? AND model = ?",
                (key_id, model),
            )
            .await
            .map_err(|e| {
                ProxyError::DatabaseError(format!("Failed to remove model limits: {e}"))
            })?;
        Ok(affected > 0)
    }

    /// Reset per-model usage by advancing count_from in key_model_limits.
    /// Historical data in request_log is preserved.
    pub async fn reset_model_usage(
        &self,
        key_id: &str,
        model: &str,
        _reset_type: UsageResetType,
    ) -> Result<bool, ProxyError> {
        let now = timestamp_millis();
        let conn = db::get_conn().await?;

        // Single count_from per model — resets all windows for this model
        let affected = conn
            .execute(
                "UPDATE key_model_limits SET count_from = ? WHERE key_id = ? AND model = ?",
                (now as i64, key_id, model),
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to reset model usage: {e}")))?;
        Ok(affected > 0)
    }
}

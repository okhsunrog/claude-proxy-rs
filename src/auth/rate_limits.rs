use llm_relay::Usage;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::client_keys::{
    ClientKeysStore, TokenLimits, TokenUsage, UsageResetType, i64_to_u64, opt_i64_to_u64,
};
use crate::db;
use crate::error::{DbResultExt, ProxyError};
use crate::subscription::timestamp_millis;
use crate::usage::SubscriptionState;

mod cost;
mod windows;

use cost::{aggregate_usage_costs, compute_cost, query_model_cost};
use windows::{WindowState, maybe_reset_expired_windows};

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
        let row = sqlx::query!(
            "SELECT five_hour_limit, weekly_limit, total_limit FROM client_keys WHERE id = $1",
            id,
        )
        .fetch_optional(&conn)
        .await
        .map_err(|e| format!("DB error: {e}"))?
        .ok_or_else(|| "Key not found".to_string())?;

        let five_hour_limit = opt_i64_to_u64(row.five_hour_limit);
        let weekly_limit = opt_i64_to_u64(row.weekly_limit);
        let total_limit = opt_i64_to_u64(row.total_limit);

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
        let row = sqlx::query!(
            "SELECT five_hour_reset_at, weekly_reset_at FROM client_keys WHERE id = $1",
            key_id,
        )
        .fetch_optional(&conn)
        .await
        .db_context("Failed to read timestamps")?;

        if let Some(row) = row {
            let five_hour_reset_at = i64_to_u64(row.five_hour_reset_at);
            let weekly_reset_at = i64_to_u64(row.weekly_reset_at);

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
                let _ = sqlx::query!(
                    "UPDATE client_keys SET five_hour_reset_at = $1, weekly_reset_at = $2 WHERE id = $3",
                    new_five_hour as i64,
                    new_weekly as i64,
                    key_id,
                )
                .execute(&conn)
                    .await;
            }
        }

        // Compute cost using model pricing
        let cost = compute_cost(&conn, model, report).await;

        // Single INSERT into request_log
        sqlx::query!(
            "INSERT INTO request_log (key_id, model, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, cost_microdollars, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            key_id,
            model,
            report.input_tokens as i64,
            report.output_tokens as i64,
            report.cache_read_input_tokens.unwrap_or(0) as i64,
            report.cache_creation_input_tokens.unwrap_or(0) as i64,
            cost as i64,
            now as i64,
        )
        .execute(&conn)
        .await
        .db_context("Failed to insert request log")?;

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
        let count_from_row = sqlx::query!(
            "SELECT five_hour_count_from, weekly_count_from, total_count_from FROM client_keys WHERE id = $1",
            id,
        )
        .fetch_optional(&conn)
        .await
        .db_context("Failed to read count_from")?;
        let Some(count_from_row) = count_from_row else {
            return Ok(None);
        };
        let five_hour_count_from = i64_to_u64(count_from_row.five_hour_count_from);
        let weekly_count_from = i64_to_u64(count_from_row.weekly_count_from);
        let total_count_from = i64_to_u64(count_from_row.total_count_from);

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

        let affected = match reset_type {
            UsageResetType::FiveHour => sqlx::query!(
                "UPDATE client_keys SET five_hour_count_from = $1, five_hour_reset_at = 0 WHERE id = $2",
                now as i64,
                id,
            )
            .execute(&conn)
            .await
            .db_context("Failed to reset usage")?
            .rows_affected(),
            UsageResetType::Weekly => sqlx::query!(
                "UPDATE client_keys SET weekly_count_from = $1, weekly_reset_at = 0 WHERE id = $2",
                now as i64,
                id,
            )
            .execute(&conn)
            .await
            .db_context("Failed to reset usage")?
            .rows_affected(),
            UsageResetType::Total => sqlx::query!(
                "UPDATE client_keys SET total_count_from = $1 WHERE id = $2",
                now as i64,
                id,
            )
            .execute(&conn)
            .await
            .db_context("Failed to reset usage")?
            .rows_affected(),
            UsageResetType::All => sqlx::query!(
                "UPDATE client_keys SET five_hour_count_from = $1, weekly_count_from = $2, total_count_from = $3, five_hour_reset_at = 0, weekly_reset_at = 0 WHERE id = $4",
                now as i64,
                now as i64,
                now as i64,
                id,
            )
                .execute(&conn)
                .await
                .db_context("Failed to reset usage")?
                .rows_affected(),
        };

        Ok(affected > 0)
    }

    // ========================================================================
    // Per-key model access (key_allowed_models table)
    // ========================================================================

    /// Get allowed models for a key. Empty vec means "all models allowed".
    pub async fn get_allowed_models(&self, key_id: &str) -> Result<Vec<String>, ProxyError> {
        let conn = db::get_conn().await?;
        let rows = sqlx::query!(
            "SELECT model FROM key_allowed_models WHERE key_id = $1",
            key_id
        )
        .fetch_all(&conn)
        .await
        .db_context("Failed to get allowed models")?;
        Ok(rows.into_iter().map(|row| row.model).collect())
    }

    /// Set allowed models for a key. Empty vec = allow all.
    pub async fn set_allowed_models(
        &self,
        key_id: &str,
        models: Vec<String>,
    ) -> Result<(), ProxyError> {
        let conn = db::get_conn().await?;
        sqlx::query!("DELETE FROM key_allowed_models WHERE key_id = $1", key_id)
            .execute(&conn)
            .await
            .db_context("Failed to clear allowed models")?;

        for model in &models {
            sqlx::query!(
                "INSERT INTO key_allowed_models (key_id, model) VALUES ($1, $2)",
                key_id,
                model.as_str(),
            )
            .execute(&conn)
            .await
            .db_context("Failed to insert allowed model")?;
        }
        Ok(())
    }

    /// Check if a specific model is allowed for a key.
    /// If no rows exist in key_allowed_models for this key, all models are allowed.
    pub async fn is_model_allowed(&self, key_id: &str, model: &str) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;
        // Count total allowed models for this key
        let total = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM key_allowed_models WHERE key_id = $1",
            key_id
        )
        .fetch_one(&conn)
        .await
        .db_context("Failed to check model access")?;
        let total = total.unwrap_or(0);

        if total == 0 {
            return Ok(true); // No whitelist = allow all
        }

        // Check if this specific model is in the whitelist
        let count = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM key_allowed_models WHERE key_id = $1 AND model = $2",
            key_id,
            model,
        )
        .fetch_one(&conn)
        .await
        .db_context("Failed to check model access")?;
        let count = count.unwrap_or(0);
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

        let row = sqlx::query!(
            "SELECT five_hour_limit, weekly_limit, total_limit, count_from FROM key_model_limits WHERE key_id = $1 AND model = $2",
            key_id,
            model,
        )
        .fetch_optional(&conn)
        .await
        .map_err(|e| format!("DB error: {e}"))?;

        let Some(row) = row else {
            return Ok(()); // No row = no limits
        };

        let five_hour_limit = opt_i64_to_u64(row.five_hour_limit);
        let weekly_limit = opt_i64_to_u64(row.weekly_limit);
        let total_limit = opt_i64_to_u64(row.total_limit);
        let model_count_from = i64_to_u64(row.count_from);

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
        let ts_row = sqlx::query!(
            "SELECT five_hour_reset_at, weekly_reset_at, five_hour_count_from, weekly_count_from, total_count_from FROM client_keys WHERE id = $1",
            key_id,
        )
        .fetch_optional(&conn)
        .await
        .db_context("Failed to read window state")?;
        let Some(ts_row) = ts_row else {
            return Ok(Vec::new());
        };
        let five_hour_reset_at = i64_to_u64(ts_row.five_hour_reset_at);
        let weekly_reset_at = i64_to_u64(ts_row.weekly_reset_at);
        let five_hour_count_from = i64_to_u64(ts_row.five_hour_count_from);
        let weekly_count_from = i64_to_u64(ts_row.weekly_count_from);
        let total_count_from = i64_to_u64(ts_row.total_count_from);

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
        let limit_rows = sqlx::query!(
            "SELECT model, five_hour_limit, weekly_limit, total_limit, count_from FROM key_model_limits WHERE key_id = $1",
            key_id,
        )
        .fetch_all(&conn)
        .await
        .db_context("Failed to read model limits")?;

        let mut model_limits: Vec<(String, TokenLimits, u64)> = Vec::new();
        for row in limit_rows {
            model_limits.push((
                row.model,
                TokenLimits {
                    five_hour_limit: opt_i64_to_u64(row.five_hour_limit),
                    weekly_limit: opt_i64_to_u64(row.weekly_limit),
                    total_limit: opt_i64_to_u64(row.total_limit),
                },
                i64_to_u64(row.count_from),
            ));
        }

        // Get the minimum count_from across all windows for the broad query
        let min_from = effective_five_hour
            .min(effective_weekly)
            .min(total_count_from);

        // Query aggregated usage from request_log grouped by model
        let usage_rows = sqlx::query!(
            "SELECT model, \
                 COALESCE(SUM(CASE WHEN created_at >= $1 THEN input_tokens ELSE 0 END), 0)::BIGINT AS \"five_hour_input!\", \
                 COALESCE(SUM(CASE WHEN created_at >= $1 THEN output_tokens ELSE 0 END), 0)::BIGINT AS \"five_hour_output!\", \
                 COALESCE(SUM(CASE WHEN created_at >= $1 THEN cache_read_tokens ELSE 0 END), 0)::BIGINT AS \"five_hour_cache_read!\", \
                 COALESCE(SUM(CASE WHEN created_at >= $1 THEN cache_write_tokens ELSE 0 END), 0)::BIGINT AS \"five_hour_cache_write!\", \
                 COALESCE(SUM(CASE WHEN created_at >= $2 THEN input_tokens ELSE 0 END), 0)::BIGINT AS \"weekly_input!\", \
                 COALESCE(SUM(CASE WHEN created_at >= $2 THEN output_tokens ELSE 0 END), 0)::BIGINT AS \"weekly_output!\", \
                 COALESCE(SUM(CASE WHEN created_at >= $2 THEN cache_read_tokens ELSE 0 END), 0)::BIGINT AS \"weekly_cache_read!\", \
                 COALESCE(SUM(CASE WHEN created_at >= $2 THEN cache_write_tokens ELSE 0 END), 0)::BIGINT AS \"weekly_cache_write!\", \
                 COALESCE(SUM(CASE WHEN created_at >= $3 THEN input_tokens ELSE 0 END), 0)::BIGINT AS \"total_input!\", \
                 COALESCE(SUM(CASE WHEN created_at >= $3 THEN output_tokens ELSE 0 END), 0)::BIGINT AS \"total_output!\", \
                 COALESCE(SUM(CASE WHEN created_at >= $3 THEN cache_read_tokens ELSE 0 END), 0)::BIGINT AS \"total_cache_read!\", \
                 COALESCE(SUM(CASE WHEN created_at >= $3 THEN cache_write_tokens ELSE 0 END), 0)::BIGINT AS \"total_cache_write!\" \
                 FROM request_log WHERE key_id = $4 AND created_at >= $5 GROUP BY model",
            effective_five_hour as i64,
            effective_weekly as i64,
            total_count_from as i64,
            key_id,
            min_from as i64,
        )
        .fetch_all(&conn)
        .await
        .db_context("Failed to query model usage")?;

        // Collect usage data from request_log
        let mut usage_map: std::collections::HashMap<
            String,
            (TokenBreakdown, TokenBreakdown, TokenBreakdown),
        > = std::collections::HashMap::new();
        for row in usage_rows {
            usage_map.insert(
                row.model,
                (
                    TokenBreakdown {
                        input: i64_to_u64(row.five_hour_input),
                        output: i64_to_u64(row.five_hour_output),
                        cache_read: i64_to_u64(row.five_hour_cache_read),
                        cache_write: i64_to_u64(row.five_hour_cache_write),
                    },
                    TokenBreakdown {
                        input: i64_to_u64(row.weekly_input),
                        output: i64_to_u64(row.weekly_output),
                        cache_read: i64_to_u64(row.weekly_cache_read),
                        cache_write: i64_to_u64(row.weekly_cache_write),
                    },
                    TokenBreakdown {
                        input: i64_to_u64(row.total_input),
                        output: i64_to_u64(row.total_output),
                        cache_read: i64_to_u64(row.total_cache_read),
                        cache_write: i64_to_u64(row.total_cache_write),
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

        let h = limits.five_hour_limit.map(|v| v as i64);
        let w = limits.weekly_limit.map(|v| v as i64);
        let t = limits.total_limit.map(|v| v as i64);

        sqlx::query!(
            "INSERT INTO key_model_limits (key_id, model, five_hour_limit, weekly_limit, total_limit) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (key_id, model) DO UPDATE SET \
                 five_hour_limit = EXCLUDED.five_hour_limit, \
                 weekly_limit = EXCLUDED.weekly_limit, \
                 total_limit = EXCLUDED.total_limit",
            key_id,
            model,
            h,
            w,
            t,
        )
        .execute(&conn)
        .await
        .db_context("Failed to upsert model limits")?;
        Ok(())
    }

    /// Remove per-model limits for a key
    pub async fn remove_model_limits(&self, key_id: &str, model: &str) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;
        let affected = sqlx::query!(
            "DELETE FROM key_model_limits WHERE key_id = $1 AND model = $2",
            key_id,
            model,
        )
        .execute(&conn)
        .await
        .db_context("Failed to remove model limits")?
        .rows_affected();
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
        let affected = sqlx::query!(
            "UPDATE key_model_limits SET count_from = $1 WHERE key_id = $2 AND model = $3",
            now as i64,
            key_id,
            model,
        )
        .execute(&conn)
        .await
        .db_context("Failed to reset model usage")?
        .rows_affected();
        Ok(affected > 0)
    }
}

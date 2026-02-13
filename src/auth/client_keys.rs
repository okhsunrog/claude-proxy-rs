use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use turso::Connection;
use utoipa::ToSchema;
use uuid::Uuid;

use super::models::ModelPricing;
use super::usage::TokenUsageReport;
use crate::SubscriptionState;
use crate::db;
use crate::error::ProxyError;

/// Token usage limits for a client key (all optional, in microdollars)
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenLimits {
    /// Maximum cost per 5-hour window (None = unlimited)
    #[serde(
        rename = "fiveHourLimit",
        alias = "hourlyLimit",
        skip_serializing_if = "Option::is_none"
    )]
    pub five_hour_limit: Option<u64>,
    /// Maximum cost per week (None = unlimited)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weekly_limit: Option<u64>,
    /// Maximum total cost ever (None = unlimited)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_limit: Option<u64>,
}

/// Current token usage for a client key (derived from per-model aggregation)
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct TokenUsage {
    /// Cost in current 5-hour window (microdollars)
    #[serde(rename = "fiveHourTokens")]
    pub five_hour_tokens: u64,
    /// Timestamp when 5-hour counter resets (epoch ms)
    #[serde(rename = "fiveHourResetAt")]
    pub five_hour_reset_at: u64,
    /// Cost in current week (microdollars)
    pub weekly_tokens: u64,
    /// Timestamp when weekly counter resets (epoch ms)
    pub weekly_reset_at: u64,
    /// Total cost (lifetime, microdollars)
    pub total_tokens: u64,
}

/// Which usage counter to reset
#[derive(Debug, Clone, Copy)]
pub enum UsageResetType {
    FiveHour,
    Weekly,
    Total,
    All,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClientKey {
    pub id: String,
    pub key: String,
    pub name: String,
    pub created_at: u64,
    pub last_used_at: Option<u64>,
    pub enabled: bool,
    pub allow_extra_usage: bool,
    #[serde(default)]
    pub limits: TokenLimits,
    #[serde(default)]
    pub usage: TokenUsage,
}

pub struct ClientKeysStore;

fn timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// Helper to read a nullable i64 column as Option<u64>
fn opt_u64(row: &turso::Row, idx: usize) -> Option<u64> {
    row.get::<Option<i64>>(idx).ok().flatten().map(|v| v as u64)
}

/// Helper to read a non-null i64 column as u64
fn get_u64(row: &turso::Row, idx: usize) -> u64 {
    row.get::<i64>(idx).unwrap_or(0) as u64
}

/// Parse a ClientKey from a row with columns:
/// id, key, name, enabled, created_at, last_used_at,
/// five_hour_limit, weekly_limit, total_limit,
/// five_hour_reset_at, weekly_reset_at, allow_extra_usage
fn row_to_client_key(row: &turso::Row) -> Option<ClientKey> {
    Some(ClientKey {
        id: row.get(0).ok()?,
        key: row.get(1).ok()?,
        name: row.get(2).ok()?,
        enabled: get_u64(row, 3) != 0,
        created_at: get_u64(row, 4),
        last_used_at: opt_u64(row, 5),
        allow_extra_usage: get_u64(row, 11) != 0,
        limits: TokenLimits {
            five_hour_limit: opt_u64(row, 6),
            weekly_limit: opt_u64(row, 7),
            total_limit: opt_u64(row, 8),
        },
        // Usage is derived via aggregation — zero here, populated separately
        usage: TokenUsage {
            five_hour_tokens: 0,
            five_hour_reset_at: get_u64(row, 9),
            weekly_tokens: 0,
            weekly_reset_at: get_u64(row, 10),
            total_tokens: 0,
        },
    })
}

const SELECT_ALL_COLS: &str = "id, key, name, enabled, created_at, last_used_at, five_hour_limit, weekly_limit, total_limit, five_hour_reset_at, weekly_reset_at, allow_extra_usage";

// ============================================================================
// Centralized helpers
// ============================================================================

/// Check and reset expired windows. Reads timestamps from client_keys,
/// and if expired, zeros the corresponding per-model counters in key_model_usage
/// and updates timestamps in client_keys.
///
/// Returns the current (possibly updated) reset timestamps.
async fn maybe_reset_expired_windows(
    conn: &Connection,
    key_id: &str,
    now: u64,
    window_resets: &SubscriptionState,
) -> Result<(u64, u64), ProxyError> {
    let five_hour_ms: u64 = 5 * 60 * 60 * 1000;
    let one_week_ms: u64 = 7 * 24 * 60 * 60 * 1000;

    let mut rows = conn
        .query(
            "SELECT five_hour_reset_at, weekly_reset_at FROM client_keys WHERE id = ?",
            [key_id],
        )
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to read reset timestamps: {e}")))?;

    let Some(row) = rows
        .next()
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to read reset row: {e}")))?
    else {
        return Ok((0, 0));
    };

    let five_hour_reset_at = get_u64(&row, 0);
    let weekly_reset_at = get_u64(&row, 1);

    let reset_five_hour = five_hour_reset_at > 0 && now >= five_hour_reset_at;
    let reset_weekly = weekly_reset_at > 0 && now >= weekly_reset_at;

    if !reset_five_hour && !reset_weekly {
        // Re-sync: adopt subscription timestamps if they're earlier (without resetting counters)
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

        return Ok((new_five_hour, new_weekly));
    }

    // Reset expired per-model counters
    if reset_five_hour && reset_weekly {
        conn.execute(
            "UPDATE key_model_usage SET \
             five_hour_input = 0, five_hour_output = 0, five_hour_cache_read = 0, five_hour_cache_write = 0, \
             weekly_input = 0, weekly_output = 0, weekly_cache_read = 0, weekly_cache_write = 0 \
             WHERE key_id = ?",
            [key_id],
        )
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to reset model counters: {e}")))?;
    } else if reset_five_hour {
        conn.execute(
            "UPDATE key_model_usage SET \
             five_hour_input = 0, five_hour_output = 0, five_hour_cache_read = 0, five_hour_cache_write = 0 \
             WHERE key_id = ?",
            [key_id],
        )
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to reset 5h model counters: {e}")))?;
    } else {
        conn.execute(
            "UPDATE key_model_usage SET \
             weekly_input = 0, weekly_output = 0, weekly_cache_read = 0, weekly_cache_write = 0 \
             WHERE key_id = ?",
            [key_id],
        )
        .await
        .map_err(|e| {
            ProxyError::DatabaseError(format!("Failed to reset weekly model counters: {e}"))
        })?;
    }

    // Compute new timestamps
    let new_five_hour = if reset_five_hour {
        window_resets
            .five_hour_reset_at
            .filter(|&t| t > now)
            .unwrap_or(now + five_hour_ms)
    } else if five_hour_reset_at == 0 {
        window_resets
            .five_hour_reset_at
            .filter(|&t| t > now)
            .unwrap_or(0)
    } else {
        window_resets
            .five_hour_reset_at
            .filter(|&t| t > now && t < five_hour_reset_at)
            .unwrap_or(five_hour_reset_at)
    };

    let new_weekly = if reset_weekly {
        window_resets
            .seven_day_reset_at
            .filter(|&t| t > now)
            .unwrap_or(now + one_week_ms)
    } else if weekly_reset_at == 0 {
        window_resets
            .seven_day_reset_at
            .filter(|&t| t > now)
            .unwrap_or(0)
    } else {
        window_resets
            .seven_day_reset_at
            .filter(|&t| t > now && t < weekly_reset_at)
            .unwrap_or(weekly_reset_at)
    };

    // Update timestamps in client_keys
    conn.execute(
        "UPDATE client_keys SET five_hour_reset_at = ?, weekly_reset_at = ? WHERE id = ?",
        (new_five_hour as i64, new_weekly as i64, key_id),
    )
    .await
    .map_err(|e| ProxyError::DatabaseError(format!("Failed to update reset timestamps: {e}")))?;

    Ok((new_five_hour, new_weekly))
}

/// Aggregate per-model usage into global cost (microdollars) by joining with model prices.
/// Returns (five_hour_cost, weekly_cost, total_cost).
async fn aggregate_usage_costs(
    conn: &Connection,
    key_id: &str,
) -> Result<(u64, u64, u64), ProxyError> {
    let mut rows = conn
        .query(
            "SELECT \
             COALESCE(SUM(u.five_hour_input * m.input_price + u.five_hour_output * m.output_price + u.five_hour_cache_read * m.cache_read_price + u.five_hour_cache_write * m.cache_write_price), 0.0), \
             COALESCE(SUM(u.weekly_input * m.input_price + u.weekly_output * m.output_price + u.weekly_cache_read * m.cache_read_price + u.weekly_cache_write * m.cache_write_price), 0.0), \
             COALESCE(SUM(u.total_input * m.input_price + u.total_output * m.output_price + u.total_cache_read * m.cache_read_price + u.total_cache_write * m.cache_write_price), 0.0) \
             FROM key_model_usage u \
             JOIN models m ON u.model = m.id \
             WHERE u.key_id = ?",
            [key_id],
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

    let five_hour = row.get::<f64>(0).unwrap_or(0.0).round() as u64;
    let weekly = row.get::<f64>(1).unwrap_or(0.0).round() as u64;
    let total = row.get::<f64>(2).unwrap_or(0.0).round() as u64;

    Ok((five_hour, weekly, total))
}

impl ClientKeysStore {
    pub fn new() -> Self {
        Self
    }

    pub async fn list(&self) -> Vec<ClientKey> {
        let Ok(conn) = db::get_conn().await else {
            return Vec::new();
        };
        let Ok(mut rows) = conn
            .query(&format!("SELECT {SELECT_ALL_COLS} FROM client_keys"), ())
            .await
        else {
            return Vec::new();
        };

        let mut keys = Vec::new();
        while let Ok(Some(row)) = rows.next().await {
            if let Some(key) = row_to_client_key(&row) {
                keys.push(key);
            }
        }
        keys
    }

    pub async fn create(&self, name: String) -> Result<ClientKey, ProxyError> {
        let key_suffix = {
            let mut rng = rand::rng();
            let mut bytes = [0u8; 32];
            rng.fill(&mut bytes);
            URL_SAFE_NO_PAD.encode(bytes)
        };
        let key = format!("sk-proxy-{}", key_suffix);
        let id = Uuid::new_v4().to_string();
        let now = timestamp_millis();

        let conn = db::get_conn().await?;
        conn.execute(
            "INSERT INTO client_keys (id, key, name, enabled, created_at) VALUES (?, ?, ?, 1, ?)",
            (id.as_str(), key.as_str(), name.as_str(), now as i64),
        )
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to create key: {e}")))?;

        Ok(ClientKey {
            id,
            key,
            name,
            created_at: now,
            last_used_at: None,
            enabled: true,
            allow_extra_usage: false,
            limits: TokenLimits::default(),
            usage: TokenUsage::default(),
        })
    }

    pub async fn set_enabled(&self, id: &str, enabled: bool) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;
        let affected = conn
            .execute(
                "UPDATE client_keys SET enabled = ? WHERE id = ?",
                (enabled as i64, id),
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to update key: {e}")))?;
        Ok(affected > 0)
    }

    pub async fn set_allow_extra_usage(&self, id: &str, allow: bool) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;
        let affected = conn
            .execute(
                "UPDATE client_keys SET allow_extra_usage = ? WHERE id = ?",
                (allow as i64, id),
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to update key: {e}")))?;
        Ok(affected > 0)
    }

    pub async fn delete(&self, id: &str) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;
        let affected = conn
            .execute("DELETE FROM client_keys WHERE id = ?", [id])
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to delete key: {e}")))?;
        Ok(affected > 0)
    }

    /// Validate an API key using constant-time comparison to prevent timing attacks.
    /// Fetches all enabled keys and compares in constant time.
    pub async fn validate(&self, key: &str) -> Option<ClientKey> {
        let Ok(conn) = db::get_conn().await else {
            return None;
        };
        let Ok(mut rows) = conn
            .query(
                &format!("SELECT {SELECT_ALL_COLS} FROM client_keys WHERE enabled = 1"),
                (),
            )
            .await
        else {
            return None;
        };

        let mut result = None;
        while let Ok(Some(row)) = rows.next().await {
            if let Some(ck) = row_to_client_key(&row)
                && ck.key.as_bytes().ct_eq(key.as_bytes()).into()
            {
                result = Some(ck);
            }
            // Continue iterating all rows to maintain constant time
        }
        result
    }

    pub async fn update_last_used(&self, id: &str) -> Result<(), ProxyError> {
        let now = timestamp_millis();
        let conn = db::get_conn().await?;
        conn.execute(
            "UPDATE client_keys SET last_used_at = ? WHERE id = ?",
            (now as i64, id),
        )
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to update last_used: {e}")))?;
        Ok(())
    }

    pub async fn get(&self, id: &str) -> Option<ClientKey> {
        let conn = db::get_conn().await.ok()?;
        let mut rows = conn
            .query(
                &format!("SELECT {SELECT_ALL_COLS} FROM client_keys WHERE id = ?"),
                [id],
            )
            .await
            .ok()?;
        let row = rows.next().await.ok()??;
        row_to_client_key(&row)
    }

    /// Check if a key's usage is within limits.
    /// Derives global usage from per-model aggregation.
    pub async fn check_limits(
        &self,
        id: &str,
        window_resets: &SubscriptionState,
    ) -> Result<(), String> {
        let now = timestamp_millis();
        let conn = db::get_conn().await.map_err(|e| e.to_string())?;

        // Reset expired windows (centralised)
        maybe_reset_expired_windows(&conn, id, now, window_resets)
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

        // Aggregate per-model usage into global cost
        let (five_hour_cost, weekly_cost, total_cost) = aggregate_usage_costs(&conn, id)
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

    /// Record per-model usage (all 4 token types).
    /// Reset timestamps are centralized in client_keys and handled by maybe_reset_expired_windows.
    pub async fn record_model_usage(
        &self,
        key_id: &str,
        model: &str,
        report: &TokenUsageReport,
        window_resets: &SubscriptionState,
    ) -> Result<(), ProxyError> {
        let now = timestamp_millis();
        let conn = db::get_conn().await?;

        // Reset expired windows (centralised in client_keys)
        maybe_reset_expired_windows(&conn, key_id, now, window_resets).await?;

        // Initialize timestamps if not yet set
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

        let inp = report.input_tokens as i64;
        let out = report.output_tokens as i64;
        let cr = report.cache_read_tokens as i64;
        let cw = report.cache_creation_tokens as i64;

        // Check if row exists
        let mut rows = conn
            .query(
                "SELECT COUNT(*) FROM key_model_usage WHERE key_id = ? AND model = ?",
                (key_id, model),
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to check model usage: {e}")))?;

        let exists: i64 = rows
            .next()
            .await
            .ok()
            .flatten()
            .and_then(|r| r.get::<i64>(0).ok())
            .unwrap_or(0);

        if exists > 0 {
            // Simple += since windows were already reset above
            conn.execute(
                "UPDATE key_model_usage SET \
                 five_hour_input = five_hour_input + ?, five_hour_output = five_hour_output + ?, \
                 five_hour_cache_read = five_hour_cache_read + ?, five_hour_cache_write = five_hour_cache_write + ?, \
                 weekly_input = weekly_input + ?, weekly_output = weekly_output + ?, \
                 weekly_cache_read = weekly_cache_read + ?, weekly_cache_write = weekly_cache_write + ?, \
                 total_input = total_input + ?, total_output = total_output + ?, \
                 total_cache_read = total_cache_read + ?, total_cache_write = total_cache_write + ? \
                 WHERE key_id = ? AND model = ?",
                (
                    inp, out, cr, cw, // five_hour
                    inp, out, cr, cw, // weekly
                    inp, out, cr, cw, // total
                    key_id, model,
                ),
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to update model usage: {e}")))?;
        } else {
            conn.execute(
                "INSERT INTO key_model_usage (key_id, model, \
                 five_hour_input, five_hour_output, five_hour_cache_read, five_hour_cache_write, \
                 weekly_input, weekly_output, weekly_cache_read, weekly_cache_write, \
                 total_input, total_output, total_cache_read, total_cache_write) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    key_id, model, inp, out, cr, cw, inp, out, cr, cw, inp, out, cr, cw,
                ),
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to insert model usage: {e}")))?;
        }

        Ok(())
    }

    /// Get usage statistics for a key (derived from per-model aggregation)
    pub async fn get_usage(&self, id: &str) -> Option<(TokenLimits, TokenUsage)> {
        let now = timestamp_millis();
        let key = self.get(id).await?;
        let conn = db::get_conn().await.ok()?;

        let mut five_hour_reset_at = key.usage.five_hour_reset_at;
        let mut weekly_reset_at = key.usage.weekly_reset_at;

        // Show 0 if counters have expired (read-only view)
        let five_hour_expired = five_hour_reset_at > 0 && now >= five_hour_reset_at;
        let weekly_expired = weekly_reset_at > 0 && now >= weekly_reset_at;

        if five_hour_expired {
            five_hour_reset_at = 0;
        }
        if weekly_expired {
            weekly_reset_at = 0;
        }

        let (five_hour_cost, weekly_cost, total_cost) =
            aggregate_usage_costs(&conn, id).await.ok()?;

        Some((
            key.limits,
            TokenUsage {
                five_hour_tokens: if five_hour_expired { 0 } else { five_hour_cost },
                five_hour_reset_at,
                weekly_tokens: if weekly_expired { 0 } else { weekly_cost },
                weekly_reset_at,
                total_tokens: total_cost,
            },
        ))
    }

    /// Reset usage counters for a key.
    /// Resets both timestamps in client_keys AND per-model counters in key_model_usage.
    pub async fn reset_usage(
        &self,
        id: &str,
        reset_type: UsageResetType,
    ) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;

        // Reset timestamps in client_keys
        let ts_sql = match reset_type {
            UsageResetType::FiveHour => {
                "UPDATE client_keys SET five_hour_reset_at = 0 WHERE id = ?"
            }
            UsageResetType::Weekly => "UPDATE client_keys SET weekly_reset_at = 0 WHERE id = ?",
            UsageResetType::Total => {
                // Total has no timestamp to reset, but we still need to check key exists
                "UPDATE client_keys SET id = id WHERE id = ?"
            }
            UsageResetType::All => {
                "UPDATE client_keys SET five_hour_reset_at = 0, weekly_reset_at = 0 WHERE id = ?"
            }
        };

        let affected = conn
            .execute(ts_sql, [id])
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to reset timestamps: {e}")))?;

        if affected == 0 {
            return Ok(false);
        }

        // Reset per-model counters
        let model_sql = match reset_type {
            UsageResetType::FiveHour => {
                "UPDATE key_model_usage SET five_hour_input = 0, five_hour_output = 0, five_hour_cache_read = 0, five_hour_cache_write = 0 WHERE key_id = ?"
            }
            UsageResetType::Weekly => {
                "UPDATE key_model_usage SET weekly_input = 0, weekly_output = 0, weekly_cache_read = 0, weekly_cache_write = 0 WHERE key_id = ?"
            }
            UsageResetType::Total => {
                "UPDATE key_model_usage SET total_input = 0, total_output = 0, total_cache_read = 0, total_cache_write = 0 WHERE key_id = ?"
            }
            UsageResetType::All => {
                "UPDATE key_model_usage SET \
                 five_hour_input = 0, five_hour_output = 0, five_hour_cache_read = 0, five_hour_cache_write = 0, \
                 weekly_input = 0, weekly_output = 0, weekly_cache_read = 0, weekly_cache_write = 0, \
                 total_input = 0, total_output = 0, total_cache_read = 0, total_cache_write = 0 \
                 WHERE key_id = ?"
            }
        };

        conn.execute(model_sql, [id])
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to reset model usage: {e}")))?;

        Ok(true)
    }

    /// Update limits for a key
    pub async fn set_limits(&self, id: &str, limits: TokenLimits) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;

        let h = limits.five_hour_limit.map(|v| v as i64);
        let w = limits.weekly_limit.map(|v| v as i64);
        let t = limits.total_limit.map(|v| v as i64);

        let affected = conn
            .execute(
                "UPDATE client_keys SET five_hour_limit = ?, weekly_limit = ?, total_limit = ? WHERE id = ?",
                (h, w, t, id),
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to set limits: {e}")))?;

        Ok(affected > 0)
    }

    // ========================================================================
    // Per-key model access (key_allowed_models table)
    // ========================================================================

    /// Get allowed models for a key. Empty vec means "all models allowed".
    pub async fn get_allowed_models(&self, key_id: &str) -> Vec<String> {
        let Ok(conn) = db::get_conn().await else {
            return Vec::new();
        };
        let Ok(mut rows) = conn
            .query(
                "SELECT model FROM key_allowed_models WHERE key_id = ?",
                [key_id],
            )
            .await
        else {
            return Vec::new();
        };
        let mut models = Vec::new();
        while let Ok(Some(row)) = rows.next().await {
            if let Ok(model) = row.get::<String>(0) {
                models.push(model);
            }
        }
        models
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
    pub async fn is_model_allowed(&self, key_id: &str, model: &str) -> bool {
        let Ok(conn) = db::get_conn().await else {
            return false;
        };
        // Count total allowed models for this key
        let Ok(mut rows) = conn
            .query(
                "SELECT COUNT(*) FROM key_allowed_models WHERE key_id = ?",
                [key_id],
            )
            .await
        else {
            return false;
        };
        let total: i64 = rows
            .next()
            .await
            .ok()
            .flatten()
            .and_then(|r| r.get::<i64>(0).ok())
            .unwrap_or(0);

        if total == 0 {
            return true; // No whitelist = allow all
        }

        // Check if this specific model is in the whitelist
        let Ok(mut rows) = conn
            .query(
                "SELECT COUNT(*) FROM key_allowed_models WHERE key_id = ? AND model = ?",
                (key_id, model),
            )
            .await
        else {
            return false;
        };
        rows.next()
            .await
            .ok()
            .flatten()
            .and_then(|r| r.get::<i64>(0).ok())
            .unwrap_or(0)
            > 0
    }

    // ========================================================================
    // Per-key per-model usage tracking (key_model_usage table)
    // ========================================================================

    /// Check per-model limits for a key. Returns Ok(()) if no limits set.
    /// Computes cost from stored token counters using model prices.
    pub async fn check_model_limits(
        &self,
        key_id: &str,
        model: &str,
        pricing: &ModelPricing,
        window_resets: &SubscriptionState,
    ) -> Result<(), String> {
        let now = timestamp_millis();
        let conn = db::get_conn().await.map_err(|e| e.to_string())?;

        // Reset expired windows (centralised)
        maybe_reset_expired_windows(&conn, key_id, now, window_resets)
            .await
            .map_err(|e| e.to_string())?;

        let mut rows = conn
            .query(
                "SELECT five_hour_limit, weekly_limit, total_limit, \
                 five_hour_input, five_hour_output, five_hour_cache_read, five_hour_cache_write, \
                 weekly_input, weekly_output, weekly_cache_read, weekly_cache_write, \
                 total_input, total_output, total_cache_read, total_cache_write \
                 FROM key_model_usage WHERE key_id = ? AND model = ?",
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

        let h_in = get_u64(&row, 3);
        let h_out = get_u64(&row, 4);
        let h_cr = get_u64(&row, 5);
        let h_cw = get_u64(&row, 6);
        let w_in = get_u64(&row, 7);
        let w_out = get_u64(&row, 8);
        let w_cr = get_u64(&row, 9);
        let w_cw = get_u64(&row, 10);
        let t_in = get_u64(&row, 11);
        let t_out = get_u64(&row, 12);
        let t_cr = get_u64(&row, 13);
        let t_cw = get_u64(&row, 14);

        // Compute costs using model prices
        let compute_cost = |inp: u64, out: u64, cr: u64, cw: u64| -> u64 {
            let cost = inp as f64 * pricing.input_price
                + out as f64 * pricing.output_price
                + cr as f64 * pricing.cache_read_price
                + cw as f64 * pricing.cache_write_price;
            cost.round() as u64
        };

        if let Some(limit) = five_hour_limit {
            let cost = compute_cost(h_in, h_out, h_cr, h_cw);
            if cost >= limit {
                return Err(format!(
                    "5-hour model limit exceeded for {model} (${:.2}/${:.2})",
                    cost as f64 / 1_000_000.0,
                    limit as f64 / 1_000_000.0
                ));
            }
        }

        if let Some(limit) = weekly_limit {
            let cost = compute_cost(w_in, w_out, w_cr, w_cw);
            if cost >= limit {
                return Err(format!(
                    "Weekly model limit exceeded for {model} (${:.2}/${:.2})",
                    cost as f64 / 1_000_000.0,
                    limit as f64 / 1_000_000.0
                ));
            }
        }

        if let Some(limit) = total_limit {
            let cost = compute_cost(t_in, t_out, t_cr, t_cw);
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

    /// Get per-model usage entries for a key
    pub async fn get_model_usage(&self, key_id: &str) -> Vec<ModelUsageEntry> {
        let now = timestamp_millis();
        let Ok(conn) = db::get_conn().await else {
            return Vec::new();
        };

        // Read centralized reset timestamps from client_keys
        let Ok(mut ts_rows) = conn
            .query(
                "SELECT five_hour_reset_at, weekly_reset_at FROM client_keys WHERE id = ?",
                [key_id],
            )
            .await
        else {
            return Vec::new();
        };
        let (five_hour_reset_at, weekly_reset_at) = if let Ok(Some(row)) = ts_rows.next().await {
            (get_u64(&row, 0), get_u64(&row, 1))
        } else {
            (0, 0)
        };

        let five_hour_expired = five_hour_reset_at > 0 && now >= five_hour_reset_at;
        let weekly_expired = weekly_reset_at > 0 && now >= weekly_reset_at;

        let Ok(mut rows) = conn
            .query(
                "SELECT model, five_hour_limit, weekly_limit, total_limit, \
                 five_hour_input, five_hour_output, five_hour_cache_read, five_hour_cache_write, \
                 weekly_input, weekly_output, weekly_cache_read, weekly_cache_write, \
                 total_input, total_output, total_cache_read, total_cache_write \
                 FROM key_model_usage WHERE key_id = ?",
                [key_id],
            )
            .await
        else {
            return Vec::new();
        };

        let mut entries = Vec::new();
        while let Ok(Some(row)) = rows.next().await {
            let Ok(model) = row.get::<String>(0) else {
                continue;
            };

            entries.push(ModelUsageEntry {
                model,
                limits: TokenLimits {
                    five_hour_limit: opt_u64(&row, 1),
                    weekly_limit: opt_u64(&row, 2),
                    total_limit: opt_u64(&row, 3),
                },
                five_hour: if five_hour_expired {
                    TokenBreakdown::default()
                } else {
                    TokenBreakdown {
                        input: get_u64(&row, 4),
                        output: get_u64(&row, 5),
                        cache_read: get_u64(&row, 6),
                        cache_write: get_u64(&row, 7),
                    }
                },
                weekly: if weekly_expired {
                    TokenBreakdown::default()
                } else {
                    TokenBreakdown {
                        input: get_u64(&row, 8),
                        output: get_u64(&row, 9),
                        cache_read: get_u64(&row, 10),
                        cache_write: get_u64(&row, 11),
                    }
                },
                total: TokenBreakdown {
                    input: get_u64(&row, 12),
                    output: get_u64(&row, 13),
                    cache_read: get_u64(&row, 14),
                    cache_write: get_u64(&row, 15),
                },
                five_hour_reset_at: if five_hour_expired {
                    0
                } else {
                    five_hour_reset_at
                },
                weekly_reset_at: if weekly_expired { 0 } else { weekly_reset_at },
            });
        }
        entries
    }

    /// Set per-model limits for a key (UPSERT)
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
                "SELECT COUNT(*) FROM key_model_usage WHERE key_id = ? AND model = ?",
                (key_id, model),
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to check model usage: {e}")))?;

        let exists: i64 = rows
            .next()
            .await
            .ok()
            .flatten()
            .and_then(|r| r.get::<i64>(0).ok())
            .unwrap_or(0);

        if exists > 0 {
            let h = limits.five_hour_limit.map(|v| v as i64);
            let w = limits.weekly_limit.map(|v| v as i64);
            let t = limits.total_limit.map(|v| v as i64);
            conn.execute(
                "UPDATE key_model_usage SET five_hour_limit = ?, weekly_limit = ?, total_limit = ? WHERE key_id = ? AND model = ?",
                (h, w, t, key_id, model),
            )
            .await
            .map_err(|e| {
                ProxyError::DatabaseError(format!("Failed to update model limits: {e}"))
            })?;
        } else {
            // Insert new row with limits only
            let h = limits.five_hour_limit.map(|v| v as i64);
            let w = limits.weekly_limit.map(|v| v as i64);
            let t = limits.total_limit.map(|v| v as i64);
            conn.execute(
                "INSERT INTO key_model_usage (key_id, model, five_hour_limit, weekly_limit, total_limit) VALUES (?, ?, ?, ?, ?)",
                (key_id, model, h, w, t),
            )
            .await
            .map_err(|e| {
                ProxyError::DatabaseError(format!("Failed to insert model limits: {e}"))
            })?;
        }
        Ok(())
    }

    /// Remove per-model limits (and usage) for a key
    pub async fn remove_model_limits(&self, key_id: &str, model: &str) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;
        let affected = conn
            .execute(
                "DELETE FROM key_model_usage WHERE key_id = ? AND model = ?",
                (key_id, model),
            )
            .await
            .map_err(|e| {
                ProxyError::DatabaseError(format!("Failed to remove model limits: {e}"))
            })?;
        Ok(affected > 0)
    }

    /// Reset per-model usage counters
    pub async fn reset_model_usage(
        &self,
        key_id: &str,
        model: &str,
        reset_type: UsageResetType,
    ) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;

        let sql = match reset_type {
            UsageResetType::FiveHour => {
                "UPDATE key_model_usage SET five_hour_input = 0, five_hour_output = 0, five_hour_cache_read = 0, five_hour_cache_write = 0 WHERE key_id = ? AND model = ?"
            }
            UsageResetType::Weekly => {
                "UPDATE key_model_usage SET weekly_input = 0, weekly_output = 0, weekly_cache_read = 0, weekly_cache_write = 0 WHERE key_id = ? AND model = ?"
            }
            UsageResetType::Total => {
                "UPDATE key_model_usage SET total_input = 0, total_output = 0, total_cache_read = 0, total_cache_write = 0 WHERE key_id = ? AND model = ?"
            }
            UsageResetType::All => {
                "UPDATE key_model_usage SET five_hour_input = 0, five_hour_output = 0, five_hour_cache_read = 0, five_hour_cache_write = 0, weekly_input = 0, weekly_output = 0, weekly_cache_read = 0, weekly_cache_write = 0, total_input = 0, total_output = 0, total_cache_read = 0, total_cache_write = 0 WHERE key_id = ? AND model = ?"
            }
        };

        let affected = conn
            .execute(sql, (key_id, model))
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to reset model usage: {e}")))?;
        Ok(affected > 0)
    }
}

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

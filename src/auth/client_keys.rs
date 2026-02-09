use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use utoipa::ToSchema;
use uuid::Uuid;

use super::models::ModelPricing;
use super::usage::TokenUsageReport;
use crate::db;
use crate::error::ProxyError;

/// Token usage limits for a client key (all optional)
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenLimits {
    /// Maximum tokens per hour (None = unlimited)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hourly_limit: Option<u64>,
    /// Maximum tokens per week (None = unlimited)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weekly_limit: Option<u64>,
    /// Maximum total tokens ever (None = unlimited)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_limit: Option<u64>,
}

/// Current token usage for a client key
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct TokenUsage {
    /// Tokens used in current hour
    pub hourly_tokens: u64,
    /// Timestamp when hourly counter resets (epoch ms)
    pub hourly_reset_at: u64,
    /// Tokens used in current week
    pub weekly_tokens: u64,
    /// Timestamp when weekly counter resets (epoch ms)
    pub weekly_reset_at: u64,
    /// Total tokens used (lifetime)
    pub total_tokens: u64,
}

/// Which usage counter to reset
#[derive(Debug, Clone, Copy)]
pub enum UsageResetType {
    Hourly,
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
/// hourly_limit, weekly_limit, total_limit,
/// hourly_usage, weekly_usage, total_usage,
/// hourly_reset_at, weekly_reset_at
fn row_to_client_key(row: &turso::Row) -> Option<ClientKey> {
    Some(ClientKey {
        id: row.get(0).ok()?,
        key: row.get(1).ok()?,
        name: row.get(2).ok()?,
        enabled: get_u64(row, 3) != 0,
        created_at: get_u64(row, 4),
        last_used_at: opt_u64(row, 5),
        limits: TokenLimits {
            hourly_limit: opt_u64(row, 6),
            weekly_limit: opt_u64(row, 7),
            total_limit: opt_u64(row, 8),
        },
        usage: TokenUsage {
            hourly_tokens: get_u64(row, 9),
            hourly_reset_at: get_u64(row, 12),
            weekly_tokens: get_u64(row, 10),
            weekly_reset_at: get_u64(row, 13),
            total_tokens: get_u64(row, 11),
        },
    })
}

const SELECT_ALL_COLS: &str = "id, key, name, enabled, created_at, last_used_at, hourly_limit, weekly_limit, total_limit, hourly_usage, weekly_usage, total_usage, hourly_reset_at, weekly_reset_at";

impl ClientKeysStore {
    pub fn new() -> Self {
        Self
    }

    pub async fn list(&self) -> Vec<ClientKey> {
        let Ok(conn) = db::get_conn() else {
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

        let conn = db::get_conn()?;
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
            limits: TokenLimits::default(),
            usage: TokenUsage::default(),
        })
    }

    pub async fn delete(&self, id: &str) -> Result<bool, ProxyError> {
        let conn = db::get_conn()?;
        let affected = conn
            .execute("DELETE FROM client_keys WHERE id = ?", [id])
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to delete key: {e}")))?;
        Ok(affected > 0)
    }

    /// Validate an API key using constant-time comparison to prevent timing attacks.
    /// Fetches all enabled keys and compares in constant time.
    pub async fn validate(&self, key: &str) -> Option<ClientKey> {
        let Ok(conn) = db::get_conn() else {
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
        let conn = db::get_conn()?;
        conn.execute(
            "UPDATE client_keys SET last_used_at = ? WHERE id = ?",
            (now as i64, id),
        )
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to update last_used: {e}")))?;
        Ok(())
    }

    pub async fn get(&self, id: &str) -> Option<ClientKey> {
        let conn = db::get_conn().ok()?;
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
    /// Automatically resets hourly/weekly counters if time has passed.
    pub async fn check_limits(&self, id: &str) -> Result<(), String> {
        let now = timestamp_millis();
        let conn = db::get_conn().map_err(|e| e.to_string())?;

        // Read current state
        let mut rows = conn
            .query(
                "SELECT hourly_limit, weekly_limit, total_limit, hourly_usage, weekly_usage, total_usage, hourly_reset_at, weekly_reset_at FROM client_keys WHERE id = ?",
                [id],
            )
            .await
            .map_err(|e| format!("DB error: {e}"))?;

        let row = rows
            .next()
            .await
            .map_err(|e| format!("DB error: {e}"))?
            .ok_or_else(|| "Key not found".to_string())?;

        let hourly_limit = opt_u64(&row, 0);
        let weekly_limit = opt_u64(&row, 1);
        let total_limit = opt_u64(&row, 2);
        let mut hourly_usage = get_u64(&row, 3);
        let mut weekly_usage = get_u64(&row, 4);
        let total_usage = get_u64(&row, 5);
        let mut hourly_reset_at = get_u64(&row, 6);
        let mut weekly_reset_at = get_u64(&row, 7);

        // Auto-reset expired counters
        let mut needs_update = false;
        if hourly_reset_at > 0 && now >= hourly_reset_at {
            hourly_usage = 0;
            hourly_reset_at = 0;
            needs_update = true;
        }
        if weekly_reset_at > 0 && now >= weekly_reset_at {
            weekly_usage = 0;
            weekly_reset_at = 0;
            needs_update = true;
        }

        if needs_update {
            let _ = conn
                .execute(
                    "UPDATE client_keys SET hourly_usage = ?, weekly_usage = ?, hourly_reset_at = ?, weekly_reset_at = ? WHERE id = ?",
                    (hourly_usage as i64, weekly_usage as i64, hourly_reset_at as i64, weekly_reset_at as i64, id),
                )
                .await;
        }

        if let Some(limit) = hourly_limit
            && hourly_usage >= limit
        {
            return Err(format!(
                "Hourly token limit exceeded ({}/{})",
                hourly_usage, limit
            ));
        }

        if let Some(limit) = weekly_limit
            && weekly_usage >= limit
        {
            return Err(format!(
                "Weekly token limit exceeded ({}/{})",
                weekly_usage, limit
            ));
        }

        if let Some(limit) = total_limit
            && total_usage >= limit
        {
            return Err(format!(
                "Total token limit exceeded ({}/{})",
                total_usage, limit
            ));
        }

        Ok(())
    }

    /// Record token usage for a key.
    /// Initializes reset timestamps if not set.
    pub async fn record_usage(&self, id: &str, tokens: u64) -> Result<(), ProxyError> {
        let now = timestamp_millis();
        let one_hour_ms: u64 = 60 * 60 * 1000;
        let one_week_ms: u64 = 7 * 24 * 60 * 60 * 1000;

        let conn = db::get_conn()?;

        // Read current reset timestamps
        let mut rows = conn
            .query(
                "SELECT hourly_reset_at, weekly_reset_at FROM client_keys WHERE id = ?",
                [id],
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to read usage: {e}")))?;

        let Some(row) = rows
            .next()
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to read usage row: {e}")))?
        else {
            return Ok(());
        };

        let hourly_reset_at = get_u64(&row, 0);
        let weekly_reset_at = get_u64(&row, 1);

        // Determine if counters need reset
        let reset_hourly = hourly_reset_at == 0 || now >= hourly_reset_at;
        let reset_weekly = weekly_reset_at == 0 || now >= weekly_reset_at;

        let new_hourly_reset = if reset_hourly {
            now + one_hour_ms
        } else {
            hourly_reset_at
        };
        let new_weekly_reset = if reset_weekly {
            now + one_week_ms
        } else {
            weekly_reset_at
        };

        // Build update: reset counters if needed, then add tokens
        if reset_hourly && reset_weekly {
            conn.execute(
                "UPDATE client_keys SET hourly_usage = ?, weekly_usage = ?, total_usage = total_usage + ?, hourly_reset_at = ?, weekly_reset_at = ? WHERE id = ?",
                (tokens as i64, tokens as i64, tokens as i64, new_hourly_reset as i64, new_weekly_reset as i64, id),
            ).await
        } else if reset_hourly {
            conn.execute(
                "UPDATE client_keys SET hourly_usage = ?, weekly_usage = weekly_usage + ?, total_usage = total_usage + ?, hourly_reset_at = ?, weekly_reset_at = ? WHERE id = ?",
                (tokens as i64, tokens as i64, tokens as i64, new_hourly_reset as i64, new_weekly_reset as i64, id),
            ).await
        } else if reset_weekly {
            conn.execute(
                "UPDATE client_keys SET hourly_usage = hourly_usage + ?, weekly_usage = ?, total_usage = total_usage + ?, hourly_reset_at = ?, weekly_reset_at = ? WHERE id = ?",
                (tokens as i64, tokens as i64, tokens as i64, new_hourly_reset as i64, new_weekly_reset as i64, id),
            ).await
        } else {
            conn.execute(
                "UPDATE client_keys SET hourly_usage = hourly_usage + ?, weekly_usage = weekly_usage + ?, total_usage = total_usage + ? WHERE id = ?",
                (tokens as i64, tokens as i64, tokens as i64, id),
            ).await
        }
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to record usage: {e}")))?;

        Ok(())
    }

    /// Get usage statistics for a key
    pub async fn get_usage(&self, id: &str) -> Option<(TokenLimits, TokenUsage)> {
        let now = timestamp_millis();
        let key = self.get(id).await?;

        let mut usage = key.usage;
        // Show 0 if counters have expired (read-only view)
        if usage.hourly_reset_at > 0 && now >= usage.hourly_reset_at {
            usage.hourly_tokens = 0;
            usage.hourly_reset_at = 0;
        }
        if usage.weekly_reset_at > 0 && now >= usage.weekly_reset_at {
            usage.weekly_tokens = 0;
            usage.weekly_reset_at = 0;
        }

        Some((key.limits, usage))
    }

    /// Reset usage counters for a key
    pub async fn reset_usage(
        &self,
        id: &str,
        reset_type: UsageResetType,
    ) -> Result<bool, ProxyError> {
        let conn = db::get_conn()?;

        let sql = match reset_type {
            UsageResetType::Hourly => {
                "UPDATE client_keys SET hourly_usage = 0, hourly_reset_at = 0 WHERE id = ?"
            }
            UsageResetType::Weekly => {
                "UPDATE client_keys SET weekly_usage = 0, weekly_reset_at = 0 WHERE id = ?"
            }
            UsageResetType::Total => "UPDATE client_keys SET total_usage = 0 WHERE id = ?",
            UsageResetType::All => {
                "UPDATE client_keys SET hourly_usage = 0, weekly_usage = 0, total_usage = 0, hourly_reset_at = 0, weekly_reset_at = 0 WHERE id = ?"
            }
        };

        let affected = conn
            .execute(sql, [id])
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to reset usage: {e}")))?;

        Ok(affected > 0)
    }

    /// Update limits for a key
    pub async fn set_limits(&self, id: &str, limits: TokenLimits) -> Result<bool, ProxyError> {
        let conn = db::get_conn()?;

        // Build SET clause dynamically to handle NULLs properly
        let hourly_sql = match limits.hourly_limit {
            Some(v) => format!("hourly_limit = {v}"),
            None => "hourly_limit = NULL".to_string(),
        };
        let weekly_sql = match limits.weekly_limit {
            Some(v) => format!("weekly_limit = {v}"),
            None => "weekly_limit = NULL".to_string(),
        };
        let total_sql = match limits.total_limit {
            Some(v) => format!("total_limit = {v}"),
            None => "total_limit = NULL".to_string(),
        };

        let sql = format!(
            "UPDATE client_keys SET {}, {}, {} WHERE id = ?",
            hourly_sql, weekly_sql, total_sql
        );

        let affected = conn
            .execute(&sql, [id])
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to set limits: {e}")))?;

        Ok(affected > 0)
    }

    // ========================================================================
    // Per-key model access (key_allowed_models table)
    // ========================================================================

    /// Get allowed models for a key. Empty vec means "all models allowed".
    pub async fn get_allowed_models(&self, key_id: &str) -> Vec<String> {
        let Ok(conn) = db::get_conn() else {
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
        let conn = db::get_conn()?;
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
        let Ok(conn) = db::get_conn() else {
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
    ) -> Result<(), String> {
        let now = timestamp_millis();
        let conn = db::get_conn().map_err(|e| e.to_string())?;

        let mut rows = conn
            .query(
                "SELECT hourly_limit, weekly_limit, total_limit, \
                 hourly_input, hourly_output, hourly_cache_read, hourly_cache_write, \
                 weekly_input, weekly_output, weekly_cache_read, weekly_cache_write, \
                 total_input, total_output, total_cache_read, total_cache_write, \
                 hourly_reset_at, weekly_reset_at \
                 FROM key_model_usage WHERE key_id = ? AND model = ?",
                (key_id, model),
            )
            .await
            .map_err(|e| format!("DB error: {e}"))?;

        let Some(row) = rows.next().await.map_err(|e| format!("DB error: {e}"))? else {
            return Ok(()); // No row = no limits
        };

        let hourly_limit = opt_u64(&row, 0);
        let weekly_limit = opt_u64(&row, 1);
        let total_limit = opt_u64(&row, 2);

        let mut h_in = get_u64(&row, 3);
        let mut h_out = get_u64(&row, 4);
        let mut h_cr = get_u64(&row, 5);
        let mut h_cw = get_u64(&row, 6);
        let mut w_in = get_u64(&row, 7);
        let mut w_out = get_u64(&row, 8);
        let mut w_cr = get_u64(&row, 9);
        let mut w_cw = get_u64(&row, 10);
        let t_in = get_u64(&row, 11);
        let t_out = get_u64(&row, 12);
        let t_cr = get_u64(&row, 13);
        let t_cw = get_u64(&row, 14);
        let mut hourly_reset_at = get_u64(&row, 15);
        let mut weekly_reset_at = get_u64(&row, 16);

        // Auto-reset expired counters
        let mut needs_update = false;
        if hourly_reset_at > 0 && now >= hourly_reset_at {
            h_in = 0;
            h_out = 0;
            h_cr = 0;
            h_cw = 0;
            hourly_reset_at = 0;
            needs_update = true;
        }
        if weekly_reset_at > 0 && now >= weekly_reset_at {
            w_in = 0;
            w_out = 0;
            w_cr = 0;
            w_cw = 0;
            weekly_reset_at = 0;
            needs_update = true;
        }

        if needs_update {
            let _ = conn
                .execute(
                    "UPDATE key_model_usage SET \
                     hourly_input = ?, hourly_output = ?, hourly_cache_read = ?, hourly_cache_write = ?, \
                     weekly_input = ?, weekly_output = ?, weekly_cache_read = ?, weekly_cache_write = ?, \
                     hourly_reset_at = ?, weekly_reset_at = ? \
                     WHERE key_id = ? AND model = ?",
                    (
                        h_in as i64, h_out as i64, h_cr as i64, h_cw as i64,
                        w_in as i64, w_out as i64, w_cr as i64, w_cw as i64,
                        hourly_reset_at as i64, weekly_reset_at as i64,
                        key_id, model,
                    ),
                )
                .await;
        }

        // Compute costs using model prices
        let compute_cost = |inp: u64, out: u64, cr: u64, cw: u64| -> u64 {
            let cost = inp as f64 * pricing.input_price
                + out as f64 * pricing.output_price
                + cr as f64 * pricing.cache_read_price
                + cw as f64 * pricing.cache_write_price;
            cost.round() as u64
        };

        if let Some(limit) = hourly_limit {
            let cost = compute_cost(h_in, h_out, h_cr, h_cw);
            if cost >= limit {
                return Err(format!(
                    "Hourly model limit exceeded for {model} (${:.2}/${:.2})",
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

    /// Record per-model usage (all 4 token types).
    pub async fn record_model_usage(
        &self,
        key_id: &str,
        model: &str,
        report: &TokenUsageReport,
    ) -> Result<(), ProxyError> {
        let now = timestamp_millis();
        let one_hour_ms: u64 = 60 * 60 * 1000;
        let one_week_ms: u64 = 7 * 24 * 60 * 60 * 1000;
        let conn = db::get_conn()?;

        // Check if row exists
        let mut rows = conn
            .query(
                "SELECT hourly_reset_at, weekly_reset_at FROM key_model_usage WHERE key_id = ? AND model = ?",
                (key_id, model),
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to read model usage: {e}")))?;

        let inp = report.input_tokens as i64;
        let out = report.output_tokens as i64;
        let cr = report.cache_read_tokens as i64;
        let cw = report.cache_creation_tokens as i64;

        if let Some(row) = rows.next().await.map_err(|e| {
            ProxyError::DatabaseError(format!("Failed to read model usage row: {e}"))
        })? {
            // Row exists - update with reset logic
            let hourly_reset_at = get_u64(&row, 0);
            let weekly_reset_at = get_u64(&row, 1);

            let reset_hourly = hourly_reset_at == 0 || now >= hourly_reset_at;
            let reset_weekly = weekly_reset_at == 0 || now >= weekly_reset_at;

            let new_hourly_reset = if reset_hourly {
                now + one_hour_ms
            } else {
                hourly_reset_at
            } as i64;
            let new_weekly_reset = if reset_weekly {
                now + one_week_ms
            } else {
                weekly_reset_at
            } as i64;

            // Build UPDATE based on which counters need reset
            let h_prefix = if reset_hourly { "" } else { "hourly_input + " };
            let w_prefix = if reset_weekly { "" } else { "weekly_input + " };

            let sql = format!(
                "UPDATE key_model_usage SET \
                 hourly_input = {h_prefix}?, hourly_output = {}?, hourly_cache_read = {}?, hourly_cache_write = {}?, \
                 weekly_input = {w_prefix}?, weekly_output = {}?, weekly_cache_read = {}?, weekly_cache_write = {}?, \
                 total_input = total_input + ?, total_output = total_output + ?, total_cache_read = total_cache_read + ?, total_cache_write = total_cache_write + ?, \
                 hourly_reset_at = ?, weekly_reset_at = ? \
                 WHERE key_id = ? AND model = ?",
                if reset_hourly { "" } else { "hourly_output + " },
                if reset_hourly {
                    ""
                } else {
                    "hourly_cache_read + "
                },
                if reset_hourly {
                    ""
                } else {
                    "hourly_cache_write + "
                },
                if reset_weekly { "" } else { "weekly_output + " },
                if reset_weekly {
                    ""
                } else {
                    "weekly_cache_read + "
                },
                if reset_weekly {
                    ""
                } else {
                    "weekly_cache_write + "
                },
            );

            conn.execute(
                &sql,
                (
                    inp,
                    out,
                    cr,
                    cw, // hourly
                    inp,
                    out,
                    cr,
                    cw, // weekly
                    inp,
                    out,
                    cr,
                    cw, // total
                    new_hourly_reset,
                    new_weekly_reset,
                    key_id,
                    model,
                ),
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to update model usage: {e}")))?;
        } else {
            // No row - insert new
            let new_hourly_reset = (now + one_hour_ms) as i64;
            let new_weekly_reset = (now + one_week_ms) as i64;

            conn.execute(
                "INSERT INTO key_model_usage (key_id, model, \
                 hourly_input, hourly_output, hourly_cache_read, hourly_cache_write, \
                 weekly_input, weekly_output, weekly_cache_read, weekly_cache_write, \
                 total_input, total_output, total_cache_read, total_cache_write, \
                 hourly_reset_at, weekly_reset_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    key_id,
                    model,
                    inp,
                    out,
                    cr,
                    cw,
                    inp,
                    out,
                    cr,
                    cw,
                    inp,
                    out,
                    cr,
                    cw,
                    new_hourly_reset,
                    new_weekly_reset,
                ),
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to insert model usage: {e}")))?;
        }

        Ok(())
    }

    /// Get per-model usage entries for a key
    pub async fn get_model_usage(&self, key_id: &str) -> Vec<ModelUsageEntry> {
        let now = timestamp_millis();
        let Ok(conn) = db::get_conn() else {
            return Vec::new();
        };
        let Ok(mut rows) = conn
            .query(
                "SELECT model, hourly_limit, weekly_limit, total_limit, \
                 hourly_input, hourly_output, hourly_cache_read, hourly_cache_write, \
                 weekly_input, weekly_output, weekly_cache_read, weekly_cache_write, \
                 total_input, total_output, total_cache_read, total_cache_write, \
                 hourly_reset_at, weekly_reset_at \
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

            let hourly_reset_at = get_u64(&row, 16);
            let weekly_reset_at = get_u64(&row, 17);

            // Show 0 for expired counters
            let hourly_expired = hourly_reset_at > 0 && now >= hourly_reset_at;
            let weekly_expired = weekly_reset_at > 0 && now >= weekly_reset_at;

            entries.push(ModelUsageEntry {
                model,
                limits: TokenLimits {
                    hourly_limit: opt_u64(&row, 1),
                    weekly_limit: opt_u64(&row, 2),
                    total_limit: opt_u64(&row, 3),
                },
                hourly: if hourly_expired {
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
                hourly_reset_at: if hourly_expired { 0 } else { hourly_reset_at },
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
        let conn = db::get_conn()?;

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
            let hourly_sql = match limits.hourly_limit {
                Some(v) => format!("hourly_limit = {v}"),
                None => "hourly_limit = NULL".to_string(),
            };
            let weekly_sql = match limits.weekly_limit {
                Some(v) => format!("weekly_limit = {v}"),
                None => "weekly_limit = NULL".to_string(),
            };
            let total_sql = match limits.total_limit {
                Some(v) => format!("total_limit = {v}"),
                None => "total_limit = NULL".to_string(),
            };
            let sql = format!(
                "UPDATE key_model_usage SET {hourly_sql}, {weekly_sql}, {total_sql} WHERE key_id = ? AND model = ?"
            );
            conn.execute(&sql, (key_id, model)).await.map_err(|e| {
                ProxyError::DatabaseError(format!("Failed to update model limits: {e}"))
            })?;
        } else {
            // Insert new row with limits only
            let h = limits.hourly_limit.map(|v| v as i64);
            let w = limits.weekly_limit.map(|v| v as i64);
            let t = limits.total_limit.map(|v| v as i64);
            conn.execute(
                "INSERT INTO key_model_usage (key_id, model, hourly_limit, weekly_limit, total_limit) VALUES (?, ?, ?, ?, ?)",
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
        let conn = db::get_conn()?;
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
        let conn = db::get_conn()?;

        let sql = match reset_type {
            UsageResetType::Hourly => {
                "UPDATE key_model_usage SET hourly_input = 0, hourly_output = 0, hourly_cache_read = 0, hourly_cache_write = 0, hourly_reset_at = 0 WHERE key_id = ? AND model = ?"
            }
            UsageResetType::Weekly => {
                "UPDATE key_model_usage SET weekly_input = 0, weekly_output = 0, weekly_cache_read = 0, weekly_cache_write = 0, weekly_reset_at = 0 WHERE key_id = ? AND model = ?"
            }
            UsageResetType::Total => {
                "UPDATE key_model_usage SET total_input = 0, total_output = 0, total_cache_read = 0, total_cache_write = 0 WHERE key_id = ? AND model = ?"
            }
            UsageResetType::All => {
                "UPDATE key_model_usage SET hourly_input = 0, hourly_output = 0, hourly_cache_read = 0, hourly_cache_write = 0, weekly_input = 0, weekly_output = 0, weekly_cache_read = 0, weekly_cache_write = 0, total_input = 0, total_output = 0, total_cache_read = 0, total_cache_write = 0, hourly_reset_at = 0, weekly_reset_at = 0 WHERE key_id = ? AND model = ?"
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
    pub hourly: TokenBreakdown,
    pub weekly: TokenBreakdown,
    pub total: TokenBreakdown,
    pub hourly_reset_at: u64,
    pub weekly_reset_at: u64,
}

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use utoipa::ToSchema;
use uuid::Uuid;

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
}

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngExt;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use subtle::ConstantTimeEq;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::db;
use crate::error::ProxyError;
use crate::subscription::timestamp_millis;

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

/// Helper to read a nullable i64 column as Option<u64>
pub(crate) fn opt_u64(row: &db::DbRow, idx: usize) -> Option<u64> {
    row.try_get::<Option<i64>, _>(idx)
        .ok()
        .flatten()
        .map(|v| v as u64)
}

/// Helper to read a non-null i64 column as u64
pub(crate) fn get_u64(row: &db::DbRow, idx: usize) -> u64 {
    row.try_get::<i64, _>(idx).unwrap_or(0) as u64
}

/// Parse a ClientKey from a row with columns:
/// id, key, name, enabled, created_at, last_used_at,
/// five_hour_limit, weekly_limit, total_limit,
/// five_hour_reset_at, weekly_reset_at, allow_extra_usage
fn row_to_client_key(row: &db::DbRow) -> Option<ClientKey> {
    Some(ClientKey {
        id: row.try_get(0).ok()?,
        key: row.try_get(1).ok()?,
        name: row.try_get(2).ok()?,
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

const SELECT_ALL: &str = "SELECT id, key, name, enabled, created_at, last_used_at, five_hour_limit, weekly_limit, total_limit, five_hour_reset_at, weekly_reset_at, allow_extra_usage FROM client_keys";
const SELECT_ENABLED: &str = "SELECT id, key, name, enabled, created_at, last_used_at, five_hour_limit, weekly_limit, total_limit, five_hour_reset_at, weekly_reset_at, allow_extra_usage FROM client_keys WHERE enabled = 1";
const SELECT_BY_ID: &str = "SELECT id, key, name, enabled, created_at, last_used_at, five_hour_limit, weekly_limit, total_limit, five_hour_reset_at, weekly_reset_at, allow_extra_usage FROM client_keys WHERE id = $1";

impl ClientKeysStore {
    pub fn new() -> Self {
        Self
    }

    pub async fn list(&self) -> Result<Vec<ClientKey>, ProxyError> {
        let conn = db::get_conn().await?;
        let rows = sqlx::query(SELECT_ALL)
            .fetch_all(&conn)
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to list keys: {e}")))?;

        let mut keys = Vec::new();
        for row in rows {
            if let Some(key) = row_to_client_key(&row) {
                keys.push(key);
            }
        }
        Ok(keys)
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
        sqlx::query(
            "INSERT INTO client_keys (id, key, name, enabled, created_at) VALUES ($1, $2, $3, 1, $4)",
        )
        .bind(id.as_str())
        .bind(key.as_str())
        .bind(name.as_str())
        .bind(now as i64)
        .execute(&conn)
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
        let affected = sqlx::query("UPDATE client_keys SET enabled = $1 WHERE id = $2")
            .bind(enabled as i64)
            .bind(id)
            .execute(&conn)
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to update key: {e}")))?
            .rows_affected();
        Ok(affected > 0)
    }

    pub async fn set_allow_extra_usage(&self, id: &str, allow: bool) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;
        let affected = sqlx::query("UPDATE client_keys SET allow_extra_usage = $1 WHERE id = $2")
            .bind(allow as i64)
            .bind(id)
            .execute(&conn)
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to update key: {e}")))?
            .rows_affected();
        Ok(affected > 0)
    }

    pub async fn delete(&self, id: &str) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;
        let affected = sqlx::query("DELETE FROM client_keys WHERE id = $1")
            .bind(id)
            .execute(&conn)
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to delete key: {e}")))?
            .rows_affected();
        Ok(affected > 0)
    }

    /// Validate an API key using constant-time comparison to prevent timing attacks.
    /// Fetches all enabled keys and compares in constant time.
    pub async fn validate(&self, key: &str) -> Result<Option<ClientKey>, ProxyError> {
        let conn = db::get_conn().await?;
        let rows = sqlx::query(SELECT_ENABLED)
            .fetch_all(&conn)
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to validate key: {e}")))?;

        let mut result = None;
        for row in rows {
            if let Some(ck) = row_to_client_key(&row)
                && ck.key.as_bytes().ct_eq(key.as_bytes()).into()
            {
                result = Some(ck);
            }
            // Continue iterating all rows to maintain constant time
        }
        Ok(result)
    }

    pub async fn update_last_used(&self, id: &str) -> Result<(), ProxyError> {
        let now = timestamp_millis();
        let conn = db::get_conn().await?;
        sqlx::query("UPDATE client_keys SET last_used_at = $1 WHERE id = $2")
            .bind(now as i64)
            .bind(id)
            .execute(&conn)
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to update last_used: {e}")))?;
        Ok(())
    }

    pub async fn get(&self, id: &str) -> Result<Option<ClientKey>, ProxyError> {
        let conn = db::get_conn().await?;
        let row = sqlx::query(SELECT_BY_ID)
            .bind(id)
            .fetch_optional(&conn)
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to get key: {e}")))?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(row_to_client_key(&row))
    }

    /// Update limits for a key
    pub async fn set_limits(&self, id: &str, limits: TokenLimits) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;

        let h = limits.five_hour_limit.map(|v| v as i64);
        let w = limits.weekly_limit.map(|v| v as i64);
        let t = limits.total_limit.map(|v| v as i64);

        let affected = sqlx::query(
            "UPDATE client_keys SET five_hour_limit = $1, weekly_limit = $2, total_limit = $3 WHERE id = $4",
        )
        .bind(h)
        .bind(w)
        .bind(t)
        .bind(id)
        .execute(&conn)
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to set limits: {e}")))?
            .rows_affected();

        Ok(affected > 0)
    }
}

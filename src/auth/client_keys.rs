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

pub(crate) fn timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// Helper to read a nullable i64 column as Option<u64>
pub(crate) fn opt_u64(row: &turso::Row, idx: usize) -> Option<u64> {
    row.get::<Option<i64>>(idx).ok().flatten().map(|v| v as u64)
}

/// Helper to read a non-null i64 column as u64
pub(crate) fn get_u64(row: &turso::Row, idx: usize) -> u64 {
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
}

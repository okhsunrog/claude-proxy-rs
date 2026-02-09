use serde::{Deserialize, Serialize};

use crate::db;
use crate::error::ProxyError;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
#[allow(clippy::enum_variant_names)]
pub enum Auth {
    OAuth {
        access: String,
        refresh: String,
        expires: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "accountId")]
        account_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "enterpriseUrl")]
        enterprise_url: Option<String>,
    },
    Api {
        key: String,
    },
    #[serde(rename = "wellknown")]
    WellKnown {
        key: String,
        token: String,
    },
}

pub struct AuthStore;

impl AuthStore {
    pub fn new() -> Self {
        Self
    }

    pub async fn get(&self, provider: &str) -> Option<Auth> {
        let conn = db::get_conn().await.ok()?;
        let mut rows = conn
            .query(
                "SELECT auth_type, access_token, refresh_token, expires_at, account_id, enterprise_url FROM auth WHERE provider = ?",
                [provider],
            )
            .await
            .ok()?;

        let row = rows.next().await.ok()??;
        let auth_type: String = row.get(0).ok()?;

        match auth_type.as_str() {
            "oauth" => Some(Auth::OAuth {
                access: row.get(1).ok()?,
                refresh: row.get(2).ok()?,
                expires: row.get::<i64>(3).ok()? as u64,
                account_id: row
                    .get::<Option<String>>(4)
                    .ok()
                    .flatten()
                    .filter(|s| !s.is_empty()),
                enterprise_url: row
                    .get::<Option<String>>(5)
                    .ok()
                    .flatten()
                    .filter(|s| !s.is_empty()),
            }),
            "api" => Some(Auth::Api {
                key: row.get(1).ok()?,
            }),
            "wellknown" => Some(Auth::WellKnown {
                key: row.get(1).ok()?,
                token: row.get(2).ok()?,
            }),
            _ => None,
        }
    }

    pub async fn set(&self, provider: &str, auth: Auth) -> Result<(), ProxyError> {
        let conn = db::get_conn().await?;

        match &auth {
            Auth::OAuth {
                access,
                refresh,
                expires,
                account_id,
                enterprise_url,
            } => {
                // Insert core fields first
                conn.execute(
                    r#"INSERT OR REPLACE INTO auth (provider, auth_type, access_token, refresh_token, expires_at)
                       VALUES (?, 'oauth', ?, ?, ?)"#,
                    (provider, access.as_str(), refresh.as_str(), *expires as i64),
                )
                .await
                .map_err(|e| ProxyError::DatabaseError(format!("Failed to save auth: {e}")))?;

                // Set optional fields
                if let Some(aid) = account_id
                    && let Err(e) = conn
                        .execute(
                            "UPDATE auth SET account_id = ? WHERE provider = ?",
                            (aid.as_str(), provider),
                        )
                        .await
                {
                    tracing::warn!("Failed to save account_id for {provider}: {e}");
                }
                if let Some(eurl) = enterprise_url
                    && let Err(e) = conn
                        .execute(
                            "UPDATE auth SET enterprise_url = ? WHERE provider = ?",
                            (eurl.as_str(), provider),
                        )
                        .await
                {
                    tracing::warn!("Failed to save enterprise_url for {provider}: {e}");
                }
            }
            Auth::Api { key } => {
                conn.execute(
                    r#"INSERT OR REPLACE INTO auth (provider, auth_type, access_token, refresh_token, expires_at)
                       VALUES (?, 'api', ?, '', 0)"#,
                    (provider, key.as_str()),
                )
                .await
                .map_err(|e| ProxyError::DatabaseError(format!("Failed to save auth: {e}")))?;
            }
            Auth::WellKnown { key, token } => {
                conn.execute(
                    r#"INSERT OR REPLACE INTO auth (provider, auth_type, access_token, refresh_token, expires_at)
                       VALUES (?, 'wellknown', ?, ?, 0)"#,
                    (provider, key.as_str(), token.as_str()),
                )
                .await
                .map_err(|e| ProxyError::DatabaseError(format!("Failed to save auth: {e}")))?;
            }
        }

        Ok(())
    }

    pub async fn remove(&self, provider: &str) -> Result<(), ProxyError> {
        let conn = db::get_conn().await?;
        conn.execute("DELETE FROM auth WHERE provider = ?", [provider])
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to remove auth: {e}")))?;
        Ok(())
    }

    pub async fn has(&self, provider: &str) -> bool {
        let Ok(conn) = db::get_conn().await else {
            return false;
        };
        let Ok(mut rows) = conn
            .query("SELECT 1 FROM auth WHERE provider = ? LIMIT 1", [provider])
            .await
        else {
            return false;
        };
        rows.next().await.ok().flatten().is_some()
    }

    pub async fn update_tokens(
        &self,
        provider: &str,
        access: String,
        refresh: String,
        expires: u64,
    ) -> Result<(), ProxyError> {
        let conn = db::get_conn().await?;
        conn.execute(
            "UPDATE auth SET access_token = ?, refresh_token = ?, expires_at = ? WHERE provider = ? AND auth_type = 'oauth'",
            (access.as_str(), refresh.as_str(), expires as i64, provider),
        )
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to update tokens: {e}")))?;
        Ok(())
    }
}

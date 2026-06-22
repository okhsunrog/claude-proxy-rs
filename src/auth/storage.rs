use serde::{Deserialize, Serialize};
use sqlx::Row;

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
    /// Claude.ai web session credentials scraped from a browser login.
    /// Used only for the subscription-usage endpoint, which is aggressively
    /// rate-limited for OAuth clients but not for web sessions. The session
    /// key is rotated on every successful request (Set-Cookie), so stored
    /// value is updated automatically.
    ///
    /// Stored in the auth table with auth_type='web_session' and columns
    /// repurposed as: access_token=session_key, refresh_token=org_uuid,
    /// account_id=device_id, enterprise_url=anonymous_id.
    #[serde(rename = "web_session")]
    WebSession {
        session_key: String,
        org_uuid: String,
        device_id: String,
        anonymous_id: String,
    },
}

pub struct AuthStore;

impl AuthStore {
    pub fn new() -> Self {
        Self
    }

    pub async fn get(&self, provider: &str) -> Option<Auth> {
        let conn = db::get_conn().await.ok()?;
        let row = sqlx::query(
            "SELECT auth_type, access_token, refresh_token, expires_at, account_id, enterprise_url FROM auth WHERE provider = $1",
        )
        .bind(provider)
        .fetch_optional(&conn)
        .await
            .ok()?;

        let row = row?;
        let auth_type: String = row.try_get(0).ok()?;

        match auth_type.as_str() {
            "oauth" => Some(Auth::OAuth {
                access: row.try_get(1).ok()?,
                refresh: row.try_get(2).ok()?,
                expires: row.try_get::<i64, _>(3).ok()? as u64,
                account_id: row
                    .try_get::<Option<String>, _>(4)
                    .ok()
                    .flatten()
                    .filter(|s| !s.is_empty()),
                enterprise_url: row
                    .try_get::<Option<String>, _>(5)
                    .ok()
                    .flatten()
                    .filter(|s| !s.is_empty()),
            }),
            "api" => Some(Auth::Api {
                key: row.try_get(1).ok()?,
            }),
            "wellknown" => Some(Auth::WellKnown {
                key: row.try_get(1).ok()?,
                token: row.try_get(2).ok()?,
            }),
            "web_session" => Some(Auth::WebSession {
                session_key: row.try_get(1).ok()?,
                org_uuid: row.try_get(2).ok()?,
                device_id: row
                    .try_get::<Option<String>, _>(4)
                    .ok()
                    .flatten()
                    .unwrap_or_default(),
                anonymous_id: row
                    .try_get::<Option<String>, _>(5)
                    .ok()
                    .flatten()
                    .unwrap_or_default(),
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
                sqlx::query(
                    r#"INSERT INTO auth (provider, auth_type, access_token, refresh_token, expires_at, account_id, enterprise_url)
                       VALUES ($1, 'oauth', $2, $3, $4, $5, $6)
                       ON CONFLICT (provider) DO UPDATE SET
                           auth_type = EXCLUDED.auth_type,
                           access_token = EXCLUDED.access_token,
                           refresh_token = EXCLUDED.refresh_token,
                           expires_at = EXCLUDED.expires_at,
                           account_id = EXCLUDED.account_id,
                           enterprise_url = EXCLUDED.enterprise_url"#,
                )
                .bind(provider)
                .bind(access.as_str())
                .bind(refresh.as_str())
                .bind(*expires as i64)
                .bind(account_id.as_deref())
                .bind(enterprise_url.as_deref())
                .execute(&conn)
                .await
                .map_err(|e| ProxyError::DatabaseError(format!("Failed to save auth: {e}")))?;
            }
            Auth::Api { key } => {
                sqlx::query(
                    r#"INSERT INTO auth (provider, auth_type, access_token, refresh_token, expires_at, account_id, enterprise_url)
                       VALUES ($1, 'api', $2, '', 0, NULL, NULL)
                       ON CONFLICT (provider) DO UPDATE SET
                           auth_type = EXCLUDED.auth_type,
                           access_token = EXCLUDED.access_token,
                           refresh_token = EXCLUDED.refresh_token,
                           expires_at = EXCLUDED.expires_at,
                           account_id = EXCLUDED.account_id,
                           enterprise_url = EXCLUDED.enterprise_url"#,
                )
                .bind(provider)
                .bind(key.as_str())
                .execute(&conn)
                .await
                .map_err(|e| ProxyError::DatabaseError(format!("Failed to save auth: {e}")))?;
            }
            Auth::WellKnown { key, token } => {
                sqlx::query(
                    r#"INSERT INTO auth (provider, auth_type, access_token, refresh_token, expires_at, account_id, enterprise_url)
                       VALUES ($1, 'wellknown', $2, $3, 0, NULL, NULL)
                       ON CONFLICT (provider) DO UPDATE SET
                           auth_type = EXCLUDED.auth_type,
                           access_token = EXCLUDED.access_token,
                           refresh_token = EXCLUDED.refresh_token,
                           expires_at = EXCLUDED.expires_at,
                           account_id = EXCLUDED.account_id,
                           enterprise_url = EXCLUDED.enterprise_url"#,
                )
                .bind(provider)
                .bind(key.as_str())
                .bind(token.as_str())
                .execute(&conn)
                .await
                .map_err(|e| ProxyError::DatabaseError(format!("Failed to save auth: {e}")))?;
            }
            Auth::WebSession {
                session_key,
                org_uuid,
                device_id,
                anonymous_id,
            } => {
                sqlx::query(
                    r#"INSERT INTO auth (provider, auth_type, access_token, refresh_token, expires_at, account_id, enterprise_url)
                       VALUES ($1, 'web_session', $2, $3, 0, $4, $5)
                       ON CONFLICT (provider) DO UPDATE SET
                           auth_type = EXCLUDED.auth_type,
                           access_token = EXCLUDED.access_token,
                           refresh_token = EXCLUDED.refresh_token,
                           expires_at = EXCLUDED.expires_at,
                           account_id = EXCLUDED.account_id,
                           enterprise_url = EXCLUDED.enterprise_url"#,
                )
                .bind(provider)
                .bind(session_key.as_str())
                .bind(org_uuid.as_str())
                .bind(device_id.as_str())
                .bind(anonymous_id.as_str())
                .execute(&conn)
                .await
                .map_err(|e| ProxyError::DatabaseError(format!("Failed to save auth: {e}")))?;
            }
        }

        Ok(())
    }

    pub async fn remove(&self, provider: &str) -> Result<(), ProxyError> {
        let conn = db::get_conn().await?;
        sqlx::query("DELETE FROM auth WHERE provider = $1")
            .bind(provider)
            .execute(&conn)
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to remove auth: {e}")))?;
        Ok(())
    }

    pub async fn has(&self, provider: &str) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;
        let row = sqlx::query("SELECT 1 FROM auth WHERE provider = $1 LIMIT 1")
            .bind(provider)
            .fetch_optional(&conn)
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to check auth: {e}")))?;
        Ok(row.is_some())
    }

    pub async fn update_tokens(
        &self,
        provider: &str,
        access: String,
        refresh: String,
        expires: u64,
    ) -> Result<(), ProxyError> {
        let conn = db::get_conn().await?;
        sqlx::query(
            "UPDATE auth SET access_token = $1, refresh_token = $2, expires_at = $3 WHERE provider = $4 AND auth_type = 'oauth'",
        )
        .bind(access.as_str())
        .bind(refresh.as_str())
        .bind(expires as i64)
        .bind(provider)
        .execute(&conn)
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to update tokens: {e}")))?;
        Ok(())
    }

    /// Update just the session_key of a stored WebSession. Used when the
    /// claude.ai server rotates the cookie on a response.
    pub async fn update_web_session_key(
        &self,
        provider: &str,
        new_session_key: &str,
    ) -> Result<(), ProxyError> {
        let conn = db::get_conn().await?;
        sqlx::query(
            "UPDATE auth SET access_token = $1 WHERE provider = $2 AND auth_type = 'web_session'",
        )
        .bind(new_session_key)
        .bind(provider)
        .execute(&conn)
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to update web session: {e}")))?;
        Ok(())
    }
}

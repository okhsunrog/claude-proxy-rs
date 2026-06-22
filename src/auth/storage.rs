use serde::{Deserialize, Serialize};

use super::client_keys::i64_to_u64;
use crate::db;
use crate::error::{DbResultExt, ProxyError};

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
        let row = sqlx::query!(
            "SELECT auth_type, access_token, refresh_token, expires_at, account_id, enterprise_url FROM auth WHERE provider = $1",
            provider,
        )
        .fetch_optional(&conn)
        .await
        .ok()?;

        let row = row?;

        match row.auth_type.as_str() {
            "oauth" => Some(Auth::OAuth {
                access: row.access_token,
                refresh: row.refresh_token,
                expires: i64_to_u64(row.expires_at),
                account_id: row.account_id.filter(|s| !s.is_empty()),
                enterprise_url: row.enterprise_url.filter(|s| !s.is_empty()),
            }),
            "api" => Some(Auth::Api {
                key: row.access_token,
            }),
            "wellknown" => Some(Auth::WellKnown {
                key: row.access_token,
                token: row.refresh_token,
            }),
            "web_session" => Some(Auth::WebSession {
                session_key: row.access_token,
                org_uuid: row.refresh_token,
                device_id: row.account_id.unwrap_or_default(),
                anonymous_id: row.enterprise_url.unwrap_or_default(),
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
                sqlx::query!(
                    r#"INSERT INTO auth (provider, auth_type, access_token, refresh_token, expires_at, account_id, enterprise_url)
                       VALUES ($1, 'oauth', $2, $3, $4, $5, $6)
                       ON CONFLICT (provider) DO UPDATE SET
                           auth_type = EXCLUDED.auth_type,
                           access_token = EXCLUDED.access_token,
                           refresh_token = EXCLUDED.refresh_token,
                           expires_at = EXCLUDED.expires_at,
                           account_id = EXCLUDED.account_id,
                           enterprise_url = EXCLUDED.enterprise_url"#,
                    provider,
                    access.as_str(),
                    refresh.as_str(),
                    *expires as i64,
                    account_id.as_deref(),
                    enterprise_url.as_deref(),
                )
                .execute(&conn)
                .await
                .db_context("Failed to save auth")?;
            }
            Auth::Api { key } => {
                sqlx::query!(
                    r#"INSERT INTO auth (provider, auth_type, access_token, refresh_token, expires_at, account_id, enterprise_url)
                       VALUES ($1, 'api', $2, '', 0, NULL, NULL)
                       ON CONFLICT (provider) DO UPDATE SET
                           auth_type = EXCLUDED.auth_type,
                           access_token = EXCLUDED.access_token,
                           refresh_token = EXCLUDED.refresh_token,
                           expires_at = EXCLUDED.expires_at,
                           account_id = EXCLUDED.account_id,
                           enterprise_url = EXCLUDED.enterprise_url"#,
                    provider,
                    key.as_str(),
                )
                .execute(&conn)
                .await
                .db_context("Failed to save auth")?;
            }
            Auth::WellKnown { key, token } => {
                sqlx::query!(
                    r#"INSERT INTO auth (provider, auth_type, access_token, refresh_token, expires_at, account_id, enterprise_url)
                       VALUES ($1, 'wellknown', $2, $3, 0, NULL, NULL)
                       ON CONFLICT (provider) DO UPDATE SET
                           auth_type = EXCLUDED.auth_type,
                           access_token = EXCLUDED.access_token,
                           refresh_token = EXCLUDED.refresh_token,
                           expires_at = EXCLUDED.expires_at,
                           account_id = EXCLUDED.account_id,
                           enterprise_url = EXCLUDED.enterprise_url"#,
                    provider,
                    key.as_str(),
                    token.as_str(),
                )
                .execute(&conn)
                .await
                .db_context("Failed to save auth")?;
            }
            Auth::WebSession {
                session_key,
                org_uuid,
                device_id,
                anonymous_id,
            } => {
                sqlx::query!(
                    r#"INSERT INTO auth (provider, auth_type, access_token, refresh_token, expires_at, account_id, enterprise_url)
                       VALUES ($1, 'web_session', $2, $3, 0, $4, $5)
                       ON CONFLICT (provider) DO UPDATE SET
                           auth_type = EXCLUDED.auth_type,
                           access_token = EXCLUDED.access_token,
                           refresh_token = EXCLUDED.refresh_token,
                           expires_at = EXCLUDED.expires_at,
                           account_id = EXCLUDED.account_id,
                           enterprise_url = EXCLUDED.enterprise_url"#,
                    provider,
                    session_key.as_str(),
                    org_uuid.as_str(),
                    device_id.as_str(),
                    anonymous_id.as_str(),
                )
                .execute(&conn)
                .await
                .db_context("Failed to save auth")?;
            }
        }

        Ok(())
    }

    pub async fn remove(&self, provider: &str) -> Result<(), ProxyError> {
        let conn = db::get_conn().await?;
        sqlx::query!("DELETE FROM auth WHERE provider = $1", provider)
            .execute(&conn)
            .await
            .db_context("Failed to remove auth")?;
        Ok(())
    }

    pub async fn has(&self, provider: &str) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;
        let row = sqlx::query!(
            "SELECT 1 AS \"exists!\" FROM auth WHERE provider = $1 LIMIT 1",
            provider
        )
        .fetch_optional(&conn)
        .await
        .db_context("Failed to check auth")?;
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
        sqlx::query!(
            "UPDATE auth SET access_token = $1, refresh_token = $2, expires_at = $3 WHERE provider = $4 AND auth_type = 'oauth'",
            access.as_str(),
            refresh.as_str(),
            expires as i64,
            provider,
        )
        .execute(&conn)
        .await
        .db_context("Failed to update tokens")?;
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
        sqlx::query!(
            "UPDATE auth SET access_token = $1 WHERE provider = $2 AND auth_type = 'web_session'",
            new_session_key,
            provider,
        )
        .execute(&conn)
        .await
        .db_context("Failed to update web session")?;
        Ok(())
    }
}

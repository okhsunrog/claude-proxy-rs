use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Instant;
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use tokio::sync::{Mutex, RwLock};
use tracing::warn;
use urlencoding::encode;

use super::storage::{Auth, AuthStore};
use crate::error::ProxyError;

const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const AUTHORIZE_URL: &str = "https://claude.com/cai/oauth/authorize";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
/// Scopes requested at initial authorization (ALL_OAUTH_SCOPES from Claude Code).
const AUTHORIZE_SCOPES: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";

/// Scopes requested at token refresh (CLAUDE_AI_OAUTH_SCOPES from Claude Code).
/// Excludes org:create_api_key — matches Claude Code's refresh behavior.
const REFRESH_SCOPES: &str =
    "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";

#[derive(Debug, Serialize, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
    token_type: String,
    #[serde(default)]
    account: Option<serde_json::Value>,
}

fn account_uuid_from_value(value: &serde_json::Value) -> Option<String> {
    value
        .get("uuid")
        .or_else(|| value.get("account_uuid"))
        .and_then(serde_json::Value::as_str)
        .filter(|id| uuid::Uuid::parse_str(id).is_ok())
        .map(str::to_owned)
}

struct PendingFlow {
    verifier: String,
    state: String,
    started: Instant,
}

pub struct OAuthManager {
    client: Client,
    pending: RwLock<Option<PendingFlow>>,
    auth_store: Arc<AuthStore>,
    /// Prevents concurrent token refreshes (Anthropic rotates refresh tokens,
    /// so two simultaneous refreshes would invalidate each other).
    refresh_lock: Mutex<()>,
    account_cache: Mutex<Option<(String, Option<String>, Instant)>>,
}

impl OAuthManager {
    pub fn new(client: Client, auth_store: Arc<AuthStore>) -> Self {
        Self {
            client,
            pending: RwLock::new(None),
            auth_store,
            refresh_lock: Mutex::new(()),
            account_cache: Mutex::new(None),
        }
    }

    fn generate_verifier() -> String {
        let mut rng = rand::rng();
        let mut bytes = [0u8; 32];
        rng.fill(&mut bytes);
        URL_SAFE_NO_PAD.encode(bytes)
    }

    fn generate_challenge(verifier: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let hash = hasher.finalize();
        URL_SAFE_NO_PAD.encode(hash)
    }

    pub async fn start_flow(&self) -> String {
        let verifier = Self::generate_verifier();
        let challenge = Self::generate_challenge(&verifier);

        let state = Self::generate_verifier();
        *self.pending.write().await = Some(PendingFlow {
            verifier,
            state: state.clone(),
            started: Instant::now(),
        });

        format!(
            "{}?code=true&client_id={}&response_type=code&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
            AUTHORIZE_URL,
            CLIENT_ID,
            encode(REDIRECT_URI),
            encode(AUTHORIZE_SCOPES),
            challenge,
            state
        )
    }

    pub async fn exchange_code(&self, code: &str) -> Result<(), String> {
        let (actual_code, state) = code.trim().split_once('#').ok_or("Expected code#state")?;
        let verifier = self.take_verifier(actual_code, state).await?;

        let body = json!({
            "code": actual_code,
            "state": state,
            "grant_type": "authorization_code",
            "client_id": CLIENT_ID,
            "redirect_uri": REDIRECT_URI,
            "code_verifier": verifier,
        });

        let response = self
            .client
            .post(TOKEN_URL)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Failed to exchange code: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Token exchange failed ({}): {}", status, text));
        }

        let token = response
            .json::<TokenResponse>()
            .await
            .map_err(|e| format!("Failed to parse token response: {}", e))?;

        let expires = now_millis() + (token.expires_in * 1000);
        let account_id = token.account.as_ref().and_then(account_uuid_from_value);

        self.auth_store
            .set(
                "anthropic",
                Auth::OAuth {
                    access: token.access_token,
                    refresh: token.refresh_token,
                    expires,
                    account_id,
                    enterprise_url: None,
                },
            )
            .await
            .map_err(|e| format!("Failed to save auth: {}", e))?;

        Ok(())
    }

    /// Resolve identity for the token owner, never from caller-provided metadata.
    /// Failed profile lookups are retried after a minute without blocking inference.
    pub async fn account_uuid(&self, token: &str) -> Option<String> {
        self.account_uuid_at(token, crate::constants::ANTHROPIC_PROFILE_URL)
            .await
    }

    async fn account_uuid_at(&self, token: &str, profile_url: &str) -> Option<String> {
        if let Some(Auth::OAuth {
            account_id: Some(id),
            access,
            ..
        }) = self.auth_store.get("anthropic").await
            && access == token
            && uuid::Uuid::parse_str(&id).is_ok()
        {
            return Some(id);
        }
        let mut cache = self.account_cache.lock().await;
        if let Some((cached_token, id, time)) = cache.as_ref() {
            let ttl = if id.is_some() { 1800 } else { 60 };
            if cached_token == token && time.elapsed().as_secs() < ttl {
                return id.clone();
            }
        }
        let id = match self
            .client
            .get(profile_url)
            .bearer_auth(token)
            .header("anthropic-beta", crate::constants::OAUTH_USAGE_BETA)
            .header("user-agent", crate::constants::USER_AGENT)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => response
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| v.get("account").and_then(account_uuid_from_value)),
            _ => None,
        };
        *cache = Some((token.to_owned(), id.clone(), Instant::now()));
        id
    }

    async fn take_verifier(&self, code: &str, state: &str) -> Result<String, String> {
        let mut pending = self.pending.write().await;
        let flow = pending.as_ref().ok_or("No OAuth flow in progress")?;
        if flow.started.elapsed() >= std::time::Duration::from_secs(600) {
            *pending = None;
            return Err("OAuth flow expired; start again".into());
        }
        if code.is_empty()
            || state.is_empty()
            || !bool::from(flow.state.as_bytes().ct_eq(state.as_bytes()))
        {
            return Err("Invalid OAuth callback state or empty code".into());
        }
        // Consume before network I/O: concurrent callbacks cannot exchange twice.
        pending
            .take()
            .map(|flow| flow.verifier)
            .ok_or_else(|| "No OAuth flow in progress".into())
    }

    async fn do_refresh(&self, refresh: String) -> Result<Option<String>, String> {
        let body = json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh,
            "client_id": CLIENT_ID,
            "scope": REFRESH_SCOPES,
        });

        let response = self
            .client
            .post(TOKEN_URL)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Failed to refresh token: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();

            // If the refresh token is invalid (e.g. rotated or revoked),
            // clear the stale credentials so the UI shows "Connect" instead
            // of endlessly failing.
            if text.contains("invalid_grant") {
                warn!("OAuth refresh token is invalid, clearing stale credentials");
                if let Err(e) = self.auth_store.remove("anthropic").await {
                    warn!("Failed to clear stale OAuth credentials: {e}");
                }
                return Ok(None);
            }

            return Err(format!("Token refresh failed ({}): {}", status, text));
        }

        let token = response
            .json::<TokenResponse>()
            .await
            .map_err(|e| format!("Failed to parse refresh response: {}", e))?;

        let new_expires = now_millis() + (token.expires_in * 1000);

        self.auth_store
            .update_tokens(
                "anthropic",
                token.access_token.clone(),
                token.refresh_token,
                new_expires,
            )
            .await
            .map_err(|e| format!("Failed to save refreshed auth: {}", e))?;

        Ok(Some(token.access_token))
    }

    pub async fn refresh_if_needed(&self) -> Result<Option<String>, String> {
        // Fast path: check without the lock first.
        {
            let auth = match self.auth_store.get("anthropic").await {
                Some(auth) => auth,
                None => return Ok(None),
            };
            match auth {
                Auth::Api { key } => return Ok(Some(key)),
                Auth::WellKnown { token, .. } => return Ok(Some(token)),
                Auth::WebSession { .. } => return Ok(None),
                Auth::OAuth {
                    access, expires, ..
                } => {
                    if now_millis() + 300_000 < expires {
                        return Ok(Some(access));
                    }
                }
            }
        }

        // Acquire the mutex so only one refresh happens at a time.
        let _guard = self.refresh_lock.lock().await;

        // Re-check after acquiring the lock — another task may have already refreshed.
        let auth = match self.auth_store.get("anthropic").await {
            Some(auth) => auth,
            None => return Ok(None),
        };
        let (access, refresh, expires) = match auth {
            Auth::Api { key } => return Ok(Some(key)),
            Auth::WellKnown { token, .. } => return Ok(Some(token)),
            Auth::WebSession { .. } => return Ok(None),
            Auth::OAuth {
                access,
                refresh,
                expires,
                ..
            } => (access, refresh, expires),
        };
        if now_millis() + 300_000 < expires {
            return Ok(Some(access));
        }

        self.do_refresh(refresh).await
    }

    /// Force a token refresh regardless of expiry. Used when Anthropic returns 401
    /// to recover from server-side token revocation without waiting for local expiry.
    pub async fn force_refresh(&self) -> Result<Option<String>, String> {
        let _guard = self.refresh_lock.lock().await;

        let auth = match self.auth_store.get("anthropic").await {
            Some(auth) => auth,
            None => return Ok(None),
        };

        let refresh = match auth {
            Auth::OAuth { refresh, .. } => refresh,
            Auth::Api { key } => return Ok(Some(key)),
            Auth::WellKnown { token, .. } => return Ok(Some(token)),
            Auth::WebSession { .. } => return Ok(None),
        };

        self.do_refresh(refresh).await
    }

    pub async fn logout(&self) -> Result<(), ProxyError> {
        *self.pending.write().await = None;
        self.auth_store.remove("anthropic").await
    }

    pub async fn is_authenticated(&self) -> bool {
        self.auth_store.has("anthropic").await.unwrap_or(false)
    }
}

fn now_millis() -> u64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    u64::try_from(millis).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn callback_is_bound_to_latest_flow_and_consumed_once() {
        let manager = OAuthManager::new(Client::new(), Arc::new(AuthStore::new()));
        let first = manager.start_flow().await;
        let first_state = url::Url::parse(&first)
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .into_owned();
        let second = manager.start_flow().await;
        let state = url::Url::parse(&second)
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .into_owned();
        manager
            .take_verifier("code", &first_state)
            .await
            .unwrap_err();
        manager.take_verifier("", &state).await.unwrap_err();
        let verifier = manager.take_verifier("code", &state).await.unwrap();
        assert_ne!(verifier, state);
        manager.take_verifier("code", &state).await.unwrap_err();
    }

    #[tokio::test]
    async fn expired_callback_is_rejected() {
        let manager = OAuthManager::new(Client::new(), Arc::new(AuthStore::new()));
        *manager.pending.write().await = Some(PendingFlow {
            verifier: "v".into(),
            state: "s".into(),
            started: Instant::now()
                .checked_sub(std::time::Duration::from_secs(601))
                .unwrap(),
        });
        manager.take_verifier("code", "s").await.unwrap_err();
        assert!(manager.pending.read().await.is_none());
    }
    #[tokio::test]
    async fn resolves_and_caches_profile_identity_for_existing_logins() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/profile", listener.local_addr().unwrap());
        let expected = uuid::Uuid::new_v4().to_string();
        let owner = expected.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut received = Vec::new();
            loop {
                let mut buffer = [0; 4096];
                let count = socket.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                received.extend_from_slice(&buffer[..count]);
                if received.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(received).unwrap().to_lowercase();
            assert!(request.contains("authorization: bearer local-profile-token"));
            let body = serde_json::json!({"account":{"uuid":owner}}).to_string();
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes()).await.unwrap();
        });
        let manager = OAuthManager::new(
            Client::builder().no_proxy().build().unwrap(),
            Arc::new(AuthStore::new()),
        );
        assert_eq!(
            manager
                .account_uuid_at("local-profile-token", &url)
                .await
                .as_deref(),
            Some(expected.as_str())
        );
        server.await.unwrap();
        // The server is gone; this must come from the token-specific cache.
        assert_eq!(
            manager
                .account_uuid_at("local-profile-token", &url)
                .await
                .as_deref(),
            Some(expected.as_str())
        );
        let response: TokenResponse = serde_json::from_value(serde_json::json!({"access_token":"test", "refresh_token":"test", "expires_in":3600,"token_type":"Bearer","account":{"uuid":expected}})).unwrap();
        assert_eq!(
            response
                .account
                .as_ref()
                .and_then(account_uuid_from_value)
                .as_deref(),
            Some(expected.as_str())
        );
    }
}

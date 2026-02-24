use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::warn;

use super::storage::{Auth, AuthStore};
use crate::error::ProxyError;

const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
const REDIRECT_URI: &str = "https://console.anthropic.com/oauth/code/callback";
const SCOPES: &str = "org:create_api_key user:profile user:inference";

#[derive(Debug, Serialize, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
    token_type: String,
}

pub struct OAuthManager {
    client: Client,
    verifier: RwLock<Option<String>>,
    auth_store: Arc<AuthStore>,
}

impl OAuthManager {
    pub fn new(client: Client, auth_store: Arc<AuthStore>) -> Self {
        Self {
            client,
            verifier: RwLock::new(None),
            auth_store,
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

        *self.verifier.write().await = Some(verifier.clone());

        format!(
            "{}?code=true&client_id={}&response_type=code&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
            AUTHORIZE_URL,
            CLIENT_ID,
            urlencoding::encode(REDIRECT_URI),
            urlencoding::encode(SCOPES),
            challenge,
            verifier
        )
    }

    pub async fn exchange_code(&self, code: &str) -> Result<(), String> {
        let verifier = self
            .verifier
            .read()
            .await
            .clone()
            .ok_or("No OAuth flow in progress")?;

        // Code format is "actual_code#state"
        let parts: Vec<&str> = code.split('#').collect();
        let actual_code = parts[0];
        let state = parts.get(1).copied().unwrap_or("");

        let body = serde_json::json!({
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

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let expires = now + (token.expires_in * 1000);

        self.auth_store
            .set(
                "anthropic",
                Auth::OAuth {
                    access: token.access_token,
                    refresh: token.refresh_token,
                    expires,
                    account_id: None,
                    enterprise_url: None,
                },
            )
            .await
            .map_err(|e| format!("Failed to save auth: {}", e))?;

        *self.verifier.write().await = None;

        Ok(())
    }

    pub async fn refresh_if_needed(&self) -> Result<Option<String>, String> {
        let auth = match self.auth_store.get("anthropic").await {
            Some(auth) => auth,
            None => return Ok(None),
        };

        let (access, refresh, expires) = match auth {
            Auth::OAuth {
                access,
                refresh,
                expires,
                ..
            } => (access, refresh, expires),
            Auth::Api { key } => return Ok(Some(key)),
            Auth::WellKnown { token, .. } => return Ok(Some(token)),
        };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        if now + 60_000 < expires {
            return Ok(Some(access));
        }

        let body = serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh,
            "client_id": CLIENT_ID,
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
                let _ = self.auth_store.remove("anthropic").await;
                return Ok(None);
            }

            return Err(format!("Token refresh failed ({}): {}", status, text));
        }

        let token = response
            .json::<TokenResponse>()
            .await
            .map_err(|e| format!("Failed to parse refresh response: {}", e))?;

        let new_expires = now + (token.expires_in * 1000);

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

    pub async fn logout(&self) -> Result<(), ProxyError> {
        *self.verifier.write().await = None;
        self.auth_store.remove("anthropic").await
    }

    pub async fn is_authenticated(&self) -> bool {
        self.auth_store.has("anthropic").await.unwrap_or(false)
    }
}

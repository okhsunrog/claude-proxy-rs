//! Single-account subscription credentials. Device codes remain server-side except
//! for the user-facing verification code; tokens are never returned by admin APIs.
use super::storage::{Auth, AuthStore};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;
use utoipa::ToSchema;

const PROVIDER: &str = "chatgpt";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AUTH_BASE: &str = "https://auth.openai.com";
const LOGIN_TTL: Duration = Duration::from_secs(900);

#[derive(Serialize, ToSchema)]
pub struct DeviceLogin {
    pub verification_url: String,
    pub user_code: String,
    pub interval: u64,
}
#[derive(Serialize, ToSchema)]
pub struct ConnectionStatus {
    pub authenticated: bool,
}
#[derive(Serialize, ToSchema)]
pub struct AvailableModel {
    pub id: String,
    pub name: String,
}
#[derive(Deserialize)]
struct DeviceCode {
    device_auth_id: String,
    #[serde(alias = "usercode")]
    user_code: String,
    interval: Option<Value>,
}
struct Pending {
    code: DeviceCode,
    started: Instant,
    next_poll: Instant,
    interval: Duration,
}
#[derive(Deserialize)]
struct AuthorizationCode {
    authorization_code: String,
    code_verifier: String,
}
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    expires_in: u64,
    id_token: Option<String>,
}

pub struct ChatGptAuth {
    client: Client,
    store: Arc<AuthStore>,
    // Serialize login, polling, refresh and logout, including persistence.
    pending: Mutex<Option<Pending>>,
}

impl ChatGptAuth {
    pub async fn available_models(&self) -> Result<Vec<AvailableModel>, String> {
        let (mut token, mut account) = self.credentials(None).await?;
        for attempt in 0..2 {
            let mut request = self
                .client
                .get("https://chatgpt.com/backend-api/codex/models?client_version=0.154.0")
                .header("originator", "codex_cli_rs")
                .bearer_auth(&token)
                .timeout(Duration::from_secs(20));
            if let Some(account) = &account {
                request = request.header("ChatGPT-Account-Id", account);
            }
            let response = request
                .send()
                .await
                .map_err(|_error| "Cannot load ChatGPT models")?;
            if response.status() == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                (token, account) = self.credentials(Some(&token)).await?;
                continue;
            }
            let data: Value = read_response(response).await?;
            let models = data
                .get("models")
                .and_then(Value::as_array)
                .ok_or("Invalid model catalog")?;
            return Ok(models
                .iter()
                .filter_map(|model| {
                    let id = model.get("slug")?.as_str()?;
                    Some(AvailableModel {
                        id: id.into(),
                        name: model
                            .get("display_name")
                            .and_then(Value::as_str)
                            .unwrap_or(id)
                            .into(),
                    })
                })
                .collect());
        }
        Err("Cannot authenticate model catalog request".into())
    }
    pub fn new(client: Client, store: Arc<AuthStore>) -> Self {
        Self {
            client,
            store,
            pending: Mutex::new(None),
        }
    }
    pub async fn status(&self) -> Result<ConnectionStatus, String> {
        Ok(ConnectionStatus {
            authenticated: self
                .store
                .has(PROVIDER)
                .await
                .map_err(|_error| "Cannot read ChatGPT credentials")?,
        })
    }
    pub async fn start(&self) -> Result<DeviceLogin, String> {
        let mut pending = self.pending.lock().await;
        let response = self
            .client
            .post(format!("{AUTH_BASE}/api/accounts/deviceauth/usercode"))
            .timeout(Duration::from_secs(20))
            .json(&json!({"client_id": CLIENT_ID}))
            .send()
            .await
            .map_err(|_error| "Cannot reach ChatGPT login service")?;
        let code: DeviceCode = read_response(response).await?;
        if code.device_auth_id.is_empty() || code.user_code.is_empty() {
            return Err("Invalid device login response".into());
        }
        let interval = poll_interval(code.interval.as_ref());
        let result = DeviceLogin {
            verification_url: format!("{AUTH_BASE}/codex/device"),
            user_code: code.user_code.clone(),
            interval,
        };
        let now = Instant::now();
        *pending = Some(Pending {
            code,
            started: now,
            next_poll: now + Duration::from_secs(interval),
            interval: Duration::from_secs(interval),
        });
        Ok(result)
    }
    pub async fn poll(&self) -> Result<ConnectionStatus, String> {
        let mut pending = self.pending.lock().await;
        let flow = pending.as_mut().ok_or("No pending ChatGPT login")?;
        if flow.started.elapsed() >= LOGIN_TTL {
            *pending = None;
            return Err("Login expired; connect again".into());
        }
        if Instant::now() < flow.next_poll {
            return Ok(ConnectionStatus {
                authenticated: false,
            });
        }
        flow.next_poll = Instant::now() + flow.interval;
        let response = self.client.post(format!("{AUTH_BASE}/api/accounts/deviceauth/token"))
            .timeout(Duration::from_secs(20))
            .json(&json!({"device_auth_id": flow.code.device_auth_id, "user_code": flow.code.user_code}))
            .send().await.map_err(|_error| "Cannot check ChatGPT login")?;
        if matches!(response.status().as_u16(), 403 | 404) {
            return Ok(ConnectionStatus {
                authenticated: false,
            });
        }
        let code: AuthorizationCode = read_response(response).await?;
        // Authorization codes are single-use, including when exchange fails.
        *pending = None;
        let response = self
            .client
            .post(format!("{AUTH_BASE}/oauth/token"))
            .timeout(Duration::from_secs(20))
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", CLIENT_ID),
                ("code", code.authorization_code.as_str()),
                ("code_verifier", code.code_verifier.as_str()),
                (
                    "redirect_uri",
                    "https://auth.openai.com/deviceauth/callback",
                ),
            ])
            .send()
            .await
            .map_err(|_error| "Cannot exchange ChatGPT login code; connect again")?;
        let token: TokenResponse = read_response(response).await?;
        self.save(token, None, None).await?;
        Ok(ConnectionStatus {
            authenticated: true,
        })
    }
    pub async fn logout(&self) -> Result<(), String> {
        let mut pending = self.pending.lock().await;
        *pending = None;
        self.store
            .remove(PROVIDER)
            .await
            .map_err(|_error| "Cannot remove ChatGPT credentials".into())
    }
    /// A rejected token triggers at most one refresh across concurrent 401s.
    pub async fn credentials(
        &self,
        rejected: Option<&str>,
    ) -> Result<(String, Option<String>), String> {
        let _lock = self.pending.lock().await;
        let Some(Auth::OAuth {
            access,
            refresh,
            expires,
            account_id,
            ..
        }) = self.store.get(PROVIDER).await
        else {
            return Err("Connect a ChatGPT account in the admin panel".into());
        };
        if expires > now_ms() + 300_000 && rejected != Some(access.as_str()) {
            return Ok((access, account_id));
        }
        let response = self
            .client
            .post(format!("{AUTH_BASE}/oauth/token"))
            .timeout(Duration::from_secs(20))
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", CLIENT_ID),
                ("refresh_token", refresh.as_str()),
            ])
            .send()
            .await
            .map_err(|_error| "Cannot refresh ChatGPT login")?;
        let token: TokenResponse = read_response(response).await?;
        self.save(token, Some(refresh), account_id).await
    }
    async fn save(
        &self,
        token: TokenResponse,
        old_refresh: Option<String>,
        old_account: Option<String>,
    ) -> Result<(String, Option<String>), String> {
        if token.access_token.is_empty() || token.expires_in == 0 {
            return Err("Invalid token response".into());
        }
        let refresh = if token.refresh_token.is_empty() {
            old_refresh.unwrap_or_default()
        } else {
            token.refresh_token
        };
        if refresh.is_empty() {
            return Err("Login did not return a refresh token".into());
        }
        // Read identity metadata only; JWT parsing here is not token verification.
        let account_id = token
            .id_token
            .as_deref()
            .and_then(account_id)
            .or_else(|| account_id(&token.access_token))
            .or(old_account);
        self.store
            .set(
                PROVIDER,
                Auth::OAuth {
                    access: token.access_token.clone(),
                    refresh,
                    expires: now_ms().saturating_add(token.expires_in.saturating_mul(1000)),
                    account_id: account_id.clone(),
                    enterprise_url: None,
                },
            )
            .await
            .map_err(|_error| "Cannot save ChatGPT credentials")?;
        Ok((token.access_token, account_id))
    }
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .saturating_mul(1000)
}
fn poll_interval(value: Option<&Value>) -> u64 {
    value
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(5)
        .clamp(5, 60)
}
fn account_id(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let value: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
    value
        .get("https://api.openai.com/auth")?
        .get("chatgpt_account_id")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
async fn read_response<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, String> {
    if !response.status().is_success() {
        return Err(format!(
            "ChatGPT login service returned HTTP {}",
            response.status().as_u16()
        ));
    }
    response
        .json()
        .await
        .map_err(|_error| "Invalid ChatGPT login response".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_numeric_and_string_poll_intervals() {
        assert_eq!(poll_interval(Some(&json!("8"))), 8);
        assert_eq!(poll_interval(Some(&json!(0))), 5);
        assert_eq!(poll_interval(None), 5);
    }
    #[test]
    fn reads_account_claim_without_panicking_on_malformed_tokens() {
        assert!(account_id("broken").is_none());
        let payload = URL_SAFE_NO_PAD
            .encode(br#"{"https://api.openai.com/auth":{"chatgpt_account_id":"account"}}"#);
        assert_eq!(
            account_id(&format!("header.{payload}.signature")).as_deref(),
            Some("account")
        );
    }
}

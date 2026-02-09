use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use utoipa::ToSchema;
use uuid::Uuid;

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
    /// Token usage limits (optional)
    #[serde(default, skip_serializing_if = "is_default_limits")]
    pub limits: TokenLimits,
    /// Current token usage
    #[serde(default, skip_serializing_if = "is_default_usage")]
    pub usage: TokenUsage,
}

fn is_default_limits(limits: &TokenLimits) -> bool {
    limits.hourly_limit.is_none() && limits.weekly_limit.is_none() && limits.total_limit.is_none()
}

fn is_default_usage(usage: &TokenUsage) -> bool {
    usage.hourly_tokens == 0
        && usage.weekly_tokens == 0
        && usage.total_tokens == 0
        && usage.hourly_reset_at == 0
        && usage.weekly_reset_at == 0
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct KeysFile {
    keys: Vec<ClientKey>,
}

pub struct ClientKeysStore {
    path: PathBuf,
    keys: RwLock<Vec<ClientKey>>,
}

fn timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

impl ClientKeysStore {
    pub async fn new(path: PathBuf) -> Self {
        let keys = if path.exists() {
            match fs::read_to_string(&path).await {
                Ok(content) => {
                    let file: KeysFile = serde_json::from_str(&content).unwrap_or_default();
                    file.keys
                }
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        };

        Self {
            path,
            keys: RwLock::new(keys),
        }
    }

    pub async fn list(&self) -> Vec<ClientKey> {
        self.keys.read().await.clone()
    }

    pub async fn create(&self, name: String) -> Result<ClientKey, std::io::Error> {
        // Generate random bytes before any await to avoid Send issues with ThreadRng
        let key_suffix = {
            let mut rng = rand::rng();
            let mut bytes = [0u8; 32];
            rng.fill(&mut bytes);
            URL_SAFE_NO_PAD.encode(bytes)
        };
        let key = format!("sk-proxy-{}", key_suffix);

        let client_key = ClientKey {
            id: Uuid::new_v4().to_string(),
            key,
            name,
            created_at: timestamp_millis(),
            last_used_at: None,
            enabled: true,
            limits: TokenLimits::default(),
            usage: TokenUsage::default(),
        };

        {
            let mut guard = self.keys.write().await;
            guard.push(client_key.clone());
        }

        self.save().await?;
        Ok(client_key)
    }

    pub async fn delete(&self, id: &str) -> Result<bool, std::io::Error> {
        let deleted = {
            let mut guard = self.keys.write().await;
            let len_before = guard.len();
            guard.retain(|k| k.id != id);
            guard.len() < len_before
        };

        if deleted {
            self.save().await?;
        }
        Ok(deleted)
    }

    /// Validate an API key using constant-time comparison to prevent timing attacks
    pub async fn validate(&self, key: &str) -> Option<ClientKey> {
        let guard = self.keys.read().await;
        guard
            .iter()
            .find(|k| k.enabled && k.key.as_bytes().ct_eq(key.as_bytes()).into())
            .cloned()
    }

    pub async fn update_last_used(&self, id: &str) -> Result<(), std::io::Error> {
        {
            let mut guard = self.keys.write().await;
            if let Some(key) = guard.iter_mut().find(|k| k.id == id) {
                key.last_used_at = Some(timestamp_millis());
            }
        }

        self.save().await
    }

    /// Get a key by ID
    pub async fn get(&self, id: &str) -> Option<ClientKey> {
        let guard = self.keys.read().await;
        guard.iter().find(|k| k.id == id).cloned()
    }

    /// Check if a key's usage is within limits.
    /// Automatically resets hourly/weekly counters if time has passed.
    /// Returns Ok(()) if within limits, Err with message if exceeded.
    pub async fn check_limits(&self, id: &str) -> Result<(), String> {
        let now = timestamp_millis();

        let mut guard = self.keys.write().await;
        let key = guard
            .iter_mut()
            .find(|k| k.id == id)
            .ok_or_else(|| "Key not found".to_string())?;

        // Auto-reset hourly counter if hour has passed
        if key.usage.hourly_reset_at > 0 && now >= key.usage.hourly_reset_at {
            key.usage.hourly_tokens = 0;
            key.usage.hourly_reset_at = 0;
        }

        // Auto-reset weekly counter if week has passed
        if key.usage.weekly_reset_at > 0 && now >= key.usage.weekly_reset_at {
            key.usage.weekly_tokens = 0;
            key.usage.weekly_reset_at = 0;
        }

        // Check hourly limit
        if let Some(limit) = key.limits.hourly_limit
            && key.usage.hourly_tokens >= limit
        {
            return Err(format!(
                "Hourly token limit exceeded ({}/{})",
                key.usage.hourly_tokens, limit
            ));
        }

        // Check weekly limit
        if let Some(limit) = key.limits.weekly_limit
            && key.usage.weekly_tokens >= limit
        {
            return Err(format!(
                "Weekly token limit exceeded ({}/{})",
                key.usage.weekly_tokens, limit
            ));
        }

        // Check total limit
        if let Some(limit) = key.limits.total_limit
            && key.usage.total_tokens >= limit
        {
            return Err(format!(
                "Total token limit exceeded ({}/{})",
                key.usage.total_tokens, limit
            ));
        }

        Ok(())
    }

    /// Record token usage for a key.
    /// Initializes reset timestamps if not set.
    pub async fn record_usage(&self, id: &str, tokens: u64) -> Result<(), std::io::Error> {
        let now = timestamp_millis();
        let one_hour_ms = 60 * 60 * 1000;
        let one_week_ms = 7 * 24 * 60 * 60 * 1000;

        {
            let mut guard = self.keys.write().await;
            if let Some(key) = guard.iter_mut().find(|k| k.id == id) {
                // Auto-reset and initialize hourly counter
                if key.usage.hourly_reset_at == 0 || now >= key.usage.hourly_reset_at {
                    key.usage.hourly_tokens = 0;
                    key.usage.hourly_reset_at = now + one_hour_ms;
                }

                // Auto-reset and initialize weekly counter
                if key.usage.weekly_reset_at == 0 || now >= key.usage.weekly_reset_at {
                    key.usage.weekly_tokens = 0;
                    key.usage.weekly_reset_at = now + one_week_ms;
                }

                // Add tokens to all counters
                key.usage.hourly_tokens += tokens;
                key.usage.weekly_tokens += tokens;
                key.usage.total_tokens += tokens;
            }
        }

        self.save().await
    }

    /// Get usage statistics for a key
    pub async fn get_usage(&self, id: &str) -> Option<(TokenLimits, TokenUsage)> {
        let now = timestamp_millis();
        let guard = self.keys.read().await;

        guard.iter().find(|k| k.id == id).map(|key| {
            let mut usage = key.usage.clone();

            // Show 0 if counters have expired (but don't modify stored data)
            if usage.hourly_reset_at > 0 && now >= usage.hourly_reset_at {
                usage.hourly_tokens = 0;
                usage.hourly_reset_at = 0;
            }
            if usage.weekly_reset_at > 0 && now >= usage.weekly_reset_at {
                usage.weekly_tokens = 0;
                usage.weekly_reset_at = 0;
            }

            (key.limits.clone(), usage)
        })
    }

    /// Reset usage counters for a key
    pub async fn reset_usage(
        &self,
        id: &str,
        reset_type: UsageResetType,
    ) -> Result<bool, std::io::Error> {
        let modified = {
            let mut guard = self.keys.write().await;
            if let Some(key) = guard.iter_mut().find(|k| k.id == id) {
                match reset_type {
                    UsageResetType::Hourly => {
                        key.usage.hourly_tokens = 0;
                        key.usage.hourly_reset_at = 0;
                    }
                    UsageResetType::Weekly => {
                        key.usage.weekly_tokens = 0;
                        key.usage.weekly_reset_at = 0;
                    }
                    UsageResetType::Total => {
                        key.usage.total_tokens = 0;
                    }
                    UsageResetType::All => {
                        key.usage = TokenUsage::default();
                    }
                }
                true
            } else {
                false
            }
        };

        if modified {
            self.save().await?;
        }
        Ok(modified)
    }

    /// Update limits for a key
    pub async fn set_limits(&self, id: &str, limits: TokenLimits) -> Result<bool, std::io::Error> {
        let modified = {
            let mut guard = self.keys.write().await;
            if let Some(key) = guard.iter_mut().find(|k| k.id == id) {
                key.limits = limits;
                true
            } else {
                false
            }
        };

        if modified {
            self.save().await?;
        }
        Ok(modified)
    }

    async fn save(&self) -> Result<(), std::io::Error> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).await?;
        }

        let guard = self.keys.read().await;
        let file = KeysFile {
            keys: guard.clone(),
        };
        let content = serde_json::to_string_pretty(&file)?;

        // Write to a temp file first, then rename for atomicity
        let temp_path = self.path.with_extension("tmp");

        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temp_path)
            .await?;

        f.write_all(content.as_bytes()).await?;
        f.sync_all().await?;

        fs::rename(&temp_path, &self.path).await?;
        Ok(())
    }
}

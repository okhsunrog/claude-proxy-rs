use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;

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

pub struct AuthStore {
    path: PathBuf,
    auth: RwLock<HashMap<String, Auth>>,
}

impl AuthStore {
    pub async fn new(path: PathBuf) -> Self {
        let auth = if path.exists() {
            match fs::read_to_string(&path).await {
                Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
                Err(_) => HashMap::new(),
            }
        } else {
            HashMap::new()
        };

        Self {
            path,
            auth: RwLock::new(auth),
        }
    }

    pub async fn get(&self, provider: &str) -> Option<Auth> {
        self.auth.read().await.get(provider).cloned()
    }

    pub async fn set(&self, provider: &str, auth: Auth) -> Result<(), std::io::Error> {
        {
            let mut guard = self.auth.write().await;
            guard.insert(provider.to_string(), auth);
        }
        self.save().await
    }

    pub async fn remove(&self, provider: &str) -> Result<(), std::io::Error> {
        {
            let mut guard = self.auth.write().await;
            guard.remove(provider);
        }
        self.save().await
    }

    pub async fn has(&self, provider: &str) -> bool {
        self.auth.read().await.contains_key(provider)
    }

    async fn save(&self) -> Result<(), std::io::Error> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).await?;
        }

        let guard = self.auth.read().await;
        let content = serde_json::to_string_pretty(&*guard)?;

        // Write to a temp file first, then rename for atomicity
        let temp_path = self.path.with_extension("tmp");

        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temp_path)
            .await?;

        file.write_all(content.as_bytes()).await?;
        file.sync_all().await?;

        fs::rename(&temp_path, &self.path).await?;
        Ok(())
    }

    pub async fn update_tokens(
        &self,
        provider: &str,
        access: String,
        refresh: String,
        expires: u64,
    ) -> Result<(), std::io::Error> {
        let mut guard = self.auth.write().await;
        if let Some(Auth::OAuth {
            account_id,
            enterprise_url,
            ..
        }) = guard.get(provider)
        {
            let account_id = account_id.clone();
            let enterprise_url = enterprise_url.clone();
            guard.insert(
                provider.to_string(),
                Auth::OAuth {
                    access,
                    refresh,
                    expires,
                    account_id,
                    enterprise_url,
                },
            );
        }
        drop(guard);
        self.save().await
    }
}

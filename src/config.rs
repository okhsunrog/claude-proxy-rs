use std::env;
use std::path::PathBuf;

use dotenvy::dotenv;

/// CORS configuration mode
#[derive(Debug, Clone)]
pub enum CorsMode {
    /// Only allow localhost origins (default, for local development)
    LocalhostOnly,
    /// Allow all origins (for public API deployment with API key auth)
    AllowAll,
    /// Allow specific origins (comma-separated list)
    AllowList(Vec<String>),
}

pub struct Config {
    pub host: String,
    pub port: u16,
    pub data_dir: PathBuf,
    pub admin_username: String,
    pub admin_password: String,
    pub cors_mode: CorsMode,
}

impl Config {
    pub fn from_env() -> Self {
        dotenv().ok();

        let host = env::var("CLAUDE_PROXY_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let port = env::var("CLAUDE_PROXY_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(4096);

        let data_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("claude-proxy");

        let admin_username = env::var("CLAUDE_PROXY_ADMIN_USERNAME")
            .expect("CLAUDE_PROXY_ADMIN_USERNAME must be set");
        let admin_password = env::var("CLAUDE_PROXY_ADMIN_PASSWORD")
            .expect("CLAUDE_PROXY_ADMIN_PASSWORD must be set");

        // CORS configuration: "localhost" (default), "*" (allow all), or comma-separated origins
        let cors_mode = match env::var("CLAUDE_PROXY_CORS_ORIGINS").as_deref() {
            Ok("*") => CorsMode::AllowAll,
            Ok(origins) if !origins.is_empty() => {
                CorsMode::AllowList(origins.split(',').map(|s| s.trim().to_string()).collect())
            }
            _ => CorsMode::LocalhostOnly,
        };

        Self {
            host,
            port,
            data_dir,
            admin_username,
            admin_password,
            cors_mode,
        }
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("proxy.db")
    }
}

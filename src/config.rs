use std::env;
use std::path::PathBuf;

use dotenvy::dotenv;

/// Cloaking mode — controls when Claude Code identity spoofing is applied
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloakMode {
    /// Always apply cloaking (fake user ID, system prefix)
    Always,
    /// Never apply cloaking
    Never,
    /// Auto-detect: skip cloaking when client is already Claude Code (User-Agent: claude-cli/*)
    Auto,
}

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
    pub disable_auth: bool,
    pub cloak_mode: CloakMode,
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

        let disable_auth = env::var("CLAUDE_PROXY_DISABLE_AUTH")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        let admin_username = if disable_auth {
            env::var("CLAUDE_PROXY_ADMIN_USERNAME").unwrap_or_default()
        } else {
            env::var("CLAUDE_PROXY_ADMIN_USERNAME")
                .expect("CLAUDE_PROXY_ADMIN_USERNAME must be set")
        };
        let admin_password = if disable_auth {
            env::var("CLAUDE_PROXY_ADMIN_PASSWORD").unwrap_or_default()
        } else {
            env::var("CLAUDE_PROXY_ADMIN_PASSWORD")
                .expect("CLAUDE_PROXY_ADMIN_PASSWORD must be set")
        };

        let cloak_mode = match env::var("CLAUDE_PROXY_CLOAK_MODE")
            .as_deref()
            .map(str::to_lowercase)
            .as_deref()
        {
            Ok("always") => CloakMode::Always,
            Ok("never") => CloakMode::Never,
            _ => CloakMode::Auto,
        };

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
            disable_auth,
            cloak_mode,
        }
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("proxy.db")
    }
}

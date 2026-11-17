use dotenvy::dotenv;
use std::env;

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
    pub database_url: String,
    pub admin_username: String,
    pub admin_password: String,
    pub cors_mode: CorsMode,
    pub disable_auth: bool,
    pub cloak_mode: CloakMode,
    pub compatibility: crate::compatibility::Compatibility,
    pub stt_requests_per_minute: u32,
    pub stt_max_concurrent: usize,
}

impl Config {
    pub fn from_env() -> Self {
        drop(dotenv());

        let host = env::var("CLAUDE_PROXY_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let port = env::var("CLAUDE_PROXY_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(4096);

        let database_url = env::var("CLAUDE_PROXY_DATABASE_URL")
            .or_else(|_| env::var("DATABASE_URL"))
            .expect("CLAUDE_PROXY_DATABASE_URL or DATABASE_URL must be set");

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
            database_url,
            admin_username,
            admin_password,
            cors_mode,
            disable_auth,
            cloak_mode,
            compatibility: crate::compatibility::Compatibility {
                profile: crate::compatibility::ClientProfile::parse(
                    &env::var("CLAUDE_PROXY_CLIENT_PROFILE").unwrap_or_else(|_| "auto".into()),
                )
                .expect("Invalid CLAUDE_PROXY_CLIENT_PROFILE"),
                cache: crate::compatibility::CachePolicy::parse(
                    &env::var("CLAUDE_PROXY_CACHE_POLICY").unwrap_or_else(|_| "auto-5m".into()),
                )
                .expect("Invalid CLAUDE_PROXY_CACHE_POLICY"),
            },
            // Zero explicitly disables only the local per-key rate limit.
            stt_requests_per_minute: env_number("CLAUDE_PROXY_STT_REQUESTS_PER_MINUTE", 120, true),
            stt_max_concurrent: usize::from(env_number::<u16>(
                "CLAUDE_PROXY_STT_MAX_CONCURRENT",
                4,
                false,
            )),
        }
    }
}

fn env_number<T>(name: &str, default: T, allow_zero: bool) -> T
where
    T: std::str::FromStr + Default + PartialEq,
{
    match env::var(name) {
        Err(env::VarError::NotPresent) => default,
        value => value
            .ok()
            .and_then(|value| value.parse::<T>().ok())
            .filter(|value| allow_zero || *value != T::default())
            .expect("Invalid STT limit environment variable: use a nonnegative integer rate and positive u16 concurrency"),
    }
}

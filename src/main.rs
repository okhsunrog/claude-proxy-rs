mod admin_session;
mod auth;
mod capture;
mod config;
mod constants;
mod db;
mod error;
mod routes;
mod subscription;
mod transforms;
mod usage;

use admin_session::{AdminCredentials, admin_auth_middleware};
use anyhow::{Context, Result};
use auth::{AuthStore, ClientKeysStore, ModelsStore, OAuthManager};
use axum::ServiceExt;
use axum::{
    Router,
    extract::{DefaultBodyLimit, Request},
    http::{HeaderName, HeaderValue, Method, header},
    middleware,
    routing::{get, post},
    serve,
};
use capture::CaptureConfig;
use clap::Parser;
use config::{CloakMode, Config, CorsMode};
use reqwest::Client;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::normalize_path::NormalizePath;
use tracing::{info, warn};
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};
use url::Url;
use usage::UsageCache;
use utoipa::openapi::{InfoBuilder, OpenApi, OpenApiBuilder};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GIT_HASH: &str = env!("GIT_HASH");
pub const BUILD_TIME: &str = env!("BUILD_TIME");

use crate::routes::{admin, anthropic, health, openai, user_usage};

pub struct AppState {
    pub auth_store: Arc<AuthStore>,
    pub client_keys: Arc<ClientKeysStore>,
    pub models: Arc<ModelsStore>,
    pub oauth: OAuthManager,
    pub http_client: Client,
    pub admin_credentials: AdminCredentials,
    /// Whether to set Secure flag on cookies (true when not binding to localhost)
    pub secure_cookies: bool,
    /// When true, admin auth middleware is bypassed (for local development)
    pub disable_auth: bool,
    /// Cloaking mode (always / never / auto)
    pub cloak_mode: CloakMode,
    /// Single source of truth for Claude subscription usage. Owns cached
    /// snapshot, freshness timestamps, fetcher dispatch, and header-based
    /// patching. See `usage::UsageCache` for the freshness model.
    pub usage_cache: UsageCache,
    /// Stable session UUID sent as X-Claude-Code-Session-Id header on every inference request.
    /// Matches Claude Code's per-process session ID behavior.
    pub session_id: String,
    /// Optional request/response capture sink for debugging client compatibility.
    pub capture: CaptureConfig,
}

impl AppState {
    /// Determine whether to apply cloaking based on mode and client User-Agent.
    pub fn should_cloak(&self, user_agent: Option<&str>) -> bool {
        match self.cloak_mode {
            CloakMode::Always => true,
            CloakMode::Never => false,
            CloakMode::Auto => {
                // Skip cloaking if client is already Claude Code
                !user_agent.is_some_and(|ua| ua.starts_with("claude-cli"))
            }
        }
    }
}

#[derive(Parser)]
#[command(name = "claude-proxy")]
#[command(about = "OpenAI-compatible proxy for Claude API")]
struct Args {
    /// Host to bind to
    #[arg(short = 'H', long, env = "CLAUDE_PROXY_HOST")]
    host: Option<String>,

    /// Port to bind to
    #[arg(short, long, env = "CLAUDE_PROXY_PORT")]
    port: Option<u16>,

    /// Dump OpenAPI spec as JSON and exit (no config/DB needed)
    #[arg(long)]
    openapi: bool,
}

fn full_openapi_router() -> OpenApiRouter<Arc<AppState>> {
    admin_openapi_router().merge(user_usage::user_usage_router())
}

fn admin_openapi_router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::with_openapi(
        OpenApiBuilder::new()
            .info(
                InfoBuilder::new()
                    .title("Claude Proxy Admin API")
                    .description(Some("Admin API for Claude Proxy"))
                    .version(VERSION)
                    .build(),
            )
            .build(),
    )
    // OAuth
    .routes(routes!(admin::get_oauth_status))
    .routes(routes!(admin::start_oauth_flow))
    .routes(routes!(admin::exchange_oauth_code))
    .routes(routes!(admin::delete_oauth))
    .routes(routes!(admin::get_subscription_usage))
    .routes(routes!(
        admin::get_web_session_status,
        admin::save_web_session,
        admin::delete_web_session
    ))
    // Keys
    .routes(routes!(admin::create_key))
    .routes(routes!(admin::list_keys))
    .routes(routes!(admin::delete_key))
    .routes(routes!(admin::set_key_enabled))
    .routes(routes!(admin::set_allow_extra_usage))
    .routes(routes!(admin::get_key_usage))
    .routes(routes!(admin::update_key_limits))
    .routes(routes!(admin::reset_key_usage))
    // Models
    .routes(routes!(admin::list_models_admin))
    .routes(routes!(admin::add_model))
    .routes(routes!(admin::delete_model, admin::update_model))
    .routes(routes!(admin::reorder_models))
    // Per-key model access
    .routes(routes!(admin::get_key_models, admin::set_key_models))
    // Per-key per-model usage
    .routes(routes!(admin::get_key_model_usage))
    .routes(routes!(
        admin::set_key_model_limits,
        admin::remove_key_model_limits
    ))
    .routes(routes!(admin::reset_key_model_usage))
    // Usage history (charts)
    .routes(routes!(admin::get_usage_history_timeseries))
    .routes(routes!(admin::get_usage_history_by_model))
    .routes(routes!(admin::get_usage_history_by_key))
    .routes(routes!(admin::delete_usage_history))
}

fn build_openapi() -> OpenApi {
    let (_, openapi) = full_openapi_router().split_for_parts();
    openapi
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Dump OpenAPI spec and exit (no config/DB needed)
    if args.openapi {
        let openapi = build_openapi();
        println!(
            "{}",
            openapi
                .to_pretty_json()
                .context("Failed to serialize OpenAPI spec")?
        );
        return Ok(());
    }

    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(fmt::layer())
        .init();
    let config = Config::from_env();

    // Initialize database (before moving fields out of config)
    db::init_db(&config.database_url)
        .await
        .context("Failed to initialize database")?;

    let host = args.host.unwrap_or(config.host);
    let port = args.port.unwrap_or(config.port);

    let auth_store = Arc::new(AuthStore::new());
    let client_keys = Arc::new(ClientKeysStore::new());
    let models = Arc::new(ModelsStore::new());

    // Shared HTTP client with connection pooling
    let http_client = Client::builder()
        .timeout(Duration::from_secs(300)) // 5 min timeout for long requests
        .pool_max_idle_per_host(10)
        .build()
        .context("Failed to create HTTP client")?;

    let oauth = OAuthManager::new(http_client.clone(), auth_store.clone());

    let admin_credentials = AdminCredentials {
        username: config.admin_username,
        password: config.admin_password,
    };

    let is_localhost = matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1");
    let secure_cookies = !is_localhost;

    let disable_auth = config.disable_auth;
    if disable_auth {
        warn!("Admin authentication is DISABLED (CLAUDE_PROXY_DISABLE_AUTH=1)");
    }

    let cloak_mode = config.cloak_mode;
    info!("Cloaking mode: {:?}", cloak_mode);
    let capture = CaptureConfig::from_env();
    if capture.is_enabled() {
        info!("Request capture is enabled");
    }

    let state = Arc::new(AppState {
        auth_store,
        client_keys,
        models,
        oauth,
        http_client,
        admin_credentials,
        secure_cookies,
        disable_auth,
        cloak_mode,
        usage_cache: UsageCache::new(),
        session_id: Uuid::new_v4().to_string(),
        capture,
    });

    // CORS configuration based on environment
    let cors_origins = config.cors_mode.clone();
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(move |origin: &HeaderValue, _| {
            let Ok(origin_str) = origin.to_str() else {
                return false;
            };

            match &cors_origins {
                CorsMode::AllowAll => true,
                CorsMode::LocalhostOnly => {
                    let Ok(url) = Url::parse(origin_str) else {
                        return false;
                    };
                    matches!(
                        url.host_str(),
                        Some("localhost") | Some("127.0.0.1") | Some("::1")
                    )
                }
                CorsMode::AllowList(allowed) => allowed.iter().any(|a| a == origin_str),
            }
        }))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::CONTENT_TYPE,
            header::AUTHORIZATION,
            HeaderName::from_static("x-api-key"),
            HeaderName::from_static("anthropic-version"),
        ])
        .allow_credentials(true);

    match &config.cors_mode {
        CorsMode::AllowAll => info!("CORS: Allowing all origins"),
        CorsMode::LocalhostOnly => info!("CORS: Localhost only"),
        CorsMode::AllowList(list) => info!("CORS: Allowing origins: {:?}", list),
    }

    // Admin API routes (protected)
    let (api_router, _) = admin_openapi_router().split_for_parts();

    // User-facing usage routes (unprotected — Bearer key auth handled in handlers)
    let (user_router, _) = user_usage::user_usage_router().split_for_parts();

    // Auth endpoints (accessible without authentication)
    let auth_routes = Router::new()
        .route("/auth/login", post(admin::login))
        .route("/auth/logout", post(admin::logout))
        .route("/auth/check", get(admin::auth_check))
        .with_state(state.clone());

    // Protected admin routes (session cookie or Basic Auth)
    let protected_routes = api_router.layer(middleware::from_fn_with_state(
        state.clone(),
        admin_auth_middleware,
    ));

    // Combine: auth routes (unprotected) + user usage (unprotected) + protected API + static SPA
    let admin_routes = Router::new()
        .merge(auth_routes)
        .merge(user_router)
        .merge(protected_routes)
        .merge(admin::static_routes());

    // API routes
    let api_routes = Router::new()
        .route("/chat/completions", post(openai::chat_completions))
        .route("/models", get(openai::list_models))
        .route("/messages", post(anthropic::messages))
        .route("/messages/count_tokens", post(anthropic::count_tokens));

    let app = NormalizePath::trim_trailing_slash(
        Router::new()
            .route("/health", get(health::health))
            .route("/version", get(health::version))
            .nest("/admin", admin_routes)
            .nest("/v1", api_routes)
            .layer(cors)
            .layer(DefaultBodyLimit::max(100 * 1024 * 1024)) // 100 MB
            .with_state(state),
    );

    let bind_addr = format!("{}:{}", host, port);
    let addr: SocketAddr = bind_addr
        .parse()
        .with_context(|| format!("Invalid bind address: {bind_addr}"))?;
    info!(
        "Starting claude-proxy v{}-{} (built {})",
        VERSION, GIT_HASH, BUILD_TIME
    );
    info!("Listening on http://{}", addr);
    info!("Admin UI: http://{}/admin", addr);

    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("Failed to bind {addr}"))?;
    serve(listener, ServiceExt::<Request>::into_make_service(app))
        .await
        .context("HTTP server failed")?;

    Ok(())
}

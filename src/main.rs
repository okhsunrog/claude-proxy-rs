mod auth;
mod config;
mod constants;
mod db;
mod error;
mod routes;
mod transforms;

use auth::{AuthStore, ClientKeysStore, ModelsStore, OAuthManager};
use axum::ServiceExt;
use axum::{
    Router,
    extract::State,
    http::{HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::Engine;
use clap::Parser;
use config::{Config, CorsMode};
use reqwest::Client;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use tokio::sync::RwLock;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::normalize_path::NormalizePath;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use utoipa_axum::{router::OpenApiRouter, routes};

/// Session TTL: 24 hours (matches cookie Max-Age)
const SESSION_TTL_SECS: u64 = 86400;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GIT_HASH: &str = env!("GIT_HASH");
pub const BUILD_TIME: &str = env!("BUILD_TIME");

/// Cached subscription window reset times (epoch ms).
/// Used to sync per-key rate-limit windows with Claude's actual subscription windows.
#[derive(Debug, Clone, Default)]
pub struct WindowResets {
    pub five_hour_reset_at: Option<u64>,
    pub seven_day_reset_at: Option<u64>,
}

pub struct AppState {
    pub auth_store: Arc<AuthStore>,
    pub client_keys: Arc<ClientKeysStore>,
    pub models: Arc<ModelsStore>,
    pub oauth: OAuthManager,
    pub http_client: Client,
    pub admin_credentials: (String, String),
    /// Whether to set Secure flag on cookies (true when not binding to localhost)
    pub secure_cookies: bool,
    /// When true, admin auth middleware is bypassed (for local development)
    pub disable_auth: bool,
    /// Cached subscription window reset times for syncing rate-limit windows
    pub window_resets: RwLock<WindowResets>,
}

/// Save a session token to the database
pub async fn save_session(token: &str, expires_at: u64) {
    if let Ok(conn) = db::get_conn().await
        && let Err(e) = conn
            .execute(
                "INSERT OR REPLACE INTO admin_sessions (token, expires_at) VALUES (?, ?)",
                (token, expires_at as i64),
            )
            .await
    {
        tracing::warn!("Failed to save session: {e}");
    }
}

/// Validate a session token, returns true if valid and not expired
pub async fn validate_session(token: &str) -> bool {
    let Ok(conn) = db::get_conn().await else {
        return false;
    };
    let Ok(mut rows) = conn
        .query(
            "SELECT expires_at FROM admin_sessions WHERE token = ?",
            [token],
        )
        .await
    else {
        return false;
    };
    let Some(row) = rows.next().await.ok().flatten() else {
        return false;
    };
    let Ok(expires_at) = row.get::<i64>(0) else {
        return false;
    };
    let now = now_secs() as i64;
    if now < expires_at {
        return true;
    }
    // Expired — clean it up
    let _ = conn
        .execute("DELETE FROM admin_sessions WHERE token = ?", [token])
        .await;
    false
}

/// Remove a session token from the database
pub async fn remove_session(token: &str) {
    if let Ok(conn) = db::get_conn().await {
        let _ = conn
            .execute("DELETE FROM admin_sessions WHERE token = ?", [token])
            .await;
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
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
}

/// Parse a named cookie from the Cookie header
pub fn parse_cookie(header: &str, name: &str) -> Option<String> {
    header.split(';').find_map(|cookie| {
        let (key, value) = cookie.trim().split_once('=')?;
        if key.trim() == name {
            Some(value.trim().to_string())
        } else {
            None
        }
    })
}

/// Middleware for admin routes authentication (session cookie or Basic Auth)
async fn admin_auth_middleware(
    State(state): State<Arc<AppState>>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    if state.disable_auth {
        return next.run(request).await;
    }

    let (username, password) = &state.admin_credentials;

    // Check for session cookie first
    if let Some(cookie_header) = request
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        && let Some(token) = parse_cookie(cookie_header, "admin_session")
        && validate_session(&token).await
    {
        return next.run(request).await;
    }

    // Fall through to Basic Auth check
    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let Some(auth_value) = auth_header else {
        return unauthorized_response();
    };

    let Some(encoded) = auth_value.strip_prefix("Basic ") else {
        return unauthorized_response();
    };

    let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
        return unauthorized_response();
    };

    let Ok(credentials) = String::from_utf8(decoded) else {
        return unauthorized_response();
    };

    let Some((provided_user, provided_pass)) = credentials.split_once(':') else {
        return unauthorized_response();
    };

    // Constant-time comparison to prevent timing attacks
    let user_match = provided_user.as_bytes().ct_eq(username.as_bytes());
    let pass_match = provided_pass.as_bytes().ct_eq(password.as_bytes());

    if user_match.into() && pass_match.into() {
        next.run(request).await
    } else {
        unauthorized_response()
    }
}

fn unauthorized_response() -> Response {
    (StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let args = Args::parse();
    let config = Config::from_env();

    // Initialize database (before moving fields out of config)
    db::init_db(&config.db_path())
        .await
        .expect("Failed to initialize database");

    let host = args.host.unwrap_or(config.host);
    let port = args.port.unwrap_or(config.port);

    let auth_store = Arc::new(AuthStore::new());
    let client_keys = Arc::new(ClientKeysStore::new());
    let models = Arc::new(ModelsStore::new());
    let oauth = OAuthManager::new(auth_store.clone());

    // Shared HTTP client with connection pooling
    let http_client = Client::builder()
        .timeout(Duration::from_secs(300)) // 5 min timeout for long requests
        .pool_max_idle_per_host(10)
        .build()
        .expect("Failed to create HTTP client");

    let admin_credentials = (config.admin_username, config.admin_password);

    let is_localhost = matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1");
    let secure_cookies = !is_localhost;

    let disable_auth = config.disable_auth;
    if disable_auth {
        tracing::warn!("Admin authentication is DISABLED (CLAUDE_PROXY_DISABLE_AUTH=1)");
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
        window_resets: RwLock::new(WindowResets::default()),
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
                    let Ok(url) = url::Url::parse(origin_str) else {
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
            header::HeaderName::from_static("x-api-key"),
            header::HeaderName::from_static("anthropic-version"),
        ])
        .allow_credentials(true);

    match &config.cors_mode {
        CorsMode::AllowAll => info!("CORS: Allowing all origins"),
        CorsMode::LocalhostOnly => info!("CORS: Localhost only"),
        CorsMode::AllowList(list) => info!("CORS: Allowing origins: {:?}", list),
    }

    // Admin API routes with OpenAPI spec generation
    let (api_router, openapi) = OpenApiRouter::with_openapi(Default::default())
        // OAuth
        .routes(routes!(routes::admin::get_oauth_status))
        .routes(routes!(routes::admin::start_oauth_flow))
        .routes(routes!(routes::admin::exchange_oauth_code))
        .routes(routes!(routes::admin::delete_oauth))
        .routes(routes!(routes::admin::get_subscription_usage))
        // Keys
        .routes(routes!(routes::admin::create_key))
        .routes(routes!(routes::admin::list_keys))
        .routes(routes!(routes::admin::delete_key))
        .routes(routes!(routes::admin::get_key_usage))
        .routes(routes!(routes::admin::update_key_limits))
        .routes(routes!(routes::admin::reset_key_usage))
        // Models
        .routes(routes!(routes::admin::list_models_admin))
        .routes(routes!(routes::admin::add_model))
        .routes(routes!(
            routes::admin::delete_model,
            routes::admin::update_model
        ))
        .routes(routes!(routes::admin::reorder_models))
        // Per-key model access
        .routes(routes!(
            routes::admin::get_key_models,
            routes::admin::set_key_models
        ))
        // Per-key per-model usage
        .routes(routes!(routes::admin::get_key_model_usage))
        .routes(routes!(
            routes::admin::set_key_model_limits,
            routes::admin::remove_key_model_limits
        ))
        .routes(routes!(routes::admin::reset_key_model_usage))
        .split_for_parts();

    // Swagger UI + OpenAPI spec (accessible without authentication)
    let swagger_routes = Router::new().merge(
        utoipa_swagger_ui::SwaggerUi::new("/swagger").url("/api-docs/openapi.json", openapi),
    );

    // Auth endpoints (accessible without authentication)
    let auth_routes = Router::new()
        .route("/auth/login", post(routes::admin::login))
        .route("/auth/logout", post(routes::admin::logout))
        .route("/auth/check", get(routes::admin::auth_check))
        .with_state(state.clone());

    // Protected admin routes (session cookie or Basic Auth)
    let protected_routes = api_router.layer(middleware::from_fn_with_state(
        state.clone(),
        admin_auth_middleware,
    ));

    // Combine: swagger (unprotected) + auth routes (unprotected) + protected API + static SPA
    let admin_routes = Router::new()
        .merge(swagger_routes)
        .merge(auth_routes)
        .merge(protected_routes)
        .merge(routes::admin::static_routes());

    // API routes
    let api_routes = Router::new()
        .route("/chat/completions", post(routes::openai::chat_completions))
        .route("/models", get(routes::openai::list_models))
        .route("/messages", post(routes::anthropic::messages))
        .route(
            "/messages/count_tokens",
            post(routes::anthropic::count_tokens),
        );

    let app = NormalizePath::trim_trailing_slash(
        Router::new()
            .route("/health", get(routes::health::health))
            .route("/version", get(routes::health::version))
            .nest("/admin", admin_routes)
            .nest("/v1", api_routes)
            .layer(cors)
            .with_state(state),
    );

    let addr: SocketAddr = format!("{}:{}", host, port)
        .parse()
        .expect("Invalid address");
    info!(
        "Starting claude-proxy v{}-{} (built {})",
        VERSION, GIT_HASH, BUILD_TIME
    );
    info!("Listening on http://{}", addr);
    info!("Admin UI: http://{}/admin", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(
        listener,
        ServiceExt::<axum::extract::Request>::into_make_service(app),
    )
    .await
    .unwrap();
}

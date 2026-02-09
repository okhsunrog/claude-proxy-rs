use std::path::Path;
use std::sync::Arc;

use tokio::sync::OnceCell;
use tracing::info;
use turso::{Builder, Connection, Database};

use crate::error::ProxyError;

/// Global database instance
static DATABASE: OnceCell<Arc<Database>> = OnceCell::const_new();

/// Initialize the database and create all tables
pub async fn init_db(path: &Path) -> Result<(), ProxyError> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            ProxyError::DatabaseError(format!("Failed to create DB directory: {e}"))
        })?;
    }

    let path_str = path.to_str().unwrap_or("proxy.db");
    let db = Builder::new_local(path_str)
        .build()
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to open database: {e}")))?;

    let conn = db
        .connect()
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to connect: {e}")))?;

    // Create auth table
    conn.execute(
        r#"
        CREATE TABLE IF NOT EXISTS auth (
            provider TEXT PRIMARY KEY,
            auth_type TEXT NOT NULL,
            access_token TEXT NOT NULL,
            refresh_token TEXT NOT NULL,
            expires_at INTEGER NOT NULL,
            account_id TEXT,
            enterprise_url TEXT
        )
        "#,
        (),
    )
    .await
    .map_err(|e| ProxyError::DatabaseError(format!("Failed to create auth table: {e}")))?;

    // Create client_keys table
    conn.execute(
        r#"
        CREATE TABLE IF NOT EXISTS client_keys (
            id TEXT PRIMARY KEY,
            key TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            created_at INTEGER NOT NULL,
            last_used_at INTEGER,
            hourly_limit INTEGER,
            weekly_limit INTEGER,
            total_limit INTEGER,
            hourly_usage INTEGER NOT NULL DEFAULT 0,
            weekly_usage INTEGER NOT NULL DEFAULT 0,
            total_usage INTEGER NOT NULL DEFAULT 0,
            hourly_reset_at INTEGER NOT NULL DEFAULT 0,
            weekly_reset_at INTEGER NOT NULL DEFAULT 0
        )
        "#,
        (),
    )
    .await
    .map_err(|e| ProxyError::DatabaseError(format!("Failed to create client_keys table: {e}")))?;

    // Future: per-key allowed model list
    conn.execute(
        r#"
        CREATE TABLE IF NOT EXISTS key_allowed_models (
            key_id TEXT NOT NULL REFERENCES client_keys(id) ON DELETE CASCADE,
            model TEXT NOT NULL,
            PRIMARY KEY (key_id, model)
        )
        "#,
        (),
    )
    .await
    .map_err(|e| {
        ProxyError::DatabaseError(format!("Failed to create key_allowed_models table: {e}"))
    })?;

    // Future: per-key per-model usage tracking
    conn.execute(
        r#"
        CREATE TABLE IF NOT EXISTS key_model_usage (
            key_id TEXT NOT NULL REFERENCES client_keys(id) ON DELETE CASCADE,
            model TEXT NOT NULL,
            hourly_limit INTEGER,
            weekly_limit INTEGER,
            total_limit INTEGER,
            hourly_usage INTEGER NOT NULL DEFAULT 0,
            weekly_usage INTEGER NOT NULL DEFAULT 0,
            total_usage INTEGER NOT NULL DEFAULT 0,
            hourly_reset_at INTEGER NOT NULL DEFAULT 0,
            weekly_reset_at INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (key_id, model)
        )
        "#,
        (),
    )
    .await
    .map_err(|e| {
        ProxyError::DatabaseError(format!("Failed to create key_model_usage table: {e}"))
    })?;

    DATABASE
        .set(Arc::new(db))
        .map_err(|_| ProxyError::DatabaseError("Database already initialized".into()))?;

    info!("Database initialized at {}", path_str);
    Ok(())
}

/// Get a database connection
pub fn get_conn() -> Result<Connection, ProxyError> {
    let db = DATABASE
        .get()
        .ok_or_else(|| ProxyError::DatabaseError("Database not initialized".into()))?;
    db.connect()
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to get connection: {e}")))
}

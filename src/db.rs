use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::OnceCell;
use tracing::info;
use turso::{Builder, Connection, Database};

use crate::constants::SEED_MODELS;
use crate::error::ProxyError;

/// Global database instance
static DATABASE: OnceCell<Arc<Database>> = OnceCell::const_new();

// ---------------------------------------------------------------------------
// Migration framework
// ---------------------------------------------------------------------------

type MigrationFn =
    fn(&Connection) -> Pin<Box<dyn Future<Output = Result<(), ProxyError>> + Send + '_>>;

struct Migration {
    version: i64,
    description: &'static str,
    migrate: MigrationFn,
}

/// Ordered list of all migrations. Each migration assumes all prior migrations
/// have already been applied. New migrations are appended at the end.
static MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        description: "initial schema: auth, client_keys",
        migrate: migrate_v1,
    },
    Migration {
        version: 2,
        description: "add models, key_allowed_models, key_model_usage tables",
        migrate: migrate_v2,
    },
];

/// Read the current schema version (0 if table is empty or doesn't exist yet).
async fn get_schema_version(conn: &Connection) -> Result<i64, ProxyError> {
    let mut rows = conn
        .query("SELECT version FROM schema_version LIMIT 1", ())
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to read schema version: {e}")))?;
    let version = rows
        .next()
        .await
        .ok()
        .flatten()
        .and_then(|r| r.get::<i64>(0).ok())
        .unwrap_or(0);
    Ok(version)
}

/// Set the schema version (insert or update the single row).
async fn set_schema_version(conn: &Connection, version: i64) -> Result<(), ProxyError> {
    conn.execute("DELETE FROM schema_version", ())
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to clear schema version: {e}")))?;
    conn.execute(
        "INSERT INTO schema_version (version) VALUES (?)",
        (version,),
    )
    .await
    .map_err(|e| ProxyError::DatabaseError(format!("Failed to set schema version: {e}")))?;
    Ok(())
}

/// Detect whether a table exists in the database.
async fn table_exists(conn: &Connection, table_name: &str) -> Result<bool, ProxyError> {
    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
            (table_name,),
        )
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to check table existence: {e}")))?;
    let count: i64 = rows
        .next()
        .await
        .ok()
        .flatten()
        .and_then(|r| r.get::<i64>(0).ok())
        .unwrap_or(0);
    Ok(count > 0)
}

/// Run all pending migrations.
async fn run_migrations(conn: &Connection) -> Result<(), ProxyError> {
    // Ensure the schema_version table exists
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)",
        (),
    )
    .await
    .map_err(|e| {
        ProxyError::DatabaseError(format!("Failed to create schema_version table: {e}"))
    })?;

    let mut current = get_schema_version(conn).await?;

    // Handle pre-migration databases: if tables exist but no version is recorded,
    // set version to 1 (the original schema) so only newer migrations run.
    if current == 0 && table_exists(conn, "auth").await? {
        info!("Existing database detected without schema version — setting to v1");
        set_schema_version(conn, 1).await?;
        current = 1;
    }

    for migration in MIGRATIONS {
        if migration.version > current {
            info!(
                "Running migration v{}: {}",
                migration.version, migration.description
            );
            (migration.migrate)(conn).await?;
            set_schema_version(conn, migration.version).await?;
            current = migration.version;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Seed the models table with default models if it's empty.
async fn seed_models_if_empty(conn: &Connection) -> Result<(), ProxyError> {
    let mut count_rows = conn
        .query("SELECT COUNT(*) FROM models", ())
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to count models: {e}")))?;
    let model_count: i64 = count_rows
        .next()
        .await
        .ok()
        .flatten()
        .and_then(|r| r.get::<i64>(0).ok())
        .unwrap_or(0);

    if model_count == 0 {
        info!(
            "Seeding models table with {} default models",
            SEED_MODELS.len()
        );
        for (i, &(id, input_price, output_price, cache_read_price, cache_write_price)) in
            SEED_MODELS.iter().enumerate()
        {
            conn.execute(
                "INSERT INTO models (id, sort_order, enabled, input_price, output_price, cache_read_price, cache_write_price) VALUES (?, ?, 1, ?, ?, ?, ?)",
                (id, i as i64, input_price, output_price, cache_read_price, cache_write_price),
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to seed model {id}: {e}")))?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Migration v1 — original schema (auth + client_keys)
// ---------------------------------------------------------------------------

fn migrate_v1(
    conn: &Connection,
) -> Pin<Box<dyn Future<Output = Result<(), ProxyError>> + Send + '_>> {
    Box::pin(async move {
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
        .map_err(|e| {
            ProxyError::DatabaseError(format!("Failed to create client_keys table: {e}"))
        })?;

        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Migration v2 — models, per-key model access, per-model usage
// ---------------------------------------------------------------------------

fn migrate_v2(
    conn: &Connection,
) -> Pin<Box<dyn Future<Output = Result<(), ProxyError>> + Send + '_>> {
    Box::pin(async move {
        // Models table (dynamic model list with pricing)
        conn.execute(
            r#"
            CREATE TABLE IF NOT EXISTS models (
                id TEXT PRIMARY KEY,
                sort_order INTEGER NOT NULL DEFAULT 0,
                enabled INTEGER NOT NULL DEFAULT 1,
                input_price REAL NOT NULL DEFAULT 0,
                output_price REAL NOT NULL DEFAULT 0,
                cache_read_price REAL NOT NULL DEFAULT 0,
                cache_write_price REAL NOT NULL DEFAULT 0
            )
            "#,
            (),
        )
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to create models table: {e}")))?;

        seed_models_if_empty(conn).await?;

        // Per-key allowed model list
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

        // Drop old key_model_usage schema if it exists (had aggregate columns)
        conn.execute("DROP TABLE IF EXISTS key_model_usage", ())
            .await
            .map_err(|e| {
                ProxyError::DatabaseError(format!("Failed to drop old key_model_usage: {e}"))
            })?;

        // Per-key per-model usage tracking with 4 separate token type counters
        conn.execute(
            r#"
            CREATE TABLE IF NOT EXISTS key_model_usage (
                key_id TEXT NOT NULL REFERENCES client_keys(id) ON DELETE CASCADE,
                model TEXT NOT NULL,
                hourly_limit INTEGER,
                weekly_limit INTEGER,
                total_limit INTEGER,
                hourly_input INTEGER NOT NULL DEFAULT 0,
                hourly_output INTEGER NOT NULL DEFAULT 0,
                hourly_cache_read INTEGER NOT NULL DEFAULT 0,
                hourly_cache_write INTEGER NOT NULL DEFAULT 0,
                weekly_input INTEGER NOT NULL DEFAULT 0,
                weekly_output INTEGER NOT NULL DEFAULT 0,
                weekly_cache_read INTEGER NOT NULL DEFAULT 0,
                weekly_cache_write INTEGER NOT NULL DEFAULT 0,
                total_input INTEGER NOT NULL DEFAULT 0,
                total_output INTEGER NOT NULL DEFAULT 0,
                total_cache_read INTEGER NOT NULL DEFAULT 0,
                total_cache_write INTEGER NOT NULL DEFAULT 0,
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

        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the database and run all pending migrations.
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

    // Enable foreign key enforcement (required for ON DELETE CASCADE)
    conn.execute("PRAGMA foreign_keys = ON", ())
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to enable foreign keys: {e}")))?;

    run_migrations(&conn).await?;

    DATABASE
        .set(Arc::new(db))
        .map_err(|_| ProxyError::DatabaseError("Database already initialized".into()))?;

    info!("Database initialized at {}", path_str);
    Ok(())
}

/// Get a database connection with foreign keys enabled.
pub async fn get_conn() -> Result<Connection, ProxyError> {
    let db = DATABASE
        .get()
        .ok_or_else(|| ProxyError::DatabaseError("Database not initialized".into()))?;
    let conn = db
        .connect()
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to get connection: {e}")))?;
    conn.execute("PRAGMA foreign_keys = ON", ())
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to enable foreign keys: {e}")))?;
    Ok(conn)
}

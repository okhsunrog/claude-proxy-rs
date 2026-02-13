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
    Migration {
        version: 3,
        description: "add admin_sessions table for persistent sessions",
        migrate: migrate_v3,
    },
    Migration {
        version: 4,
        description: "add allow_extra_usage to client_keys",
        migrate: migrate_v4,
    },
    Migration {
        version: 5,
        description: "remove redundant global usage, centralize reset timestamps, rename hourly→five_hour",
        migrate: migrate_v5,
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
async fn run_migrations(conn: &Connection, db_path: &Path) -> Result<(), ProxyError> {
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

    // Back up the database before running any pending migrations
    let has_pending = MIGRATIONS.iter().any(|m| m.version > current);
    if has_pending && db_path.exists() {
        let backup_name = format!(
            "{}.backup-v{}",
            db_path.file_name().unwrap_or_default().to_string_lossy(),
            current
        );
        let backup_path = db_path.with_file_name(&backup_name);
        std::fs::copy(db_path, &backup_path).map_err(|e| {
            ProxyError::DatabaseError(format!("Failed to backup database before migration: {e}"))
        })?;
        // Also copy WAL file if present (contains recent uncommitted writes)
        let wal_path = db_path.with_extension("db-wal");
        if wal_path.exists() {
            let wal_backup = db_path.with_file_name(format!("{backup_name}-wal"));
            let _ = std::fs::copy(&wal_path, &wal_backup);
        }
        info!("Database backup created at {}", backup_path.display());
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
// Migration v3 — persistent admin sessions
// ---------------------------------------------------------------------------

fn migrate_v3(
    conn: &Connection,
) -> Pin<Box<dyn Future<Output = Result<(), ProxyError>> + Send + '_>> {
    Box::pin(async move {
        conn.execute(
            r#"
            CREATE TABLE IF NOT EXISTS admin_sessions (
                token TEXT PRIMARY KEY,
                expires_at INTEGER NOT NULL
            )
            "#,
            (),
        )
        .await
        .map_err(|e| {
            ProxyError::DatabaseError(format!("Failed to create admin_sessions table: {e}"))
        })?;

        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Migration v4 — per-key allow_extra_usage flag
// ---------------------------------------------------------------------------

fn migrate_v4(
    conn: &Connection,
) -> Pin<Box<dyn Future<Output = Result<(), ProxyError>> + Send + '_>> {
    Box::pin(async move {
        conn.execute(
            "ALTER TABLE client_keys ADD COLUMN allow_extra_usage INTEGER NOT NULL DEFAULT 0",
            (),
        )
        .await
        .map_err(|e| {
            ProxyError::DatabaseError(format!("Failed to add allow_extra_usage column: {e}"))
        })?;

        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Migration v5 — remove redundant global usage, centralize reset timestamps
// ---------------------------------------------------------------------------

fn migrate_v5(
    conn: &Connection,
) -> Pin<Box<dyn Future<Output = Result<(), ProxyError>> + Send + '_>> {
    Box::pin(async move {
        // IMPORTANT: Disable foreign keys during migration to prevent CASCADE
        // deletion when dropping parent tables. Without this, DROP TABLE client_keys
        // triggers an implicit DELETE FROM client_keys, which cascades to
        // key_model_usage and destroys all per-model usage data.
        conn.execute("PRAGMA foreign_keys = OFF", ())
            .await
            .map_err(|e| {
                ProxyError::DatabaseError(format!("v5: Failed to disable foreign keys: {e}"))
            })?;

        // 1. Recreate client_keys: drop usage columns, rename hourly→five_hour
        conn.execute(
            r#"
            CREATE TABLE client_keys_new (
                id TEXT PRIMARY KEY,
                key TEXT NOT NULL UNIQUE,
                name TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at INTEGER NOT NULL,
                last_used_at INTEGER,
                five_hour_limit INTEGER,
                weekly_limit INTEGER,
                total_limit INTEGER,
                five_hour_reset_at INTEGER NOT NULL DEFAULT 0,
                weekly_reset_at INTEGER NOT NULL DEFAULT 0,
                allow_extra_usage INTEGER NOT NULL DEFAULT 0
            )
            "#,
            (),
        )
        .await
        .map_err(|e| {
            ProxyError::DatabaseError(format!("v5: Failed to create client_keys_new: {e}"))
        })?;

        conn.execute(
            r#"
            INSERT INTO client_keys_new (id, key, name, enabled, created_at, last_used_at,
                five_hour_limit, weekly_limit, total_limit,
                five_hour_reset_at, weekly_reset_at, allow_extra_usage)
            SELECT id, key, name, enabled, created_at, last_used_at,
                hourly_limit, weekly_limit, total_limit,
                hourly_reset_at, weekly_reset_at, allow_extra_usage
            FROM client_keys
            "#,
            (),
        )
        .await
        .map_err(|e| {
            ProxyError::DatabaseError(format!("v5: Failed to copy client_keys data: {e}"))
        })?;

        conn.execute("DROP TABLE client_keys", ())
            .await
            .map_err(|e| {
                ProxyError::DatabaseError(format!("v5: Failed to drop old client_keys: {e}"))
            })?;

        conn.execute("ALTER TABLE client_keys_new RENAME TO client_keys", ())
            .await
            .map_err(|e| {
                ProxyError::DatabaseError(format!("v5: Failed to rename client_keys_new: {e}"))
            })?;

        // 2. Recreate key_model_usage: drop reset timestamps, rename hourly→five_hour
        conn.execute(
            r#"
            CREATE TABLE key_model_usage_new (
                key_id TEXT NOT NULL REFERENCES client_keys(id) ON DELETE CASCADE,
                model TEXT NOT NULL,
                five_hour_limit INTEGER,
                weekly_limit INTEGER,
                total_limit INTEGER,
                five_hour_input INTEGER NOT NULL DEFAULT 0,
                five_hour_output INTEGER NOT NULL DEFAULT 0,
                five_hour_cache_read INTEGER NOT NULL DEFAULT 0,
                five_hour_cache_write INTEGER NOT NULL DEFAULT 0,
                weekly_input INTEGER NOT NULL DEFAULT 0,
                weekly_output INTEGER NOT NULL DEFAULT 0,
                weekly_cache_read INTEGER NOT NULL DEFAULT 0,
                weekly_cache_write INTEGER NOT NULL DEFAULT 0,
                total_input INTEGER NOT NULL DEFAULT 0,
                total_output INTEGER NOT NULL DEFAULT 0,
                total_cache_read INTEGER NOT NULL DEFAULT 0,
                total_cache_write INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (key_id, model)
            )
            "#,
            (),
        )
        .await
        .map_err(|e| {
            ProxyError::DatabaseError(format!("v5: Failed to create key_model_usage_new: {e}"))
        })?;

        conn.execute(
            r#"
            INSERT INTO key_model_usage_new (key_id, model,
                five_hour_limit, weekly_limit, total_limit,
                five_hour_input, five_hour_output, five_hour_cache_read, five_hour_cache_write,
                weekly_input, weekly_output, weekly_cache_read, weekly_cache_write,
                total_input, total_output, total_cache_read, total_cache_write)
            SELECT key_id, model,
                hourly_limit, weekly_limit, total_limit,
                hourly_input, hourly_output, hourly_cache_read, hourly_cache_write,
                weekly_input, weekly_output, weekly_cache_read, weekly_cache_write,
                total_input, total_output, total_cache_read, total_cache_write
            FROM key_model_usage
            "#,
            (),
        )
        .await
        .map_err(|e| {
            ProxyError::DatabaseError(format!("v5: Failed to copy key_model_usage data: {e}"))
        })?;

        conn.execute("DROP TABLE key_model_usage", ())
            .await
            .map_err(|e| {
                ProxyError::DatabaseError(format!("v5: Failed to drop old key_model_usage: {e}"))
            })?;

        conn.execute(
            "ALTER TABLE key_model_usage_new RENAME TO key_model_usage",
            (),
        )
        .await
        .map_err(|e| {
            ProxyError::DatabaseError(format!("v5: Failed to rename key_model_usage_new: {e}"))
        })?;

        // Re-enable foreign keys after migration
        conn.execute("PRAGMA foreign_keys = ON", ())
            .await
            .map_err(|e| {
                ProxyError::DatabaseError(format!("v5: Failed to re-enable foreign keys: {e}"))
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

    run_migrations(&conn, path).await?;

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

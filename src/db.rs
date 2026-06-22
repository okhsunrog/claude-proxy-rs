use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::OnceCell;
use tracing::info;

use crate::constants::SEED_MODELS;
use crate::error::ProxyError;

/// Global database pool.
static DATABASE: OnceCell<PgPool> = OnceCell::const_new();

pub type Connection = PgPool;

/// Initialize the PostgreSQL database and ensure the current schema exists.
pub async fn init_db(database_url: &str) -> Result<(), ProxyError> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to connect to PostgreSQL: {e}")))?;

    create_current_schema(&pool).await?;
    seed_models_if_empty(&pool).await?;

    DATABASE
        .set(pool)
        .map_err(|_| ProxyError::DatabaseError("Database already initialized".into()))?;

    info!("PostgreSQL database initialized");
    Ok(())
}

/// Get the shared PostgreSQL connection pool.
pub async fn get_conn() -> Result<Connection, ProxyError> {
    DATABASE
        .get()
        .cloned()
        .ok_or_else(|| ProxyError::DatabaseError("Database not initialized".into()))
}

async fn create_current_schema(conn: &Connection) -> Result<(), ProxyError> {
    sqlx::query!(
        r#"
        CREATE TABLE IF NOT EXISTS auth (
            provider TEXT PRIMARY KEY,
            auth_type TEXT NOT NULL,
            access_token TEXT NOT NULL,
            refresh_token TEXT NOT NULL,
            expires_at BIGINT NOT NULL,
            account_id TEXT,
            enterprise_url TEXT
        )
        "#,
    )
    .execute(conn)
    .await
    .map_err(|e| ProxyError::DatabaseError(format!("Failed to create auth table: {e}")))?;

    sqlx::query!(
        r#"
        CREATE TABLE IF NOT EXISTS client_keys (
            id TEXT PRIMARY KEY,
            key TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            enabled BIGINT NOT NULL DEFAULT 1,
            created_at BIGINT NOT NULL,
            last_used_at BIGINT,
            five_hour_limit BIGINT,
            weekly_limit BIGINT,
            total_limit BIGINT,
            five_hour_reset_at BIGINT NOT NULL DEFAULT 0,
            weekly_reset_at BIGINT NOT NULL DEFAULT 0,
            five_hour_count_from BIGINT NOT NULL DEFAULT 0,
            weekly_count_from BIGINT NOT NULL DEFAULT 0,
            total_count_from BIGINT NOT NULL DEFAULT 0,
            allow_extra_usage BIGINT NOT NULL DEFAULT 0
        )
        "#,
    )
    .execute(conn)
    .await
    .map_err(|e| ProxyError::DatabaseError(format!("Failed to create client_keys table: {e}")))?;

    sqlx::query!(
        r#"
        CREATE TABLE IF NOT EXISTS models (
            id TEXT PRIMARY KEY,
            sort_order BIGINT NOT NULL DEFAULT 0,
            enabled BIGINT NOT NULL DEFAULT 1,
            input_price DOUBLE PRECISION NOT NULL DEFAULT 0,
            output_price DOUBLE PRECISION NOT NULL DEFAULT 0,
            cache_read_price DOUBLE PRECISION NOT NULL DEFAULT 0,
            cache_write_price DOUBLE PRECISION NOT NULL DEFAULT 0
        )
        "#,
    )
    .execute(conn)
    .await
    .map_err(|e| ProxyError::DatabaseError(format!("Failed to create models table: {e}")))?;

    sqlx::query!(
        r#"
        CREATE TABLE IF NOT EXISTS key_allowed_models (
            key_id TEXT NOT NULL REFERENCES client_keys(id) ON DELETE CASCADE,
            model TEXT NOT NULL,
            PRIMARY KEY (key_id, model)
        )
        "#,
    )
    .execute(conn)
    .await
    .map_err(|e| {
        ProxyError::DatabaseError(format!("Failed to create key_allowed_models table: {e}"))
    })?;

    sqlx::query!(
        r#"
        CREATE TABLE IF NOT EXISTS admin_sessions (
            token TEXT PRIMARY KEY,
            expires_at BIGINT NOT NULL
        )
        "#,
    )
    .execute(conn)
    .await
    .map_err(|e| {
        ProxyError::DatabaseError(format!("Failed to create admin_sessions table: {e}"))
    })?;

    sqlx::query!(
        r#"
        CREATE TABLE IF NOT EXISTS request_log (
            id BIGSERIAL PRIMARY KEY,
            key_id TEXT NOT NULL,
            model TEXT NOT NULL,
            input_tokens BIGINT NOT NULL DEFAULT 0,
            output_tokens BIGINT NOT NULL DEFAULT 0,
            cache_read_tokens BIGINT NOT NULL DEFAULT 0,
            cache_write_tokens BIGINT NOT NULL DEFAULT 0,
            cost_microdollars BIGINT NOT NULL DEFAULT 0,
            created_at BIGINT NOT NULL
        )
        "#,
    )
    .execute(conn)
    .await
    .map_err(|e| ProxyError::DatabaseError(format!("Failed to create request_log table: {e}")))?;

    sqlx::query!(
        "CREATE INDEX IF NOT EXISTS idx_request_log_created_at ON request_log(created_at)"
    )
    .execute(conn)
    .await
    .map_err(|e| ProxyError::DatabaseError(format!("Failed to create created_at index: {e}")))?;

    sqlx::query!(
        "CREATE INDEX IF NOT EXISTS idx_request_log_key_created ON request_log(key_id, created_at)",
    )
    .execute(conn)
    .await
    .map_err(|e| ProxyError::DatabaseError(format!("Failed to create key_created index: {e}")))?;

    sqlx::query!(
        r#"
        CREATE TABLE IF NOT EXISTS key_model_limits (
            key_id TEXT NOT NULL REFERENCES client_keys(id) ON DELETE CASCADE,
            model TEXT NOT NULL,
            five_hour_limit BIGINT,
            weekly_limit BIGINT,
            total_limit BIGINT,
            count_from BIGINT NOT NULL DEFAULT 0,
            PRIMARY KEY (key_id, model)
        )
        "#,
    )
    .execute(conn)
    .await
    .map_err(|e| {
        ProxyError::DatabaseError(format!("Failed to create key_model_limits table: {e}"))
    })?;

    Ok(())
}

async fn seed_models_if_empty(conn: &Connection) -> Result<(), ProxyError> {
    let model_count = sqlx::query_scalar!("SELECT COUNT(*) FROM models")
        .fetch_one(conn)
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to count models: {e}")))?;

    if model_count.unwrap_or(0) == 0 {
        info!(
            "Seeding models table with {} default models",
            SEED_MODELS.len()
        );
        for (i, &(id, input_price, output_price, cache_read_price, cache_write_price)) in
            SEED_MODELS.iter().enumerate()
        {
            sqlx::query!(
                "INSERT INTO models (id, sort_order, enabled, input_price, output_price, cache_read_price, cache_write_price) VALUES ($1, $2, 1, $3, $4, $5, $6)",
                id,
                i as i64,
                input_price,
                output_price,
                cache_read_price,
                cache_write_price,
            )
            .execute(conn)
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to seed model {id}: {e}")))?;
        }
    }

    Ok(())
}

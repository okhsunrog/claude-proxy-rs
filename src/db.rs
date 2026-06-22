use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::OnceCell;
use tracing::info;

use crate::constants::SEED_MODELS;
use crate::error::ProxyError;

/// Global database pool.
static DATABASE: OnceCell<PgPool> = OnceCell::const_new();

pub type Connection = PgPool;

/// Initialize the PostgreSQL database and apply schema migrations.
pub async fn init_db(database_url: &str) -> Result<(), ProxyError> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to connect to PostgreSQL: {e}")))?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to run migrations: {e}")))?;
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

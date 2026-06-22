use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::OnceCell;
use tracing::info;

use crate::constants::SEED_MODELS;
use crate::error::{DbResultExt, ProxyError};

/// Global database pool.
static DATABASE: OnceCell<PgPool> = OnceCell::const_new();

pub type Connection = PgPool;

/// Initialize the PostgreSQL database and apply schema migrations.
pub async fn init_db(database_url: &str) -> Result<(), ProxyError> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await
        .db_context("Failed to connect to PostgreSQL")?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .db_context("Failed to run migrations")?;
    seed_models_if_empty(&pool).await?;

    DATABASE
        .set(pool)
        .map_err(|_pool| ProxyError::DatabaseState("Database already initialized"))?;

    info!("PostgreSQL database initialized");
    Ok(())
}

/// Get the shared PostgreSQL connection pool.
pub async fn get_conn() -> Result<Connection, ProxyError> {
    DATABASE
        .get()
        .cloned()
        .ok_or(ProxyError::DatabaseState("Database not initialized"))
}

async fn seed_models_if_empty(conn: &Connection) -> Result<(), ProxyError> {
    let model_count = sqlx::query_scalar!("SELECT COUNT(*) FROM models")
        .fetch_one(conn)
        .await
        .db_context("Failed to count models")?;

    if model_count.unwrap_or(0) == 0 {
        info!(
            "Seeding models table with {} default models",
            SEED_MODELS.len()
        );
        for (i, &(id, input_price, output_price, cache_read_price, cache_write_price)) in
            SEED_MODELS.iter().enumerate()
        {
            sqlx::query!(
                "INSERT INTO models (id, sort_order, enabled, input_price, output_price, cache_read_price, cache_write_price) VALUES ($1, $2, TRUE, $3, $4, $5, $6)",
                id,
                i as i64,
                input_price,
                output_price,
                cache_read_price,
                cache_write_price,
            )
            .execute(conn)
            .await
            .db_context("Failed to seed model")?;
        }
    }

    Ok(())
}

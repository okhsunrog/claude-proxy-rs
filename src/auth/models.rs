use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::db;
use crate::error::{DbResultExt, ProxyError};

/// Model pricing for cost calculation during requests
#[derive(Debug, Clone)]
pub struct ModelPricing {
    pub input_price: f64,
    pub output_price: f64,
    pub cache_read_price: f64,
    pub cache_write_price: f64,
}

/// A model entry from the database
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub sort_order: i64,
    pub enabled: bool,
    pub input_price: f64,
    pub output_price: f64,
    pub cache_read_price: f64,
    pub cache_write_price: f64,
}

pub struct ModelsStore;

struct ModelRow {
    id: String,
    sort_order: i64,
    enabled: bool,
    input_price: f64,
    output_price: f64,
    cache_read_price: f64,
    cache_write_price: f64,
}

fn row_to_model(row: ModelRow) -> Model {
    Model {
        id: row.id,
        sort_order: row.sort_order,
        enabled: row.enabled,
        input_price: row.input_price,
        output_price: row.output_price,
        cache_read_price: row.cache_read_price,
        cache_write_price: row.cache_write_price,
    }
}

impl ModelsStore {
    pub fn new() -> Self {
        Self
    }

    /// List all models ordered by sort_order
    pub async fn list(&self) -> Result<Vec<Model>, ProxyError> {
        let conn = db::get_conn().await?;
        let rows = sqlx::query_as!(
            ModelRow,
            "SELECT id, sort_order, enabled, input_price, output_price, cache_read_price, cache_write_price FROM models ORDER BY sort_order",
        )
        .fetch_all(&conn)
        .await
        .db_context("Failed to list models")?;

        Ok(rows.into_iter().map(row_to_model).collect())
    }

    /// List only enabled models (for API endpoints)
    pub async fn list_enabled(&self) -> Result<Vec<Model>, ProxyError> {
        let conn = db::get_conn().await?;
        let rows = sqlx::query_as!(
            ModelRow,
            "SELECT id, sort_order, enabled, input_price, output_price, cache_read_price, cache_write_price FROM models WHERE enabled = TRUE ORDER BY sort_order",
        )
        .fetch_all(&conn)
        .await
        .db_context("Failed to list enabled models")?;

        Ok(rows.into_iter().map(row_to_model).collect())
    }

    /// List only enabled model IDs (for /v1/models endpoint)
    pub async fn list_enabled_ids(&self) -> Result<Vec<String>, ProxyError> {
        let conn = db::get_conn().await?;
        let rows = sqlx::query!("SELECT id FROM models WHERE enabled = TRUE ORDER BY sort_order")
            .fetch_all(&conn)
            .await
            .db_context("Failed to list model IDs")?;

        Ok(rows.into_iter().map(|row| row.id).collect())
    }

    /// Get pricing for a model (for cost calculation)
    pub async fn get_pricing(&self, model_id: &str) -> Option<ModelPricing> {
        let conn = db::get_conn().await.ok()?;
        let row = sqlx::query!(
            "SELECT input_price, output_price, cache_read_price, cache_write_price FROM models WHERE id = $1 AND enabled = TRUE",
            model_id,
        )
        .fetch_optional(&conn)
        .await
        .ok()??;
        Some(ModelPricing {
            input_price: row.input_price,
            output_price: row.output_price,
            cache_read_price: row.cache_read_price,
            cache_write_price: row.cache_write_price,
        })
    }

    /// Add a new model
    pub async fn add(
        &self,
        id: &str,
        input_price: f64,
        output_price: f64,
        cache_read_price: f64,
        cache_write_price: f64,
    ) -> Result<(), ProxyError> {
        let conn = db::get_conn().await?;
        // Set sort_order to max + 1
        let next_order =
            sqlx::query_scalar!("SELECT COALESCE(MAX(sort_order), -1) + 1 FROM models")
                .fetch_one(&conn)
                .await
                .db_context("Failed to get max sort_order")?;

        sqlx::query!(
            "INSERT INTO models (id, sort_order, enabled, input_price, output_price, cache_read_price, cache_write_price) VALUES ($1, $2, TRUE, $3, $4, $5, $6)",
            id,
            next_order,
            input_price,
            output_price,
            cache_read_price,
            cache_write_price,
        )
        .execute(&conn)
        .await
        .db_context("Failed to add model")?;
        Ok(())
    }

    /// Remove a model (cascades to key_allowed_models and key_model_usage via FK)
    pub async fn remove(&self, id: &str) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;
        let affected = sqlx::query!("DELETE FROM models WHERE id = $1", id)
            .execute(&conn)
            .await
            .db_context("Failed to remove model")?
            .rows_affected();
        Ok(affected > 0)
    }

    /// Reorder models (accepts list of model IDs in desired order)
    pub async fn reorder(&self, ids: Vec<String>) -> Result<(), ProxyError> {
        let conn = db::get_conn().await?;
        for (i, id) in ids.iter().enumerate() {
            sqlx::query!(
                "UPDATE models SET sort_order = $1 WHERE id = $2",
                i as i64,
                id
            )
            .execute(&conn)
            .await
            .db_context("Failed to reorder models")?;
        }
        Ok(())
    }

    /// Toggle model enabled/disabled
    pub async fn set_enabled(&self, id: &str, enabled: bool) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;
        let affected = sqlx::query!("UPDATE models SET enabled = $1 WHERE id = $2", enabled, id)
            .execute(&conn)
            .await
            .db_context("Failed to set model enabled")?
            .rows_affected();
        Ok(affected > 0)
    }

    /// Update model prices
    pub async fn update(
        &self,
        id: &str,
        input_price: Option<f64>,
        output_price: Option<f64>,
        cache_read_price: Option<f64>,
        cache_write_price: Option<f64>,
        enabled: Option<bool>,
    ) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;

        let affected = sqlx::query!(
            "UPDATE models SET \
             input_price = COALESCE($1, input_price), \
             output_price = COALESCE($2, output_price), \
             cache_read_price = COALESCE($3, cache_read_price), \
             cache_write_price = COALESCE($4, cache_write_price), \
             enabled = COALESCE($5, enabled) \
             WHERE id = $6",
            input_price,
            output_price,
            cache_read_price,
            cache_write_price,
            enabled,
            id,
        )
        .execute(&conn)
        .await
        .db_context("Failed to update model")?
        .rows_affected();
        Ok(affected > 0)
    }

    /// Check if a model exists and is enabled
    pub async fn is_valid(&self, model_id: &str) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;
        let count = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM models WHERE id = $1 AND enabled = TRUE",
            model_id
        )
        .fetch_one(&conn)
        .await
        .db_context("Failed to check model")?;
        Ok(count.unwrap_or(0) > 0)
    }
}

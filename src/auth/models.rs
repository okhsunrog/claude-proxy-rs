use serde::{Deserialize, Serialize};
use sqlx::Row;
use utoipa::ToSchema;

use crate::db;
use crate::error::ProxyError;

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

impl ModelsStore {
    pub fn new() -> Self {
        Self
    }

    /// List all models ordered by sort_order
    pub async fn list(&self) -> Result<Vec<Model>, ProxyError> {
        let conn = db::get_conn().await?;
        let rows = sqlx::query(
            "SELECT id, sort_order, enabled, input_price, output_price, cache_read_price, cache_write_price FROM models ORDER BY sort_order",
        )
        .fetch_all(&conn)
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to list models: {e}")))?;

        let mut models = Vec::new();
        for row in rows {
            if let Ok(id) = row.try_get::<String, _>(0) {
                models.push(Model {
                    id,
                    sort_order: row.try_get::<i64, _>(1).unwrap_or(0),
                    enabled: row.try_get::<i64, _>(2).unwrap_or(1) != 0,
                    input_price: row.try_get::<f64, _>(3).unwrap_or(0.0),
                    output_price: row.try_get::<f64, _>(4).unwrap_or(0.0),
                    cache_read_price: row.try_get::<f64, _>(5).unwrap_or(0.0),
                    cache_write_price: row.try_get::<f64, _>(6).unwrap_or(0.0),
                });
            }
        }
        Ok(models)
    }

    /// List only enabled models (for API endpoints)
    pub async fn list_enabled(&self) -> Result<Vec<Model>, ProxyError> {
        let conn = db::get_conn().await?;
        let rows = sqlx::query(
            "SELECT id, sort_order, enabled, input_price, output_price, cache_read_price, cache_write_price FROM models WHERE enabled = 1 ORDER BY sort_order",
        )
        .fetch_all(&conn)
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to list enabled models: {e}")))?;

        let mut models = Vec::new();
        for row in rows {
            if let Ok(id) = row.try_get::<String, _>(0) {
                models.push(Model {
                    id,
                    sort_order: row.try_get::<i64, _>(1).unwrap_or(0),
                    enabled: true,
                    input_price: row.try_get::<f64, _>(3).unwrap_or(0.0),
                    output_price: row.try_get::<f64, _>(4).unwrap_or(0.0),
                    cache_read_price: row.try_get::<f64, _>(5).unwrap_or(0.0),
                    cache_write_price: row.try_get::<f64, _>(6).unwrap_or(0.0),
                });
            }
        }
        Ok(models)
    }

    /// List only enabled model IDs (for /v1/models endpoint)
    pub async fn list_enabled_ids(&self) -> Result<Vec<String>, ProxyError> {
        let conn = db::get_conn().await?;
        let rows = sqlx::query("SELECT id FROM models WHERE enabled = 1 ORDER BY sort_order")
            .fetch_all(&conn)
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to list model IDs: {e}")))?;

        let mut ids = Vec::new();
        for row in rows {
            if let Ok(id) = row.try_get::<String, _>(0) {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    /// Get pricing for a model (for cost calculation)
    pub async fn get_pricing(&self, model_id: &str) -> Option<ModelPricing> {
        let conn = db::get_conn().await.ok()?;
        let row = sqlx::query(
            "SELECT input_price, output_price, cache_read_price, cache_write_price FROM models WHERE id = $1 AND enabled = 1",
        )
        .bind(model_id)
        .fetch_optional(&conn)
        .await
        .ok()??;
        Some(ModelPricing {
            input_price: row.try_get::<f64, _>(0).unwrap_or(0.0),
            output_price: row.try_get::<f64, _>(1).unwrap_or(0.0),
            cache_read_price: row.try_get::<f64, _>(2).unwrap_or(0.0),
            cache_write_price: row.try_get::<f64, _>(3).unwrap_or(0.0),
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
        let row = sqlx::query("SELECT COALESCE(MAX(sort_order), -1) + 1 FROM models")
            .fetch_one(&conn)
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to get max sort_order: {e}")))?;
        let next_order: i64 = row.try_get::<i64, _>(0).unwrap_or(0);

        sqlx::query(
            "INSERT INTO models (id, sort_order, enabled, input_price, output_price, cache_read_price, cache_write_price) VALUES ($1, $2, 1, $3, $4, $5, $6)",
        )
        .bind(id)
        .bind(next_order)
        .bind(input_price)
        .bind(output_price)
        .bind(cache_read_price)
        .bind(cache_write_price)
        .execute(&conn)
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to add model: {e}")))?;
        Ok(())
    }

    /// Remove a model (cascades to key_allowed_models and key_model_usage via FK)
    pub async fn remove(&self, id: &str) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;
        let affected = sqlx::query("DELETE FROM models WHERE id = $1")
            .bind(id)
            .execute(&conn)
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to remove model: {e}")))?
            .rows_affected();
        Ok(affected > 0)
    }

    /// Reorder models (accepts list of model IDs in desired order)
    pub async fn reorder(&self, ids: Vec<String>) -> Result<(), ProxyError> {
        let conn = db::get_conn().await?;
        for (i, id) in ids.iter().enumerate() {
            sqlx::query("UPDATE models SET sort_order = $1 WHERE id = $2")
                .bind(i as i64)
                .bind(id.as_str())
                .execute(&conn)
                .await
                .map_err(|e| ProxyError::DatabaseError(format!("Failed to reorder models: {e}")))?;
        }
        Ok(())
    }

    /// Toggle model enabled/disabled
    pub async fn set_enabled(&self, id: &str, enabled: bool) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;
        let affected = sqlx::query("UPDATE models SET enabled = $1 WHERE id = $2")
            .bind(enabled as i64)
            .bind(id)
            .execute(&conn)
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to set model enabled: {e}")))?
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
        let enabled_i64 = enabled.map(|v| v as i64);

        let affected = sqlx::query(
            "UPDATE models SET \
             input_price = COALESCE($1, input_price), \
             output_price = COALESCE($2, output_price), \
             cache_read_price = COALESCE($3, cache_read_price), \
             cache_write_price = COALESCE($4, cache_write_price), \
             enabled = COALESCE($5, enabled) \
             WHERE id = $6",
        )
        .bind(input_price)
        .bind(output_price)
        .bind(cache_read_price)
        .bind(cache_write_price)
        .bind(enabled_i64)
        .bind(id)
        .execute(&conn)
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to update model: {e}")))?
        .rows_affected();
        Ok(affected > 0)
    }

    /// Check if a model exists and is enabled
    pub async fn is_valid(&self, model_id: &str) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;
        let row = sqlx::query("SELECT COUNT(*) FROM models WHERE id = $1 AND enabled = 1")
            .bind(model_id)
            .fetch_one(&conn)
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to check model: {e}")))?;
        let count = row.try_get::<i64, _>(0).unwrap_or(0);
        Ok(count > 0)
    }
}

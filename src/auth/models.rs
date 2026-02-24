use serde::{Deserialize, Serialize};
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
        let mut rows = conn
            .query(
                "SELECT id, sort_order, enabled, input_price, output_price, cache_read_price, cache_write_price FROM models ORDER BY sort_order",
                (),
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to list models: {e}")))?;

        let mut models = Vec::new();
        while let Ok(Some(row)) = rows.next().await {
            if let Ok(id) = row.get::<String>(0) {
                models.push(Model {
                    id,
                    sort_order: row.get::<i64>(1).unwrap_or(0),
                    enabled: row.get::<i64>(2).unwrap_or(1) != 0,
                    input_price: row.get::<f64>(3).unwrap_or(0.0),
                    output_price: row.get::<f64>(4).unwrap_or(0.0),
                    cache_read_price: row.get::<f64>(5).unwrap_or(0.0),
                    cache_write_price: row.get::<f64>(6).unwrap_or(0.0),
                });
            }
        }
        Ok(models)
    }

    /// List only enabled models (for API endpoints)
    pub async fn list_enabled(&self) -> Result<Vec<Model>, ProxyError> {
        let conn = db::get_conn().await?;
        let mut rows = conn
            .query(
                "SELECT id, sort_order, enabled, input_price, output_price, cache_read_price, cache_write_price FROM models WHERE enabled = 1 ORDER BY sort_order",
                (),
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to list enabled models: {e}")))?;

        let mut models = Vec::new();
        while let Ok(Some(row)) = rows.next().await {
            if let Ok(id) = row.get::<String>(0) {
                models.push(Model {
                    id,
                    sort_order: row.get::<i64>(1).unwrap_or(0),
                    enabled: true,
                    input_price: row.get::<f64>(3).unwrap_or(0.0),
                    output_price: row.get::<f64>(4).unwrap_or(0.0),
                    cache_read_price: row.get::<f64>(5).unwrap_or(0.0),
                    cache_write_price: row.get::<f64>(6).unwrap_or(0.0),
                });
            }
        }
        Ok(models)
    }

    /// List only enabled model IDs (for /v1/models endpoint)
    pub async fn list_enabled_ids(&self) -> Result<Vec<String>, ProxyError> {
        let conn = db::get_conn().await?;
        let mut rows = conn
            .query(
                "SELECT id FROM models WHERE enabled = 1 ORDER BY sort_order",
                (),
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to list model IDs: {e}")))?;

        let mut ids = Vec::new();
        while let Ok(Some(row)) = rows.next().await {
            if let Ok(id) = row.get::<String>(0) {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    /// Get pricing for a model (for cost calculation)
    pub async fn get_pricing(&self, model_id: &str) -> Option<ModelPricing> {
        let conn = db::get_conn().await.ok()?;
        let mut rows = conn
            .query(
                "SELECT input_price, output_price, cache_read_price, cache_write_price FROM models WHERE id = ? AND enabled = 1",
                [model_id],
            )
            .await
            .ok()?;
        let row = rows.next().await.ok()??;
        Some(ModelPricing {
            input_price: row.get::<f64>(0).unwrap_or(0.0),
            output_price: row.get::<f64>(1).unwrap_or(0.0),
            cache_read_price: row.get::<f64>(2).unwrap_or(0.0),
            cache_write_price: row.get::<f64>(3).unwrap_or(0.0),
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
        let mut rows = conn
            .query("SELECT COALESCE(MAX(sort_order), -1) + 1 FROM models", ())
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to get max sort_order: {e}")))?;
        let next_order: i64 = rows
            .next()
            .await
            .ok()
            .flatten()
            .and_then(|r| r.get::<i64>(0).ok())
            .unwrap_or(0);

        conn.execute(
            "INSERT INTO models (id, sort_order, enabled, input_price, output_price, cache_read_price, cache_write_price) VALUES (?, ?, 1, ?, ?, ?, ?)",
            (id, next_order, input_price, output_price, cache_read_price, cache_write_price),
        )
        .await
        .map_err(|e| ProxyError::DatabaseError(format!("Failed to add model: {e}")))?;
        Ok(())
    }

    /// Remove a model (cascades to key_allowed_models and key_model_usage via FK)
    pub async fn remove(&self, id: &str) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;
        let affected = conn
            .execute("DELETE FROM models WHERE id = ?", [id])
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to remove model: {e}")))?;
        Ok(affected > 0)
    }

    /// Reorder models (accepts list of model IDs in desired order)
    pub async fn reorder(&self, ids: Vec<String>) -> Result<(), ProxyError> {
        let conn = db::get_conn().await?;
        for (i, id) in ids.iter().enumerate() {
            conn.execute(
                "UPDATE models SET sort_order = ? WHERE id = ?",
                (i as i64, id.as_str()),
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to reorder models: {e}")))?;
        }
        Ok(())
    }

    /// Toggle model enabled/disabled
    pub async fn set_enabled(&self, id: &str, enabled: bool) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;
        let affected = conn
            .execute(
                "UPDATE models SET enabled = ? WHERE id = ?",
                (enabled as i64, id),
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to set model enabled: {e}")))?;
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

        let affected = conn
            .execute(
                "UPDATE models SET \
                 input_price = COALESCE(?, input_price), \
                 output_price = COALESCE(?, output_price), \
                 cache_read_price = COALESCE(?, cache_read_price), \
                 cache_write_price = COALESCE(?, cache_write_price), \
                 enabled = COALESCE(?, enabled) \
                 WHERE id = ?",
                (
                    input_price,
                    output_price,
                    cache_read_price,
                    cache_write_price,
                    enabled_i64,
                    id,
                ),
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to update model: {e}")))?;
        Ok(affected > 0)
    }

    /// Check if a model exists and is enabled
    pub async fn is_valid(&self, model_id: &str) -> Result<bool, ProxyError> {
        let conn = db::get_conn().await?;
        let mut rows = conn
            .query(
                "SELECT COUNT(*) FROM models WHERE id = ? AND enabled = 1",
                [model_id],
            )
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to check model: {e}")))?;
        let count = rows
            .next()
            .await
            .ok()
            .flatten()
            .and_then(|r| r.get::<i64>(0).ok())
            .unwrap_or(0);
        Ok(count > 0)
    }
}

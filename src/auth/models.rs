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
    pub async fn list(&self) -> Vec<Model> {
        let Ok(conn) = db::get_conn() else {
            return Vec::new();
        };
        let Ok(mut rows) = conn
            .query(
                "SELECT id, sort_order, enabled, input_price, output_price, cache_read_price, cache_write_price FROM models ORDER BY sort_order",
                (),
            )
            .await
        else {
            return Vec::new();
        };

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
        models
    }

    /// List only enabled models (for API endpoints)
    pub async fn list_enabled(&self) -> Vec<Model> {
        self.list()
            .await
            .into_iter()
            .filter(|m| m.enabled)
            .collect()
    }

    /// List only enabled model IDs (for /v1/models endpoint)
    pub async fn list_enabled_ids(&self) -> Vec<String> {
        self.list_enabled()
            .await
            .into_iter()
            .map(|m| m.id)
            .collect()
    }

    /// Get pricing for a model (for cost calculation)
    pub async fn get_pricing(&self, model_id: &str) -> Option<ModelPricing> {
        let conn = db::get_conn().ok()?;
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
        let conn = db::get_conn()?;
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
        let conn = db::get_conn()?;
        let affected = conn
            .execute("DELETE FROM models WHERE id = ?", [id])
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to remove model: {e}")))?;
        Ok(affected > 0)
    }

    /// Reorder models (accepts list of model IDs in desired order)
    pub async fn reorder(&self, ids: Vec<String>) -> Result<(), ProxyError> {
        let conn = db::get_conn()?;
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
        let conn = db::get_conn()?;
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
        let conn = db::get_conn()?;
        let mut sets = Vec::new();

        if let Some(v) = input_price {
            sets.push(format!("input_price = {v}"));
        }
        if let Some(v) = output_price {
            sets.push(format!("output_price = {v}"));
        }
        if let Some(v) = cache_read_price {
            sets.push(format!("cache_read_price = {v}"));
        }
        if let Some(v) = cache_write_price {
            sets.push(format!("cache_write_price = {v}"));
        }
        if let Some(v) = enabled {
            sets.push(format!("enabled = {}", v as i64));
        }

        if sets.is_empty() {
            return Ok(false);
        }

        let sql = format!("UPDATE models SET {} WHERE id = ?", sets.join(", "));
        let affected = conn
            .execute(&sql, [id])
            .await
            .map_err(|e| ProxyError::DatabaseError(format!("Failed to update model: {e}")))?;
        Ok(affected > 0)
    }

    /// Check if a model exists and is enabled
    pub async fn is_valid(&self, model_id: &str) -> bool {
        let Ok(conn) = db::get_conn() else {
            return false;
        };
        let Ok(mut rows) = conn
            .query(
                "SELECT COUNT(*) FROM models WHERE id = ? AND enabled = 1",
                [model_id],
            )
            .await
        else {
            return false;
        };
        rows.next()
            .await
            .ok()
            .flatten()
            .and_then(|r| r.get::<i64>(0).ok())
            .unwrap_or(0)
            > 0
    }
}

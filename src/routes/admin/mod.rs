mod keys;
mod models;
mod oauth;
mod session;
mod usage_history;

// Glob re-exports so utoipa's `routes!()` macro can find the hidden `__path_*` structs
// alongside the handler functions at the `crate::routes::admin::*` path.
pub use keys::*;
pub use models::*;
pub use oauth::*;
pub use session::*;
pub use usage_history::*;

use axum::Router;
use serde::Serialize;
use std::sync::Arc;
use utoipa::ToSchema;

use crate::AppState;

// --- Shared response types ---

#[derive(Serialize, ToSchema)]
pub struct SuccessResponse {
    pub success: bool,
}

#[derive(Serialize, ToSchema)]
pub struct ErrorResponse {
    pub error: String,
}

// --- Validation helpers ---

const MAX_KEY_NAME_LENGTH: usize = 100;

pub(super) fn validate_key_name(name: &str) -> Result<(), &'static str> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Key name cannot be empty");
    }
    if name.len() > MAX_KEY_NAME_LENGTH {
        return Err("Key name too long (max 100 characters)");
    }
    if name.chars().any(|c| c.is_control()) {
        return Err("Key name cannot contain control characters");
    }
    Ok(())
}

const MAX_MODEL_ID_LENGTH: usize = 100;

pub(super) fn validate_model_id(id: &str) -> Result<(), &'static str> {
    let id = id.trim();
    if id.is_empty() {
        return Err("Model ID cannot be empty");
    }
    if id.len() > MAX_MODEL_ID_LENGTH {
        return Err("Model ID too long (max 100 characters)");
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'))
    {
        return Err(
            "Model ID can only contain letters, digits, dots, underscores, colons, and hyphens",
        );
    }
    Ok(())
}

pub(super) fn validate_price(price: f64) -> Result<(), &'static str> {
    if !price.is_finite() {
        return Err("Price must be a finite number");
    }
    if price < 0.0 {
        return Err("Price cannot be negative");
    }
    Ok(())
}

// --- Static file serving ---

pub fn static_routes() -> Router<Arc<AppState>> {
    memory_serve::load!()
        .index_file(Some("/index.html"))
        .fallback(Some("/index.html"))
        .into_router()
}

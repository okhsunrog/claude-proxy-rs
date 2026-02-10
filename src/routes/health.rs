use axum::response::Json;
use serde_json::{Value, json};

use crate::{BUILD_TIME, GIT_HASH, VERSION};

pub async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

pub async fn version() -> Json<Value> {
    Json(json!({
        "version": VERSION,
        "git_hash": GIT_HASH,
        "build_time": BUILD_TIME,
    }))
}

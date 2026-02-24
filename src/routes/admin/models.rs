use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use super::{ErrorResponse, SuccessResponse, validate_model_id, validate_price};
use crate::AppState;
use crate::auth::Model;

// --- Types ---

#[derive(Serialize, ToSchema)]
pub struct ListModelsResponse {
    pub models: Vec<Model>,
}

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AddModelRequest {
    pub id: String,
    #[serde(default)]
    pub input_price: f64,
    #[serde(default)]
    pub output_price: f64,
    #[serde(default)]
    pub cache_read_price: f64,
    #[serde(default)]
    pub cache_write_price: f64,
}

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateModelRequest {
    pub enabled: Option<bool>,
    pub input_price: Option<f64>,
    pub output_price: Option<f64>,
    pub cache_read_price: Option<f64>,
    pub cache_write_price: Option<f64>,
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct ReorderModelsRequest {
    pub ids: Vec<String>,
}

// --- Handlers ---

/// List all models (admin sees enabled + disabled)
#[utoipa::path(
    get,
    path = "/models",
    tag = "models",
    responses(
        (status = 200, body = ListModelsResponse),
    )
)]
pub async fn list_models_admin(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ListModelsResponse>, (StatusCode, Json<ErrorResponse>)> {
    let models = state.models.list().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;
    Ok(Json(ListModelsResponse { models }))
}

/// Add a new model
#[utoipa::path(
    post,
    path = "/models",
    tag = "models",
    request_body = AddModelRequest,
    responses(
        (status = 200, body = SuccessResponse),
        (status = 400, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn add_model(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AddModelRequest>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    let id = body.id.trim();
    if let Err(e) = validate_model_id(id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse { error: e.into() }),
        ));
    }

    for (label, price) in [
        ("Input price", body.input_price),
        ("Output price", body.output_price),
        ("Cache read price", body.cache_read_price),
        ("Cache write price", body.cache_write_price),
    ] {
        if let Err(e) = validate_price(price) {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("{label}: {e}"),
                }),
            ));
        }
    }

    match state
        .models
        .add(
            id,
            body.input_price,
            body.output_price,
            body.cache_read_price,
            body.cache_write_price,
        )
        .await
    {
        Ok(_) => Ok(Json(SuccessResponse { success: true })),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

/// Delete a model
#[utoipa::path(
    delete,
    path = "/models/{id}",
    tag = "models",
    params(("id" = String, Path, description = "Model ID")),
    responses(
        (status = 200, body = SuccessResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn delete_model(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.models.remove(&id).await {
        Ok(true) => Ok(Json(SuccessResponse { success: true })),
        Ok(false) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Model not found".into(),
            }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

/// Update a model (prices, enabled)
#[utoipa::path(
    put,
    path = "/models/{id}",
    tag = "models",
    params(("id" = String, Path, description = "Model ID")),
    request_body = UpdateModelRequest,
    responses(
        (status = 200, body = SuccessResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn update_model(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateModelRequest>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    for (label, price) in [
        ("Input price", body.input_price),
        ("Output price", body.output_price),
        ("Cache read price", body.cache_read_price),
        ("Cache write price", body.cache_write_price),
    ] {
        if let Some(p) = price
            && let Err(e) = validate_price(p)
        {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("{label}: {e}"),
                }),
            ));
        }
    }

    match state
        .models
        .update(
            &id,
            body.input_price,
            body.output_price,
            body.cache_read_price,
            body.cache_write_price,
            body.enabled,
        )
        .await
    {
        Ok(true) => Ok(Json(SuccessResponse { success: true })),
        Ok(false) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Model not found".into(),
            }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

/// Reorder models
#[utoipa::path(
    put,
    path = "/models/reorder",
    tag = "models",
    request_body = ReorderModelsRequest,
    responses(
        (status = 200, body = SuccessResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn reorder_models(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ReorderModelsRequest>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.models.reorder(body.ids).await {
        Ok(_) => Ok(Json(SuccessResponse { success: true })),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

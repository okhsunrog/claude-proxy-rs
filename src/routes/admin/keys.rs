use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use super::{ErrorResponse, SuccessResponse, validate_key_name};
use crate::AppState;
use crate::auth::{ClientKey, ModelUsageEntry, TokenLimits, TokenUsage, UsageResetType};

// --- Types ---

#[derive(Serialize, ToSchema)]
pub struct CreateKeyResponse {
    pub key: String,
    pub id: String,
}

#[derive(Serialize, ToSchema)]
pub struct ListKeysResponse {
    pub keys: Vec<ClientKey>,
}

#[derive(Serialize, ToSchema)]
pub struct KeyUsageResponse {
    pub limits: TokenLimits,
    pub usage: TokenUsage,
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct CreateKeyRequest {
    name: String,
}

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateLimitsRequest {
    #[serde(rename = "fiveHourLimit", alias = "hourlyLimit")]
    five_hour_limit: Option<u64>,
    weekly_limit: Option<u64>,
    total_limit: Option<u64>,
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct SetKeyEnabledRequest {
    enabled: bool,
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct SetAllowExtraUsageRequest {
    allow_extra_usage: bool,
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct ResetUsageRequest {
    /// Which counter to reset: "hourly", "weekly", "total", or "all"
    #[serde(rename = "type")]
    reset_type: String,
}

// --- Per-key model types ---

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyModelsResponse {
    pub allow_all: bool,
    pub models: Vec<String>,
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct SetKeyModelsRequest {
    pub models: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct KeyModelUsageResponse {
    pub entries: Vec<ModelUsageEntry>,
}

// --- Handlers ---

/// Create a new API key
#[utoipa::path(
    post,
    path = "/keys",
    tag = "keys",
    request_body = CreateKeyRequest,
    responses(
        (status = 200, body = CreateKeyResponse),
        (status = 400, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn create_key(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateKeyRequest>,
) -> Result<Json<CreateKeyResponse>, (StatusCode, Json<ErrorResponse>)> {
    let name = body.name.trim().to_string();

    if let Err(e) = validate_key_name(&name) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        ));
    }

    match state.client_keys.create(name).await {
        Ok(key) => Ok(Json(CreateKeyResponse {
            key: key.key,
            id: key.id,
        })),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

/// List all API keys
#[utoipa::path(
    get,
    path = "/keys/list",
    tag = "keys",
    responses(
        (status = 200, body = ListKeysResponse),
    )
)]
pub async fn list_keys(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ListKeysResponse>, (StatusCode, Json<ErrorResponse>)> {
    let keys = state.client_keys.list().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;
    Ok(Json(ListKeysResponse { keys }))
}

/// Delete an API key
#[utoipa::path(
    delete,
    path = "/keys/{id}",
    tag = "keys",
    params(("id" = String, Path, description = "Key ID")),
    responses(
        (status = 200, body = SuccessResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn delete_key(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.client_keys.delete(&id).await {
        Ok(true) => Ok(Json(SuccessResponse { success: true })),
        Ok(false) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Key not found".into(),
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

/// Toggle a key enabled/disabled
#[utoipa::path(
    put,
    path = "/keys/{id}/enabled",
    tag = "keys",
    params(("id" = String, Path, description = "Key ID")),
    request_body = SetKeyEnabledRequest,
    responses(
        (status = 200, body = SuccessResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn set_key_enabled(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<SetKeyEnabledRequest>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.client_keys.set_enabled(&id, body.enabled).await {
        Ok(true) => Ok(Json(SuccessResponse { success: true })),
        Ok(false) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Key not found".into(),
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

/// Toggle allow_extra_usage for a key
#[utoipa::path(
    put,
    path = "/keys/{id}/allow-extra-usage",
    tag = "keys",
    params(("id" = String, Path, description = "Key ID")),
    request_body = SetAllowExtraUsageRequest,
    responses(
        (status = 200, body = SuccessResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn set_allow_extra_usage(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<SetAllowExtraUsageRequest>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state
        .client_keys
        .set_allow_extra_usage(&id, body.allow_extra_usage)
        .await
    {
        Ok(true) => Ok(Json(SuccessResponse { success: true })),
        Ok(false) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Key not found".into(),
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

/// Get usage statistics for a key
#[utoipa::path(
    get,
    path = "/keys/{id}/usage",
    tag = "keys",
    params(("id" = String, Path, description = "Key ID")),
    responses(
        (status = 200, body = KeyUsageResponse),
        (status = 404, body = ErrorResponse),
    )
)]
pub async fn get_key_usage(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<KeyUsageResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.client_keys.get_usage(&id).await {
        Ok(Some((limits, usage))) => Ok(Json(KeyUsageResponse { limits, usage })),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Key not found".into(),
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

/// Update limits for a key
#[utoipa::path(
    put,
    path = "/keys/{id}/limits",
    tag = "keys",
    params(("id" = String, Path, description = "Key ID")),
    request_body = UpdateLimitsRequest,
    responses(
        (status = 200, body = SuccessResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn update_key_limits(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateLimitsRequest>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    let limits = TokenLimits {
        five_hour_limit: body.five_hour_limit,
        weekly_limit: body.weekly_limit,
        total_limit: body.total_limit,
    };

    match state.client_keys.set_limits(&id, limits).await {
        Ok(true) => Ok(Json(SuccessResponse { success: true })),
        Ok(false) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Key not found".into(),
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

/// Reset usage counters for a key
#[utoipa::path(
    post,
    path = "/keys/{id}/usage/reset",
    tag = "keys",
    params(("id" = String, Path, description = "Key ID")),
    request_body = ResetUsageRequest,
    responses(
        (status = 200, body = SuccessResponse),
        (status = 400, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn reset_key_usage(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<ResetUsageRequest>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    let reset_type = match body.reset_type.to_lowercase().as_str() {
        "fivehour" | "hourly" => UsageResetType::FiveHour,
        "weekly" => UsageResetType::Weekly,
        "total" => UsageResetType::Total,
        "all" => UsageResetType::All,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "Invalid reset type. Use: fiveHour, weekly, total, or all".into(),
                }),
            ));
        }
    };

    match state.client_keys.reset_usage(&id, reset_type).await {
        Ok(true) => Ok(Json(SuccessResponse { success: true })),
        Ok(false) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Key not found".into(),
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

// ========================================================================
// Per-key model access
// ========================================================================

/// Get allowed models for a key
#[utoipa::path(
    get,
    path = "/keys/{id}/models",
    tag = "keys",
    params(("id" = String, Path, description = "Key ID")),
    responses(
        (status = 200, body = KeyModelsResponse),
    )
)]
pub async fn get_key_models(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<KeyModelsResponse>, (StatusCode, Json<ErrorResponse>)> {
    let models = state
        .client_keys
        .get_allowed_models(&id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;
    let allow_all = models.is_empty();
    Ok(Json(KeyModelsResponse { allow_all, models }))
}

/// Set allowed models for a key (empty = allow all)
#[utoipa::path(
    put,
    path = "/keys/{id}/models",
    tag = "keys",
    params(("id" = String, Path, description = "Key ID")),
    request_body = SetKeyModelsRequest,
    responses(
        (status = 200, body = SuccessResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn set_key_models(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<SetKeyModelsRequest>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.client_keys.set_allowed_models(&id, body.models).await {
        Ok(_) => Ok(Json(SuccessResponse { success: true })),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

// ========================================================================
// Per-key per-model usage
// ========================================================================

/// Get per-model usage for a key
#[utoipa::path(
    get,
    path = "/keys/{id}/model-usage",
    tag = "keys",
    params(("id" = String, Path, description = "Key ID")),
    responses(
        (status = 200, body = KeyModelUsageResponse),
    )
)]
pub async fn get_key_model_usage(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<KeyModelUsageResponse>, (StatusCode, Json<ErrorResponse>)> {
    let entries = state.client_keys.get_model_usage(&id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;
    Ok(Json(KeyModelUsageResponse { entries }))
}

/// Set per-model limits for a key
#[utoipa::path(
    put,
    path = "/keys/{id}/model-usage/{model}/limits",
    tag = "keys",
    params(
        ("id" = String, Path, description = "Key ID"),
        ("model" = String, Path, description = "Model ID"),
    ),
    request_body = UpdateLimitsRequest,
    responses(
        (status = 200, body = SuccessResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn set_key_model_limits(
    State(state): State<Arc<AppState>>,
    Path((id, model)): Path<(String, String)>,
    Json(body): Json<UpdateLimitsRequest>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    let limits = TokenLimits {
        five_hour_limit: body.five_hour_limit,
        weekly_limit: body.weekly_limit,
        total_limit: body.total_limit,
    };

    match state
        .client_keys
        .set_model_limits(&id, &model, limits)
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

/// Remove per-model limits for a key
#[utoipa::path(
    delete,
    path = "/keys/{id}/model-usage/{model}/limits",
    tag = "keys",
    params(
        ("id" = String, Path, description = "Key ID"),
        ("model" = String, Path, description = "Model ID"),
    ),
    responses(
        (status = 200, body = SuccessResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn remove_key_model_limits(
    State(state): State<Arc<AppState>>,
    Path((id, model)): Path<(String, String)>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.client_keys.remove_model_limits(&id, &model).await {
        Ok(true) => Ok(Json(SuccessResponse { success: true })),
        Ok(false) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Model usage entry not found".into(),
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

/// Reset per-model usage counters for a key
#[utoipa::path(
    post,
    path = "/keys/{id}/model-usage/{model}/reset",
    tag = "keys",
    params(
        ("id" = String, Path, description = "Key ID"),
        ("model" = String, Path, description = "Model ID"),
    ),
    request_body = ResetUsageRequest,
    responses(
        (status = 200, body = SuccessResponse),
        (status = 400, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
pub async fn reset_key_model_usage(
    State(state): State<Arc<AppState>>,
    Path((id, model)): Path<(String, String)>,
    Json(body): Json<ResetUsageRequest>,
) -> Result<Json<SuccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    let reset_type = match body.reset_type.to_lowercase().as_str() {
        "fivehour" | "hourly" => UsageResetType::FiveHour,
        "weekly" => UsageResetType::Weekly,
        "total" => UsageResetType::Total,
        "all" => UsageResetType::All,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "Invalid reset type. Use: fiveHour, weekly, total, or all".into(),
                }),
            ));
        }
    };

    match state
        .client_keys
        .reset_model_usage(&id, &model, reset_type)
        .await
    {
        Ok(true) => Ok(Json(SuccessResponse { success: true })),
        Ok(false) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Model usage entry not found".into(),
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

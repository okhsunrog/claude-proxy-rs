use super::{ErrorResponse, SuccessResponse};
use crate::{
    AppState,
    auth::chatgpt::{AvailableModel, ConnectionStatus, DeviceLogin},
};
use axum::{Json, extract::State, http::StatusCode};
use std::sync::Arc;
type AdminResult<T> = Result<Json<T>, (StatusCode, Json<ErrorResponse>)>;
fn failure(error: String) -> (StatusCode, Json<ErrorResponse>) {
    (StatusCode::BAD_REQUEST, Json(ErrorResponse { error }))
}
#[utoipa::path(get, path = "/chatgpt/models", tag = "chatgpt", responses((status = 200, body = Vec<AvailableModel>), (status = 400, body = ErrorResponse)))]
pub async fn chatgpt_models(
    State(state): State<Arc<AppState>>,
) -> AdminResult<Vec<AvailableModel>> {
    state
        .chatgpt
        .available_models()
        .await
        .map(Json)
        .map_err(failure)
}
#[utoipa::path(get, path = "/chatgpt/status", tag = "chatgpt", responses((status = 200, body = ConnectionStatus), (status = 400, body = ErrorResponse)))]
pub async fn chatgpt_status(State(state): State<Arc<AppState>>) -> AdminResult<ConnectionStatus> {
    state.chatgpt.status().await.map(Json).map_err(failure)
}
#[utoipa::path(post, path = "/chatgpt/login", tag = "chatgpt", responses((status = 200, body = DeviceLogin), (status = 400, body = ErrorResponse)))]
pub async fn chatgpt_login(State(state): State<Arc<AppState>>) -> AdminResult<DeviceLogin> {
    state.chatgpt.start().await.map(Json).map_err(failure)
}
#[utoipa::path(post, path = "/chatgpt/poll", tag = "chatgpt", responses((status = 200, body = ConnectionStatus), (status = 400, body = ErrorResponse)))]
pub async fn chatgpt_poll(State(state): State<Arc<AppState>>) -> AdminResult<ConnectionStatus> {
    state.chatgpt.poll().await.map(Json).map_err(failure)
}
#[utoipa::path(delete, path = "/chatgpt", tag = "chatgpt", responses((status = 200, body = SuccessResponse), (status = 400, body = ErrorResponse)))]
pub async fn chatgpt_logout(State(state): State<Arc<AppState>>) -> AdminResult<SuccessResponse> {
    state
        .chatgpt
        .logout()
        .await
        .map(|()| Json(SuccessResponse { success: true }))
        .map_err(failure)
}

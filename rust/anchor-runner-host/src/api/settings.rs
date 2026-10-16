use super::*;
use axum::routing::delete;

pub(super) fn router() -> Router<ApiState> {
    Router::new()
        .route("/settings/model", get(model).put(save_model))
        .route("/settings/model/test", post(test_model))
        .route(
            "/settings/model/providers/{slot}",
            axum::routing::put(save_provider),
        )
        .route("/settings/model/providers/{slot}/test", post(test_provider))
        .route(
            "/settings/model/credentials/{provider}",
            delete(clear_model_key).put(set_model_key),
        )
        .route("/settings/runtime", get(runtime))
        .route("/settings/runtime/prepare", post(prepare_runtime))
        .route("/settings/runtime/cancel", post(cancel_runtime))
}
pub(super) fn is_private_path(path: &str) -> bool {
    path.starts_with("/settings/")
}
#[allow(clippy::result_large_err)] // Shared HTTP adapter returns Axum responses directly.
async fn model(State(state): State<ApiState>) -> Result<Json<Value>, HttpResponse> {
    blocking(move || {
        state
            .model_configuration
            .status()
            .map(Json)
            .map_err(|reason| error(StatusCode::CONFLICT, reason))
    })
    .await
}
#[allow(clippy::result_large_err)]
async fn save_model(
    State(state): State<ApiState>,
    Json(body): Json<crate::model_configuration::ModelUpdate>,
) -> Result<Json<Value>, HttpResponse> {
    blocking(move || {
        state
            .model_configuration
            .save(body)
            .map(Json)
            .map_err(|reason| error(StatusCode::BAD_REQUEST, reason))
    })
    .await
}
async fn test_model(
    State(state): State<ApiState>,
    Json(body): Json<crate::model_configuration::ModelUpdate>,
) -> Result<Json<Value>, HttpResponse> {
    state
        .model_configuration
        .test(body)
        .await
        .map(Json)
        .map_err(|reason| error(StatusCode::BAD_REQUEST, reason))
}
#[allow(clippy::result_large_err)]
async fn save_provider(
    State(state): State<ApiState>,
    AxumPath(slot): AxumPath<String>,
    Json(body): Json<crate::model_configuration::ProviderUpdate>,
) -> Result<Json<Value>, HttpResponse> {
    blocking(move || {
        state
            .model_configuration
            .save_provider(&slot, body)
            .map(Json)
            .map_err(|reason| error(StatusCode::BAD_REQUEST, reason))
    })
    .await
}
async fn test_provider(
    State(state): State<ApiState>,
    AxumPath(slot): AxumPath<String>,
    Json(body): Json<crate::model_configuration::ProviderTest>,
) -> Result<Json<Value>, HttpResponse> {
    state
        .model_configuration
        .test_provider(&slot, body)
        .await
        .map(Json)
        .map_err(|reason| error(StatusCode::BAD_REQUEST, reason))
}
#[allow(clippy::result_large_err)]
async fn clear_model_key(
    State(state): State<ApiState>,
    AxumPath(provider): AxumPath<String>,
) -> Result<Json<Value>, HttpResponse> {
    blocking(move || {
        state
            .model_configuration
            .clear_provider_credential(&provider)
            .map(Json)
            .map_err(|reason| error(StatusCode::BAD_REQUEST, reason))
    })
    .await
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderKey {
    value: String,
}
#[allow(clippy::result_large_err)]
async fn set_model_key(
    State(state): State<ApiState>,
    AxumPath(provider): AxumPath<String>,
    Json(body): Json<ProviderKey>,
) -> Result<Json<Value>, HttpResponse> {
    blocking(move || {
        state
            .model_configuration
            .set_provider_credential(&provider, body.value)
            .map(Json)
            .map_err(|reason| error(StatusCode::BAD_REQUEST, reason))
    })
    .await
}
#[allow(clippy::result_large_err)]
async fn runtime(State(state): State<ApiState>) -> Result<Json<Value>, HttpResponse> {
    blocking(move || {
        state
            .runtime_management
            .observed_status()
            .map(Json)
            .map_err(|reason| error(StatusCode::CONFLICT, reason))
    })
    .await
}
async fn prepare_runtime(
    State(state): State<ApiState>,
    Json(body): Json<crate::runtime_management::RuntimePrepare>,
) -> Result<Json<Value>, HttpResponse> {
    state
        .runtime_management
        .start(body)
        .map(Json)
        .map_err(|reason| error(StatusCode::BAD_REQUEST, reason))
}
async fn cancel_runtime(State(state): State<ApiState>) -> Result<Json<Value>, HttpResponse> {
    state
        .runtime_management
        .cancel()
        .map(Json)
        .map_err(|reason| error(StatusCode::CONFLICT, reason))
}

#[cfg(test)]
mod tests;

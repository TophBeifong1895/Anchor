//! Operator adapters for current Library configuration; no Pilot Turn is created.
use super::*;
mod lifecycle;
#[cfg(test)]
mod tests;

pub(super) fn router() -> Router<ApiState> {
    Router::new()
        .route("/plugins/{plugin}/credentials", get(credentials))
        .route(
            "/plugins/{plugin}/credentials/{target}/{field}",
            axum::routing::put(configure).delete(clear),
        )
        .layer(RequestBodyLimitLayer::new(2 * 1024 * 1024))
        .merge(lifecycle::router())
        .layer(middleware::from_fn(private_responses))
}

pub(super) fn is_private_path(path: &str) -> bool {
    let parts = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
    parts.first() == Some(&"plugins")
        && (parts.get(2) == Some(&"credentials")
            || parts.get(1) == Some(&"install-preflight")
            || parts.get(2) == Some(&"uninstall-preflight")
            || parts.get(1) == Some(&"preflights"))
}

async fn private_responses(request: axum::extract::Request, next: Next) -> HttpResponse {
    credential_requests::secure_private_response(next.run(request).await)
}

#[allow(clippy::result_large_err)] // Shared HTTP adapter returns Axum responses directly.
async fn credentials(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AxumPath(plugin): AxumPath<String>,
) -> Result<Json<Value>, HttpResponse> {
    blocking(move || {
        crate::credential_store::credentials(
            &state.data_root,
            &crate::library_root(&state.catalog_root),
            &private_owner(&state, &headers),
            &plugin,
        )
        .map(Json)
        .map_err(|reason| error(StatusCode::CONFLICT, reason))
    })
    .await
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SetCredential {
    authorization_digest: String,
    value: String,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ClearCredential {
    authorization_digest: String,
}

async fn configure(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AxumPath((plugin, target, field)): AxumPath<(String, String, String)>,
    Json(body): Json<SetCredential>,
) -> Result<Json<Value>, HttpResponse> {
    mutate(
        state,
        headers,
        plugin,
        target,
        field,
        body.authorization_digest,
        Some(body.value),
    )
    .await
}
async fn clear(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AxumPath((plugin, target, field)): AxumPath<(String, String, String)>,
    Json(body): Json<ClearCredential>,
) -> Result<Json<Value>, HttpResponse> {
    mutate(
        state,
        headers,
        plugin,
        target,
        field,
        body.authorization_digest,
        None,
    )
    .await
}
#[allow(clippy::result_large_err)]
async fn mutate(
    state: ApiState,
    headers: HeaderMap,
    plugin: String,
    target: String,
    field: String,
    digest: String,
    value: Option<String>,
) -> Result<Json<Value>, HttpResponse> {
    let _catalog = state.application.graph_catalog_mutation_guard().await;
    let _reference = state
        .application
        .plugin_reference_lease()
        .await
        .map_err(application_error)?;
    blocking(move || {
        crate::credential_store::configure(
            &state.data_root,
            &crate::library_root(&state.catalog_root),
            &private_owner(&state, &headers),
            &plugin,
            &target,
            &field,
            &digest,
            value.as_deref(),
            &store(&state)?,
        )
        .map(Json)
        .map_err(|reason| error(StatusCode::CONFLICT, reason))
    })
    .await
}

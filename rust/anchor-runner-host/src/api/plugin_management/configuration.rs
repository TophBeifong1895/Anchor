//! HTTP adaptation of the same owner-bound Plugin configuration use case.
use super::*;

#[cfg(test)]
mod tests;

pub(super) fn router() -> Router<ApiState> {
    Router::new()
        .route("/plugins/{plugin}/configuration", get(read).put(write))
        .route("/plugins/{plugin}/configuration/test", post(test))
}
#[allow(clippy::result_large_err)]
async fn read(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AxumPath(plugin): AxumPath<String>,
) -> Result<Json<Value>, HttpResponse> {
    blocking(move || {
        crate::plugin_configuration::configuration(
            &state.data_root,
            state.application.library_root(),
            &private_owner(&state, &headers),
            &plugin,
        )
        .map(Json)
        .map_err(|reason| error(StatusCode::CONFLICT, reason))
    })
    .await
}
#[allow(clippy::result_large_err)]
async fn write(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AxumPath(plugin): AxumPath<String>,
    Json(update): Json<crate::plugin_configuration::ConfigurationUpdate>,
) -> Result<Json<Value>, HttpResponse> {
    let _catalog = state.application.graph_catalog_mutation_guard().await;
    let _reference = state
        .application
        .plugin_reference_lease()
        .await
        .map_err(application_error)?;
    blocking(move || {
        crate::plugin_configuration::configure(
            &state.data_root,
            state.application.library_root(),
            &private_owner(&state, &headers),
            &plugin,
            update,
            &store(&state)?,
        )
        .map(Json)
        .map_err(|reason| error(StatusCode::CONFLICT, reason))
    })
    .await
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TestConfiguration {
    authorization_digest: String,
    revision: u64,
}
async fn test(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AxumPath(plugin): AxumPath<String>,
    Json(body): Json<TestConfiguration>,
) -> Result<Json<Value>, HttpResponse> {
    // Preparation owns the publication lease. Holding the management reference
    // lease here would recursively acquire the same Library lock and deadlock.
    crate::plugin_configuration_test::test(
        state.application.bwrap.clone(),
        state.data_root.clone(),
        state.application.library_root().to_path_buf(),
        crate::credential_store::binding_owner(&private_owner(&state, &headers)),
        plugin,
        body.authorization_digest,
        body.revision,
    )
    .await
    .map(Json)
    .map_err(|reason| error(StatusCode::CONFLICT, reason))
}

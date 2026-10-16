//! HTTP management uses the same owner-scoped application operations as Pilot.
use super::*;
use crate::application::channel_management::ChannelBinding;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct EnabledRequest {
    enabled: bool,
    expected_binding: ChannelBinding,
}

fn channel_error(failure: ApplicationError) -> HttpResponse {
    match failure {
        ApplicationError::Missing => error(
            StatusCode::NOT_FOUND,
            "channel binding, Graph, or Plugin is unavailable",
        ),
        failure => application_error(failure),
    }
}

pub(super) async fn status(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AxumPath(platform): AxumPath<String>,
) -> Result<Json<Value>, HttpResponse> {
    state
        .application
        .channel_status(&private_owner(&state, &headers), &platform)
        .await
        .map(Json)
        .map_err(channel_error)
}

pub(super) async fn configure(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AxumPath(platform): AxumPath<String>,
    body: axum::body::Bytes,
) -> Result<Json<Value>, HttpResponse> {
    let binding = serde_json::from_slice::<ChannelBinding>(&body)
        .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid channel binding request"))?;
    state
        .application
        .configure_channel(&private_owner(&state, &headers), &platform, binding)
        .await
        .map(Json)
        .map_err(channel_error)
}

pub(super) async fn set_enabled(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AxumPath(platform): AxumPath<String>,
    body: axum::body::Bytes,
) -> Result<Json<Value>, HttpResponse> {
    let request = serde_json::from_slice::<EnabledRequest>(&body)
        .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid channel enabled request"))?;
    state
        .application
        .set_channel_enabled_checked(
            &private_owner(&state, &headers),
            &platform,
            request.enabled,
            &request.expected_binding,
        )
        .await
        .map(Json)
        .map_err(channel_error)
}

#[cfg(test)]
mod tests;

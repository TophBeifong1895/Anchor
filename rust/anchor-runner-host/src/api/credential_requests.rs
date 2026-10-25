use super::*;

pub(super) fn router() -> Router<ApiState> {
    Router::new()
        .route("/sessions/{session}/credential-requests", get(list))
        .route(
            "/sessions/{session}/credential-requests/{request}/value",
            post(submit),
        )
        .route(
            "/sessions/{session}/credential-requests/{request}/cancel",
            post(cancel),
        )
}

pub(super) fn is_private_path(path: &str) -> bool {
    let segments = path.split('/').collect::<Vec<_>>();
    segments.len() >= 4 && segments[1] == "sessions" && segments[3] == "credential-requests"
}

pub(super) fn secure_private_response(mut response: HttpResponse) -> HttpResponse {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        "referrer-policy",
        axum::http::HeaderValue::from_static("no-referrer"),
    );
    response
}

pub(super) async fn list(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AxumPath(session): AxumPath<String>,
) -> Result<Json<Value>, HttpResponse> {
    #[allow(clippy::result_large_err)]
    blocking(move || {
        let (sessions, owner, session) = owned_session(&state, &headers, &session)?;
        let requests = crate::credential_store::list(&state.data_root, &sessions, &owner, &session)
            .map_err(|_| error(StatusCode::CONFLICT, "credential request state unavailable"))?;
        Ok(Json(json!({"requests":requests})))
    })
    .await
}
pub(super) async fn submit(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AxumPath((session, request)): AxumPath<(String, String)>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, HttpResponse> {
    let value = body
        .as_object()
        .filter(|fields| fields.len() == 1)
        .and_then(|fields| fields.get("value"))
        .and_then(Value::as_str)
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "provide credential value only"))?
        .to_owned();
    let lookup = state.clone();
    let lookup_headers = headers.clone();
    let lookup_session = session.clone();
    #[allow(clippy::result_large_err)]
    let (_, owner, current) =
        blocking(move || owned_session(&lookup, &lookup_headers, &lookup_session)).await?;
    let _control = state
        .pilots
        .credential_submission_guard(&crate::pilot_host::scope(
            &state.data_root,
            &owner,
            &current,
        ))
        .await
        .map_err(session_error)?;
    let _catalog = state.application.graph_catalog_mutation_guard().await;
    let _lease = state
        .application
        .plugin_reference_lease()
        .await
        .map_err(application_error)?;
    // A bot's credentials follow the bot's rules: re-checked at submission
    // because the bot may have been enabled since the form was requested.
    if let Some((plugin, graph)) =
        crate::credential_store::request_channel_graph(&state.data_root, &owner, &current, &request)
            .map_err(|reason| error(StatusCode::CONFLICT, reason))?
    {
        let plan = state
            .application
            .check_channel_credentials_mutable(&owner, &graph)
            .map_err(application_error)?;
        if plan.plugin != plugin {
            return Err(error(
                StatusCode::CONFLICT,
                "this Graph's bot no longer uses that Plugin; request a fresh credential form",
            ));
        }
    }
    #[allow(clippy::result_large_err)]
    blocking(move || {
        let (sessions, owner, session) = owned_session(&state, &headers, &session)?;
        let request = crate::credential_store::submit(
            &state.data_root,
            state.application.library_root(),
            &sessions,
            &owner,
            &session,
            &request,
            &value,
        )
        .map_err(|reason| error(StatusCode::CONFLICT, reason))?;
        Ok(Json(crate::credential_store::public_result(&request)))
    })
    .await
}
pub(super) async fn cancel(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AxumPath((session, request)): AxumPath<(String, String)>,
) -> Result<Json<Value>, HttpResponse> {
    #[allow(clippy::result_large_err)]
    blocking(move || {
        let (_, owner, session) = owned_session(&state, &headers, &session)?;
        let request =
            crate::credential_store::cancel(&state.data_root, &owner, &session, &request, false)
                .map_err(|reason| error(StatusCode::CONFLICT, reason))?;
        Ok(Json(crate::credential_store::public_result(&request)))
    })
    .await
}

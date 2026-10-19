use super::*;
use crate::application::run_access::http::{RunHttpError, abandon_response};
use anchor_platform_session::{ChannelInboundAdmission, TurnStatus};
#[cfg(test)]
mod tests;

fn channel_attachment_manifest_matches(
    inbound: &ChannelInboundAdmission,
    actual: &[crate::channel_inputs::AttachmentManifest],
) -> bool {
    inbound.request.attachments.files.len() == actual.len()
        && inbound
            .request
            .attachments
            .files
            .iter()
            .zip(actual)
            .all(|(expected, actual)| {
                expected.name == actual.name
                    && expected.sha256 == actual.sha256
                    && expected.size == actual.size
                    && expected.media_type == actual.media_type
            })
}

async fn resolve_channel_source(
    state: &ApiState,
    headers: &HeaderMap,
    request: &crate::application::ConversationAdmission,
    attachments: &crate::channel_inputs::PreparedAttachments,
) -> Result<Option<crate::application::ChannelRunSource>, HttpResponse> {
    let Some(inbound_id) = request.channel_inbound.clone() else {
        return Ok(None);
    };
    let owner = private_owner(state, headers);
    let session = request.session.clone();
    let graph = request.graph.clone();
    let reply_node = request.reply_node.clone();
    let previous_run = request.previous_run.clone();
    let input_message = request
        .input
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let attachment_manifest = attachments.manifest();
    let run_id = request.run.clone();
    let lookup = state.clone();
    blocking(move || {
        let sessions = store(&lookup)?;
        let inbound = sessions
            .get_channel_inbound(&owner, &session, &inbound_id)
            .map_err(session_error)?;
        if inbound.relation.superseded_by_turn_id.is_some() {
            return Err(error(
                StatusCode::CONFLICT,
                "channel inbound was superseded by a newer Turn",
            ));
        }
        if inbound.request.graph != graph || inbound.request.reply_node != reply_node {
            return Err(error(
                StatusCode::CONFLICT,
                "channel inbound Graph or reply node differs from the Run request",
            ));
        }
        if inbound.request.text != input_message {
            return Err(error(
                StatusCode::CONFLICT,
                "channel inbound text differs from the Run input",
            ));
        }
        if !channel_attachment_manifest_matches(&inbound, &attachment_manifest) {
            return Err(error(
                StatusCode::CONFLICT,
                "channel inbound attachment manifest differs from the Run request",
            ));
        }
        if inbound.previous_run != previous_run {
            return Err(error(
                StatusCode::CONFLICT,
                "previous_run does not name the previous channel Run",
            ));
        }
        if inbound
            .relation
            .run_id
            .as_deref()
            .is_some_and(|run| run != run_id)
        {
            return Err(error(
                StatusCode::CONFLICT,
                "channel inbound is already bound to another Run",
            ));
        }
        if inbound.relation.run_id.is_none() && inbound.turn.status != TurnStatus::Running {
            return Err(error(
                StatusCode::CONFLICT,
                "channel Turn is no longer available for Graph admission",
            ));
        }
        Ok(Some(crate::application::ChannelRunSource {
            owner,
            session,
            inbound: inbound_id,
            turn: inbound.relation.turn_id,
        }))
    })
    .await
}

async fn settle_channel_admission_failure(
    state: &ApiState,
    source: &crate::application::ChannelRunSource,
) -> Result<(), HttpResponse> {
    let source = source.clone();
    let state = state.clone();
    blocking(move || {
        let sessions = store(&state)?;
        let turn = sessions
            .get_turn(&source.owner, &source.session, &source.turn)
            .map_err(session_error)?;
        if turn.status == TurnStatus::Running {
            sessions
                .finish_turn(
                    &source.owner,
                    &source.session,
                    &source.turn,
                    TurnStatus::Failed,
                    Some("Graph admission failed"),
                )
                .map_err(session_error)?;
        }
        Ok(())
    })
    .await
}

pub(super) async fn conversation_run(
    State(state): State<ApiState>,
    headers: HeaderMap,
    body: Result<
        Json<crate::application::ConversationAdmission>,
        axum::extract::rejection::JsonRejection,
    >,
) -> Result<(StatusCode, Json<Value>), HttpResponse> {
    let Json(request) = body.map_err(|rejection| {
        let status = if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            StatusCode::PAYLOAD_TOO_LARGE
        } else {
            StatusCode::BAD_REQUEST
        };
        error(status, "invalid conversation Run request")
    })?;
    request
        .validate()
        .map_err(|message| error(StatusCode::BAD_REQUEST, message))?;
    let attachments = crate::channel_inputs::prepare(&request.attachments)
        .map_err(|message| error(StatusCode::BAD_REQUEST, message))?;
    let channel = resolve_channel_source(&state, &headers, &request, &attachments).await?;
    let oauth_owner = super::oauth::binding_owner(&private_owner(&state, &headers));
    let path = graph_path(&state, &request.graph)?;
    let graph = request.graph.clone();
    let session = request.session.clone();
    let run = if let Some(run) = state
        .application
        .retry_conversation_admission(&request, &path, &attachments, channel.as_ref())
        .await
        .map_err(application_error)?
    {
        run
    } else {
        let lease = state
            .application
            .acquire_graph_lease_waiting(&path)
            .await
            .map_err(application_error)?;
        let (path, bundle) = load_graph_definition(&state, &graph)?;
        let admitted = state
            .application
            .admit_conversation(
                request,
                &path,
                bundle,
                lease,
                &attachments,
                channel.clone(),
                oauth_owner,
            )
            .await;
        match admitted {
            Ok(run) => run,
            Err(failure) => {
                if let Some(channel) = channel.as_ref() {
                    settle_channel_admission_failure(&state, channel).await?;
                }
                return Err(application_error(failure));
            }
        }
    };
    if let Some(channel) = channel {
        let state_for_association = state.clone();
        let run_for_association = run.clone();
        blocking(move || {
            store(&state_for_association)?
                .associate_channel_run(
                    &channel.owner,
                    &channel.session,
                    &channel.inbound,
                    &run_for_association,
                )
                .map_err(session_error)?;
            Ok(())
        })
        .await?;
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"run":run,"graph":graph,"session":session})),
    ))
}

pub(super) async fn trigger(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), HttpResponse> {
    let owner = super::oauth::binding_owner(&private_owner(&state, &headers));
    trigger_as_owner(state, body, owner).await
}

pub(super) async fn trigger_as_owner(
    state: ApiState,
    body: Value,
    oauth_owner: String,
) -> Result<(StatusCode, Json<Value>), HttpResponse> {
    let requested = body
        .get("graph")
        .and_then(Value::as_str)
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "graph is required"))?;
    let input = body.get("input").cloned().unwrap_or(Value::Null);
    if !input.is_null() && !input.is_object() {
        return Err(error(StatusCode::BAD_REQUEST, "input must be an object"));
    }
    let objective = match body.get("objective") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(_) => return Err(error(StatusCode::BAD_REQUEST, "objective must be a string")),
    };
    let trigger = match body.get("trigger") {
        None | Some(Value::Null) => crate::application::RunTrigger::default(),
        Some(value) => serde_json::from_value::<crate::application::RunTrigger>(value.clone())
            .map_err(|message| error(StatusCode::BAD_REQUEST, message.to_string()))?,
    };
    trigger
        .validate()
        .map_err(|message| error(StatusCode::BAD_REQUEST, message))?;
    let path = graph_path(&state, requested)?;
    let graph_lease = state
        .application
        .graph_admission_lease(&path)
        .map_err(application_error)?;
    let (path, bundle) = load_graph_definition(&state, requested)?;
    let run_id = state
        .application
        .admit(
            requested.to_owned(),
            &path,
            bundle,
            input,
            crate::application::AdmissionOptions {
                objective,
                trigger,
                pilot: None,
                oauth_owner: Some(oauth_owner),
            },
            graph_lease,
        )
        .await
        .map_err(application_error)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"run":run_id,"graph":requested})),
    ))
}

pub(super) async fn list_runs(State(state): State<ApiState>) -> Result<Json<Value>, HttpResponse> {
    Ok(Json(json!({"runs":projected_runs(&state).await?})))
}

#[allow(clippy::result_large_err)]
pub(super) async fn projected_runs(state: &ApiState) -> Result<Vec<Value>, HttpResponse> {
    state
        .application
        .projected_runs()
        .await
        .map_err(application_error)
}

pub(super) async fn get_run(
    State(state): State<ApiState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Value>, HttpResponse> {
    if id.is_empty() || id.contains('/') || id == "." || id == ".." {
        return Err(error(StatusCode::BAD_REQUEST, "invalid run id"));
    }
    state
        .application
        .http_run_detail(&id, &state.workspace_root)
        .await
        .map(Json)
        .map_err(run_http_error)
}

fn run_http_error(failure: RunHttpError) -> HttpResponse {
    match failure {
        RunHttpError::Application(failure) => application_error(failure),
        RunHttpError::Missing => error(StatusCode::NOT_FOUND, "no such run"),
        RunHttpError::Internal(message) => error(StatusCode::INTERNAL_SERVER_ERROR, message),
        RunHttpError::Unprocessable(message) => error(StatusCode::UNPROCESSABLE_ENTITY, message),
    }
}

#[cfg(test)]
mod http_error_tests {
    use super::*;

    #[tokio::test]
    async fn detail_error_adapter_preserves_http_status_and_message_bytes() {
        for (failure, status, message) in [
            (RunHttpError::Missing, StatusCode::NOT_FOUND, "no such run"),
            (
                RunHttpError::Internal("raw store error".into()),
                StatusCode::INTERNAL_SERVER_ERROR,
                "raw store error",
            ),
            (
                RunHttpError::Unprocessable("raw artifact error".into()),
                StatusCode::UNPROCESSABLE_ENTITY,
                "raw artifact error",
            ),
            (
                RunHttpError::Application(ApplicationError::Missing),
                StatusCode::NOT_FOUND,
                "no such run",
            ),
            (
                RunHttpError::Application(ApplicationError::Conflict("conflict".into())),
                StatusCode::CONFLICT,
                "conflict",
            ),
            (
                RunHttpError::Application(ApplicationError::Invalid("invalid".into())),
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid",
            ),
            (
                RunHttpError::Application(ApplicationError::Storage("storage".into())),
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
            ),
        ] {
            let response = run_http_error(failure);
            assert_eq!(response.status(), status);
            let body = axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap();
            assert_eq!(
                body.as_ref(),
                format!(r#"{{"error":"{message}"}}"#).as_bytes()
            );
        }
    }
}

pub(super) async fn delete_run(
    State(state): State<ApiState>,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode, HttpResponse> {
    if id.is_empty()
        || id == "."
        || id == ".."
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(error(StatusCode::NOT_FOUND, "no such run"));
    }
    state
        .application
        .delete_run(&id, &state.workspace_root)
        .await
        .map_err(application_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RecoveryBody {
    node_id: String,
    invocation: u64,
    attempt_id: i64,
    decision: String,
    #[serde(default)]
    observation: Option<String>,
}

pub(super) async fn recover_run(
    State(state): State<ApiState>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<RecoveryBody>,
) -> Result<(StatusCode, Json<Value>), HttpResponse> {
    let decision = match body.decision.as_str() {
        "retry" if body.observation.is_none() => anchor_runtime::graph::RecoveryDecision::Retry,
        "completed"
            if body
                .observation
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty()) =>
        {
            let observation = body.observation.expect("validated observation");
            anchor_runtime::graph::RecoveryDecision::Completed { observation }
        }
        "abort" if body.observation.is_none() => anchor_runtime::graph::RecoveryDecision::Abort,
        "retry" | "completed" | "abort" => {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "completed requires a non-empty observation; other decisions do not accept one",
            ));
        }
        _ => return Err(error(StatusCode::BAD_REQUEST, "unknown recovery decision")),
    };
    state
        .application
        .recover(
            &id,
            &body.node_id,
            body.invocation,
            body.attempt_id,
            decision,
        )
        .await
        .map_err(application_error)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"run":id,"accepted":true})),
    ))
}

pub(super) async fn control(
    State(state): State<ApiState>,
    AxumPath((id, operation)): AxumPath<(String, String)>,
) -> Result<(StatusCode, Json<Value>), HttpResponse> {
    if !matches!(operation.as_str(), "pause" | "resume" | "stop") {
        return Err(error(StatusCode::BAD_REQUEST, "unknown Run control"));
    }
    state
        .application
        .control(&id, &operation)
        .await
        .map_err(application_error)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"run":id,"asked":operation})),
    ))
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AbandonBody {
    #[serde(default)]
    reason: Option<String>,
}

/// Abandon one whole Run: cancel its execution and make it terminal `Aborted`.
///
/// A body is optional (`{"reason":"plugin_update"}`, default `operator`), and an
/// already abandoned Run answers with the identical body so an operations
/// script can retry safely.
pub(super) async fn abandon_run(
    State(state): State<ApiState>,
    AxumPath(id): AxumPath<String>,
    body: String,
) -> Result<(StatusCode, Json<Value>), HttpResponse> {
    let reason = if body.trim().is_empty() {
        None
    } else {
        let body: AbandonBody = serde_json::from_str(&body)
            .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid abandon body"))?;
        body.reason.filter(|reason| !reason.trim().is_empty())
    };
    let result = state
        .application
        .abandon_run(&id, reason.as_deref())
        .await
        .map_err(application_error)?;
    let code = match result.action {
        crate::application::AbandonAction::Abandoning => StatusCode::ACCEPTED,
        _ => StatusCode::OK,
    };
    Ok((code, Json(abandon_response(&id, &result))))
}

pub(super) async fn execute_session_call(
    State(state): State<ApiState>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, HttpResponse> {
    let session = body
        .get("session")
        .and_then(Value::as_str)
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "session is required"))?;
    let previous = match body.get("previous_run") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.clone()),
        _ => {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "previous_run must be a string",
            ));
        }
    };
    state
        .application
        .execute_session_call(&id, session, previous)
        .await
        .map_err(application_error)?;
    Ok(Json(json!({"accepted":true})))
}

pub(super) async fn yield_session_call(
    State(state): State<ApiState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Value>, HttpResponse> {
    state
        .application
        .yield_session_call(&id)
        .await
        .map_err(application_error)?;
    Ok(Json(json!({"accepted":true})))
}

pub(super) async fn settle_session_call(
    State(state): State<ApiState>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, HttpResponse> {
    let status = body
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "status is required"))?;
    state
        .application
        .settle_session_call(
            &id,
            status,
            body.get("error").and_then(Value::as_str).unwrap_or(""),
        )
        .await
        .map_err(application_error)?;
    Ok(Json(json!({"accepted":true})))
}

use super::*;
use crate::{
    pilot_host::{PilotAdmission, scope},
    pilot_tools::PilotTools,
};
use anchor_platform_session::{Session, SessionStore, TurnStatus};
use axum::response::sse::{Event, KeepAlive, Sse};
use std::{collections::VecDeque, convert::Infallible, time::Duration};

fn pilot_session(session: &Session) -> Result<(), HttpResponse> {
    if !session.graph.is_empty() || !session.channel.is_empty() {
        return Err(error(
            StatusCode::NOT_IMPLEMENTED,
            "native Graph/channel Session Turn admission is not available in this slice",
        ));
    }
    Ok(())
}

pub(super) async fn recover_pilot_turns(state: &ApiState) -> Result<(), String> {
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        let sessions = store(&state).map_err(|_| "Pilot storage startup failed".to_owned())?;
        sessions
            .recover_channel_deliveries()
            .map_err(|_| "Channel delivery startup recovery failed".to_owned())?;
        let mut recoverable_assistants = Vec::new();
        for candidate in state
            .application
            .channel_startup_runs()
            .map_err(|_| "Pilot Run facts unavailable".to_owned())?
        {
            let run = &candidate.run_id;
            state
                .application
                .settle_startup_channel_run(&candidate)
                .map_err(|_| "Channel Turn startup settlement failed".to_owned())?;
            let metadata = state
                .application
                .metadata(run)
                .map_err(|_| "Pilot Run identity unavailable".to_owned())?;
            if let Some(metadata) = &metadata
                && candidate.preserves_assistant(metadata)
                && let Some(source) = &metadata.assistant
                && sessions
                    .get_channel_assistant(&source.owner, &source.session)
                    .map_err(|_| "Assistant startup identity unavailable".to_owned())?
                    .is_some_and(|current| {
                        current.run_id == *run
                            && current.wait_node == source.wait_node
                            && current.work_node == source.work_node
                            && current.reply_node == source.reply_node
                    })
            {
                recoverable_assistants.push(run.clone());
            }
            if let Some(source) = metadata
                .as_ref()
                .and_then(|metadata| metadata.pilot.clone())
            {
                match sessions.associate_run(&source.owner, &source.session, &source.turn, run) {
                    Ok(_) | Err(anchor_platform_session::SessionError::Missing) => {}
                    Err(_) => return Err("Pilot Run association recovery failed".into()),
                }
            }
        }
        sessions
            .interrupt_running_except_assistants(&recoverable_assistants)
            .map_err(|_| "Pilot startup recovery failed".to_owned())?;
        Ok(())
    })
    .await
    .map_err(|_| "Pilot startup recovery failed".to_owned())?
}

pub(super) async fn list_pilot_turns(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AxumPath(identifier): AxumPath<String>,
) -> Result<Json<Value>, HttpResponse> {
    blocking(move || {
        let (sessions, owner, session) = owned_session(&state, &headers, &identifier)?;
        pilot_session(&session)?;
        let turns = sessions
            .list_turns(&owner, &identifier)
            .map_err(session_error)?;
        let turns = turns
            .iter()
            .map(|turn| crate::pilot_attachments::project_turn(&sessions, &owner, turn))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| {
                error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Pilot attachment references unavailable",
                )
            })?;
        Ok(Json(json!({"turns":turns})))
    })
    .await
}

pub(super) async fn get_pilot_turn(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AxumPath((identifier, turn)): AxumPath<(String, String)>,
) -> Result<Json<Value>, HttpResponse> {
    blocking(move || {
        let (sessions, owner, session) = owned_session(&state, &headers, &identifier)?;
        pilot_session(&session)?;
        let turn = sessions
            .get_turn(&owner, &identifier, &turn)
            .map_err(session_error)?;
        let continuation =
            crate::pilot_host::continuation_status(&sessions, &owner, &identifier, &turn.id)
                .map_err(session_error)?;
        let turn =
            crate::pilot_attachments::project_turn(&sessions, &owner, &turn).map_err(|_| {
                error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Pilot attachment references unavailable",
                )
            })?;
        Ok(Json(json!({"turn":turn,"continuation":continuation})))
    })
    .await
}

pub(super) async fn list_pilot_questions(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AxumPath((identifier, turn)): AxumPath<(String, String)>,
) -> Result<Json<Value>, HttpResponse> {
    blocking(move || {
        let (sessions, owner, session) = owned_session(&state, &headers, &identifier)?;
        pilot_session(&session)?;
        let questions = sessions
            .list_questions(&owner, &identifier, &turn)
            .map_err(session_error)?;
        Ok(Json(json!({"questions":questions})))
    })
    .await
}

pub(super) async fn answer_pilot_question(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AxumPath((identifier, turn, question)): AxumPath<(String, String, String)>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, HttpResponse> {
    blocking(move || {
        let (sessions, owner, session) = owned_session(&state, &headers, &identifier)?;
        pilot_session(&session)?;
        let answer = serde_json::from_value(body)
            .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid question answer"))?;
        let question = sessions
            .answer_question(&owner, &identifier, &turn, &question, answer)
            .map_err(session_error)?;
        Ok(Json(json!({"question":question})))
    })
    .await
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateTurnBody {
    request_id: String,
    message: Option<String>,
    #[serde(default)]
    resume: bool,
    #[serde(default)]
    attachments: Vec<crate::attachments::UploadedAttachment>,
}

pub(super) async fn create_pilot_turn(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AxumPath(identifier): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), HttpResponse> {
    let lookup = state.clone();
    let (sessions, owner, session) =
        blocking(move || owned_session(&lookup, &headers, &identifier)).await?;
    pilot_session(&session)?;
    let body: CreateTurnBody = serde_json::from_value(body).map_err(|_| {
        error(
            StatusCode::BAD_REQUEST,
            "provide request_id with message and/or attachments, or resume:true",
        )
    })?;
    if (body.resume && (body.message.is_some() || !body.attachments.is_empty()))
        || (!body.resume && body.message.is_none() && body.attachments.is_empty())
    {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "provide message and/or attachments, or resume:true without attachments",
        ));
    }
    let attachments = crate::attachments::prepare(&body.attachments)
        .map_err(|reason| error(StatusCode::BAD_REQUEST, reason))?;
    if body.request_id.trim().is_empty()
        || body.request_id.len() > 128
        || body
            .message
            .as_ref()
            .is_some_and(|message| message.trim().is_empty() || message.len() > 64 * 1024)
    {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "request_id or message is empty or too long",
        ));
    }
    let root = scope(&state.data_root, &owner, &session);
    let tools = PilotTools {
        application: state.application.clone(),
        graph_name: state.graph_name.clone(),
        library_root: state.application.library_root().to_path_buf(),
        data_root: state.data_root.clone(),
        workspace_root: state.workspace_root.clone(),
        sessions: sessions.clone(),
        owner: owner.clone(),
        session: session.id.clone(),
        turn: String::new(),
        context: None,
    };
    let projection_sessions = sessions.clone();
    let projection_owner = owner.clone();
    let turn = state
        .pilots
        .admit(PilotAdmission {
            sessions,
            owner,
            session,
            request_id: body.request_id,
            prompt: body.message,
            attachments,
            input_source: None,
            root,
            tools,
        })
        .await
        .map_err(|failure| match failure {
            crate::pilot_host::AdmissionError::Session(failure) => session_error(failure),
            crate::pilot_host::AdmissionError::Provider => error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Pilot provider is not configured or unavailable",
            ),
        })?;
    let turn =
        crate::pilot_attachments::project_turn(&projection_sessions, &projection_owner, &turn)
            .map_err(|_| {
                error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Pilot attachment references unavailable",
                )
            })?;
    Ok((StatusCode::ACCEPTED, Json(json!({"turn":turn}))))
}

pub(super) async fn pilot_messages(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AxumPath(identifier): AxumPath<String>,
) -> Result<Json<Value>, HttpResponse> {
    blocking(move || {
        let (sessions, owner, session) = owned_session(&state, &headers, &identifier)?;
        pilot_session(&session)?;
        let messages = crate::goose_acp::pilot::messages(&sessions, &owner, &identifier);
        let messages = messages.map_err(|_| {
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Pilot history is unavailable",
            )
        })?;
        let messages = messages
            .into_iter()
            .filter(|message| {
                matches!(message["role"].as_str(), Some("user" | "assistant"))
                    && message["text"].as_str().is_some_and(|text| {
                        !text.is_empty()
                            || message["attachments"]
                                .as_array()
                                .is_some_and(|items| !items.is_empty())
                    })
            })
            .collect::<Vec<_>>();
        Ok(Json(json!({"messages":messages})))
    })
    .await
}

pub(super) async fn stop_pilot(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AxumPath(identifier): AxumPath<String>,
) -> Result<(StatusCode, Json<Value>), HttpResponse> {
    let lookup = state.clone();
    let (sessions, owner, session) =
        blocking(move || owned_session(&lookup, &headers, &identifier)).await?;
    pilot_session(&session)?;
    let stopping = state
        .pilots
        .stop_session(sessions, state.data_root.clone(), owner, session.clone())
        .await
        .map_err(session_error)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"session":session,"stopping":stopping})),
    ))
}

#[derive(serde::Deserialize, Default)]
pub(super) struct TurnEventsQuery {
    after: Option<u64>,
}

struct Cursor {
    sessions: SessionStore,
    owner: String,
    session: String,
    turn: String,
    after: u64,
    pending: VecDeque<Event>,
    ended: bool,
}

pub(super) async fn pilot_turn_events(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AxumPath((identifier, turn)): AxumPath<(String, String)>,
    Query(query): Query<TurnEventsQuery>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, HttpResponse> {
    let last = match headers.get("last-event-id") {
        Some(value) => value
            .to_str()
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| error(StatusCode::BAD_REQUEST, "invalid Last-Event-ID"))?,
        None => 0,
    };
    let after = query.after.unwrap_or(0).max(last);
    let (sessions, owner, session) =
        blocking(move || owned_session(&state, &headers, &identifier)).await?;
    pilot_session(&session)?;
    let lookup = sessions.clone();
    let lookup_owner = owner.clone();
    let lookup_session = session.id.clone();
    let lookup_turn = turn.clone();
    blocking(move || {
        lookup
            .get_turn(&lookup_owner, &lookup_session, &lookup_turn)
            .map_err(session_error)
    })
    .await?;
    let cursor = Cursor {
        sessions,
        owner,
        session: session.id,
        turn,
        after,
        pending: VecDeque::new(),
        ended: false,
    };
    let stream = futures_util::stream::unfold(cursor, |mut cursor| async move {
        loop {
            if let Some(event) = cursor.pending.pop_front() {
                return Some((Ok(event), cursor));
            }
            if cursor.ended {
                return None;
            }
            let sessions = cursor.sessions.clone();
            let owner = cursor.owner.clone();
            let session = cursor.session.clone();
            let turn = cursor.turn.clone();
            let after = cursor.after;
            let loaded = tokio::task::spawn_blocking(move || {
                let turn_record = sessions.get_turn(&owner, &session, &turn)?;
                let events = sessions.turn_events(&owner, &session, &turn, after)?;
                let continuation =
                    crate::pilot_host::continuation_status(&sessions, &owner, &session, &turn)?;
                Ok::<_, anchor_platform_session::SessionError>((turn_record, events, continuation))
            })
            .await;
            match loaded {
                Ok(Ok((turn, events, continuation))) => {
                    for event in events {
                        cursor.after = event.seq;
                        cursor.pending.push_back(
                            Event::default()
                                .id(event.seq.to_string())
                                .data(event.data.to_string()),
                        );
                    }
                    if turn.status != TurnStatus::Running {
                        let mut value = serde_json::to_value(&turn).expect("Turn is JSON");
                        value["continuation"] = continuation;
                        cursor
                            .pending
                            .push_back(Event::default().event("turn").data(value.to_string()));
                        cursor.ended = true;
                    }
                }
                _ => {
                    cursor.pending.push_back(Event::default().event("error").data(json!({"type":"error","errorText":"Pilot delivery history unavailable"}).to_string()));
                    cursor.ended = true;
                }
            }
            if cursor.pending.is_empty() {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(10))))
}

#[derive(serde::Deserialize, Default)]
pub(super) struct AttachmentQuery {
    download: Option<u8>,
}

pub(super) async fn pilot_attachment(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AxumPath((identifier, turn, id)): AxumPath<(String, String, String)>,
    Query(query): Query<AttachmentQuery>,
) -> Result<HttpResponse, HttpResponse> {
    blocking(move || {
        let (sessions, owner, session) = owned_session(&state, &headers, &identifier)?;
        pilot_session(&session)?;
        let turn = sessions
            .get_turn(&owner, &identifier, &turn)
            .map_err(session_error)?;
        let (attachment, bytes) = crate::pilot_attachments::read(
            &state.data_root,
            &sessions,
            &owner,
            &session,
            &turn,
            &id,
        )
        .map_err(|_| error(StatusCode::NOT_FOUND, "Pilot attachment is unavailable"))?;
        let inline = query.download != Some(1)
            && matches!(
                attachment.media_type.as_deref(),
                Some("image/png" | "image/jpeg" | "image/webp")
            );
        let filename = crate::pilot_attachments::url_component(&attachment.name);
        let disposition = format!(
            "{}; filename=\"attachment\"; filename*=UTF-8''{filename}",
            if inline { "inline" } else { "attachment" }
        );
        let content_type = if inline {
            attachment
                .media_type
                .as_deref()
                .unwrap_or("application/octet-stream")
        } else {
            "application/octet-stream"
        };
        axum::http::Response::builder()
            .status(StatusCode::OK)
            .header(axum::http::header::CACHE_CONTROL, "no-store")
            .header("x-content-type-options", "nosniff")
            .header(axum::http::header::CONTENT_TYPE, content_type)
            .header(axum::http::header::CONTENT_DISPOSITION, disposition)
            .body(axum::body::Body::from(bytes))
            .map_err(|_| {
                error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Pilot attachment response unavailable",
                )
            })
    })
    .await
}

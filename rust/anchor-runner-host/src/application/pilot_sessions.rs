//! Pilot-specific creation policy. Protocol adapters supply identity and title;
//! generic Sessions and channel Sessions keep their own creation contracts.
use anchor_platform_session::{CreateSession, Session, SessionError, SessionStore};
use sha2::{Digest, Sha256};

pub(crate) fn validate_request_id(key: &str) -> Result<(), SessionError> {
    if key.is_empty() || key.len() > 128 || !key.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) {
        return Err(SessionError::Invalid(
            "operation key must contain 1 to 128 visible ASCII bytes".into(),
        ));
    }
    Ok(())
}

pub(crate) fn request_identity(owner: &str, namespace: &str, key: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(owner, namespace, key)).expect("string tuple serializes")
        )
    )
}

pub(crate) fn create_with_request(
    store: &SessionStore,
    owner: &str,
    mut request: CreateSession,
    key: &str,
    content: &serde_json::Value,
    namespace: &str,
    existing: Option<&str>,
) -> Result<Session, SessionError> {
    validate_request_id(key)?;
    if !request.graph.is_empty() || !request.channel.is_empty() {
        return Err(SessionError::Invalid(
            "Pilot Sessions cannot bind a Graph or channel".into(),
        ));
    }
    request
        .plugin_mounts
        .entry(crate::plugin_network::NETWORK_PLUGIN.to_owned())
        .or_insert(true);
    store.resolve_creation_request(owner, request, (namespace, key), content, existing)
}

pub(crate) fn create(
    store: &SessionStore,
    owner: &str,
    mut request: CreateSession,
) -> Result<Session, SessionError> {
    if !request.graph.is_empty() || !request.channel.is_empty() {
        return Err(SessionError::Invalid(
            "Pilot Sessions cannot bind a Graph or channel".into(),
        ));
    }
    request
        .plugin_mounts
        .entry(crate::plugin_network::NETWORK_PLUGIN.to_owned())
        .or_insert(true);
    store.create(owner, request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn native_admission_replays_running_and_terminal_turns_without_provider_or_execution() {
        use crate::{
            pilot_host::{PilotAdmission, PilotService},
            pilot_tools::PilotTools,
        };
        use anchor_platform_session::TurnStatus;
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("state");
        let sessions = SessionStore::open(root.path().join("sessions.sqlite")).unwrap();
        let session = create(&sessions, "owner", CreateSession::default()).unwrap();
        let (first, _) = sessions
            .create_turn("owner", &session.id, "accepted", Some("inspect"))
            .unwrap();
        for terminal in [
            None,
            Some(TurnStatus::Completed),
            Some(TurnStatus::Failed),
            Some(TurnStatus::Stopped),
            Some(TurnStatus::Interrupted),
        ] {
            if let Some(status) = terminal {
                let id = format!("accepted-{status:?}");
                let running = sessions
                    .list_turns("owner", &session.id)
                    .unwrap()
                    .into_iter()
                    .find(|turn| turn.status == TurnStatus::Running);
                if let Some(turn) = running {
                    sessions
                        .finish_turn("owner", &session.id, &turn.id, status, None)
                        .unwrap();
                }
                if terminal != Some(TurnStatus::Completed) {
                    let (turn, _) = sessions
                        .create_turn("owner", &session.id, &id, Some("inspect"))
                        .unwrap();
                    sessions
                        .finish_turn("owner", &session.id, &turn.id, status, None)
                        .unwrap();
                }
            }
            let retained = sessions.list_turns("owner", &session.id).unwrap();
            for turn in &retained {
                let tools = PilotTools {
                    application: crate::application::RunApplication::new(
                        data.clone(),
                        root.path().join("catalog"),
                    ),
                    graph_name: String::new(),
                    library_root: root.path().join("library"),
                    data_root: data.clone(),
                    workspace_root: root.path().join("workspace"),
                    sessions: sessions.clone(),
                    owner: "owner".into(),
                    session: session.id.clone(),
                    turn: String::new(),
                    context: None,
                };
                let service = PilotService::default();
                let result = service
                    .admit(PilotAdmission {
                        sessions: sessions.clone(),
                        owner: "owner".into(),
                        session: session.clone(),
                        request_id: turn.request_id.clone(),
                        prompt: Some("inspect".into()),
                        attachments: Default::default(),
                        input_source: None,
                        root: crate::pilot_host::scope(&data, "owner", &session),
                        tools,
                    })
                    .await;
                let replay = match result {
                    Ok(turn) => turn,
                    Err(_) => panic!("retry must not require a provider"),
                };
                assert_eq!(replay.id, turn.id);
                assert_eq!(replay.status, turn.status);
            }
            assert_eq!(
                sessions.list_turns("owner", &session.id).unwrap().len(),
                retained.len()
            );
            assert!(!data.exists(), "retry registered resources or execution");
        }
        assert!(sessions.get_turn("owner", &session.id, &first.id).is_ok());
    }

    #[test]
    fn keyed_creation_reuses_original_fact_after_edit_and_reopen_and_isolates_owners() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("sessions.sqlite");
        let store = SessionStore::open(&path).unwrap();
        let request = || CreateSession {
            title: "original".into(),
            ..Default::default()
        };
        let content = serde_json::json!({"title":"original"});
        let first = create_with_request(
            &store,
            "owner-a",
            request(),
            "create-1",
            &content,
            "pilot",
            None,
        )
        .unwrap();
        store.rename("owner-a", &first.id, "edited").unwrap();
        drop(store);
        let store = SessionStore::open(path).unwrap();
        let retry = create_with_request(
            &store,
            "owner-a",
            request(),
            "create-1",
            &content,
            "pilot",
            None,
        )
        .unwrap();
        assert_eq!(retry.id, first.id);
        assert_eq!(retry.title, "edited");
        assert!(matches!(
            create_with_request(
                &store,
                "owner-a",
                request(),
                "create-1",
                &serde_json::json!({"title":"changed"}),
                "pilot",
                None
            ),
            Err(SessionError::Conflict(_))
        ));
        let other = create_with_request(
            &store,
            "owner-b",
            request(),
            "create-1",
            &content,
            "pilot",
            None,
        )
        .unwrap();
        assert_ne!(other.id, first.id);
        let distinct = create_with_request(
            &store,
            "owner-a",
            request(),
            "create-2",
            &content,
            "pilot",
            None,
        )
        .unwrap();
        assert_ne!(distinct.id, first.id);
        assert_eq!(store.list("owner-a").unwrap().len(), 2);
    }

    #[test]
    fn pilot_defaults_are_persisted_without_changing_generic_sessions_or_explicit_policy() {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::open(root.path().join("sessions.sqlite")).unwrap();
        let request = |id: &str| CreateSession {
            id: Some(id.into()),
            ..Default::default()
        };
        let pilot = create(&store, "local", request("pilot")).unwrap();
        assert_eq!(pilot.plugin_mounts.get("network"), Some(&true));
        assert_eq!(
            store.get("local", "pilot").unwrap().plugin_mounts,
            pilot.plugin_mounts
        );
        let mut disabled = request("disabled");
        disabled.plugin_mounts.insert("network".into(), false);
        assert_eq!(
            create(&store, "local", disabled)
                .unwrap()
                .plugin_mounts
                .get("network"),
            Some(&false)
        );
        assert!(
            store
                .create("local", request("generic"))
                .unwrap()
                .plugin_mounts
                .is_empty()
        );
    }
}

use super::*;
use crate::application::channel_management::{
    ChannelBinding, ChannelManagementPort, ChannelRuntimeState,
};
use futures_util::future::BoxFuture;

struct Port(ChannelRuntimeState);

impl ChannelManagementPort for Port {
    fn status<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
    ) -> BoxFuture<'a, Result<Value, ApplicationError>> {
        Box::pin(async { Ok(json!({})) })
    }
    fn configure<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: ChannelBinding,
    ) -> BoxFuture<'a, Result<Value, ApplicationError>> {
        Box::pin(async { Err(ApplicationError::Missing) })
    }
    fn set_enabled<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: bool,
    ) -> BoxFuture<'a, Result<Value, ApplicationError>> {
        Box::pin(async { Err(ApplicationError::Missing) })
    }
    fn snapshot(&self, _: &str) -> Result<Option<ChannelRuntimeState>, ApplicationError> {
        Ok(Some(self.0.clone()))
    }
    fn callback_owner(&self, _: &str, _: &str) -> Option<String> {
        None
    }
}

fn port(enabled: bool, account: &str) -> std::sync::Arc<Port> {
    std::sync::Arc::new(Port(ChannelRuntimeState {
        owner: "local".into(),
        binding: ChannelBinding {
            graph: "fixture".into(),
            reply_node: "assistant".into(),
            plugin: "wecom".into(),
            users: vec!["user-a".into()],
            send_users: vec!["user-a".into()],
        },
        enabled,
        account: Some(account.into()),
        descriptor: None,
    }))
}

#[tokio::test]
async fn disabled_or_replaced_deployments_do_not_handover_and_explain_blocked_recovery() {
    let _environment = PROCESS_ENV.lock().await;
    for (enabled, account) in [(false, "corp-a"), (true, "another-bot")] {
        let (root, mut state) = fixture();
        set_host_env(root.path(), &state.data_root);
        let (sessions, session, _) = saved_assistant(&state, "local");
        state.application = state
            .application
            .clone()
            .with_channel_management(port(enabled, account));
        crate::api::assistant_recovery::resume_channel_assistants(&state)
            .await
            .unwrap();
        assert_eq!(
            sessions
                .get_channel_assistant("local", &session)
                .unwrap()
                .unwrap()
                .run_id,
            "persistent-fixture-run"
        );
        assert!(auto_resume_events(&sessions, "local", &session).is_empty());
        let app = router_with_web_root(state, root.path().join("web"));
        let (status, body) = call(
            app.clone(),
            "GET",
            &format!("/channel-sessions/{session}/assistant"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["needs_recovery"], false);
        assert!(
            body["recovery_blocked"]
                .as_str()
                .is_some_and(|reason| reason.contains("deployment binding"))
        );
        let (status, _) = call(app, "POST", "/runs/persistent-fixture-run/resume", None).await;
        assert_eq!(status, StatusCode::CONFLICT);
    }
}

#[tokio::test]
async fn account_alias_collision_recovers_only_the_canonical_session_without_erasing_legacy_history()
 {
    let (root, mut state) = fixture();
    let _environment = PROCESS_ENV.lock().await;
    set_host_env(root.path(), &state.data_root);
    let (sessions, old_session, _) = saved_assistant(&state, "local");
    let old_history = sessions.get("local", &old_session).unwrap();
    let old_run = run_store(&state)
        .load("persistent-fixture-run")
        .unwrap()
        .unwrap();
    let mut input = recovery_inbound("canonical-first-input");
    input.identity.account = Some("canonical-account".into());
    let canonical = sessions.admit_channel_inbound("local", input).unwrap();
    sessions
        .bind_channel_assistant(
            "local",
            &canonical.session.id,
            "canonical-run",
            "wait_input",
            "assistant",
            "reply",
        )
        .unwrap();
    sessions
        .register_channel_account("local", "wecom", "canonical-account", &["corp-a".into()])
        .unwrap();
    state.application = state
        .application
        .clone()
        .with_channel_management(port(true, "canonical-account"));
    crate::api::assistant_recovery::resume_channel_assistants(&state)
        .await
        .unwrap();
    assert_eq!(old_history, sessions.get("local", &old_session).unwrap());
    assert_eq!(
        old_run,
        run_store(&state)
            .load("persistent-fixture-run")
            .unwrap()
            .unwrap()
    );
    assert_eq!(sessions.list_channel_assistants().unwrap().len(), 2);
    assert!(auto_resume_events(&sessions, "local", &old_session).is_empty());
    let app = router_with_web_root(state, root.path().join("web"));
    let (status, body) = call(
        app,
        "GET",
        &format!("/channel-sessions/{old_session}/assistant"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["recovery_blocked"]
            .as_str()
            .unwrap()
            .contains("canonical")
    );
}

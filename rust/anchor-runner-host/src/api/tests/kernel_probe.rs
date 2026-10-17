use super::*;
use anchor_kernel::{Outcome, Phase};
use anchor_runtime::graph::RunStore;

#[tokio::test]
async fn explicit_kernel_probe_runs_real_host_ops_and_projects_existing_queries() {
    let (root, state) = fixture();
    let _env = PROCESS_ENV.lock().await;
    set_host_env(root.path(), &state.data_root);
    let definition = json!({"objective":"kernel probe","entry":"a","agents":{},
        "ops":{"work":{"run":"sh -c 'printf kernel-artifact > report.txt'"}},
        "nodes":[{"id":"a","op":"work"},{"id":"b","op":"work"},{"id":"c","op":"work"}],
        "edges":[{"from":"a","to":"b"},{"from":"b","to":"c"}]});
    std::fs::write(state.bundle_root.join("graph.json"), definition.to_string()).unwrap();
    let router = router(state.clone());
    let (status, body) = call(
        router.clone(),
        "POST",
        "/trigger",
        Some(r#"{"graph":"fixture","input":{},"execution":"kernel_probe"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let id = body["run"].as_str().unwrap();
    let runs = state.application.kernel_owner(id).unwrap().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            if runs.load(id).unwrap().unwrap().is_settled() && !runs.active(id) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        runs.load(id).unwrap().unwrap().phase(),
        Some(&Phase::Settled {
            result: Outcome::Done
        })
    );
    assert!(
        FileRunStore::new(state.data_root.join("runs"))
            .load(id)
            .unwrap()
            .is_none()
    );
    let (status, detail) = call(router.clone(), "GET", &format!("/runs/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["state"]["status"], "completed");
    assert_eq!(detail["projection"]["terminal"], true);
    for node in ["a", "b", "c"] {
        assert_eq!(detail["state"]["nodes"][node]["files"][0], "report.txt");
    }
    let (status, list) = call(router.clone(), "GET", "/runs", None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["runs"].as_array().unwrap().len(), 1);
    assert_eq!(list["runs"][0]["status"], "completed");
    let (status, late) = call(router, "POST", &format!("/runs/{id}/stop"), None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{late}");
}

#[tokio::test]
async fn kernel_read_view_http_tracks_running_human_wait_and_settlement_with_host_time() {
    use anchor_runtime::graph::*;
    use std::{future::Future, pin::Pin};
    struct Gate {
        started: tokio::sync::Notify,
        release: tokio::sync::Semaphore,
    }
    impl NodeExecutionPort for Gate {
        fn capabilities(&self) -> NodeExecutionCapabilities {
            NodeExecutionCapabilities {
                agent: true,
                exact_provider_request_budget: true,
                ..Default::default()
            }
        }
        fn completion_fact<'a>(
            &'a self,
            _: &'a InvocationKey,
        ) -> Pin<Box<dyn Future<Output = Result<CompletionFact, GraphError>> + Send + 'a>> {
            Box::pin(async { Ok(CompletionFact::NotStarted) })
        }
        fn execute<'a>(
            &'a self,
            _: NodeExecutionRequest,
        ) -> Pin<Box<dyn Future<Output = Result<NodeExecutionOutcome, GraphError>> + Send + 'a>>
        {
            Box::pin(async move {
                self.started.notify_one();
                self.release.acquire().await.unwrap().forget();
                Ok(NodeExecutionOutcome::Completed(NodeCompletion {
                    submission: "late".into(),
                    route: None,
                    output: json!({}),
                    model_requests: 1,
                }))
            })
        }
    }
    let (root, state) = fixture();
    let _env = PROCESS_ENV.lock().await;
    set_host_env(root.path(), &state.data_root);
    let id = "r-http-view";
    let record=GraphRunRecord::create_with_id(serde_json::from_value(json!({"objective":"read","entry":"work","agents":{"role":{"model":"fixture","instructions":"work"}},"nodes":[{"id":"work","agent":"role"}],"edges":[]})).unwrap(),json!({}),id).unwrap();
    let metadata = crate::application::RunMetadata::new(
        id.into(),
        "fixture".into(),
        record.graph_digest.clone(),
        &state.bundle_root.join("graph.json"),
    )
    .unwrap();
    crate::application::metadata::save(&state.data_root, &metadata).unwrap();
    let runs = state.application.kernel_runs(true).unwrap().unwrap();
    runs.create(&record, 50).unwrap();
    let gate = Arc::new(Gate {
        started: tokio::sync::Notify::new(),
        release: tokio::sync::Semaphore::new(0),
    });
    let worker = {
        let runs = runs.clone();
        let gate = gate.clone();
        tokio::spawn(async move {
            runs.drive(
                id,
                &anchor_runtime::graph::test_support::Artifacts,
                gate.as_ref(),
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
            )
            .await
            .unwrap()
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), gate.started.notified())
        .await
        .unwrap();
    let router = router(state.clone());
    let observation = state
        .data_root
        .join("kernel-run-observations")
        .join(format!("{id}.json"));
    let sentinel = "2001-02-03T04:05:06Z";
    std::fs::write(&observation, json!({"updated":sentinel}).to_string()).unwrap();
    let (status, running) = call(router.clone(), "GET", &format!("/runs/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{running}");
    assert_eq!(running["projection"]["phase"], "running");
    assert_eq!(
        running["projection"]["actions"],
        json!({"pause":false,"stop":true,"resume":false})
    );
    assert_eq!(running["state"]["updated"], sentinel);
    let (status, body) = call(router.clone(), "POST", &format!("/runs/{id}/stop"), None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let (_, waiting) = call(router.clone(), "GET", &format!("/runs/{id}"), None).await;
    assert_eq!(waiting["projection"]["phase"], "stopped");
    assert_eq!(
        waiting["projection"]["actions"],
        json!({"pause":false,"stop":false,"resume":false})
    );
    let updated: Value = serde_json::from_slice(&std::fs::read(&observation).unwrap()).unwrap();
    assert_ne!(updated["updated"], sentinel);
    assert_eq!(waiting["state"]["updated"], updated["updated"]);
    gate.release.add_permits(1);
    worker.await.unwrap();
    let (_, waiting) = call(router.clone(), "GET", &format!("/runs/{id}"), None).await;
    assert_eq!(waiting["projection"]["actions"]["resume"], true);
    assert_eq!(
        waiting["state"]["updated"], updated["updated"],
        "refused late results must not stamp an accepted-write observation"
    );
    let (status, body) = call(
        router.clone(),
        "POST",
        &format!("/runs/{id}/abandon"),
        Some(r#"{"reason":"operator"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, settled) = call(router.clone(), "GET", &format!("/runs/{id}"), None).await;
    assert_eq!(settled["projection"]["phase"], "aborted");
    assert_eq!(settled["projection"]["terminal"], true);
    assert_eq!(
        settled["projection"]["actions"],
        json!({"pause":false,"stop":false,"resume":false})
    );
    let before = std::fs::read(&observation).unwrap();
    let (status, _) = call(router, "POST", &format!("/runs/{id}/stop"), None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(std::fs::read(&observation).unwrap(), before);
}

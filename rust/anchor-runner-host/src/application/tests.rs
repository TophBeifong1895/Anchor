use super::*;
use std::time::Duration;

async fn forwarder(application: &RunApplication, run_id: &str) -> tokio::task::AbortHandle {
    application.active.lock().await[run_id]
        .wait_cancellation
        .as_ref()
        .unwrap()
        .abort_handle()
}

async fn wait_for_forwarder_exit(task: &tokio::task::AbortHandle, parent: &Arc<AtomicBool>) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !task.is_finished() || Arc::strong_count(parent) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancellation forwarder must release its captured parent token");
}

#[tokio::test]
async fn wait_child_completion_joins_forwarder_without_requesting_stop() {
    let root = tempfile::tempdir().unwrap();
    let application = RunApplication::new(root.path().join("state"), root.path().into());
    let parent = Arc::new(AtomicBool::new(false));
    let child = application
        .begin_wait_child("child", "graph", parent.clone())
        .await
        .unwrap();
    let task = forwarder(&application, "child").await;
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    assert_eq!(Arc::strong_count(&parent), 2);

    application.end_wait_child("child").await;
    assert!(task.is_finished());
    assert_eq!(Arc::strong_count(&parent), 1);
    assert_eq!(Arc::strong_count(&child.cancellation), 1);
    assert!(!application.run_is_active("child").await);
    assert!(!child.cancellation.load(Ordering::Acquire));
    assert!(!child.pause.load(Ordering::Acquire));
    parent.store(true, Ordering::Release);
    tokio::task::yield_now().await;
    assert!(!child.cancellation.load(Ordering::Acquire));
}

#[tokio::test]
async fn predecessor_handover_waits_for_child_terminal_status_and_executor_cleanup() {
    use anchor_runtime::graph::GraphSnapshot;
    let root = tempfile::tempdir().unwrap();
    let application = RunApplication::new(root.path().join("state"), root.path().into());
    let snapshot = GraphSnapshot::from_authoring(serde_json::json!({
        "entry":"work", "agents":{}, "ops":{"work":{"run":"true"}},
        "nodes":[{"id":"work","op":"work"}], "edges":[]
    }))
    .unwrap();
    let mut parent =
        GraphRunRecord::create_with_id(snapshot.clone(), Value::Null, "parent").unwrap();
    parent.status = RunStatus::Stopped;
    let mut child = GraphRunRecord::create_with_id(snapshot, Value::Null, "child").unwrap();
    let metadata = RunMetadata::graph_call_child(
        "child".into(),
        "graph".into(),
        child.graph_digest.clone(),
        root.path(),
        metadata::GraphCallSource {
            parent_run: parent.run_id.clone(),
            parent_graph: "graph".into(),
            parent_graph_digest: parent.graph_digest.clone(),
            node: "work".into(),
            invocation: 1,
            mode: "wait".into(),
            root_run: parent.run_id.clone(),
        },
    )
    .unwrap();
    metadata::save(&application.data_root, &metadata).unwrap();
    application.store().save(&child).unwrap();
    {
        let active = application.active.lock().await;
        assert!(matches!(
            application.ensure_predecessor_settled(&parent, &active),
            Err(ApplicationError::Conflict(_))
        ));
    }

    let cancellation = Arc::new(AtomicBool::new(false));
    application
        .begin_wait_child("child", "graph", cancellation)
        .await
        .unwrap();
    child.status = RunStatus::Completed;
    application.store().save(&child).unwrap();
    {
        let active = application.active.lock().await;
        assert!(matches!(
            application.ensure_predecessor_settled(&parent, &active),
            Err(ApplicationError::Conflict(_))
        ));
        assert!(application.active.try_lock().is_err());
    }

    application.end_wait_child("child").await;
    let active = application.active.lock().await;
    application
        .ensure_predecessor_settled(&parent, &active)
        .unwrap();
    assert_eq!(application.store().load("child").unwrap().unwrap(), child);
}

#[tokio::test]
async fn wait_child_parent_stop_forwards_and_releases_task_resources() {
    let root = tempfile::tempdir().unwrap();
    let application = RunApplication::new(root.path().join("state"), root.path().into());
    let parent = Arc::new(AtomicBool::new(false));
    let child = application
        .begin_wait_child("child", "graph", parent.clone())
        .await
        .unwrap();
    let task = forwarder(&application, "child").await;
    parent.store(true, Ordering::Release);
    wait_for_forwarder_exit(&task, &parent).await;
    assert!(child.cancellation.load(Ordering::Acquire));
    assert!(!child.pause.load(Ordering::Acquire));
    // A stop request does not retire an executing child before its Runner exits.
    assert!(application.run_is_active("child").await);
    application.end_wait_child("child").await;
    assert!(!application.run_is_active("child").await);
    assert_eq!(Arc::strong_count(&child.cancellation), 1);
}

#[tokio::test]
async fn wait_child_stop_releases_forwarder_without_stopping_parent() {
    let root = tempfile::tempdir().unwrap();
    let application = RunApplication::new(root.path().join("state"), root.path().into());
    let parent = Arc::new(AtomicBool::new(false));
    let child = application
        .begin_wait_child("child", "graph", parent.clone())
        .await
        .unwrap();
    let task = forwarder(&application, "child").await;
    application.control("child", "pause").await.unwrap();
    assert!(child.pause.load(Ordering::Acquire));
    assert!(!child.cancellation.load(Ordering::Acquire));
    assert!(!task.is_finished());
    application.control("child", "stop").await.unwrap();
    wait_for_forwarder_exit(&task, &parent).await;
    assert!(!parent.load(Ordering::Acquire));
    assert!(application.run_is_active("child").await);
    application.end_wait_child("child").await;
    assert_eq!(Arc::strong_count(&child.cancellation), 1);
}

#[tokio::test]
async fn wait_child_duplicate_registration_reuses_one_forwarder() {
    let root = tempfile::tempdir().unwrap();
    let application = RunApplication::new(root.path().join("state"), root.path().into());
    let parent = Arc::new(AtomicBool::new(false));
    let other_parent = Arc::new(AtomicBool::new(false));
    let first = application
        .begin_wait_child("child", "graph", parent.clone())
        .await
        .unwrap();
    let first_task = forwarder(&application, "child").await;
    let second = application
        .begin_wait_child("child", "graph", other_parent.clone())
        .await
        .unwrap();
    assert!(Arc::ptr_eq(&first.cancellation, &second.cancellation));
    assert!(Arc::ptr_eq(&first.pause, &second.pause));
    assert_eq!(forwarder(&application, "child").await.id(), first_task.id());
    assert_eq!(Arc::strong_count(&parent), 2);
    assert_eq!(Arc::strong_count(&other_parent), 1);
    application.end_wait_child("child").await;
    application.end_wait_child("child").await;
    assert!(first_task.is_finished());
    assert_eq!(Arc::strong_count(&parent), 1);
    assert!(!first.cancellation.load(Ordering::Acquire));
}

#[tokio::test]
async fn wait_child_unconfigured_executor_leaves_no_registration_or_task() {
    let root = tempfile::tempdir().unwrap();
    let mut application = RunApplication::new(root.path().join("state"), root.path().into());
    application.executor = None;
    let parent = Arc::new(AtomicBool::new(false));
    for _ in 0..2 {
        assert!(matches!(
            application.begin_wait_child("child", "graph", parent.clone()).await,
            Err(ApplicationError::Invalid(reason)) if reason.contains("executor")
        ));
        assert!(!application.run_is_active("child").await);
        assert_eq!(Arc::strong_count(&parent), 1);
    }
}

#[tokio::test]
async fn wait_child_metadata_error_leaves_no_registration_or_task() {
    let root = tempfile::tempdir().unwrap();
    let application = RunApplication::new(root.path().join("state"), root.path().into());
    let metadata_root = application.data_root.join("run-metadata");
    std::fs::create_dir_all(&metadata_root).unwrap();
    std::fs::write(metadata_root.join("child.json"), "invalid-json").unwrap();
    let parent = Arc::new(AtomicBool::new(false));
    assert!(
        application
            .begin_wait_child("child", "graph", parent.clone())
            .await
            .is_err()
    );
    assert!(!application.run_is_active("child").await);
    assert_eq!(Arc::strong_count(&parent), 1);
}

#[tokio::test]
async fn dropping_child_registry_aborts_and_releases_forwarder() {
    let root = tempfile::tempdir().unwrap();
    let application = RunApplication::new(root.path().join("state"), root.path().into());
    let parent = Arc::new(AtomicBool::new(false));
    let child = application
        .begin_wait_child("child", "graph", parent.clone())
        .await
        .unwrap();
    let task = forwarder(&application, "child").await;
    drop(application);
    wait_for_forwarder_exit(&task, &parent).await;
    assert_eq!(Arc::strong_count(&child.cancellation), 1);
    assert!(!child.cancellation.load(Ordering::Acquire));
}

#[tokio::test]
async fn detached_unconfigured_executor_preserves_ready_record_without_registration() {
    use anchor_runtime::graph::GraphSnapshot;
    let root = tempfile::tempdir().unwrap();
    let mut application = RunApplication::new(root.path().join("state"), root.path().into());
    application.executor = None;
    let snapshot = GraphSnapshot::from_authoring(serde_json::json!({
        "entry":"work", "agents":{}, "ops":{"work":{"run":"true"}},
        "nodes":[{"id":"work","op":"work"}], "edges":[]
    }))
    .unwrap();
    let record = GraphRunRecord::create_with_id(snapshot, serde_json::json!({}), "child").unwrap();
    let metadata = RunMetadata::graph_call_child(
        "child".into(),
        "graph".into(),
        record.graph_digest.clone(),
        root.path(),
        metadata::GraphCallSource {
            parent_run: "parent".into(),
            parent_graph: "parent-graph".into(),
            parent_graph_digest: "parent-digest".into(),
            node: "call".into(),
            invocation: 1,
            mode: "detach".into(),
            root_run: "parent".into(),
        },
    )
    .unwrap();
    metadata::save(&application.data_root, &metadata).unwrap();
    application.store().save(&record).unwrap();
    assert!(matches!(
        application.dispatch_detached("child").await,
        Err(ApplicationError::Invalid(reason)) if reason.contains("executor")
    ));
    assert!(!application.run_is_active("child").await);
    assert_eq!(application.store().load("child").unwrap().unwrap(), record);
}

#[tokio::test]
async fn pending_control_projects_active_tokens_with_stop_priority() {
    let root = tempfile::tempdir().unwrap();
    let data_root = root.path().join("state");
    let application = RunApplication::new(data_root.clone(), root.path().join("catalog"));
    let snapshot = anchor_runtime::graph::GraphSnapshot::from_authoring(serde_json::json!({
        "entry":"work", "ops":{"work":{"run":"true"}},
        "nodes":[{"id":"work","op":"work"}], "edges":[]
    }))
    .unwrap();
    let record =
        GraphRunRecord::create_with_id(snapshot.clone(), Value::Null, "active-run").unwrap();
    let missing = GraphRunRecord::create_with_id(snapshot, Value::Null, "missing").unwrap();
    let control = new_control();
    application.active.lock().await.insert(
        "active-run".into(),
        ActiveRun::new(root.path().join("graph"), control.clone()),
    );
    assert_eq!(
        application.project_run(&missing, None).await.unwrap().phase,
        "executing"
    );
    assert_eq!(
        application.project_run(&record, None).await.unwrap().phase,
        "executing"
    );
    control.pause.store(true, Ordering::Relaxed);
    assert_eq!(
        application.project_run(&record, None).await.unwrap().phase,
        "executing"
    );
    control.cancellation.store(true, Ordering::Relaxed);
    assert_eq!(
        application.project_run(&record, None).await.unwrap().phase,
        "executing"
    );
    application.active.lock().await.remove("active-run");
    assert_eq!(
        application.project_run(&record, None).await.unwrap().phase,
        "executing"
    );
    assert!(!data_root.exists());
}

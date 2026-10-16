use super::*;
use crate::application::{ApplicationError, scheduling::ScheduleManagementPort};
use anchor_platform_session::{CreateSession, TurnStatus};
use std::sync::Mutex;

#[derive(Default)]
struct Port {
    created: Mutex<Vec<Value>>,
}

impl ScheduleManagementPort for Port {
    fn list(&self) -> Result<Value, ApplicationError> {
        Ok(json!({"schedules": &*self.created.lock().unwrap()}))
    }
    fn create(&self, graph: &str, rule: &Value, input: &Value) -> Result<Value, ApplicationError> {
        let item = json!({"id":"fixture-schedule", "graph":graph,"rule":rule,"input":input});
        self.created.lock().unwrap().push(item.clone());
        Ok(json!({"schedule":item}))
    }
    fn update(
        &self,
        id: &str,
        graph: &str,
        rule: &Value,
        input: &Value,
        enabled: bool,
    ) -> Result<Value, ApplicationError> {
        let mut items = self.created.lock().unwrap();
        let item = items
            .iter_mut()
            .find(|item| item["id"] == id)
            .ok_or(ApplicationError::Missing)?;
        *item = json!({"id":id,"graph":graph,"rule":rule,"input":input,"enabled":enabled});
        Ok(json!({"schedule":item}))
    }
    fn delete(&self, id: &str) -> Result<Value, ApplicationError> {
        let mut items = self.created.lock().unwrap();
        let len = items.len();
        items.retain(|item| item["id"] != id);
        if items.len() == len {
            return Err(ApplicationError::Missing);
        }
        Ok(json!({"schedule":id,"deleted":true}))
    }
}

fn fixture() -> (tempfile::TempDir, PilotTools, Arc<Port>) {
    let root = tempfile::tempdir().unwrap();
    let graph = root.path().join("catalog/task");
    std::fs::create_dir_all(&graph).unwrap();
    std::fs::write(graph.join("graph.json"), r#"{"entry":"work","ops":{"work":{"run":"true"}},"nodes":[{"id":"work","op":"work"}],"edges":[]}"#).unwrap();
    std::fs::write(
        graph.join("manifest.json"),
        r#"{"format":1,"graph":"graph.json","plugins":[]}"#,
    )
    .unwrap();
    let sessions = SessionStore::open(root.path().join("sessions.sqlite")).unwrap();
    sessions
        .create(
            "local",
            CreateSession {
                id: Some("pilot".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let port = Arc::new(Port::default());
    let tools = PilotTools {
        application: RunApplication::new(root.path().join("state"), root.path().join("catalog"))
            .with_schedule_management(port.clone()),
        graph_name: String::new(),
        library_root: root.path().join("library"),
        data_root: root.path().join("state"),
        workspace_root: root.path().join("workspaces"),
        sessions,
        owner: "local".into(),
        session: "pilot".into(),
        turn: String::new(),
        context: None,
    };
    (root, tools, port)
}

#[tokio::test]
async fn pilot_schedule_management_requires_owned_running_turns_and_never_starts_runs() {
    let (_root, mut tools, port) = fixture();
    let arguments = json!({"graph":"task","rule":{"type":"daily","time":"09:00"},"input":{"label":"requested"}});
    assert!(tools.schedule_create(&arguments).await.is_err());
    let (turn, _) = tools
        .sessions
        .create_turn("local", "pilot", "scheduled-task", Some("schedule task"))
        .unwrap();
    tools = tools.for_turn(&turn.id);
    tools.owner = "other-owner".into();
    assert!(tools.schedule_create(&arguments).await.is_err());
    tools.owner = "local".into();
    let result = tools.schedule_create(&arguments).await.unwrap();
    assert_eq!(result["schedule"]["graph"], "task");
    assert_eq!(port.created.lock().unwrap().len(), 1);
    assert!(tools.application.records().unwrap().is_empty());
    assert!(tools.is_read_only("schedule_list"));
    assert!(!tools.is_read_only("schedule_create"));
    assert!(!tools.is_read_only("schedule_delete"));
    tools
        .sessions
        .finish_turn("local", "pilot", &turn.id, TurnStatus::Completed, None)
        .unwrap();
    assert!(
        tools
            .schedule_delete(&json!({"schedule":"fixture-schedule"}))
            .await
            .is_err()
    );
    assert!(tools.schedule_create(&arguments).await.is_err());
    assert_eq!(port.created.lock().unwrap().len(), 1);
    assert_eq!(
        tools.schedule_list(&json!({})).await.unwrap()["schedules"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn schedule_creation_rejects_unknown_authorization_fields_and_unavailable_graphs() {
    let (_root, tools, port) = fixture();
    let (turn, _) = tools
        .sessions
        .create_turn("local", "pilot", "schedule-fields", None)
        .unwrap();
    let tools = tools.for_turn(&turn.id);
    for arguments in [
        json!({"graph":"missing","rule":{"type":"daily","time":"09:00"}}),
        json!({"graph":"task","rule":{"type":"daily","time":"09:00"},"input":[]}),
        json!({"graph":"task","rule":{"type":"daily","time":"09:00"},"network":true}),
        json!({"graph":"task","rule":{"type":"daily","time":"09:00"},"credentials":{"TOKEN":"secret"}}),
    ] {
        assert!(tools.schedule_create(&arguments).await.is_err());
    }
    assert!(port.created.lock().unwrap().is_empty());
    assert!(tools.application.records().unwrap().is_empty());
}

#[tokio::test]
async fn schedule_creation_waits_for_catalog_publication_and_rechecks_graph() {
    let (root, tools, port) = fixture();
    let guard = tools.application.graph_catalog_mutation_guard().await;
    let application = tools.application.clone();
    let mut creation = tokio::spawn(async move {
        application
            .create_schedule("task", &json!({"type":"daily","time":"09:00"}), &json!({}))
            .await
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut creation)
            .await
            .is_err()
    );
    std::fs::remove_dir_all(root.path().join("catalog/task")).unwrap();
    drop(guard);
    assert!(matches!(
        creation.await.unwrap(),
        Err(ApplicationError::Missing)
    ));
    assert!(port.created.lock().unwrap().is_empty());
}

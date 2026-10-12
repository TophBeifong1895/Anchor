use super::*;
use anchor_platform_session::CreateSession;
use serde_json::json;
use std::{ffi::OsStr, time::Duration};

#[tokio::test]
async fn continuation_facts_keep_stop_interrupt_supersession_and_replay_distinct() {
    let fixture = Fixture::new();
    let session = fixture.create("continuation");
    let (source, _) = fixture
        .sessions
        .create_turn("owner", &session.id, "source", Some("initialize"))
        .unwrap();
    fixture
        .sessions
        .append_event(
            "owner",
            &session.id,
            "pilot.continuation.requested",
            std::collections::BTreeMap::from([
                ("turn".into(), json!(source.id)),
                (
                    "request_id".into(),
                    json!(format!("continuation:{}", source.id)),
                ),
            ]),
        )
        .unwrap();
    fixture
        .sessions
        .finish_turn("owner", &session.id, &source.id, TurnStatus::Stopped, None)
        .unwrap();
    assert_eq!(
        continuation_status(&fixture.sessions, "owner", &session.id, &source.id).unwrap()["status"],
        "cancelled"
    );
    assert!(matches!(
        fixture
            .service
            .admit(fixture.admission(&session, &format!("continuation:{}", source.id)))
            .await,
        Err(AdmissionError::Session(SessionError::Conflict(_)))
    ));
    let (interrupted, _) = fixture
        .sessions
        .create_turn("owner", &session.id, "interrupted", Some("initialize"))
        .unwrap();
    fixture
        .sessions
        .append_event(
            "owner",
            &session.id,
            "pilot.continuation.requested",
            std::collections::BTreeMap::from([
                ("turn".into(), json!(interrupted.id)),
                (
                    "request_id".into(),
                    json!(format!("continuation:{}", interrupted.id)),
                ),
            ]),
        )
        .unwrap();
    fixture
        .sessions
        .finish_turn(
            "owner",
            &session.id,
            &interrupted.id,
            TurnStatus::Interrupted,
            None,
        )
        .unwrap();
    assert_eq!(
        continuation_status(&fixture.sessions, "owner", &session.id, &interrupted.id).unwrap()["status"],
        "pending"
    );
    let (latest, _) = fixture
        .sessions
        .create_turn("owner", &session.id, "user-newer", Some("continue"))
        .unwrap();
    fixture
        .sessions
        .finish_turn(
            "owner",
            &session.id,
            &latest.id,
            TurnStatus::Completed,
            None,
        )
        .unwrap();
    assert_eq!(
        continuation_status(&fixture.sessions, "owner", &session.id, &interrupted.id).unwrap()["status"],
        "superseded"
    );
    assert!(matches!(
        fixture
            .service
            .admit(fixture.admission(&session, &format!("continuation:{}", interrupted.id)))
            .await,
        Err(AdmissionError::Session(SessionError::Conflict(_)))
    ));
    let id = format!("continuation:{}", latest.id);
    let (next, _) = fixture
        .sessions
        .create_turn("owner", &session.id, &id, Some("inspect"))
        .unwrap();
    fixture
        .sessions
        .finish_turn("owner", &session.id, &next.id, TurnStatus::Completed, None)
        .unwrap();
    let replay = fixture
        .service
        .admit(fixture.admission(&session, &id))
        .await;
    assert!(matches!(replay,Ok(turn) if turn.id==next.id));
    assert_eq!(
        continuation_status(&fixture.sessions, "owner", &session.id, &latest.id).unwrap()["turn"],
        next.id
    );
}

#[tokio::test]
async fn credential_submission_and_stop_share_one_control_boundary() {
    let fixture = Fixture::new();
    let session = fixture.create("credential-boundary");
    let root = scope(&fixture.data, "owner", &session);
    let cancellation = Arc::new(AtomicBool::new(false));
    fixture
        .service
        .active
        .lock()
        .unwrap()
        .insert(root.clone(), cancellation.clone());
    let guard = fixture
        .service
        .credential_submission_guard(&root)
        .await
        .unwrap();
    let service = fixture.service.clone();
    let stopped_root = root.clone();
    let stop = tokio::spawn(async move { service.stop(&stopped_root).await });
    tokio::task::yield_now().await;
    assert!(!stop.is_finished());
    assert!(!cancellation.load(Ordering::Acquire));
    drop(guard);
    assert!(stop.await.unwrap());
    assert!(
        fixture
            .service
            .credential_submission_guard(&root)
            .await
            .is_err()
    );
}

struct Environment(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl Environment {
    fn set(&mut self, name: &'static str, value: impl AsRef<OsStr>) {
        self.0.push((name, std::env::var_os(name)));
        unsafe { std::env::set_var(name, value) };
    }

    fn remove(&mut self, name: &'static str) {
        self.0.push((name, std::env::var_os(name)));
        unsafe { std::env::remove_var(name) };
    }
}

impl Drop for Environment {
    fn drop(&mut self) {
        for (name, value) in self.0.drain(..).rev() {
            unsafe {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    catalog: PathBuf,
    data: PathBuf,
    workspace: PathBuf,
    sessions: SessionStore,
    service: PilotService,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let catalog = root.path().to_path_buf();
        let data = catalog.join("state");
        let workspace = catalog.join("workspaces");
        std::fs::create_dir_all(data.join("platform")).unwrap();
        std::fs::create_dir_all(catalog.join("plugins/network")).unwrap();
        std::fs::write(
            catalog.join("plugins/network/plugin.json"),
            json!({"name":"Network fixture","network":true}).to_string(),
        )
        .unwrap();
        let sessions = SessionStore::open(data.join("platform/sessions.sqlite")).unwrap();
        Self {
            _root: root,
            catalog,
            data,
            workspace,
            sessions,
            service: PilotService::default(),
        }
    }

    fn configure(&self) -> Environment {
        // This fixture exercises admission and real HTTP MCP startup only. A
        // cancelled startup must never invoke Goose or the model transport.
        let binary = self.catalog.join("unstarted-goose");
        let bytes = b"Goose must not be started by this admission fixture";
        std::fs::write(&binary, bytes).unwrap();
        let mut environment = Environment(Vec::new());
        environment.set("ANCHOR_RUNNER_AGENT_RUNTIME", "goose");
        environment.set("ANCHOR_GOOSE_BINARY", &binary);
        environment.set(
            "ANCHOR_GOOSE_BINARY_SHA256",
            format!("{:x}", Sha256::digest(bytes)),
        );
        environment.set("ANCHOR_GOOSE_LOCAL_NETWORK", "0");
        environment.set("ANCHOR_GOOSE_ALLOW_SHARED_NETWORK", "1");
        environment.set("ANCHOR_MODEL_URL", "http://127.0.0.1:1/v1");
        environment.set("ANCHOR_MODEL_API_KEY", "admission-fixture-only");
        environment.set("ANCHOR_MODEL_NAME", "admission-fixture");
        environment.set("ANCHOR_MODEL_WIRE_API", "chat");
        environment.set("ANCHOR_MODEL_DIRECT", "0");
        environment.set("ANCHOR_RUNNER_ALLOWED_COMMANDS", "true");
        environment.set("ANCHOR_PILOT_PYTHON_ENABLED", "0");
        environment.remove("ANCHOR_MODEL_ALIASES");
        environment.remove("ANCHOR_MODEL_CONTEXT_WINDOW");
        environment.remove("ANCHOR_RUNNER_LIBRARY_ROOT");
        environment.remove("ANCHOR_RUNNER_FAKE_PLUGINS");
        environment.remove("ANCHOR_PILOT_RUSTUP");
        environment.remove("ANCHOR_PILOT_PYTHON");
        environment.remove("ANCHOR_BWRAP");
        environment
    }

    fn create(&self, id: &str) -> Session {
        self.sessions
            .create(
                "owner",
                CreateSession {
                    id: Some(id.into()),
                    ..Default::default()
                },
            )
            .unwrap()
    }

    fn mount_slow_plugin(&self, session: &Session, endpoint: &str) {
        let plugin = self.catalog.join("plugins/slow");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::write(
            plugin.join("plugin.json"),
            json!({"name":"Slow startup", "mcpServers":{"remote":{"type":"http","url":endpoint}}})
                .to_string(),
        )
        .unwrap();
        self.sessions
            .set_plugin_mount("owner", &session.id, "slow", true, "operator", "fixture")
            .unwrap();
    }

    fn admission(&self, session: &Session, request: &str) -> PilotAdmission {
        let tools = PilotTools {
            application: crate::application::RunApplication::new(
                self.data.clone(),
                self.catalog.clone(),
            ),
            graph_name: "pilot".into(),
            catalog_root: self.catalog.clone(),
            library_root: self.catalog.clone(),
            data_root: self.data.clone(),
            workspace_root: self.workspace.clone(),
            sessions: self.sessions.clone(),
            owner: "owner".into(),
            session: session.id.clone(),
            turn: String::new(),
            context: None,
        };
        PilotAdmission {
            sessions: self.sessions.clone(),
            owner: "owner".into(),
            session: session.clone(),
            request_id: request.into(),
            prompt: Some("inspect".into()),
            root: scope(&self.data, "owner", session),
            tools,
        }
    }

    async fn admit(&self, session: &Session, request: &str) -> Turn {
        match tokio::time::timeout(
            Duration::from_secs(2),
            self.service.admit(self.admission(session, request)),
        )
        .await
        .expect("MCP startup held Pilot admission")
        {
            Ok(turn) => turn,
            Err(AdmissionError::Session(error)) => panic!("admission failed: {error}"),
            Err(AdmissionError::Provider) => panic!("fixture provider configuration was rejected"),
        }
    }

    async fn delete(&self, session: &Session) -> Result<(), SessionError> {
        tokio::time::timeout(
            Duration::from_secs(2),
            self.service.delete_session(
                self.sessions.clone(),
                self.data.clone(),
                self.workspace.clone(),
                self.catalog.clone(),
                "owner".into(),
                session.clone(),
            ),
        )
        .await
        .expect("MCP startup held Pilot deletion")
    }

    async fn terminal(&self, session: &Session, turn: &Turn) -> Turn {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                {
                    // A terminal fact and release of the execution happen under
                    // one gate, so DELETE can only observe the complete boundary.
                    let _gate = self.service.gate.lock().await;
                    let saved = self
                        .sessions
                        .get_turn("owner", &session.id, &turn.id)
                        .unwrap();
                    if saved.status != TurnStatus::Running {
                        assert!(
                            !self
                                .service
                                .active
                                .lock()
                                .unwrap()
                                .contains_key(&scope(&self.data, "owner", session))
                        );
                        return saved;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("cancelled MCP startup did not settle its Turn")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(active) = self.service.active.lock() {
            for cancellation in active.values() {
                cancellation.store(true, Ordering::Release);
            }
        }
    }
}

struct HangingHttpMcp {
    endpoint: String,
    started: tokio::sync::mpsc::UnboundedReceiver<()>,
    task: tokio::task::JoinHandle<()>,
}

impl HangingHttpMcp {
    async fn start() -> Self {
        let (send, started) = tokio::sync::mpsc::unbounded_channel();
        let app = axum::Router::new().route(
            "/mcp",
            axum::routing::post(move || {
                let send = send.clone();
                async move {
                    send.send(()).unwrap();
                    std::future::pending::<axum::http::StatusCode>().await
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            endpoint,
            started,
            task,
        }
    }

    async fn entered(&mut self) {
        tokio::time::timeout(Duration::from_secs(2), self.started.recv())
            .await
            .expect("Pilot did not reach HTTP MCP startup")
            .expect("HTTP MCP fixture exited");
    }
}

impl Drop for HangingHttpMcp {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn slow_mcp_startup_keeps_controls_and_other_sessions_available() {
    let _env = crate::PROCESS_ENV_LOCK.lock().await;
    let fixture = Fixture::new();
    let _configured = fixture.configure();
    let mut server = HangingHttpMcp::start().await;
    let first = fixture.create("first");
    fixture.mount_slow_plugin(&first, &server.endpoint);
    let first_turn = fixture.admit(&first, "first-request").await;
    let first_scope = scope(&fixture.data, "owner", &first);
    assert!(
        fixture
            .service
            .active
            .lock()
            .unwrap()
            .contains_key(&first_scope)
    );
    server.entered().await;

    // Replaying an accepted input remains idempotent while its MCP is waiting;
    // a distinct input cannot overwrite the cancellation token or reenter.
    assert_eq!(
        fixture.admit(&first, "first-request").await.id,
        first_turn.id
    );
    let conflict = tokio::time::timeout(
        Duration::from_secs(2),
        fixture.service.admit(fixture.admission(&first, "overlap")),
    )
    .await
    .expect("MCP startup blocked the reentry check");
    assert!(matches!(
        conflict,
        Err(AdmissionError::Session(SessionError::Conflict(_)))
    ));
    assert!(matches!(
        fixture.delete(&first).await,
        Err(SessionError::Conflict(_))
    ));
    assert!(
        !tokio::time::timeout(
            Duration::from_secs(2),
            fixture
                .service
                .stop(&scope(&fixture.data, "other-owner", &first)),
        )
        .await
        .unwrap()
    );

    let idle = fixture.create("idle");
    fixture.delete(&idle).await.unwrap();
    let second = fixture.create("second");
    fixture.mount_slow_plugin(&second, &server.endpoint);
    let second_turn = fixture.admit(&second, "second-request").await;
    server.entered().await;
    assert!(
        tokio::time::timeout(
            Duration::from_secs(2),
            fixture
                .service
                .stop(&scope(&fixture.data, "owner", &second)),
        )
        .await
        .unwrap()
    );
    assert_eq!(
        fixture.terminal(&second, &second_turn).await.status,
        TurnStatus::Stopped
    );
    fixture.delete(&second).await.unwrap();
    assert_eq!(
        fixture
            .sessions
            .get_turn("owner", &first.id, &first_turn.id)
            .unwrap()
            .status,
        TurnStatus::Running
    );

    // Mount edits made after acceptance cannot affect this Turn's pinned policy.
    fixture
        .sessions
        .set_plugin_mount("owner", &first.id, "slow", false, "operator", "next Turn")
        .unwrap();
    let snapshot = crate::plugin_mounts::read_snapshot(
        &crate::plugin_mounts::snapshot_path(&fixture.data, &first.id, &first_turn.id),
        &first.id,
        &first_turn.id,
    )
    .unwrap();
    assert_eq!(snapshot.mounts.get("slow"), Some(&true));
    assert!(
        tokio::time::timeout(Duration::from_secs(2), fixture.service.stop(&first_scope))
            .await
            .unwrap()
    );
    assert_eq!(
        fixture.terminal(&first, &first_turn).await.status,
        TurnStatus::Stopped
    );
    fixture.delete(&first).await.unwrap();

    let recreated = fixture.create("first");
    assert_ne!(first.created_at, recreated.created_at);
    fixture.mount_slow_plugin(&recreated, &server.endpoint);
    let current_turn = fixture.admit(&recreated, "new-identity").await;
    server.entered().await;
    assert!(!fixture.service.stop(&first_scope).await);
    assert!(
        matches!(fixture.delete(&first).await, Err(SessionError::Conflict(reason)) if reason.contains("identity"))
    );
    let stale = fixture
        .service
        .admit(fixture.admission(&first, "stale-identity"))
        .await;
    assert!(
        matches!(stale, Err(AdmissionError::Session(SessionError::Conflict(reason))) if reason.contains("identity"))
    );
    assert_eq!(
        fixture
            .sessions
            .get_turn("owner", &recreated.id, &current_turn.id)
            .unwrap()
            .status,
        TurnStatus::Running
    );
    assert!(
        fixture
            .service
            .stop(&scope(&fixture.data, "owner", &recreated))
            .await
    );
    assert_eq!(
        fixture.terminal(&recreated, &current_turn).await.status,
        TurnStatus::Stopped
    );
    fixture.delete(&recreated).await.unwrap();
    assert!(fixture.service.active.lock().unwrap().is_empty());
}

#[tokio::test]
async fn mount_snapshot_failure_settles_the_turn_before_registering_execution() {
    let _env = crate::PROCESS_ENV_LOCK.lock().await;
    let fixture = Fixture::new();
    let _configured = fixture.configure();
    let session = fixture.create("snapshot-failure");
    std::fs::write(fixture.data.join("plugin-mounts"), "not a directory").unwrap();
    let result = fixture
        .service
        .admit(fixture.admission(&session, "failure"))
        .await;
    assert!(
        matches!(result, Err(AdmissionError::Session(SessionError::Storage(reason))) if reason.contains("snapshot"))
    );
    let turns = fixture.sessions.list_turns("owner", &session.id).unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].status, TurnStatus::Interrupted);
    assert!(fixture.service.active.lock().unwrap().is_empty());
}

#[tokio::test]
async fn stopping_an_accepted_turn_before_startup_skips_the_sandbox() {
    let _env = crate::PROCESS_ENV_LOCK.lock().await;
    let fixture = Fixture::new();
    let mut configured = fixture.configure();
    configured.set("ANCHOR_BWRAP", fixture.catalog.join("missing-bwrap"));
    let session = fixture.create("stop-before-startup");
    let turn = fixture.admit(&session, "stop").await;
    // Admission spawned the executor without yielding after active registration;
    // in this current-thread test the stop happens before its first poll.
    assert!(
        fixture
            .service
            .stop(&scope(&fixture.data, "owner", &session))
            .await
    );
    assert_eq!(
        fixture.terminal(&session, &turn).await.status,
        TurnStatus::Stopped
    );
    assert!(
        !crate::pilot_tools::session_workspace(&fixture.workspace, "owner", &session.id).exists()
    );
    fixture.delete(&session).await.unwrap();
}

#[tokio::test]
async fn background_context_failure_settles_and_releases_the_accepted_turn() {
    let _env = crate::PROCESS_ENV_LOCK.lock().await;
    let fixture = Fixture::new();
    let mut configured = fixture.configure();
    configured.set("ANCHOR_BWRAP", fixture.catalog.join("missing-bwrap"));
    let session = fixture.create("context-failure");
    let turn = fixture.admit(&session, "fail").await;
    let saved = fixture.terminal(&session, &turn).await;
    assert_eq!(saved.status, TurnStatus::Interrupted);
    assert!(saved.error.is_some());
    assert_eq!(fixture.admit(&session, "fail").await.id, turn.id);
    fixture.delete(&session).await.unwrap();
}

fn process_identity(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // comm may contain spaces or parentheses. Field 22 (starttime), relative to
    // field 3 after the final ') ', identifies this exact process across PID reuse.
    stat.rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(19)
        .map(str::to_owned)
}

fn processes_with_marker(marker: &str) -> Vec<(u32, String, bool)> {
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            let command = std::fs::read(entry.path().join("cmdline")).ok()?;
            if !command
                .windows(marker.len())
                .any(|window| window == marker.as_bytes())
            {
                return None;
            }
            let status = std::fs::read_to_string(entry.path().join("status")).ok()?;
            let parent = status
                .lines()
                .find(|line| line.starts_with("PPid:"))?
                .split_whitespace()
                .nth(1)?
                .parse::<u32>()
                .ok()?;
            Some((pid, process_identity(pid)?, parent == std::process::id()))
        })
        .collect()
}

#[tokio::test]
async fn connected_stdio_mcp_is_reaped_before_a_failed_pilot_turn_can_be_deleted() {
    let _env = crate::PROCESS_ENV_LOCK.lock().await;
    let fixture = Fixture::new();
    let mut configured = fixture.configure();
    configured.set("ANCHOR_RUNNER_ALLOWED_COMMANDS", "true,sh");
    let session = fixture.create("cleanup");
    let plugin = fixture.catalog.join("plugins/healthy");
    std::fs::create_dir_all(&plugin).unwrap();
    let marker = format!(
        "pilot-mcp-cleanup-{}",
        fixture.catalog.file_name().unwrap().to_string_lossy()
    );
    // /usr/bin/python3 follows /etc/alternatives on this host, a path the
    // ordinary Plugin sandbox intentionally does not mount.
    let python = std::fs::canonicalize("/usr/bin/python3").unwrap();
    std::fs::write(plugin.join("plugin.json"), json!({
        "name":"Healthy stdio fixture",
        "mcpServers":{"local":{"command":"sh","args":["-c",format!("exec {} /plugins/healthy/server.py {marker} 2>/workspace/fixture-stderr", python.display())]}}
    }).to_string()).unwrap();
    std::fs::write(plugin.join("server.py"), r#"
import json, os, sys, time
for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    if request["method"] == "initialize":
        result = {"protocolVersion": request["params"]["protocolVersion"], "capabilities": {"tools": {}}, "serverInfo": {"name": "pilot-cleanup", "version": "1"}}
    elif request["method"] == "tools/list":
        with open("/workspace/connected", "w") as marker:
            marker.write("MCP tools/list reached")
        while not os.path.exists("/workspace/release-startup"):
            time.sleep(0.01)
        result = {"tools": []}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
# Reaching EOF alone must not masquerade as process exit; Host shutdown reaps it.
time.sleep(600)
"#).unwrap();
    fixture
        .sessions
        .set_plugin_mount("owner", &session.id, "healthy", true, "operator", "fixture")
        .unwrap();
    let turn = fixture.admit(&session, "cleanup-request").await;
    let workspace = crate::pilot_tools::session_workspace(&fixture.workspace, "owner", &session.id);
    let initialized = tokio::time::timeout(Duration::from_secs(2), async {
        while !workspace.join("connected").is_file() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        initialized.is_ok(),
        "stdio MCP fixture did not initialize: {}",
        std::fs::read_to_string(workspace.join("fixture-stderr")).unwrap_or_default()
    );
    let processes = processes_with_marker(&marker);
    assert!(
        !processes.is_empty(),
        "MCP process was not observable while tools/list was blocked"
    );
    assert!(
        processes.iter().any(|(_, _, direct_child)| *direct_child),
        "Host-owned MCP child was not observed"
    );
    assert!(matches!(
        fixture.delete(&session).await,
        Err(SessionError::Conflict(_))
    ));
    std::fs::write(workspace.join("release-startup"), "go").unwrap();
    // The configuration-only Goose file cannot execute. This proves the error
    // path still closes already-connected MCPs before releasing active ownership.
    assert_eq!(
        fixture.terminal(&session, &turn).await.status,
        TurnStatus::Interrupted
    );
    for (pid, identity, direct_child) in &processes {
        if *direct_child {
            assert_ne!(
                process_identity(*pid).as_deref(),
                Some(identity.as_str()),
                "Host-owned MCP child {pid} was not reaped before its Turn settled"
            );
        }
    }
    // The outer child is already reaped. As in the Sandbox regressions, allow
    // bounded kernel convergence for namespace descendants receiving SIGKILL;
    // a zombie has exited, and cannot run Plugin code or keep writable handles.
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let settled = processes.iter().all(|(pid, identity, _)| {
                if process_identity(*pid).as_deref() != Some(identity.as_str()) {
                    return true;
                }
                std::fs::read_to_string(format!("/proc/{pid}/status")).is_ok_and(|status| {
                    status.lines().any(|line| {
                        line.starts_with("State:") && line.split_whitespace().nth(1) == Some("Z")
                    })
                })
            });
            if settled {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("MCP namespace descendants did not settle after the outer child was reaped");
    fixture.delete(&session).await.unwrap();
    assert!(!workspace.exists());
}

#[tokio::test]
async fn stale_admission_and_delete_cannot_reach_a_recreated_session() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("state");
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(data.join("platform")).unwrap();
    let sessions = SessionStore::open(data.join("platform/sessions.sqlite")).unwrap();
    let create = || {
        sessions
            .create(
                "owner",
                CreateSession {
                    id: Some("reused".into()),
                    ..Default::default()
                },
            )
            .unwrap()
    };
    let old = create();
    sessions.delete("owner", "reused").unwrap();
    let current = create();
    assert_ne!(old.created_at, current.created_at);
    crate::pilot_resources::register(&data, &workspace, "owner", &current).unwrap();
    let task = crate::pilot_tools::session_workspace(&workspace, "owner", "reused");
    std::fs::create_dir_all(&task).unwrap();
    std::fs::write(task.join("keep"), "new Session").unwrap();
    let service = PilotService::default();
    let rejected = service
        .delete_session(
            sessions.clone(),
            data.clone(),
            workspace.clone(),
            root.path().to_path_buf(),
            "owner".into(),
            old.clone(),
        )
        .await;
    assert!(matches!(rejected,Err(SessionError::Conflict(reason)) if reason.contains("identity")));
    let tools = PilotTools {
        application: crate::application::RunApplication::new(data.clone(), root.path().to_owned()),
        graph_name: "pilot".into(),
        catalog_root: root.path().to_owned(),
        library_root: root.path().to_owned(),
        data_root: data.clone(),
        workspace_root: workspace,
        sessions: sessions.clone(),
        owner: "owner".into(),
        session: "reused".into(),
        turn: String::new(),
        context: None,
    };
    let rejected = service
        .admit(PilotAdmission {
            sessions: sessions.clone(),
            owner: "owner".into(),
            session: old.clone(),
            request_id: "stale".into(),
            prompt: Some("develop".into()),
            root: scope(&data, "owner", &old),
            tools,
        })
        .await;
    assert!(
        matches!(rejected,Err(AdmissionError::Session(SessionError::Conflict(reason))) if reason.contains("identity"))
    );
    assert_eq!(
        sessions.get("owner", "reused").unwrap().created_at,
        current.created_at
    );
    assert!(sessions.list_turns("owner", "reused").unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(task.join("keep")).unwrap(),
        "new Session"
    );
    assert!(service.active.lock().unwrap().is_empty());
}

//! End-to-end tests for deterministic Op routing through the production Host.
use anchor_kernel::{FileStore, Store};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Child, Command, Stdio},
};

fn graph(gate_command: &str) -> Value {
    json!({
        "objective":"deterministic Op routing",
        "entry":"gate",
        "ops":{
            "gate":{"run":gate_command},
            "branch":{"run":"true"}
        },
        "nodes":[
            {"id":"gate","op":"gate"},
            {"id":"good","op":"branch"},
            {"id":"bad","op":"branch"}
        ],
        "edges":[
            {"from":"gate","to":"good"},
            {"from":"gate","to":"bad"}
        ]
    })
}

fn spawn(root: &Path, run_id: &str, graph: &Value, input: &Value) -> Child {
    spawn_with_library(root, run_id, graph, input, None)
}

/// Same Host entry, with `allowed` as the deployment's command authority. A
/// test that exercises a documented example needs the interpreter that example
/// declares, not the narrower default.
fn spawn_allowed(root: &Path, run_id: &str, graph: &Value, input: &Value, allowed: &str) -> Child {
    let bundle = root.join("bundle");
    fs::create_dir_all(&bundle).unwrap();
    fs::write(bundle.join("graph.json"), graph.to_string()).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{"format":1,"graph":"graph.json","plugins":[]}"#,
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_anchor-runner-host"))
        .env_remove("ANCHOR_RUNNER_LIBRARY_ROOT")
        .env("ANCHOR_RUNNER_STATE_ROOT", root.join("state"))
        .env("ANCHOR_RUNNER_WORKSPACE_ROOT", root.join("work"))
        .env("ANCHOR_RUNNER_BUNDLE_ROOT", bundle)
        .env("ANCHOR_RUNNER_ALLOWED_COMMANDS", allowed)
        .env_remove("ANCHOR_MODEL_API_KEY")
        .env_remove("ANCHOR_MODEL_URL")
        .env_remove("ANCHOR_MODEL_NAME")
        .env_remove("ANCHOR_RUST_MCP_SERVERS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let bytes = serde_json::to_vec(&json!({
        "op":"start_bundle", "version":1, "request_id":run_id,
        "run_id":run_id, "input":input
    }))
    .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    stdin.write_all(&bytes).unwrap();
    drop(stdin);
    child
}

fn spawn_with_library(
    root: &Path,
    run_id: &str,
    graph: &Value,
    input: &Value,
    library: Option<&Path>,
) -> Child {
    let bundle = root.join("bundle");
    fs::create_dir_all(&bundle).unwrap();
    fs::write(bundle.join("graph.json"), graph.to_string()).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{"format":1,"graph":"graph.json","plugins":[]}"#,
    )
    .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_anchor-runner-host"));
    command.env_remove("ANCHOR_RUNNER_LIBRARY_ROOT");
    if let Some(library) = library {
        command.env("ANCHOR_RUNNER_LIBRARY_ROOT", library);
    }
    let mut child = command
        .env("ANCHOR_RUNNER_STATE_ROOT", root.join("state"))
        .env("ANCHOR_RUNNER_WORKSPACE_ROOT", root.join("work"))
        .env("ANCHOR_RUNNER_BUNDLE_ROOT", bundle)
        .env("ANCHOR_RUNNER_ALLOWED_COMMANDS", "sh,true")
        .env_remove("ANCHOR_MODEL_API_KEY")
        .env_remove("ANCHOR_MODEL_URL")
        .env_remove("ANCHOR_MODEL_NAME")
        .env_remove("ANCHOR_RUST_MCP_SERVERS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let bytes = serde_json::to_vec(&json!({
        "op":"start_bundle",
        "version":1,
        "request_id":run_id,
        "run_id":run_id,
        "input":input
    }))
    .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    stdin.write_all(&bytes).unwrap();
    drop(stdin);
    child
}

#[test]
fn operator_library_tools_reach_op_without_graph_host_path_grants() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let library = root.path().join("library");
    let tool = library.join("tools/sample");
    fs::create_dir_all(&tool).unwrap();
    fs::write(tool.join("run.sh"), "#!/bin/sh\nprintf external-tool\n").unwrap();
    fs::set_permissions(tool.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(tool.join("tool.json"), r#"{"entrypoint":"run.sh"}"#).unwrap();
    let definition = json!({
        "entry":"first", "ops":{"tool":{"run":"/tools/sample/run"}},
        "nodes":[{"id":"first","op":"tool"}], "edges":[]
    });
    let denied = finish(spawn(root.path(), "no-grant", &definition, &json!({})));
    assert_eq!(denied["projection"]["outcome"], "failed", "{denied}");
    assert!(
        denied["projection"]["reason"]
            .as_str()
            .unwrap()
            .contains("exit_code=Some(127)"),
        "{denied}"
    );
    let response = finish(spawn_with_library(
        root.path(),
        "granted",
        &definition,
        &json!({}),
        Some(&library),
    ));
    assert_eq!(response["projection"]["outcome"], "done", "{response}");
    assert_eq!(
        record(root.path(), "granted")["results"]["first"][0]["completion"]["submission"],
        "external-tool"
    );
}

fn finish(child: Child) -> Value {
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout[4..]).unwrap()
}

fn record(root: &Path, run_id: &str) -> Value {
    FileStore::new(root.join("state/kernel-runs"))
        .unwrap()
        .load(&anchor_runtime::graph::validator::run_resource_id(run_id))
        .unwrap()
        .unwrap()
        .state
}

#[test]
fn successful_op_marker_routes_one_branch_and_invalid_or_failed_choices_stop() {
    let root = tempfile::tempdir().unwrap();
    let scenarios = [
        (
            "routed",
            r#"sh -c 'test "$ANCHOR_ROUTES" = "good,bad" && printf "ANCHOR_ROUTE: good\ninput=%s\n" "$ANCHOR_INPUT"'"#,
            "done",
            None,
        ),
        (
            "missing-route",
            r#"sh -c 'printf "check passed but no choice\n"'"#,
            "failed",
            Some("multiple exits require route"),
        ),
        (
            "invalid-route",
            r#"sh -c 'printf "ANCHOR_ROUTE: elsewhere\n"'"#,
            "failed",
            Some("route `elsewhere` is not an exit"),
        ),
        (
            "failed-with-marker",
            r#"sh -c 'printf "ANCHOR_ROUTE: good\n"; exit 1'"#,
            "failed",
            Some("exit_code=Some(1)"),
        ),
    ];

    for (run_id, gate_command, expected_outcome, expected_error) in scenarios {
        let response = finish(spawn(
            root.path(),
            run_id,
            &graph(gate_command),
            &json!({"release":"binary"}),
        ));
        assert_eq!(
            response["projection"]["outcome"], expected_outcome,
            "{response}"
        );
        let saved = record(root.path(), run_id);
        if let Some(expected_error) = expected_error {
            assert!(
                response["projection"]["reason"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(expected_error),
                "{saved}"
            );
            assert!(saved["results"].get("good").is_none());
            assert!(saved["results"].get("bad").is_none());
        } else {
            assert_eq!(saved["results"]["gate"][0]["completion"]["route"], "good");
            assert_eq!(
                saved["results"]["gate"][0]["completion"]["submission"],
                "input={\"release\":\"binary\"}"
            );
            assert_eq!(saved["results"]["good"].as_array().unwrap().len(), 1);
            assert!(saved["results"].get("bad").is_none());
        }
    }
}

#[test]
fn command_input_contains_merged_run_input_without_artifact_metadata() {
    let root = tempfile::tempdir().unwrap();
    let definition = json!({
        "objective":"pass effective input to each command",
        "entry":"first",
        "input":{"window":{"start":"default","end":"default"},"tags":["default"]},
        "ops":{"echo":{"run":r#"sh -c 'printf "%s" "$ANCHOR_INPUT"'"#}},
        "nodes":[{"id":"first","op":"echo"},{"id":"second","op":"echo"}],
        "edges":[{"from":"first","to":"second"}]
    });
    let input = json!({"window":{"end":"实际日期"},"tags":["a b","quote\""],"input":null});
    let expected = json!({
        "window":{"start":"default","end":"实际日期"},
        "tags":["a b","quote\""],
        "input":null
    });
    let response = finish(spawn(root.path(), "input", &definition, &input));
    assert_eq!(response["projection"]["outcome"], "done", "{response}");
    let saved = record(root.path(), "input");
    assert_eq!(saved["input"], expected);
    for node in ["first", "second"] {
        let submission = saved["results"][node][0]["completion"]["submission"]
            .as_str()
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(submission).unwrap(), expected);
    }
}

#[test]
fn original_shell_assignment_case_heredoc_and_redirect_use_native_route_helper() {
    let root = tempfile::tempdir().unwrap();
    let command = r#"set -eu
verdict=approved
cat > report.txt <<'REPORT'
quoted "content" and literal $verdict
second line
REPORT
case "$verdict" in
  approved) anchor-route --to good --reason "checked shell result" ;;
  *) anchor-route --to bad --reason rejected ;;
esac"#;
    let mut definition = graph(command);
    definition["ops"]["branch"]["run"] = json!("cat /in/gate/report.txt");
    let response = finish(spawn(
        root.path(),
        "original-shell",
        &definition,
        &json!({}),
    ));
    assert_eq!(response["projection"]["outcome"], "done", "{response}");
    let saved = record(root.path(), "original-shell");
    assert_eq!(saved["snapshot"]["ops"]["gate"]["run"], command);
    assert_eq!(saved["results"]["gate"][0]["completion"]["route"], "good");
    assert_eq!(
        saved["results"]["gate"][0]["completion"]["submission"],
        "checked shell result"
    );
    assert_eq!(
        saved["results"]["good"][0]["completion"]["submission"],
        "quoted \"content\" and literal $verdict\nsecond line"
    );
    assert!(saved["results"].get("bad").is_none());

    for (run_id, command) in [
        (
            "helper-invalid",
            "anchor-route --to elsewhere --reason rejected",
        ),
        (
            "helper-nonzero",
            "anchor-route --to good --reason incomplete; exit 7",
        ),
        ("shell-malformed", "printf 'unfinished"),
    ] {
        let response = finish(spawn(root.path(), run_id, &graph(command), &json!({})));
        assert_eq!(response["projection"]["outcome"], "failed", "{response}");
        let saved = record(root.path(), run_id);
        assert!(saved["results"].get("good").is_none());
        assert!(saved["results"].get("bad").is_none());
    }
}

#[test]
fn op_network_declaration_controls_business_network_access() {
    use std::{
        io::Read,
        net::{TcpListener, UdpSocket},
        time::{Duration, Instant},
    };
    let root = tempfile::tempdir().unwrap();
    let route = UdpSocket::bind("0.0.0.0:0").unwrap();
    route.connect("192.0.2.1:9").unwrap();
    let business_address = route.local_addr().unwrap().ip();
    let listener = TcpListener::bind((business_address, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let shell = format!(
        "bash -c 'printf business-network-authorized > /dev/tcp/{business_address}/{port}'"
    );
    let mut definition = json!({
        "entry":"network", "ops":{"connect":{"run":shell,"network":false}},
        "nodes":[{"id":"network","op":"connect"}], "edges":[]
    });
    let denied = finish(spawn(
        root.path(),
        "network-disabled",
        &definition,
        &json!({}),
    ));
    assert_eq!(denied["projection"]["outcome"], "failed", "{denied}");
    assert!(listener.accept().is_err());

    let received = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut text = String::new();
                    stream.read_to_string(&mut text).unwrap();
                    return text;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "authorized command never connected"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("business network fixture failed: {error}"),
            }
        }
    });
    definition["ops"]["connect"]["network"] = json!(true);
    let allowed = finish(spawn(
        root.path(),
        "network-enabled",
        &definition,
        &json!({}),
    ));
    assert_eq!(allowed["projection"]["outcome"], "done", "{allowed}");
    assert_eq!(received.join().unwrap(), "business-network-authorized");
}

/// Design card 005: a Run's delegated bots are decided from its own Graph
/// identity, and the sandbox mounts exactly those. `report` may speak through
/// `weekly`; nobody delegates to `other`, and `daily` names a different Graph.
mod delegated_channel {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::{
        io::{BufRead, BufReader},
        os::unix::{fs::PermissionsExt, net::UnixListener},
        path::PathBuf,
    };

    /// The Host's own state directory for a Graph's gateway.
    fn gateway_directory(state: &Path, graph: &str) -> PathBuf {
        let digest = format!("{:x}", Sha256::digest(graph.as_bytes()));
        state.join("channels/graphs").join(&digest[..16])
    }

    fn binding(state: &Path, bot: &str, delegated: &[&str], enabled: bool) {
        let directory = state.join("platform/channels/graphs");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!("{bot}.json"));
        fs::write(
            &path,
            json!({
                "format": 2, "owner": "local", "plugin": "gateway",
                "users": ["alice"], "send_users": ["alice"],
                "delegate_graphs": delegated, "enabled": enabled,
                "authorization_digest": "0".repeat(64),
                "callback_key": format!("anchor-channel-api-{}", "0".repeat(64)),
            })
            .to_string(),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    /// A running gateway: a real socket that acknowledges one request by its
    /// id, as the real gateway does, and hands back what it received.
    fn gateway(state: &Path, bot: &str) -> std::sync::mpsc::Receiver<String> {
        let directory = gateway_directory(state, bot);
        fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("control.sock");
        fs::write(
            directory.join("control.json"),
            json!({"socket": socket.display().to_string(), "token": "t".repeat(40)}).to_string(),
        )
        .unwrap();
        // A second gateway for the same bot replaces the first one's socket
        // path; unlinking is what the real supervisor does on restart.
        let _ = fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut line = String::new();
            let _ = BufReader::new(&stream).read_line(&mut line);
            let request_id = serde_json::from_str::<Value>(&line)
                .ok()
                .and_then(|request| request["request_id"].as_str().map(str::to_owned))
                .unwrap_or_default();
            let _ = sender.send(line);
            let ack = json!({"accepted": true, "request_id": request_id});
            let _ = stream.write_all(format!("{ack}\n").as_bytes());
            let _ = stream.flush();
            // Hold the connection open until the sandboxed helper has read the
            // reply; dropping it immediately would race the read.
            std::thread::sleep(std::time::Duration::from_millis(200));
        });
        receiver
    }

    /// A `start_bundle` Run that records a Run identity for `graph`, which is
    /// what the Host reads to decide the delegated set.
    fn spawn_as_graph(root: &Path, run_id: &str, graph: &str, definition: &Value) -> Child {
        let local = root.join("local-inputs");
        fs::create_dir_all(&local).unwrap();
        let bundle = root.join(format!("bundle-{run_id}"));
        fs::create_dir_all(&bundle).unwrap();
        fs::write(bundle.join("graph.json"), definition.to_string()).unwrap();
        fs::write(
            bundle.join("manifest.json"),
            r#"{"format":1,"graph":"graph.json","plugins":[]}"#,
        )
        .unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_anchor-runner-host"))
            .env_remove("ANCHOR_RUNNER_LIBRARY_ROOT")
            .env("ANCHOR_RUNNER_STATE_ROOT", root.join("state"))
            .env("ANCHOR_RUNNER_WORKSPACE_ROOT", root.join("work"))
            .env("ANCHOR_RUNNER_BUNDLE_ROOT", bundle)
            .env("ANCHOR_RUNNER_LOCAL_INPUTS_ROOT", &local)
            .env("ANCHOR_RUNNER_GRAPH_NAME", graph)
            .env("ANCHOR_RUNNER_ALLOWED_COMMANDS", "sh,true")
            .env_remove("ANCHOR_MODEL_API_KEY")
            .env_remove("ANCHOR_MODEL_URL")
            .env_remove("ANCHOR_MODEL_NAME")
            .env_remove("ANCHOR_RUST_MCP_SERVERS")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let bytes = serde_json::to_vec(&json!({
            "op":"start_bundle", "version":1, "request_id":run_id,
            "run_id":run_id, "input":{}
        }))
        .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .unwrap();
        stdin.write_all(&bytes).unwrap();
        drop(stdin);
        child
    }

    fn probe(command: &str) -> Value {
        json!({
            "entry": "probe",
            "ops": {"probe": {"run": command}},
            "nodes": [{"id": "probe", "op": "probe"}],
            "edges": []
        })
    }

    fn submission(root: &Path, run_id: &str) -> String {
        record(root, run_id)["results"]["probe"][0]["completion"]["submission"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// T20/T33: the delegated Graph's Op reaches the gateway, and the receipt
    /// carries only the accepted flag and the request id.
    #[test]
    fn a_delegated_graph_can_send_through_the_bot_it_was_granted() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        binding(&state, "weekly", &["report"], true);
        let received = gateway(&state, "weekly");
        let response = finish(spawn_as_graph(
            root.path(),
            "delegated-send",
            "report",
            &probe("printf 'weekly is ready' | anchor-channel-send --bot weekly --userid alice"),
        ));
        assert_eq!(response["projection"]["outcome"], "done", "{response}");
        let received = received
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let request: Value = serde_json::from_str(&received).unwrap();
        assert_eq!(request["operation"], "send");
        assert_eq!(request["userid"], "alice");
        assert_eq!(request["content"], "weekly is ready");
        let receipt: Value = serde_json::from_str(&submission(root.path(), "delegated-send"))
            .expect("receipt is one JSON line");
        assert_eq!(receipt["accepted"], true);
        assert_eq!(receipt["request_id"], request["request_id"]);
        let printed = receipt.to_string();
        assert!(!printed.contains(&"t".repeat(40)), "{printed}");
        assert!(!printed.contains("control.json"), "{printed}");
    }

    /// T26 across Runs: the second week's report repeats last week's wording.
    /// Two Runs must produce two distinct request ids, or the gateway would
    /// acknowledge the second one as a duplicate without sending it.
    #[test]
    fn the_same_text_in_a_new_run_reaches_the_gateway_again() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        binding(&state, "weekly", &["report"], true);

        let mut ids = Vec::new();
        for run in ["week-one", "week-two"] {
            let received = gateway(&state, "weekly");
            let response = finish(spawn_as_graph(
                root.path(),
                run,
                "report",
                &probe(
                    "printf 'weekly is ready' | anchor-channel-send --bot weekly --userid alice",
                ),
            ));
            assert_eq!(response["projection"]["outcome"], "done", "{response}");
            let received = received
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            let request: Value = serde_json::from_str(&received).unwrap();
            ids.push(request["request_id"].as_str().unwrap().to_owned());
            // Both Runs sent the identical body; only their identity differs.
            assert_eq!(request["content"], "weekly is ready");
        }
        assert_ne!(ids[0], ids[1], "{ids:?}");
    }

    /// T21: nothing is mounted for a Graph that no enabled binding names, and
    /// the helper refuses instead of discovering a gateway by itself.
    #[test]
    fn an_undelegated_graph_sees_no_channel_mount() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        binding(&state, "weekly", &["report"], true);
        let _received = gateway(&state, "weekly");
        let response = finish(spawn_as_graph(
            root.path(),
            "no-delegation",
            "other",
            &probe("printf hi | anchor-channel-send --bot weekly --userid alice"),
        ));
        assert_eq!(response["projection"]["outcome"], "failed", "{response}");
        let reason = response["projection"]["reason"].as_str().unwrap();
        assert!(reason.contains("no bot is delegated"), "{response}");
    }

    /// T24: an enabled binding whose list names the Run's Graph is still not a
    /// grant once the bot is stopped.
    #[test]
    fn a_stopped_bot_grants_nothing() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        binding(&state, "weekly", &["report"], false);
        let _received = gateway(&state, "weekly");
        let response = finish(spawn_as_graph(
            root.path(),
            "stopped-bot",
            "report",
            &probe("printf hi | anchor-channel-send --bot weekly --userid alice"),
        ));
        assert_eq!(response["projection"]["outcome"], "failed", "{response}");
        assert!(
            response["projection"]["reason"]
                .as_str()
                .unwrap()
                .contains("no bot is delegated"),
            "{response}"
        );
    }

    /// T25: a delegated Run naming a bot it was not granted is refused, even
    /// though it holds a valid grant for a different one.
    #[test]
    fn a_delegated_run_cannot_name_a_bot_it_was_not_granted() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        binding(&state, "weekly", &["report"], true);
        binding(&state, "daily", &["somebody-else"], true);
        let _weekly = gateway(&state, "weekly");
        let _daily = gateway(&state, "daily");
        let response = finish(spawn_as_graph(
            root.path(),
            "wrong-bot",
            "report",
            &probe("printf hi | anchor-channel-send --bot daily --userid alice"),
        ));
        assert_eq!(response["projection"]["outcome"], "failed", "{response}");
        assert!(
            response["projection"]["reason"]
                .as_str()
                .unwrap()
                .contains("no bot is delegated"),
            "{response}"
        );
    }

    /// T34: the documented example's validation step runs as written. It reads
    /// its two targets from the Run input and refuses an empty one, so a
    /// wrapper missing its destination fails before the report is generated.
    #[test]
    fn the_notify_example_validates_its_destination_before_running() {
        let example: Value = serde_json::from_str(include_str!(
            "../../../examples/graphs/weekly-work-report-notify.json"
        ))
        .unwrap();
        let validate = example["ops"]["validate"]["run"].as_str().unwrap();
        let definition = json!({
            "entry": "validate",
            "ops": {"validate": {"run": validate}},
            "nodes": [{"id": "validate", "op": "validate"}],
            "edges": []
        });

        let root = tempfile::tempdir().unwrap();
        let refused = finish(spawn_allowed(
            root.path(),
            "notify-missing",
            &definition,
            &json!({"notify_bot": "weekly", "notify_userid": "  "}),
            "sh,true,python3",
        ));
        assert_eq!(refused["projection"]["outcome"], "failed", "{refused}");

        let root = tempfile::tempdir().unwrap();
        let accepted = finish(spawn_allowed(
            root.path(),
            "notify-ok",
            &definition,
            &json!({"notify_bot": "weekly", "notify_userid": "alice"}),
            "sh,true,python3",
        ));
        assert_eq!(accepted["projection"]["outcome"], "done", "{accepted}");
    }
}

//! Isolated fixed-Goose API experiment. This does not change Anchor's runtime.
#[allow(dead_code)]
#[path = "support/runtime_fixture.rs"]
mod fixture;
#[allow(dead_code)]
#[path = "support/goose_fixture.rs"]
mod goose;

use goose::{Provider, Step};
use rmcp::{
    RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResult, ContentBlock, ElicitRequestParams, ErrorData,
        ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
    transport::{
        StreamableHttpServerConfig, StreamableHttpService,
        streamable_http_server::session::local::LocalSessionManager,
    },
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{Notify, Semaphore},
    time::timeout,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct Observations {
    initializes: Arc<AtomicUsize>,
    lists: Arc<AtomicUsize>,
    calls: Arc<Mutex<Vec<Value>>>,
    entered: Arc<Notify>,
    release: Arc<Semaphore>,
    wait_cancelled: Arc<AtomicBool>,
}

impl Default for Observations {
    fn default() -> Self {
        Self {
            initializes: Arc::new(AtomicUsize::new(0)),
            lists: Arc::new(AtomicUsize::new(0)),
            calls: Arc::new(Mutex::new(Vec::new())),
            entered: Arc::new(Notify::new()),
            release: Arc::new(Semaphore::new(0)),
            wait_cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[derive(Clone)]
struct Service {
    gateway: bool,
    noise_tools: usize,
    observations: Observations,
}

impl ServerHandler for Service {
    fn get_info(&self) -> ServerInfo {
        self.observations.initializes.fetch_add(1, Ordering::SeqCst);
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.observations.lists.fetch_add(1, Ordering::SeqCst);
        let names: &[&str] = if self.gateway {
            &["probe_activate"]
        } else {
            &[
                "native_echo",
                "native_confirm",
                "native_error",
                "native_wait",
            ]
        };
        let mut tools: Vec<Tool> = names
            .iter()
            .map(|name| {
                Tool::new(
                    (*name).to_owned(),
                    format!("Local native activation probe: {name}"),
                    Arc::new(
                        serde_json::from_value(if *name == "native_echo" {
                            json!({"type":"object","properties":{"value":{"type":"string"}},"required":["value"],"additionalProperties":false})
                        } else {
                            json!({"type":"object","properties":{},"additionalProperties":false})
                        })
                        .unwrap(),
                    ),
                )
            })
            .collect();
        for index in 0..self.noise_tools {
            tools.push(Tool::new(
                format!("native_noise_{index:04}"),
                "Unrelated local fixture operation; do not use for this task.".to_owned(),
                Arc::new(serde_json::from_value(json!({"type":"object","properties":{}})).unwrap()),
            ));
        }
        Ok(ListToolsResult {
            tools,
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.observations.calls.lock().unwrap().push(json!({
            "request":request,"context_meta":context.meta,
        }));
        let value = match request.name.as_ref() {
            "probe_activate" => {
                self.observations.entered.notify_one();
                self.observations.release.acquire().await.unwrap().forget();
                json!({"activated":true})
            }
            "native_echo" => json!({"echo":request.arguments.as_ref().unwrap()["value"]}),
            "native_confirm" => {
                let answer = context.peer.create_elicitation(ElicitRequestParams::FormElicitationParams {
                    meta:None,
                    message:"Approve the local probe?".into(),
                    requested_schema:serde_json::from_value(json!({"type":"object","properties":{"approve":{"type":"boolean"}},"required":["approve"]})).unwrap(),
                }).await.map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
                json!({"native_answer":answer})
            }
            "native_error" => {
                return Ok(CallToolResult::error(vec![ContentBlock::text(
                    "EXPECTED_BUSINESS_ERROR",
                )]));
            }
            "native_wait" => {
                self.observations.entered.notify_one();
                tokio::select! {
                    _ = context.ct.cancelled() => {
                        self.observations.wait_cancelled.store(true, Ordering::SeqCst);
                    }
                    permit = self.observations.release.acquire() => permit.unwrap().forget(),
                }
                json!({"wait_settled":true})
            }
            _ => return Err(ErrorData::invalid_params("unknown probe tool", None)),
        };
        Ok(CallToolResult::structured(value))
    }
}

struct Endpoint {
    url: String,
    observations: Observations,
    stopped: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl Endpoint {
    async fn start(gateway: bool, noise_tools: usize) -> Self {
        let observations = Observations::default();
        let service = Service {
            gateway,
            noise_tools,
            observations: observations.clone(),
        };
        let stopped = CancellationToken::new();
        let mcp = StreamableHttpService::new(
            move || Ok(service.clone()),
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig::default().with_cancellation_token(stopped.child_token()),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let stop = stopped.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, axum::Router::new().nest_service("/mcp", mcp))
                .with_graceful_shutdown(stop.cancelled_owned())
                .await
                .unwrap();
        });
        Self {
            url,
            observations,
            stopped,
            task,
        }
    }

    fn extension(&self, name: &str) -> Value {
        json!({"type":"mcp","server":{"type":"http","name":name,"url":self.url,"headers":[]}})
    }

    async fn close(self) {
        self.observations.release.add_permits(2);
        self.stopped.cancel();
        timeout(Duration::from_secs(5), self.task)
            .await
            .unwrap()
            .unwrap();
    }
}

/// Probe-only multiplexing of ACP request IDs, including out-of-band mutations.
/// Production AcpConnection intentionally has one outstanding request.
struct Probe {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next: u64,
    ready: BTreeMap<u64, Value>,
    frames: Vec<Value>,
}

impl Probe {
    async fn spawn(root: &Path, provider: &Provider, suffix: &str) -> Self {
        let binary = PathBuf::from(std::env::var_os("ANCHOR_GOOSE_BINARY").unwrap());
        assert_eq!(goose::digest(&binary), goose::GOOSE_SHA256);
        let home = root.join("home");
        fs::create_dir_all(root.join("config")).unwrap();
        fs::create_dir_all(&home).unwrap();
        let config = root.join("config/config.yaml");
        if !config.exists() {
            fs::write(&config,"GOOSE_MODE: auto\nextensions:\n  developer:\n    name: developer\n    type: builtin\n    enabled: false\n").unwrap();
        }
        let mut child = Command::new(binary)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &home)
            .env("GOOSE_PATH_ROOT", root)
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("GOOSE_PROVIDER", "openai")
            .env("GOOSE_MODEL", &provider.model)
            .env("OPENAI_HOST", &provider.url)
            .env("OPENAI_BASE_PATH", "v1/chat/completions")
            .env("OPENAI_API_KEY", "fixture-only")
            .env("GOOSE_MODE", "auto")
            .env("GOOSE_DISABLE_SESSION_NAMING", "true")
            .env("GOOSE_TELEMETRY_OFF", "1")
            .env("RUST_LOG", "warn")
            .current_dir(&home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(fs::File::create(root.join(format!("stderr-{suffix}.log"))).unwrap())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut probe = Self {
            child,
            stdin,
            stdout,
            next: 1,
            ready: BTreeMap::new(),
            frames: Vec::new(),
        };
        let initialized = probe.request("initialize",json!({"protocolVersion":1,"clientCapabilities":{"_meta":{"goose":{"customNotifications":true}},"elicitation":{"form":{}}},"clientInfo":{"name":"anchor-native-activation-probe","version":"0.1"}})).await;
        assert_eq!(initialized["result"]["agentInfo"]["version"], "1.53.0");
        probe
    }

    async fn write(&mut self, value: Value) {
        self.frames
            .push(json!({"direction":"client","frame":value}));
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        self.stdin
            .as_mut()
            .unwrap()
            .write_all(&bytes)
            .await
            .unwrap();
        self.stdin.as_mut().unwrap().flush().await.unwrap();
    }

    async fn send(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next;
        self.next += 1;
        self.write(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await;
        id
    }

    async fn receive(&mut self, id: u64) -> Value {
        if let Some(response) = self.ready.remove(&id) {
            return response;
        }
        timeout(Duration::from_secs(25), async {
            loop {
                let mut line=String::new();
                assert!(self.stdout.read_line(&mut line).await.unwrap()>0,"Goose closed stdout");
                let frame:Value=serde_json::from_str(&line).unwrap();
                self.frames.push(json!({"direction":"server","frame":frame}));
                if frame.get("method").is_some() {
                    if let Some(request_id)=frame.get("id") {
                        let result=match frame["method"].as_str().unwrap() {
                            "elicitation/create"=>json!({"action":"accept","content":{"approve":true}}),
                            "session/request_permission"=> {
                                let options=frame["params"]["options"].as_array().unwrap();
                                let option=options.iter().find(|v|v["kind"]=="allow_once").unwrap();
                                json!({"outcome":{"outcome":"selected","optionId":option["optionId"]}})
                            }
                            other=>panic!("Unexpected Goose request: {other}: {frame}"),
                        };
                        self.write(json!({"jsonrpc":"2.0","id":request_id,"result":result})).await;
                    }
                    continue;
                }
                let received=frame["id"].as_u64().unwrap();
                if received==id { return frame; }
                self.ready.insert(received,frame);
            }
        }).await.expect("native ACP probe response timeout")
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params).await;
        let response = self.receive(id).await;
        assert!(response.get("error").is_none(), "{method}: {response}");
        response
    }

    async fn prompt(&mut self, session: &str, message: &str) -> Value {
        self.request(
            "session/prompt",
            json!({"sessionId":session,"prompt":[{"type":"text","text":message}]}),
        )
        .await
    }

    async fn close(mut self, root: &Path, suffix: &str) {
        fs::write(
            root.join(format!("acp-{suffix}.json")),
            serde_json::to_vec_pretty(&self.frames).unwrap(),
        )
        .unwrap();
        self.stdin.take();
        if timeout(Duration::from_secs(5), self.child.wait())
            .await
            .is_err()
        {
            self.child.kill().await.unwrap();
        }
    }
}

fn has_tool(request: &Value, name: &str) -> bool {
    goose::tool_definition(request, name).is_ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "isolated fixed-Goose native API experiment; requires ANCHOR_GOOSE_BINARY"]
async fn fixed_goose_native_activation_records_runtime_semantics() {
    let provider = Provider::new(
        "goose-native-activation-spike",
        vec![
            Step::tool("probe_activate", json!({})),
            Step::text("Activation complete.").after("activated"),
            Step::tool("native_echo", json!({"value":"next-prompt-marker"})),
            Step::tool("native_confirm", json!({})).after("next-prompt-marker"),
            Step::tool("native_error", json!({})).after("approve"),
            Step::text("Business error observed.").after("EXPECTED_BUSINESS_ERROR"),
            Step::tool("native_wait", json!({})),
        ],
    );
    let root = provider.root.join("native");
    let gateway = Endpoint::start(true, 0).await;
    let selected = Endpoint::start(false, 0).await;
    let mut probe = Probe::spawn(&root, &provider, "first").await;
    let cwd = root.join("home");
    let initial = gateway.extension("anchor-probe");
    let params = json!({"cwd":cwd,"mcpServers":[initial["server"]],"_meta":{"hidden":true,"sessionTitle":"Native activation experiment","enabledExtensions":[initial,{"type":"builtin","name":"tom"}]}});
    let new = probe.request("session/new", params.clone()).await;
    let session = new["result"]["sessionId"].as_str().unwrap().to_owned();
    let listed = probe
        .request(
            "_goose/unstable/session/extensions/list",
            json!({"sessionId":session}),
        )
        .await;
    for extension in listed["result"]["extensions"].as_array().unwrap() {
        let key = extension["extensionKey"].as_str().unwrap();
        if extension["extension"]["server"]["name"] != "anchor-probe"
            && extension["extension"]["name"] != "tom"
        {
            probe
                .request(
                    "_goose/unstable/session/extensions/remove",
                    json!({"sessionId":session,"extensionKey":key}),
                )
                .await;
        }
    }
    let config_before = goose::digest(&root.join("config/config.yaml"));
    assert_eq!(selected.observations.initializes.load(Ordering::SeqCst), 0);
    let prompt=probe.send("session/prompt",json!({"sessionId":session,"prompt":[{"type":"text","text":"Activate the selected local service."}]})).await;
    timeout(
        Duration::from_secs(15),
        gateway.observations.entered.notified(),
    )
    .await
    .unwrap();
    let added = probe
        .request(
            "_goose/unstable/session/extensions/add",
            json!({"sessionId":session,"extension":selected.extension("picked")}),
        )
        .await;
    assert!(selected.observations.initializes.load(Ordering::SeqCst) > 0);
    gateway.observations.release.add_permits(1);
    let first = probe.receive(prompt).await;
    assert_eq!(first["result"]["stopReason"], "end_turn");
    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    assert!(!has_tool(&requests[0], "native_echo"));
    let same_prompt_refresh = has_tool(&requests[1], "native_echo");
    probe.prompt(&session,"Use the newly activated tools, ask for native confirmation, and report the business error.").await;
    let requests = provider.requests();
    assert!(has_tool(&requests[2], "native_echo"));
    assert!(
        probe
            .frames
            .iter()
            .any(|row| row["frame"]["method"] == "elicitation/create")
    );
    let prompt=probe.send("session/prompt",json!({"sessionId":session,"prompt":[{"type":"text","text":"Start the cancellable local tool."}]})).await;
    timeout(
        Duration::from_secs(15),
        selected.observations.entered.notified(),
    )
    .await
    .unwrap();
    probe
        .write(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}}))
        .await;
    let cancelled = probe.receive(prompt).await;
    assert_eq!(cancelled["result"]["stopReason"], "cancelled");
    let _ = timeout(Duration::from_secs(2), async {
        while !selected.observations.wait_cancelled.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    let native_tool_cancelled = selected.observations.wait_cancelled.load(Ordering::SeqCst);
    selected.observations.release.add_permits(1);
    let before_reload = selected.observations.initializes.load(Ordering::SeqCst);
    let retained = probe
        .request(
            "_goose/unstable/session/extensions/list",
            json!({"sessionId":session}),
        )
        .await;
    assert!(
        retained["result"]["extensions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["extensionKey"] == "picked")
    );
    probe.close(&root, "first").await;
    let mut probe = Probe::spawn(&root, &provider, "reload").await;
    let mut load = params;
    load["sessionId"] = json!(session);
    probe.request("session/load", load).await;
    let reloaded = probe
        .request(
            "_goose/unstable/session/extensions/list",
            json!({"sessionId":session}),
        )
        .await;
    let reload_retains_selected = reloaded["result"]["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["extensionKey"] == "picked");
    let reload_reconnects_selected =
        selected.observations.initializes.load(Ordering::SeqCst) > before_reload;
    provider.append(vec![
        Step::tool("native_echo", json!({"value":"reload-marker"})),
        Step::text("Reload call complete.").after("reload-marker"),
    ]);
    probe
        .prompt(&session, "Call the retained selected service again.")
        .await;
    probe
        .request(
            "_goose/unstable/session/extensions/remove",
            json!({"sessionId":session,"extensionKey":"picked"}),
        )
        .await;
    provider.append(vec![Step::text("Selected service removed.")]);
    probe
        .prompt(&session, "Check the tool catalog after removal.")
        .await;
    let requests = provider.requests();
    assert!(!has_tool(requests.last().unwrap(), "native_echo"));
    let config_unchanged = goose::digest(&root.join("config/config.yaml")) == config_before;
    let report = json!({
        "scope":"fixed Goose native ACP API spike, deterministic local Provider and HTTP MCP; no Anchor runtime integration",
        "goose_binary_sha256":goose::GOOSE_SHA256,"real_provider":false,"deployment_updated":false,
        "initial_selected_service_unconnected":true,"add_during_active_tool_succeeded":added,
        "same_prompt_new_schema_visible":same_prompt_refresh,"next_prompt_new_schema_visible":true,
        "native_confirmation_round_trip":true,"business_error_preserved":true,
        "prompt_cancelled":cancelled,"server_observed_tool_cancellation":native_tool_cancelled,
        "reload_retains_selected_extension":reload_retains_selected,"reload_reconnects_selected":reload_reconnects_selected,
        "explicit_removal_hides_schema_next_prompt":true,"global_config_unchanged":config_unchanged,
        "native_session":session,"actual_mcp_calls":selected.observations.calls.lock().unwrap().clone(),
        "provider_requests":requests,
    });
    fs::write(
        provider.root.join("evidence.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    println!(
        "native activation evidence: {}",
        provider.root.join("evidence.json").display()
    );
    println!(
        "same_prompt_new_schema_visible={same_prompt_refresh}, server_observed_tool_cancellation={native_tool_cancelled}, reload_reconnects_selected={reload_reconnects_selected}"
    );
    provider.assert_consumed();
    probe.close(&root, "reload").await;
    gateway.close().await;
    selected.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "isolated fixed-Goose extension manager experiment; requires ANCHOR_GOOSE_BINARY"]
async fn fixed_goose_manager_activation_refreshes_schema_and_exposes_scope() {
    let provider = Provider::new(
        "goose-native-manager-spike",
        vec![
            Step::tool(
                "manage_extensions",
                json!({"action":"enable","extension_name":"picked"}),
            ),
            Step::tool("native_echo", json!({"value":"same-prompt-marker"})).after("successfully"),
            Step::text("Native manager activation complete.").after("same-prompt-marker"),
        ],
    );
    let root = provider.root.join("native");
    let selected = Endpoint::start(false, 1000).await;
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(root.join("config/config.yaml"), format!(
        "GOOSE_MODE: auto\nextensions:\n  developer:\n    name: developer\n    type: builtin\n    enabled: false\n  picked:\n    name: picked\n    type: streamable_http\n    uri: {}\n    enabled: false\n    timeout: 20\n",
        selected.url,
    )).unwrap();
    let config_before = goose::digest(&root.join("config/config.yaml"));
    let mut probe = Probe::spawn(&root, &provider, "manager").await;
    let params = json!({"cwd":root.join("home"),"mcpServers":[],"_meta":{"hidden":true,"sessionTitle":"Native manager experiment","enabledExtensions":[{"type":"builtin","name":"tom"},{"type":"platform","name":"extensionmanager"}]}});
    let created = probe.request("session/new", params).await;
    let session = created["result"]["sessionId"].as_str().unwrap().to_owned();
    let list = probe
        .request(
            "_goose/unstable/session/extensions/list",
            json!({"sessionId":session}),
        )
        .await;
    for extension in list["result"]["extensions"].as_array().unwrap() {
        let name = extension["extension"]["name"].as_str().unwrap_or_default();
        if name != "tom" && name != "extensionmanager" {
            probe
                .request(
                    "_goose/unstable/session/extensions/remove",
                    json!({"sessionId":session,"extensionKey":extension["extensionKey"]}),
                )
                .await;
        }
    }
    assert_eq!(selected.observations.initializes.load(Ordering::SeqCst), 0);
    probe
        .prompt(
            &session,
            "Enable the predeclared picked extension, then call its echo tool in this prompt.",
        )
        .await;
    let requests = provider.requests();
    assert!(!has_tool(&requests[0], "native_echo"));
    assert!(has_tool(&requests[1], "native_echo"));
    let native_tool_count = requests[1]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|tool| {
            tool["function"]["name"]
                .as_str()
                .is_some_and(|name| name.starts_with("picked__"))
        })
        .count();
    assert_eq!(native_tool_count, 1004);
    let activated = probe
        .request(
            "_goose/unstable/session/extensions/list",
            json!({"sessionId":session}),
        )
        .await;
    // The native manager also accepts disabling extensions other than itself.
    // Record that boundary using tom; no Anchor execution or authorization is involved.
    provider.append(vec![
        Step::tool(
            "manage_extensions",
            json!({"action":"disable","extension_name":"tom"}),
        ),
        Step::text("Native tom removed.").after("disabled successfully"),
    ]);
    probe
        .prompt(&session, "Disable only the probe's tom extension.")
        .await;
    let after = probe
        .request(
            "_goose/unstable/session/extensions/list",
            json!({"sessionId":session}),
        )
        .await;
    let tom_removed = !after["result"]["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|extension| extension["extensionKey"] == "tom");
    assert!(tom_removed);
    let report = json!({
        "scope":"fixed Goose native Extension Manager probe, deterministic Provider, scoped fixture config; no Anchor runtime integration",
        "goose_binary_sha256":goose::GOOSE_SHA256,"real_provider":false,"deployment_updated":false,
        "initial_selected_service_unconnected":true,"manager_refreshes_schema_same_prompt":true,
        "selected_server_tool_count":1004,"selected_tools_sent_to_next_model_request":native_tool_count,
        "model_tools_json_bytes_after_activation":serde_json::to_vec(&requests[1]["tools"]).unwrap().len(),
        "activated_extensions":activated,"manager_can_disable_other_native_extension":tom_removed,
        "extensions_after_disabling_tom":after,"global_config_unchanged":goose::digest(&root.join("config/config.yaml"))==config_before,
        "actual_mcp_calls":selected.observations.calls.lock().unwrap().clone(),"provider_requests":provider.requests(),
    });
    fs::write(
        provider.root.join("evidence.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    println!(
        "native manager evidence: {}",
        provider.root.join("evidence.json").display()
    );
    provider.assert_consumed();
    probe.close(&root, "manager").await;
    selected.close().await;
}

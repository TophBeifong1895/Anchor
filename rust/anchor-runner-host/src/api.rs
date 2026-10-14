mod assistant_recovery;
#[allow(clippy::result_large_err)]
mod channel_sessions;
mod credential_requests;
mod files;
mod graphs;
#[allow(clippy::result_large_err)]
pub(crate) mod oauth;
mod plugins;
#[allow(clippy::result_large_err)]
mod responses;
#[allow(clippy::result_large_err)]
mod runs;
mod schedules;
#[allow(clippy::result_large_err)]
mod sessions;
#[cfg(test)]
mod tests;
mod timeline;
#[allow(clippy::result_large_err)]
mod turns;
mod webhooks;
#[allow(clippy::result_large_err)]
mod wecom;
mod wecom_progress;
use super::*;
use crate::application::{ApplicationError, RunApplication};
use anchor_graph_host::{FileGraphBundleLoader, FilePluginCatalog, PluginDefinition};
use anchor_runtime::graph::{FileRunStore, RunStatus};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path as AxumPath, Query, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response as HttpResponse},
    routing::{get, post},
};
use channel_sessions::*;
use files::*;
use graphs::*;
use oauth::*;
use plugins::*;
use responses::*;
use runs::*;
use schedules::*;
use serde_json::{Value, json};
use sessions::*;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env, io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use timeline::*;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::services::ServeDir;
use turns::*;
use webhooks::*;

#[derive(Clone)]
struct ApiState {
    #[cfg(test)]
    bundle_root: PathBuf,
    catalog_root: PathBuf,
    data_root: PathBuf,
    workspace_root: PathBuf,
    graph_name: String,
    application: RunApplication,
    loopback: bool,
    api_keys: Vec<String>,
    schedules: ScheduleStoreHandle,
    pilots: crate::pilot_host::PilotService,
    wecom: WecomSettings,
    channel_descriptors: std::collections::BTreeMap<String, PathBuf>,
    channel_event_locks:
        std::sync::Arc<tokio::sync::Mutex<HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>>,
    #[cfg(test)]
    response_fixture: Option<String>,
}

#[derive(Clone, Default)]
struct WecomSettings {
    graph: String,
    reply_node: String,
    account: Option<String>,
    users: HashSet<String>,
}

impl WecomSettings {
    fn from_env() -> Self {
        Self {
            graph: env::var("ANCHOR_WECOM_GRAPH")
                .unwrap_or_default()
                .trim()
                .to_owned(),
            reply_node: env::var("ANCHOR_WECOM_REPLY_NODE")
                .unwrap_or_else(|_| "assistant".into())
                .trim()
                .to_owned(),
            account: env::var("WECOM_BOT_ID")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
            users: env::var("ANCHOR_WECOM_USERS")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect(),
        }
    }

    fn allows(&self, user: &str) -> bool {
        self.users.contains("*") || self.users.contains(user)
    }
}

fn error(status: StatusCode, message: impl Into<String>) -> HttpResponse {
    (status, Json(json!({"error": message.into()}))).into_response()
}

async fn auth(
    State(state): State<ApiState>,
    headers: HeaderMap,
    request: axum::extract::Request,
    next: Next,
) -> HttpResponse {
    let oauth_request = is_oauth_path(request.uri().path());
    let key = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if key.starts_with("anchor-channel-api-")
        || state
            .application
            .channel_callback_owner("wecom", key)
            .is_some()
    {
        let path = request.uri().path();
        let allowed = (request.method() == axum::http::Method::POST
            && matches!(
                path,
                "/channels/wecom/events" | "/channels/wecom/settlements"
            ))
            || (request.method() == axum::http::Method::GET
                && path
                    .strip_prefix("/channels/wecom/progress/")
                    .is_some_and(|event| !event.is_empty() && !event.contains('/')));
        if !allowed
            || state
                .application
                .channel_callback_owner("wecom", key)
                .is_none()
        {
            return error(
                StatusCode::UNAUTHORIZED,
                "invalid channel callback authorization",
            );
        }
        return next.run(request).await;
    }
    if state.api_keys.is_empty() && state.loopback {
        return next.run(request).await;
    }
    if !state
        .api_keys
        .iter()
        .any(|expected| constant_time_eq(expected, key))
    {
        let response = error(StatusCode::UNAUTHORIZED, "invalid API key");
        return if oauth_request {
            oauth::secure_oauth_response(response)
        } else {
            response
        };
    }
    next.run(request).await
}

async fn secure_private_responses(request: axum::extract::Request, next: Next) -> HttpResponse {
    let oauth_request = is_oauth_path(request.uri().path());
    let credential_request = credential_requests::is_private_path(request.uri().path());
    let response = next.run(request).await;
    if oauth_request {
        oauth::secure_oauth_response(response)
    } else if credential_request {
        credential_requests::secure_private_response(response)
    } else {
        response
    }
}

fn is_oauth_path(path: &str) -> bool {
    if path == "/oauth/callback" {
        return true;
    }
    let segments = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    segments.len() == 4 && segments[0] == "plugins" && matches!(segments[2], "oauth" | "authorize")
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    let (a, b) = (left.as_bytes(), right.as_bytes());
    let mut difference = a.len() ^ b.len();
    for index in 0..a.len().max(b.len()) {
        difference |=
            usize::from(a.get(index).copied().unwrap_or(0) ^ b.get(index).copied().unwrap_or(0));
    }
    difference == 0
}

fn status(value: RunStatus) -> &'static str {
    match value {
        RunStatus::Ready => "ready",
        RunStatus::Running => "running",
        RunStatus::Paused => "paused",
        RunStatus::BudgetStopped => "budget_stopped",
        RunStatus::WaitingCall => "waiting_call",
        RunStatus::WaitingRecovery => "waiting_recovery",
        RunStatus::Completed => "completed",
        RunStatus::Aborted => "aborted",
        RunStatus::Stopped => "stopped",
        RunStatus::Failed => "failed",
    }
}

/// The one Graph this deployment configures as its pilot Graph.
///
/// It is the Graph whose `required: true` declarations are an authorization, the
/// Graph the in-assistant tools may not rewrite, and the Graph the HTTP graph
/// routes refuse to touch. Every one of those decisions must read *this* answer:
/// the explicit `ANCHOR_RUNNER_GRAPH_NAME`, else the configured bundle's own
/// directory name. A second derivation (an env read here, a default there) is how
/// one side starts trusting a Graph the other does not.
///
/// `None` means the deployment named no Graph and its bundle has no usable name:
/// then no Graph may hand out `required`, which is the fail-closed direction.
pub(crate) fn configured_pilot_graph() -> Option<String> {
    env::var("ANCHOR_RUNNER_GRAPH_NAME")
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .or_else(|| {
            env::var_os("ANCHOR_RUNNER_BUNDLE_ROOT")
                .map(PathBuf::from)
                .and_then(|root| {
                    root.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .filter(|name| !name.is_empty())
        })
}

pub async fn serve() -> io::Result<()> {
    let configured_bundle = env::var_os("ANCHOR_RUNNER_BUNDLE_ROOT")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    let data_root = env::var_os("ANCHOR_RUNNER_STATE_ROOT")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("ANCHOR_RUNNER_STATE_ROOT is required"))?;
    let workspace_root = env::var_os("ANCHOR_RUNNER_WORKSPACE_ROOT")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("ANCHOR_RUNNER_WORKSPACE_ROOT is required"))?;
    let bundle_root = configured_bundle
        .clone()
        .unwrap_or_else(|| data_root.join("unconfigured-graph"));
    // The same single authority the node call and the graph routes read.
    let graph_name = configured_pilot_graph().unwrap_or_default();
    let listen = env::var("ANCHOR_RUNNER_LISTEN").unwrap_or_else(|_| "127.0.0.1:8077".into());
    let loopback = listen
        .parse::<std::net::SocketAddr>()
        .is_ok_and(|addr| addr.ip().is_loopback())
        || listen.starts_with("localhost:");
    let api_keys = parse_api_keys(&env::var("ANCHOR_API_KEYS").unwrap_or_default())?;
    if !loopback && api_keys.is_empty() {
        return Err(io::Error::other(
            "ANCHOR_API_KEYS required on non-loopback bind",
        ));
    }
    let catalog_root = env::var_os("ANCHOR_RUNNER_CATALOG_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            if configured_bundle.is_none() {
                return data_root.join("catalog");
            }
            bundle_root
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .to_path_buf()
        });
    for root in [&data_root, &workspace_root, &catalog_root] {
        crate::goose_acp::pilot::directory(root).map_err(io::Error::other)?;
    }
    let configured_graph_ready =
        validate_service_configuration(&bundle_root, &data_root, &workspace_root, &catalog_root)?;
    let _writer = crate::acquire_deployment_writer().map_err(io::Error::other)?;
    crate::runtime_configuration::install_release_network(&crate::library_root(&catalog_root))
        .map_err(io::Error::other)?;
    // Start-up reclamation is best effort: a Library that cannot be swept (a busy
    // writer, an unreadable manifest) must never keep the host from starting, and
    // the sweep only deletes what it can prove is unreferenced.
    {
        let root = crate::library_root(&catalog_root);
        match anchor_library::reclaim(&root, true) {
            // Anything that happened, or was refused, is worth one line; a Library
            // with nothing to reclaim stays quiet.
            Ok(report)
                if report.removed_views.is_empty()
                    && report.removed_package_versions.is_empty()
                    && report.deferred.is_none()
                    && report.store_phase_abandoned.is_none() => {}
            Ok(report) => eprintln!(
                "anchor-runner-host: dependency reclamation: {}",
                serde_json::to_string(&anchor_library::summarize(&report)).unwrap_or_default()
            ),
            Err(error) => {
                eprintln!("anchor-runner-host: dependency reclamation did not run: {error}")
            }
        }
    }
    let schedules_path = env::var_os("ANCHOR_RUNNER_SCHEDULES_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| data_root.join("state/schedules.json"));
    // Environment preparation is the one capability that reaches the network.
    // A deployment that did not authorize sources and a pinned toolchain keeps
    // exactly the behaviour it had: the steward prepares packages, and
    // provisioning is refused with a reason.
    // The root dependency preparation writes its wheelhouse into: the
    // explicit Library root, otherwise the Graph catalog the Plugin API also
    // falls back to. The build root must stay disjoint from it.
    let dependency_library_root = crate::library_root(&catalog_root);
    let runtime_configuration = Arc::new(
        crate::runtime_configuration::RuntimeConfiguration::from_env(
            &data_root,
            &dependency_library_root,
            env::var_os("ANCHOR_BWRAP")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("bwrap")),
        )
        .map_err(io::Error::other)?,
    );
    let mut application = RunApplication::new(data_root.clone(), catalog_root.clone())
        .with_runtime_configuration(runtime_configuration);
    if configured_bundle.is_some() {
        application = application.with_configured_graph(graph_name.clone(), bundle_root.clone());
    }
    let schedules = ScheduleStore::open(schedules_path).map_err(io::Error::other)?;
    application = application.with_schedule_management(Arc::new(
        schedules::ScheduleManagementAdapter::new(schedules.clone()),
    ));
    let mut state = ApiState {
        #[cfg(test)]
        bundle_root: bundle_root.clone(),
        catalog_root: catalog_root.clone(),
        application,
        data_root,
        workspace_root,
        graph_name,
        loopback,
        api_keys,
        schedules,
        pilots: crate::pilot_host::PilotService::default(),
        wecom: WecomSettings::from_env(),
        channel_descriptors: Default::default(),
        channel_event_locks: Default::default(),
        #[cfg(test)]
        response_fixture: None,
    };
    recover_pilot_turns(&state)
        .await
        .map_err(io::Error::other)?;
    state
        .application
        .recover_detached_at_startup()
        .await
        .map_err(|_| io::Error::other("startup Run recovery failed"))?;
    // A restart gives every Session back the assistant instance its binding
    // still owns; a failure is reported per Session and never stops the Host.
    if let Err(error) = assistant_recovery::resume_channel_assistants(&state).await {
        eprintln!("anchor-runner-host: assistant recovery is incomplete: {error}");
    }
    if let Err(error) = skip_missed_schedules(&state, chrono::Local::now().naive_local()) {
        return Err(io::Error::other(error));
    }
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    let callback_listen = listener.local_addr()?.to_string();
    state.application = state.application.clone().with_oauth_management(Arc::new(
        oauth::OAuthManagementAdapter::new(
            state.data_root.clone(),
            crate::library_root(&state.catalog_root),
            &callback_listen,
        ),
    ));
    let mut channel_supervisor = if configured_bundle.is_some() {
        crate::channel_supervisor::ChannelSupervisor::start(
            &bundle_root,
            &crate::library_root(&state.catalog_root),
            &state.data_root,
            &callback_listen,
            &state.api_keys,
        )
        .await?
    } else {
        None
    };
    if let Some(supervisor) = channel_supervisor.as_ref()
        && let Some(descriptor) = supervisor.descriptor("wecom")
        && let Some(existing) = env::var_os("ANCHOR_CHANNEL_CONTROL_DESCRIPTOR")
        && Path::new(&existing) != descriptor
    {
        if let Some(supervisor) = channel_supervisor.as_mut() {
            supervisor.shutdown().await;
        }
        return Err(io::Error::other(
            "ANCHOR_CHANNEL_CONTROL_DESCRIPTOR conflicts with the discovered WeCom channel",
        ));
    }
    if channel_supervisor.is_some()
        && (env::var_os("ANCHOR_CHANNEL_CONTROL_SOCKET").is_some()
            || env::var_os("ANCHOR_CHANNEL_CONTROL_TOKEN").is_some())
    {
        if let Some(supervisor) = channel_supervisor.as_mut() {
            supervisor.shutdown().await;
        }
        return Err(io::Error::other(
            "automatic channel supervision does not accept inherited control socket or token",
        ));
    }
    if let Some(supervisor) = channel_supervisor.as_ref() {
        state.channel_descriptors = supervisor.descriptors();
    }
    let deployment_headers = state
        .api_keys
        .first()
        .and_then(|key| axum::http::HeaderValue::from_str(&format!("Bearer {key}")).ok())
        .map(|value| HeaderMap::from_iter([(header::AUTHORIZATION, value)]))
        .unwrap_or_default();
    let deployment_owner = private_owner(&state, &deployment_headers);
    let channel_manager = Arc::new(
        crate::channel_supervisor::management::ChannelManagementAdapter::open(
            crate::channel_supervisor::management::ChannelHostConfiguration::from_env(
                state.data_root.clone(),
                state.catalog_root.clone(),
                crate::library_root(&state.catalog_root),
                callback_listen,
                deployment_owner,
            ),
            channel_supervisor.take(),
        )
        .map_err(io::Error::other)?,
    );
    channel_manager
        .resume()
        .await
        .map_err(|error| io::Error::other(format!("channel startup unavailable: {error:?}")))?;
    state.application = state
        .application
        .clone()
        .with_channel_management(channel_manager.clone());
    let ready = Arc::new(AtomicBool::new(
        configured_bundle.is_none() || configured_graph_ready,
    ));
    start_schedule_ticker(state.clone());
    let app = router_with_readiness(state, ready.clone());
    let shutdown_ready = ready.clone();
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            shutdown_ready.store(false, Ordering::Release);
        })
        .await
        .map_err(io::Error::other);
    channel_manager.shutdown().await;
    result
}

fn validate_service_configuration(
    bundle_root: &Path,
    state_root: &Path,
    workspace_root: &Path,
    catalog_root: &Path,
) -> io::Result<bool> {
    let roots = [
        (bundle_root, "bundle"),
        (state_root, "state"),
        (workspace_root, "workspace"),
        (catalog_root, "catalog"),
    ];
    for (path, label) in roots {
        if !path.is_absolute() {
            return Err(io::Error::other(format!(
                "configured {label} path must be absolute"
            )));
        }
        if path
            .components()
            .any(|component| component == std::path::Component::ParentDir)
        {
            return Err(io::Error::other(format!(
                "configured {label} path must not contain parent components"
            )));
        }
        reject_symlink_components(path, label)?;
    }
    if paths_overlap(state_root, workspace_root) {
        return Err(io::Error::other(
            "configured state and workspace directories must not overlap",
        ));
    }

    let configured_graph_ready = match std::fs::symlink_metadata(bundle_root) {
        Err(failure) if failure.kind() == io::ErrorKind::NotFound => false,
        Err(_) => return Err(io::Error::other("configured Graph bundle is unavailable")),
        Ok(metadata) if !metadata.is_dir() => {
            return Err(io::Error::other(
                "configured Graph bundle is missing or invalid",
            ));
        }
        Ok(_) if !bundle_root.join("graph.json").is_file() => false,
        Ok(_) => {
            FileGraphBundleLoader::new(bundle_root)
                .load()
                .map_err(|_| io::Error::other("configured Graph bundle is missing or invalid"))?;
            true
        }
    };

    for (path, label) in [
        (state_root, "state"),
        (workspace_root, "workspace"),
        (catalog_root, "catalog"),
    ] {
        let metadata = std::fs::symlink_metadata(path).map_err(|_| {
            io::Error::other(format!("configured {label} directory is unavailable"))
        })?;
        if !metadata.is_dir() || std::fs::File::open(path).is_err() {
            return Err(io::Error::other(format!(
                "configured {label} directory is unavailable"
            )));
        }
        let probe = path.join(format!(
            ".anchor-host-readiness-{}-{}.tmp",
            std::process::id(),
            READINESS_PROBE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
            .map_err(|_| {
                io::Error::other(format!("configured {label} directory is unavailable"))
            })?;
        std::fs::remove_file(probe).map_err(|_| {
            io::Error::other(format!("configured {label} directory is unavailable"))
        })?;
    }
    reject_symlink_components(bundle_root, "bundle")?;

    let state_canonical = std::fs::canonicalize(state_root)
        .map_err(|_| io::Error::other("configured state directory is unavailable"))?;
    let workspace_canonical = std::fs::canonicalize(workspace_root)
        .map_err(|_| io::Error::other("configured workspace directory is unavailable"))?;
    if path_prefixes_overlap(&state_canonical, &workspace_canonical) {
        return Err(io::Error::other(
            "configured state and workspace directories must not overlap",
        ));
    }
    Ok(configured_graph_ready)
}

fn reject_symlink_components(path: &Path, label: &str) -> io::Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(io::Error::other(format!(
                    "configured {label} path contains a symlink"
                )));
            }
            Ok(_) => {}
            Err(failure) if failure.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(_) => {
                return Err(io::Error::other(format!(
                    "configured {label} path is unavailable"
                )));
            }
        }
    }
    Ok(())
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    let left = normalize_absolute_path(left);
    let right = normalize_absolute_path(right);
    path_prefixes_overlap(&left, &right)
}

fn path_prefixes_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn normalize_absolute_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

static READINESS_PROBE_SEQUENCE: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(terminate) => terminate,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[allow(clippy::result_large_err)]
async fn channel_sessions_projection(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, HttpResponse> {
    let owner = private_owner(&state, &headers);
    blocking(move || {
        let sessions = store(&state)?
            .list(&owner)
            .map_err(session_error)?
            .into_iter()
            .filter(|session| {
                !session.graph.is_empty()
                    && !session.channel.is_empty()
                    && session.status != anchor_platform_session::SessionStatus::Archived
            })
            .map(|session| {
                json!({
                    "id": session.id,
                    "title": session.title,
                    "graph": session.graph,
                    "platform": session.channel.get("source")
                        .or_else(|| session.channel.get("platform"))
                        .cloned()
                        .unwrap_or_default(),
                })
            })
            .collect::<Vec<_>>();
        Ok(Json(json!({"sessions":sessions})))
    })
    .await
}

/// What one Session's current Run froze about one Plugin.
///
/// Several nodes of the same snapshot may declare the same Plugin, and the
/// Session view cannot know which node the next Turn starts at, so both fields
/// aggregate over the whole frozen snapshot: **any** node declaring `required`
/// makes it required, and it counts as mounted by default when **some** node
/// would mount it with no Session switch (it is required, or its `default` is
/// not explicitly false). For a single declaring node this is exactly what the
/// mount filter does for that node.
#[derive(Debug, Clone, Copy, Default)]
struct PluginDeclaration {
    required: bool,
    mounts_by_default: bool,
}

/// The declarations the Session's **attached Runs froze**, by Plugin id.
///
/// Deliberately not the Graph on disk: the mount filter reads the admitted Run's
/// frozen snapshot (`node_host::mounted_plugins`), so this page has to answer
/// from the same facts or it would promise a mount that Run never makes.
///
/// The answer is the **union over every Run this Session attached**, not the most
/// recent one. An assistant's `graph_run` attaches a Run of a Graph it made
/// itself, so "the last Run" is something the assistant can choose: reading only
/// that Run would let it hide a Plugin the operator's pilot Graph declares
/// required, the page would offer the switch, and a human click would write a
/// `session.plugin_mount.changed` event authorizing a switch-off that the filter
/// then ignores. A Plugin is therefore only "not required" when **no** attached
/// Run requires it, and a Run that cannot be read (a deleted Run still listed)
/// contributes nothing rather than an unlock.
///
/// `required: true` is an authorization, and the filter recognizes exactly one
/// source for it: `configured_pilot_graph()`, the deployment's own pilot Graph. A
/// Run whose metadata names any other Graph — one an assistant created or edited,
/// or a Session's own choice — has its `required` downgraded to optional here
/// too, by the same comparison. The two sides must never disagree about who may
/// hand out a lock.
///
/// Aggregation is per Plugin across the whole frozen snapshots: **any** declaring
/// node/run makes it required, and it counts as mounted by default when **some**
/// declaration would mount it with no Session switch (it is required, or its
/// `default` is not explicitly false). For a single declaring node this is
/// exactly what the mount filter does for that node.
///
/// A Session with no attached Run froze no declarations. Then nothing is reported
/// as required, and `on` follows only the Session's own switches: unknown is
/// reported as "not mounted by any Run's declaration", never as a promise.
///
/// Note for readers of the audit trail: mounting has two distinct sources. A
/// Plugin the Session switched on is authorized by the human's click plus the
/// `session.plugin_mount.changed` event; a Plugin mounted because a declaration
/// says so (required, or `default`) was never clicked and writes no such event.
fn session_plugin_declarations(
    state: &ApiState,
    session: &anchor_platform_session::Session,
) -> BTreeMap<String, PluginDeclaration> {
    let configured_pilot_graph = configured_pilot_graph();
    let runs = FileRunStore::new(state.data_root.join("runs"));
    let mut declarations: BTreeMap<String, PluginDeclaration> = BTreeMap::new();
    for run in &session.run_ids {
        let Ok(Some(record)) = runs.load(run) else {
            continue;
        };
        // A Run that declares no Plugin cannot contribute a declaration, so its
        // Graph name does not matter either.
        if record
            .snapshot
            .nodes
            .iter()
            .all(|node| node.plugins.is_empty())
        {
            continue;
        }
        // The same comparison, from the same two facts, as `mounted_plugins`: the
        // Run's own recorded Graph name, and the deployment's configured pilot
        // Graph.
        let declaring_graph = crate::application::metadata::load(&state.data_root, run)
            .ok()
            .flatten()
            .map(|metadata| metadata.graph);
        let required_is_authoritative = declaring_graph.is_some()
            && declaring_graph.as_deref() == configured_pilot_graph.as_deref();
        for node in &record.snapshot.nodes {
            for reference in &node.plugins {
                let required = reference.required && required_is_authoritative;
                let entry = declarations.entry(reference.id.clone()).or_default();
                entry.required |= required;
                entry.mounts_by_default |= required || reference.default.unwrap_or(true);
            }
        }
    }
    declarations
}

/// The same Plugin-id rule the Library catalog applies to `plugins/<id>`.
fn valid_plugin_id(value: &str) -> bool {
    !value.is_empty()
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

/// One installed Plugin's row for the Session view.
///
/// `available` is a report, not an action: this reads whether the view the
/// Plugin's own declaration maps to is usable right now, and never builds,
/// repairs, downloads or installs anything.
fn session_plugin_row(
    library_root: &Path,
    definition: &PluginDefinition,
    declaration: Option<PluginDeclaration>,
    mounts: &BTreeMap<String, bool>,
) -> Value {
    let declaration = declaration.unwrap_or_default();
    let required = declaration.required;
    // The Session's own switch wins; otherwise the frozen declaration decides,
    // exactly as the mount filter does for the node that declaration belongs to.
    let on = required
        || mounts
            .get(&definition.id)
            .copied()
            .unwrap_or(declaration.mounts_by_default);
    let (available, error) =
        match crate::plugin_view::prepare_for_plugin(library_root, &definition.id) {
            Ok(_) => (true, Value::Null),
            Err(unusable) => (false, Value::String(unusable.to_string())),
        };
    json!({
        "id": definition.id,
        "name": definition.name,
        "description": definition.description,
        "skills": definition.skills,
        // Named apart from the `/plugins` catalog's `mcpServers` object: this one
        // is the plain list of server names the Session row shows.
        "mcpServerNames": definition
            .mcp_servers
            .iter()
            .map(|server| server.name.clone())
            .collect::<Vec<_>>(),
        "required": required,
        "on": on,
        "requiresNetwork": crate::plugin_network::requires_network(definition),
        "networkProvider": definition.id == crate::plugin_network::NETWORK_PLUGIN,
        "available": available,
        "error": error,
    })
}

/// Every installed Plugin, for one Session. A directory the catalog cannot
/// resolve is not installed and is not listed.
#[allow(clippy::result_large_err)]
fn session_plugin_rows(
    state: &ApiState,
    session: &anchor_platform_session::Session,
) -> Result<Vec<Value>, HttpResponse> {
    let root = crate::library_root(&state.catalog_root);
    let catalog = FilePluginCatalog::new(&root);
    let declarations = session_plugin_declarations(state, session);
    let entries = match std::fs::read_dir(root.join("plugins")) {
        Ok(entries) => entries,
        Err(failure) if failure.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => {
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot read Plugin catalog",
            ));
        }
    };
    let mut rows = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| {
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot read Plugin catalog",
            )
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(definition) = catalog.definition(&name) else {
            continue;
        };
        rows.push(session_plugin_row(
            &root,
            &definition,
            declarations.get(&name).copied(),
            &session.plugin_mounts,
        ));
    }
    let mounted = rows
        .iter()
        .filter(|row| row["on"] == true && row["available"] == true)
        .filter_map(|row| row["id"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    let network = crate::plugin_network::resolve(&root, &mounted, session.network_allowed);
    for row in &mut rows {
        let id = row["id"].as_str().unwrap_or_default().to_owned();
        row["explicitOn"] = row["on"].clone();
        row["networkAllowed"] = json!(network.allowed);
        row["networkPermission"] = json!(session.network_allowed);
        if id == crate::plugin_network::NETWORK_PLUGIN {
            row["on"] = json!(network.enabled);
            row["enabledBy"] = json!(network.enabled_by);
        }
        if let Some(problem) = network.errors.get(&id) {
            row["available"] = json!(false);
            row["error"] = json!(problem);
        }
    }
    rows.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));
    Ok(rows)
}

/// Which installed Plugins this Session can mount, and which it already has on.
#[allow(clippy::result_large_err)]
async fn session_plugins(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AxumPath(identifier): AxumPath<String>,
) -> Result<Json<Value>, HttpResponse> {
    blocking(move || {
        let (_, _, session) = owned_session(&state, &headers, &identifier)?;
        let plugins = session_plugin_rows(&state, &session)?;
        Ok(Json(json!({"plugins": plugins})))
    })
    .await
}

/// One Plugin's mount switch for a Session.
///
/// The authorization is this click plus the audit event the store writes with
/// it. Mounting never installs: the Plugin must already be installed with a
/// usable view, and a Plugin the Graph declares `required` cannot be switched
/// off at all.
#[allow(clippy::result_large_err)]
async fn set_session_plugin(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AxumPath((identifier, plugin)): AxumPath<(String, String)>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, HttpResponse> {
    let fields = body
        .as_object()
        .filter(|fields| fields.len() == 1 && fields.contains_key("on"))
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "provide on only"))?;
    let on = fields
        .get("on")
        .and_then(Value::as_bool)
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "on must be a boolean"))?;
    if !valid_plugin_id(&plugin) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            format!("插件 id `{plugin}` 不合法"),
        ));
    }
    let pilot_guard = state.pilots.plugin_mount_guard().await;
    let catalog_guard = state.application.graph_catalog_mutation_guard().await;
    // Known refusals must not create a Library or its publication lock. This
    // read-only preflight is repeated after taking the lease because a writer
    // from another host may replace/uninstall the Plugin while we wait.
    blocking({
        let (state, headers, identifier, plugin) = (
            state.clone(),
            headers.clone(),
            identifier.clone(),
            plugin.clone(),
        );
        move || {
            session_plugin_mount_target(&state, &headers, &identifier, &plugin)?;
            Ok(())
        }
    })
    .await?;
    let lease = state
        .application
        .plugin_reference_lease()
        .await
        .map_err(application_error)?;
    blocking(move || {
        // Retain both guards even if validation or the transaction fails.
        let (pilot_guard, catalog_guard, lease) = (pilot_guard, catalog_guard, lease);
        let (sessions, owner, definition) =
            session_plugin_mount_target(&state, &headers, &identifier, &plugin)?;
        let before = sessions.get(&owner, &identifier).map_err(session_error)?;
        let changed = before.plugin_mounts.get(&plugin) != Some(&on);
        let updated = sessions
            .set_plugin_mount(
                &owner,
                &identifier,
                &plugin,
                on,
                &owner,
                "the Session view switched this Plugin",
            )
            .map_err(session_error)?;
        let (effect, continuation) = pilot_guard
            .effect(&sessions, &owner, &updated, &state.data_root, changed)
            .map_err(session_error)?;
        drop(lease);
        drop(catalog_guard);
        drop(pilot_guard);
        let rows = session_plugin_rows(&state, &updated)?;
        let row = rows.iter().find(|row| row["id"] == definition.id).cloned();
        Ok(Json(
            json!({"plugin": row, "plugins": rows, "effect":effect, "continuation":continuation}),
        ))
    })
    .await
}

#[allow(clippy::result_large_err)]
fn session_plugin_mount_target(
    state: &ApiState,
    headers: &HeaderMap,
    identifier: &str,
    plugin: &str,
) -> Result<
    (
        anchor_platform_session::SessionStore,
        String,
        PluginDefinition,
    ),
    HttpResponse,
> {
    let (sessions, owner, session) = owned_session(state, headers, identifier)?;
    if session.status == anchor_platform_session::SessionStatus::Archived {
        return Err(error(
            StatusCode::CONFLICT,
            "该会话已归档，不能再改变插件挂载",
        ));
    }
    let root = crate::library_root(&state.catalog_root);
    let definition = FilePluginCatalog::new(&root)
        .definition(plugin)
        .map_err(|_| {
            error(
                StatusCode::BAD_REQUEST,
                format!("插件 `{plugin}` 尚未安装，不能挂载"),
            )
        })?;
    let declaration = session_plugin_declarations(state, &session)
        .get(plugin)
        .copied();
    if declaration.is_some_and(|declaration| declaration.required) {
        return Err(error(
            StatusCode::CONFLICT,
            "该插件在图里被声明为必需（required: true）",
        ));
    }
    if let Err(unusable) = crate::plugin_view::prepare_for_plugin(&root, plugin) {
        return Err(error(StatusCode::CONFLICT, unusable.to_string()));
    }
    Ok((sessions, owner, definition))
}

#[allow(clippy::result_large_err)]
async fn set_session_network(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AxumPath(identifier): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, HttpResponse> {
    blocking(move || {
        let fields = body
            .as_object()
            .filter(|fields| fields.len() == 1 && fields.contains_key("allowed"))
            .ok_or_else(|| error(StatusCode::BAD_REQUEST, "provide allowed only"))?;
        let allowed = fields["allowed"]
            .as_bool()
            .ok_or_else(|| error(StatusCode::BAD_REQUEST, "allowed must be a boolean"))?;
        let (sessions, owner, session) = owned_session(&state, &headers, &identifier)?;
        if session.status == anchor_platform_session::SessionStatus::Archived {
            return Err(error(
                StatusCode::CONFLICT,
                "该会话已归档，不能再改变联网权限",
            ));
        }
        let updated = sessions
            .set_network_allowed(&owner, &identifier, allowed, &owner)
            .map_err(session_error)?;
        Ok(Json(
            json!({"plugins": session_plugin_rows(&state, &updated)?}),
        ))
    })
    .await
}

fn parse_api_keys(raw: &str) -> io::Result<Vec<String>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let keys = serde_json::from_str::<Vec<String>>(raw)
        .map_err(|_| io::Error::other("ANCHOR_API_KEYS must be a JSON array of strings"))?;
    let unique = keys.iter().collect::<HashSet<_>>().len() == keys.len();
    if keys.is_empty() || keys.iter().any(|key| key.is_empty() || key.len() < 32) || !unique {
        return Err(io::Error::other(
            "ANCHOR_API_KEYS must contain unique secrets of at least 32 bytes",
        ));
    }
    Ok(keys)
}

#[cfg(test)]
fn router(state: ApiState) -> Router {
    let web_root = env::var_os("ANCHOR_RUNNER_WEB_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/web/dist"));
    router_with_readiness_and_web_root(state, Arc::new(AtomicBool::new(true)), web_root)
}

#[cfg(test)]
fn router_with_web_root(state: ApiState, web_root: PathBuf) -> Router {
    router_with_readiness_and_web_root(state, Arc::new(AtomicBool::new(true)), web_root)
}

fn router_with_readiness(state: ApiState, ready: Arc<AtomicBool>) -> Router {
    let web_root = env::var_os("ANCHOR_RUNNER_WEB_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/web/dist"));
    router_with_readiness_and_web_root(state, ready, web_root)
}

fn router_with_readiness_and_web_root(
    state: ApiState,
    ready: Arc<AtomicBool>,
    web_root: PathBuf,
) -> Router {
    let conversations = Router::new()
        .route("/conversation-runs", post(conversation_run))
        .layer(DefaultBodyLimit::max(crate::channel_inputs::MAX_BODY_BYTES))
        .layer(RequestBodyLimitLayer::new(
            crate::channel_inputs::MAX_BODY_BYTES,
        ));
    let wecom_events = Router::new()
        .route("/channels/wecom/events", post(wecom::receive_event))
        .route(
            "/channels/wecom/progress/{event_id}",
            get(wecom_progress::wecom_progress),
        )
        .layer(DefaultBodyLimit::max(crate::channel_inputs::MAX_BODY_BYTES))
        .layer(RequestBodyLimitLayer::new(
            crate::channel_inputs::MAX_BODY_BYTES,
        ));
    let pilot_turns = Router::new()
        .route(
            "/sessions/{session}/turns",
            get(list_pilot_turns).post(create_pilot_turn),
        )
        .layer(DefaultBodyLimit::max(crate::attachments::MAX_BODY_BYTES))
        .layer(RequestBodyLimitLayer::new(
            crate::attachments::MAX_BODY_BYTES,
        ));
    let api = Router::new()
        .route("/health", get(|| async { Json(json!({"status":"ok"})) }))
        .route(
            "/ready",
            get(move || {
                let ready = ready.clone();
                async move {
                    if ready.load(Ordering::Acquire) {
                        (StatusCode::OK, Json(json!({"status":"ready"})))
                    } else {
                        (
                            StatusCode::SERVICE_UNAVAILABLE,
                            Json(json!({"status":"not_ready"})),
                        )
                    }
                }
            }),
        )
        .route("/graph-relations", get(graph_relations))
        .route("/plugins", get(plugin_catalog))
        .route("/plugins/{plugin}", get(plugin_detail))
        .route("/plugins/{plugin}/files/{*path}", get(plugin_file))
        .route(
            "/plugins/{plugin}/oauth/{server}",
            get(oauth_status).post(authorize_oauth).delete(revoke_oauth),
        )
        .route(
            "/plugins/{plugin}/authorize/{server}",
            post(authorize_oauth),
        )
        .route("/graphs", get(graphs))
        .route("/graphs", post(create_graph))
        .route("/graph-validation", post(validate_graph))
        .route(
            "/graphs/{graph}",
            get(graph).put(update_graph).delete(delete_graph),
        )
        .route(
            "/graphs/{graph}/delete-precondition",
            get(graph_delete_precondition),
        )
        .route("/trigger", post(trigger))
        .route("/v1/responses", post(create_response))
        .route("/v1/webhooks/graphs/{graph}", post(graph_webhook))
        .route("/sessions", get(list_sessions).post(create_session))
        .merge(credential_requests::router())
        .route(
            "/sessions/{session}",
            get(get_session).put(rename_session).delete(delete_session),
        )
        .route("/sessions/{session}/status", post(set_session_status))
        .route("/sessions/{session}/events", get(session_events))
        .route("/sessions/{session}/plugins", get(session_plugins))
        .route("/sessions/{session}/network", post(set_session_network))
        .route(
            "/sessions/{session}/plugins/{plugin}",
            post(set_session_plugin),
        )
        .route(
            "/sessions/{session}/messages",
            get(pilot_messages).post(session_execution_unavailable),
        )
        .route("/sessions/{session}/stop", post(stop_pilot))
        .route("/sessions/{session}/turns/{turn}", get(get_pilot_turn))
        .route(
            "/sessions/{session}/turns/{turn}/attachments/{attachment}",
            get(pilot_attachment),
        )
        .route(
            "/sessions/{session}/turns/{turn}/questions",
            get(list_pilot_questions),
        )
        .route(
            "/sessions/{session}/turns/{turn}/questions/{question}/answer",
            post(answer_pilot_question),
        )
        .route(
            "/sessions/{session}/turns/{turn}/events",
            get(pilot_turn_events),
        )
        .route("/channel-sessions/inbound", post(admit_channel_inbound))
        .route("/channel-sessions", get(channel_sessions_projection))
        .route(
            "/channel-sessions/{session}/assistant",
            get(get_channel_assistant),
        )
        .route(
            "/channel-sessions/{session}/assistant/retire",
            post(retire_channel_assistant),
        )
        .route("/channels/wecom/settlements", post(wecom::settle_delivery))
        .route(
            "/channel-sessions/{session}/inbounds/{inbound}",
            get(get_channel_relation),
        )
        .route(
            "/channel-sessions/{session}/inbounds/{inbound}/run",
            post(associate_channel_run),
        )
        .route(
            "/channel-sessions/{session}/turns/{turn}/deliveries",
            post(admit_channel_delivery),
        )
        .route(
            "/channel-sessions/{session}/turns/{turn}/deliveries/{key}/begin",
            post(begin_channel_delivery),
        )
        .route(
            "/channel-sessions/{session}/deliveries/{key}/settle",
            post(settle_channel_delivery),
        )
        .route(
            "/channel-sessions/{session}/deliveries/unresolved",
            get(list_channel_deliveries),
        )
        .route(
            "/channel-sessions/{session}",
            axum::routing::delete(delete_channel_session),
        )
        .route("/schedules", get(list_schedules).post(create_schedule))
        .route(
            "/schedules/{schedule}",
            axum::routing::delete(delete_schedule),
        )
        .route("/runs", get(list_runs))
        .route("/runs/{run}", get(get_run).delete(delete_run))
        .route("/runs/{run}/channel-reply", get(read_channel_reply))
        .route("/runs/{run}/session-execution", post(execute_session_call))
        .route("/runs/{run}/session-yield", post(yield_session_call))
        .route("/runs/{run}/session-settlement", post(settle_session_call))
        .route("/runs/{run}/recovery", post(recover_run))
        .route("/runs/{run}/abandon", post(abandon_run))
        .route("/runs/{run}/{operation}", post(control))
        .route("/runs/{run}/files/{node}", get(list_files))
        .route("/runs/{run}/files/{node}/{*path}", get(read_file))
        .route("/timeline", get(timeline))
        .layer(RequestBodyLimitLayer::new(2 * 1024 * 1024))
        .merge(pilot_turns)
        .merge(conversations)
        .merge(wecom_events)
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(state.clone(), auth));
    let oauth_callback = Router::new()
        .route("/oauth/callback", get(oauth_callback))
        .with_state(state.clone());
    Router::new()
        .merge(api)
        .merge(oauth_callback)
        .fallback_service(ServeDir::new(web_root).append_index_html_on_directories(true))
        .layer(middleware::from_fn(secure_private_responses))
}

fn application_error(failure: ApplicationError) -> HttpResponse {
    match failure {
        ApplicationError::Missing => error(StatusCode::NOT_FOUND, "no such run"),
        ApplicationError::Conflict(message) => error(StatusCode::CONFLICT, message),
        ApplicationError::Invalid(message) => error(StatusCode::UNPROCESSABLE_ENTITY, message),
        ApplicationError::Storage(message) => error(StatusCode::INTERNAL_SERVER_ERROR, message),
    }
}

async fn graph_relations(State(state): State<ApiState>) -> Result<Json<Value>, HttpResponse> {
    let catalog = state
        .application
        .list_graphs()
        .await
        .map_err(graph_projection_error)?;
    let names = catalog["graphs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|graph| graph["graph"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    let mut graphs = Vec::with_capacity(names.len());
    let mut calls = Vec::new();
    for name in names {
        match state.application.load_graph(&name) {
            Ok((_, bundle)) => {
                for node in &bundle.snapshot.nodes {
                    let Some(op_name) = node.op.as_deref() else {
                        continue;
                    };
                    let Some(call) = bundle
                        .snapshot
                        .ops
                        .get(op_name)
                        .and_then(|op| op.get("call"))
                        .and_then(Value::as_object)
                    else {
                        continue;
                    };
                    let (Some(target), Some(mode)) = (
                        call.get("graph").and_then(Value::as_str),
                        call.get("mode").and_then(Value::as_str),
                    ) else {
                        continue;
                    };
                    calls.push(json!({
                        "graph": name,
                        "node": node.id,
                        "op": op_name,
                        "target": target,
                        "mode": mode,
                    }));
                }
            }
            Err(crate::application::graphs::GraphManagementError::Missing(_)) => {}
            Err(failure) => return Err(graph_projection_error(failure)),
        }
        graphs.push(name);
    }
    let schedules = state.schedules.lock().map_err(|_| {
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "schedule store unavailable",
        )
    })?;
    let graphs = graphs
        .into_iter()
        .map(|graph| {
            let count = schedules
                .items
                .iter()
                .filter(|item| item.graph == graph)
                .count();
            json!({"graph":graph,"schedules":count})
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({"graphs":graphs,"calls":calls})))
}

fn graph_projection_error(
    failure: crate::application::graphs::GraphManagementError,
) -> HttpResponse {
    use crate::application::graphs::GraphManagementError;

    match failure {
        GraphManagementError::BadRequest(message) => error(StatusCode::BAD_REQUEST, message),
        GraphManagementError::Missing(message) => error(StatusCode::NOT_FOUND, message),
        GraphManagementError::Conflict(message) => error(StatusCode::CONFLICT, message),
        GraphManagementError::Invalid(message) => error(StatusCode::UNPROCESSABLE_ENTITY, message),
        GraphManagementError::Storage(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Graph catalog unavailable",
        ),
    }
}

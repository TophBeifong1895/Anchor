use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::pilot_tools::PilotTools;
use anchor_platform_session::{
    Session, SessionError, SessionStatus, SessionStore, Turn, TurnStatus,
};
use sha2::{Digest, Sha256};

#[cfg(test)]
mod tests;

#[derive(Clone, Default)]
pub(crate) struct PilotService {
    gate: Arc<tokio::sync::Mutex<()>>,
    active: Arc<Mutex<HashMap<PathBuf, Arc<AtomicBool>>>>,
}

pub(crate) fn scope(data_root: &std::path::Path, owner: &str, session: &Session) -> PathBuf {
    let identity = serde_json::to_vec(&(owner, &session.id, session.created_at))
        .expect("Session identity is JSON");
    std::path::absolute(data_root)
        .unwrap_or_else(|_| data_root.to_path_buf())
        .join("platform/pilot")
        .join(format!("{:x}", Sha256::digest(identity)))
}

pub(crate) struct PilotAdmission {
    pub(crate) sessions: SessionStore,
    pub(crate) owner: String,
    pub(crate) session: Session,
    pub(crate) request_id: String,
    pub(crate) prompt: Option<String>,
    pub(crate) root: PathBuf,
    pub(crate) tools: PilotTools,
}

pub(crate) enum AdmissionError {
    Session(SessionError),
    Provider,
}

impl From<SessionError> for AdmissionError {
    fn from(error: SessionError) -> Self {
        Self::Session(error)
    }
}

impl PilotService {
    pub(crate) async fn ready_to_continue(
        &self,
        mut admission: PilotAdmission,
        source: &str,
        reason: &str,
    ) -> Result<(), AdmissionError> {
        let guard = self.gate.lock().await;
        let current = admission
            .sessions
            .get(&admission.owner, &admission.session.id)?;
        if current.created_at != admission.session.created_at
            || current.status == SessionStatus::Archived
        {
            return Err(
                SessionError::Conflict("continuation Session identity changed".into()).into(),
            );
        }
        let turn = admission
            .sessions
            .get_turn(&admission.owner, &current.id, source)?;
        if turn.status == TurnStatus::Stopped
            || admission
                .sessions
                .list_turns(&admission.owner, &current.id)?
                .first()
                .is_none_or(|latest| latest.id != source)
        {
            return Err(SessionError::Conflict(
                "continuation was stopped or superseded by a newer user Turn".into(),
            )
            .into());
        }
        let request_id = format!("continuation:{source}");
        admission.sessions.append_event(
            &admission.owner,
            &current.id,
            "pilot.continuation.requested",
            std::collections::BTreeMap::from([
                ("turn".into(), serde_json::json!(source)),
                ("request_id".into(), serde_json::json!(request_id)),
                ("reason".into(), serde_json::json!(reason)),
            ]),
        )?;
        drop(guard);
        if turn.status != TurnStatus::Completed {
            return Ok(());
        }
        admission.request_id = request_id;
        admission.prompt=Some("Host 已完成此前发起的外部服务授权。继续原任务：先核查 platform_status 和授权状态，使用此轮重新冻结的工具；不要重放结果未知的外部操作。".into());
        let sessions = admission.sessions.clone();
        let owner = admission.owner.clone();
        let session = admission.session.id.clone();
        let result = self.admit(admission).await;
        if result.is_err() {
            let _ = sessions.append_event(
                &owner,
                &session,
                "pilot.continuation.failed",
                std::collections::BTreeMap::from([("turn".into(), serde_json::json!(source))]),
            );
        }
        result.map(|_| ())
    }
    pub(crate) async fn credential_submission_guard(
        &self,
        root: &std::path::Path,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, SessionError> {
        let guard = self.gate.clone().lock_owned().await;
        if self
            .active
            .lock()
            .map_err(|_| SessionError::Storage("Pilot control failed".into()))?
            .get(root)
            .is_some_and(|cancellation| cancellation.load(Ordering::Acquire))
        {
            return Err(SessionError::Conflict(
                "Pilot is stopping; credential input is no longer active".into(),
            ));
        }
        Ok(guard)
    }

    pub(crate) fn admit(
        &self,
        mut admission: PilotAdmission,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Turn, AdmissionError>> + Send + '_>,
    > {
        Box::pin(async move {
            let _gate = self.gate.lock().await;
            let current = admission
                .sessions
                .get(&admission.owner, &admission.session.id)?;
            if current.created_at != admission.session.created_at
                || admission.root != scope(&admission.tools.data_root, &admission.owner, &current)
            {
                return Err(SessionError::Conflict(
                    "Pilot Session identity changed before admission".into(),
                )
                .into());
            }
            admission.session = current;
            let sessions = admission.sessions.clone();
            let owner = admission.owner.clone();
            let session = admission.session.id.clone();
            let request_id = admission.request_id.clone();
            let prompt = admission.prompt.clone();
            let existing = tokio::task::spawn_blocking(move || {
                sessions
                    .list_turns(&owner, &session)
                    .map(|turns| turns.into_iter().any(|turn| turn.request_id == request_id))
            })
            .await
            .map_err(|_| SessionError::Storage("Pilot admission failed".into()))??;
            if !existing && let Some(source) = admission.request_id.strip_prefix("continuation:") {
                let latest = admission
                    .sessions
                    .list_turns(&admission.owner, &admission.session.id)?
                    .into_iter()
                    .next();
                if latest
                    .is_none_or(|turn| turn.id != source || turn.status != TurnStatus::Completed)
                    || continuation_request(
                        &admission.sessions,
                        &admission.owner,
                        &admission.session.id,
                        source,
                    )?
                    .as_deref()
                        != Some(&admission.request_id)
                {
                    return Err(SessionError::Conflict(
                        "automatic continuation source is no longer the latest completed Turn"
                            .into(),
                    )
                    .into());
                }
            }
            if !existing && admission.session.status == SessionStatus::Archived {
                return Err(
                    SessionError::Conflict("archived sessions cannot accept turns".into()).into(),
                );
            }
            if !existing
                && self
                    .active
                    .lock()
                    .map_err(|_| SessionError::Storage("Pilot control failed".into()))?
                    .contains_key(&admission.root)
            {
                return Err(SessionError::Conflict(
                    "Pilot Session still has active execution".into(),
                )
                .into());
            }
            let goose = if existing {
                None
            } else {
                let turns = admission
                    .sessions
                    .list_turns(&admission.owner, &admission.session.id)?;
                if crate::goose_acp::runtime_mode() != "goose" {
                    eprintln!(
                        "anchor-runner-host: Pilot provider unavailable: runtime mode is not Goose"
                    );
                    return Err(AdmissionError::Provider);
                }
                if turns.iter().any(|turn| turn.native.is_some()) {
                    return Err(SessionError::Conflict(
                        "legacy Pilot turns cannot silently switch to Goose".into(),
                    )
                    .into());
                }
                Some(
                    crate::goose_acp::pilot::GoosePilot::from_env(
                        &admission.root,
                        turns.iter().find_map(|turn| turn.goose.as_ref()),
                    )
                    .map_err(|reason| {
                        eprintln!("anchor-runner-host: Pilot provider unavailable: {reason}");
                        AdmissionError::Provider
                    })?,
                )
            };
            let sessions = admission.sessions.clone();
            let owner = admission.owner.clone();
            let session = admission.session.id.clone();
            let request_id = admission.request_id.clone();
            let (turn, created) = tokio::task::spawn_blocking(move || {
                sessions.create_turn(&owner, &session, &request_id, prompt.as_deref())
            })
            .await
            .map_err(|_| SessionError::Storage("Pilot admission failed".into()))??;
            if !created {
                return Ok(turn);
            }
            let goose = goose
                .ok_or_else(|| SessionError::Storage("Pilot admission identity changed".into()))?;
            if let Err(reason) = crate::pilot_resources::register(
                &admission.tools.data_root,
                &admission.tools.workspace_root,
                &admission.owner,
                &admission.session,
            ) {
                admission.sessions.finish_turn(
                    &admission.owner,
                    &admission.session.id,
                    &turn.id,
                    TurnStatus::Interrupted,
                    Some(&reason),
                )?;
                return Err(SessionError::Storage(reason).into());
            }
            // Persist the mount policy before acknowledging this Turn. The MCP
            // connections below reuse this snapshot, even if a human changes the
            // Session switches while startup is waiting for a remote server.
            let frozen = crate::plugin_mounts::SessionMounts::open(&admission.tools.data_root)
                .and_then(|source| {
                    crate::node_host::freeze_session_switches(
                        &admission.tools.data_root,
                        &admission.session.id,
                        &turn.id,
                        &source,
                    )
                });
            match frozen {
                Ok(_) => {}
                Err(reason) => {
                    admission.sessions.finish_turn(
                        &admission.owner,
                        &admission.session.id,
                        &turn.id,
                        TurnStatus::Interrupted,
                        Some(&reason),
                    )?;
                    return Err(SessionError::Storage(reason).into());
                }
            }
            let cancellation = Arc::new(AtomicBool::new(false));
            match self.active.lock() {
                Ok(mut active) => {
                    active.insert(admission.root.clone(), cancellation.clone());
                }
                Err(_) => {
                    admission.sessions.finish_turn(
                        &admission.owner,
                        &admission.session.id,
                        &turn.id,
                        TurnStatus::Interrupted,
                        Some("Pilot control failed"),
                    )?;
                    return Err(SessionError::Storage("Pilot control failed".into()).into());
                }
            }
            let turn_id = turn.id.clone();
            let request = crate::goose_acp::pilot::PilotTurn {
                root: admission.root.clone(),
                sessions: admission.sessions.clone(),
                owner: admission.owner.clone(),
                session: admission.session.id.clone(),
                turn: turn_id.clone(),
                prompt: admission.prompt.take().unwrap_or_else(|| {
                    "继续这个会话。先核查中断前的事实和现场，不要重放结果未知的操作。".into()
                }),
                instructions: INSTRUCTIONS.into(),
                cancellation,
            };
            let tools = admission.tools.clone().for_turn(&turn_id);
            let source_turn = turn_id.clone();
            let continuation = PilotAdmission {
                sessions: admission.sessions.clone(),
                owner: admission.owner.clone(),
                session: admission.session.clone(),
                request_id: String::new(),
                prompt: None,
                root: admission.root.clone(),
                tools: admission.tools.clone(),
            };
            let service = self.clone();
            tokio::spawn(async move {
                let root = admission.root.clone();
                let result = tokio::task::spawn_blocking(move || {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|_| "Pilot executor could not start".to_owned())?;
                    runtime.block_on(execute_turn(goose, request, tools))
                })
                .await;
                let (status, failure) = match result {
                Ok(Ok(status)) => (status, None),
                Ok(Err(message)) => {
                    eprintln!("Pilot executor failed: {message}");
                    (TurnStatus::Interrupted, Some("Pilot execution interrupted; continue with a new message to inspect the saved facts".to_owned()))
                }
                _ => (TurnStatus::Interrupted, Some("Pilot execution interrupted; continue with a new message to inspect the saved facts".to_owned())),
            };
                let _gate = service.gate.lock().await;
                let saved = tokio::task::spawn_blocking(move || {
                    admission.sessions.finish_turn(
                        &admission.owner,
                        &admission.session.id,
                        &turn_id,
                        status,
                        failure.as_deref(),
                    )
                })
                .await;
                if !matches!(saved, Ok(Ok(_))) {
                    eprintln!("Pilot terminal state could not be persisted");
                }
                if let Ok(mut active) = service.active.lock() {
                    active.remove(&root);
                }
                drop(_gate);
                if status == TurnStatus::Completed && matches!(saved, Ok(Ok(_))) {
                    let pending = continuation_request(
                        &continuation.sessions,
                        &continuation.owner,
                        &continuation.session.id,
                        &source_turn,
                    );
                    if let Ok(Some(request_id)) = pending {
                        let next=PilotAdmission {sessions:continuation.sessions.clone(),owner:continuation.owner.clone(),session:continuation.session.clone(),request_id,prompt:Some("Host 已在上一轮结束后应用用户授权或已准备的共享运行时。继续用户原任务：先核查 platform_status 与现场，使用此轮重新冻结的能力；不要重放结果未知的外部操作。".into()),root,tools:continuation.tools};
                        if let Err(error) = service.admit(next).await {
                            let reason = match error {
                                AdmissionError::Session(error) => error.to_string(),
                                AdmissionError::Provider => "Pilot provider unavailable".into(),
                            };
                            let _ = continuation.sessions.append_event(
                                &continuation.owner,
                                &continuation.session.id,
                                "pilot.continuation.failed",
                                std::collections::BTreeMap::from([(
                                    "turn".into(),
                                    serde_json::json!(source_turn),
                                )]),
                            );
                            eprintln!(
                                "Pilot automatic continuation could not be admitted: {reason}"
                            );
                        }
                    }
                }
            });
            Ok(turn)
        })
    }

    pub(crate) async fn stop(&self, root: &std::path::Path) -> bool {
        let _gate = self.gate.lock().await;
        if let Ok(active) = self.active.lock()
            && let Some(cancellation) = active.get(root)
        {
            cancellation.store(true, Ordering::Release);
            return true;
        }
        false
    }

    pub(crate) async fn delete_session(
        &self,
        sessions: SessionStore,
        data: PathBuf,
        workspaces: PathBuf,
        library: PathBuf,
        owner: String,
        session: Session,
    ) -> Result<(), SessionError> {
        // Admission, finish and cleanup share one gate. Reload under it because
        // the API's earlier ownership lookup may precede another Turn.
        let _gate = self.gate.lock().await;
        let current = sessions.get(&owner, &session.id)?;
        if current.created_at != session.created_at {
            return Err(SessionError::Conflict(
                "Pilot Session identity changed before deletion".into(),
            ));
        }
        let root = scope(&data, &owner, &current);
        if self
            .active
            .lock()
            .map_err(|_| SessionError::Storage("Pilot control failed".into()))?
            .contains_key(&root)
        {
            return Err(SessionError::Conflict(
                "Pilot Session still has active execution".into(),
            ));
        }
        tokio::task::spawn_blocking(move || {
            crate::application::development_dependencies::release_development_scope(
                &library, &root,
            )
            .map_err(SessionError::Conflict)?;
            crate::credential_store::delete_requests(&data, &owner, &current)
                .map_err(SessionError::Storage)?;
            crate::oauth_links::delete_session(&data, &owner, &current)
                .map_err(SessionError::Storage)?;
            crate::pilot_resources::delete(&sessions, &data, &workspaces, &owner, &current)
        })
        .await
        .map_err(|_| SessionError::Storage("Pilot cleanup task failed".into()))?
    }
}

fn continuation_request(
    sessions: &SessionStore,
    owner: &str,
    session: &str,
    turn: &str,
) -> Result<Option<String>, SessionError> {
    let request = sessions
        .events(owner, session, 0)?
        .into_iter()
        .rev()
        .find(|event| {
            event.kind == "pilot.continuation.requested"
                && event.data.get("turn").and_then(serde_json::Value::as_str) == Some(turn)
        })
        .and_then(|event| {
            event
                .data
                .get("request_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        });
    let Some(request) = request else {
        return Ok(None);
    };
    if sessions
        .list_turns(owner, session)?
        .iter()
        .any(|turn| turn.request_id == request)
    {
        return Ok(None);
    }
    Ok(Some(request))
}

pub(crate) fn continuation_status(
    sessions: &SessionStore,
    owner: &str,
    session: &str,
    turn: &str,
) -> Result<serde_json::Value, SessionError> {
    let request_id = format!("continuation:{turn}");
    let turns = sessions.list_turns(owner, session)?;
    if let Some(next) = turns.iter().find(|next| next.request_id == request_id) {
        return Ok(serde_json::json!({"status":"admitted","turn":next.id}));
    }
    let requested = sessions.events(owner, session, 0)?.iter().any(|event| {
        event.kind == "pilot.continuation.requested"
            && event.data.get("turn").and_then(serde_json::Value::as_str) == Some(turn)
    });
    if !requested {
        return Ok(serde_json::json!({"status":"none"}));
    }
    let current = sessions.get_turn(owner, session, turn)?;
    let failed = sessions.events(owner, session, 0)?.iter().any(|event| {
        event.kind == "pilot.continuation.failed"
            && event.data.get("turn").and_then(serde_json::Value::as_str) == Some(turn)
    });
    let status = if current.status == TurnStatus::Stopped {
        "cancelled"
    } else if turns.first().is_none_or(|latest| latest.id != turn) {
        "superseded"
    } else if current.status == TurnStatus::Interrupted || failed {
        "pending"
    } else {
        "scheduled"
    };
    Ok(serde_json::json!({"status":status,"request_id":request_id}))
}

async fn execute_turn(
    goose: crate::goose_acp::pilot::GoosePilot,
    request: crate::goose_acp::pilot::PilotTurn,
    mut tools: PilotTools,
) -> Result<TurnStatus, String> {
    if request.cancellation.load(Ordering::Acquire) {
        return Ok(TurnStatus::Stopped);
    }
    let mut context = crate::pilot_tools::PilotTurnContext::new(
        &tools.workspace_root,
        &request.owner,
        &request.session,
        false,
        request.cancellation.clone(),
    )?;
    // Startup belongs to the registered execution, not admission: no global
    // gate is held while OAuth or MCP is waiting.
    let runtime = tools
        .application
        .runtime_snapshot()
        .map_err(|error| format!("{error:?}"))?;
    if let Some(snapshot) = &runtime {
        tools.application = tools
            .application
            .clone()
            .with_frozen_runtime_snapshot(snapshot.clone());
    }
    let (plugin_tools, network) = match crate::pilot_tools::frozen_turn_tools(
        &tools.data_root,
        &tools.library_root,
        &tools.workspace_root,
        &request.owner,
        &request.session,
        &request.turn,
        request.cancellation.clone(),
        runtime.as_ref(),
    )
    .await
    {
        Ok((ports, reasons, network)) => {
            for reason in reasons {
                eprintln!(
                    "anchor-runner-host: Pilot Turn {} Plugin: {reason}",
                    request.turn
                );
            }
            (ports, network)
        }
        Err(reason) => {
            if request.cancellation.load(Ordering::Acquire) {
                return if reason == "Pilot tool startup was cancelled" {
                    Ok(TurnStatus::Stopped)
                } else {
                    Err(reason)
                };
            }
            eprintln!(
                "anchor-runner-host: Pilot Turn {} has no Plugin tools: {reason}",
                request.turn
            );
            (Vec::new(), false)
        }
    };
    let outcome = if request.cancellation.load(Ordering::Acquire) {
        Ok(TurnStatus::Stopped)
    } else {
        context.freeze_network(network);
        if let Some(snapshot) = runtime.as_ref() {
            context.configure(snapshot, &tools.library_root, &request.root)?;
        }
        tools.context = Some(Arc::new(context));
        let execution_plugins = plugin_tools
            .iter()
            .cloned()
            .map(|port| port as Arc<dyn anchor_runtime::ToolPort>)
            .collect();
        goose.run(request, Arc::new(tools), execution_plugins).await
    };
    // Goose has closed its bridge and released the dispatch references. Reap
    // every admitted Plugin before finish can release this Session for another
    // Turn or deletion, including cancellation after startup returned its ports.
    let mut cleanup = Vec::new();
    for port in plugin_tools {
        match Arc::try_unwrap(port) {
            Ok(port) => {
                if let Err(reason) = port.shutdown().await {
                    cleanup.push(reason);
                }
            }
            Err(_) => cleanup.push("MCP port still has an active reference".into()),
        }
    }
    if cleanup.is_empty() {
        outcome
    } else {
        Err(format!(
            "Pilot Plugin cleanup failed: {}",
            cleanup.join("; ")
        ))
    }
}

const INSTRUCTIONS: &str = "你是 Anchor Pilot，帮助用户理解、管理 Graph、Plugin、Run 和产物。先回答用户实际问题，讨论能力不等于授权操作；只有用户明确要求才创建/更新 Graph、启动或控制 Run。只读取当前问题直接相关的资源。用户明确要求获取、配置或开发 Plugin 时，先检查 plugin_list/plugin_read，按实际需求复用或在本 Session 的 /workspace 获取源码、编辑和构建。anchor_read/anchor_edit 的 path 与 plugin_review/plugin_install 的 directory 都相对此工作区，跨 Turn 保留；可用 anchor_run 执行授权命令，联网仅按本 Turn 冻结权限，候选声明不能给自己授权。空环境先核查 platform_status。需要联网时用 session_network 发起原生确认；成功后结束当前 Turn，让 Host 自动在同一 Session 继续，新的授权下一 Turn 生效。基础 Python/Rust 尚未准备时用 runtime_prepare，由 Host 准备共享解释器或工具链；changed 为 true 时结束当前 Turn 自动继续。第三方 Python 库和开发测试工具按工作区项目调用 development_prepare，分别传 requirements 和 development_requirements；普通脚本、python -m pytest、控制台命令与 shell 编排用 anchor_run 的 project 选择项目。共享解释器与依赖视图只读，不用 pip 原地改写。正式运行依赖只写 plugin.json 顶层 dependencies.python，再用 plugin_prepare/plugin_test 准备并真实自测；开发测试依赖不自动进入正式声明。Rust 复用授权共享 Cargo/rustup 工具链，按 Cargo.toml/Cargo.lock 构建，把 bin 入口、源码和构建说明整理成独立候选。修改后重新测试摘要。plugin_test 只证明其实际返回结果，列工具不等于业务测试通过。plugin_install 沿用原生确认，安装不自动挂载；需要启用时用 plugin_mount 发起对话原生确认，成功后结束当前 Turn，由 Host 自动继续并装配新的工具。外部凭据用 credential_request 的专用私密控件，模型不得索取或回显凭据值；只根据配置状态继续。OAuth先用 oauth_status 检查，再用 oauth_authorize 返回的链接引导用户在浏览器授权；Host保存凭据并在同一Session继续，模型只看授权状态。渠道先用 channel_status 核查；用户要求时通过 channel_configure 和 channel_enable 的原生确认配置、启用，渠道所需私密字段同样用 credential_request。只讨论方案时不要执行这些动作。保存 Graph 不等于启动 Graph；graph_run 仅接纳并返回 Run ID，不等于执行完成。暂停和停止是安全边界上的请求，应通过 run_status 核查最终状态。需要补充信息时使用 ask_user 原生表单提问，等待用户回答；删除只用 graph_delete，它会再次请求用户确认绑定的精确 Graph 内容。用户拒绝或取消不删除，内容变化需重新确认。本切片尚不提供对外发布，不得声称完成未提供的操作。只有真实工具结果才证明动作；无法获取的事实如实说明。恢复或续聊先核查保存的事实和 session_wait/run_status/graph_read，不盲目重放未知结果或旧确认。使用 #anchor/graph/<id>、#anchor/run/<id>、#anchor/artifact/<run>/<node>/<path> 链接引用对象。";

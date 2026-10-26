//! Host-owned mutable project environments. Only the env subtree is writable
//! in a command; selection, interpreter binding and locks stay outside it.

use super::{ApplicationError, RunApplication, development_dependencies};
use crate::dependency_provision::{DependencyAccess, DependencyProvisioner};
use crate::plugin_library::PluginLibrary;
use anchor_library::DevelopmentScope;
use anchor_runtime::{
    NetworkPolicy, ReadOnlyInput, SandboxEnvironment, SandboxPort, SandboxRequest, SandboxResult,
    SandboxStatus, WritableInput,
};
use anchor_sandbox_bwrap::BubblewrapSandbox;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

pub(crate) const ENVIRONMENT_PATH: &str = "/development/env";
const ENVIRONMENT_BIN: &str = "/development/env/bin";

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn invalid(error: impl std::fmt::Display) -> ApplicationError {
    ApplicationError::Invalid(error.to_string())
}
fn project_root(scope: &Path, project: &str) -> PathBuf {
    scope
        .join("development/projects")
        .join(hash(project.as_bytes()))
}

fn present(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PrepareAction {
    Ensure,
    Rebuild,
    Discard,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SelectionState {
    Private,
    Discarded,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    format: u32,
    scope: PathBuf,
    project: String,
    state: SelectionState,
    interpreter: PathBuf,
    interpreter_digest: String,
    #[serde(default)]
    operation_in_progress: bool,
    #[serde(default)]
    inspection_required: bool,
    #[serde(default)]
    last_status: Option<String>,
}

fn selection(root: &Path, scope: &Path, project: &str) -> Result<Option<Selection>, String> {
    let selected = crate::assistant::read_json::<Selection>(&root.join("selection.json"))?;
    if selected.as_ref().is_some_and(|fact| {
        fact.format != 1
            || fact.scope != scope
            || fact.project != project
            || !fact.interpreter.is_absolute()
            || fact.interpreter_digest.len() != 64
    }) {
        return Err("development environment ownership or selection changed; inspect the project before rebuilding".into());
    }
    Ok(selected)
}

fn save_selection(root: &Path, selected: &Selection) -> Result<(), String> {
    let mut temporary = tempfile::NamedTempFile::new_in(root).map_err(|e| e.to_string())?;
    temporary
        .write_all(&serde_json::to_vec(selected).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    temporary
        .persist(root.join("selection.json"))
        .map_err(|e| e.to_string())?;
    File::open(root)
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())
}

fn remove_env(env: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(env) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            crate::host_fs::directory(env)?;
            std::fs::remove_dir_all(env).map_err(|e| e.to_string())
        }
        Ok(_) => Err(
            "development environment root is not a private directory; inspect it before rebuilding"
                .into(),
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn checked_env(root: &Path, selected: &Selection, interpreter: &Path) -> Result<PathBuf, String> {
    let env = root.join("env");
    // Never execute writable Python/.pth/build code on the Host to probe health.
    // These checks establish the binding and basic layout, not installed health.
    if selected.interpreter != interpreter
        || hash(&std::fs::read(interpreter).map_err(|e| e.to_string())?)
            != selected.interpreter_digest
    {
        return Err("development environment is bound to a different or changed frozen interpreter; use explicit rebuild to change the binding".into());
    }
    for directory in [&env, &env.join("bin"), &env.join("lib")] {
        let metadata = std::fs::symlink_metadata(directory).map_err(|e| {
            format!(
                "development environment layout is damaged ({e}); inspect it or explicitly rebuild"
            )
        })?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(
                "development environment layout is damaged; inspect it or explicitly rebuild"
                    .into(),
            );
        }
    }
    let cfg = env.join("pyvenv.cfg");
    let metadata = std::fs::symlink_metadata(&cfg).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 16 * 1024 {
        return Err("development environment has no bounded regular pyvenv.cfg".into());
    }
    let cfg = std::fs::read_to_string(cfg).map_err(|e| e.to_string())?;
    if !cfg
        .lines()
        .any(|line| line.trim() == "include-system-site-packages = false")
        || env
            .join("bin/python")
            .canonicalize()
            .map_err(|e| e.to_string())?
            != interpreter
    {
        return Err("development environment interpreter or isolation layout changed; inspect it or explicitly rebuild".into());
    }
    let pip = std::fs::symlink_metadata(env.join("bin/pip")).map_err(|e| {
        format!("private pip entry is unavailable ({e}); inspect it or explicitly rebuild")
    })?;
    use std::os::unix::fs::PermissionsExt;
    if !pip.is_file() || pip.file_type().is_symlink() || pip.permissions().mode() & 0o111 == 0 {
        return Err("private pip entry is unavailable; inspect it or explicitly rebuild".into());
    }
    Ok(env)
}

async fn lease(
    library: &Path,
    scope: &Path,
    access: &DependencyAccess,
) -> Result<DevelopmentScope, String> {
    access.check().map_err(|e| e.to_string())?;
    let library_handle = PluginLibrary::new(library);
    if let Some(leased) = library_handle
        .development_scope(scope, false)
        .map_err(|e| e.to_string())?
    {
        return Ok(leased);
    }
    let leased = library_handle
        .development_scope(scope, true)
        .map_err(|e| e.to_string())?
        .ok_or("development scope unavailable")?;
    let dependencies = anchor_graph_host::resolved_library_root(library)
        .join(anchor_graph_host::DEPENDENCIES_DIRECTORY);
    {
        let _writer = crate::plugin_dependencies::write_lock(&dependencies, access)
            .await
            .map_err(|e| e.to_string())?;
        leased.register().map_err(|e| e.to_string())?;
    }
    Ok(leased)
}

pub(crate) enum EnvironmentKind {
    Private(PathBuf),
    Legacy(development_dependencies::LeasedDevelopmentView),
}

/// Scope lease prevents deletion, project lock prevents preparation or another
/// command changing the env until all Sandbox children have exited.
pub(crate) struct LeasedEnvironment {
    pub(crate) kind: EnvironmentKind,
    _project: anchor_file_lease::FileLease,
    _scope: DevelopmentScope,
    root: PathBuf,
    scope: PathBuf,
    project: String,
}

impl LeasedEnvironment {
    pub(crate) fn begin_operation(&self) -> Result<(), String> {
        if self.is_private() {
            begin_operation(&self.root, &self.scope, &self.project)?;
        }
        Ok(())
    }

    pub(crate) fn finish_operation(
        &self,
        status: &str,
        exit_code: Option<i32>,
    ) -> Result<Value, String> {
        if self.is_private() {
            finish_operation(&self.root, &self.scope, &self.project, status, exit_code)
        } else {
            Ok(Value::Null)
        }
    }
    pub(crate) fn apply(
        &self,
        sandbox: Arc<BubblewrapSandbox>,
        request: &mut SandboxRequest,
        python_alias: bool,
    ) -> Result<Arc<BubblewrapSandbox>, String> {
        let (bin, scoped) = match &self.kind {
            EnvironmentKind::Private(env) => {
                let grant = WritableInput::new(env, ENVIRONMENT_PATH);
                let scoped = sandbox
                    .with_writable_grants(std::slice::from_ref(&grant))
                    .and_then(|sandbox| sandbox.with_executable_dirs(&[env.join("bin")]))
                    .map_err(|e| e.to_string())?;
                request.writable_inputs.push(grant);
                request.tool_dirs.push(env.join("bin"));
                set_environment(request, "VIRTUAL_ENV", ENVIRONMENT_PATH);
                (PathBuf::from(ENVIRONMENT_BIN), Arc::new(scoped))
            }
            EnvironmentKind::Legacy(legacy) => {
                let scoped = legacy.view.authorize(sandbox)?;
                legacy.view.apply(request);
                (legacy.view.view().join("bin"), scoped)
            }
        };
        let previous = request
            .environment
            .iter()
            .find(|e| e.key == "PATH")
            .map(|e| e.value().to_owned())
            .unwrap_or_else(|| "/usr/bin:/bin".into());
        set_environment(request, "PATH", &format!("{}:{previous}", bin.display()));
        set_environment(request, "PYTHONNOUSERSITE", "1");
        // Disallow inherited user/global pip configuration; project-specific
        // flags/files remain ordinary pip arguments in the command sandbox.
        set_environment(request, "PIP_CONFIG_FILE", "/dev/null");
        set_environment(request, "PIP_DISABLE_PIP_VERSION_CHECK", "1");
        if python_alias {
            request.command[0] = bin.join("python").to_string_lossy().into_owned();
        } else if let Some(command) = request.command.first().filter(|c| !c.contains('/')) {
            use std::os::unix::fs::PermissionsExt;
            let host_bin = match &self.kind {
                EnvironmentKind::Private(env) => env.join("bin"),
                EnvironmentKind::Legacy(legacy) => legacy.view.view().join("bin"),
            };
            if std::fs::metadata(host_bin.join(command))
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            {
                request.command[0] = bin.join(command).to_string_lossy().into_owned();
            }
        }
        Ok(scoped)
    }

    pub(crate) fn is_private(&self) -> bool {
        matches!(self.kind, EnvironmentKind::Private(_))
    }
}

fn set_environment(request: &mut SandboxRequest, key: &str, value: &str) {
    request.environment.retain(|entry| entry.key != key);
    request
        .environment
        .push(SandboxEnvironment::new(key, value));
}

pub(crate) async fn select_environment(
    library: &Path,
    scope: &Path,
    project: &str,
    interpreter: Option<&Path>,
    access: &DependencyAccess,
) -> Result<LeasedEnvironment, String> {
    let project = development_dependencies::normalize_project(project)?;
    let scope_path = std::path::absolute(scope).map_err(|e| e.to_string())?;
    let leased = lease(library, &scope_path, access).await?;
    let root = project_root(&scope_path, &project);
    let project_lock = crate::plugin_dependencies::write_lock(&root, access)
        .await
        .map_err(|e| e.to_string())?;
    let interpreter = interpreter.ok_or("Pilot Python is not authorized in this frozen Turn")?;
    let selected = selection(&root, &scope_path, &project)?;
    if selected.is_none() && present(&root.join("env"))? {
        return Err("private environment exists but its Host selection is missing; inspect the retained state and explicitly rebuild before executing, rather than falling back to a legacy view".into());
    }
    let kind = match selected {
        Some(selected) if selected.state == SelectionState::Private => {
            EnvironmentKind::Private(checked_env(&root, &selected, interpreter)?)
        }
        Some(_) => return Err("project environment was discarded; call development_prepare to create an empty private environment".into()),
        None => {
            let legacy = development_dependencies::select_development_view(library, &scope_path, &project)?;
            if legacy.view.view().join("bin/python").canonicalize().map_err(|e| e.to_string())? != interpreter {
                return Err("legacy development view uses a different frozen interpreter; explicit rebuild is required to change the binding".into());
            }
            EnvironmentKind::Legacy(legacy)
        }
    };
    Ok(LeasedEnvironment {
        kind,
        _project: project_lock,
        _scope: leased,
        root,
        scope: scope_path,
        project,
    })
}

fn observation(selected: &Selection) -> Value {
    json!({"operation_in_progress":selected.operation_in_progress,
        "inspection_required":selected.inspection_required || selected.operation_in_progress,
        "last_status":selected.last_status,
        "guidance": if selected.inspection_required || selected.operation_in_progress {
            "A previous command may have partially changed this environment. Inspect pip list/check and project state before choosing the next change; do not replay from an old receipt. This notice does not block inspection or execution."
        } else { "This observation records command settlement, not package health or test success." }})
}

fn begin_operation(root: &Path, scope: &Path, project: &str) -> Result<(), String> {
    let mut selected = selection(root, scope, project)?.ok_or("private selection unavailable")?;
    selected.inspection_required |= selected.operation_in_progress;
    selected.operation_in_progress = true;
    selected.last_status = Some("in_progress".into());
    save_selection(root, &selected)
}

fn finish_operation(
    root: &Path,
    scope: &Path,
    project: &str,
    status: &str,
    exit_code: Option<i32>,
) -> Result<Value, String> {
    let mut selected = selection(root, scope, project)?.ok_or("private selection unavailable")?;
    selected.operation_in_progress = false;
    selected.inspection_required |=
        status != "not_executed" && (status != "completed" || exit_code != Some(0));
    selected.last_status = Some(status.into());
    save_selection(root, &selected)?;
    Ok(observation(&selected))
}

#[allow(clippy::too_many_arguments)]
impl RunApplication {
    pub(crate) async fn prepare_development_environment(
        &self,
        library: &Path,
        scope: &Path,
        workspace: &Path,
        project: &str,
        action: PrepareAction,
        requirements: &[String],
        development: &[String],
        access: &DependencyAccess,
    ) -> Result<Value, ApplicationError> {
        let project = development_dependencies::normalize_project(project).map_err(invalid)?;
        let scope_path = std::path::absolute(scope).map_err(invalid)?;
        let leased = lease(library, &scope_path, access).await.map_err(invalid)?;
        let root = project_root(&scope_path, &project);
        let _project_lock = crate::plugin_dependencies::write_lock(&root, access)
            .await
            .map_err(invalid)?;
        let selected = selection(&root, &scope_path, &project).map_err(invalid)?;
        if action == PrepareAction::Rebuild && (!requirements.is_empty() || !development.is_empty())
        {
            return Err(invalid(
                "rebuild creates only an empty environment with pip; install dependencies afterwards in a separate anchor_run(project=...) pip command",
            ));
        }
        if requirements.len() + development.len() > 64
            || requirements.iter().chain(development).any(|entry| {
                entry.is_empty()
                    || entry.len() > 512
                    || entry.trim() != entry
                    || entry.chars().any(char::is_control)
                    || entry.starts_with('-')
            })
        {
            return Err(invalid(
                "inline dependencies must contain at most 64 requirements; use anchor_run pip arguments for files/options",
            ));
        }
        if action == PrepareAction::Discard {
            if !requirements.is_empty() || !development.is_empty() {
                return Err(invalid("discard cannot also install dependencies"));
            }
            if selected.is_none() && present(&root.join("env")).map_err(invalid)? {
                return Err(invalid(
                    "private environment exists but its Host selection is missing; discard cannot claim a successful release. Inspect the retained state and explicitly rebuild its binding before discarding",
                ));
            }
            if let Some(mut selected) = selected {
                remove_env(&root.join("env")).map_err(invalid)?;
                selected.state = SelectionState::Discarded;
                save_selection(&root, &selected).map_err(invalid)?;
            }
            remove_env(&root.join("creating")).map_err(invalid)?;
            return Ok(
                json!({"project":project,"discarded":true,"source_preserved":true,"legacy_views_preserved":true}),
            );
        }
        let provisioner = self.dependency_provisioner()?;
        let interpreter = provisioner.config.python.canonicalize().map_err(invalid)?;
        if action == PrepareAction::Ensure
            && selected.is_none()
            && present(&root.join("env")).map_err(invalid)?
        {
            return Err(invalid(
                "private environment exists but its Host selection is missing; inspect the retained state and use explicit rebuild, rather than falling back or clearing it",
            ));
        }
        if action == PrepareAction::Ensure
            && selected.is_none()
            && let Some(legacy) =
                development_dependencies::legacy_preparation(&leased, &project).map_err(invalid)?
        {
            if legacy.interpreter_path() != interpreter {
                return Err(invalid(
                    "legacy development view uses a different frozen interpreter; explicit rebuild is required to change the binding",
                ));
            }
            development_dependencies::select_development_view(library, &scope_path, &project)
                .map_err(invalid)?;
            if !requirements.is_empty() || !development.is_empty() {
                return Err(invalid(
                    "legacy project uses a read-only dependency view; explicit rebuild is required before private pip installation",
                ));
            }
            let mut result = legacy.result(access.network);
            result["environment_kind"] = json!("legacy_read_only");
            result["pip_available"] = json!(false);
            result["guidance"] = json!(
                "Legacy view preserved. Explicit rebuild creates an empty private venv; install dependencies afterwards with anchor_run(project=...) pip."
            );
            return Ok(result);
        }
        let env = root.join("env");
        let created = action == PrepareAction::Rebuild
            || selected
                .as_ref()
                .is_none_or(|s| s.state == SelectionState::Discarded);
        if created {
            // Explicit rebuild may lose trial packages. Unpublished creation
            // remnants can retry; a published damaged env never reaches here
            // through default ensure.
            let creating = root.join("creating");
            remove_env(&creating).map_err(invalid)?;
            crate::host_fs::directory(&creating).map_err(invalid)?;
            create_environment(&provisioner, &creating, access)
                .await
                .map_err(invalid)?;
            access.check().map_err(invalid)?;
            // Only a successfully bootstrapped temporary env replaces the
            // current one. Stable sandbox paths make its scripts relocatable.
            remove_env(&env).map_err(invalid)?;
            std::fs::rename(&creating, &env).map_err(invalid)?;
            let fact = Selection {
                format: 1,
                scope: scope_path.clone(),
                project: project.clone(),
                state: SelectionState::Private,
                interpreter: interpreter.clone(),
                interpreter_digest: hash(&std::fs::read(&interpreter).map_err(invalid)?),
                operation_in_progress: false,
                inspection_required: false,
                last_status: None,
            };
            checked_env(&root, &fact, &interpreter).map_err(invalid)?;
            save_selection(&root, &fact).map_err(invalid)?;
        } else {
            checked_env(
                &root,
                selected.as_ref().expect("private selection"),
                &interpreter,
            )
            .map_err(invalid)?;
        }
        let mut result = json!({"prepared":true,"project":project,"action":match action { PrepareAction::Ensure=>"ensure", PrepareAction::Rebuild=>"rebuild", PrepareAction::Discard=>unreachable!()},
            "created":created,"environment_kind":"private","environment_path":ENVIRONMENT_PATH,"pip_available":true,"pip_bootstrap":"bundled ensurepip from frozen shared interpreter",
            "interpreter":interpreter,"network_allowed":access.network,"dependencies_verified":false,
            "guidance":"Environment entry prepared; inspect installed state with pip list/pip check and run tests. ensure preserves trial packages. Interrupted pip may have partially changed the environment; inspect it before continuing, do not replay from an old receipt."});
        if !requirements.is_empty() || !development.is_empty() {
            begin_operation(&root, &scope_path, &project).map_err(invalid)?;
            let installed = match install_inline(
                &provisioner,
                &env,
                workspace,
                &project,
                requirements,
                development,
                access,
            )
            .await
            {
                Ok(result) => result,
                Err(error) => {
                    finish_operation(&root, &scope_path, &project, "failed", None)
                        .map_err(invalid)?;
                    return Err(invalid(format!(
                        "inline installation could not complete: {error}; inspect the private environment before continuing"
                    )));
                }
            };
            finish_operation(
                &root,
                &scope_path,
                &project,
                status(&installed),
                installed.exit_code,
            )
            .map_err(invalid)?;
            result["installation"] = json!({"status":status(&installed),"exit_code":installed.exit_code,
                "stdout":installed.stdout,"stderr":installed.stderr,"incomplete":installed.incomplete});
            result["dependencies_installed"] = json!(
                installed.status == SandboxStatus::Completed && installed.exit_code == Some(0)
            );
            if installed.status != SandboxStatus::Completed || installed.exit_code != Some(0) {
                result["partial_changes_possible"] = json!(true);
            }
        }
        result["environment_observation"] = observation(
            &selection(&root, &scope_path, &project)
                .map_err(invalid)?
                .ok_or_else(|| invalid("private selection unavailable"))?,
        );
        Ok(result)
    }
}

fn status(result: &SandboxResult) -> &'static str {
    match result.status {
        SandboxStatus::Completed => "completed",
        SandboxStatus::Cancelled => "cancelled",
        SandboxStatus::TimedOut => "timed_out",
        SandboxStatus::NotExecuted => "not_executed",
    }
}

async fn create_environment(
    provisioner: &DependencyProvisioner,
    env: &Path,
    access: &DependencyAccess,
) -> Result<(), String> {
    let config = &provisioner.config;
    let uv = config.verified_uv().map_err(|e| e.to_string())?;
    let interpreter = config.python.canonicalize().map_err(|e| e.to_string())?;
    let mut environment = crate::tool_environment::ToolEnvironment::default();
    environment.configure_pilot_python(&interpreter)?;
    let grant = WritableInput::new(env, ENVIRONMENT_PATH);
    let uv_grant = ReadOnlyInput::new(&uv, &uv);
    let sandbox = environment
        .grant_sandbox(&provisioner.sandbox)?
        .with_readonly_grants(std::slice::from_ref(&uv_grant))
        .map_err(|e| e.to_string())?
        .with_writable_grants(std::slice::from_ref(&grant))
        .map_err(|e| e.to_string())?;
    let workspace = tempfile::tempdir_in(&config.build_root).map_err(|e| e.to_string())?;
    let mut request = SandboxRequest::new(
        workspace.path(),
        vec![
            uv.to_string_lossy().into_owned(),
            "venv".into(),
            "--offline".into(),
            "--no-config".into(),
            "--python".into(),
            interpreter.to_string_lossy().into_owned(),
            ENVIRONMENT_PATH.into(),
        ],
    );
    environment.apply(&mut request);
    request
        .tool_dirs
        .push(uv.parent().ok_or("uv has no parent")?.to_path_buf());
    request.readonly_inputs.push(uv_grant);
    request.writable_inputs.push(grant.clone());
    request.cancellation = access.cancellation.clone();
    request.timeout = config.step_timeout();
    request.max_output_bytes = config.max_step_output();
    set_environment(&mut request, "UV_PYTHON_DOWNLOADS", "never");
    set_environment(&mut request, "UV_LINK_MODE", "copy");
    let result = sandbox.run(request).await.map_err(|e| e.to_string())?;
    successful(&result, "empty venv creation")?;
    let sandbox = sandbox
        .with_executable_dirs(&[env.join("bin")])
        .map_err(|e| e.to_string())?;
    let mut request = SandboxRequest::new(
        workspace.path(),
        [
            format!("{ENVIRONMENT_BIN}/python"),
            "-I".into(),
            "-m".into(),
            "ensurepip".into(),
            "--default-pip".into(),
        ],
    );
    environment.apply(&mut request);
    request.tool_dirs.push(env.join("bin"));
    request.writable_inputs.push(grant);
    request.cancellation = access.cancellation.clone();
    request.timeout = config.step_timeout();
    request.max_output_bytes = config.max_step_output();
    let result = sandbox.run(request).await.map_err(|e| e.to_string())?;
    successful(
        &result,
        "private pip bootstrap (requires the frozen interpreter's bundled ensurepip)",
    )
}

fn successful(result: &SandboxResult, what: &str) -> Result<(), String> {
    if result.status == SandboxStatus::Completed && result.exit_code == Some(0) {
        Ok(())
    } else {
        Err(format!(
            "{what} {} (exit {:?}): {}{}; inspect partial environment state before continuing",
            status(result),
            result.exit_code,
            result.stdout,
            result.stderr
        ))
    }
}

async fn install_inline(
    provisioner: &DependencyProvisioner,
    env: &Path,
    workspace: &Path,
    project: &str,
    requirements: &[String],
    development: &[String],
    access: &DependencyAccess,
) -> Result<SandboxResult, String> {
    // Use the task command sandbox, not Host resolution or the formal wheel
    // builder. Inline values are installs, never exact synchronization.
    let mut environment = crate::tool_environment::ToolEnvironment::default();
    environment.configure_pilot_python(&provisioner.config.python)?;
    let policy = anchor_sandbox_bwrap::BubblewrapPolicy::new(&provisioner.config.bwrap, ["sh"])
        .authorize_workspace_root(workspace);
    let policy = environment.authorize(if access.network {
        policy.allow_network()
    } else {
        policy
    });
    let sandbox = BubblewrapSandbox::new(policy).map_err(|e| e.to_string())?;
    let grant = WritableInput::new(env, ENVIRONMENT_PATH);
    let sandbox = environment
        .grant_sandbox(&sandbox)?
        .with_writable_grants(std::slice::from_ref(&grant))
        .map_err(|e| e.to_string())?
        .with_executable_dirs(&[env.join("bin")])
        .map_err(|e| e.to_string())?;
    let command = [
        format!("{ENVIRONMENT_BIN}/python"),
        "-m".into(),
        "pip".into(),
        "install".into(),
    ]
    .into_iter()
    .chain(requirements.iter().chain(development).cloned())
    .collect::<Vec<_>>();
    let mut request = SandboxRequest::new(workspace, command);
    environment.apply(&mut request);
    request.working_directory = Some(PathBuf::from("/workspace").join(project));
    request.writable_inputs.push(grant);
    request.tool_dirs.push(env.join("bin"));
    request.cancellation = access.cancellation.clone();
    request.network = if access.network {
        NetworkPolicy::Enabled
    } else {
        NetworkPolicy::Disabled
    };
    request.timeout = provisioner.config.step_timeout();
    request.max_output_bytes = provisioner.config.max_step_output();
    set_environment(
        &mut request,
        "PATH",
        &format!("{ENVIRONMENT_BIN}:/usr/bin:/bin"),
    );
    set_environment(&mut request, "VIRTUAL_ENV", ENVIRONMENT_PATH);
    set_environment(&mut request, "PYTHONNOUSERSITE", "1");
    set_environment(&mut request, "PIP_CONFIG_FILE", "/dev/null");
    set_environment(&mut request, "PIP_DISABLE_PIP_VERSION_CHECK", "1");
    sandbox.run(request).await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;

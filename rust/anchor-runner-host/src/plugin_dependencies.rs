//! Landing a Plugin's declared dependencies into the shared store and its view.
//!
//! The order is deliberate. Fetch first (only the artifacts whose bytes hash to
//! the approved lock), unpack each `(package, version)` into the shared store
//! once, create the view, and *then* let the caller publish the Plugin. A failed
//! solve, fetch, unpack or verification therefore leaves no Plugin installed and
//! no existing view changed — the deployment keeps exactly the state the human
//! last approved.
//!
//! Every step that runs foreign bytes runs inside the build sandbox, which is
//! constructed without network authority: the unpacking and the `uv venv` cannot
//! reach the network even if a later edit asks them to. The only network use is
//! the fetch, on the host, against the deployment's authorized sources.

use std::path::{Path, PathBuf};

use anchor_graph_host::{
    DEPENDENCIES_DIRECTORY, plugin_view_path, resolved_library_root, store_root,
};
use anchor_runtime::ReadOnlyInput;

use crate::environment_provision::{
    EnvironmentProvisioner, LockedPackage, ProvisionError, ResolvedRequirements, authority,
    fetch_wheelhouse, normalize_name, run_step,
};
use crate::plugin_view::{
    VIEW_CONFIGURATION, referenced_store_directories, site_packages_directories,
};

/// What one landed declaration produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LandedView {
    pub(crate) view: PathBuf,
    pub(crate) packages: usize,
    /// True when the view already existed and nothing was rebuilt.
    pub(crate) reused: bool,
}

/// Publish the shared store and the view for one Plugin declaration.
pub(crate) async fn land(
    provisioner: &EnvironmentProvisioner,
    library_root: &Path,
    plugin: &str,
    declaration_digest: &str,
    resolved: &ResolvedRequirements,
) -> Result<LandedView, ProvisionError> {
    let config = &provisioner.config;
    let library_root = resolved_library_root(library_root);
    let store = store_root(&library_root);
    let view = plugin_view_path(&library_root, plugin, declaration_digest);
    if let Some(landed) =
        reuse_existing(provisioner, &library_root, plugin, &view, &store, resolved).await?
    {
        return Ok(landed);
    }

    // One writer for the shared store and the views, held across the whole
    // landing so two installs cannot interleave their renames.
    let dependencies_root = library_root.join(DEPENDENCIES_DIRECTORY);
    let lock_root = dependencies_root.clone();
    let _lock = tokio::task::spawn_blocking(move || write_lock(&lock_root))
        .await
        .map_err(|error| {
            ProvisionError::Failed(format!("dependency store lock failed: {error}"))
        })??;
    // Another install may have landed the same declaration while we waited.
    if let Some(landed) =
        reuse_existing(provisioner, &library_root, plugin, &view, &store, resolved).await?
    {
        return Ok(landed);
    }

    let staging = config
        .build_root
        .join("plugin-dependencies")
        .join(resolved.lock_digest.chars().take(32).collect::<String>());
    cleanup(&staging)?;
    let workspace = staging.join("run");
    let input = workspace.join("input");
    let wheelhouse = staging.join("wheelhouse");
    std::fs::create_dir_all(&input)
        .and_then(|_| std::fs::create_dir_all(&workspace))
        .map_err(|error| ProvisionError::Failed(format!("dependency staging failed: {error}")))?;

    fetch_wheelhouse(config, &resolved.packages, &wheelhouse).await?;
    for package in &resolved.packages {
        write_requirement(&input, package)?;
    }

    let uv = config.verified_uv()?;
    let uv_dir = uv
        .parent()
        .ok_or_else(|| ProvisionError::Failed("the uv binary has no directory".to_owned()))?
        .to_path_buf();
    let interpreter = std::fs::canonicalize(&config.python).map_err(|error| {
        ProvisionError::Failed(format!(
            "the configured interpreter {} is unavailable: {error}",
            config.python.display()
        ))
    })?;
    // The wheelhouse is a delivery mechanism, not an authority: it is mounted
    // read-only and the install still runs with `--require-hashes`.
    let build = authority(
        &provisioner.sandbox,
        vec![
            ReadOnlyInput::new(&uv, &uv),
            ReadOnlyInput::new(&wheelhouse, &wheelhouse),
        ],
        &[uv_dir],
    )?;

    let uv_arg = uv.to_string_lossy().into_owned();
    let python_arg = interpreter.to_string_lossy().into_owned();
    let wheelhouse_arg = wheelhouse.to_string_lossy().into_owned();
    for package in &resolved.packages {
        let name = normalize_name(&package.name);
        run_step(
            config,
            &build,
            &workspace,
            &[
                uv_arg.clone(),
                "pip".into(),
                "install".into(),
                "--no-deps".into(),
                "--no-index".into(),
                "--find-links".into(),
                wheelhouse_arg.clone(),
                "--require-hashes".into(),
                "--python".into(),
                python_arg.clone(),
                "--target".into(),
                format!("/workspace/store/{name}/{}", package.version),
                "--requirement".into(),
                format!("/workspace/input/{name}.txt"),
            ],
            &format!("unpacking `{}=={}`", package.name, package.version),
        )
        .await?;
    }
    // A view is created here and published at its final path, so the interpreter
    // it names is the deployment's own — one Python, many views.
    run_step(
        config,
        &build,
        &workspace,
        &[
            uv_arg,
            "venv".into(),
            "--python".into(),
            python_arg,
            "/workspace/view".into(),
        ],
        "creating the Plugin dependency view",
    )
    .await?;

    assert_interpreter(&workspace.join("view"), &interpreter)?;
    pointer_into_store(&workspace, &store, &resolved.packages)?;
    publish_store(&workspace, &store, &library_root, &resolved.packages)?;
    publish_view(&workspace, &view)?;

    // The view is only proven where it now lives: a Python environment can carry
    // absolute paths that break the moment it moves.
    if let Err(error) = verify_view(provisioner, &library_root, &view, resolved).await {
        // Nothing else references the view yet — the Plugin is published after
        // this returns — so an unusable one is removed rather than left to look
        // installed.
        let _ = std::fs::remove_dir_all(&view);
        return Err(error);
    }
    Ok(LandedView {
        view,
        packages: resolved.packages.len(),
        reused: false,
    })
}

/// Reuse the view this declaration already has, or say why it cannot be reused.
///
/// A view is content-addressed by the *declaration*, and a declaration need not
/// pin versions (`idna>=3`). What a human approves, and what the form and the
/// answer report, is the *resolved set* — so an existing view is only reused when
/// it really holds that set. It is never rewritten in place: that could change an
/// environment a Run is already using, and switching views is a later slice.
async fn reuse_existing(
    provisioner: &EnvironmentProvisioner,
    library_root: &Path,
    plugin: &str,
    view: &Path,
    store: &Path,
    resolved: &ResolvedRequirements,
) -> Result<Option<LandedView>, ProvisionError> {
    if !view.exists() {
        return Ok(None);
    }
    if !complete_view(view, store) {
        // The view is published by a single rename, so it is either the whole
        // environment or a leftover somebody has to look at.
        return Err(ProvisionError::Failed(format!(
            "Plugin `{plugin}` view {} exists but is not a usable environment; remove it deliberately and install again",
            view.display()
        )));
    }
    verify_view(provisioner, library_root, view, resolved)
        .await
        .map_err(|error| {
            ProvisionError::Failed(format!(
                "Plugin `{plugin}` already has the view {} for this declaration, but it does not match the set this declaration resolves to: {error}. The platform does not switch an existing view in place — pin the versions in the declaration, or remove {} deliberately and install again",
                view.display(),
                view.display()
            ))
        })?;
    Ok(Some(LandedView {
        view: view.to_path_buf(),
        packages: resolved.packages.len(),
        reused: true,
    }))
}

/// True when the view is complete: a real environment whose pointers all resolve.
fn complete_view(view: &Path, store: &Path) -> bool {
    view.join(VIEW_CONFIGURATION).is_file()
        && view.join("bin/python").exists()
        && store.exists()
        && referenced_store_directories(view, store).is_ok()
}

/// A view must run on the deployment's one interpreter, not on a copy of it.
///
/// This is invariant 3 of the standard: many views, one Python. A `uv venv` that
/// somehow produced its own interpreter would look like a working view while
/// quietly doubling every deployment's Python.
fn assert_interpreter(view: &Path, interpreter: &Path) -> Result<(), ProvisionError> {
    let python = view.join("bin/python");
    let resolved = python.canonicalize().map_err(|error| {
        ProvisionError::Failed(format!(
            "the created view has no usable interpreter at {}: {error}",
            python.display()
        ))
    })?;
    if resolved != interpreter {
        return Err(ProvisionError::Failed(format!(
            "the created view runs on {} instead of the deployment interpreter {}",
            resolved.display(),
            interpreter.display()
        )));
    }
    Ok(())
}

/// Write one package's pinned requirement, in the shape `uv` reads hashes from.
///
/// `uv` (like pip) only accepts `--hash` options on a continuation line, so the
/// requirement line ends with a backslash exactly as `uv pip compile` writes it.
fn write_requirement(input: &Path, package: &LockedPackage) -> Result<(), ProvisionError> {
    let name = normalize_name(&package.name);
    let path = input.join(format!("{name}.txt"));
    let mut hashes = package.hashes.clone();
    hashes.sort();
    let mut text = format!("{}=={}", package.name, package.version);
    for hash in &hashes {
        text.push_str(&format!(" \\\n    --hash=sha256:{hash}"));
    }
    text.push('\n');
    std::fs::write(&path, text).map_err(|error| {
        ProvisionError::Failed(format!("{} could not be written: {error}", path.display()))
    })
}

/// Point every installed entry at its final `(package, version)` directory.
///
/// The target is the final store path, not the staging one: the view is renamed
/// into place afterwards, so the pointers must already be right when it lands.
fn pointer_into_store(
    workspace: &Path,
    store: &Path,
    packages: &[LockedPackage],
) -> Result<(), ProvisionError> {
    let staged_view = workspace.join("view");
    let site_packages = site_packages_directories(&staged_view)
        .map_err(ProvisionError::Failed)?
        .into_iter()
        .next()
        .ok_or_else(|| {
            ProvisionError::Failed("the created view has no site-packages directory".to_owned())
        })?;
    for package in packages {
        let name = normalize_name(&package.name);
        let staged = workspace.join("store").join(&name).join(&package.version);
        // `uv pip install --target` leaves its own lock file behind; it is not
        // part of the distribution.
        let _ = std::fs::remove_file(staged.join(".lock"));
        let entries = std::fs::read_dir(&staged).map_err(|error| {
            ProvisionError::Failed(format!("{} is not readable: {error}", staged.display()))
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                ProvisionError::Failed(format!("{} is not readable: {error}", staged.display()))
            })?;
            let name_of_entry = entry.file_name();
            let link = site_packages.join(&name_of_entry);
            if link.exists() {
                continue;
            }
            let target = store
                .join(&name)
                .join(&package.version)
                .join(&name_of_entry);
            std::os::unix::fs::symlink(&target, &link).map_err(|error| {
                ProvisionError::Failed(format!(
                    "{} could not be pointed at the shared store: {error}",
                    link.display()
                ))
            })?;
        }
    }
    Ok(())
}

/// Move each staged `(package, version)` into the shared store exactly once.
fn publish_store(
    workspace: &Path,
    store: &Path,
    library_root: &Path,
    packages: &[LockedPackage],
) -> Result<(), ProvisionError> {
    for package in packages {
        let name = normalize_name(&package.name);
        let staged = workspace.join("store").join(&name).join(&package.version);
        let destination = store.join(&name).join(&package.version);
        if destination.exists() {
            // Already stored. The layout says one (package, version) is one byte
            // set, so a different set of bytes under the same name and version is
            // refused rather than silently kept.
            if !same_tree(&staged, &destination)? {
                return Err(ProvisionError::Failed(format!(
                    "the shared store already holds `{}=={}` with different bytes than this source delivered; the store is keyed by (package, version), so it cannot hold both",
                    package.name, package.version
                )));
            }
            continue;
        }
        let parent = destination
            .parent()
            .expect("a package directory has a parent");
        std::fs::create_dir_all(parent).map_err(|error| {
            ProvisionError::Failed(format!("{} is unusable: {error}", parent.display()))
        })?;
        std::fs::rename(&staged, &destination).map_err(|error| {
            ProvisionError::Failed(format!(
                "`{}=={}` could not be moved into the shared store: {error} (the build root {} and the library root {} must be on one filesystem)",
                package.name,
                package.version,
                workspace.display(),
                library_root.display()
            ))
        })?;
    }
    Ok(())
}

/// Metadata a package installer writes about the installation rather than from
/// the distribution: `uv` records a timestamp in `uv_cache.json`, and `RECORD`
/// lists the files it just wrote. Comparing them would refuse a legitimate
/// rebuild of the same wheel over a difference that carries no package bytes.
const INSTALLER_METADATA: &[&str] = &[
    ".lock",
    "RECORD",
    "INSTALLER",
    "REQUESTED",
    "uv_cache.json",
    "direct_url.json",
];

fn is_installer_metadata(relative: &Path) -> bool {
    let Some(name) = relative.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if !INSTALLER_METADATA.contains(&name) {
        return false;
    }
    name == ".lock"
        || relative
            .parent()
            .and_then(Path::extension)
            .is_some_and(|extension| extension == "dist-info")
}

/// Whether two stored trees hold the same distribution bytes.
///
/// The installer's own metadata is ignored on both sides (see
/// [`INSTALLER_METADATA`]); everything else must match file for file.
fn same_tree(left: &Path, right: &Path) -> Result<bool, ProvisionError> {
    fn entries(
        root: &Path,
    ) -> Result<std::collections::BTreeMap<PathBuf, PathBuf>, ProvisionError> {
        let mut found = std::collections::BTreeMap::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            let listing = std::fs::read_dir(&directory).map_err(|error| {
                ProvisionError::Failed(format!("{} is not readable: {error}", directory.display()))
            })?;
            for entry in listing {
                let entry = entry.map_err(|error| {
                    ProvisionError::Failed(format!(
                        "{} is not readable: {error}",
                        directory.display()
                    ))
                })?;
                let path = entry.path();
                let relative = path
                    .strip_prefix(root)
                    .expect("a walked path is under its root")
                    .to_path_buf();
                if is_installer_metadata(&relative) {
                    continue;
                }
                let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
                    ProvisionError::Failed(format!("{} is not readable: {error}", path.display()))
                })?;
                if metadata.is_dir() {
                    pending.push(path);
                    found.insert(relative, PathBuf::new());
                } else {
                    found.insert(relative, path);
                }
            }
        }
        Ok(found)
    }
    let left = entries(left)?;
    let right = entries(right)?;
    if left.keys().ne(right.keys()) {
        return Ok(false);
    }
    for (relative, left_path) in &left {
        let right_path = right.get(relative).expect("the key sets are equal");
        if left_path.as_os_str().is_empty() || right_path.as_os_str().is_empty() {
            continue;
        }
        let left_bytes = std::fs::read(left_path).map_err(|error| {
            ProvisionError::Failed(format!("{} is not readable: {error}", left_path.display()))
        })?;
        let right_bytes = std::fs::read(right_path).map_err(|error| {
            ProvisionError::Failed(format!("{} is not readable: {error}", right_path.display()))
        })?;
        if left_bytes != right_bytes {
            return Ok(false);
        }
    }
    Ok(true)
}

fn publish_view(workspace: &Path, view: &Path) -> Result<(), ProvisionError> {
    let parent = view.parent().expect("a view directory has a parent");
    std::fs::create_dir_all(parent).map_err(|error| {
        ProvisionError::Failed(format!("{} is unusable: {error}", parent.display()))
    })?;
    std::fs::rename(workspace.join("view"), view).map_err(|error| {
        ProvisionError::Failed(format!(
            "the view could not be published at {}: {error}",
            view.display()
        ))
    })
}

/// Run the published view at its final path and report what it actually holds.
async fn verify_view(
    provisioner: &EnvironmentProvisioner,
    library_root: &Path,
    view: &Path,
    resolved: &ResolvedRequirements,
) -> Result<(), ProvisionError> {
    let config = &provisioner.config;
    let store = store_root(library_root);
    let mut grants = vec![ReadOnlyInput::new(view, view)];
    for directory in referenced_store_directories(view, &store).map_err(ProvisionError::Failed)? {
        grants.push(ReadOnlyInput::new(&directory, &directory));
    }
    let bin = view.join("bin");
    let build = authority(&provisioner.sandbox, grants, std::slice::from_ref(&bin))?;
    let workspace = config.build_root.join("plugin-dependencies/verify");
    std::fs::create_dir_all(&workspace).map_err(|error| {
        ProvisionError::Failed(format!(
            "the dependency verification workspace is unavailable: {error}"
        ))
    })?;
    let probe = "import json, sys\n\
         report = {'prefix': sys.prefix, 'exe': sys.executable, 'distributions': []}\n\
         try:\n\
         \x20   import importlib.metadata as metadata\n\
         \x20   report['distributions'] = sorted(\n\
         \x20       [d.metadata['Name'], d.version] for d in metadata.distributions())\n\
         except Exception as error:\n\
         \x20   report['error'] = repr(error)\n\
         print(json.dumps(report))";

    let output = run_step(
        config,
        &build,
        &workspace,
        &[
            view.join("bin/python").to_string_lossy().into_owned(),
            "-c".into(),
            probe.to_owned(),
        ],
        "running the Plugin dependency view",
    )
    .await?;
    let report: serde_json::Value = serde_json::from_str(output.trim()).map_err(|error| {
        ProvisionError::Failed(format!("the view reported no usable result: {error}"))
    })?;
    let prefix = report
        .get("prefix")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if prefix != view.to_string_lossy() {
        return Err(ProvisionError::Failed(format!(
            "the view runs with prefix `{prefix}` instead of `{}`, so it is not usable where it was published",
            view.display()
        )));
    }
    if let Some(error) = report.get("error").and_then(serde_json::Value::as_str) {
        return Err(ProvisionError::Failed(format!(
            "the view cannot read its own installed set ({error})"
        )));
    }
    // A view carries `lib64 -> lib`, so the same `site-packages` is reachable
    // under two path strings and importlib.metadata reports each distribution
    // once per alias. The installed *set* is what the lock pins.
    let observed: std::collections::BTreeSet<(String, String)> = report
        .get("distributions")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let pair = entry.as_array()?;
            Some((
                pair.first()?.as_str()?.to_owned(),
                pair.get(1)?.as_str()?.to_owned(),
            ))
        })
        .map(|(name, version)| (normalize_name(&name), version))
        .collect();
    let planned: std::collections::BTreeSet<(String, String)> = resolved
        .packages
        .iter()
        .map(|package| (normalize_name(&package.name), package.version.clone()))
        .collect();
    if observed != planned {
        return Err(ProvisionError::Failed(format!(
            "the view holds {observed:?}, not the approved {planned:?}"
        )));
    }
    Ok(())
}

/// Take the one write lock that store and view publication share.
fn write_lock(root: &Path) -> Result<std::fs::File, ProvisionError> {
    std::fs::create_dir_all(root).map_err(|error| {
        ProvisionError::Failed(format!("{} is unusable: {error}", root.display()))
    })?;
    let path = root.join(".lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| {
            ProvisionError::Failed(format!("{} is unusable: {error}", path.display()))
        })?;
    rustix::fs::flock(&file, rustix::fs::FlockOperation::LockExclusive).map_err(|error| {
        ProvisionError::Failed(format!("the dependency store lock failed: {error}"))
    })?;
    Ok(file)
}

fn cleanup(path: &Path) -> Result<(), ProvisionError> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => {
            return Err(ProvisionError::Failed(format!(
                "the staging directory {} could not be cleared: {error}",
                path.display()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::RunApplication;
    use crate::environment_provision::test_support::{
        builder_required, config, fixture_wheel, toolchain,
    };
    use crate::tool_host::{McpToolConfig, SandboxLaunch};
    use anchor_graph_host::{FilePluginCatalog, PluginCatalog, PluginDependencies};
    use anchor_runtime::ToolPort;
    use anchor_sandbox_bwrap::{BubblewrapPolicy, BubblewrapSandbox};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    /// The built-in Plugin this slice ships, in the repository next to the others.
    fn built_in_probe() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/python-probe")
    }

    /// The built-in probe Plugin, with one test-only addition: the address this
    /// case listens on, so the server can report whether it can reach the network.
    /// Its manifest declaration and its server are the committed ones.
    fn write_candidate(directory: &Path, probe_address: &str) {
        let source = built_in_probe();
        std::fs::create_dir_all(directory).unwrap();
        std::fs::copy(source.join("server.py"), directory.join("server.py")).unwrap();
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(source.join("plugin.json")).unwrap()).unwrap();
        manifest["mcpServers"]["probe"]["env"] =
            serde_json::json!({"ANCHOR_PROBE_ADDRESS": probe_address});
        std::fs::write(directory.join("plugin.json"), manifest.to_string()).unwrap();
    }

    fn sandbox(workspace: &Path) -> Option<BubblewrapSandbox> {
        // The deployment's allowlist deliberately does not name `python`: the
        // view's interpreter must be authorized by the invocation's executable
        // grant, so this case fails if that grant ever stops being applied.
        let policy = BubblewrapPolicy::new("bwrap", ["sh"]).authorize_workspace_root(workspace);
        match BubblewrapSandbox::new(policy) {
            Ok(sandbox) => Some(sandbox),
            Err(error) => {
                let reason = format!("Bubblewrap is unavailable: {error}");
                if builder_required() {
                    panic!("ANCHOR_REQUIRE_ENV_BUILDER=1 but {reason}");
                }
                eprintln!("SKIPPED (no verification): {reason}");
                None
            }
        }
    }

    /// The whole slice in one case: the built-in Python Plugin declares a package,
    /// the host solves and lands it through uv, a view is generated, and the
    /// Plugin's tool imports the package inside a no-network sandbox — from the
    /// view, not from the deployment's interpreter.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_plugin_tool_imports_its_declared_package_from_its_view_without_network() {
        let Some(toolchain) = toolchain() else {
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let library = root.path().join("library");
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let Some(sandbox) = sandbox(&workspace) else {
            return;
        };

        // The deployment authorizes one directory of already-built distributions.
        // The wheel is crafted here so the case needs no network at all; the
        // declaration's own `idna==3.6` decides what is installed.
        let wheels = root.path().join("wheels");
        std::fs::create_dir_all(&wheels).unwrap();
        fixture_wheel(&wheels, "idna", "3.6");
        let build_root = root.path().join("environment-build");
        let provisioner = Arc::new(
            crate::environment_provision::EnvironmentProvisioner::new(config(
                &build_root,
                &toolchain,
                wheels,
            ))
            .unwrap(),
        );

        // The deployment interpreter is the control, but whether this particular
        // host happens to have the package is not the proof: the proof is where
        // the tool's import came from.
        let control = std::process::Command::new(&toolchain.python)
            .arg("-c")
            .arg("import idna")
            .output()
            .unwrap();
        if control.status.success() {
            eprintln!("NOTE: this host already imports idna; the view path below is the evidence");
        }

        // The host listens here; only a process with the host's network can reach
        // it, so this is what makes "the sandbox has no network" testable.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let probe_address = listener.local_addr().unwrap().to_string();
        let candidate = root.path().join("probe-candidate");
        write_candidate(&candidate, &probe_address);
        let application = RunApplication::new(root.path().join("state"), root.path().to_path_buf())
            .with_environment_provisioner(Arc::clone(&provisioner));

        // 1. The declaration is solved before a human is asked anything.
        let precondition = application
            .plugin_install_precondition(library.clone(), candidate, "python-probe".into())
            .await
            .unwrap();
        let plan = precondition
            .dependencies
            .clone()
            .expect("the declaration resolved to a plan");
        assert_eq!(
            plan.packages
                .iter()
                .map(|package| (package.name.as_str(), package.version.as_str()))
                .collect::<Vec<_>>(),
            vec![("idna", "3.6")]
        );
        assert_eq!(plan.interpreter, resolved_interpreter(&toolchain));
        assert!(plan.view.starts_with(&library), "{}", plan.view.display());
        assert_eq!(plan.sources.len(), 1);

        let outcome = application
            .install_plugin_if_unchanged(library.clone(), precondition, false)
            .await
            .unwrap();
        assert_eq!(outcome.id, "python-probe");

        // 2. The view is a real environment whose pointer names the shared store.
        let catalog = FilePluginCatalog::new(&library);
        let view = catalog.view_path("python-probe").unwrap().expect("a view");
        assert!(view.join("pyvenv.cfg").is_file(), "{}", view.display());
        assert!(
            view.join("bin/python").is_symlink(),
            "the view must point at the deployment's one interpreter"
        );
        let store = store_root(&library.canonicalize().unwrap());
        let site_packages = site_packages_directories(&view).unwrap().remove(0);
        let pointer = site_packages.join("idna");
        assert!(pointer.is_symlink(), "{}", pointer.display());
        let resolved = pointer.canonicalize().unwrap();
        assert!(
            resolved.starts_with(store.canonicalize().unwrap()),
            "{} is not in the shared store",
            resolved.display()
        );
        // The store holds exactly one copy of the (package, version).
        assert_eq!(std::fs::read_dir(&store).unwrap().count(), 1);
        let reused = land(
            &provisioner,
            &library,
            "python-probe",
            &plan.declaration_digest,
            &ResolvedRequirements {
                interpreter: plan.interpreter.clone(),
                interpreter_path: plan.interpreter_path.clone(),
                interpreter_prefix: PathBuf::from("/usr"),
                interpreter_digest: plan.interpreter_digest.clone(),
                lock: String::new(),
                lock_digest: plan.lock_digest.clone(),
                packages: plan.packages.clone(),
            },
        )
        .await
        .unwrap();
        assert!(reused.reused, "a landed declaration is not rebuilt");
        assert_eq!(reused.view, view);

        // 3. The Plugin's tool runs on the view's interpreter, in a sandbox with
        //    no network, and reports what that interpreter imported.
        let binding = catalog.resolve(&["python-probe".into()]).unwrap().remove(0);
        let prepared = crate::plugin_view::prepare(&library, &binding)
            .unwrap()
            .expect("the installed Plugin has a view");
        assert_eq!(prepared.view(), view);
        let mcp = McpToolConfig::from_catalog(&library, std::slice::from_ref(&binding)).unwrap();
        let server_environments = BTreeMap::new();
        let plugin_views = BTreeMap::from([("python-probe".to_owned(), prepared)]);
        let tools = mcp
            .bind_with_node_inputs(
                std::slice::from_ref(&binding),
                false,
                SandboxLaunch {
                    sandbox: &sandbox,
                    workspace: &workspace,
                    readonly_inputs: &[],
                    server_environments: &server_environments,
                    plugin_views: &plugin_views,
                },
            )
            .await
            .unwrap_or_else(|error| panic!("the python Plugin server did not bind: {error}"));
        let name = "python-probe-probe_report";
        assert!(
            tools
                .definitions()
                .iter()
                .any(|definition| definition.name == name),
            "{:?}",
            tools
                .definitions()
                .iter()
                .map(|definition| definition.name.to_string())
                .collect::<Vec<_>>()
        );
        let result = tools.call(name, serde_json::json!({})).await.unwrap();
        let report: serde_json::Value = serde_json::from_str(result[0].as_text().unwrap()).unwrap();
        assert_eq!(report["version"], "3.6", "{report}");
        assert_eq!(
            report["prefix"],
            view.to_string_lossy().as_ref(),
            "{report}"
        );
        assert_eq!(
            report["exe"],
            view.join("bin/python").to_string_lossy().as_ref(),
            "{report}"
        );
        assert!(
            report["file"]
                .as_str()
                .unwrap()
                .starts_with(view.to_string_lossy().as_ref()),
            "the imported package did not come from the view: {report}"
        );
        assert!(
            report["network"].as_str().unwrap().starts_with("denied"),
            "the sandbox shares the host network: {report}"
        );
        drop(listener);

        // 4. A declaration that cannot be solved is refused before anything is
        //    written, and the view that already exists is untouched.
        let broken = root.path().join("broken-candidate");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(
            broken.join("plugin.json"),
            serde_json::json!({
                "name": "Broken",
                "dependencies": {"python": ["idna==9.9.9"]},
            })
            .to_string(),
        )
        .unwrap();
        let failure = application
            .plugin_install_precondition(library.clone(), broken, "broken".into())
            .await
            .unwrap_err();
        let crate::application::ApplicationError::Invalid(reason) = failure else {
            panic!("a declaration that cannot be solved is an invalid ask: {failure:?}");
        };
        assert!(
            reason.contains("dependency resolution failed"),
            "the refusal must carry uv's own reason: {reason}"
        );
        assert!(!library.join("plugins/broken").exists());
        assert_eq!(catalog.view_path("python-probe").unwrap(), Some(view));
        assert_eq!(
            catalog.definition("python-probe").unwrap().dependencies,
            PluginDependencies {
                python: vec!["idna==3.6".to_owned()]
            }
        );
    }

    /// The built-in Python Plugin this slice ships stays well formed: it declares
    /// exactly the package its server imports and launches on its own view's
    /// interpreter, so a deployment can install it as it stands.
    #[test]
    fn the_built_in_python_probe_is_a_valid_declaration() {
        let directory = built_in_probe();
        let root = directory.parent().unwrap().parent().unwrap();
        let catalog = FilePluginCatalog::new(root);
        let definition = catalog.definition("python-probe").unwrap();
        assert_eq!(definition.dependencies.python, vec!["idna==3.6"]);
        // The view exists, and the manifest names it as a deployment path: the
        // catalog keeps it as written, because only the host that *grants* the
        // view may substitute it (see the launcher's `named_view`).
        let view = catalog.view_path("python-probe").unwrap().unwrap();
        assert!(!view.ends_with("bin/python"));
        let server = catalog.mcp_servers("python-probe", true).unwrap().remove(0);
        assert_eq!(
            server
                .config
                .get("command")
                .and_then(serde_json::Value::as_str),
            Some("${ANCHOR_PLUGIN_VIEW}/bin/python")
        );
        assert_eq!(
            server.config.get("args"),
            Some(&serde_json::json!(["server.py"]))
        );
        let source = std::fs::read_to_string(directory.join("server.py")).unwrap();
        assert!(
            source.contains("import idna"),
            "the probe must import the package its manifest declares"
        );
    }

    /// One deployment of the built-in probe Plugin against a local wheelhouse.
    struct Deployment {
        root: tempfile::TempDir,
        library: PathBuf,
        workspace: PathBuf,
        sandbox: BubblewrapSandbox,
        provisioner: Arc<crate::environment_provision::EnvironmentProvisioner>,
        application: RunApplication,
        candidate: PathBuf,
        /// Kept alive so the address the server is told to reach answers.
        listener: std::net::TcpListener,
    }

    async fn deploy() -> Option<Deployment> {
        let toolchain = toolchain()?;
        let root = tempfile::tempdir().unwrap();
        let library = root.path().join("library");
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let sandbox = sandbox(&workspace)?;
        let wheels = root.path().join("wheels");
        std::fs::create_dir_all(&wheels).unwrap();
        fixture_wheel(&wheels, "idna", "3.6");
        let provisioner = Arc::new(
            crate::environment_provision::EnvironmentProvisioner::new(config(
                &root.path().join("environment-build"),
                &toolchain,
                wheels,
            ))
            .unwrap(),
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let candidate = root.path().join("probe-candidate");
        write_candidate(&candidate, &listener.local_addr().unwrap().to_string());
        let application = RunApplication::new(root.path().join("state"), root.path().to_path_buf())
            .with_environment_provisioner(Arc::clone(&provisioner));
        Some(Deployment {
            root,
            library,
            workspace,
            sandbox,
            provisioner,
            application,
            candidate,
            listener,
        })
    }

    impl Deployment {
        /// The view the declaration maps to, whether or not it exists.
        fn view(&self) -> PathBuf {
            FilePluginCatalog::new(&self.library)
                .view_path("python-probe")
                .unwrap()
                .expect("the Plugin declares dependencies")
        }

        async fn precondition(
            &self,
        ) -> Result<
            crate::application::plugins::PluginInstallPrecondition,
            crate::application::ApplicationError,
        > {
            self.application
                .plugin_install_precondition(
                    self.library.clone(),
                    self.candidate.clone(),
                    "python-probe".into(),
                )
                .await
        }

        async fn install(
            &self,
        ) -> Result<anchor_library::InstallOutcome, crate::application::ApplicationError> {
            let precondition = self.precondition().await?;
            self.application
                .install_plugin_if_unchanged(self.library.clone(), precondition, false)
                .await
        }
    }

    /// The same candidate with a different declaration, so a case can use a
    /// requirement that does not pin a version.
    fn write_candidate_with(directory: &Path, probe_address: &str, declaration: &[&str]) {
        write_candidate(directory, probe_address);
        let path = directory.join("plugin.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        manifest["dependencies"]["python"] = serde_json::json!(declaration);
        std::fs::write(&path, manifest.to_string()).unwrap();
    }

    fn resolved_of(
        plan: &crate::application::plugins::PluginDependencyPlan,
    ) -> ResolvedRequirements {
        ResolvedRequirements {
            interpreter: plan.interpreter.clone(),
            interpreter_path: plan.interpreter_path.clone(),
            interpreter_prefix: PathBuf::from("/usr"),
            interpreter_digest: plan.interpreter_digest.clone(),
            lock: String::new(),
            lock_digest: plan.lock_digest.clone(),
            packages: plan.packages.clone(),
        }
    }

    /// A wheel for the same (package, version) with different bytes.
    fn write_other_wheel(directory: &Path, name: &str, version: &str) -> PathBuf {
        use crate::environment_provision::test_support::write_zip;
        use base64::Engine;
        use sha2::Digest;
        let dist = format!("{name}-{version}");
        let mut files = vec![
            (
                format!("{name}/__init__.py"),
                format!("__version__ = {version:?}\nOTHER = True\n").into_bytes(),
            ),
            (
                format!("{dist}.dist-info/METADATA"),
                format!("Metadata-Version: 2.1\nName: {name}\nVersion: {version}\n").into_bytes(),
            ),
            (
                format!("{dist}.dist-info/WHEEL"),
                b"Wheel-Version: 1.0\nGenerator: other\nRoot-Is-Purelib: true\nTag: py3-none-any\n"
                    .to_vec(),
            ),
        ];
        // A wheel is a valid wheel: without a RECORD the installer refuses it, and
        // then the case would prove nothing about the store.
        let mut record = String::new();
        for (path, data) in &files {
            let digest =
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(data));
            record.push_str(&format!("{path},sha256={digest},{}\n", data.len()));
        }
        record.push_str(&format!("{dist}.dist-info/RECORD,,\n"));
        files.push((format!("{dist}.dist-info/RECORD"), record.into_bytes()));
        let path = directory.join(format!("{dist}-py3-none-any.whl"));
        std::fs::write(&path, write_zip(&files)).unwrap();
        path
    }

    /// A view that is gone must be repairable through the one installation gate:
    /// the same Plugin digest is not "nothing to do" while its view is missing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_deleted_view_is_rebuilt_by_installing_the_same_plugin_again() {
        let Some(deployment) = deploy().await else {
            return;
        };
        deployment.install().await.unwrap();
        let view = deployment.view();
        assert!(view.join("pyvenv.cfg").is_file(), "{}", view.display());
        std::fs::remove_dir_all(&view).unwrap();
        assert!(!view.exists());

        // The one gate must see work to do rather than reporting "already current".
        assert!(
            crate::plugin_view::view_needs_repair(&deployment.library, "python-probe"),
            "a missing view is exactly what a repair has to fix"
        );
        let precondition = deployment.precondition().await.unwrap();
        assert!(
            precondition.already_installed(),
            "the Plugin bytes themselves are unchanged"
        );
        let outcome = deployment
            .application
            .install_plugin_if_unchanged(deployment.library.clone(), precondition, false)
            .await
            .expect("a missing view is rebuilt through the one gate");
        assert_eq!(outcome.id, "python-probe");
        assert!(view.join("pyvenv.cfg").is_file(), "the view is back");
        assert!(
            crate::plugin_view::prepare_for_plugin(&deployment.library, "python-probe")
                .unwrap()
                .is_some(),
            "the rebuilt view is usable"
        );
        // The repair published no second copy of the Plugin.
        assert!(deployment.library.join("plugins/python-probe").is_dir());
    }

    /// A view is keyed by the declaration, and a declaration need not pin
    /// versions. What a human approves is the *resolved set*, so a view holding a
    /// different set must never be reported as this declaration's answer.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unpinned_declaration_never_disagrees_with_the_view_it_reuses() {
        let Some(deployment) = deploy().await else {
            return;
        };
        let declaration = ["idna>=3"];
        write_candidate_with(
            &deployment.candidate,
            &deployment.listener.local_addr().unwrap().to_string(),
            &declaration,
        );
        // First resolution: the authorized directory holds 3.6.
        deployment.install().await.unwrap();
        let view = deployment.view();
        let first = deployment.precondition().await.unwrap();
        let first_plan = first.dependencies.clone().unwrap();
        assert_eq!(first_plan.packages[0].version, "3.6");
        verify_view(
            &deployment.provisioner,
            &deployment.library,
            &view,
            &resolved_of(&first_plan),
        )
        .await
        .expect("the landed view holds the first resolution");

        // The authorized directory now only holds 3.7, so the *same* declaration
        // resolves differently while the view stays where it is.
        let wheels = deployment.root.path().join("wheels");
        std::fs::remove_file(wheels.join("idna-3.6-py3-none-any.whl")).unwrap();
        fixture_wheel(&wheels, "idna", "3.7");
        let precondition = deployment.precondition().await.unwrap();
        let plan = precondition.dependencies.clone().unwrap();
        assert_eq!(
            plan.packages[0].version, "3.7",
            "resolution moved on while the view did not"
        );

        match deployment
            .application
            .install_plugin_if_unchanged(deployment.library.clone(), precondition, false)
            .await
        {
            Ok(_) => {
                // If reuse was allowed, the view must really hold the new set.
                verify_view(
                    &deployment.provisioner,
                    &deployment.library,
                    &view,
                    &resolved_of(&plan),
                )
                .await
                .expect("a reused view must hold the set this declaration resolved to");
            }
            Err(crate::application::ApplicationError::Invalid(reason)) => {
                assert!(
                    reason.contains("3.7") && reason.contains("3.6"),
                    "a refusal must name both sets: {reason}"
                );
            }
            Err(other) => panic!("unexpected failure: {other:?}"),
        }
    }

    /// The shared store is keyed by (package, version), so two different byte sets
    /// under one name and version are refused rather than silently merged.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_second_byte_set_for_one_package_version_is_refused() {
        let Some(deployment) = deploy().await else {
            return;
        };
        deployment.install().await.unwrap();
        let view = deployment.view();
        assert!(view.join("pyvenv.cfg").is_file());
        // Rebuild from a different wheel that claims the same name and version.
        std::fs::remove_dir_all(&view).unwrap();
        let wheels = deployment.root.path().join("wheels");
        std::fs::remove_file(wheels.join("idna-3.6-py3-none-any.whl")).unwrap();
        write_other_wheel(&wheels, "idna", "3.6");

        let failure = deployment.install().await.unwrap_err();
        let crate::application::ApplicationError::Invalid(reason) = failure else {
            panic!("a byte conflict is an invalid ask: {failure:?}");
        };
        assert!(reason.contains("different bytes"), "{reason}");
        assert!(
            !view.exists(),
            "a view built from bytes the store refused is not published"
        );
    }

    /// A view must run on the deployment's one interpreter, not on a copy.
    #[test]
    fn a_view_that_does_not_point_at_the_one_interpreter_is_refused() {
        let Some(toolchain) = toolchain() else {
            return;
        };
        let interpreter = std::fs::canonicalize(&toolchain.python).unwrap();
        let root = tempfile::tempdir().unwrap();
        let view = root.path().join("view");
        std::fs::create_dir_all(view.join("bin")).unwrap();
        std::os::unix::fs::symlink("/bin/sh", view.join("bin/python")).unwrap();
        let error = assert_interpreter(&view, &interpreter).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("instead of the deployment interpreter"),
            "{error}"
        );
        std::fs::remove_file(view.join("bin/python")).unwrap();
        std::os::unix::fs::symlink(&interpreter, view.join("bin/python")).unwrap();
        assert!(assert_interpreter(&view, &interpreter).is_ok());
    }

    /// The real deployment layout: the Plugin is installed in the Library, while
    /// the Graph catalog's bundle carries its own copy of the same tree. The
    /// command the launcher builds must name the *granted* view, so a Plugin
    /// resolved through the catalog root still runs on its Library view.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_plugin_resolved_from_a_bundle_root_still_runs_on_its_library_view() {
        let Some(deployment) = deploy().await else {
            return;
        };
        deployment.install().await.unwrap();
        let library_view = deployment.view();
        assert!(library_view.join("pyvenv.cfg").is_file());

        // Three separate roots, as `deploy/systemd/anchor.env.example` describes:
        // the Library holds what was installed, the catalog holds the bundle, and
        // the bundle carries its own copy of the Plugin resources.
        let catalog = deployment.root.path().join("catalog");
        let bundle = catalog.join("demo-bundle");
        let bundled_plugin = bundle.join("plugins/python-probe");
        std::fs::create_dir_all(&bundled_plugin).unwrap();
        for name in ["plugin.json", "server.py"] {
            std::fs::copy(
                deployment.library.join("plugins/python-probe").join(name),
                bundled_plugin.join(name),
            )
            .unwrap();
        }
        assert!(!library_view.starts_with(&catalog));

        // The binding is resolved through the *catalog*, exactly as the node host
        // does when a bundle serves the Plugin.
        let catalog_source = FilePluginCatalog::new(&bundle);
        assert_eq!(
            catalog_source.definition("python-probe").unwrap().digest,
            FilePluginCatalog::new(&deployment.library)
                .definition("python-probe")
                .unwrap()
                .digest,
            "the bundle copy is the same Plugin the Run froze"
        );
        let binding = catalog_source
            .resolve(&["python-probe".into()])
            .unwrap()
            .remove(0);
        // The view is prepared from the *Library*, which is what the host grants.
        let prepared = crate::plugin_view::prepare(&deployment.library, &binding)
            .unwrap()
            .expect("the installed Plugin has a view");
        assert_eq!(prepared.view(), library_view);

        let mcp = McpToolConfig::from_catalog(&bundle, std::slice::from_ref(&binding)).unwrap();
        let server_environments = BTreeMap::new();
        let plugin_views = BTreeMap::from([("python-probe".to_owned(), prepared)]);
        let tools = mcp
            .bind_with_node_inputs(
                std::slice::from_ref(&binding),
                false,
                SandboxLaunch {
                    sandbox: &deployment.sandbox,
                    workspace: &deployment.workspace,
                    readonly_inputs: &[],
                    server_environments: &server_environments,
                    plugin_views: &plugin_views,
                },
            )
            .await
            .unwrap_or_else(|error| {
                panic!("a Plugin resolved from a bundle must still launch: {error}")
            });
        let result = tools
            .call("python-probe-probe_report", serde_json::json!({}))
            .await
            .unwrap();
        let report: serde_json::Value = serde_json::from_str(result[0].as_text().unwrap()).unwrap();
        assert_eq!(report["version"], "3.6", "{report}");
        assert!(
            report["file"]
                .as_str()
                .unwrap()
                .starts_with(library_view.to_string_lossy().as_ref()),
            "the interpreter must be the one the host granted: {report}"
        );
    }

    /// The interpreter version the fixture wheel was built for, as the plan
    /// reports it.
    fn resolved_interpreter(
        toolchain: &crate::environment_provision::test_support::Toolchain,
    ) -> String {
        let output = std::process::Command::new(&toolchain.python)
            .arg("-c")
            .arg("import sys; print('%d.%d.%d' % sys.version_info[:3])")
            .output()
            .unwrap();
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }
}

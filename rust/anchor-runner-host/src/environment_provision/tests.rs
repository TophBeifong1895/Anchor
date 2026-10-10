//! The builder's regression surface.

use std::path::Path;

use anchor_runtime::{NetworkPolicy, ReadOnlyInput, SandboxPort, SandboxRequest, SandboxStatus};

use super::test_support::*;
use super::*;

/// The whole builder: resolve against a local wheel, fetch it, build it inside a
/// network-disabled sandbox, and run the result where it was published.
#[tokio::test(flavor = "multi_thread")]
async fn a_locked_set_is_resolved_fetched_built_and_verified_after_it_moves() {
    let Some(toolchain) = toolchain() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let wheels = root.path().join("wheels");
    std::fs::create_dir_all(&wheels).unwrap();
    fixture_wheel(&wheels, "probepkg", "0.1.0");
    let config = config(root.path(), &toolchain, wheels);

    let resolved = resolve_requirements(&config, None, &["probepkg==0.1.0".to_owned()])
        .await
        .expect("the ask resolves against the authorized directory");
    assert_eq!(resolved.packages.len(), 1, "{resolved:?}");
    assert_eq!(resolved.packages[0].name, "probepkg");
    assert_eq!(resolved.packages[0].version, "0.1.0");
    assert_eq!(resolved.packages[0].hashes.len(), 1);
    assert!(resolved.lock.contains("--hash=sha256:"));
    assert_eq!(resolved.lock_digest.len(), 64);

    let wheelhouse = root.path().join("wheelhouse");
    let fetched = fetch_wheelhouse(&config, &resolved.packages, &wheelhouse).unwrap();
    assert_eq!(fetched.len(), 1, "{fetched:?}");
    assert_eq!(
        fetched[0].sha256, resolved.packages[0].hashes[0],
        "the fetched bytes are the ones the human approved"
    );

    let sandbox = provision_sandbox(&config).expect("bubblewrap is available");
    let built = build_environment(&config, &sandbox, &resolved, &wheelhouse)
        .await
        .expect("the locked set builds inside the sandbox");
    assert!(
        built.tree.join("bin").is_dir(),
        "the tree provides commands"
    );
    assert!(built.files > 0 && built.bytes > 0, "{built:?}");

    // The Library publishes only regular files and directories, so a materialized
    // build is what makes a virtual environment publishable at all.
    let store = anchor_library::EnvironmentStore::new(root.path().join("library"));
    let outcome = store
        .publish_expecting(&built.tree, None)
        .expect("the built tree is publishable as it stands");
    assert!(!outcome.reused);
    let published = store.published_path(&outcome.id);
    assert!(
        store.show(&outcome.id).is_ok(),
        "the published environment verifies on its own identity"
    );
    verify_published(&config, &sandbox, &published, &resolved)
        .await
        .expect("the published tree runs where it was published");

    // The published environment is not just readable: it provides the command it
    // was built for, and the approved package is importable.
    let output = command_output(
        &config,
        &sandbox,
        &published,
        &[
            published.join("bin/python").to_string_lossy().into_owned(),
            "-c".into(),
            "import probepkg; print(probepkg.VALUE)".into(),
        ],
    )
    .await;
    assert_eq!(output.trim(), "0.1.0");

    // The other half of the boundary: this environment runs with no network at
    // all, so a package that tries to fetch something mid-run cannot.
    let probe = command_output(
        &config,
        &sandbox,
        &published,
        &[
            published.join("bin/python").to_string_lossy().into_owned(),
            "-c".into(),
            "import socket\n\
             try:\n\
             \x20   socket.create_connection(('one.one.one.one', 443), timeout=3)\n\
             \x20   print('REACHABLE')\n\
             except Exception as error:\n\
             \x20   print('BLOCKED', type(error).__name__)\n"
                .into(),
        ],
    )
    .await;
    assert!(
        probe.contains("BLOCKED"),
        "a published environment must not reach the network: {probe}"
    );
}

/// The boundary the whole design rests on: this sandbox cannot reach the network
/// even when a request asks for it.
#[tokio::test(flavor = "multi_thread")]
async fn the_build_sandbox_refuses_network_and_has_none() {
    let Some(toolchain) = toolchain() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let wheels = root.path().join("wheels");
    std::fs::create_dir_all(&wheels).unwrap();
    let config = config(root.path(), &toolchain, wheels);
    let Ok(sandbox) = provision_sandbox(&config) else {
        eprintln!("skipping the sandbox boundary test: bubblewrap is unavailable");
        return;
    };
    std::fs::create_dir_all(&config.build_root).unwrap();

    let mut request = SandboxRequest::new(config.build_root.clone(), ["uv", "--version"]);
    request.network = NetworkPolicy::Enabled;
    let error = sandbox.run(request).await.unwrap_err().to_string();
    assert!(
        error.contains("not authorized by the host policy"),
        "a network request must be refused by this adapter: {error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_artifact_that_is_not_the_approved_bytes_is_refused() {
    let Some(toolchain) = toolchain() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let wheels = root.path().join("wheels");
    std::fs::create_dir_all(&wheels).unwrap();
    fixture_wheel(&wheels, "probepkg", "0.1.0");
    let config = config(root.path(), &toolchain, wheels.clone());
    let resolved = resolve_requirements(&config, None, &["probepkg==0.1.0".to_owned()])
        .await
        .unwrap();

    // The same version, different bytes: the honest failure is "not the approved
    // digest", not a build that quietly installs something else.
    fixture_wheel(&wheels, "probepkg", "0.1.0");
    std::fs::write(
        wheels.join("probepkg-0.1.0-py3-none-any.whl"),
        b"not the wheel that was approved",
    )
    .unwrap();
    let error =
        fetch_wheelhouse(&config, &resolved.packages, &root.path().join("wheelhouse")).unwrap_err();
    assert!(matches!(error, ProvisionError::Failed(_)), "{error:?}");
    assert!(error.to_string().contains("approved digest"), "{error}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_requested_interpreter_this_deployment_cannot_build_is_refused_before_approval() {
    let Some(toolchain) = toolchain() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path(), &toolchain, root.path().join("wheels"));
    let error = resolve_requirements(&config, Some("2.6"), &["probepkg==0.1.0".to_owned()])
        .await
        .unwrap_err();
    assert!(matches!(error, ProvisionError::Invalid(_)), "{error:?}");
    assert!(error.to_string().contains("does not satisfy"), "{error}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ask_that_is_not_a_package_requirement_never_reaches_uv() {
    let Some(toolchain) = toolchain() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path(), &toolchain, root.path().join("wheels"));
    for ask in [
        "--index-url https://evil.example",
        "../escape",
        "-r other.txt",
    ] {
        let error = resolve_requirements(&config, None, &[ask.to_owned()])
            .await
            .unwrap_err();
        assert!(
            matches!(error, ProvisionError::Invalid(_)),
            "{ask}: {error:?}"
        );
    }
}

/// Run one command in the build sandbox with `tree` granted read-only and its
/// `bin` authorized, exactly as an execution request would describe it.
async fn command_output(
    config: &EnvironmentProvisionConfig,
    sandbox: &anchor_sandbox_bwrap::BubblewrapSandbox,
    tree: &Path,
    command: &[String],
) -> String {
    let workspace = config.build_root.join("probe-workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let bin = tree.join("bin");
    let authority = super::build::authority(
        sandbox,
        vec![ReadOnlyInput::new(tree, tree)],
        std::slice::from_ref(&bin),
    )
    .unwrap();
    let mut request = authority.request(&workspace, command);
    request.timeout = config.step_timeout();
    request.max_output_bytes = config.max_step_output();
    let result = authority.sandbox.run(request).await.unwrap();
    assert_eq!(result.status, SandboxStatus::Completed, "{}", result.reason);
    assert_eq!(
        result.exit_code,
        Some(0),
        "{}{}",
        result.stdout,
        result.stderr
    );
    result.stdout
}

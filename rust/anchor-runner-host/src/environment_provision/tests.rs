//! The builder's regression surface.

use std::path::Path;
use std::sync::Arc;

use anchor_runtime::{NetworkPolicy, ReadOnlyInput, SandboxPort, SandboxRequest, SandboxStatus};

use super::config::PackageSource;
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
    let fetched = fetch_wheelhouse(&config, &resolved.packages, &wheelhouse)
        .await
        .unwrap();
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
    let sandbox = match provision_sandbox(&config) {
        Ok(sandbox) => sandbox,
        Err(error) if builder_required() => {
            panic!("ANCHOR_REQUIRE_ENV_BUILDER=1 but the sandbox is unavailable: {error}")
        }
        Err(error) => {
            eprintln!("SKIPPED (no verification): bubblewrap is unavailable: {error}");
            return;
        }
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
    let error = fetch_wheelhouse(&config, &resolved.packages, &root.path().join("wheelhouse"))
        .await
        .unwrap_err();
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

/// A PEP 691 index served on loopback.
///
/// The networked half of fetching used to be written but never executed. This is
/// the smallest thing that runs it for real: a real HTTP server, a real JSON
/// document with a real digest, and the host's own hardened client, without
/// depending on the internet.
struct IndexFixture {
    /// The index root, the way a deployment would configure it.
    endpoint: String,
    bytes: Arc<std::sync::Mutex<Vec<u8>>>,
    missing: Arc<std::sync::atomic::AtomicBool>,
    queried: Arc<std::sync::atomic::AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for IndexFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl IndexFixture {
    /// Serve different bytes after resolution, keeping the advertised digest.
    fn replace_bytes(&self, bytes: impl Into<Vec<u8>>) {
        *self.bytes.lock().unwrap() = bytes.into();
    }

    fn queries(&self) -> usize {
        self.queried.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[derive(Clone)]
struct IndexState {
    filename: String,
    endpoint: String,
    digest: String,
    bytes: Arc<std::sync::Mutex<Vec<u8>>>,
    /// Answer a PEP 503 HTML page instead of the JSON document.
    as_html: bool,
    /// Report `404` for the project, as an index without it does.
    missing: Arc<std::sync::atomic::AtomicBool>,
    queried: Arc<std::sync::atomic::AtomicUsize>,
}

async fn simple_page(
    axum::extract::State(state): axum::extract::State<IndexState>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    state
        .queried
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if state.missing.load(std::sync::atomic::Ordering::SeqCst) {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    if state.as_html {
        return (
            axum::http::StatusCode::OK,
            [("content-type", "text/html")],
            format!(
                "<html><body><a href=\"{}/files/{}\">{}</a></body></html>",
                state.endpoint, state.filename, state.filename
            ),
        )
            .into_response();
    }
    let body = serde_json::json!({
        "meta": {"api-version": "1.0"},
        "name": "probepkg",
        "files": [{
            "filename": state.filename,
            "url": format!("{}/files/{}", state.endpoint, state.filename),
            "hashes": {"sha256": state.digest},
        }],
    })
    .to_string();
    (
        axum::http::StatusCode::OK,
        [("content-type", "application/vnd.pypi.simple.v1+json")],
        body,
    )
        .into_response()
}

async fn distribution(
    axum::extract::State(state): axum::extract::State<IndexState>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    state
        .queried
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let bytes = state.bytes.lock().unwrap().clone();
    (
        axum::http::StatusCode::OK,
        [("content-type", "application/octet-stream")],
        bytes,
    )
        .into_response()
}

async fn index_fixture(wheel: &Path, as_html: bool) -> IndexFixture {
    index_fixture_named(wheel, as_html, None).await
}

/// An index that reports it does not have the project at all.
async fn index_fixture_missing(wheel: &Path) -> IndexFixture {
    let fixture = index_fixture_named(wheel, false, None).await;
    fixture
        .missing
        .store(true, std::sync::atomic::Ordering::SeqCst);
    fixture
}

/// The same fixture, optionally advertising a different file name — an sdist, so
/// a case can prove this host never fetches one.
async fn index_fixture_named(wheel: &Path, as_html: bool, rename_to: Option<&str>) -> IndexFixture {
    let filename = rename_to
        .map(str::to_owned)
        .unwrap_or_else(|| wheel.file_name().unwrap().to_string_lossy().into_owned());
    let digest = super::config::sha256_file(wheel).unwrap();
    let bytes = Arc::new(std::sync::Mutex::new(std::fs::read(wheel).unwrap()));
    let queried = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let missing = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let endpoint = format!("http://{address}/simple");
    let state = IndexState {
        filename,
        endpoint: format!("http://{address}"),
        digest,
        bytes: Arc::clone(&bytes),
        as_html,
        missing: Arc::clone(&missing),
        queried: Arc::clone(&queried),
    };
    let app = axum::Router::new()
        .route("/simple/{project}", axum::routing::get(simple_page))
        .route("/simple/{project}/", axum::routing::get(simple_page))
        .route("/files/{name}", axum::routing::get(distribution))
        .with_state(state);
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    IndexFixture {
        endpoint,
        bytes,
        missing,
        queried,
        task,
    }
}

/// A package the authorized index does not serve must fail, not be resolved
/// somewhere else.
///
/// `uv`'s `--index` *adds to* its default index, so this is the behavioral half
/// of the boundary the argument construction above pins: measured before the
/// fix, this exact shape produced a lock whose digest came from PyPI.
#[tokio::test(flavor = "multi_thread")]
async fn a_package_the_authorized_index_lacks_is_not_resolved_elsewhere() {
    let Some(toolchain) = toolchain() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let wheels = root.path().join("wheels");
    std::fs::create_dir_all(&wheels).unwrap();
    let wheel = fixture_wheel(&wheels, "probepkg", "0.1.0");
    let index = index_fixture_missing(&wheel).await;
    let config = config_with_sources(
        root.path(),
        &toolchain,
        vec![PackageSource::Index(index.endpoint.clone())],
    );
    // A package the authorized index answers 404 for. If the resolver could fall
    // back to an implicit default index, this would resolve from the internet and
    // the plan the human approves would name digests nobody authorized.
    let error = resolve_requirements(&config, None, &["six==1.16.0".to_owned()])
        .await
        .expect_err("an authorized index that lacks the package must refuse");
    assert!(
        matches!(error, ProvisionError::Failed(_)),
        "the refusal is a resolution failure, not a fallback: {error:?}"
    );
    assert!(
        index.queries() > 0,
        "the authorized index was actually consulted"
    );
}

/// The networked source, end to end: resolve against a real index, fetch the
/// artifact over HTTP, then build and publish exactly as the offline case does.
#[tokio::test(flavor = "multi_thread")]
async fn an_authorized_index_is_resolved_fetched_and_built() {
    let Some(toolchain) = toolchain() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let wheels = root.path().join("wheels");
    std::fs::create_dir_all(&wheels).unwrap();
    let wheel = fixture_wheel(&wheels, "probepkg", "0.1.0");
    let index = index_fixture(&wheel, false).await;
    let config = config_with_sources(
        root.path(),
        &toolchain,
        vec![PackageSource::Index(index.endpoint.clone())],
    );

    let resolved = resolve_requirements(&config, None, &["probepkg==0.1.0".to_owned()])
        .await
        .expect("uv resolves against the authorized index");
    assert_eq!(resolved.packages.len(), 1, "{resolved:?}");
    assert_eq!(resolved.packages[0].version, "0.1.0");

    let wheelhouse = root.path().join("wheelhouse");
    let fetched = fetch_wheelhouse(&config, &resolved.packages, &wheelhouse)
        .await
        .unwrap();
    assert_eq!(fetched.len(), 1, "{fetched:?}");
    assert!(fetched[0].source.starts_with("index:"), "{fetched:?}");
    assert_eq!(
        fetched[0].sha256, resolved.packages[0].hashes[0],
        "the artifact came from the index and matched the approved digest"
    );
    assert!(
        index.queries() >= 2,
        "the index really answered both the page and the artifact: {}",
        index.queries()
    );

    // Everything after the fetch is identical to the offline case, but running it
    // here is what makes this an end-to-end proof of the networked shape.
    let sandbox = provision_sandbox(&config).expect("bubblewrap is available");
    let built = build_environment(&config, &sandbox, &resolved, &wheelhouse)
        .await
        .expect("the locked set builds inside the sandbox");
    let store = anchor_library::EnvironmentStore::new(root.path().join("library"));
    let outcome = store
        .publish_expecting(&built.tree, None)
        .expect("the built tree is publishable");
    let published = store.published_path(&outcome.id);
    verify_published(&config, &sandbox, &published, &resolved)
        .await
        .expect("the published tree runs where it was published");
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
}

/// An index can lie about, or simply change, what it serves. The bytes decide.
#[tokio::test(flavor = "multi_thread")]
async fn an_index_that_serves_other_bytes_than_it_advertised_is_refused() {
    let Some(toolchain) = toolchain() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let wheels = root.path().join("wheels");
    std::fs::create_dir_all(&wheels).unwrap();
    let wheel = fixture_wheel(&wheels, "probepkg", "0.1.0");
    let index = index_fixture(&wheel, false).await;
    let config = config_with_sources(
        root.path(),
        &toolchain,
        vec![PackageSource::Index(index.endpoint.clone())],
    );
    let resolved = resolve_requirements(&config, None, &["probepkg==0.1.0".to_owned()])
        .await
        .unwrap();

    // The digest the human approved stays the same; the artifact behind the URL
    // changes.
    let substituted = b"not the wheel that was approved".to_vec();
    index.replace_bytes(substituted.clone());
    let wheelhouse = root.path().join("wheelhouse");
    let error = fetch_wheelhouse(&config, &resolved.packages, &wheelhouse)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("approved digest"), "{error}");
    // The refusal names the approved digest, never what the index actually
    // served: a substituted response must not be echoed to a reader.
    let observed = super::config::sha256_hex(&substituted);
    assert!(
        !error.to_string().contains(&observed),
        "the observed digest must not be echoed: {error}"
    );
    assert!(
        !error
            .to_string()
            .contains("not the wheel that was approved"),
        "no response content belongs in the refusal: {error}"
    );
    assert!(
        std::fs::read_dir(&wheelhouse).unwrap().next().is_none(),
        "nothing may be left behind for a build to pick up"
    );
}

/// A page this host cannot read is a refusal with a reason, not a silent
/// fallback to whatever `uv` could resolve.
#[tokio::test(flavor = "multi_thread")]
async fn an_index_that_answers_html_instead_of_json_is_refused_with_a_reason() {
    let Some(toolchain) = toolchain() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let wheels = root.path().join("wheels");
    std::fs::create_dir_all(&wheels).unwrap();
    let wheel = fixture_wheel(&wheels, "probepkg", "0.1.0");
    let index = index_fixture(&wheel, true).await;
    let config = config_with_sources(
        root.path(),
        &toolchain,
        vec![PackageSource::Index(index.endpoint.clone())],
    );
    // `uv` accepts a PEP 503 page, so the boundary is here: the lock can be
    // resolved but the artifact fetch needs the JSON document.
    let resolved = resolve_requirements(&config, None, &["probepkg==0.1.0".to_owned()])
        .await
        .expect("uv resolves an html index");
    let error = fetch_wheelhouse(&config, &resolved.packages, &root.path().join("wheelhouse"))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("PEP 691"),
        "the refusal says which document it needed: {error}"
    );
}

/// The same path against the real package index.
///
/// Opt-in, because the default suite must not need the internet: this is the
/// only case that proves the online shape against a real deployment's source
/// rather than a fixture. Run it with `ANCHOR_ENVIRONMENT_LIVE_PYPI=1`.
#[tokio::test(flavor = "multi_thread")]
async fn live_the_public_index_builds_and_publishes_a_real_package() {
    if std::env::var("ANCHOR_ENVIRONMENT_LIVE_PYPI").as_deref() != Ok("1") {
        // `=0` must not run it: this case reaches the public internet.
        eprintln!("SKIPPED (by design): set ANCHOR_ENVIRONMENT_LIVE_PYPI=1 to run it");
        return;
    }
    let Some(toolchain) = toolchain() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let config = config_with_sources(
        root.path(),
        &toolchain,
        vec![PackageSource::Index("https://pypi.org/simple".to_owned())],
    );
    let resolved = resolve_requirements(&config, None, &["idna==3.7".to_owned()])
        .await
        .expect("the public index resolves idna");
    assert_eq!(resolved.packages.len(), 1, "{resolved:?}");

    let wheelhouse = root.path().join("wheelhouse");
    let fetched = fetch_wheelhouse(&config, &resolved.packages, &wheelhouse)
        .await
        .expect("the artifact downloads and matches the approved digest");
    // A real lock approves more than one artifact for a version (the index also
    // records the sdist digest), so the invariant is membership, not which one.
    assert!(
        resolved.packages[0].hashes.contains(&fetched[0].sha256),
        "{fetched:?} vs {:?}",
        resolved.packages[0].hashes
    );
    assert!(fetched[0].filename.ends_with(".whl"), "{fetched:?}");

    let sandbox = provision_sandbox(&config).expect("bubblewrap is available");
    let built = build_environment(&config, &sandbox, &resolved, &wheelhouse)
        .await
        .expect("the locked set builds inside the offline sandbox");
    let store = anchor_library::EnvironmentStore::new(root.path().join("library"));
    let outcome = store.publish_expecting(&built.tree, None).unwrap();
    let published = store.published_path(&outcome.id);
    verify_published(&config, &sandbox, &published, &resolved)
        .await
        .expect("the published tree runs where it was published");
    let output = command_output(
        &config,
        &sandbox,
        &published,
        &[
            published.join("bin/python").to_string_lossy().into_owned(),
            "-c".into(),
            "import idna; print(idna.__version__)".into(),
        ],
    )
    .await;
    assert_eq!(output.trim(), "3.7");
}

/// `uv pip compile --generate-hashes` records the sdist digest of a version too,
/// and fetching one would mean compiling it. The approved-lock entry here is the
/// file's real digest, so only the "wheels only" rule can refuse it.
#[tokio::test(flavor = "multi_thread")]
async fn an_sdist_is_never_fetched_even_when_it_carries_an_approved_digest() {
    let Some(toolchain) = toolchain() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let wheels = root.path().join("wheels");
    std::fs::create_dir_all(&wheels).unwrap();
    let wheel = fixture_wheel(&wheels, "probepkg", "0.1.0");
    let digest = super::config::sha256_file(&wheel).unwrap();
    let index = index_fixture_named(&wheel, false, Some("probepkg-0.1.0.tar.gz")).await;
    let config = config_with_sources(
        root.path(),
        &toolchain,
        vec![PackageSource::Index(index.endpoint.clone())],
    );
    let package = LockedPackage {
        name: "probepkg".into(),
        version: "0.1.0".into(),
        hashes: vec![digest],
    };
    let error = fetch_wheelhouse(
        &config,
        std::slice::from_ref(&package),
        &root.path().join("wheelhouse"),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("no wheel here"), "{error}");
}

/// And a source that can only be built never even resolves, because the ask is
/// locked with `--only-binary :all:`.
#[tokio::test(flavor = "multi_thread")]
async fn a_source_that_offers_only_source_distributions_is_refused_at_resolution() {
    let Some(toolchain) = toolchain() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let wheels = root.path().join("wheels");
    std::fs::create_dir_all(&wheels).unwrap();
    let wheel = fixture_wheel(&wheels, "probepkg", "0.1.0");
    let index = index_fixture_named(&wheel, false, Some("probepkg-0.1.0.tar.gz")).await;
    let config = config_with_sources(
        root.path(),
        &toolchain,
        vec![PackageSource::Index(index.endpoint.clone())],
    );
    let error = resolve_requirements(&config, None, &["probepkg==0.1.0".to_owned()])
        .await
        .unwrap_err();
    assert!(
        matches!(error, ProvisionError::Failed(ref reason) if reason.contains("probepkg")),
        "{error:?}"
    );
}

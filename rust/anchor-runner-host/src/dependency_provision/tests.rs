//! The builder's regression surface.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anchor_runtime::{NetworkPolicy, SandboxPort, SandboxRequest};

use super::config::PackageSource;
use super::fetch::{FetchedArtifact, fetch_wheelhouse_reporting};
use super::test_support::*;
use super::*;

fn wheel_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let wheels = root.path().join("wheels");
    std::fs::create_dir_all(&wheels).unwrap();
    let wheel = fixture_wheel(&wheels, "probepkg", "0.1.0");
    (root, wheels, wheel)
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
    /// The `content-type` the JSON document is served with. PEP 691 allows a
    /// charset parameter, so a case can exercise the media-type comparison.
    json_content_type: String,
    /// The shape of the `files[].url` the fixture advertises.
    url_style: IndexUrlStyle,
    /// Report `404` for the project, as an index without it does.
    missing: Arc<std::sync::atomic::AtomicBool>,
    queried: Arc<std::sync::atomic::AtomicUsize>,
}

/// How the fixture's file list names its artifact URL.
///
/// The real mirrors use a URL relative to the package page, so that is the
/// default: a fixture with absolute URLs would let a relative-URL regression pass
/// the whole suite again.
#[derive(Clone)]
enum IndexUrlStyle {
    /// What TUNA and USTC serve: `../../files/<name>` from the package page.
    Relative,
    /// The shape the first fixtures used: an absolute URL.
    Absolute,
    /// A path this fixture does not serve, so the artifact answers `404`.
    AbsentPath,
    /// An exact value, for a URL the outbound policy must refuse.
    Exact(String),
}

impl IndexState {
    fn artifact_url(&self) -> String {
        match &self.url_style {
            IndexUrlStyle::Relative => format!("../../files/{}", self.filename),
            IndexUrlStyle::Absolute => format!("{}/files/{}", self.endpoint, self.filename),
            IndexUrlStyle::AbsentPath => format!("{}/absent/{}", self.endpoint, self.filename),
            IndexUrlStyle::Exact(value) => value.clone(),
        }
    }
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
                "<html><body><a href=\"{}\">{}</a></body></html>",
                state.artifact_url(),
                state.filename
            ),
        )
            .into_response();
    }
    let body = serde_json::json!({
        "meta": {"api-version": "1.0"},
        "name": "probepkg",
        "files": [{
            "filename": state.filename,
            "url": state.artifact_url(),
            "hashes": {"sha256": state.digest},
        }],
    })
    .to_string();
    (
        axum::http::StatusCode::OK,
        [("content-type", state.json_content_type.clone())],
        body,
    )
        .into_response()
}

/// The file endpoint of a real mirror: TUNA answers `403` to a request that
/// carries no `User-Agent` at all and `200` to any non-empty one, so the fixture
/// does the same. Without this the suite would not notice a client that looks
/// like an anonymous downloader.
async fn distribution(
    axum::extract::State(state): axum::extract::State<IndexState>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    state
        .queried
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let user_agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if user_agent.trim().is_empty() {
        return axum::http::StatusCode::FORBIDDEN.into_response();
    }
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

/// The same fixture, optionally advertising a different file name — an sdist, so
/// a case can prove this host never fetches one. Its artifact URL is relative,
/// which is the shape the real mirrors serve.
async fn index_fixture_named(wheel: &Path, as_html: bool, rename_to: Option<&str>) -> IndexFixture {
    index_fixture_serving(
        wheel,
        as_html,
        rename_to,
        "application/vnd.pypi.simple.v1+json",
        IndexUrlStyle::Relative,
    )
    .await
}

/// The same fixture with the exact `content-type` the JSON document is served
/// with, so a case can cover PEP 691's optional `charset` parameter.
async fn index_fixture_serving(
    wheel: &Path,
    as_html: bool,
    rename_to: Option<&str>,
    json_content_type: &str,
    url_style: IndexUrlStyle,
) -> IndexFixture {
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
        json_content_type: json_content_type.to_owned(),
        url_style,
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
    let (root, _wheels, wheel) = wheel_fixture();
    let index = index_fixture(&wheel, false).await;
    // Report that the authorized index does not contain the requested project.
    index
        .missing
        .store(true, std::sync::atomic::Ordering::SeqCst);
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

/// An index can lie about, or simply change, what it serves. The bytes decide.
#[tokio::test(flavor = "multi_thread")]
async fn an_index_that_serves_other_bytes_than_it_advertised_is_refused() {
    let Some(toolchain) = toolchain() else {
        return;
    };
    let (root, _wheels, wheel) = wheel_fixture();
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
    let (root, _wheels, wheel) = wheel_fixture();
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

/// The probe is a capability check, not a blanket refusal of indices: the JSON
/// fixture passes and the HTML fixture does not.
#[tokio::test(flavor = "multi_thread")]
async fn the_json_probe_distinguishes_a_pep_691_index_from_an_html_one() {
    let (_root, _wheels, wheel) = wheel_fixture();

    let json = index_fixture(&wheel, false).await;
    verify_index_json(&json.endpoint, "probepkg")
        .await
        .expect("a PEP 691 index passes the probe");

    let html = index_fixture(&wheel, true).await;
    let error = verify_index_json(&html.endpoint, "probepkg")
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("PEP 691") && error.to_string().contains(&html.endpoint),
        "{error}"
    );
}

/// PEP 691 allows a `charset` parameter on the JSON document's content type, so
/// the probe compares the media type rather than the whole header value.
#[tokio::test(flavor = "multi_thread")]
async fn the_json_probe_accepts_a_pep_691_content_type_with_parameters() {
    let (_root, _wheels, wheel) = wheel_fixture();
    let index = index_fixture_serving(
        &wheel,
        false,
        None,
        "application/vnd.pypi.simple.v1+json; charset=utf-8",
        IndexUrlStyle::Relative,
    )
    .await;

    verify_index_json(&index.endpoint, "probepkg")
        .await
        .expect("a charset parameter is allowed on the PEP 691 JSON content type");
}

/// A deployment config for fetching only: nothing here executes `uv` or the
/// interpreter, so these cases do not need the builder toolchain.
fn fetch_only_config(root: &Path, index: &str) -> DependencyProvisionConfig {
    fetch_only_config_for(root, vec![PackageSource::Index(index.to_owned())])
}

/// The same config with the sources a case wants to see tried, in order.
fn fetch_only_config_for(root: &Path, sources: Vec<PackageSource>) -> DependencyProvisionConfig {
    DependencyProvisionConfig {
        uv: PathBuf::from("/nonexistent/uv"),
        uv_digest: "a".repeat(64),
        python: PathBuf::from("/usr/bin/python3"),
        sources,
        build_root: root.join("build"),
        bwrap: PathBuf::from("bwrap"),
    }
}

/// Run one fetch with its per-source report collected, so a case can read exactly
/// the lines the host writes to its stderr.
async fn fetch_with_report(
    config: &DependencyProvisionConfig,
    packages: &[LockedPackage],
    wheelhouse: &Path,
) -> (Result<Vec<FetchedArtifact>, ProvisionError>, Vec<String>) {
    let mut report = Vec::new();
    let result = fetch_wheelhouse_reporting(config, packages, wheelhouse, &mut report).await;
    let lines = String::from_utf8(report)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    (result, lines)
}

/// Run one production fetch with this process's stderr descriptor captured, so a
/// case can prove the report reaches stderr and not only an in-process buffer.
async fn fetch_capturing_descriptor(
    config: &DependencyProvisionConfig,
    packages: &[LockedPackage],
    wheelhouse: &Path,
) -> (Result<Vec<FetchedArtifact>, ProvisionError>, String) {
    use std::os::fd::AsFd;
    let log = tempfile::NamedTempFile::new().unwrap();
    let sink = std::fs::OpenOptions::new()
        .write(true)
        .open(log.path())
        .unwrap();
    let saved = rustix::io::dup(std::io::stderr().as_fd()).unwrap();
    rustix::stdio::dup2_stderr(sink.as_fd()).unwrap();
    let result = fetch_wheelhouse(config, packages, wheelhouse).await;
    rustix::stdio::dup2_stderr(&saved).unwrap();
    let captured = std::fs::read_to_string(log.path()).unwrap();
    (result, captured)
}

/// The lock entry for one package, as resolution would have produced it.
fn locked_package(name: &str, version: &str, digest: &str) -> LockedPackage {
    LockedPackage {
        name: name.to_owned(),
        version: version.to_owned(),
        hashes: vec![digest.to_owned()],
    }
}

/// The real mirrors offer the artifact as a path relative to the package page
/// (`../../packages/…`), so the fetch has to resolve it against that page.
#[tokio::test(flavor = "multi_thread")]
async fn a_relative_artifact_url_is_resolved_against_the_package_page() {
    let (root, _wheels, wheel) = wheel_fixture();
    let digest = super::config::sha256_file(&wheel).unwrap();
    let index = index_fixture(&wheel, false).await;
    let config = fetch_only_config(root.path(), &index.endpoint);
    let wheelhouse = root.path().join("wheelhouse");

    let fetched = fetch_wheelhouse(
        &config,
        &[locked_package("probepkg", "0.1.0", &digest)],
        &wheelhouse,
    )
    .await
    .expect("a relative artifact URL resolves against its package page");
    assert_eq!(fetched[0].sha256, digest, "{fetched:?}");
    assert!(fetched[0].source.starts_with("index:"), "{fetched:?}");
    assert!(wheelhouse.join(&fetched[0].filename).is_file());
}

/// An absolute artifact URL keeps working: both shapes are legal PEP 691.
#[tokio::test(flavor = "multi_thread")]
async fn an_absolute_artifact_url_is_still_fetched() {
    let (root, _wheels, wheel) = wheel_fixture();
    let digest = super::config::sha256_file(&wheel).unwrap();
    let index = index_fixture_serving(
        &wheel,
        false,
        None,
        "application/vnd.pypi.simple.v1+json",
        IndexUrlStyle::Absolute,
    )
    .await;
    let config = fetch_only_config(root.path(), &index.endpoint);

    let fetched = fetch_wheelhouse(
        &config,
        &[locked_package("probepkg", "0.1.0", &digest)],
        &root.path().join("wheelhouse"),
    )
    .await
    .expect("an absolute artifact URL is fetched as it stands");
    assert_eq!(fetched[0].sha256, digest, "{fetched:?}");
}

/// A relative slot must not become a way around the outbound policy: the page
/// offers a protocol-relative address to a private host, and it is refused
/// without the artifact being requested at all.
#[tokio::test(flavor = "multi_thread")]
async fn an_artifact_url_the_policy_refuses_is_never_requested() {
    let (root, _wheels, wheel) = wheel_fixture();
    let digest = super::config::sha256_file(&wheel).unwrap();
    let index = index_fixture_serving(
        &wheel,
        false,
        None,
        "application/vnd.pypi.simple.v1+json",
        IndexUrlStyle::Exact("//10.0.0.1/packages/probepkg-0.1.0.whl".into()),
    )
    .await;
    let config = fetch_only_config(root.path(), &index.endpoint);

    let error = fetch_wheelhouse(
        &config,
        &[locked_package("probepkg", "0.1.0", &digest)],
        &root.path().join("wheelhouse"),
    )
    .await
    .unwrap_err();
    assert!(
        error.to_string().contains("is not an authorized address"),
        "the policy refuses the resolved address: {error}"
    );
    assert_eq!(
        index.queries(),
        1,
        "only the package page was read; the refused artifact was never requested"
    );
}

/// Two authorized sources, the first without the bytes: the host log must say so
/// per source, and the artifact must come from the second one.
#[tokio::test(flavor = "multi_thread")]
async fn the_fetch_reports_every_source_and_serves_from_the_one_that_has_the_bytes() {
    let (root, _wheels, wheel) = wheel_fixture();
    let digest = super::config::sha256_file(&wheel).unwrap();
    // The first source lists the package but not the artifact; the second serves it.
    let broken = index_fixture_serving(
        &wheel,
        false,
        None,
        "application/vnd.pypi.simple.v1+json",
        IndexUrlStyle::AbsentPath,
    )
    .await;
    let good = index_fixture(&wheel, false).await;
    let config = fetch_only_config_for(
        root.path(),
        vec![
            PackageSource::Index(broken.endpoint.clone()),
            PackageSource::Index(good.endpoint.clone()),
        ],
    );

    let (result, lines) = fetch_with_report(
        &config,
        &[locked_package("probepkg", "0.1.0", &digest)],
        &root.path().join("wheelhouse"),
    )
    .await;
    let fetched = result.expect("the second source supplies the bytes");
    assert_eq!(fetched[0].sha256, digest, "{fetched:?}");
    assert_eq!(fetched[0].source, format!("index:{}", good.endpoint));

    assert_eq!(lines.len(), 2, "one line per source: {lines:?}");
    assert!(
        lines[0].contains(&broken.endpoint)
            && lines[0].contains("failed 1 package(s)")
            && lines[0].contains("probepkg==0.1.0")
            && lines[0].contains("404"),
        "{lines:?}"
    );
    assert!(
        lines[1].contains(&good.endpoint)
            && lines[1].contains("served 1 package(s)")
            && lines[1].contains("probepkg==0.1.0"),
        "{lines:?}"
    );
}

/// One source that has the bytes: exactly one line, marked as serving them.
#[tokio::test(flavor = "multi_thread")]
async fn a_single_source_fetch_reports_one_served_line() {
    let (root, _wheels, wheel) = wheel_fixture();
    let digest = super::config::sha256_file(&wheel).unwrap();
    let index = index_fixture(&wheel, false).await;
    let config = fetch_only_config(root.path(), &index.endpoint);

    let (result, lines) = fetch_with_report(
        &config,
        &[locked_package("probepkg", "0.1.0", &digest)],
        &root.path().join("wheelhouse"),
    )
    .await;
    assert!(result.is_ok(), "{result:?}");

    assert_eq!(lines.len(), 1, "one line per source: {lines:?}");
    assert!(
        lines[0].contains(&index.endpoint)
            && lines[0].contains("served 1 package(s)")
            && lines[0].contains("probepkg==0.1.0"),
        "{lines:?}"
    );
}

/// A source an earlier one made unnecessary is still recorded, as "not used":
/// the log then says which mirror was skipped rather than leaving it to inference.
#[tokio::test(flavor = "multi_thread")]
async fn a_source_that_was_not_needed_is_recorded_as_such() {
    let (root, _wheels, wheel) = wheel_fixture();
    let digest = super::config::sha256_file(&wheel).unwrap();
    let first = index_fixture(&wheel, false).await;
    let unused = index_fixture(&wheel, false).await;
    let config = fetch_only_config_for(
        root.path(),
        vec![
            PackageSource::Index(first.endpoint.clone()),
            PackageSource::Index(unused.endpoint.clone()),
        ],
    );

    let (result, lines) = fetch_with_report(
        &config,
        &[locked_package("probepkg", "0.1.0", &digest)],
        &root.path().join("wheelhouse"),
    )
    .await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(unused.queries(), 0, "the second source is never queried");

    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].contains("served 1 package(s)"), "{lines:?}");
    assert!(
        lines[1].contains(&unused.endpoint) && lines[1].contains("not used"),
        "{lines:?}"
    );
}

/// `uv pip compile --generate-hashes` records the sdist digest of a version too,
/// and fetching one would mean compiling it. The approved-lock entry here is the
/// file's real digest, so only the "wheels only" rule can refuse it.
#[tokio::test(flavor = "multi_thread")]
async fn an_sdist_is_never_fetched_even_when_it_carries_an_approved_digest() {
    let Some(toolchain) = toolchain() else {
        return;
    };
    let (root, _wheels, wheel) = wheel_fixture();
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
    let (root, _wheels, wheel) = wheel_fixture();
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

/// The production entry point writes that report to this process's stderr, which
/// is what the deployment's host log records.
#[tokio::test(flavor = "multi_thread")]
async fn the_fetch_report_reaches_this_process_stderr() {
    let (root, _wheels, wheel) = wheel_fixture();
    let digest = super::config::sha256_file(&wheel).unwrap();
    let index = index_fixture(&wheel, false).await;
    let config = fetch_only_config(root.path(), &index.endpoint);

    let (result, captured) = fetch_capturing_descriptor(
        &config,
        &[locked_package("probepkg", "0.1.0", &digest)],
        &root.path().join("wheelhouse"),
    )
    .await;
    assert!(result.is_ok(), "{result:?}");
    // Another case's report may share this descriptor, so the assertion is scoped
    // to this fixture's own address.
    let mine = captured
        .lines()
        .filter(|line| line.contains(&index.endpoint))
        .collect::<Vec<_>>();
    assert_eq!(mine.len(), 1, "stderr carried: {captured:?}");
    assert!(
        mine[0].contains("served 1 package(s)") && mine[0].contains("probepkg==0.1.0"),
        "{captured:?}"
    );
}

/// A deployment may authorize an index that cannot serve a package *and* a
/// directory that can. The interface check mirrors the fetch — a package is
/// deliverable as soon as one authorized source can serve it — so a Plugin whose
/// packages the directory holds must not be refused before a human ever sees it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_directory_source_keeps_a_plugin_package_deliverable_when_an_index_answers_html() {
    let Some(toolchain) = toolchain() else {
        return;
    };
    let (root, wheels, wheel) = wheel_fixture();
    // The index answers PEP 503 HTML, which this host cannot fetch from; the
    // directory can still deliver the artifact.
    let index = index_fixture(&wheel, true).await;
    let config = config_with_sources(
        &root.path().join("build"),
        &toolchain,
        vec![
            PackageSource::Index(index.endpoint.clone()),
            PackageSource::Directory(wheels),
        ],
    );
    let provisioner = Arc::new(
        DependencyProvisioner::new(config).expect("the deployment configuration builds a sandbox"),
    );
    let library = root.path().join("library");
    let candidate = root.path().join("candidate");
    std::fs::create_dir_all(&candidate).unwrap();
    std::fs::write(
        candidate.join("plugin.json"),
        serde_json::json!({
            "name": "WithDirectory",
            "dependencies": {"python": ["probepkg==0.1.0"]},
        })
        .to_string(),
    )
    .unwrap();
    let application = crate::application::RunApplication::new(
        root.path().join("state"),
        root.path().to_path_buf(),
    )
    .with_dependency_provisioner(provisioner);

    let precondition = application
        .plugin_install_precondition(library, candidate, "with-directory".into())
        .await
        .expect("the authorized directory keeps the package deliverable");
    assert_eq!(
        precondition
            .dependencies
            .expect("the declaration resolved to a plan")
            .packages
            .len(),
        1
    );
    // The index really was consulted and really answered HTML, so this case is
    // about the directory saving the package rather than about skipping the check.
    assert!(index.queries() >= 1);
}

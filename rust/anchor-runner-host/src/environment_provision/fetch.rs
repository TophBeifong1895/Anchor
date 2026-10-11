//! Fetching exactly the artifacts the lock pinned.
//!
//! Nothing here trusts a filename, a URL or an index claim: an artifact is
//! accepted only when its bytes hash to a digest the approved lock already
//! contains. That makes the index a delivery mechanism rather than an authority
//! — a substituted or redirected download simply fails to match.
//!
//! Index and artifact URLs must pass the same URL policy the host already uses
//! for outbound OAuth requests (public address, no embedded credentials, no
//! redirects), so an authorized source cannot become a general-purpose fetcher.
//!
//! Fetching is asynchronous on purpose. A *blocking* HTTP client builds its own
//! runtime internally, and dropping one on a runtime thread panics; keeping this
//! interface async means no caller can land in that trap by calling it from the
//! wrong place.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use url::Url;

use super::config::{EnvironmentProvisionConfig, PackageSource, ProvisionError};
use super::resolve::LockedPackage;

/// One artifact written into the wheelhouse.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FetchedArtifact {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) filename: String,
    pub(crate) sha256: String,
    /// Which authorized source supplied it.
    pub(crate) source: String,
}

/// The content type a PEP 691 JSON package page answers with.
const PEP691_JSON: &str = "application/vnd.pypi.simple.v1+json";

/// What this host calls itself on an environment index request.
///
/// A mirror's file endpoint may refuse a request that carries no `User-Agent` at
/// all — TUNA answers `403` to one and `200` to any non-empty value (measured),
/// and `uv` sends its own — so the host identifies itself instead of looking like
/// an anonymous client. This is a request header, not an address-policy change:
/// the URL is still checked by [`crate::oauth_http::validate_oauth_url`].
const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

/// Confirm one authorized index really serves a PEP 691 JSON package page.
///
/// This runs while the plan is being frozen, *before* a human is asked to approve
/// it: many mirrors answer the package page with PEP 503 HTML, which `uv` can
/// resolve from but this host cannot fetch from. Finding that out after approval
/// would mean asking somebody to approve a plan that can never be delivered.
///
/// `index` is the configured index root (it already ends in `/simple`), exactly
/// as [`from_index`] uses it. The request goes through the same hardened client as
/// every other outbound host request, so the address policy is the fetch's own.
pub(crate) async fn verify_index_json(index: &str, package: &str) -> Result<(), ProvisionError> {
    let project = normalize_name(package);
    let page = format!("{index}/{project}/");
    let page_url = Url::parse(&page).map_err(|error| {
        ProvisionError::Invalid(format!("package index `{index}` is not a URL: {error}"))
    })?;
    let client = crate::oauth_http::async_client(&page_url)
        .await
        .map_err(|_| {
            ProvisionError::Invalid(format!(
                "package index `{index}` is not an authorized address (a public https index, or http on loopback)"
            ))
        })?;
    let response = client
        .get(page_url)
        .header("Accept", PEP691_JSON)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await
        .map_err(|error| {
            ProvisionError::Invalid(format!(
                "package index `{index}` could not be reached to check its JSON interface: {error}"
            ))
        })?;
    if !response.status().is_success() {
        return Err(ProvisionError::Invalid(format!(
            "package index `{index}` answered {} for `{project}`, so it cannot be checked for the required JSON interface",
            response.status()
        )));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    // PEP 691 allows a charset parameter, so compare the media type only.
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if media_type != PEP691_JSON {
        return Err(ProvisionError::Invalid(format!(
            "package index `{index}` answered `{content_type}` for `{project}`, not `{PEP691_JSON}`; this host needs a PEP 691 JSON index, and a mirror that only serves HTML package pages cannot be used"
        )));
    }
    Ok(())
}

/// Copy every locked artifact into `wheelhouse`, or fail.
///
/// Partial downloads are never left behind, and a failure names every source it
/// tried, because "the package is missing" and "the index is unreachable" are
/// different problems for whoever has to fix the ask.
pub(crate) async fn fetch_wheelhouse(
    config: &EnvironmentProvisionConfig,
    packages: &[LockedPackage],
    wheelhouse: &Path,
) -> Result<Vec<FetchedArtifact>, ProvisionError> {
    fetch_wheelhouse_reporting(config, packages, wheelhouse, &mut std::io::stderr()).await
}

/// The same fetch, with the per-source report going to `sink`.
///
/// The host passes its stderr, so the report lands in the deployment's host log;
/// the seam exists so a test can read exactly the lines that go there.
pub(super) async fn fetch_wheelhouse_reporting(
    config: &EnvironmentProvisionConfig,
    packages: &[LockedPackage],
    wheelhouse: &Path,
    sink: &mut (dyn std::io::Write + Send),
) -> Result<Vec<FetchedArtifact>, ProvisionError> {
    std::fs::create_dir_all(wheelhouse).map_err(|error| {
        ProvisionError::Failed(format!("the wheelhouse could not be created: {error}"))
    })?;
    let mut fetched = Vec::new();
    let mut outcomes: Vec<SourceOutcome> = config
        .sources
        .iter()
        .map(|_| SourceOutcome::default())
        .collect();
    let mut failure = None;
    for package in packages {
        let spec = format!("{}=={}", package.name, package.version);
        let mut reasons = Vec::new();
        let mut found = None;
        for (index, source) in config.sources.iter().enumerate() {
            let result = match source {
                PackageSource::Directory(path) => from_directory(package, path, wheelhouse),
                PackageSource::Index(url) => from_index(config, package, url, wheelhouse).await,
            };
            match result {
                Ok(artifact) => {
                    outcomes[index].served.push(spec.clone());
                    found = Some(artifact);
                    break;
                }
                Err(reason) => {
                    outcomes[index]
                        .failed
                        .push((spec.clone(), reason_summary(&reason)));
                    reasons.push(format!("{}: {reason}", source.as_str()));
                }
            }
        }
        match found {
            Some(artifact) => fetched.push(artifact),
            // The report is written even when nothing could be fetched: that is
            // the case an operator most needs to see per source.
            None => {
                failure = Some(ProvisionError::Failed(format!(
                    "no authorized source has `{}=={}` with the approved digest ({})",
                    package.name,
                    package.version,
                    reasons.join("; ")
                )));
                break;
            }
        }
    }
    report_sources(sink, &config.sources, &outcomes);
    match failure {
        Some(error) => Err(error),
        None => Ok(fetched),
    }
}

/// What one authorized source did while a fetch ran.
#[derive(Default)]
struct SourceOutcome {
    /// The package specs this source supplied.
    served: Vec<String>,
    /// The package specs it did not supply, each with a one-line reason.
    failed: Vec<(String, String)>,
}

impl SourceOutcome {
    fn line(&self) -> String {
        if self.served.is_empty() && self.failed.is_empty() {
            return "not used; no package needed it".to_owned();
        }
        let mut parts = Vec::new();
        if !self.served.is_empty() {
            parts.push(format!(
                "served {} package(s): {}",
                self.served.len(),
                list(&self.served)
            ));
        }
        if !self.failed.is_empty() {
            parts.push(format!(
                "failed {} package(s): {}",
                self.failed.len(),
                self.failed
                    .iter()
                    .map(|(spec, reason)| format!("{spec} ({reason})"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        parts.join("; ")
    }
}

/// Write one line per configured source, so the host log records which source
/// actually supplied the bytes instead of leaving it to be inferred.
///
/// Only the source address (credential-free by validation), the outcome and a
/// bounded reason are written: never a response body. A write failure is ignored
/// — the report may not turn a successful fetch into a failed one.
fn report_sources(
    sink: &mut (dyn std::io::Write + Send),
    sources: &[PackageSource],
    outcomes: &[SourceOutcome],
) {
    for (source, outcome) in sources.iter().zip(outcomes) {
        let _ = writeln!(
            sink,
            "anchor environment fetch: {} {}",
            source.as_str(),
            outcome.line()
        );
    }
}

/// A bounded, single-line list of package specs.
fn list(specs: &[String]) -> String {
    const SHOWN: usize = 6;
    let mut shown = specs
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if specs.len() > SHOWN {
        shown.push_str(&format!(", +{} more", specs.len() - SHOWN));
    }
    shown
}

/// A failure reason as one bounded line: whitespace collapsed, no response body.
fn reason_summary(reason: &str) -> String {
    const MAX_CHARS: usize = 160;
    let mut summary = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some((cut, _)) = summary.char_indices().nth(MAX_CHARS) {
        summary.truncate(cut);
        summary.push('…');
    }
    summary
}

/// Find the approved artifact in a directory of already-built distributions.
fn from_directory(
    package: &LockedPackage,
    directory: &Path,
    wheelhouse: &Path,
) -> Result<FetchedArtifact, String> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("{} is not readable: {error}", directory.display()))?;
    let prefix = format!("{}-", normalize_name(&package.name));
    let mut candidates: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if !name.ends_with(".whl") || !name.replace('_', "-").starts_with(&prefix) {
            continue;
        }
        candidates.push(path);
    }
    candidates.sort();
    for candidate in candidates {
        let digest = super::config::sha256_file(&candidate).map_err(|error| error.to_string())?;
        if !package.hashes.contains(&digest) {
            continue;
        }
        let filename = candidate
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| "the candidate has no file name".to_owned())?;
        safe_filename(&filename)?;
        let target = wheelhouse.join(&filename);
        std::fs::copy(&candidate, &target)
            .map_err(|error| format!("{filename} could not be copied: {error}"))?;
        return Ok(FetchedArtifact {
            name: package.name.clone(),
            version: package.version.clone(),
            filename,
            sha256: digest,
            source: format!("directory:{}", directory.display()),
        });
    }
    Err("no distribution with the approved digest".to_owned())
}

/// The absolute URL of one artifact, resolved against the page that offered it.
///
/// PEP 691 lets the file list carry a URL relative to the package page, and the
/// real mirrors use that shape (`../../packages/…`), so the page URL is the base.
/// Resolving first and checking second is what keeps a relative slot from being a
/// way around the outbound policy: a protocol-relative (`//host/…`) or absolute
/// private address becomes an absolute URL here and is then refused by the same
/// rule every other host request uses.
fn artifact_url(page_url: &Url, value: &str) -> Result<Url, String> {
    let resolved = page_url.join(value).map_err(|error| {
        format!("the artifact URL `{value}` is unusable relative to the index page: {error}")
    })?;
    crate::oauth_http::validate_oauth_url(&resolved).map_err(|_| {
        format!("the artifact URL `{value}` is not an authorized address (a public https or loopback address is required)")
    })?;
    Ok(resolved)
}

/// Ask one simple index for the approved artifact.
async fn from_index(
    config: &EnvironmentProvisionConfig,
    package: &LockedPackage,
    index: &str,
    wheelhouse: &Path,
) -> Result<FetchedArtifact, String> {
    let project = normalize_name(&package.name);
    let page = format!("{index}/{project}/");
    let page_url = Url::parse(&page).map_err(|error| format!("{page} is not a URL: {error}"))?;
    crate::oauth_http::validate_oauth_url(&page_url)
        .map_err(|_| format!("{page} is not an authorized index URL"))?;
    let client = crate::oauth_http::async_client(&page_url)
        .await
        .map_err(|_| format!("{page} could not be reached safely"))?;
    let response = client
        .get(page_url.clone())
        .header("Accept", "application/vnd.pypi.simple.v1+json")
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .timeout(config.http_timeout())
        .send()
        .await
        .map_err(|error| format!("the index could not be queried: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("the index answered {}", response.status()));
    }
    let body = response
        .text()
        .await
        .map_err(|error| format!("the index answered unreadable content: {error}"))?;
    let document: serde_json::Value = serde_json::from_str(&body)
        .map_err(|_| "the index did not answer a PEP 691 JSON document".to_owned())?;
    let files = document
        .get("files")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "the index document has no file list".to_owned())?;

    // A wheel or nothing. `uv pip compile --generate-hashes` also records the
    // sdist digest of the same version, and installing an sdist would mean
    // building it — which this slice deliberately does not do, offline or
    // otherwise. Refusing here keeps "no compilation" structural rather than a
    // property of whatever the index happens to offer.
    let mut candidate: Option<(String, String, String)> = None;
    for file in files {
        let Some(filename) = file.get("filename").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if !filename.ends_with(".whl") {
            continue;
        }
        let Some(hash) = file
            .get("hashes")
            .and_then(|hashes| hashes.get("sha256"))
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let hash = hash.to_ascii_lowercase();
        if !package.hashes.contains(&hash) {
            continue;
        }
        let Some(url) = file.get("url").and_then(serde_json::Value::as_str) else {
            continue;
        };
        candidate = Some((filename.to_owned(), hash, url.to_owned()));
        break;
    }
    let (filename, digest, url) = candidate.ok_or_else(|| {
        format!(
            "`{}=={}` has no wheel here with the approved digest",
            package.name, package.version
        )
    })?;
    safe_filename(&filename)?;
    let file_url = artifact_url(&page_url, &url)?;
    let client = crate::oauth_http::async_client(&file_url)
        .await
        .map_err(|_| "the artifact could not be reached safely".to_owned())?;
    let mut response = client
        .get(file_url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .timeout(config.http_timeout())
        .send()
        .await
        .map_err(|error| format!("the artifact could not be downloaded: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("the artifact answered {}", response.status()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > config.max_artifact_bytes())
    {
        return Err("the artifact is larger than this host will download".to_owned());
    }

    let temporary = wheelhouse.join(format!(".{filename}.part"));
    let written = write_verified(
        &mut response,
        &temporary,
        &digest,
        config.max_artifact_bytes(),
    )
    .await;
    if let Err(reason) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(reason);
    }
    std::fs::rename(&temporary, wheelhouse.join(&filename))
        .map_err(|error| format!("{filename} could not be stored: {error}"))?;
    Ok(FetchedArtifact {
        name: package.name.clone(),
        version: package.version.clone(),
        filename,
        sha256: digest,
        source: format!("index:{index}"),
    })
}

/// Stream a response into a file, refusing anything whose bytes are not the
/// approved digest or that exceeds the size this host will keep.
async fn write_verified(
    response: &mut reqwest::Response,
    path: &Path,
    expected: &str,
    limit: u64,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        total += chunk.len() as u64;
        if total > limit {
            return Err("the artifact is larger than this host will download".to_owned());
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|error| error.to_string())?;
    }
    file.sync_all().await.map_err(|error| error.to_string())?;
    let observed = format!("{:x}", hasher.finalize());
    if observed != expected {
        // The observed value is deliberately not echoed: a substituted artifact
        // must not put response content (or its digest) into a message a model
        // can read. Only the digest the human approved is named.
        return Err(format!(
            "the downloaded artifact does not match the approved digest (expected {expected})"
        ));
    }
    Ok(())
}

/// PEP 503 normalization, which is what an index path uses.
pub(crate) fn normalize_name(name: &str) -> String {
    let mut normalized = String::with_capacity(name.len());
    let mut last_dash = false;
    for character in name.chars() {
        let character = character.to_ascii_lowercase();
        if character == '-' || character == '_' || character == '.' {
            if !last_dash {
                normalized.push('-');
                last_dash = true;
            }
        } else {
            normalized.push(character);
            last_dash = false;
        }
    }
    normalized
}

/// An artifact name from an index is a file name, never a path.
fn safe_filename(filename: &str) -> Result<(), String> {
    if filename.is_empty()
        || filename.len() > 200
        || filename.starts_with('.')
        || filename.contains('/')
        || filename.contains('\\')
        || filename.contains('\0')
    {
        return Err(format!("`{filename}` is not a usable artifact name"));
    }
    Ok(())
}

#[cfg(test)]
mod fetch_tests {
    use super::*;

    #[test]
    fn an_index_path_is_normalized_the_pep_503_way() {
        assert_eq!(normalize_name("Typing_Extensions"), "typing-extensions");
        assert_eq!(normalize_name("zope.interface"), "zope-interface");
        assert_eq!(normalize_name("a__b"), "a-b");
    }

    #[test]
    fn an_index_cannot_name_a_path_outside_the_wheelhouse() {
        assert!(safe_filename("pkg-1.0-py3-none-any.whl").is_ok());
        assert!(safe_filename("../escape.whl").is_err());
        assert!(safe_filename("/etc/passwd").is_err());
        assert!(safe_filename(".hidden.whl").is_err());
        assert!(safe_filename("").is_err());
    }

    /// The real mirrors offer the artifact as a path relative to the package
    /// page, and the resolved address must pass the same policy as any absolute
    /// one — a relative slot is not a way around it.
    #[test]
    fn an_artifact_url_is_resolved_against_the_page_and_then_policed() {
        let page = Url::parse("https://pypi.tuna.tsinghua.edu.cn/simple/pygments/").unwrap();

        // What the mirrors actually serve.
        let relative = artifact_url(
            &page,
            "../../packages/aa/bb/cc/pygments-2.20.0-py3-none-any.whl",
        )
        .unwrap();
        assert_eq!(
            relative.as_str(),
            "https://pypi.tuna.tsinghua.edu.cn/packages/aa/bb/cc/pygments-2.20.0-py3-none-any.whl"
        );

        // An absolute authorized URL is used as it stands.
        let absolute =
            artifact_url(&page, "https://files.pythonhosted.org/packages/x.whl").unwrap();
        assert_eq!(
            absolute.as_str(),
            "https://files.pythonhosted.org/packages/x.whl"
        );

        // A relative path cannot leave the authority the page belongs to: the
        // policy checks scheme, host and port, never the path.
        let traversed = artifact_url(&page, "../../../../etc/passwd").unwrap();
        assert_eq!(traversed.host_str(), Some("pypi.tuna.tsinghua.edu.cn"));

        // Protocol-relative and absolute private targets are refused by the
        // policy itself, not silently fetched.
        for value in [
            "//10.0.0.1/packages/x.whl",
            "//169.254.169.254/latest/meta-data/",
            "http://10.0.0.1/x.whl",
            "file:///etc/passwd",
            "https://user:secret@10.0.0.1/x.whl",
        ] {
            let error = artifact_url(&page, value).unwrap_err();
            assert!(
                error.contains("is not an authorized address"),
                "{value}: {error}"
            );
        }
    }

    /// Opt-in: the recommended mirror really serves a *relative* artifact URL,
    /// and the fetch turns it into the approved bytes. Run it with
    /// `ANCHOR_ENVIRONMENT_LIVE_MIRROR=1`.
    ///
    /// The digest is the one a lock for `idna==3.7` contains, which is what makes
    /// this the fetch path rather than a plain download: the candidate is chosen
    /// by the approved hash, exactly as a real plan does.
    #[tokio::test(flavor = "multi_thread")]
    async fn live_the_recommended_mirror_yields_the_approved_bytes_from_a_relative_url() {
        if std::env::var("ANCHOR_ENVIRONMENT_LIVE_MIRROR").as_deref() != Ok("1") {
            eprintln!("SKIPPED (by design): set ANCHOR_ENVIRONMENT_LIVE_MIRROR=1 to run it");
            return;
        }
        let index = "https://pypi.tuna.tsinghua.edu.cn/simple";
        let root = tempfile::tempdir().unwrap();
        let wheelhouse = root.path().join("wheelhouse");
        std::fs::create_dir_all(&wheelhouse).unwrap();
        let config = EnvironmentProvisionConfig {
            uv: std::path::PathBuf::from("/nonexistent/uv"),
            uv_digest: "a".repeat(64),
            python: std::path::PathBuf::from("/usr/bin/python3"),
            sources: vec![PackageSource::Index(index.to_owned())],
            build_root: root.path().join("build"),
            bwrap: std::path::PathBuf::from("bwrap"),
        };
        let package = LockedPackage {
            name: "idna".to_owned(),
            version: "3.7".to_owned(),
            hashes: vec![
                "82fee1fc78add43492d3a1898bfa6d8a904cc97d8427f683ed8e798d07761aa0".to_owned(),
            ],
        };

        let fetched = from_index(&config, &package, index, &wheelhouse)
            .await
            .expect("the mirror's relative artifact URL yields the approved bytes");
        assert_eq!(fetched.sha256, package.hashes[0]);
        assert!(wheelhouse.join(&fetched.filename).is_file(), "{fetched:?}");
    }
}

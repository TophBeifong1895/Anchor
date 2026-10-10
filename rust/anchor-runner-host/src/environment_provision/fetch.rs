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
    std::fs::create_dir_all(wheelhouse).map_err(|error| {
        ProvisionError::Failed(format!("the wheelhouse could not be created: {error}"))
    })?;
    let mut fetched = Vec::new();
    for package in packages {
        let mut reasons = Vec::new();
        let mut found = None;
        for source in &config.sources {
            let result = match source {
                PackageSource::Directory(path) => from_directory(package, path, wheelhouse),
                PackageSource::Index(url) => from_index(config, package, url, wheelhouse).await,
            };
            match result {
                Ok(artifact) => {
                    found = Some(artifact);
                    break;
                }
                Err(reason) => reasons.push(format!("{}: {reason}", source.as_str())),
            }
        }
        match found {
            Some(artifact) => fetched.push(artifact),
            None => {
                return Err(ProvisionError::Failed(format!(
                    "no authorized source has `{}=={}` with the approved digest ({})",
                    package.name,
                    package.version,
                    reasons.join("; ")
                )));
            }
        }
    }
    Ok(fetched)
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
    let file_url =
        Url::parse(&url).map_err(|error| format!("the artifact URL is unusable: {error}"))?;
    crate::oauth_http::validate_oauth_url(&file_url)
        .map_err(|_| "the artifact URL is not an authorized address".to_owned())?;
    let client = crate::oauth_http::async_client(&file_url)
        .await
        .map_err(|_| "the artifact could not be reached safely".to_owned())?;
    let mut response = client
        .get(file_url)
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
        return Err(format!(
            "the downloaded artifact does not match the approved digest (expected {expected}, found {observed})"
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
}

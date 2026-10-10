//! Pre-activation review: what a human confirms before a Plugin is installed.

#[allow(dead_code)]
mod support;

use std::{fs, os::unix::fs::PermissionsExt, path::Path};

use anchor_graph_host::PluginCatalog;
use anchor_library::{InstallError, Library};
use serde_json::json;
use support::{FixtureCheckout, fixture, request, write};

/// A candidate that declares everything the review has to surface.
fn declared_fixture(root: &Path) -> std::path::PathBuf {
    let directory = root.join("candidate");
    write(
        directory.join("plugin.json"),
        json!({
            "name": "Declared",
            "description": "review fixture",
            "skills": "skills",
            "mcpServers": {
                "local": {
                    "command": "bin/run",
                    "args": ["serve"],
                    "env_vars": ["CANDIDATE_TOKEN", "CANDIDATE_REGION"],
                    "optional_env_vars": ["CANDIDATE_OPTIONAL"]
                },
                "remote": {
                    "url": "https://example.test/mcp",
                    "auth": "oauth"
                },
                "bearer": {
                    "url": "https://example.test/bearer",
                    "bearer_token_env_var": "CANDIDATE_BEARER"
                },
                "off": {"enabled": false, "url": "https://example.test/off"}
            }
        })
        .to_string(),
    );
    write(directory.join("skills/check/SKILL.md"), "# declared\n");
    write(directory.join("resources/evidence.txt"), "evidence");
    directory
}

#[test]
fn review_describes_declarations_and_never_touches_the_library() {
    let temporary = tempfile::tempdir().unwrap();
    let library_root = temporary.path().join("library");
    let candidate = declared_fixture(temporary.path());
    let review = Library::new(&library_root)
        .review_directory("declared", &candidate)
        .unwrap();

    assert_eq!(review.id, "declared");
    assert_eq!(review.name, "Declared");
    assert_eq!(review.description, "review fixture");
    assert_eq!(review.skills, ["skills/check/SKILL.md"]);
    assert_eq!(
        review.resources,
        [
            "plugin.json",
            "resources/evidence.txt",
            "skills/check/SKILL.md"
        ]
    );
    assert!(review.resource_bytes > 0);

    let local = review
        .servers
        .iter()
        .find(|server| server.name == "local")
        .unwrap();
    assert_eq!(local.transport, "stdio");
    assert_eq!(local.command.as_deref(), Some("bin/run"));
    assert_eq!(local.args, ["serve"]);
    assert_eq!(local.environment, ["CANDIDATE_REGION", "CANDIDATE_TOKEN"]);
    assert_eq!(local.optional_environment, ["CANDIDATE_OPTIONAL"]);

    let remote = review
        .servers
        .iter()
        .find(|server| server.name == "remote")
        .unwrap();
    assert_eq!(remote.transport, "http");
    assert_eq!(remote.url.as_deref(), Some("https://example.test/mcp"));
    assert_eq!(remote.auth.as_deref(), Some("oauth"));
    assert!(remote.environment.is_empty());

    let bearer = review
        .servers
        .iter()
        .find(|server| server.name == "bearer")
        .unwrap();
    assert_eq!(bearer.auth, None);
    assert_eq!(bearer.environment, ["CANDIDATE_BEARER"]);

    let off = review.servers.iter().find(|server| server.name == "off");
    // A server the manifest disables is dropped by the catalog, so the review
    // describes the surface that can actually be launched.
    assert!(off.is_none(), "{:?}", review.servers);

    // The credential *names* are reported; no value can appear.
    let encoded = serde_json::to_string(&review).unwrap();
    assert!(!encoded.contains("CANDIDATE_TOKEN="));
    // Nothing was published and no Library tree was created.
    assert!(!library_root.exists());
}

#[test]
fn review_digest_is_the_digest_the_installation_publishes() {
    let temporary = tempfile::tempdir().unwrap();
    let library_root = temporary.path().join("library");
    let candidate = declared_fixture(temporary.path());
    let library = Library::new(&library_root);

    let review = library.review_directory("declared", &candidate).unwrap();
    let installed = library
        .install_directory("declared", &candidate, false)
        .unwrap();

    assert_eq!(review.digest, installed.digest);
    let binding = anchor_graph_host::FilePluginCatalog::new(&library_root)
        .resolve(&["declared".to_owned()])
        .unwrap()
        .remove(0);
    assert_eq!(binding.digest, review.digest);
    assert_eq!(binding.resources, review.resources);
}

#[test]
fn review_reports_the_github_source_without_installing_it() {
    let temporary = tempfile::tempdir().unwrap();
    let library_root = temporary.path().join("library");
    let review = Library::new(&library_root)
        .review_with_checkout(&request(None, false), &FixtureCheckout)
        .unwrap();

    assert_eq!(review.id, "demo");
    assert_eq!(review.name, "Checkout fixture");
    assert_eq!(review.resources, ["plugin.json", "skills/check/SKILL.md"]);
    assert!(!library_root.exists());
}

#[test]
fn review_flags_literal_values_that_would_ship_inside_the_plugin() {
    let temporary = tempfile::tempdir().unwrap();
    let candidate = temporary.path().join("candidate");
    write(
        candidate.join("plugin.json"),
        json!({
            "name": "Literal",
            "mcpServers": {
                "local": {"command": "bin/run", "env": {"TOKEN": "top-secret"}},
                "remote": {
                    "url": "https://example.test/mcp",
                    "headers": {"Authorization": "Bearer literal"}
                }
            }
        })
        .to_string(),
    );
    let review = Library::new(temporary.path().join("library"))
        .review_directory("literal", &candidate)
        .unwrap();

    assert_eq!(
        review.inline_values,
        ["local:env.TOKEN", "remote:headers.Authorization"]
    );
    assert!(
        review
            .warnings
            .iter()
            .any(|warning| warning.contains("literal MCP values")),
        "{:?}",
        review.warnings
    );
}

#[test]
fn review_separates_readable_sources_from_unreadable_compiled_output() {
    let temporary = tempfile::tempdir().unwrap();
    let library = Library::new(temporary.path().join("library"));

    let bare = temporary.path().join("bare");
    write(
        bare.join("plugin.json"),
        json!({"name": "Bare", "mcpServers": {"local": {"command": "bin/run"}}}).to_string(),
    );
    write(bare.join("bin/run"), b"\x7fELFbinary");
    fs::set_permissions(bare.join("bin/run"), fs::Permissions::from_mode(0o755)).unwrap();
    let review = library.review_directory("bare", &bare).unwrap();
    assert_eq!(review.binaries, ["bin/run"]);
    assert!(review.sources.is_empty());
    assert!(review.binary_without_source);
    assert!(
        review
            .warnings
            .iter()
            .any(|warning| warning.contains("compiled binaries")),
        "{:?}",
        review.warnings
    );

    let sourced = temporary.path().join("sourced");
    write(
        sourced.join("plugin.json"),
        json!({"name": "Sourced", "mcpServers": {"local": {"command": "bin/run"}}}).to_string(),
    );
    write(sourced.join("bin/run"), b"\x7fELFbinary");
    fs::set_permissions(sourced.join("bin/run"), fs::Permissions::from_mode(0o755)).unwrap();
    write(
        sourced.join("Cargo.toml"),
        "[package]\nname = \"sourced\"\n",
    );
    write(sourced.join("src/main.rs"), "fn main() {}\n");
    let review = library.review_directory("sourced", &sourced).unwrap();
    assert_eq!(review.binaries, ["bin/run"]);
    assert_eq!(review.sources, ["Cargo.toml", "src/main.rs"]);
    assert!(!review.binary_without_source);
}

#[test]
fn review_rejects_a_candidate_without_a_manifest_and_explains_why() {
    let temporary = tempfile::tempdir().unwrap();
    let candidate = temporary.path().join("candidate");
    write(candidate.join("skills/check/SKILL.md"), "# no manifest\n");
    let error = Library::new(temporary.path().join("library"))
        .review_directory("candidate", &candidate)
        .unwrap_err();
    assert!(matches!(error, InstallError::MissingManifest), "{error:?}");
}

#[test]
fn review_reports_the_catalog_reason_for_an_invalid_manifest() {
    let temporary = tempfile::tempdir().unwrap();
    let candidate = temporary.path().join("candidate");
    write(
        candidate.join("plugin.json"),
        json!({
            "name": "Invalid",
            "mcpServers": {"probe": {"command": "bin/run", "dependencyFile": "requirements.txt"}}
        })
        .to_string(),
    );
    let error = Library::new(temporary.path().join("library"))
        .review_directory("invalid", &candidate)
        .unwrap_err();
    match error {
        InstallError::InvalidPluginReason(reason) => {
            assert!(reason.contains("unsupported MCP field"), "{reason}");
        }
        other => panic!("expected a catalog reason, got {other:?}"),
    }
}

#[test]
fn review_ignores_git_history_and_flattens_a_hidden_manifest() {
    let temporary = tempfile::tempdir().unwrap();
    let candidate = fixture(temporary.path(), "source", true);
    write(candidate.join(".git/config"), "[core]\n");
    let review = Library::new(temporary.path().join("library"))
        .review_directory("source", &candidate)
        .unwrap();

    assert_eq!(
        review.resources,
        [
            "plugin.json",
            "resources/evidence.txt",
            "skills/check/SKILL.md"
        ]
    );
    let installed = Library::new(temporary.path().join("library"))
        .install_directory("source", &candidate, false)
        .unwrap();
    assert_eq!(review.digest, installed.digest);
}

#[test]
fn cli_review_prints_the_review_and_installs_nothing() {
    let temporary = tempfile::tempdir().unwrap();
    let library_root = temporary.path().join("library");
    let candidate = declared_fixture(temporary.path());
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_anchor-library"))
        .arg("--root")
        .arg(&library_root)
        .args(["install", "--directory"])
        .arg(&candidate)
        .args(["--id", "declared", "--review"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let review: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(review["id"], "declared");
    assert!(
        review["servers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|server| server["name"] == "local"),
        "{review}"
    );
    assert!(!library_root.exists());
}

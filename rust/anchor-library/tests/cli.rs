//! The operator CLI is the second installation door.
//!
//! It has no host that could solve a Plugin's declared dependencies or land
//! their view, so it refuses such a Plugin instead of publishing one whose tools
//! can never start. Reviewing it stays available.

use std::process::Command;

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_anchor-library"))
}

fn dependent_candidate(root: &std::path::Path) -> std::path::PathBuf {
    let candidate = root.join("candidate");
    std::fs::create_dir_all(&candidate).unwrap();
    std::fs::write(
        candidate.join("plugin.json"),
        r#"{"name":"Dependent","description":"declares one package","dependencies":{"python":["idna==3.6"]}}"#,
    )
    .unwrap();
    candidate
}

#[test]
fn the_cli_refuses_to_install_a_plugin_that_declares_dependencies() {
    let root = tempfile::tempdir().unwrap();
    let candidate = dependent_candidate(root.path());
    let library = root.path().join("library");
    let output = cli()
        .args([
            "--root",
            library.to_str().unwrap(),
            "install",
            "--directory",
            candidate.to_str().unwrap(),
            "--id",
            "dependent",
        ])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "the CLI must refuse what it cannot complete"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("run-time dependencies"), "{stderr}");
    assert!(stderr.contains("idna==3.6"), "{stderr}");
    assert!(
        !library.join("plugins/dependent").exists(),
        "a refused Plugin must not be published"
    );
}

#[test]
fn the_cli_still_reviews_the_same_candidate() {
    let root = tempfile::tempdir().unwrap();
    let candidate = dependent_candidate(root.path());
    let library = root.path().join("library");
    let output = cli()
        .args([
            "--root",
            library.to_str().unwrap(),
            "install",
            "--directory",
            candidate.to_str().unwrap(),
            "--id",
            "dependent",
            "--review",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "a review publishes nothing and must still describe the candidate: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reviewed: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("a review prints JSON");
    assert_eq!(reviewed["id"], "dependent");
    assert_eq!(reviewed["dependencies"]["python"][0], "idna==3.6");
    assert!(!library.exists() || !library.join("plugins/dependent").exists());
}

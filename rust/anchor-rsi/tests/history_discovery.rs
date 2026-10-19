use serde_json::Value;
use std::{fs, process::Command};

#[test]
fn operator_discovery_emits_reusable_node_scoped_grants_without_reading_credentials() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("another-user");
    for (directory, history) in [
        (".codex", "sessions"),
        (".claude", "projects"),
        (".dsh", "sessions"),
    ] {
        fs::create_dir_all(home.join(directory).join(history)).unwrap();
        fs::write(
            home.join(directory).join("credentials"),
            "fixture-credential-must-not-be-read-or-granted",
        )
        .unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_anchor-rsi"))
        .arg("weekly-discover")
        .env_clear()
        .env("HOME", &home)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    for (source, directory, history) in [
        ("codex", ".codex", "sessions"),
        ("claude", ".claude", "projects"),
        ("deepseek", ".dsh", "sessions"),
    ] {
        assert_eq!(
            report["local_inputs"]["collect"][source],
            home.join(directory).join(history).to_str().unwrap()
        );
    }
    assert_eq!(report["local_inputs"].as_object().unwrap().len(), 3);
    let executable = fs::canonicalize(env!("CARGO_BIN_EXE_anchor-rsi")).unwrap();
    assert_eq!(
        report["local_inputs"]["collect"]["rsi"],
        executable.to_str().unwrap()
    );
    for node in ["gate", "publish"] {
        assert_eq!(report["local_inputs"][node].as_object().unwrap().len(), 1);
        assert_eq!(
            report["local_inputs"][node]["rsi"],
            executable.to_str().unwrap()
        );
    }
    assert!(!String::from_utf8_lossy(&output.stdout).contains("fixture-credential"));
    assert!(!root.path().join("local-inputs.json").exists());
}

#[test]
fn operator_discovery_succeeds_with_explicit_gaps_on_a_machine_without_histories() {
    let root = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_anchor-rsi"))
        .arg("weekly-discover")
        .env_clear()
        .env("HOME", root.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["local_inputs"]["collect"].as_object().unwrap().len(),
        1
    );
    for source in report["sources"].as_array().unwrap() {
        assert_eq!(source["status"], "unavailable");
        assert!(!source["reason"].as_str().unwrap().is_empty());
    }
}

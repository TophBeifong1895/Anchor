use std::{fs, process::Command};

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_anchor-platform"));
    command.env_clear().env("PATH", "/usr/bin:/bin");
    command
}

#[test]
fn native_platform_cli_exposes_install_and_start_without_runtime_grant_flags() {
    let result = command().arg("--help").output().unwrap();
    assert!(result.status.success());
    let help = String::from_utf8(result.stdout).unwrap();
    assert!(help.contains("install"));
    assert!(help.contains("start"));
    let result = command().args(["install", "--help"]).output().unwrap();
    assert!(result.status.success());
    let help = String::from_utf8(result.stdout).unwrap();
    for flag in ["--archive", "--sha256", "--install-root", "--model-config"] {
        assert!(help.contains(flag));
    }
    assert!(!help.contains("--python"));
    assert!(!help.contains("--network"));
}

#[test]
fn native_platform_cli_does_not_evaluate_or_print_model_config() {
    let root = tempfile::tempdir().unwrap();
    let sentinel = root.path().join("sentinel");
    let model = root.path().join("model.env");
    fs::write(&model, format!(
        "ANCHOR_MODEL_URL=https://provider.example/v1\nANCHOR_MODEL_NAME=fixture\nANCHOR_MODEL_API_KEY='$(touch {})'\n",
        sentinel.display()
    )).unwrap();
    let install_root = root.path().join("installation");
    let result = command()
        .args(["install", "--archive", "/unavailable/archive", "--sha256"])
        .arg("0".repeat(64))
        .arg("--install-root")
        .arg(&install_root)
        .arg("--model-config")
        .arg(&model)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&result.stderr).contains("touch"));
    assert!(!sentinel.exists());
    assert!(!install_root.exists());
}

#[test]
fn native_launcher_rejects_start_outside_a_private_installation() {
    let result = command().arg("start").output().unwrap();
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(String::from_utf8_lossy(&result.stderr).contains("launcher must"));
}

#[test]
fn native_platform_cli_accepts_install_without_model_config() {
    let root = tempfile::tempdir().unwrap();
    let result = command()
        .args(["install", "--archive", "/unavailable/archive", "--sha256"])
        .arg("0".repeat(64))
        .arg("--install-root")
        .arg(root.path().join("installation"))
        .output()
        .unwrap();
    assert!(!result.status.success());
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(!error.contains("required arguments"), "{error}");
    assert!(!error.contains("--model-config"), "{error}");
    assert!(result.stdout.is_empty());
}

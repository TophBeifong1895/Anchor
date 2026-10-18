use std::{fs, net::TcpListener, process::Command};

#[test]
fn development_launcher_ignores_shell_configuration_files() {
    let root = tempfile::tempdir().unwrap();
    let scripts = root.path().join("scripts");
    fs::create_dir(&scripts).unwrap();
    let script = scripts.join("dev.sh");
    fs::write(&script, include_bytes!("../../../scripts/dev.sh")).unwrap();
    let sentinel = root.path().join("sourced-env");
    fs::write(
        root.path().join(".env"),
        format!("touch '{}'\nif\n", sentinel.display()),
    )
    .unwrap();
    let api = TcpListener::bind("127.0.0.1:0").unwrap();
    let web = TcpListener::bind("127.0.0.1:0").unwrap();
    let output = Command::new("bash")
        .arg(&script)
        .arg("status")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env(
            "ANCHOR_RUNNER_LISTEN",
            api.local_addr().unwrap().to_string(),
        )
        .env(
            "ANCHOR_WEB_PORT",
            web.local_addr().unwrap().port().to_string(),
        )
        .env("ANCHOR_DEV_ENV_FILE", root.path().join("missing.env"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(!sentinel.exists());
    assert!(root.path().join(".env").is_file());
    assert!(root.path().join(".local/rust/dev/lifecycle.lock").is_file());
}

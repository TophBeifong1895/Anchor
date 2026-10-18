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

fn executable(path: &std::path::Path, contents: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

struct SourceCheckout {
    root: tempfile::TempDir,
    tools: std::path::PathBuf,
}

impl SourceCheckout {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let tools = root.path().join("tools");
        fs::create_dir_all(&tools).unwrap();
        fs::create_dir_all(root.path().join("scripts")).unwrap();
        fs::create_dir_all(root.path().join("apps/web")).unwrap();
        fs::write(
            root.path().join("scripts/dev.sh"),
            include_bytes!("../../../scripts/dev.sh"),
        )
        .unwrap();
        fs::write(root.path().join("apps/web/package.json"), "{}").unwrap();
        fs::write(root.path().join("apps/web/package-lock.json"), "{}").unwrap();
        executable(&tools.join("bwrap"), "#!/bin/sh\nexit 0\n");
        executable(
            &tools.join("cargo"),
            r#"#!/bin/sh
printf 'cargo %s\n' "$*" >> "$CHECKOUT/commands"
mkdir -p "$CARGO_TARGET_DIR/debug"
for tool in anchor-runner-host anchor-net-relay; do
  printf '#!/bin/sh\nexit 0\n' > "$CARGO_TARGET_DIR/debug/$tool"
  chmod +x "$CARGO_TARGET_DIR/debug/$tool"
done
"#,
        );
        executable(
            &tools.join("npm"),
            r#"#!/bin/sh
printf 'npm %s\n' "$*" >> "$CHECKOUT/commands"
case "$3" in
  ci)
    mkdir -p "$2/node_modules/.bin"
    for tool in vite tsc; do
      printf '#!/bin/sh\nexit 0\n' > "$2/node_modules/.bin/$tool"
      chmod +x "$2/node_modules/.bin/$tool"
    done ;;
  run) mkdir -p "$2/dist"; printf built > "$2/dist/index.html" ;;
esac
"#,
        );
        // Exercise launcher orchestration without downloading a model runtime.
        // The actual builder's pin/error behavior is covered below separately.
        executable(
            &root.path().join("scripts/build-goose-acp.sh"),
            r#"#!/bin/sh
[ "$1" = --ensure ] && [ "$2" = --output ] || exit 80
[ -x "$3" ] && exit 0
printf 'goose build\n' >> "$CHECKOUT/commands"
mkdir -p "$(dirname "$3")"
printf '#!/bin/sh\nexit 0\n' > "$3"
chmod +x "$3"
"#,
        );
        Self { root, tools }
    }

    fn run(&self, action: &str, overrides: &[(&str, &std::path::Path)]) -> std::process::Output {
        let mut command = Command::new("bash");
        command
            .arg(self.root.path().join("scripts/dev.sh"))
            .arg(action)
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", self.tools.display()))
            .env("CHECKOUT", self.root.path());
        for (name, value) in overrides {
            command.env(name, value);
        }
        command.output().unwrap()
    }

    fn log(&self) -> String {
        fs::read_to_string(self.root.path().join("commands")).unwrap_or_default()
    }
}

#[test]
fn fresh_source_setup_prepares_web_host_relay_and_goose_and_reuses_them() {
    let checkout = SourceCheckout::new();
    // A leftover release output must not hide a missing development relay.
    fs::create_dir_all(checkout.root.path().join("rust/target/release")).unwrap();
    executable(
        &checkout
            .root
            .path()
            .join("rust/target/release/anchor-runner-host"),
        "#!/bin/sh\nexit 77\n",
    );
    let output = checkout.run("setup", &[]);
    assert!(output.status.success(), "{output:?}");
    let first = checkout.log();
    assert!(first.contains("npm --prefix"));
    assert!(first.contains(" ci\n"));
    assert!(first.contains("-p anchor-runner-host --bins --locked"));
    assert!(first.contains("goose build"));
    assert!(first.contains("run build"));
    assert!(checkout.root.path().join(".local/bin/goose").is_file());
    assert!(
        !checkout
            .root
            .path()
            .join(".local/rust/dev/native-host.pid")
            .exists()
    );
    let output = checkout.run("setup", &[]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        checkout.log(),
        first,
        "repeated setup must not rebuild or reinstall"
    );
    // A missing relay is repaired even when the Host already exists.
    fs::remove_file(
        checkout
            .root
            .path()
            .join("rust/target/debug/anchor-net-relay"),
    )
    .unwrap();
    let output = checkout.run("setup", &[]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(checkout.log().matches("cargo ").count(), 2);
    // Updating the dependency lock invalidates the install stamp.
    fs::write(
        checkout.root.path().join("apps/web/package-lock.json"),
        "{\"updated\":true}",
    )
    .unwrap();
    let output = checkout.run("setup", &[]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(checkout.log().matches(" ci\n").count(), 2);
}

#[test]
fn source_setup_reports_namespace_failure_before_builds_or_downloads() {
    let checkout = SourceCheckout::new();
    executable(&checkout.tools.join("bwrap"), "#!/bin/sh\nexit 1\n");
    let output = checkout.run("setup", &[]);
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("namespace"));
    assert!(checkout.log().is_empty());
}

#[test]
fn source_setup_does_not_replace_missing_explicit_runtime_paths() {
    let checkout = SourceCheckout::new();
    let missing = checkout.root.path().join("custom-host");
    let output = checkout.run("setup", &[("ANCHOR_RUNNER_BINARY", &missing)]);
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("Explicit Host/relay paths"));
    assert!(!checkout.log().contains("cargo "));
    assert!(!missing.exists());
}

#[test]
fn source_status_and_invalid_action_do_not_prepare_dependencies() {
    let checkout = SourceCheckout::new();
    assert!(checkout.run("status", &[]).status.success());
    let invalid = checkout.run("unknown", &[]);
    assert_eq!(invalid.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("setup"));
    assert!(checkout.log().is_empty());
}

#[test]
fn ensure_goose_rejects_wrong_existing_output_without_overwriting_it() {
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("build-goose-acp.sh");
    fs::write(
        &script,
        include_bytes!("../../../scripts/build-goose-acp.sh"),
    )
    .unwrap();
    let output_path = root.path().join("goose");
    let contents = "#!/bin/sh\nexit 0\n";
    executable(&output_path, contents);
    let output = Command::new("bash")
        .arg(&script)
        .args(["--ensure", "--output"])
        .arg(&output_path)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not match the pinned runtime"));
    assert_eq!(fs::read_to_string(output_path).unwrap(), contents);
    let output = Command::new("bash")
        .arg(script)
        .arg("--ensure")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--ensure requires --output"));
}

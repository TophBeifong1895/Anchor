use super::*;
use std::sync::atomic::Ordering;

fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/status")).is_ok_and(|status| {
        !status
            .lines()
            .any(|line| line.starts_with("State:") && line.split_whitespace().nth(1) == Some("Z"))
    })
}

#[tokio::test]
async fn resolver_command_timeout_and_cancellation_reap_the_child_and_its_process_group() {
    for mode in ["timeout", "cancel"] {
        let root = tempfile::tempdir().unwrap();
        let pids = root.path().join("pids");
        let mut command = tokio::process::Command::new("/bin/sh");
        command
            .args([
                "-c",
                "sleep 30 & printf '%s\\n%s\\n' $$ $! > \"$1\"; wait",
                "resolver-fixture",
            ])
            .arg(&pids)
            .env_clear()
            .env("PATH", "/usr/bin:/bin");
        let access = super::super::DependencyAccess::default();
        let control = async {
            tokio::time::timeout(Duration::from_secs(2), async {
                while !pids.is_file() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("resolver process did not start");
            if mode == "cancel" {
                access.cancellation.store(true, Ordering::Release);
            }
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(
                command_output(
                    &mut command,
                    Duration::from_millis(500),
                    &access,
                    "resolver fixture"
                ),
                control
            )
        })
        .await
        .expect("resolver timeout/cancellation must be bounded");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains(if mode == "cancel" {
                    "cancelled"
                } else {
                    "timed out"
                })
        );
        let ids: Vec<u32> = std::fs::read_to_string(&pids)
            .unwrap()
            .lines()
            .map(|pid| pid.parse().unwrap())
            .collect();
        assert_eq!(ids.len(), 2);
        assert!(
            !std::path::Path::new(&format!("/proc/{}/status", ids[0])).exists(),
            "resolver child must be reaped"
        );
        tokio::time::timeout(Duration::from_secs(1), async {
            while alive(ids[1]) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("resolver descendant was left running");
        let mut next = tokio::process::Command::new("/bin/sh");
        next.args(["-c", "printf resolver-retry-ok"]);
        let result = command_output(
            &mut next,
            Duration::from_secs(1),
            &super::super::DependencyAccess::default(),
            "resolver retry",
        )
        .await
        .unwrap();
        assert!(result.status.success());
        assert_eq!(result.stdout, b"resolver-retry-ok");
    }
}

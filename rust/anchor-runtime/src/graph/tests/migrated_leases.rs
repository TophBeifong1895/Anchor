#[test]
fn file_run_lease_provider_read_leases_coexist_and_each_blocks_writers() {
    let root = std::env::temp_dir().join(format!(
        "anchor-run-lease-read-leases-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let store = FileRunLeaseProvider::new(&root);
    let other_store = FileRunLeaseProvider::new(&root);
    let run_id = "shared-run";

    for release_first in [true, false] {
        let first = store.acquire_read_lease(run_id).unwrap();
        let second = other_store.acquire_read_lease(run_id).unwrap();
        assert!(matches!(
            store.acquire_lease(run_id),
            Err(GraphError::RunBusy(id)) if id == run_id
        ));

        let remaining = if release_first {
            drop(first);
            second
        } else {
            drop(second);
            first
        };
        assert!(matches!(
            other_store.acquire_lease(run_id),
            Err(GraphError::RunBusy(id)) if id == run_id
        ));
        drop(remaining);

        let writer = store.acquire_lease(run_id).unwrap();
        assert!(matches!(
            other_store.acquire_read_lease(run_id),
            Err(GraphError::RunBusy(id)) if id == run_id
        ));
        assert!(matches!(
            other_store.acquire_lease(run_id),
            Err(GraphError::RunBusy(id)) if id == run_id
        ));
        drop(writer);
    }

    drop(other_store.acquire_read_lease(run_id).unwrap());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn file_run_lease_provider_read_and_write_leases_preserve_lock_identity() {
    let root = std::env::temp_dir().join(format!(
        "anchor-run-lease-lock-identity-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let store = FileRunLeaseProvider::new(&root);
    let run_id = "shared-run";
    let reader = store.acquire_read_lease(run_id).unwrap();
    let lock_path = root.join(format!(".{run_id}.lock"));
    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    drop(reader);
    assert!(lock_path.is_file());

    lock_file.try_lock().unwrap();
    assert!(matches!(
        store.acquire_read_lease(run_id),
        Err(GraphError::RunBusy(id)) if id == run_id
    ));
    assert!(matches!(
        store.acquire_lease(run_id),
        Err(GraphError::RunBusy(id)) if id == run_id
    ));
    lock_file.unlock().unwrap();

    drop(store.acquire_lease(run_id).unwrap());
    assert!(lock_path.is_file());
    lock_file.try_lock_shared().unwrap();
    assert!(matches!(
        store.acquire_lease(run_id),
        Err(GraphError::RunBusy(id)) if id == run_id
    ));
    drop(store.acquire_read_lease(run_id).unwrap());
    assert!(matches!(
        store.acquire_lease(run_id),
        Err(GraphError::RunBusy(id)) if id == run_id
    ));
    lock_file.unlock().unwrap();

    drop(store.acquire_lease(run_id).unwrap());
    drop(lock_file);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn file_run_lease_provider_read_and_write_leases_reject_invalid_run_ids() {
    let root = std::env::temp_dir().join(format!(
        "anchor-run-lease-invalid-lease-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let store = FileRunLeaseProvider::new(&root);

    for run_id in [
        "",
        ".",
        "..",
        "../escape",
        "nested/run",
        r"nested\run",
        "/absolute",
        "run\0id",
        "run id",
    ] {
        assert!(matches!(
            store.acquire_read_lease(run_id),
            Err(GraphError::InvalidRunId(id)) if id == run_id
        ));
        assert!(matches!(
            store.acquire_lease(run_id),
            Err(GraphError::InvalidRunId(id)) if id == run_id
        ));
        assert!(!root.exists());
    }
}

#[test]
fn kernel_file_store_rejects_corruption_and_run_lease_releases_after_process_crash() {
    let root = std::env::temp_dir().join(format!(
        "anchor-run-lease-crash-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let store = FileRunLeaseProvider::new(&root);
    let record = GraphRunRecord::create(graph(&["one"], &[], "one"), Value::Null).unwrap();
    let kernel_store = FileStore::new(root.join("kernel")).unwrap();
    let resource = validator::run_resource(&record, 100).unwrap();
    kernel_store.create(&resource).unwrap();
    assert_eq!(
        kernel_store.load(&resource.id).unwrap(),
        Some(resource.clone())
    );
    let lease = store.acquire_lease(&record.run_id).unwrap();
    assert!(matches!(
        store.acquire_lease(&record.run_id),
        Err(GraphError::RunBusy(_))
    ));
    drop(lease);
    let reopened = store.acquire_lease(&record.run_id).unwrap();
    drop(reopened);

    // Hold the lease in another test process and synchronize using pipes:
    // the child announces only after acquiring the OS lock, and exits via
    // process::exit so Rust destructors cannot release it explicitly.
    let token = format!(
        "{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        RUN_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let executable = std::env::current_exe().unwrap();
    let mut child = Command::new(&executable)
        .args([
            "--exact",
            "graph::tests::file_run_lease_process_helper",
            "--nocapture",
        ])
        .env("ANCHOR_FILE_LEASE_TEST_TOKEN", &token)
        .env("ANCHOR_FILE_LEASE_TEST_MODE", "holder")
        .env("ANCHOR_FILE_LEASE_TEST_ROOT", &root)
        .env("ANCHOR_FILE_LEASE_TEST_RUN", &record.run_id)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let child_stdout = child.stdout.take().unwrap();
    let mut child_output = io::BufReader::new(child_stdout);
    let marker = format!("ANCHOR_FILE_LEASE_HELD:{token}");
    let mut line = String::new();
    let mut acquired = false;
    loop {
        line.clear();
        let bytes = child_output.read_line(&mut line).unwrap();
        if bytes == 0 {
            break;
        }
        if line.trim_end() == marker {
            acquired = true;
            break;
        }
    }
    if !acquired {
        let output = child.wait_with_output().unwrap();
        panic!(
            "lease holder exited before announcing acquisition; status={}, stdout={}, stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let contender = Command::new(&executable)
        .args([
            "--exact",
            "graph::tests::file_run_lease_process_helper",
            "--nocapture",
        ])
        .env("ANCHOR_FILE_LEASE_TEST_TOKEN", &token)
        .env("ANCHOR_FILE_LEASE_TEST_MODE", "contender")
        .env("ANCHOR_FILE_LEASE_TEST_ROOT", &root)
        .env("ANCHOR_FILE_LEASE_TEST_RUN", &record.run_id)
        .output();
    let release_result = writeln!(child.stdin.as_mut().unwrap(), "exit-crash:{token}");
    drop(child.stdin.take());
    let child_status = child.wait().unwrap();
    release_result.unwrap();
    assert!(
        child_status.success(),
        "lease holder exited unsuccessfully: {child_status}"
    );

    let contender = contender.unwrap();
    assert!(
        contender.status.success(),
        "lease contender failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&contender.stdout),
        String::from_utf8_lossy(&contender.stderr)
    );
    let contender_output = String::from_utf8_lossy(&contender.stdout);
    assert!(
        contender_output.contains(&format!("ANCHOR_FILE_LEASE_BUSY:{token}")),
        "contending process did not observe RunBusy: {contender_output}"
    );

    let after_crash = store.acquire_lease(&record.run_id).unwrap();
    drop(after_crash);

    let resource_path = fs::read_dir(root.join("kernel"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.join("resource.json").is_file())
        .unwrap()
        .join("resource.json");
    fs::write(resource_path, b"not json").unwrap();
    assert!(kernel_store.load(&resource.id).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn file_run_lease_process_helper() {
    // This test is intentionally inert when run by the normal harness. The
    // parent test supplies a per-invocation token and a private temp root.
    let (Ok(token), Ok(mode), Ok(root), Ok(run_id)) = (
        std::env::var("ANCHOR_FILE_LEASE_TEST_TOKEN"),
        std::env::var("ANCHOR_FILE_LEASE_TEST_MODE"),
        std::env::var("ANCHOR_FILE_LEASE_TEST_ROOT"),
        std::env::var("ANCHOR_FILE_LEASE_TEST_RUN"),
    ) else {
        return;
    };
    if token.is_empty()
        || !["holder", "contender"].contains(&mode.as_str())
        || !PathBuf::from(&root)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("anchor-run-lease-crash-"))
    {
        return;
    }

    let store = FileRunLeaseProvider::new(root);
    if mode == "contender" {
        match store.acquire_lease(&run_id) {
            Err(GraphError::RunBusy(_)) => println!("ANCHOR_FILE_LEASE_BUSY:{token}"),
            Ok(lease) => {
                drop(lease);
                println!("ANCHOR_FILE_LEASE_ACQUIRED:{token}");
            }
            Err(error) => panic!("contender failed to check lease: {error}"),
        }
        return;
    }

    let _lease = store.acquire_lease(&run_id).unwrap();
    println!("ANCHOR_FILE_LEASE_HELD:{token}");
    io::stdout().flush().unwrap();
    let mut command = String::new();
    io::stdin().read_line(&mut command).unwrap();
    assert_eq!(command.trim_end(), format!("exit-crash:{token}"));
    // Deliberately bypass `drop(lease)`: process termination must release
    // the kernel lock while leaving its stable lock file/inode in place.
    std::process::exit(0);
    #[allow(unreachable_code)]
    drop(_lease);
}

use anchor_runtime::{NetworkPolicy, SandboxPort, SandboxRequest};
use anchor_sandbox_bwrap::{BubblewrapPolicy, BubblewrapSandbox};
use std::{
    net::{IpAddr, UdpSocket},
    path::PathBuf,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn workspace() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "anchor-network-boundary-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    root
}

async fn service(listener: TcpListener) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut bytes = [0; 4096];
                let _ = stream.read(&mut bytes).await;
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\nConnection: close\r\n\r\nbusiness-egress").await.unwrap();
            });
        }
    })
}

#[tokio::test]
async fn online_commands_use_egress_without_reaching_loopback_management() {
    assert!(std::path::Path::new(env!("CARGO_BIN_EXE_anchor-sandbox-network")).is_file());
    let route = UdpSocket::bind("0.0.0.0:0").unwrap();
    route.connect("192.0.2.1:9").unwrap();
    let IpAddr::V4(address) = route.local_addr().unwrap().ip() else {
        panic!("IPv4 host is required")
    };
    let business = TcpListener::bind((address, 0)).await.unwrap();
    let business_port = business.local_addr().unwrap().port();
    let business = service(business).await;
    let management = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let management_port = management.local_addr().unwrap().port();
    let root = workspace();
    let sandbox = BubblewrapSandbox::new(
        BubblewrapPolicy::new("bwrap", ["sh"])
            .authorize_workspace_root(&root)
            .allow_network(),
    )
    .unwrap();
    let script = format!(
        "set -e; curl --noproxy '*' --connect-timeout 5 --max-time 10 -fsS http://{address}:{business_port}/; for host in 127.0.0.1 10.0.2.2 10.0.2.3; do if curl --noproxy '*' --connect-timeout 1 --max-time 2 -fsS -X POST http://$host:{management_port}/plugins; then exit 9; fi; done; printf '\\nmanagement-isolated\\n'"
    );
    let mut request = SandboxRequest::new(&root, ["sh", "-c", &script]);
    request.network = NetworkPolicy::Enabled;
    request.timeout = std::time::Duration::from_secs(20);
    let result = sandbox.run(request.clone()).await.unwrap();
    assert_eq!(result.exit_code, Some(0), "{result:?}");
    assert!(result.stdout.contains("business-egress"));
    assert!(result.stdout.contains("management-isolated"));
    let output = sandbox
        .isolated_command(request)
        .unwrap()
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("management-isolated"));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), management.accept())
            .await
            .is_err()
    );
    business.abort();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn cancelling_an_online_command_reaps_its_network_process_tree() {
    let root = workspace();
    let sandbox = BubblewrapSandbox::new(
        BubblewrapPolicy::new("bwrap", ["sh"])
            .authorize_workspace_root(&root)
            .allow_network(),
    )
    .unwrap();
    let cancellation = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut request = SandboxRequest::new(
        &root,
        [
            "sh",
            "-c",
            "printf started > started; sleep 5; printf leaked > escaped",
        ],
    );
    request.network = NetworkPolicy::Enabled;
    request.cancellation = cancellation.clone();
    let running = tokio::spawn(async move { sandbox.run(request).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !root.join("started").is_file() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    cancellation.store(true, std::sync::atomic::Ordering::Release);
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result.status, anchor_runtime::SandboxStatus::Cancelled);
    assert!(!root.join("escaped").exists());
    std::fs::remove_dir_all(root).unwrap();
}

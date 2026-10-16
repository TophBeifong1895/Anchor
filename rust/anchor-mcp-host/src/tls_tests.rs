//! Exercise the default RMCP HTTP client in a fresh process with a real trust
//! store. Environment changes stay out of the concurrent test runner.
use super::*;
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use rustls::pki_types::PrivatePkcs8KeyDer;
use std::{path::Path, time::Duration};
use tokio_util::sync::CancellationToken;

fn ca() -> (rcgen::Certificate, Issuer<'static, KeyPair>) {
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let key = KeyPair::generate().unwrap();
    let cert = params.self_signed(&key).unwrap();
    (cert, Issuer::new(params, key))
}

async fn client_process(endpoint: &str, trust: &Path, expected: &str) {
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            "tls_tests::https_client_subprocess",
            "--nocapture",
        ])
        .env("ANCHOR_TEST_MCP_HTTPS_ENDPOINT", endpoint)
        .env("ANCHOR_TEST_MCP_HTTPS_EXPECTED", expected)
        .env("SSL_CERT_FILE", trust)
        .env_remove("SSL_CERT_DIR")
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(15), command.output())
        .await
        .expect("HTTPS MCP subprocess timed out")
        .unwrap();
    assert!(
        output.status.success(),
        "HTTPS MCP subprocess ({expected}) failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn https_mcp_uses_trusted_tls_and_rejects_untrusted_ca_and_hostname() {
    let temp = tempfile::tempdir().unwrap();
    let (root, issuer) = ca();
    let trust = temp.path().join("trusted.pem");
    std::fs::write(&trust, root.pem()).unwrap();
    let untrusted = temp.path().join("unrelated.pem");
    std::fs::write(&untrusted, ca().0.pem()).unwrap();
    let key = KeyPair::generate().unwrap();
    let cert = CertificateParams::new(vec!["localhost".into()])
        .unwrap()
        .signed_by(&key, &issuer)
        .unwrap();
    let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.der().clone()],
        PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
    )
    .unwrap();
    let cancellation = CancellationToken::new();
    let service = rmcp::transport::StreamableHttpService::new(
        || Ok(super::tests::Fixture),
        Arc::new(
            rmcp::transport::streamable_http_server::session::local::LocalSessionManager::default(),
        ),
        rmcp::transport::StreamableHttpServerConfig::default()
            .with_sse_keep_alive(None)
            .with_cancellation_token(cancellation.child_token()),
    );
    let http = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = http.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(http, axum::Router::new().nest_service("/mcp", service))
            .await
            .unwrap();
    });
    // Terminate real TLS before forwarding to the existing local MCP fixture.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
    let proxy = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(mut socket) = acceptor.accept(socket).await {
                    let mut target = tokio::net::TcpStream::connect(upstream).await.unwrap();
                    let _ = tokio::io::copy_bidirectional(&mut socket, &mut target).await;
                }
            });
        }
    });
    client_process(&format!("https://localhost:{port}/mcp"), &trust, "success").await;
    client_process(
        &format!("https://localhost:{port}/mcp"),
        &untrusted,
        "reject",
    )
    .await;
    client_process(&format!("https://127.0.0.1:{port}/mcp"), &trust, "reject").await;
    cancellation.cancel();
    proxy.abort();
    server.abort();
}

#[tokio::test]
#[ignore = "helper invoked by the HTTPS fixture with an isolated CA environment"]
async fn https_client_subprocess() {
    let endpoint = std::env::var("ANCHOR_TEST_MCP_HTTPS_ENDPOINT").expect("fixture endpoint");
    let expected = std::env::var("ANCHOR_TEST_MCP_HTTPS_EXPECTED").expect("fixture expectation");
    let result = McpHost::connect(McpServerConfig {
        server_id: "fixture-https".into(),
        transport: McpTransportConfig::StreamableHttp {
            endpoint,
            bearer_token: None,
            headers: BTreeMap::new(),
        },
    })
    .await;
    if expected == "reject" {
        assert!(
            matches!(result, Err(McpHostError::Connect(_))),
            "invalid certificate was accepted"
        );
        return;
    }
    let host = result.expect("HTTPS MCP initialize and tools/list");
    assert_eq!(
        host.tools()
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        ["echo", "outside"]
    );
    let result = host
        .call("echo", serde_json::json!({"value":"trusted HTTPS"}))
        .await
        .unwrap();
    assert!(!result.is_error);
    assert!(
        result
            .content
            .iter()
            .any(|part| part.to_string().contains("trusted HTTPS"))
    );
    let _ = host.service.cancel().await;
}

/// Opt-in deployment diagnosis: initialize and inventory only, without calling
/// a business tool. Supply credentials through the environment, never argv.
#[tokio::test]
#[ignore = "requires an explicitly supplied HTTPS MCP endpoint and credentials"]
async fn https_mcp_readonly_probe() {
    let endpoint = std::env::var("ANCHOR_MCP_PROBE_ENDPOINT").expect("probe endpoint");
    assert!(endpoint.starts_with("https://"), "HTTPS endpoint required");
    let token = std::env::var("ANCHOR_MCP_PROBE_BEARER_TOKEN").ok();
    let headers: BTreeMap<String, String> = std::env::var("ANCHOR_MCP_PROBE_HEADERS")
        .map(|value| serde_json::from_str(&value).expect("probe headers JSON"))
        .unwrap_or_default();
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        McpHost::connect(McpServerConfig {
            server_id: "readonly-probe".into(),
            transport: McpTransportConfig::StreamableHttp {
                endpoint,
                bearer_token: token.clone().map(Secret::new),
                headers: headers.clone(),
            },
        }),
    )
    .await
    .expect("MCP readonly probe timed out");
    match result {
        Ok(host) => {
            assert!(
                std::env::var("ANCHOR_MCP_PROBE_EXPECT_ERROR").is_err(),
                "expected a connection error"
            );
            println!(
                "MCP initialize + tools/list succeeded ({} tools)",
                host.tools().len()
            );
            let _ = host.service.cancel().await;
        }
        Err(error) => {
            let mut safe = error.to_string();
            for secret in token
                .iter()
                .chain(headers.values())
                .filter(|secret| !secret.is_empty())
            {
                safe = safe.replace(secret, "[redacted]");
            }
            println!("MCP readonly probe: {safe}");
            let expected =
                std::env::var("ANCHOR_MCP_PROBE_EXPECT_ERROR").expect("MCP readonly probe failed");
            assert!(
                safe.contains(&expected),
                "probe error differed from expectation"
            );
        }
    }
}

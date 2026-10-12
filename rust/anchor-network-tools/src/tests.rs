use super::*;
use axum::{
    Router,
    routing::{get, post},
};

#[tokio::test]
async fn requests_preserve_method_body_and_non_success_status_and_limit_response_bytes() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route(
                    "/echo",
                    post(|body: String| async move { (axum::http::StatusCode::CONFLICT, body) }),
                )
                .route("/large", get(|| async { "abcdef" })),
        )
        .await
        .unwrap();
    });
    let workspace = tempfile::tempdir().unwrap();
    let server = NetworkServer::new(workspace.path()).unwrap();
    let request = serde_json::from_value(
        json!({"url":format!("{endpoint}/echo"),"method":"POST","body":"payload"}),
    )
    .unwrap();
    let value = server.request(request).await.unwrap();
    assert_eq!(value["status"], 409);
    assert_eq!(value["body"], "payload");
    assert_eq!(value["truncated"], false);
    let value = server
        .request(
            serde_json::from_value(json!({"url":format!("{endpoint}/large"),"max_bytes":3}))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(value["body"], "abc");
    assert_eq!(value["truncated"], true);
    task.abort();
}

#[tokio::test]
async fn downloads_are_complete_confined_and_never_overwrite_existing_files() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/file", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/file", get(|| async { vec![0, 1, 2, 255] })),
        )
        .await
        .unwrap();
    });
    let workspace = tempfile::tempdir().unwrap();
    let server = NetworkServer::new(workspace.path()).unwrap();
    let download = |path: &str, max_bytes| {
        serde_json::from_value(json!({"url":url,"path":path,"max_bytes":max_bytes})).unwrap()
    };
    let result = server
        .download(download("files/data.bin", 4))
        .await
        .unwrap();
    assert_eq!(result["bytes"], 4);
    assert_eq!(
        std::fs::read(workspace.path().join("files/data.bin")).unwrap(),
        [0, 1, 2, 255]
    );
    assert!(
        server
            .download(download("files/data.bin", 4))
            .await
            .is_err()
    );
    assert!(server.download(download("too-large.bin", 3)).await.is_err());
    assert!(!workspace.path().join("too-large.bin").exists());
    assert!(server.download(download("../escape.bin", 4)).await.is_err());
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), workspace.path().join("link")).unwrap();
    assert!(
        server
            .download(download("link/escape.bin", 4))
            .await
            .is_err()
    );
    assert!(!outside.path().join("escape.bin").exists());
    task.abort();
}

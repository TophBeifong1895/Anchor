use super::*;
use anchor_platform_session::{CreateSession, SessionError, TurnStatus};
use base64::{Engine, engine::general_purpose::STANDARD};

use crate::test_support::EnvVar;

async fn raw(app: Router, uri: &str, key: &str) -> (StatusCode, HeaderMap, Vec<u8>) {
    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, headers, bytes)
}

#[tokio::test]
async fn pilot_attachments_download_requires_the_frozen_turn_identity() {
    let (root, state) = fixture();
    let first = "a".repeat(32);
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {first}").parse().unwrap(),
    );
    let owner = private_owner(&state, &headers);
    let sessions = store(&state).unwrap();
    let session = sessions
        .create(
            &owner,
            CreateSession {
                id: Some("responses-attachment-session".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let prepared = crate::attachments::from_bytes(
        "notes 中文.txt".into(),
        b"private document".to_vec(),
        Some("text/plain"),
    )
    .unwrap();
    crate::pilot_resources::register(&state.data_root, &state.workspace_root, &owner, &session)
        .unwrap();
    let (turn, _) = sessions
        .create_turn_with_attachments(
            &owner,
            &session.id,
            "request",
            None,
            &crate::pilot_attachments::identity(&prepared, None),
            |turn| {
                crate::pilot_attachments::freeze_inputs(
                    &state.data_root,
                    &owner,
                    &session,
                    turn,
                    None,
                    &prepared,
                )
                .map_err(SessionError::Storage)
            },
        )
        .unwrap();
    let reference =
        crate::pilot_attachments::input_references(&sessions, &owner, &session.id, &turn.id)
            .unwrap()
            .remove(0);
    sessions
        .finish_turn(&owner, &session.id, &turn.id, TurnStatus::Completed, None)
        .unwrap();
    let app = router_with_web_root(state.clone(), root.path().join("web"));
    let (status, headers, bytes) = raw(app.clone(), &reference.url, &first).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, b"private document");
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(headers[header::CONTENT_TYPE], "application/octet-stream");
    assert!(
        headers[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("attachment;")
    );
    assert!(
        headers[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .contains("%E4%B8%AD")
    );
    assert_eq!(
        raw(
            app.clone(),
            &reference.url.replace(&turn.id, "missing-turn"),
            &first
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (status, messages) = authorized_call(
        app.clone(),
        "GET",
        "/sessions/responses-attachment-session/messages",
        &first,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(messages["messages"][0]["text"], "");
    assert_eq!(messages["messages"][0]["turn"], turn.id);
    assert_eq!(messages["messages"][0]["attachments"], json!([reference]));
    let (status, saved) = authorized_call(
        app.clone(),
        "GET",
        &format!("/sessions/responses-attachment-session/turns/{}", turn.id),
        &first,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["turn"]["attachments"], json!([reference]));
    let input = crate::pilot_host::scope(&state.data_root, &owner, &session)
        .join("attachments")
        .join(&turn.id)
        .join("input/files/notes 中文.txt");
    std::fs::remove_file(input).unwrap();
    assert_eq!(
        raw(app, &reference.url, &first).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn pilot_attachment_image_inline_and_download_headers_follow_verified_bytes() {
    let (root, state) = fixture();
    let sessions = store(&state).unwrap();
    let session = sessions
        .create(
            "local",
            CreateSession {
                id: Some("image-session".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let (turn, _) = sessions
        .create_turn("local", &session.id, "image", Some("draw chart"))
        .unwrap();
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::ImageBuffer::from_pixel(
        1,
        1,
        image::Rgb([0, 255, 0]),
    ))
    .write_to(&mut png, image::ImageFormat::Png)
    .unwrap();
    let bytes = png.into_inner();
    let reference = crate::pilot_attachments::publish(
        &state.data_root,
        &sessions,
        "local",
        &session.id,
        &turn.id,
        crate::attachments::from_bytes("chart.png".into(), bytes.clone(), None).unwrap(),
    )
    .unwrap();
    sessions
        .finish_turn("local", &session.id, &turn.id, TurnStatus::Completed, None)
        .unwrap();
    let app = router_with_web_root(state, root.path().join("web"));
    let (status, headers, inline) = raw(app.clone(), &reference.url, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(inline, bytes);
    assert_eq!(headers[header::CONTENT_TYPE], "image/png");
    assert!(
        headers[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("inline;")
    );
    let (status, headers, download) = raw(app.clone(), &reference.download_url, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(download, bytes);
    assert!(
        headers[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("attachment;")
    );
    let (_, sse) = call_text(
        app,
        "GET",
        &format!("/sessions/image-session/turns/{}/events", turn.id),
    )
    .await;
    assert_eq!(sse.matches("\"type\":\"attachment\"").count(), 1);
    assert!(sse.contains(&reference.id));
}

#[tokio::test]
async fn pilot_attachment_validation_and_route_limit_do_not_accept_partial_provider_failures() {
    let _guard = PROCESS_ENV.lock().await;
    let _runtime = EnvVar::set("ANCHOR_RUNNER_AGENT_RUNTIME", "unavailable-fixture");
    let (root, state) = fixture();
    let sessions = store(&state).unwrap();
    let session = sessions
        .create(
            "local",
            CreateSession {
                id: Some("input-session".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let app = router_with_web_root(state.clone(), root.path().join("web"));
    let upload = json!({"name":"notes.txt","data_base64":STANDARD.encode(b"actual bytes"),"media_type":"text/plain"});
    for invalid in [
        json!({"request_id":"bad","resume":true,"attachments":[upload]}),
        json!({"request_id":"bad","attachments":[{"name":"../file","data_base64":"AA=="}]}),
        json!({"request_id":"bad","attachments":[{"name":"file","data_base64":"invalid"}]}),
        json!({"request_id":"bad","attachments":[{"name":"chart.png","data_base64":"AA==","media_type":"image/png"}]}),
        json!({"request_id":"bad","attachments":[upload,upload]}),
    ] {
        assert_eq!(
            call(
                app.clone(),
                "POST",
                "/sessions/input-session/turns",
                Some(&invalid.to_string())
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    let body = json!({"request_id":"valid","attachments":[upload]});
    assert_eq!(
        call(
            app.clone(),
            "POST",
            "/sessions/input-session/turns",
            Some(&body.to_string())
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(
        sessions
            .list_turns("local", &session.id)
            .unwrap()
            .is_empty()
    );
    assert!(
        !crate::pilot_host::scope(&state.data_root, "local", &session)
            .join("attachments")
            .exists()
    );
    let large = json!({"request_id":"large","message":"x".repeat(2*1024*1024+1)}).to_string();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions/input-session/turns")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(large.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(large))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

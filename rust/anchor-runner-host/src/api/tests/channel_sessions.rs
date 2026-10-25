use super::*;
use anchor_platform_session::SessionStore;

fn inbound(id: &str, conversation: &str) -> Value {
    json!({
        "inbound_id": id,
        "identity": {
            "source": "wecom",
            "account": "corp-a",
            "conversation_id": conversation,
            "sender_id": "user-1"
        },
        "graph": "fixture",
        "reply_node": "work",
        "text": "hello"
    })
}

#[tokio::test]
async fn channel_admission_is_idempotent() {
    let (root, state) = fixture();
    let first_key = "a".repeat(32);
    let app = router_with_web_root(state.clone(), root.path().join("web"));

    let (status, first) = authorized_call(
        app.clone(),
        "POST",
        "/channel-sessions/inbound",
        &first_key,
        inbound("message-1", "conversation-1"),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{first}");
    assert_eq!(first["session"]["graph"], "fixture");
    assert_eq!(first["session"]["plugin_mounts"], json!({}));
    assert_eq!(first["session"]["network_allowed"], true);
    assert_eq!(first["relation"]["run_id"], Value::Null);
    let session = first["session"]["id"].as_str().unwrap().to_owned();
    let turn = first["turn"]["id"].as_str().unwrap().to_owned();

    let (status, duplicate) = authorized_call(
        app.clone(),
        "POST",
        "/channel-sessions/inbound",
        &first_key,
        inbound("message-1", "conversation-1"),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{duplicate}");
    assert_eq!(duplicate["session"]["id"], session);
    assert_eq!(duplicate["session"]["plugin_mounts"], json!({}));
    assert_eq!(duplicate["turn"]["id"], turn);

    let (status, relation) = authorized_call(
        app.clone(),
        "GET",
        &format!("/channel-sessions/{session}/inbounds/message-1"),
        &first_key,
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{relation}");
    assert_eq!(relation["relation"]["turn_id"], turn);

    let (status, detail) = authorized_call(
        app.clone(),
        "GET",
        &format!("/channel-sessions/{session}"),
        &first_key,
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["session"]["id"], session);
    assert_eq!(detail["session"]["run_ids"], json!([]));
    let (status, _) = authorized_call(
        app,
        "POST",
        "/channel-sessions/inbound",
        &first_key,
        json!({"owner":"forged", "inbound_id":"message-2"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn channel_delivery_routes_preserve_the_store_state_machine() {
    let (root, state) = fixture();
    let app = router_with_web_root(state.clone(), root.path().join("web"));
    let (_, admitted) = call(
        app.clone(),
        "POST",
        "/channel-sessions/inbound",
        Some(&inbound("message-1", "conversation-1").to_string()),
    )
    .await;
    let session = admitted["session"]["id"].as_str().unwrap();
    let turn = admitted["turn"]["id"].as_str().unwrap();
    let hash = "b".repeat(64);
    let delivery = json!({"key":"reply-1","kind":"text","content_sha256":hash});

    let (status, pending) = call(
        app.clone(),
        "POST",
        &format!("/channel-sessions/{session}/turns/{turn}/deliveries"),
        Some(&delivery.to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{pending}");
    assert_eq!(pending["delivery"]["status"], "pending");

    let (status, sending) = call(
        app.clone(),
        "POST",
        &format!("/channel-sessions/{session}/turns/{turn}/deliveries/reply-1/begin"),
        Some(&json!({"kind":"text","content_sha256":hash}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{sending}");
    assert_eq!(sending["delivery"]["status"], "sending");

    let (status, unfinished) = call(
        app.clone(),
        "GET",
        &format!("/channel-sessions/{session}/deliveries/unresolved"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{unfinished}");
    assert_eq!(unfinished["deliveries"].as_array().unwrap().len(), 1);

    let (status, confirmed) = call(
        app.clone(),
        "POST",
        &format!("/channel-sessions/{session}/deliveries/reply-1/settle"),
        Some(
            &json!({
                "kind":"text",
                "content_sha256":hash,
                "status":"confirmed"
            })
            .to_string(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{confirmed}");
    assert_eq!(confirmed["delivery"]["status"], "confirmed");

    let (status, unfinished) = call(
        app.clone(),
        "GET",
        &format!("/channel-sessions/{session}/deliveries/unresolved"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{unfinished}");
    assert!(unfinished["deliveries"].as_array().unwrap().is_empty());

    let sessions = SessionStore::open(state.data_root.join("platform/sessions.sqlite")).unwrap();
    assert_eq!(sessions.list("local").unwrap().len(), 1);
}

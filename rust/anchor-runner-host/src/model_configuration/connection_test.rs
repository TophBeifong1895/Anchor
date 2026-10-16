use super::*;
use std::time::Duration;

pub(super) async fn run(registry: &ModelRegistry, fields: &ModelFields) -> Result<Value, String> {
    run_with_timeout(registry, fields, Duration::from_secs(20)).await
}

async fn run_with_timeout(
    registry: &ModelRegistry,
    fields: &ModelFields,
    timeout: Duration,
) -> Result<Value, String> {
    let upstream = registry.connection_test_upstream();
    let body = if fields.wire_api == "chat" {
        json!({"model":fields.model,"messages":[{"role":"user","content":"Reply with OK."}],"max_tokens":16,"stream":false})
    } else {
        json!({"model":fields.model,"input":"Reply with OK.","max_output_tokens":16,"stream":false})
    };
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "model test client unavailable")?;
    let started = std::time::Instant::now();
    let mut response = client
        .post(upstream.url)
        .bearer_auth(upstream.api_key)
        .json(&body)
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                "model connection test timed out"
            } else {
                "model connection test failed; check the endpoint and network"
            }
        })?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "model provider returned HTTP {}; check the endpoint, key, model and protocol",
            status.as_u16()
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        if error.is_timeout() {
            "model provider response timed out"
        } else {
            "model provider response could not be read"
        }
    })? {
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Err("model provider response exceeded the connection test limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let response: Value = serde_json::from_slice(&bytes)
        .map_err(|_| "model provider did not return a JSON response")?;
    let valid = if fields.wire_api == "chat" {
        response
            .get("choices")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
    } else {
        response
            .get("output")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
            || response
                .get("output_text")
                .and_then(Value::as_str)
                .is_some_and(|text| !text.is_empty())
    };
    if !valid {
        return Err("model provider response does not match the selected protocol".into());
    }
    Ok(
        json!({"ok":true,"elapsed_ms":started.elapsed().as_millis(),"model":fields.model,"saved":false}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, body::Body, http::Response, routing::post};
    use futures_util::{StreamExt, stream};

    fn configuration(endpoint: &str, wire_api: &str) -> (ModelRegistry, ModelFields) {
        let fields = ModelFields {
            endpoint: endpoint.into(),
            wire_api: wire_api.into(),
            model: "working".into(),
            ..Default::default()
        };
        let mut values = fields.values();
        values.insert(
            "ANCHOR_MODEL_API_KEY".into(),
            "connection-private-key".into(),
        );
        let registry = ModelRegistry::from_values(false, |name| values.get(name).cloned()).unwrap();
        (registry, fields)
    }

    async fn serve(app: Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (endpoint, server)
    }

    #[tokio::test]
    async fn successful_http_with_delayed_body_reports_provider_response_timeout() {
        let app = Router::new().route(
            "/v1/chat/completions",
            post(|| async {
                let chunks = stream::once(async {
                    Ok::<_, std::io::Error>("{\"private\":\"response-private-key\",")
                })
                .chain(stream::once(async {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    Ok::<_, std::io::Error>("\"choices\":[{}]}")
                }));
                Response::builder()
                    .status(200)
                    .body(Body::from_stream(chunks))
                    .unwrap()
            }),
        );
        let (endpoint, server) = serve(app).await;
        let (registry, fields) = configuration(&endpoint, "chat");
        let error = run_with_timeout(&registry, &fields, Duration::from_millis(200))
            .await
            .unwrap_err();
        assert_eq!(error, "model provider response timed out");
        assert!(!error.contains("response-private-key"));
        assert!(!error.contains("connection-private-key"));
        server.abort();
    }

    #[tokio::test]
    async fn delayed_http_headers_keep_the_connection_timeout_diagnostic() {
        let app = Router::new().route(
            "/v1/chat/completions",
            post(|| async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Json(json!({"choices":[{}]}))
            }),
        );
        let (endpoint, server) = serve(app).await;
        let (registry, fields) = configuration(&endpoint, "chat");
        let error = run_with_timeout(&registry, &fields, Duration::from_millis(200))
            .await
            .unwrap_err();
        assert_eq!(error, "model connection test timed out");
        server.abort();
    }

    #[tokio::test]
    async fn completed_response_requires_valid_model_output_for_each_protocol() {
        for (wire_api, path) in [
            ("chat", "/v1/chat/completions"),
            ("responses", "/v1/responses"),
        ] {
            let app = Router::new().route(
                path,
                post(move |Json(body): Json<Value>| async move {
                    assert_eq!(body["stream"], false);
                    if wire_api == "chat" {
                        assert_eq!(body["max_tokens"], 16);
                        assert_eq!(body["messages"][0]["content"], "Reply with OK.");
                    } else {
                        assert_eq!(body["max_output_tokens"], 16);
                        assert_eq!(body["input"], "Reply with OK.");
                    }
                    if body["model"] == "invalid" {
                        return Json(json!({"message":"response-private-key"}));
                    }
                    Json(json!({"choices":[{"message":{"content":"OK"}}],"output_text":"OK"}))
                }),
            );
            let (endpoint, server) = serve(app).await;
            let (registry, mut fields) = configuration(&endpoint, wire_api);
            let result = run_with_timeout(&registry, &fields, Duration::from_secs(2))
                .await
                .unwrap();
            assert_eq!(result["ok"], true);
            assert_eq!(result["saved"], false);
            assert_eq!(result["model"], "working");
            fields.model = "invalid".into();
            let error = run_with_timeout(&registry, &fields, Duration::from_secs(2))
                .await
                .unwrap_err();
            assert_eq!(
                error,
                "model provider response does not match the selected protocol"
            );
            assert!(!error.contains("response-private-key"));
            assert!(!error.contains("connection-private-key"));
            server.abort();
        }
    }
}

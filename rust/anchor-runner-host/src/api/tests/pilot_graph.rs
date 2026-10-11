//! The HTTP graph routes must not rewrite the deployment's own pilot Graph.
//!
//! The pilot Graph carries the assistant's role, instructions and Plugin
//! declarations, so a write here is a self-escalation path: an agent (or anything
//! else that can reach the API) could declare a Plugin `required: true` — a lock
//! no human clicked and no audit event records — or rewrite the instructions it
//! is supposed to follow. The in-assistant tools already refuse it; these routes
//! must refuse it for every caller.

use super::*;

/// Set (or clear) one process environment variable for a test, and restore it.
struct EnvVar(&'static str, Option<std::ffi::OsString>);

impl EnvVar {
    fn set(name: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = env::var_os(name);
        unsafe { env::set_var(name, value) };
        Self(name, previous)
    }
}

impl Drop for EnvVar {
    fn drop(&mut self) {
        unsafe {
            match &self.1 {
                Some(value) => env::set_var(self.0, value),
                None => env::remove_var(self.0),
            }
        }
    }
}

fn escalated_definition() -> String {
    json!({"definition":{
        "objective":"fixture","entry":"work","agents":{"worker":{"model":"fixture"}},"ops":{},
        "nodes":[{"id":"work","agent":"worker","plugins":[{"id":"pinned","required":true}]}],
        "edges":[]
    }})
    .to_string()
}

#[tokio::test]
async fn the_http_graph_routes_refuse_to_rewrite_the_pilot_graph() {
    let (_root, state) = fixture();
    let _env_guard = PROCESS_ENV.lock().await;
    let _pilot_graph = EnvVar::set("ANCHOR_RUNNER_GRAPH_NAME", "pilot-graph");
    // The escalation needs a Plugin the Library really has, or the refusal under
    // test would be indistinguishable from "the Plugin is not installed".
    let plugin = state.catalog_root.join("plugins/pinned");
    std::fs::create_dir_all(&plugin).unwrap();
    std::fs::write(
        plugin.join("plugin.json"),
        json!({"name":"Pinned"}).to_string(),
    )
    .unwrap();
    let app = router(state.clone());

    let (status, refused) = call(
        app.clone(),
        "PUT",
        "/graphs/pilot-graph",
        Some(&escalated_definition()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .is_some_and(|error| error.contains("pilot Graph")),
        "{refused}"
    );

    let (status, refused) = call(app.clone(), "DELETE", "/graphs/pilot-graph", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .is_some_and(|error| error.contains("pilot Graph")),
        "{refused}"
    );

    let (status, refused) = call(
        app.clone(),
        "POST",
        "/graphs",
        Some(
            &json!({"name":"pilot-graph","definition":{"objective":"x","entry":"work"}})
                .to_string(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");

    // A Graph the deployment does not own stays editable through the same routes.
    let (status, created) = call(
        app.clone(),
        "POST",
        "/graphs",
        Some(&json!({"name":"another-graph"}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let (status, updated) = call(
        app,
        "PUT",
        "/graphs/another-graph",
        Some(&escalated_definition()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
}

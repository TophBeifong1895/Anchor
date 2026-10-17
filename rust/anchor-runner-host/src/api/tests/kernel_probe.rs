use super::*;
use anchor_kernel::{Outcome, Phase};
use anchor_runtime::graph::RunStore;

#[tokio::test]
async fn explicit_kernel_probe_runs_real_host_ops_and_projects_existing_queries() {
    let (root, state) = fixture();
    let _env = PROCESS_ENV.lock().await;
    set_host_env(root.path(), &state.data_root);
    let definition = json!({"objective":"kernel probe","entry":"a","agents":{},
        "ops":{"work":{"run":"sh -c 'printf kernel-artifact > report.txt'"}},
        "nodes":[{"id":"a","op":"work"},{"id":"b","op":"work"},{"id":"c","op":"work"}],
        "edges":[{"from":"a","to":"b"},{"from":"b","to":"c"}]});
    std::fs::write(state.bundle_root.join("graph.json"), definition.to_string()).unwrap();
    let router = router(state.clone());
    let (status, body) = call(
        router.clone(),
        "POST",
        "/trigger",
        Some(r#"{"graph":"fixture","input":{},"execution":"kernel_probe"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let id = body["run"].as_str().unwrap();
    let runs = state.application.kernel_owner(id).unwrap().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            if runs.load(id).unwrap().unwrap().is_settled() && !runs.active(id) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        runs.load(id).unwrap().unwrap().phase(),
        Some(&Phase::Settled {
            result: Outcome::Done
        })
    );
    assert!(
        FileRunStore::new(state.data_root.join("runs"))
            .load(id)
            .unwrap()
            .is_none()
    );
    let (status, detail) = call(router.clone(), "GET", &format!("/runs/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["state"]["status"], "completed");
    assert_eq!(detail["projection"]["terminal"], true);
    for node in ["a", "b", "c"] {
        assert_eq!(detail["state"]["nodes"][node]["files"][0], "report.txt");
    }
    let (status, list) = call(router.clone(), "GET", "/runs", None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["runs"].as_array().unwrap().len(), 1);
    assert_eq!(list["runs"][0]["status"], "completed");
    let (status, late) = call(router, "POST", &format!("/runs/{id}/stop"), None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{late}");
}

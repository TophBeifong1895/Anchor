use super::*;

use anchor_library::EnvironmentStore;
use anchor_runtime::graph::{RUN_RECORD_FORMAT, RunStatus};
use std::os::unix::fs::PermissionsExt;

/// Published trees are read-only; a test that tampers with one must restore the
/// write bits first, exactly as the same owner could.
fn force_writable(root: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let mut permissions = std::fs::metadata(&directory).unwrap().permissions();
        permissions.set_mode(permissions.mode() | 0o700);
        std::fs::set_permissions(&directory, permissions).unwrap();
        for entry in std::fs::read_dir(&directory).unwrap() {
            let entry = entry.unwrap();
            let metadata = entry.metadata().unwrap();
            if metadata.is_dir() {
                stack.push(entry.path());
            } else {
                let mut permissions = metadata.permissions();
                permissions.set_mode(permissions.mode() | 0o600);
                std::fs::set_permissions(entry.path(), permissions).unwrap();
            }
        }
    }
}

/// Publish one environment into the deployment library root and point a name at
/// it, exactly as the operator commands would.
fn publish_environment(root: &std::path::Path, marker: &str) -> (EnvironmentStore, String) {
    let prepared = root.join(format!("prepared-{marker}"));
    std::fs::create_dir_all(prepared.join("bin")).unwrap();
    std::fs::write(prepared.join("marker.txt"), marker).unwrap();
    std::fs::write(prepared.join("bin/probe"), "#!/bin/sh\necho probe\n").unwrap();
    std::fs::set_permissions(
        prepared.join("bin/probe"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let store = EnvironmentStore::new(root);
    let published = store.publish(&prepared).unwrap();
    store.set_name("research", &published.id, None).unwrap();
    (store, published.id)
}

#[tokio::test]
async fn a_declared_environment_is_resolved_recorded_and_mounted_for_the_run() {
    let (root, state) = fixture();
    let _env_guard = PROCESS_ENV.lock().await;
    set_host_env(root.path(), &state.data_root);
    let (_store, id) = publish_environment(&state.catalog_root, "managed-environment");
    let environment = state.catalog_root.join("envs").join(&id);

    // The op only succeeds if the published environment is really mounted: it
    // reads a file that exists nowhere else and checks its contents.
    let definition = json!({
        "objective": "environment binding",
        "entry": "work",
        "agents": {},
        "ops": {"work": {"run": format!(
            "sh -c 'test \"$(cat {}/marker.txt)\" = managed-environment'",
            environment.display()
        )}},
        "nodes": [{"id": "work", "op": "work", "plugins": [], "environment": "research"}],
        "edges": []
    });
    let app = router(state.clone());
    let body = json!({"name": "env-graph", "definition": definition}).to_string();
    let (status, created) = call(app.clone(), "POST", "/graphs", Some(&body)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    let body = json!({"graph": "env-graph"}).to_string();
    let (status, accepted) = call(app.clone(), "POST", "/trigger", Some(&body)).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let run = accepted["run"].as_str().unwrap().to_owned();
    wait_idle(&state).await;

    let record = FileRunStore::new(state.data_root.join("runs"))
        .load(&run)
        .unwrap()
        .unwrap();
    assert_eq!(record.status, RunStatus::Completed, "{:?}", record.error);
    assert_eq!(record.format, RUN_RECORD_FORMAT);
    // The Run records exactly the environment it ran with.
    let binding = record
        .environments
        .get("node_task:work")
        .expect("frozen binding");
    assert_eq!(binding.name, "research");
    assert_eq!(binding.id, id);
}

#[tokio::test]
async fn an_unresolvable_environment_refuses_admission() {
    let (root, state) = fixture();
    let _env_guard = PROCESS_ENV.lock().await;
    set_host_env(root.path(), &state.data_root);
    // A published environment exists, but the Graph asks for another name.
    let (_store, _id) = publish_environment(&state.catalog_root, "managed-environment");

    let definition = json!({
        "objective": "unknown environment",
        "entry": "work",
        "agents": {},
        "ops": {"work": {"run": "true"}},
        "nodes": [{"id": "work", "op": "work", "plugins": [], "environment": "missing"}],
        "edges": []
    });
    let app = router(state.clone());
    let body = json!({"name": "unknown-env", "definition": definition}).to_string();
    let (status, created) = call(app.clone(), "POST", "/graphs", Some(&body)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    let body = json!({"graph": "unknown-env"}).to_string();
    let (status, rejected) = call(app.clone(), "POST", "/trigger", Some(&body)).await;
    assert!(!status.is_success(), "{rejected}");
    assert!(
        rejected
            .to_string()
            .contains("unknown dependency environment"),
        "{rejected}"
    );

    // Nothing ran: no Run record was created for the refused trigger.
    let runs = std::fs::read_dir(state.data_root.join("runs"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
                .count()
        })
        .unwrap_or(0);
    assert_eq!(runs, 0, "a refused admission must not create a Run");
}

#[tokio::test]
async fn a_tampered_published_tree_is_refused_at_the_next_admission() {
    let (root, state) = fixture();
    let _env_guard = PROCESS_ENV.lock().await;
    set_host_env(root.path(), &state.data_root);
    let (_store, id) = publish_environment(&state.catalog_root, "managed-environment");
    let environment = state.catalog_root.join("envs").join(&id);

    let definition = json!({
        "objective": "tampered environment",
        "entry": "work",
        "agents": {},
        "ops": {"work": {"run": "true"}},
        "nodes": [{"id": "work", "op": "work", "plugins": [], "environment": "research"}],
        "edges": []
    });
    let app = router(state.clone());
    let body = json!({"name": "tampered-env", "definition": definition}).to_string();
    let (status, created) = call(app.clone(), "POST", "/graphs", Some(&body)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    // The store's own verify already reports this; admission must refuse too.
    force_writable(&environment);
    std::fs::write(environment.join("marker.txt"), "rewritten").unwrap();

    let body = json!({"graph": "tampered-env"}).to_string();
    let (status, rejected) = call(app.clone(), "POST", "/trigger", Some(&body)).await;
    assert!(!status.is_success(), "{rejected}");
    assert!(rejected.to_string().contains("not usable"), "{rejected}");
}

#[tokio::test]
async fn a_called_graph_freezes_and_uses_its_own_environment() {
    let (root, state) = fixture();
    let _env_guard = PROCESS_ENV.lock().await;
    set_host_env(root.path(), &state.data_root);
    let (_store, id) = publish_environment(&state.catalog_root, "managed-environment");
    let environment = state.catalog_root.join("envs").join(&id);
    let app = router(state.clone());

    // The called Graph is its own catalog entry, and it declares the environment.
    let child = json!({
        "objective": "child environment",
        "entry": "inside",
        "agents": {},
        "ops": {"inside": {"run": format!(
            "sh -c 'test \"$(cat {}/marker.txt)\" = managed-environment'",
            environment.display()
        )}},
        "nodes": [{"id": "inside", "op": "inside", "environment": "research"}],
        "edges": []
    });
    let body = json!({"name": "inner", "definition": child}).to_string();
    let (status, created) = call(app.clone(), "POST", "/graphs", Some(&body)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    let parent = json!({
        "objective": "parent calls a managed child",
        "entry": "work",
        "agents": {},
        "ops": {"work": {"call": {"graph": "inner", "mode": "wait"}}},
        "nodes": [{"id": "work", "op": "work"}],
        "edges": []
    });
    let body = json!({"name": "child-env-graph", "definition": parent}).to_string();
    let (status, created) = call(app.clone(), "POST", "/graphs", Some(&body)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    let body = json!({"graph": "child-env-graph"}).to_string();
    let (status, accepted) = call(app.clone(), "POST", "/trigger", Some(&body)).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let parent_run = accepted["run"].as_str().unwrap().to_owned();
    wait_idle(&state).await;

    let store = FileRunStore::new(state.data_root.join("runs"));
    let parent = store.load(&parent_run).unwrap().unwrap();
    assert_eq!(parent.status, RunStatus::Completed, "{:?}", parent.error);
    let child_run = parent
        .graph_calls
        .values()
        .next()
        .and_then(|call| call.child_run_id.clone())
        .expect("the call records its child");

    // The child Run froze the identity from the same store its parent used, and
    // its command only succeeded because that tree was really mounted.
    let child = store.load(&child_run).unwrap().unwrap();
    let frozen = child
        .environments
        .get("node_task:inside")
        .expect("the child Run froze its declared environment");
    assert_eq!(frozen.name, "research");
    assert_eq!(frozen.id, id);
}

#[tokio::test]
async fn the_operator_can_see_environment_requests_and_how_they_ended() {
    let (root, state) = fixture();
    let _env_guard = PROCESS_ENV.lock().await;
    set_host_env(root.path(), &state.data_root);
    let (_store, id) = publish_environment(&state.catalog_root, "managed-environment");

    // Two recorded decisions: one published, one refused.
    let plan = |name: &str,
                digest: &str,
                status: crate::application::environments::EnvironmentPlanStatus| {
        crate::application::environments::EnvironmentPlan {
            requester: crate::application::environments::EnvironmentPlanRequester {
                owner: "local".into(),
                session: "pilot".into(),
                turn: "turn-7".into(),
            },
            directory: root.path().join("prepared"),
            name: name.into(),
            digest: digest.into(),
            bytes: 12,
            files: 2,
            observed_id: None,
            observed_present: false,
            observed_name_version: None,
            references: None,
            status,
            created: 0,
        }
    };
    state
        .application
        .write_environment_plan(&plan(
            "research",
            &id,
            crate::application::environments::EnvironmentPlanStatus::Published {
                id: id.clone(),
                name_version: 1,
            },
        ))
        .unwrap();
    state
        .application
        .write_environment_plan(&plan(
            "other",
            &"b".repeat(64),
            crate::application::environments::EnvironmentPlanStatus::Refused,
        ))
        .unwrap();
    // An approved act that did not complete must not read as unanswered.
    state
        .application
        .write_environment_plan(&plan(
            "aborted",
            &"c".repeat(64),
            crate::application::environments::EnvironmentPlanStatus::Aborted {
                reason: "the candidate changed".into(),
            },
        ))
        .unwrap();

    let app = router(state.clone());
    let (status, listed) = call(app, "GET", "/environment-publications", None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let requests = listed["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 3, "{listed}");
    let published = requests
        .iter()
        .find(|request| request["name"] == "research")
        .unwrap();
    assert_eq!(published["status"], "published");
    assert_eq!(published["id"], id);
    assert_eq!(published["digest"], id);
    assert_eq!(published["name_version"], 1);
    assert_eq!(published["files"], 2);
    assert_eq!(published["requester"]["turn"], "turn-7");
    // A request or a refusal has no published identity, only the content the
    // human was shown.
    let refused = requests
        .iter()
        .find(|request| request["name"] == "other")
        .unwrap();
    assert_eq!(refused["status"], "refused");
    assert_eq!(refused["id"], Value::Null);
    assert_eq!(refused["digest"], "b".repeat(64));
    let aborted = requests
        .iter()
        .find(|request| request["name"] == "aborted")
        .unwrap();
    assert_eq!(aborted["status"], "aborted");
    assert_eq!(aborted["note"], "the candidate changed");
    assert_eq!(aborted["id"], Value::Null);
}

#[tokio::test]
async fn the_operator_can_see_what_environments_the_assistant_asked_for() {
    let (root, state) = fixture();
    let _env_guard = PROCESS_ENV.lock().await;
    set_host_env(root.path(), &state.data_root);

    // One need nobody has provided yet, and one whose name already points at
    // published content: the read model must tell them apart by deriving it.
    let record = |name: &str| crate::application::environments::EnvironmentRequirement {
        format: 1,
        name: name.into(),
        dropped_asks: 0,
        requests: vec![
            crate::application::environments::EnvironmentRequirementAsk {
                requester: crate::application::environments::EnvironmentPlanRequester {
                    owner: "local".into(),
                    session: "pilot".into(),
                    turn: "turn-9".into(),
                },
                purpose: "parse the weekly reports".into(),
                interpreter: Some("python>=3.11".into()),
                packages: vec!["pandas".into(), "pyarrow".into()],
                notes: Some("needs the parquet reader".into()),
            },
        ],
        observed: None,
    };
    std::fs::create_dir_all(state.data_root.join("environment-requirements")).unwrap();
    for name in ["open", "research"] {
        std::fs::write(
            state
                .data_root
                .join(format!("environment-requirements/{name}.json")),
            serde_json::to_vec(&record(name)).unwrap(),
        )
        .unwrap();
    }
    let (_store, id) = publish_environment(&state.catalog_root, "managed-environment");

    let app = router(state.clone());
    let (status, listed) = call(app, "GET", "/environment-requirements", None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["unreadable"], 0);
    let requirements = listed["requirements"].as_array().unwrap();
    assert_eq!(requirements.len(), 2, "{listed}");

    let provisioned = requirements
        .iter()
        .find(|requirement| requirement["name"] == "research")
        .unwrap();
    assert_eq!(provisioned["progress"], "provisioned");
    assert_eq!(provisioned["changed_since_request"], true);
    assert_eq!(provisioned["id"], id);
    assert_eq!(provisioned["requests"][0]["interpreter"], "python>=3.11");
    assert_eq!(provisioned["requests"][0]["packages"][1], "pyarrow");
    assert_eq!(provisioned["requests"][0]["requester"]["turn"], "turn-9");

    let open = requirements
        .iter()
        .find(|requirement| requirement["name"] == "open")
        .unwrap();
    assert_eq!(open["progress"], "requested");
    assert_eq!(open["changed_since_request"], false);
    assert_eq!(open["id"], Value::Null);
}

#[tokio::test]
async fn the_operator_can_see_what_environments_were_built_and_how_they_ended() {
    use crate::application::environments::{EnvironmentProvisionPlan, EnvironmentProvisionStatus};
    use crate::environment_provision::LockedPackage;

    let (root, state) = fixture();
    let _env_guard = PROCESS_ENV.lock().await;
    set_host_env(root.path(), &state.data_root);

    let requester = crate::application::environments::EnvironmentPlanRequester {
        owner: "local".into(),
        session: "pilot".into(),
        turn: "turn-11".into(),
    };
    let plan =
        |name: &str, lock: &str, status: EnvironmentProvisionStatus| EnvironmentProvisionPlan {
            requester: requester.clone(),
            name: name.into(),
            ask: crate::application::environments::EnvironmentRequirementAsk {
                requester: requester.clone(),
                purpose: "analyse documents".into(),
                interpreter: Some("3.12".into()),
                packages: vec!["pandas==2.2.0".into()],
                notes: None,
            },
            sources: vec!["index:https://pypi.org/simple".into()],
            interpreter: "3.12.13".into(),
            interpreter_path: std::path::PathBuf::from("/usr/bin/python3.12"),
            interpreter_prefix: std::path::PathBuf::from("/usr"),
            interpreter_digest: "a".repeat(64),
            lock_digest: lock.into(),
            lock: "pandas==2.2.0 --hash=sha256:".to_owned() + &"b".repeat(64) + "\n",
            packages: vec![LockedPackage {
                name: "pandas".into(),
                version: "2.2.0".into(),
                hashes: vec!["b".repeat(64)],
            }],
            references: None,
            observed_id: None,
            observed_present: false,
            observed_name_version: None,
            status,
            created: 0,
        };
    state
        .application
        .write_environment_provision_plan(&plan(
            "research",
            &"1".repeat(64),
            EnvironmentProvisionStatus::Provisioned {
                id: "3".repeat(64),
                name_version: 1,
                files: 42,
                bytes: 4096,
            },
        ))
        .unwrap();
    // An approved build that did not finish must be distinguishable from one
    // nobody answered.
    state
        .application
        .write_environment_provision_plan(&plan(
            "other",
            &"2".repeat(64),
            EnvironmentProvisionStatus::Aborted {
                reason: "the fetch found no matching digest".into(),
            },
        ))
        .unwrap();

    // A build nobody answered must read as cancelled, never as requested.
    state
        .application
        .write_environment_provision_plan(&plan(
            "cancelled",
            &"4".repeat(64),
            EnvironmentProvisionStatus::Cancelled {
                reason: "the Pilot was cancelled while the form was open".into(),
            },
        ))
        .unwrap();

    let app = router(state.clone());
    let (status, listed) = call(app, "GET", "/environment-provisions", None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let requests = listed["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 3, "{listed}");
    let built = requests
        .iter()
        .find(|request| request["name"] == "research")
        .unwrap();
    assert_eq!(built["status"], "provisioned");
    assert_eq!(built["id"], "3".repeat(64));
    assert_eq!(built["name_version"], 1);
    assert_eq!(built["files"], 42);
    assert_eq!(built["bytes"], 4096);
    assert_eq!(built["packages"], 1);
    assert_eq!(built["interpreter"], "3.12.13");
    assert_eq!(built["lock_digest"], "1".repeat(64));
    assert_eq!(built["requester"]["turn"], "turn-11");
    let aborted = requests
        .iter()
        .find(|request| request["name"] == "other")
        .unwrap();
    assert_eq!(aborted["status"], "aborted");
    assert_eq!(aborted["note"], "the fetch found no matching digest");
    assert_eq!(aborted["id"], Value::Null);
    let cancelled = requests
        .iter()
        .find(|request| request["name"] == "cancelled")
        .unwrap();
    assert_eq!(cancelled["status"], "cancelled");
    assert_eq!(
        cancelled["note"],
        "the Pilot was cancelled while the form was open"
    );
    assert_eq!(cancelled["id"], Value::Null);
    assert_eq!(listed["unreadable"], 0);
    assert_eq!(listed["truncated"], false);
    assert_eq!(
        listed["pruned"], 0,
        "nothing is deleted while every candidate is distinct: {listed}"
    );
}

/// One durable approval record, as the pilot paths write it.
fn approval_record(
    name: &str,
    digest: &str,
    turn: &str,
    status: crate::application::environments::EnvironmentPlanStatus,
    created: u64,
) -> crate::application::environments::EnvironmentPlan {
    crate::application::environments::EnvironmentPlan {
        requester: crate::application::environments::EnvironmentPlanRequester {
            owner: "local".into(),
            session: "pilot".into(),
            turn: turn.into(),
        },
        directory: std::path::PathBuf::from("/prepared"),
        name: name.into(),
        digest: digest.into(),
        bytes: 12,
        files: 2,
        observed_id: None,
        observed_present: false,
        observed_name_version: None,
        references: None,
        status,
        created,
    }
}

/// T4's read-model half (§D1B/I4): a cancelled request is reported as
/// `cancelled` — never as `requested` — and the deletions that bounded the
/// directory are reported as `pruned` instead of being silent.
#[tokio::test]
async fn the_operator_can_see_cancelled_requests_and_what_retention_pruned() {
    use crate::application::environments::EnvironmentPlanStatus;

    let (root, state) = fixture();
    let _env_guard = PROCESS_ENV.lock().await;
    set_host_env(root.path(), &state.data_root);

    let digest = "a".repeat(64);
    for index in 0..8u64 {
        state
            .application
            .write_environment_plan(&approval_record(
                "research",
                &digest,
                &format!("turn-{index}"),
                EnvironmentPlanStatus::Cancelled {
                    reason: "the Pilot was cancelled while the form was open".into(),
                },
                100 + index,
            ))
            .unwrap();
    }

    let app = router(state.clone());
    let (status, listed) = call(app, "GET", "/environment-publications", None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let requests = listed["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 1, "{listed}");
    assert_eq!(requests[0]["status"], "cancelled");
    assert_eq!(
        requests[0]["note"],
        "the Pilot was cancelled while the form was open"
    );
    assert_eq!(requests[0]["id"], Value::Null);
    assert!(
        requests
            .iter()
            .all(|request| request["status"] != "requested"),
        "nobody can answer these any more: {listed}"
    );
    assert_eq!(
        listed["pruned"],
        json!(7),
        "every deletion is reported: {listed}"
    );
}

/// T7 (§D4A): a `requested` record that survives a restart is a request nobody
/// can answer, so startup settles it into a terminal state instead of reporting
/// a live question forever.
#[tokio::test]
async fn startup_settles_requests_whose_turn_is_over() {
    use crate::application::environments::{EnvironmentPlanStatus, list_environment_plans};
    use anchor_platform_session::{CreateSession, TurnStatus};

    let (root, state) = fixture();
    let _env_guard = PROCESS_ENV.lock().await;
    set_host_env(root.path(), &state.data_root);

    // The Turn that asked was still running when the host stopped.
    let sessions = crate::api::sessions::store(&state).unwrap();
    sessions
        .create(
            "local",
            CreateSession {
                id: Some("pilot".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let (running, _) = sessions
        .create_turn("local", "pilot", "request-1", Some("publish it"))
        .unwrap();
    state
        .application
        .write_environment_plan(&approval_record(
            "research",
            &"a".repeat(64),
            &running.id,
            EnvironmentPlanStatus::Requested,
            0,
        ))
        .unwrap();
    // A Turn this host has no record of cannot be answered either.
    state
        .application
        .write_environment_plan(&approval_record(
            "forgotten",
            &"b".repeat(64),
            "turn-that-never-existed",
            EnvironmentPlanStatus::Requested,
            0,
        ))
        .unwrap();
    // A request a human already answered is not reconciliation's business.
    state
        .application
        .write_environment_plan(&approval_record(
            "answered",
            &"c".repeat(64),
            "turn-refused",
            EnvironmentPlanStatus::Refused,
            0,
        ))
        .unwrap();

    let before = list_environment_plans(&state.data_root).unwrap().records;
    let created = |name: &str| {
        before
            .iter()
            .find(|plan| plan.name == name)
            .unwrap()
            .created
    };

    crate::api::turns::recover_pilot_turns(&state)
        .await
        .expect("startup recovery");

    assert_eq!(
        sessions
            .get_turn("local", "pilot", &running.id)
            .unwrap()
            .status,
        TurnStatus::Interrupted,
        "startup recovery ends the Turn that was running"
    );
    let listed = list_environment_plans(&state.data_root).unwrap();
    for name in ["research", "forgotten"] {
        let plan = listed
            .records
            .iter()
            .find(|plan| plan.name == name)
            .unwrap_or_else(|| panic!("{name} is still listed: {:?}", listed.records));
        assert!(
            matches!(plan.status, EnvironmentPlanStatus::Cancelled { .. }),
            "{name} must not stay a live request: {:?}",
            plan.status
        );
        assert_eq!(
            plan.created,
            created(name),
            "reconciliation rewrites the record in place instead of replacing it"
        );
    }
    let answered = listed
        .records
        .iter()
        .find(|plan| plan.name == "answered")
        .unwrap();
    assert_eq!(answered.status, EnvironmentPlanStatus::Refused);

    // The operator read model shows the same thing.
    let app = router(state.clone());
    let (status, projection) = call(app, "GET", "/environment-publications", None).await;
    assert_eq!(status, StatusCode::OK, "{projection}");
    assert!(
        projection["requests"]
            .as_array()
            .unwrap()
            .iter()
            .all(|request| request["status"] != "requested"),
        "{projection}"
    );
}

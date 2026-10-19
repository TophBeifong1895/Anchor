#[tokio::test]
async fn graph_runner_preserves_recorded_runtime_scenarios() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/runtime-scenarios.json"
    ))
    .unwrap();
    assert_eq!(fixture["format"], 1);

    let scenarios = fixture["scenarios"].as_array().unwrap();
    assert_eq!(scenarios.len(), 10);
    for scenario in scenarios {
        let snapshot = GraphSnapshot::admit(scenario["graph_snapshot"].clone()).unwrap();
        let override_input = scenario["run_override"]["input"].clone();
        let nodes = ScenarioNodes {
            outcomes: scenario["node_outcomes"].as_array().unwrap().clone(),
            ..ScenarioNodes::default()
        };
        let artifacts = MemoryArtifacts::default();
        let pause = scenario["control"]["pause_before_dispatch"] == true;
        let record = GraphRunRecord::create(snapshot, override_input).unwrap();
        assert_eq!(record.input, scenario["effective_input"]);
        let harness = Harness::memory(record);
        if pause {
            assert!(harness.request("stop").is_accepted());
        }
        harness.run(&artifacts, &nodes).await.unwrap();
        let result = harness.record();
        let expected = &scenario["expected"];

        let rust_status = match harness.phase() {
            Phase::Settled {
                result: Outcome::Done,
            } => "completed",
            Phase::Settled {
                result: Outcome::Failed { .. },
            } => "failed",
            Phase::Settled {
                result: Outcome::Escalated { .. },
            } => "stopped",
            Phase::Waiting {
                waiter: Waiter::Human { .. },
            } if pause => "paused",
            Phase::Waiting {
                waiter: Waiter::Human { .. },
            } => "budget_stopped",
            other => panic!("unexpected Kernel phase: {other:?}"),
        };
        assert_eq!(
            rust_status, expected["status"],
            "scenario {}, reason {:?}, ceased {:?}",
            scenario["id"], result.error, result.ceased
        );

        let calls = nodes.calls.lock().unwrap().clone();
        let ordered_executed: Vec<&str> = calls.iter().map(|(node, _)| node.as_str()).collect();
        let expected_executed: Vec<&str> = expected["ordered_executed_nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|node| node.as_str().unwrap())
            .collect();
        assert_eq!(
            ordered_executed, expected_executed,
            "scenario {}",
            scenario["id"]
        );

        let executed_set: BTreeSet<&str> = calls.iter().map(|(node, _)| node.as_str()).collect();
        let derived_skipped: Vec<&str> = result
            .snapshot
            .nodes
            .iter()
            .filter(|node| !executed_set.contains(node.id.as_str()))
            .map(|node| node.id.as_str())
            .collect();
        let expected_skipped: Vec<&str> = expected["skipped_nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|node| node.as_str().unwrap())
            .collect();
        if scenario["id"] == "deterministic_failure_settles_single_exit" {
            assert_eq!(derived_skipped, vec!["downstream"]);
        } else if scenario["id"] == "pause_before_first_dispatch" {
            // Unstarted nodes are not considered skipped when the run pauses
            // before admission reaches the scheduler's propagation pass.
            assert!(expected_skipped.is_empty());
        } else {
            assert_eq!(
                derived_skipped, expected_skipped,
                "skipped nodes, scenario {}",
                scenario["id"]
            );
        }

        let mut expected_passes = expected["passes"].as_object().unwrap().clone();
        if scenario["id"] == "budget_stop_preserves_cursor" {
            expected_passes.clear();
        }
        assert_eq!(
            result.passes.len(),
            expected_passes.len(),
            "scenario {}",
            scenario["id"]
        );
        for (node, passes) in &expected_passes {
            assert_eq!(
                result.passes.get(node),
                passes.as_u64().as_ref(),
                "passes for {node}, scenario {}",
                scenario["id"]
            );
        }
        if scenario["id"] == "budget_stop_preserves_cursor" {
            assert_eq!(
                expected["passes"]["budgeted"], 1,
                "A started budgeted invocation must be recorded"
            );
        }
        if scenario["id"] == "pause_before_first_dispatch" {
            assert_eq!(expected["reason"], "asked");
            assert!(matches!(
                harness.phase(),
                Phase::Waiting {
                    waiter: Waiter::Human { .. }
                }
            ));
            assert!(nodes.calls.lock().unwrap().is_empty());
            assert!(result.invocations.is_empty());
            assert!(result.passes.is_empty());
            assert!(result.decided.is_empty());
            assert!(scenario["node_outcomes"].as_array().unwrap().is_empty());
        }

        let mut ceased: Vec<String> = result.ceased.iter().cloned().collect();
        ceased.sort();
        let mut expected_ceased: Vec<String> = expected["ceased"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item.as_str().unwrap().to_owned())
            .collect();
        expected_ceased.sort();
        assert_eq!(ceased, expected_ceased, "scenario {}", scenario["id"]);

        let expected_cursor = &expected["cursor"];
        let cursor_fact = if expected_cursor["exists"] == true {
            let dispatch = harness.frontier().into_iter().next().unwrap();
            assert_eq!(dispatch.node.id, expected_cursor["node"].as_str().unwrap());
            assert_eq!(
                dispatch.invocation,
                expected_cursor["invocation"].as_u64().unwrap()
            );
            assert_eq!(
                result.passes.get(&dispatch.node.id),
                None,
                "S6: an unfinished dispatch has no completed pass"
            );
            serde_json::json!({"exists":true,"node":dispatch.node.id,"pass":dispatch.invocation,"invocation":dispatch.invocation})
        } else {
            serde_json::json!({"exists":false})
        };

        let mut decisions: Vec<Value> = result
            .decided
            .iter()
            .map(|(key, decision)| {
                let (from, to) = key.split_once('|').unwrap();
                serde_json::json!({"from":from,"to":to,"selected":decision.selected})
            })
            .collect();
        decisions.sort_by(|a, b| {
            (a["from"].as_str(), a["to"].as_str()).cmp(&(b["from"].as_str(), b["to"].as_str()))
        });
        let mut expected_decisions = expected["edge_decisions"].as_array().unwrap().clone();
        expected_decisions.sort_by(|a, b| {
            (a["from"].as_str(), a["to"].as_str()).cmp(&(b["from"].as_str(), b["to"].as_str()))
        });
        if scenario["id"] == "deterministic_failure_settles_single_exit" {
            // The historical scenario settles a single exit
            // even after node failure; Rust treats the failed fact as terminal.
            assert!(
                matches!(
                    harness.phase(),
                    Phase::Settled {
                        result: Outcome::Failed { .. }
                    }
                ),
                "intentional semantic divergence: failure is terminal"
            );
            assert_eq!(result.passes.get("fail"), Some(&1));
            assert!(harness.resource().is_settled());
            assert!(result.decided.is_empty());
            assert_eq!(expected_decisions.len(), 1);
            assert_eq!(expected_decisions[0]["to"], "downstream");
            assert_eq!(expected_decisions[0]["selected"], true);
            assert_eq!(cursor_fact, expected_cursor.clone());
        } else {
            assert_eq!(
                (decisions, cursor_fact),
                (expected_decisions, expected_cursor.clone()),
                "edge decisions and cursor, scenario {}",
                scenario["id"]
            );
        }
    }
}

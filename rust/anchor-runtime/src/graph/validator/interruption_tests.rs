//! Host interruption facts share the existing control completion contract.

use anchor_kernel::{Effect, Principal};

use super::*;

fn graph() -> GraphSnapshot {
    GraphSnapshot {
        objective: "interruption control".into(),
        input: Value::Null,
        entry: "work".into(),
        agents: BTreeMap::from([(
            "worker".into(),
            AgentDefinition {
                model: "model".into(),
                instructions: "work".into(),
                network: false,
                max_steps: None,
                wall_time_limit_seconds: None,
                reads: vec![],
                writes: vec![],
            },
        )]),
        ops: BTreeMap::new(),
        nodes: ["work", "wait", "reply"]
            .into_iter()
            .map(|id| GraphNode {
                id: id.into(),
                agent: Some("worker".into()),
                op: None,
                input: None,
                plugins: vec![],
                max_rounds: None,
            })
            .collect(),
        edges: ["wait", "reply"]
            .into_iter()
            .map(|to| GraphEdge {
                from_node: "work".into(),
                to_node: to.into(),
            })
            .collect(),
        module_rounds: BTreeMap::new(),
    }
}

fn resource(snapshot: GraphSnapshot) -> Resource {
    run_resource(
        &GraphRunRecord::create_with_id(snapshot, Value::Null, "interruption-test").unwrap(),
        u64::MAX,
    )
    .unwrap()
}

fn content(node: &str) -> Value {
    serde_json::json!({
        "summary": "superseded",
        "route": "wait",
        "output": {"interrupted": true, "reason": "superseded"},
        "model_requests": 0,
        "commit": {"id": "control-artifact", "node_id": node, "invocation": 1},
        "interruption": "superseded"
    })
}

fn judge(resource: &Resource, kind: PrincipalKind, node: &str, content: Value) -> Judgement {
    GraphValidator.judge(
        &JudgeCtx {
            resource,
            principal: &Principal::new("executor", kind),
        },
        &Submission::new(&resource.id, resource.version, Action::Write)
            .slot(format!("{node}#1"))
            .content(content),
    )
}

#[test]
fn only_host_code_can_submit_an_interruption_field_including_null() {
    let resource = resource(graph());
    for kind in [PrincipalKind::Agent, PrincipalKind::Human] {
        for interruption in [serde_json::json!("superseded"), Value::Null] {
            let mut content = content("work");
            content["interruption"] = interruption;
            let judgement = judge(&resource, kind, "work", content);
            assert_eq!(judgement.verdict.effect, Effect::Refused);
            assert_eq!(judgement.verdict.reason, Reason::Unauthorized);
            assert!(judgement.state.is_none());
        }
    }
}

#[test]
fn ordinary_submissions_omit_interruption_and_remain_agent_writable() {
    let resource = resource(graph());
    let ordinary = NodeSubmission {
        summary: "completed".into(),
        interruption: None,
        route: Some("reply".into()),
        output: serde_json::json!({"answer": 42}),
        model_requests: 1,
        commit: CommitRef {
            id: "node-artifact".into(),
            node_id: "work".into(),
            invocation: 1,
        },
        plugin_views: BTreeMap::new(),
        plugin_digests: BTreeMap::new(),
    };
    let content = serde_json::to_value(&ordinary).unwrap();
    assert!(content.get("interruption").is_none());
    let judgement = judge(&resource, PrincipalKind::Agent, "work", content.clone());
    assert_eq!(judgement.verdict.effect, Effect::Accepted);
    let state: GraphRunRecord = serde_json::from_value(judgement.state.unwrap()).unwrap();
    assert_eq!(state.results["work"][0].interruption, None);
    assert_eq!(state.results["work"][0].completion.model_requests, 1);
    let mut explicit_null = content;
    explicit_null["interruption"] = Value::Null;
    let judgement = judge(&resource, PrincipalKind::Code, "work", explicit_null);
    assert_eq!(judgement.verdict.effect, Effect::Accepted);
}

#[test]
fn host_interruption_records_control_completion_and_takes_the_ordinary_route() {
    let mut resource = resource(graph());
    let judgement = judge(&resource, PrincipalKind::Code, "work", content("work"));
    assert_eq!(judgement.verdict.effect, Effect::Accepted);
    assert!(judgement.phase.is_none());
    resource.state = judgement.state.unwrap();
    let record = record_of(&resource).unwrap();
    let result = &record.results["work"][0];
    assert_eq!(result.interruption.as_deref(), Some("superseded"));
    assert_eq!(
        result.completion,
        interruption_completion("superseded", Some("wait".into()))
    );
    assert_eq!(result.commit.id, "control-artifact");
    assert!(record.decided["work|wait"].selected);
    assert!(!record.decided["work|reply"].selected);
    record.validate().unwrap();
    let frontier = GraphValidator::frontier(&resource).unwrap();
    assert_eq!(frontier.len(), 1);
    assert_eq!(frontier[0].node.id, "wait");
    assert_eq!(frontier[0].inputs, vec![result.commit.clone()]);
}

#[test]
fn interrupted_completion_cannot_claim_a_successful_model_result() {
    let resource = resource(graph());
    for field in ["summary", "model_requests", "output"] {
        let mut content = content("work");
        content[field] = match field {
            "summary" => serde_json::json!("model finished"),
            "model_requests" => serde_json::json!(1),
            "output" => serde_json::json!({"answer": 42}),
            _ => unreachable!(),
        };
        let judgement = judge(&resource, PrincipalKind::Code, "work", content);
        assert_eq!(judgement.verdict.effect, Effect::Returned);
        assert_eq!(judgement.verdict.reason, Reason::ContentInvalid);
        assert!(judgement.state.is_none());
    }
}

#[test]
fn interrupted_completion_requires_a_valid_route_before_acceptance() {
    let resource = resource(graph());
    for route in [Value::Null, serde_json::json!("missing")] {
        let mut content = content("work");
        content["route"] = route;
        let judgement = judge(&resource, PrincipalKind::Code, "work", content);
        assert_eq!(judgement.verdict.effect, Effect::Returned);
        assert_eq!(judgement.verdict.reason, Reason::RouteInvalid);
        assert!(judgement.state.is_none());
    }
}

#[test]
fn interruptions_preserve_the_existing_parallel_region_boundary() {
    let mut snapshot = graph();
    snapshot.entry = "f".into();
    snapshot.ops = BTreeMap::from([
        ("fan".into(), serde_json::json!({"fanout": {"join": "j"}})),
        ("join".into(), serde_json::json!({"join": {}})),
    ]);
    snapshot.nodes.retain(|node| node.id != "reply");
    for (id, op) in [("f", "fan"), ("j", "join")] {
        snapshot.nodes.push(GraphNode {
            id: id.into(),
            agent: None,
            op: Some(op.into()),
            input: None,
            plugins: vec![],
            max_rounds: None,
        });
    }
    snapshot.edges = [("f", "work"), ("f", "wait"), ("work", "j"), ("wait", "j")]
        .into_iter()
        .map(|(from, to)| GraphEdge {
            from_node: from.into(),
            to_node: to.into(),
        })
        .collect();
    let mut resource = resource(snapshot);
    let mut fanout = content("f");
    fanout.as_object_mut().unwrap().remove("interruption");
    fanout["route"] = Value::Null;
    let accepted = judge(&resource, PrincipalKind::Code, "f", fanout);
    assert_eq!(accepted.verdict.effect, Effect::Accepted);
    let rejected = judge(&resource, PrincipalKind::Code, "f", content("f"));
    assert_eq!(rejected.verdict.effect, Effect::Refused);
    assert_eq!(rejected.verdict.reason, Reason::ContentInvalid);
    resource.state = accepted.state.unwrap();
    let mut branch = content("work");
    branch["route"] = serde_json::json!("j");
    let rejected = judge(&resource, PrincipalKind::Code, "work", branch);
    assert_eq!(rejected.verdict.effect, Effect::Refused);
    assert_eq!(rejected.verdict.reason, Reason::ContentInvalid);
    assert!(rejected.state.is_none());
}

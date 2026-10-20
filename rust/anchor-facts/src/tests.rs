use super::*;
use anchor_kernel::{Effect, MemoryStore, Resource};
use serde_json::json;

fn invocation() -> Invocation {
    Invocation {
        run_id: "rust-1".into(),
        graph_digest: "g".into(),
        node_id: "worker".into(),
        invocation: 1,
    }
}

fn configuration() -> AgentConfiguration {
    AgentConfiguration {
        version: VERSION,
        graph_digest: "g".into(),
        node_id: "worker".into(),
        model: "flash".into(),
        model_binding: "binding".into(),
        runtime_sha256: "goose".into(),
        instructions_sha256: text_digest("do it"),
        plugin_instructions_sha256: text_digest(""),
        plugins: vec![
            PluginIdentity {
                id: "b".into(),
                digest: "2".into(),
                authorization: "x".into(),
            },
            PluginIdentity {
                id: "a".into(),
                digest: "1".into(),
                authorization: "y".into(),
            },
        ],
        routes: vec!["verify".into(), "done".into()],
    }
}

fn tool_call_fact(ok: bool) -> Fact {
    Fact::ToolCall {
        tool: "anchor_run".into(),
        tool_call_id: Some("call_1".into()),
        model_request: 2,
        arguments_shape: arguments_shape(&json!({"command": ["cat", "x"]})),
        started_at_ms: Some(10),
        ended_at_ms: Some(25),
        ok,
        failure: (!ok).then_some(Failure::NotAuthorized),
    }
}

fn tool_call(ok: bool) -> Observation {
    Observation {
        version: VERSION,
        invocation: invocation(),
        facts: vec![tool_call_fact(ok)],
    }
}

fn kernel() -> Kernel<MemoryStore> {
    let mut kernel = Kernel::new(MemoryStore::default(), Box::new(RecorderOnly)).unwrap();
    install(&mut kernel).unwrap();
    kernel
        .create(Resource::new(invocation().trace(), OBSERVATION, Value::Null).configured("cfg"))
        .unwrap();
    kernel
}

fn record(kernel: &Kernel<MemoryStore>, principal: &Principal, content: Value) -> Reason {
    let seen = kernel.load(&invocation().trace()).unwrap().unwrap().version;
    kernel
        .submit(
            principal,
            0,
            Submission::new(invocation().trace(), seen, Action::Write).content(content),
        )
        .unwrap()
        .reason
}

#[test]
fn the_recorder_records_observations_of_the_named_invocation() {
    let kernel = kernel();
    let content = serde_json::to_value(tool_call(false)).unwrap();
    assert_eq!(
        record(&kernel, &recorder("cfg"), content.clone()),
        Reason::Ok
    );
    let history = kernel.history(&invocation().trace()).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].verdict.effect, Effect::Accepted);
    assert_eq!(history[0].stamp.config(), Some("cfg"));
    let read: Observation = serde_json::from_value(history[0].submission.content.clone()).unwrap();
    assert_eq!(read, tool_call(false));
    assert_eq!(content["facts"][0]["kind"], "tool_call");
    // The trace stays empty; only its history grows.
    assert_eq!(
        kernel.load(&invocation().trace()).unwrap().unwrap().state,
        Value::Null
    );
}

#[test]
fn only_the_recorder_may_record_and_only_well_formed_facts_of_its_trace() {
    let kernel = kernel();
    let content = serde_json::to_value(tool_call(true)).unwrap();
    let agent = Principal::new(RECORDER_ID, PrincipalKind::Agent);
    assert_eq!(
        record(&kernel, &agent, content.clone()),
        Reason::Unauthorized
    );
    let other = Principal::new("anchor/host", PrincipalKind::Code);
    assert_eq!(
        record(&kernel, &other, content.clone()),
        Reason::Unauthorized
    );

    let mut elsewhere = tool_call(true);
    elsewhere.invocation.invocation = 2;
    assert_eq!(
        record(
            &kernel,
            &recorder("cfg"),
            serde_json::to_value(elsewhere).unwrap()
        ),
        Reason::ContentInvalid
    );
    let mut future = content.clone();
    future["version"] = json!(VERSION + 1);
    assert_eq!(
        record(&kernel, &recorder("cfg"), future),
        Reason::ContentInvalid
    );
    assert_eq!(
        record(&kernel, &recorder("cfg"), json!({"kind": "tool_call"})),
        Reason::ContentInvalid
    );
    let mut empty = tool_call(true);
    empty.facts.clear();
    assert_eq!(
        record(
            &kernel,
            &recorder("cfg"),
            serde_json::to_value(empty).unwrap()
        ),
        Reason::ContentInvalid
    );
}

#[test]
fn configuration_manifests_are_content_addressed_and_immutable() {
    let mut kernel = Kernel::new(MemoryStore::default(), Box::new(AllowAll)).unwrap();
    install(&mut kernel).unwrap();
    let manifest = configuration();
    let digest = manifest.digest();
    kernel
        .create(
            Resource::new(
                configuration_id(&digest),
                CONFIGURATION,
                serde_json::to_value(&manifest).unwrap(),
            )
            .configured(&digest),
        )
        .unwrap();
    let verdict = kernel
        .submit(
            &Principal::new("owner", PrincipalKind::Human),
            0,
            Submission::new(configuration_id(&digest), 0, Action::Write).content(json!({})),
        )
        .unwrap();
    assert_eq!(verdict.reason, Reason::Unauthorized);

    // Order of Plugins and routes does not change the identity; content does.
    let mut reordered = configuration();
    reordered.plugins.reverse();
    reordered.routes.reverse();
    assert_eq!(reordered.digest(), digest);
    let mut changed = configuration();
    changed.model = "pro".into();
    assert_ne!(changed.digest(), digest);
}

struct AllowAll;
impl Authority for AllowAll {
    fn permits(&self, _: &Principal, _: Action, _: &str) -> bool {
        true
    }
}

#[test]
fn argument_shapes_ignore_values_but_not_structure() {
    let a = arguments_shape(&json!({"command": ["cat", "a"], "cwd": "/x"}));
    let b = arguments_shape(&json!({"cwd": "/y", "command": ["ls"]}));
    let c = arguments_shape(&json!({"command": ["cat", "a"]}));
    let d = arguments_shape(&json!({"command": "cat a"}));
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert_ne!(c, d);
}

#[test]
fn failures_are_classified_from_the_host_refusal_wording() {
    let cases = [
        (
            "tool failed: anchor_run not_executed: command `find` is not authorized by the host sandbox policy",
            Failure::NotAuthorized,
        ),
        (
            "anchor_run not_executed: invalid arguments: absolute read paths must belong to an authorized read-only input",
            Failure::NotAuthorized,
        ),
        (
            "anchor_run not_executed: invalid arguments: missing field `command`",
            Failure::InvalidArguments,
        ),
        (
            "final_result.observed_receipt must match an anchor_receipt",
            Failure::CompletionRejected,
        ),
        ("Anchor invocation was cancelled", Failure::Cancelled),
        (
            "tool failed: anchor_run not_executed: busy",
            Failure::NotExecuted,
        ),
        ("exit status 2", Failure::ToolError),
    ];
    for (error, class) in cases {
        assert_eq!(classify_failure(error), class, "{error}");
    }
}

#[test]
fn every_fact_kind_round_trips_with_its_tag() {
    let facts = [
        Fact::Lifecycle {
            started_at_ms: 1,
            ended_at_ms: 9,
            outcome: Outcome::Interrupted,
            model_turns: 2,
            tool_calls: 3,
            tool_calls_dropped: 0,
            resumed: true,
        },
        Fact::ModelTurn {
            index: 1,
            message_id: Some("m".into()),
            observed_at_ms: Some(5),
            elapsed_ms: Some(4),
            time_to_first_token_ms: None,
            input_tokens: 10,
            output_tokens: 2,
            total_tokens: 12,
            cache_read_tokens: Some(8),
            cache_write_tokens: None,
            cost_usd: Some(0.001),
            compaction: false,
        },
        tool_call_fact(true),
    ];
    let observation = Observation {
        version: VERSION,
        invocation: invocation(),
        facts: facts.to_vec(),
    };
    let value = serde_json::to_value(&observation).unwrap();
    let kinds: Vec<_> = value["facts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|fact| fact["kind"].clone())
        .collect();
    assert_eq!(kinds, vec!["lifecycle", "model_turn", "tool_call"]);
    assert_eq!(
        serde_json::from_value::<Observation>(value).unwrap(),
        observation
    );
}

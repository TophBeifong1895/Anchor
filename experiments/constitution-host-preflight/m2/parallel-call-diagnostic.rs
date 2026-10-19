use anchor_runtime::graph::{*, validator::*};
use anchor_kernel::*;
use std::sync::Arc;
struct AuthorityFixture;
impl Authority for AuthorityFixture {
    fn permits(&self, _: &Principal, _: Action, _: &str) -> bool { true }
}
fn main() {
    let definition = serde_json::json!({
        "objective":"parallel Call boundary", "entry":"fork", "agents":{},
        "ops":{"fork":{"fanout":{"join":"join"}}, "call":{"call":{"graph":"child","mode":"wait"}},
            "work":{"run":"true"},"join":{"join":{}}},
        "nodes":[{"id":"fork","op":"fork"},{"id":"call","op":"call"},
            {"id":"work","op":"work"},{"id":"join","op":"join"}],
        "edges":[{"from":"fork","to":"call"},{"from":"fork","to":"work"},
            {"from":"call","to":"join"},{"from":"work","to":"join"}]
    });
    let snapshot:GraphSnapshot=serde_json::from_value(definition).unwrap();
    let record = GraphRunRecord::create_with_id(snapshot, serde_json::Value::Null,"parallel-call-diagnostic").unwrap();
    let resource=run_resource(&record,u64::MAX).unwrap();
    let id=resource.id.clone();
    let mut kernel=Kernel::new(MemoryStore::default(),Box::new(AuthorityFixture)).unwrap();
    kernel.install(GRAPH_VALIDATOR,Arc::new(GraphValidator)).unwrap();
    kernel.create(resource).unwrap();
    let resource=kernel.load(&id).unwrap().unwrap();
    let fork=GraphValidator::frontier(&resource).unwrap().remove(0);
    let content=NodeSubmission {
        summary:"fork".into(),route:None,output:serde_json::json!({}),model_requests:0,
        commit:CommitRef { id:"fork-commit".into(),node_id:"fork".into(),invocation:1 },
        interruption:None,plugin_views:Default::default(),plugin_digests:Default::default(),
    };
    let verdict=kernel.submit(&Principal::new("host",PrincipalKind::Code),0,
        Submission::new(&id,resource.version,Action::Write).slot(fork.slot()).content(serde_json::to_value(content).unwrap())).unwrap();
    assert!(verdict.is_accepted(),"{verdict:?}");
    let resource=kernel.load(&id).unwrap().unwrap();
    let frontier=GraphValidator::frontier(&resource).unwrap();
    for dispatch in &frontier { println!("node={} kind={:?} branch={:?}",dispatch.node.id,dispatch.kind,dispatch.branch); }
    let call=frontier.iter().find(|d|d.node.id=="call").unwrap();
    assert_eq!(call.kind,DispatchKind::Work,"diagnostic: current unsupported combination changed");
    println!("admission accepted; Call branch dispatches as Work, so GraphCallPort and CallSubmission are bypassed");
}

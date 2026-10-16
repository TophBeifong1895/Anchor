//! H1 storage adapter boundary; does not extend GraphValidator.
use anchor_kernel::{
    Action, Authority, FileStore, Kernel, Principal, PrincipalKind, Reason, Submission,
};
use anchor_runtime::graph::validator::{GRAPH_VALIDATOR, GraphValidator, record_of, run_resource};
use anchor_runtime::graph::{GraphNode, GraphRunRecord, GraphSnapshot, RunStatus};
use serde_json::json;
use std::{collections::BTreeMap, error::Error, sync::Arc};
struct Allow;
impl Authority for Allow {
    fn permits(&self, _: &Principal, _: Action, _: &str) -> bool {
        true
    }
}
fn main() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let mut kernel = Kernel::new(FileStore::new(directory.path())?, Box::new(Allow))?;
    kernel.install(GRAPH_VALIDATOR, Arc::new(GraphValidator))?;
    let record = GraphRunRecord::create_with_id(
        GraphSnapshot {
            objective: "H1 storage boundary".into(),
            input: json!({}),
            entry: "work".into(),
            agents: BTreeMap::new(),
            ops: BTreeMap::from([("work".into(), json!({"run": "printf fixture"}))]),
            nodes: vec![GraphNode {
                id: "work".into(),
                agent: None,
                op: Some("work".into()),
                input: None,
                plugins: vec![],
                max_rounds: None,
            }],
            edges: vec![],
            module_rounds: BTreeMap::new(),
        },
        json!({}),
        "h1-boundary",
    )?;
    let resource = run_resource(&record, 100)?;
    let id = resource.id.clone();
    kernel.create(resource)?;
    let code = Principal::new("fixture-host", PrincipalKind::Code);
    let slot = kernel.open(&id)?[0].id.clone();
    let mut started = record.clone();
    started.status = RunStatus::Running;
    started.invocations.insert("work".into(), 1);
    started.passes.insert("work".into(), 1);
    let mut verdicts = vec![];
    for content in [
        serde_json::to_value(started)?,
        json!({"started": true, "invocation": 1}),
    ] {
        let verdict = kernel.submit(
            &code,
            0,
            Submission::new(&id, 0, Action::Write)
                .slot(slot.clone())
                .content(content),
        )?;
        assert_eq!(verdict.reason, Reason::ContentInvalid);
        let after = kernel.load(&id)?.ok_or("missing Run")?;
        assert_eq!(after.version, 0);
        assert_eq!(record_of(&after)?, record);
        verdicts.push(verdict);
    }
    assert_eq!(kernel.history(&id)?.len(), 2);
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "kind": "h1_stage_boundary", "status": "boundary_reproduced",
            "storage": "anchor_kernel::FileStore in owned tempfile",
            "whole_run_snapshot": verdicts[0], "dispatch_started_fact": verdicts[1],
            "state_unchanged": true, "history_records": 2,
            "conclusion": "Runner save cannot become a generic submit adapter under the current GraphValidator; this is a stage dependency, not a kernel correctness defect.",
            "host_execution": "not_run", "real_provider": "not_used"
        }))?
    );
    Ok(())
}

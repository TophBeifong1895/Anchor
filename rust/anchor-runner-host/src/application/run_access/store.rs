use crate::application::RunApplication;
use anchor_runtime::graph::{FileRunLeaseProvider, GraphRunRecord};

impl RunApplication {
    pub(in crate::application::run_access) fn run_leases(&self) -> FileRunLeaseProvider {
        FileRunLeaseProvider::new(self.data_root.join("run-locks"))
    }
}

pub(super) fn wait_child_ids(record: &GraphRunRecord) -> Vec<String> {
    record
        .graph_calls
        .values()
        .filter(|call| call.mode == "wait")
        .filter_map(|call| call.child_run_id.clone())
        .collect()
}

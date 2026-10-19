use crate::application::RunApplication;
use anchor_runtime::graph::{FileRunStore, GraphRunRecord};

impl RunApplication {
    pub(in crate::application) fn store(&self) -> FileRunStore {
        FileRunStore::new(self.data_root.join("runs"))
    }
}

pub(in crate::application) fn wait_child_ids(record: &GraphRunRecord) -> Vec<String> {
    record
        .graph_calls
        .values()
        .filter(|call| call.mode == "wait")
        .filter_map(|call| call.child_run_id.clone())
        .collect()
}

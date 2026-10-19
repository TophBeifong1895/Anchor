//! Kernel resource fixtures for node port authorization tests. These fixtures
//! never create a second execution store or lifecycle implementation.
use crate::application::{RunApplication, run_access::fixtures};
use anchor_kernel::FileStore;
use anchor_runtime::graph::{GraphRunRecord, validator::run_resource_id};
use std::path::Path;

pub(crate) fn application(root: &Path) -> RunApplication {
    RunApplication::new(root.to_path_buf(), root.to_path_buf())
}

pub(crate) fn seed(root: &Path, record: &GraphRunRecord) {
    fixtures::create(&application(root), record);
}

pub(crate) fn remove(root: &Path, id: &str) {
    FileStore::new(root.join("kernel-runs"))
        .unwrap()
        .remove(&run_resource_id(id))
        .unwrap();
}

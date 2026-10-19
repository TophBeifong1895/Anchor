use super::RunView;
use crate::{application::RunMetadata, assistant::TurnBinding};
use anchor_runtime::graph::{FileRunStore, GraphError, InvocationKey, RunLease, RunStore};
use std::path::Path;

pub(crate) fn graph_lease(root: &Path, source: &Path) -> Result<Box<dyn RunLease>, GraphError> {
    FileRunStore::new(root.join("admission-locks"))
        .acquire_lease(&crate::assistant_plugins::graph_lease_key(source)?)
}

pub(crate) fn graph_read_lease(
    root: &Path,
    source: &Path,
) -> Result<Box<dyn RunLease>, GraphError> {
    FileRunStore::new(root.join("admission-locks"))
        .acquire_read_lease(&crate::assistant_plugins::graph_lease_key(source)?)
}

impl RunView<'_> {
    pub(crate) fn executable_assistant_binding(
        &self,
        root: &Path,
        metadata: &RunMetadata,
    ) -> Result<Option<TurnBinding>, String> {
        let Some(source) = &metadata.assistant else {
            return Ok(None);
        };
        let Some(cursor) = self.cursor() else {
            return Ok(None);
        };
        let mut binding = crate::assistant::binding(root, &cursor.key)?;
        if binding.is_none() && cursor.node_id != source.wait_node {
            for commit in &cursor.input_commits {
                let key = InvocationKey {
                    node_id: commit.node_id.clone(),
                    invocation: commit.invocation,
                    ..cursor.key.clone()
                };
                if let Some(prior) = crate::assistant::binding(root, &key)? {
                    binding = Some(prior);
                    break;
                }
            }
        }
        if binding.is_none() && cursor.node_id != source.wait_node {
            return Err("assistant active invocation has no Turn binding".into());
        }
        Ok(binding)
    }

    pub(crate) fn latest_assistant_binding(
        &self,
        root: &Path,
    ) -> Result<Option<TurnBinding>, String> {
        let last = self
            .record
            .results
            .values()
            .flatten()
            .max_by_key(|result| result.sequence);
        let cursor_binding = self
            .cursor()
            .map(|cursor| crate::assistant::binding(root, &cursor.key))
            .transpose()?
            .flatten();
        match cursor_binding {
            Some(binding) => Ok(Some(binding)),
            None => last
                .map(|result| crate::assistant::binding(root, &result.key))
                .transpose()
                .map(Option::flatten),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::metadata::AssistantSource;
    use anchor_runtime::graph::{
        CommitRef, GraphRunRecord, GraphSnapshot, NodeCompletion, RunCursor, RunResult,
    };
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::path::PathBuf;

    fn fixture() -> (tempfile::TempDir, GraphRunRecord, RunMetadata) {
        let root = tempfile::tempdir().unwrap();
        let snapshot = GraphSnapshot::from_authoring(json!({
            "objective":"assistant", "entry":"wait",
            "agents":{"worker":{"model":"fixture","instructions":"work"}},
            "ops":{"wait":{"host":{"operation":"session.wait_input"}},"reply":{"host":{"operation":"session.reply"}}},
            "nodes":[{"id":"wait","op":"wait"},{"id":"agent","agent":"worker"},{"id":"reply","op":"reply"}],
            "edges":[{"from":"wait","to":"agent"},{"from":"agent","to":"reply"},{"from":"reply","to":"wait"}]
        }))
        .unwrap();
        let mut record =
            GraphRunRecord::create_with_id(snapshot, json!({}), "assistant-policy-test").unwrap();
        let key = invocation(&record, "agent", 1);
        record.cursor = Some(RunCursor {
            node_id: key.node_id.clone(),
            key,
            input_commits: Vec::new(),
            prepared_input: json!({}),
        });
        let mut metadata = RunMetadata::new(
            record.run_id.clone(),
            "assistant".into(),
            record.graph_digest.clone(),
            root.path(),
        )
        .unwrap();
        metadata.assistant = Some(AssistantSource {
            owner: "owner".into(),
            session: "session".into(),
            wait_node: "wait".into(),
            work_node: "agent".into(),
            reply_node: "reply".into(),
        });
        (root, record, metadata)
    }

    fn invocation(record: &GraphRunRecord, node: &str, number: u64) -> InvocationKey {
        InvocationKey {
            run_id: record.run_id.clone(),
            graph_digest: record.graph_digest.clone(),
            node_id: node.into(),
            invocation: number,
        }
    }

    fn commit(key: &InvocationKey) -> CommitRef {
        CommitRef {
            id: format!("commit-{}-{}", key.node_id, key.invocation),
            node_id: key.node_id.clone(),
            invocation: key.invocation,
        }
    }

    fn binding_path(root: &Path, key: &InvocationKey) -> PathBuf {
        root.join("assistant-invocations").join(format!(
            "{:x}.json",
            Sha256::digest(key.durable_key().as_bytes())
        ))
    }

    fn save_binding(root: &Path, key: &InvocationKey, turn: &str) -> TurnBinding {
        let binding = TurnBinding {
            key: key.clone(),
            wait_key: "wait".into(),
            inbound: "inbound".into(),
            turn: turn.into(),
            pending_inbounds: Vec::new(),
            pending_truncated: false,
        };
        crate::assistant::save_immutable(&binding_path(root, key), &binding).unwrap();
        binding
    }

    fn complete(record: &mut GraphRunRecord, key: &InvocationKey, sequence: u64) {
        record
            .results
            .entry(key.node_id.clone())
            .or_default()
            .push(RunResult {
                plugin_views: Default::default(),
                plugin_digests: Default::default(),
                node_id: key.node_id.clone(),
                key: key.clone(),
                completion: NodeCompletion {
                    submission: "fixture".into(),
                    route: None,
                    model_requests: 0,
                    output: json!({}),
                },
                interruption: None,
                commit: commit(key),
                sequence,
            });
    }

    #[test]
    fn executable_binding_prefers_cursor_then_first_bound_input_commit() {
        let (root, mut record, metadata) = fixture();
        let current = record.cursor.as_ref().unwrap().key.clone();
        let missing = invocation(&record, "reply", 99);
        let first = invocation(&record, "wait", 1);
        let second = invocation(&record, "agent", 2);
        let expected = save_binding(root.path(), &first, "first");
        save_binding(root.path(), &second, "second");
        record.cursor.as_mut().unwrap().input_commits =
            vec![commit(&missing), commit(&first), commit(&second)];
        assert_eq!(
            RunView::from(&record)
                .executable_assistant_binding(root.path(), &metadata)
                .unwrap(),
            Some(expected)
        );
        let expected = save_binding(root.path(), &current, "current");
        std::fs::write(binding_path(root.path(), &first), b"{invalid").unwrap();
        assert_eq!(
            RunView::from(&record)
                .executable_assistant_binding(root.path(), &metadata)
                .unwrap(),
            Some(expected)
        );
    }

    #[test]
    fn executable_binding_requires_work_binding_but_does_not_scan_idle_history() {
        let (root, mut record, mut metadata) = fixture();
        let prior = invocation(&record, "agent", 99);
        save_binding(root.path(), &prior, "prior");
        complete(&mut record, &prior, 1);
        assert_eq!(
            RunView::from(&record)
                .executable_assistant_binding(root.path(), &metadata)
                .unwrap_err(),
            "assistant active invocation has no Turn binding"
        );
        let wait = invocation(&record, "wait", 2);
        let cursor = record.cursor.as_mut().unwrap();
        cursor.node_id = wait.node_id.clone();
        cursor.key = wait;
        cursor.input_commits = vec![commit(&prior)];
        assert_eq!(
            RunView::from(&record)
                .executable_assistant_binding(root.path(), &metadata)
                .unwrap(),
            None
        );
        record.cursor = None;
        assert_eq!(
            RunView::from(&record)
                .executable_assistant_binding(root.path(), &metadata)
                .unwrap(),
            None
        );
        metadata.assistant = None;
        assert_eq!(
            RunView::from(&record)
                .executable_assistant_binding(root.path(), &metadata)
                .unwrap(),
            None
        );
    }

    #[test]
    fn latest_binding_prefers_cursor_then_highest_sequence_result() {
        let (root, mut record, _) = fixture();
        let current = record.cursor.as_ref().unwrap().key.clone();
        let earlier = invocation(&record, "reply", 2);
        let latest = invocation(&record, "wait", 3);
        save_binding(root.path(), &earlier, "earlier");
        let latest_binding = save_binding(root.path(), &latest, "latest");
        complete(&mut record, &latest, 9);
        complete(&mut record, &earlier, 2);
        assert_eq!(
            RunView::from(&record)
                .latest_assistant_binding(root.path())
                .unwrap(),
            Some(latest_binding.clone())
        );
        let current_binding = save_binding(root.path(), &current, "current");
        std::fs::write(binding_path(root.path(), &latest), b"{invalid").unwrap();
        assert_eq!(
            RunView::from(&record)
                .latest_assistant_binding(root.path())
                .unwrap(),
            Some(current_binding)
        );
        record.cursor = None;
        assert!(
            RunView::from(&record)
                .latest_assistant_binding(root.path())
                .is_err()
        );
        std::fs::write(
            binding_path(root.path(), &latest),
            serde_json::to_vec(&latest_binding).unwrap(),
        )
        .unwrap();
        assert_eq!(
            RunView::from(&record)
                .latest_assistant_binding(root.path())
                .unwrap(),
            Some(latest_binding)
        );
    }

    #[test]
    fn latest_binding_does_not_search_older_results_or_input_commits() {
        let (root, mut record, _) = fixture();
        let earlier = invocation(&record, "wait", 1);
        let latest = invocation(&record, "reply", 2);
        save_binding(root.path(), &earlier, "earlier");
        record.cursor.as_mut().unwrap().input_commits = vec![commit(&earlier)];
        complete(&mut record, &earlier, 1);
        complete(&mut record, &latest, 2);
        assert_eq!(
            RunView::from(&record)
                .latest_assistant_binding(root.path())
                .unwrap(),
            None
        );
    }

    #[test]
    fn corrupt_cursor_binding_is_not_masked_by_commits_or_history() {
        let (root, mut record, metadata) = fixture();
        let current = record.cursor.as_ref().unwrap().key.clone();
        let prior = invocation(&record, "wait", 1);
        save_binding(root.path(), &current, "current");
        save_binding(root.path(), &prior, "prior");
        record.cursor.as_mut().unwrap().input_commits = vec![commit(&prior)];
        complete(&mut record, &prior, 1);
        std::fs::write(binding_path(root.path(), &current), b"{invalid").unwrap();
        let expected = crate::assistant::binding(root.path(), &current).unwrap_err();
        let view = RunView::from(&record);
        assert_eq!(
            view.executable_assistant_binding(root.path(), &metadata)
                .unwrap_err(),
            expected
        );
        assert_eq!(
            view.latest_assistant_binding(root.path()).unwrap_err(),
            expected
        );
    }

    #[test]
    fn corrupt_input_binding_stops_the_ordered_commit_scan() {
        let (root, mut record, metadata) = fixture();
        let first = invocation(&record, "wait", 1);
        let second = invocation(&record, "reply", 2);
        save_binding(root.path(), &first, "first");
        save_binding(root.path(), &second, "second");
        record.cursor.as_mut().unwrap().input_commits = vec![commit(&first), commit(&second)];
        std::fs::write(binding_path(root.path(), &first), b"{invalid").unwrap();
        assert_eq!(
            RunView::from(&record)
                .executable_assistant_binding(root.path(), &metadata)
                .unwrap_err(),
            crate::assistant::binding(root.path(), &first).unwrap_err()
        );
    }

    #[test]
    fn graph_read_leases_share_identity_and_exclude_writers() {
        let root = tempfile::tempdir().unwrap();
        let first = graph_read_lease(root.path(), root.path()).unwrap();
        let second = graph_read_lease(root.path(), &root.path().join(".")).unwrap();
        assert!(graph_lease(root.path(), root.path()).is_err());
        drop(first);
        assert!(graph_lease(root.path(), root.path()).is_err());
        drop(second);
        let writer = graph_lease(root.path(), root.path()).unwrap();
        assert!(graph_read_lease(root.path(), root.path()).is_err());
        drop(writer);
        assert!(graph_read_lease(root.path(), root.path()).is_ok());
    }
}

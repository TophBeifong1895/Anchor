use crate::application::{ApplicationError, RunApplication, RunMetadata};
use anchor_runtime::graph::{CommitRef, GraphRunRecord, InvocationKey, RunStatus, RunStore};
use serde_json::Value;
use std::collections::BTreeMap;

pub(crate) struct ChannelFileSnapshot {
    pub(crate) commit: Option<CommitRef>,
}

pub(crate) struct ChannelReplyResult {
    pub(crate) key: InvocationKey,
    pub(crate) text: String,
    pub(crate) suppressed: bool,
    pub(crate) reply_for: Option<String>,
    pub(crate) source_commit: Value,
    pub(crate) items: Value,
}

pub(crate) struct ChannelReplyRun {
    replies: BTreeMap<String, Vec<ChannelReplyResult>>,
    pub(crate) completed: bool,
    pub(crate) unsuccessful: bool,
    pub(crate) assistant_expects_executor: bool,
}

impl ChannelReplyRun {
    fn from_record(record: GraphRunRecord) -> Self {
        let view = super::RunView::inactive(&record);
        Self {
            completed: view.has_succeeded(),
            unsuccessful: view.has_stopped_execution() && !view.has_succeeded(),
            assistant_expects_executor: view.occupies_conversation_execution(),
            replies: record
                .results
                .into_iter()
                .map(|(node, results)| {
                    let replies = results
                        .into_iter()
                        .map(|result| ChannelReplyResult {
                            suppressed: result.completion.output.get("suppressed")
                                == Some(&Value::Bool(true)),
                            reply_for: result
                                .completion
                                .output
                                .get("reply_for")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            source_commit: result.completion.output["source_commit"].clone(),
                            items: result.completion.output["items"].clone(),
                            key: result.key,
                            text: result.completion.submission,
                        })
                        .collect();
                    (node, replies)
                })
                .collect(),
        }
    }

    pub(crate) fn replies_for(&self, node: &str) -> Option<&[ChannelReplyResult]> {
        self.replies.get(node).map(Vec::as_slice)
    }

    pub(crate) fn latest_reply(&self, node: &str) -> Option<&str> {
        self.replies
            .get(node)
            .and_then(|replies| replies.last())
            .map(|reply| reply.text.as_str())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum StartupChannelSettlement {
    Complete,
    Fail,
    Abort,
    Stop,
}

pub(crate) struct ChannelStartupRun {
    pub(crate) run_id: String,
    assistant_graph_digest: Option<String>,
    settlement: Option<StartupChannelSettlement>,
}

impl ChannelStartupRun {
    pub(crate) fn preserves_assistant(&self, metadata: &RunMetadata) -> bool {
        self.assistant_graph_digest.as_deref() == Some(metadata.graph_digest.as_str())
    }
}

impl RunApplication {
    fn load_channel_run(&self, run: &str) -> Result<Option<GraphRunRecord>, String> {
        self.store().load(run).map_err(|error| error.to_string())
    }

    pub(crate) fn channel_run_is_admitted(&self, run: &str) -> Result<bool, String> {
        self.load_channel_run(run).map(|record| record.is_some())
    }

    pub(crate) fn channel_file_snapshot(
        &self,
        run: &str,
        node: &str,
    ) -> Result<Option<ChannelFileSnapshot>, String> {
        Ok(self
            .load_channel_run(run)?
            .map(|record| ChannelFileSnapshot {
                commit: record
                    .results
                    .get(node)
                    .and_then(|results| results.last())
                    .map(|result| result.commit.clone()),
            }))
    }

    pub(crate) fn channel_reply_is_completed(&self, run: &str) -> Result<Option<bool>, String> {
        Ok(self
            .load_channel_run(run)?
            .map(|record| super::RunView::inactive(&record).has_succeeded()))
    }

    pub(crate) fn channel_reply_run(&self, run: &str) -> Result<Option<ChannelReplyRun>, String> {
        Ok(self
            .load_channel_run(run)?
            .map(ChannelReplyRun::from_record))
    }

    pub(crate) fn assistant_progress_work_invocation(
        &self,
        run: &str,
        work_node: &str,
    ) -> Result<Option<InvocationKey>, String> {
        let Some(record) = self.load_channel_run(run)? else {
            return Ok(None);
        };
        if let Some(cursor) = record.cursor.as_ref()
            && cursor.node_id == work_node
        {
            return Ok(Some(cursor.key.clone()));
        }
        Ok(record
            .results
            .get(work_node)
            .and_then(|results| results.last())
            .map(|result| result.key.clone()))
    }

    pub(crate) fn channel_progress_status(&self, run: &str) -> String {
        match self.load_channel_run(run) {
            Ok(Some(record)) => serde_json::to_value(record.status)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "missing".to_owned()),
            _ => "missing".to_owned(),
        }
    }

    pub(crate) fn channel_progress_is_settled(&self, run: &str) -> bool {
        match self.load_channel_run(run) {
            Ok(Some(record)) => {
                super::RunView::inactive(&record).has_stopped_execution()
                    || record.status == RunStatus::BudgetStopped
            }
            _ => true,
        }
    }

    pub(crate) fn channel_startup_runs(&self) -> Result<Vec<ChannelStartupRun>, ApplicationError> {
        Ok(self
            .records()?
            .into_iter()
            .map(|(run_id, record)| ChannelStartupRun {
                run_id,
                assistant_graph_digest: (!super::RunView::inactive(&record)
                    .has_stopped_execution())
                .then_some(record.graph_digest),
                settlement: match record.status {
                    RunStatus::Completed => Some(StartupChannelSettlement::Complete),
                    RunStatus::Failed => Some(StartupChannelSettlement::Fail),
                    RunStatus::Aborted => Some(StartupChannelSettlement::Abort),
                    RunStatus::Stopped => Some(StartupChannelSettlement::Stop),
                    _ => None,
                },
            })
            .collect())
    }

    pub(crate) fn settle_startup_channel_run(
        &self,
        run: &ChannelStartupRun,
    ) -> Result<(), ApplicationError> {
        let status = match run.settlement {
            Some(StartupChannelSettlement::Complete) => RunStatus::Completed,
            Some(StartupChannelSettlement::Fail) => RunStatus::Failed,
            Some(StartupChannelSettlement::Abort) => RunStatus::Aborted,
            Some(StartupChannelSettlement::Stop) => RunStatus::Stopped,
            None => return Ok(()),
        };
        self.settle_channel_run(&run.run_id, status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_runtime::graph::{
        NodeCompletion, PendingRecovery, RecoveryAttempt, RunCursor, RunResult,
    };
    use serde_json::json;

    fn record() -> GraphRunRecord {
        GraphRunRecord::create_with_id(
            serde_json::from_value(json!({
                "objective":"test", "entry":"work",
                "agents":{"role":{"model":"fixture","instructions":"work"}},
                "nodes":[{"id":"work","agent":"role"}], "edges":[]
            }))
            .unwrap(),
            json!({}),
            "channel-facade-test",
        )
        .unwrap()
    }

    fn key(record: &GraphRunRecord, invocation: u64) -> InvocationKey {
        InvocationKey {
            run_id: record.run_id.clone(),
            graph_digest: record.graph_digest.clone(),
            node_id: "work".into(),
            invocation,
        }
    }

    #[test]
    fn channel_queries_preserve_each_legacy_state_and_startup_action() {
        use StartupChannelSettlement::{Abort, Complete, Fail, Stop};

        let root = tempfile::tempdir().unwrap();
        let application = RunApplication::new(root.path().into(), root.path().join("catalog"));
        for (status, wire, completed, unsuccessful, executor, settled, settlement) in [
            (RunStatus::Ready, "ready", false, false, true, false, None),
            (
                RunStatus::Running,
                "running",
                false,
                false,
                true,
                false,
                None,
            ),
            (
                RunStatus::Paused,
                "paused",
                false,
                false,
                false,
                false,
                None,
            ),
            (
                RunStatus::BudgetStopped,
                "budget_stopped",
                false,
                false,
                false,
                true,
                None,
            ),
            (
                RunStatus::WaitingCall,
                "waiting_call",
                false,
                false,
                false,
                false,
                None,
            ),
            (
                RunStatus::WaitingRecovery,
                "waiting_recovery",
                false,
                false,
                false,
                false,
                None,
            ),
            (
                RunStatus::Completed,
                "completed",
                true,
                false,
                false,
                true,
                Some(Complete),
            ),
            (
                RunStatus::Failed,
                "failed",
                false,
                true,
                false,
                true,
                Some(Fail),
            ),
            (
                RunStatus::Aborted,
                "aborted",
                false,
                true,
                false,
                true,
                Some(Abort),
            ),
            (
                RunStatus::Stopped,
                "stopped",
                false,
                true,
                false,
                true,
                Some(Stop),
            ),
        ] {
            let mut record = record();
            record.status = status;
            if matches!(
                status,
                RunStatus::BudgetStopped | RunStatus::WaitingRecovery
            ) {
                let key = key(&record, 1);
                record.invocations.insert("work".into(), 1);
                record.passes.insert("work".into(), 1);
                record.cursor = Some(RunCursor {
                    node_id: "work".into(),
                    key: key.clone(),
                    input_commits: Vec::new(),
                    prepared_input: Value::Null,
                });
                if status == RunStatus::WaitingRecovery {
                    record.recovery.push(PendingRecovery {
                        key,
                        attempt: RecoveryAttempt {
                            attempt_id: 1,
                            step: 1,
                            tool: "fixture-effect".into(),
                            started_at: "2026-10-19T00:00:00+00:00".into(),
                        },
                    });
                }
            }
            application.store().save(&record).unwrap();
            let run = record.run_id.as_str();
            let reply = application.channel_reply_run(run).unwrap().unwrap();
            assert_eq!(reply.completed, completed, "{wire}");
            assert_eq!(reply.unsuccessful, unsuccessful, "{wire}");
            assert_eq!(reply.assistant_expects_executor, executor, "{wire}");
            assert_eq!(
                application.channel_reply_is_completed(run).unwrap(),
                Some(completed)
            );
            assert_eq!(
                application.channel_progress_is_settled(run),
                settled,
                "{wire}"
            );
            assert_eq!(application.channel_progress_status(run), wire);
            assert!(application.channel_run_is_admitted(run).unwrap());
            let candidates = application.channel_startup_runs().unwrap();
            assert_eq!(candidates.len(), 1);
            assert_eq!(candidates[0].run_id, run);
            assert_eq!(candidates[0].settlement, settlement, "{wire}");
            let mut metadata = RunMetadata::new(
                run.into(),
                "fixture".into(),
                record.graph_digest.clone(),
                root.path(),
            )
            .unwrap();
            assert_eq!(
                candidates[0].preserves_assistant(&metadata),
                !completed && !unsuccessful,
                "{wire}"
            );
            metadata.graph_digest = "another-graph".into();
            assert!(!candidates[0].preserves_assistant(&metadata));
        }
    }

    #[test]
    fn missing_and_unreadable_runs_keep_the_channel_error_policies() {
        let root = tempfile::tempdir().unwrap();
        let application = RunApplication::new(root.path().into(), root.path().join("catalog"));
        let run = "channel-facade-test";
        assert!(!application.channel_run_is_admitted(run).unwrap());
        assert!(application.channel_reply_run(run).unwrap().is_none());
        assert!(
            application
                .channel_reply_is_completed(run)
                .unwrap()
                .is_none()
        );
        assert!(
            application
                .channel_file_snapshot(run, "work")
                .unwrap()
                .is_none()
        );
        assert!(
            application
                .assistant_progress_work_invocation(run, "work")
                .unwrap()
                .is_none()
        );
        assert!(application.channel_progress_is_settled(run));
        assert_eq!(application.channel_progress_status(run), "missing");
        assert!(application.channel_startup_runs().unwrap().is_empty());
        std::fs::create_dir_all(root.path().join("runs")).unwrap();
        std::fs::write(root.path().join(format!("runs/{run}.json")), b"not JSON").unwrap();
        let expected = application.store().load(run).unwrap_err().to_string();
        assert_eq!(
            application.channel_run_is_admitted(run).err(),
            Some(expected.clone())
        );
        assert_eq!(
            application.channel_reply_run(run).err(),
            Some(expected.clone())
        );
        assert_eq!(
            application.channel_reply_is_completed(run).err(),
            Some(expected.clone())
        );
        assert_eq!(
            application.channel_file_snapshot(run, "work").err(),
            Some(expected.clone())
        );
        assert_eq!(
            application
                .assistant_progress_work_invocation(run, "work")
                .err(),
            Some(expected)
        );
        assert!(application.channel_progress_is_settled(run));
        assert_eq!(application.channel_progress_status(run), "missing");
        assert!(application.channel_startup_runs().is_err());
    }

    #[test]
    fn reply_and_file_projections_keep_result_order_and_cursor_fallback() {
        let root = tempfile::tempdir().unwrap();
        let application = RunApplication::new(root.path().into(), root.path().join("catalog"));
        let mut record = record();
        record.status = RunStatus::Running;
        let results = (1..=2)
            .map(|invocation| RunResult {
                node_id: "work".into(),
                key: key(&record, invocation),
                plugin_views: Default::default(),
                plugin_digests: Default::default(),
                interruption: None,
                completion: NodeCompletion {
                    submission: format!("  reply {invocation}  "),
                    route: None,
                    model_requests: 0,
                    output: json!({"suppressed":true,"reply_for":"turn","source_commit":"invalid-until-used","items":[{"type":"image"}]}),
                },
                commit: CommitRef { id: format!("commit-{invocation}"), node_id: "work".into(), invocation },
                sequence: invocation,
            })
            .collect();
        record.results.insert("work".into(), results);
        record.sequence = 2;
        record.invocations.insert("work".into(), 3);
        record.passes.insert("work".into(), 3);
        let current = key(&record, 3);
        record.cursor = Some(RunCursor {
            node_id: "work".into(),
            key: current.clone(),
            input_commits: Vec::new(),
            prepared_input: Value::Null,
        });
        application.store().save(&record).unwrap();
        let run = record.run_id.as_str();
        assert_eq!(
            application
                .assistant_progress_work_invocation(run, "work")
                .unwrap(),
            Some(current)
        );
        assert!(
            application
                .assistant_progress_work_invocation(run, "other")
                .unwrap()
                .is_none()
        );
        let reply = application.channel_reply_run(run).unwrap().unwrap();
        let replies = reply.replies_for("work").unwrap();
        assert_eq!(
            replies
                .iter()
                .map(|reply| reply.key.invocation)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(reply.latest_reply("work"), Some("  reply 2  "));
        assert!(replies[1].suppressed);
        assert_eq!(replies[1].reply_for.as_deref(), Some("turn"));
        assert_eq!(replies[1].source_commit, json!("invalid-until-used"));
        assert_eq!(replies[1].items, json!([{"type":"image"}]));
        assert_eq!(
            application
                .channel_file_snapshot(run, "work")
                .unwrap()
                .unwrap()
                .commit,
            Some(record.results["work"][1].commit.clone())
        );
        assert!(
            application
                .channel_file_snapshot(run, "other")
                .unwrap()
                .unwrap()
                .commit
                .is_none()
        );
        record.cursor = None;
        application.store().save(&record).unwrap();
        assert_eq!(
            application
                .assistant_progress_work_invocation(run, "work")
                .unwrap(),
            Some(key(&record, 2))
        );
    }
}

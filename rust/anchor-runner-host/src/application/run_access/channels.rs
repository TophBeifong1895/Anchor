use crate::application::{ApplicationError, RunApplication, RunMetadata};
use anchor_runtime::graph::{CommitRef, InvocationKey};
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
    /// The Host answered this Turn itself (a provider rejection); it carries
    /// no Agent images.
    pub(crate) host_reply: bool,
}

pub(crate) struct ChannelReplyRun {
    replies: BTreeMap<String, Vec<ChannelReplyResult>>,
    pub(crate) completed: bool,
    pub(crate) unsuccessful: bool,
    pub(crate) assistant_expects_executor: bool,
}

impl ChannelReplyRun {
    fn from_view(view: &super::RunView<'_>) -> Self {
        let progress = view.progress();
        Self {
            completed: matches!(
                view.phase(),
                super::RunPhase::Settled {
                    outcome: super::RunOutcome::Done,
                    ..
                }
            ),
            unsuccessful: channel_reply_is_unsuccessful(view),
            assistant_expects_executor: view.occupies_conversation_execution(),
            replies: progress
                .results
                .iter()
                .map(|(node, results)| {
                    let replies = results
                        .iter()
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
                            host_reply: result.completion.output.get("host_reply")
                                == Some(&Value::from("provider_rejected")),
                            key: result.key.clone(),
                            text: result.completion.submission.clone(),
                        })
                        .collect();
                    (node.clone(), replies)
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

fn channel_reply_is_unsuccessful(view: &super::RunView<'_>) -> bool {
    match view.phase() {
        super::RunPhase::Settled {
            outcome:
                super::RunOutcome::Failed
                | super::RunOutcome::Escalated
                | super::RunOutcome::Unknown
                | super::RunOutcome::Aborted,
            ..
        } => true,
        super::RunPhase::Waiting {
            for_what: super::RunWait::Human,
            ..
        } => view.has_stopped_execution(),
        super::RunPhase::Executing
        | super::RunPhase::Waiting { .. }
        | super::RunPhase::Settled {
            outcome: super::RunOutcome::Done,
            ..
        } => false,
    }
}

fn startup_settlement(view: &super::RunView<'_>) -> Option<StartupChannelSettlement> {
    match view.phase() {
        super::RunPhase::Settled {
            outcome: super::RunOutcome::Done,
            ..
        } => Some(StartupChannelSettlement::Complete),
        super::RunPhase::Settled {
            outcome: super::RunOutcome::Failed | super::RunOutcome::Unknown,
            ..
        } => Some(StartupChannelSettlement::Fail),
        super::RunPhase::Settled {
            outcome: super::RunOutcome::Aborted,
            ..
        } => Some(StartupChannelSettlement::Abort),
        super::RunPhase::Waiting {
            for_what: super::RunWait::Human,
            ..
        } if view.has_stopped_execution() => Some(StartupChannelSettlement::Stop),
        super::RunPhase::Executing
        | super::RunPhase::Waiting { .. }
        | super::RunPhase::Settled {
            outcome: super::RunOutcome::Escalated,
            ..
        } => None,
    }
}

fn progress_status(view: &super::RunView<'_>) -> &'static str {
    match view.phase() {
        super::RunPhase::Executing => "executing",
        super::RunPhase::Waiting { .. } => "waiting",
        super::RunPhase::Settled { .. } => "settled",
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
    fn read_channel_run(&self, run: &str) -> Result<Option<super::RunView<'static>>, String> {
        self.read_run(run).map_err(|error| match error {
            ApplicationError::Conflict(message)
            | ApplicationError::Invalid(message)
            | ApplicationError::Storage(message) => message,
            ApplicationError::Missing => "Run is missing".into(),
        })
    }

    pub(crate) fn channel_run_is_admitted(&self, run: &str) -> Result<bool, String> {
        self.read_channel_run(run).map(|view| view.is_some())
    }

    pub(crate) fn channel_file_snapshot(
        &self,
        run: &str,
        node: &str,
    ) -> Result<Option<ChannelFileSnapshot>, String> {
        Ok(self.read_channel_run(run)?.map(|view| {
            let progress = view.progress();
            ChannelFileSnapshot {
                commit: progress
                    .results
                    .get(node)
                    .and_then(|results| results.last())
                    .map(|result| result.commit.clone()),
            }
        }))
    }

    pub(crate) fn channel_reply_is_completed(&self, run: &str) -> Result<Option<bool>, String> {
        Ok(self.read_channel_run(run)?.map(|view| {
            matches!(
                view.phase(),
                super::RunPhase::Settled {
                    outcome: super::RunOutcome::Done,
                    ..
                }
            )
        }))
    }

    pub(crate) fn channel_reply_run(&self, run: &str) -> Result<Option<ChannelReplyRun>, String> {
        Ok(self
            .read_channel_run(run)?
            .as_ref()
            .map(ChannelReplyRun::from_view))
    }

    pub(crate) fn assistant_progress_work_invocation(
        &self,
        run: &str,
        work_node: &str,
    ) -> Result<Option<InvocationKey>, String> {
        let Some(view) = self.read_channel_run(run)? else {
            return Ok(None);
        };
        if let Some(cursor) = view.cursor().filter(|c| c.node_id == work_node) {
            return Ok(Some(cursor.key.clone()));
        }
        let progress = view.progress();
        let latest_result = progress
            .results
            .get(work_node)
            .and_then(|results| results.last());
        let started = progress.invocations.get(work_node).copied().unwrap_or(0);
        let has_current_invocation = view.active()
            || view
                .frontier
                .iter()
                .any(|dispatch| dispatch.node_id == work_node);
        if has_current_invocation
            && started > latest_result.map_or(0, |result| result.key.invocation)
        {
            return Ok(Some(InvocationKey {
                run_id: progress.run_id.to_owned(),
                graph_digest: progress.graph_digest.to_owned(),
                node_id: work_node.to_owned(),
                invocation: started,
            }));
        }
        Ok(latest_result.map(|result| result.key.clone()))
    }

    pub(crate) fn channel_progress_status(&self, run: &str) -> String {
        match self.read_channel_run(run) {
            Ok(Some(view)) => progress_status(&view).to_owned(),
            _ => "missing".to_owned(),
        }
    }

    pub(crate) fn channel_progress_is_settled(&self, run: &str) -> bool {
        match self.read_channel_run(run) {
            Ok(Some(view)) => {
                view.is_settled()
                    || (!view.has_pending_recovery()
                        && matches!(
                            view.phase(),
                            super::RunPhase::Waiting {
                                for_what: super::RunWait::Human,
                                reason,
                            } if reason != Some("paused")
                        ))
            }
            _ => true,
        }
    }

    pub(crate) fn channel_startup_runs(&self) -> Result<Vec<ChannelStartupRun>, ApplicationError> {
        Ok(self
            .run_views()?
            .into_iter()
            .map(|(run_id, view)| {
                let progress = view.progress();
                ChannelStartupRun {
                    run_id,
                    assistant_graph_digest: (!view.is_settled()
                        && !channel_reply_is_unsuccessful(&view))
                    .then(|| progress.graph_digest.to_owned()),
                    settlement: startup_settlement(&view),
                }
            })
            .collect())
    }

    pub(crate) fn settle_startup_channel_run(
        &self,
        run: &ChannelStartupRun,
    ) -> Result<(), ApplicationError> {
        if run.settlement.is_none() {
            return Ok(());
        }
        self.settle_channel_run(&run.run_id)
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures;
    use super::*;
    use anchor_runtime::graph::{GraphRunRecord, GraphSnapshot};
    use serde_json::json;

    #[test]
    fn channel_queries_preserve_kernel_facts_and_startup_actions() {
        use StartupChannelSettlement::{Abort, Complete, Fail, Stop};
        let root = tempfile::tempdir().unwrap();
        let application = RunApplication::new(root.path().into(), root.path().join("catalog"));
        for id in fixtures::scenarios(&application) {
            let record = fixtures::record(&application, id);
            let completed = id == "done";
            let unsuccessful = matches!(id, "stopped" | "failed" | "aborted");
            let executor = id == "executing";
            let settled = matches!(id, "budget" | "stopped" | "done" | "failed" | "aborted");
            let wire = if id == "executing" {
                "executing"
            } else if matches!(id, "done" | "failed" | "aborted") {
                "settled"
            } else {
                "waiting"
            };
            let settlement = match id {
                "done" => Some(Complete),
                "failed" => Some(Fail),
                "aborted" => Some(Abort),
                "stopped" => Some(Stop),
                _ => None,
            };
            let reply = application.channel_reply_run(id).unwrap().unwrap();
            assert_eq!(reply.completed, completed, "{id}");
            assert_eq!(reply.unsuccessful, unsuccessful, "{id}");
            assert_eq!(reply.assistant_expects_executor, executor, "{id}");
            assert_eq!(
                application.channel_reply_is_completed(id).unwrap(),
                Some(completed)
            );
            assert_eq!(application.channel_progress_is_settled(id), settled, "{id}");
            assert_eq!(application.channel_progress_status(id), wire);
            assert!(application.channel_run_is_admitted(id).unwrap());
            let candidate = application
                .channel_startup_runs()
                .unwrap()
                .into_iter()
                .find(|candidate| candidate.run_id == id)
                .unwrap();
            assert_eq!(candidate.settlement, settlement, "{id}");
            let mut metadata = RunMetadata::new(
                id.into(),
                "fixture".into(),
                record.graph_digest,
                root.path(),
            )
            .unwrap();
            assert_eq!(
                candidate.preserves_assistant(&metadata),
                !completed && !unsuccessful,
                "{id}"
            );
            metadata.graph_digest = "another-graph".into();
            assert!(!candidate.preserves_assistant(&metadata));
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
        let expected = match application.read_run(run).err().unwrap() {
            ApplicationError::Conflict(message)
            | ApplicationError::Invalid(message)
            | ApplicationError::Storage(message) => message,
            ApplicationError::Missing => panic!("legacy bytes must be rejected by name"),
        };
        assert!(expected.contains("legacy"));
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
    fn reply_and_file_projections_keep_result_order_and_frontier_fallback() {
        let root = tempfile::tempdir().unwrap();
        let application = RunApplication::new(root.path().into(), root.path().join("catalog"));
        let record = GraphRunRecord::create_with_id(
            GraphSnapshot::from_authoring(json!({
                "objective":"test", "entry":"work",
                "agents":{"role":{"model":"fixture","instructions":"work"}},
                "nodes":[{"id":"work","agent":"role","max_rounds":10}],
                "edges":[{"from":"work","to":"work"}]
            }))
            .unwrap(),
            json!({}),
            "channel-facade-test",
        )
        .unwrap();
        fixtures::create(&application, &record);
        let run = record.run_id.as_str();
        for invocation in 1..=2 {
            let mut completion = fixtures::completion(json!({
                "suppressed":true,"reply_for":"turn","source_commit":"invalid-until-used",
                "items":[{"type":"image"}]
            }));
            completion.submission = format!("  reply {invocation}  ");
            fixtures::complete(&application, run, "work", completion, None);
        }
        let current = InvocationKey {
            run_id: run.into(),
            graph_digest: record.graph_digest.clone(),
            node_id: "work".into(),
            invocation: 3,
        };
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
        let committed = fixtures::record(&application, run);
        assert_eq!(
            application
                .channel_file_snapshot(run, "work")
                .unwrap()
                .unwrap()
                .commit,
            Some(committed.results["work"][1].commit.clone())
        );
        assert!(
            application
                .channel_file_snapshot(run, "other")
                .unwrap()
                .unwrap()
                .commit
                .is_none()
        );
        fixtures::control(&application, run, "abandon", "operator");
        assert_eq!(
            application
                .assistant_progress_work_invocation(run, "work")
                .unwrap(),
            Some(committed.results["work"][1].key.clone())
        );
    }
}

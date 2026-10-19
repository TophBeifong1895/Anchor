use super::{ActiveRun, ApplicationError, RunApplication, new_control};
use crate::execution::PreparedExecution;
use anchor_runtime::graph::{GraphRunRecord, RunStatus, RunStore};
use std::sync::atomic::Ordering;

impl RunApplication {
    pub(crate) async fn recover_detached_at_startup(&self) -> Result<(), ApplicationError> {
        self.recover_detached().await?;
        // An abandon request accepted just before a crash must still make its
        // Run terminal: it is the precondition for replacing a Graph's Plugin
        // resources, and channel assistant recovery follows this step.
        self.finalize_recorded_abandons_at_startup().await?;
        // A delivery receipt can be committed just before the service exits,
        // leaving its wait parent parked. Reuse ordinary child completion.
        for (id, _) in self.records()? {
            if let Some(metadata) = self.metadata(&id)?
                && metadata
                    .session_call
                    .as_ref()
                    .is_some_and(|call| call.status != "pending")
            {
                self.child_finished(&id, &metadata.graph, RunStatus::Completed)
                    .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn dispatch_detached(&self, run_id: &str) -> Result<(), ApplicationError> {
        let mut active = self.active.lock().await;
        if active.contains_key(run_id) {
            return Ok(());
        }
        let metadata = self.metadata(run_id)?.ok_or(ApplicationError::Missing)?;
        if metadata.session_call.is_some() {
            return Ok(());
        }
        let source = metadata.graph_call.as_ref().ok_or_else(|| {
            ApplicationError::Invalid("detached child has no durable Graph call source".into())
        })?;
        if source.mode != "detach" || metadata.trigger_source != "graph_call" {
            return Err(ApplicationError::Invalid(
                "Run is not an admitted detached child".into(),
            ));
        }
        let lease = self.store().acquire_lease(run_id)?;
        let record = self
            .store()
            .load(run_id)?
            .ok_or(ApplicationError::Missing)?;
        if !super::run_access::RunView::inactive(&record).can_dispatch() {
            // Running means an execution may have escaped before a crash. Never
            // infer that it is safe to replay.
            return Ok(());
        }
        if record.graph_digest != metadata.graph_digest {
            return Err(ApplicationError::Invalid(
                "detached child snapshot digest changed".into(),
            ));
        }
        self.executor()?;
        let control = new_control();
        let execution = self.prepare_resume(&record, &metadata, control.clone())?;
        active.insert(
            run_id.to_owned(),
            ActiveRun::new(Self::graph_identity(&metadata.bundle_source)?, control),
        );
        drop(lease);
        drop(active);
        self.spawn(record, execution)?;
        Ok(())
    }

    pub(crate) async fn recover_detached(&self) -> Result<(), ApplicationError> {
        let candidates = self
            .run_views()?
            .into_iter()
            .filter_map(|(id, record)| record.can_dispatch().then_some(id))
            .collect::<Vec<_>>();
        for run_id in candidates {
            if self
                .metadata(&run_id)?
                .and_then(|m| m.graph_call)
                .is_some_and(|source| source.mode == "detach")
            {
                self.dispatch_detached(&run_id).await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn begin_wait_child(
        &self,
        run_id: &str,
        graph: &str,
        parent_cancellation: anchor_runtime::Cancellation,
    ) -> Result<anchor_graph_host::ChildRunControl, ApplicationError> {
        let mut active = self.active.lock().await;
        if let Some(existing) = active.get(run_id) {
            return Ok(anchor_graph_host::ChildRunControl {
                cancellation: existing.control.cancellation.clone(),
                pause: existing.control.pause.clone(),
            });
        }
        // Preflight before registering: an unavailable executor must not leave
        // an active child that no task can ever finish.
        let executor = self.executor()?;
        let graph_path = match self.metadata(run_id)? {
            Some(metadata) => Self::graph_identity(&metadata.bundle_source)?,
            None => Self::graph_identity(&self.graph_bundle_path(graph))?,
        };
        let control = new_control();
        let cancel = control.cancellation.clone();
        let forwarder = executor.spawn(async move {
            while !parent_cancellation.load(Ordering::Acquire) {
                if cancel.load(Ordering::Acquire) {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            cancel.store(true, Ordering::Release);
        });
        let mut registration = ActiveRun::new(graph_path, control.clone());
        registration.wait_cancellation = Some(forwarder);
        active.insert(run_id.to_owned(), registration);
        Ok(anchor_graph_host::ChildRunControl {
            cancellation: control.cancellation,
            pause: control.pause,
        })
    }

    pub(crate) async fn end_wait_child(&self, run_id: &str) {
        let registration = self.active.lock().await.remove(run_id);
        if let Some(mut registration) = registration {
            // Never turn ordinary completion into a business stop request.
            registration.finish_wait_cancellation().await;
        }
    }

    pub(crate) fn child_finished<'a>(
        &'a self,
        run_id: &'a str,
        _graph: &str,
        status: RunStatus,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), ApplicationError>> + Send + 'a>,
    > {
        Box::pin(async move {
            if status != RunStatus::Completed {
                return Ok(());
            }
            let metadata = self.metadata(run_id)?.ok_or(ApplicationError::Missing)?;
            if metadata
                .session_call
                .as_ref()
                .is_some_and(|call| call.status == "pending")
            {
                return Ok(());
            }
            let Some(source) = metadata.graph_call else {
                return Ok(());
            };
            if source.mode != "wait" {
                return Ok(());
            }
            if self.active.lock().await.contains_key(&source.parent_run) {
                // Inline wait execution returns the child result directly to the
                // still-running parent Runner; only independently resumed child
                // Runs need to kick a WaitingCall parent.
                return Ok(());
            }
            if let Some(parent) = self.store().load(&source.parent_run)?
                && parent.status == RunStatus::WaitingCall
                && parent.cursor.as_ref().is_some_and(|cursor| {
                    cursor.node_id == source.node && cursor.key.invocation == source.invocation
                })
            {
                let application = self.clone();
                let parent_run = source.parent_run.clone();
                self.executor()?.spawn(async move {
                    tokio::task::yield_now().await;
                    if let Err(error) = application
                        .control_expected(
                            &parent_run,
                            "resume",
                            Some((&source.node, source.invocation)),
                        )
                        .await
                    {
                        eprintln!("waiting parent {parent_run} could not resume: {error:?}");
                    }
                });
            }
            Ok(())
        })
    }

    pub(super) fn executor(&self) -> Result<&tokio::runtime::Handle, ApplicationError> {
        self.executor.as_ref().ok_or_else(|| {
            ApplicationError::Invalid("Run execution requires the owning Host executor".into())
        })
    }

    pub(super) fn spawn(
        &self,
        record: GraphRunRecord,
        execution: PreparedExecution,
    ) -> Result<(), ApplicationError> {
        let application = self.clone();
        self.executor()?.spawn(async move {
            let run_id = record.run_id.clone();
            let result = execution.run(record).await;
            let (graph, status) = match result {
                Ok(record) => (record.snapshot.objective.clone(), record.status),
                Err(error) => {
                    eprintln!("Run {run_id} execution failed: {error}");
                    let status = application
                        .store()
                        .load(&run_id)
                        .ok()
                        .flatten()
                        .map_or(RunStatus::Failed, |record| record.status);
                    (String::new(), status)
                }
            };
            let mut finished = application.active.lock().await.remove(&run_id);
            if let Some(run) = &mut finished {
                run.finish_wait_cancellation().await;
            }
            if finished.is_some_and(|run| run.control.cancellation.load(Ordering::Acquire)) {
                if let Err(error) = application.stop_wait_session_children(&run_id).await {
                    eprintln!("Run {run_id} wait cancellation failed: {error:?}");
                }
            } else if status == RunStatus::WaitingCall {
                // Settlement can race the parent's final WaitingCall write. Once the
                // execution slot is released, recheck the durable child facts once.
                if let Ok(children) = application.child_metadata(&run_id) {
                    for child in children {
                        if child
                            .session_call
                            .as_ref()
                            .is_some_and(|call| call.status != "pending")
                        {
                            let _ = application
                                .child_finished(&child.run_id, &child.graph, RunStatus::Completed)
                                .await;
                        }
                    }
                }
            }
            // A recorded abandon request makes the Run terminal even though the
            // Runner could only stop it: either the cancellation this request
            // caused, or a completion that won the race first.
            let status = match application.finalize_recorded_abandon(&run_id).await {
                Ok(Some(status)) => status,
                Ok(None) => status,
                Err(error) => {
                    eprintln!("Run {run_id} abandon finalization failed: {error:?}");
                    status
                }
            };
            if let Err(error) = application.settle_channel_run(&run_id, status) {
                eprintln!("Run {run_id} channel Turn settlement failed: {error:?}");
            }
            if let Err(error) = application.child_finished(&run_id, &graph, status).await {
                eprintln!("Run {run_id} completion callback failed: {error:?}");
            }
        });
        Ok(())
    }
}

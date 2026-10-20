use super::*;

fn park(runs: &KernelRuns, record: &GraphRunRecord) -> InvocationKey {
    runs.create(record, 100).unwrap();
    let resource = runs.load(&record.run_id).unwrap().unwrap();
    let key = InvocationKey {
        run_id: record.run_id.clone(),
        graph_digest: record.graph_digest.clone(),
        node_id: "a".into(),
        invocation: 1,
    };
    let attempts = [1, 2].map(|attempt_id| RecoveryAttempt {
        attempt_id,
        step: 1,
        tool: "fixture effect".into(),
        started_at: "2026-10-19T00:00:00Z".into(),
    });
    assert!(runs.kernel.submit(&Principal::new(HOST_ID, PrincipalKind::Code), 0,
        Submission::new(&resource.id, resource.version, Action::Request)
            .content(json!({"control":"stop","reason":"unknown external tool effects","invocation":key,"attempts":attempts}))
            .evidence(Evidence::Observation { id:"fixture/unknown-effect".into() })).unwrap().is_accepted());
    key
}

#[tokio::test]
async fn recovery_persists_effect_fact_before_human_release_and_never_dispatches_the_unknown_node()
{
    let root = tempfile::tempdir().unwrap();
    let application = app(root.path());
    let runs = application.kernel_runs(true).unwrap().unwrap();
    let record = record("m2-recovery");
    let key = park(&runs, &record);
    let nodes = Nodes::new(&root.path().join("facts"), Mode::Complete);
    let decision = RecoveryDecision::Completed {
        observation: "verified against external receipt".into(),
    };
    let view = application.read_run(&record.run_id).unwrap().unwrap();
    assert!(view.has_pending_recovery());
    assert!(view.can_resume());
    assert!(!view.is_settled());
    let disk_view = crate::application::run_access::reads::load(root.path(), &record.run_id)
        .unwrap()
        .unwrap();
    assert!(disk_view.has_pending_recovery());
    assert_eq!(disk_view.recovery, view.recovery);
    let graph_facts = &runs.load(&record.run_id).unwrap().unwrap().state;
    for field in ["error", "updated", "recovery", "recovery_submissions"] {
        assert!(
            graph_facts.get(field).is_none(),
            "{field} is not a Graph fact"
        );
    }

    assert_eq!(
        application
            .read_run(&record.run_id)
            .unwrap()
            .unwrap()
            .recovery
            .len(),
        2
    );
    assert!(
        !runs
            .recover_attempt(&key, 1, decision.clone(), || {
                assert!(matches!(
                    runs.load(&key.run_id).unwrap().unwrap().phase(),
                    Some(Phase::Waiting { .. })
                ));
                Ok(())
            })
            .unwrap()
    );
    assert!(matches!(
        runs.load(&key.run_id).unwrap().unwrap().phase(),
        Some(Phase::Waiting { .. })
    ));
    // A lost response must neither rewrite the execution fact nor release
    // other attempts. Conflicting decisions cannot reach the port.
    assert!(
        !runs
            .recover_attempt(&key, 1, decision.clone(), || panic!("duplicate port call"))
            .unwrap()
    );
    assert!(
        runs.recover_attempt(&key, 1, RecoveryDecision::Abort, || panic!(
            "conflicting port call"
        ))
        .is_err()
    );
    assert!(
        runs.recover_attempt(&key, 2, decision.clone(), || {
            nodes.save(&key);
            Ok(())
        })
        .unwrap()
    );
    assert_eq!(
        runs.load(&key.run_id).unwrap().unwrap().phase(),
        Some(&Phase::Executing)
    );
    // Simulate host loss between the resume commit and worker registration.
    drop(runs);
    drop(application);
    let restarted = app(root.path());
    let runs = restarted.kernel_runs(false).unwrap().unwrap();
    assert!(
        runs.recover_attempt(&key, 2, decision.clone(), || panic!(
            "durable decision must be reused"
        ))
        .unwrap()
    );
    let done = runs
        .drive(&key.run_id, &Artifacts, &nodes, cancel())
        .await
        .unwrap();
    assert!(done.is_settled());
    assert!(nodes.trace.lock().unwrap().iter().all(|k| k.node_id != "a"));
    assert!(
        !runs
            .recover_attempt(&key, 2, decision, || panic!("settled idempotent retry"))
            .unwrap()
    );
    let (_, decisions) = runs.recovery_facts(&key.run_id).unwrap();
    assert_eq!(decisions.len(), 2);
    assert!(runs.recovery_facts(&key.run_id).unwrap().0.is_empty());
    assert!(!root.path().join("runs").exists());
}

#[test]
fn recovery_failure_does_not_release_and_abort_does_not_replay_or_complete_other_attempts() {
    let root = tempfile::tempdir().unwrap();
    let application = app(root.path());
    let runs = application.kernel_runs(true).unwrap().unwrap();
    let key = park(&runs, &record("m2-abort-recovery"));
    let before = runs.load(&key.run_id).unwrap().unwrap();
    let complete = RecoveryDecision::Completed {
        observation: "inspected".into(),
    };
    assert!(
        runs.recover_attempt(&key, 1, complete.clone(), || Err(GraphError::CorruptRun(
            "port could not persist its fact".into()
        )))
        .is_err()
    );
    assert_eq!(runs.load(&key.run_id).unwrap().unwrap(), before);
    assert!(
        runs.recover_attempt(&key, 999, complete, || panic!("unknown attempt"))
            .is_err()
    );
    assert!(
        runs.recover_attempt(&key, 1, RecoveryDecision::Retry, || panic!(
            "must not replay"
        ))
        .is_err()
    );
    assert!(
        !runs
            .recover_attempt(&key, 1, RecoveryDecision::Abort, || Ok(()))
            .unwrap()
    );
    assert!(matches!(
        runs.load(&key.run_id).unwrap().unwrap().phase(),
        Some(Phase::Settled {
            result: Outcome::Aborted { .. }
        })
    ));
    assert!(
        !runs
            .recover_attempt(&key, 1, RecoveryDecision::Abort, || panic!(
                "idempotent abort"
            ))
            .unwrap()
    );
    assert!(
        runs.recover_attempt(
            &key,
            2,
            RecoveryDecision::Completed {
                observation: "too late".into()
            },
            || panic!("cannot reopen abandonment")
        )
        .is_err()
    );
}

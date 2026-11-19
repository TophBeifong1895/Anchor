use super::tests::{completion_with_files, fixture, key};
use super::*;

fn interruption() -> (NodeCompletion, ArtifactFreezeContext) {
    (
        NodeCompletion {
            submission: "input superseded by a newer Turn".into(),
            ..completion_with_files(&[])
        },
        ArtifactFreezeContext {
            kind: ArtifactKind::Interruption,
            input_commits: vec![],
        },
    )
}

#[tokio::test]
async fn interruption_releases_recovered_owner_before_next_invocation() {
    for already_published in [false, true] {
        let (_temp, artifacts) = fixture();
        let invocation = key("assistant");
        let workspaces = artifacts.workspaces();
        let scene = workspaces.prepare(&invocation).unwrap();
        fs::write(scene.join("draft.txt"), b"retained scene").unwrap();
        // A process exit drops its lock without releasing its durable owner.
        drop(workspaces.acquire(&invocation).unwrap());
        let (completion, context) = interruption();
        if already_published {
            artifacts
                .publish_owned(&invocation, &completion, &context)
                .unwrap();
        }
        let commit = artifacts
            .freeze_with_context(&invocation, &completion, &context)
            .await
            .unwrap();
        let next = InvocationKey {
            invocation: 2,
            ..invocation.clone()
        };
        let writer = workspaces.acquire(&next).unwrap();
        assert_eq!(
            fs::read(scene.join("draft.txt")).unwrap(),
            b"retained scene"
        );

        // Replaying an older committed interruption cannot release a new writer,
        // even when it no longer holds a process lock after another restart.
        assert_eq!(
            artifacts
                .freeze_with_context(&invocation, &completion, &context)
                .await
                .unwrap(),
            commit
        );
        assert_eq!(
            workspaces.initialized_owner(&next).unwrap(),
            Some(next.clone())
        );
        drop(writer);
        artifacts
            .freeze_with_context(&invocation, &completion, &context)
            .await
            .unwrap();
        assert!(matches!(
            workspaces.acquire(&InvocationKey {
                invocation: 3,
                ..next
            }),
            Err(GraphError::RunConflict)
        ));
    }
}

#[tokio::test]
async fn interruption_cannot_publish_or_release_a_live_writer() {
    let (_temp, artifacts) = fixture();
    let invocation = key("assistant");
    let writer = artifacts.workspaces().acquire(&invocation).unwrap();
    let (completion, context) = interruption();
    assert!(matches!(
        artifacts
            .freeze_with_context(&invocation, &completion, &context)
            .await,
        Err(GraphError::RunConflict)
    ));
    assert!(artifacts.published_commit(&invocation).is_none());
    drop(writer);
    artifacts
        .freeze_with_context(&invocation, &completion, &context)
        .await
        .unwrap();
}

#[tokio::test]
async fn interruption_publication_failure_keeps_owner_reserved() {
    let (_temp, artifacts) = fixture();
    let invocation = key("assistant");
    let workspaces = artifacts.workspaces();
    drop(workspaces.acquire(&invocation).unwrap());
    let (completion, mut context) = interruption();
    context.input_commits.push(commit_for(&key("missing")));
    assert!(
        artifacts
            .freeze_with_context(&invocation, &completion, &context)
            .await
            .is_err()
    );
    assert!(artifacts.published_commit(&invocation).is_none());
    assert!(matches!(
        workspaces.acquire(&InvocationKey {
            invocation: 2,
            ..invocation.clone()
        }),
        Err(GraphError::RunConflict)
    ));
    context.input_commits.clear();
    // A published but mismatched completion must not certify a release either.
    artifacts
        .publish_owned(&invocation, &completion, &context)
        .unwrap();
    let mut changed = completion.clone();
    changed.submission = "different interruption".into();
    assert!(matches!(
        artifacts
            .freeze_with_context(&invocation, &changed, &context)
            .await,
        Err(GraphError::RunConflict)
    ));
    assert!(matches!(
        workspaces.acquire(&InvocationKey {
            invocation: 2,
            ..invocation
        }),
        Err(GraphError::RunConflict)
    ));
}

#[tokio::test]
async fn interruption_does_not_create_or_replace_a_released_owner() {
    for prepared in [false, true] {
        let (_temp, artifacts) = fixture();
        let invocation = key("assistant");
        let workspaces = artifacts.workspaces();
        if prepared {
            workspaces.prepare(&invocation).unwrap();
        }
        let (completion, context) = interruption();
        artifacts
            .freeze_with_context(&invocation, &completion, &context)
            .await
            .unwrap();
        assert!(workspaces.initialized_owner(&invocation).unwrap().is_none());
    }

    for released_invocation in [1, 2] {
        let (_temp, artifacts) = fixture();
        let invocation = key("assistant");
        let released = InvocationKey {
            invocation: released_invocation,
            ..invocation.clone()
        };
        let workspaces = artifacts.workspaces();
        workspaces
            .acquire(&released)
            .unwrap()
            .finish_published()
            .unwrap();
        let owner_path = workspaces
            .path(&released)
            .unwrap()
            .parent()
            .unwrap()
            .join("owner.json");
        let owner = fs::read(&owner_path).unwrap();
        let (completion, context) = interruption();
        artifacts
            .freeze_with_context(&invocation, &completion, &context)
            .await
            .unwrap();
        assert_eq!(fs::read(&owner_path).unwrap(), owner);
        assert_eq!(
            workspaces.initialized_owner(&released).unwrap(),
            Some(released)
        );
        workspaces
            .acquire(&InvocationKey {
                invocation: 3,
                ..invocation
            })
            .unwrap();
    }
}

#[tokio::test]
async fn new_interruption_cannot_take_over_a_different_unreleased_owner() {
    let (_temp, artifacts) = fixture();
    let invocation = key("assistant");
    let workspaces = artifacts.workspaces();
    let resource = workspaces.resource(&invocation).unwrap();
    workspaces
        .acquire(&invocation)
        .unwrap()
        .finish_published()
        .unwrap();
    let foreign = InvocationKey {
        run_id: "other-run".into(),
        ..invocation.clone()
    };
    workspaces.bind_resource(&foreign, &resource).unwrap();
    drop(workspaces.acquire(&foreign).unwrap());
    let (completion, context) = interruption();
    assert!(matches!(
        artifacts
            .freeze_with_context(&invocation, &completion, &context)
            .await,
        Err(GraphError::RunConflict)
    ));
    assert!(artifacts.published_commit(&invocation).is_none());
    assert_eq!(
        workspaces.initialized_owner(&invocation).unwrap(),
        Some(foreign)
    );
    assert!(matches!(
        workspaces.acquire(&invocation),
        Err(GraphError::RunConflict)
    ));
}

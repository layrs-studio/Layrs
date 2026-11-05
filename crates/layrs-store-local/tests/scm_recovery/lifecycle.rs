use super::*;

#[test]
fn transitions_replays_and_missing_evidence_are_strict() {
    let root = initialize("transitions", &["worktree:one"]);
    let mut store = open_exclusive(&root.0).unwrap();
    let mut recovery = NativeRecovery::new(&mut store).unwrap();
    let candidate = plan(
        "recovery:one",
        "idem:one",
        "worktree:one",
        "src/main.rs",
        RecoveryType::Materialize,
    );

    assert_eq!(
        recovery.create(&candidate).unwrap(),
        MutationOutcome::Applied
    );
    assert_eq!(
        recovery.create(&candidate).unwrap(),
        MutationOutcome::Replayed
    );
    let mismatch = plan(
        "recovery:other",
        "idem:one",
        "worktree:one",
        "src/other.rs",
        RecoveryType::Materialize,
    );
    assert_eq!(
        recovery.create(&mismatch).unwrap_err().code(),
        "recovery_replay_mismatch"
    );
    assert!(RecoveryPreparation::new(json!({}), vec![]).is_err());

    let mut handler = CompleteHandler::default();
    let report = recovery.recover("owner", 11, 100, &mut handler).unwrap();
    assert_eq!(report.skipped, 1);
    assert_eq!(
        report.findings[0].code,
        RecoveryFindingCode::MissingEvidence
    );
    assert_eq!(handler.calls, 0);

    let prepared = preparation(Some("src/main.rs"), false);
    assert_eq!(
        recovery.prepare(candidate.id(), &prepared, 11).unwrap(),
        MutationOutcome::Applied
    );
    assert_eq!(
        recovery.prepare(candidate.id(), &prepared, 11).unwrap(),
        MutationOutcome::Replayed
    );
    let claim = recovery
        .claim(candidate.id(), "owner:one", 12, 100)
        .unwrap();
    assert_eq!(claim.outcome, MutationOutcome::Applied);
    assert_eq!(
        recovery
            .claim(candidate.id(), "owner:one", 12, 100)
            .unwrap()
            .outcome,
        MutationOutcome::Replayed
    );
    assert!(matches!(
        recovery.prepare(candidate.id(), &prepared, 13),
        Err(RecoveryError::InvalidTransition {
            from: RecoveryState::Applying,
            to: RecoveryState::Prepared
        })
    ));
    assert_eq!(
        recovery
            .complete(
                candidate.id(),
                "owner:one",
                &claim.leases,
                json!({"verified": true}),
                &[applied("src/main.rs")],
                13,
            )
            .unwrap(),
        MutationOutcome::Applied
    );
    assert_eq!(
        recovery
            .complete(
                candidate.id(),
                "owner:one",
                &claim.leases,
                json!({"verified": true}),
                &[applied("src/main.rs")],
                13,
            )
            .unwrap(),
        MutationOutcome::Replayed
    );
    assert!(recovery.list_pending().unwrap().is_empty());
}

#[test]
fn leases_allow_parallel_worktrees_and_fence_the_same_resource() {
    let root = initialize("leases", &["worktree:one", "worktree:two"]);
    let before = operation_snapshot(&root.0);
    let mut store = open_exclusive(&root.0).unwrap();
    let mut recovery = NativeRecovery::new(&mut store).unwrap();
    let first = plan(
        "recovery:first",
        "idem:first",
        "worktree:one",
        "same.txt",
        RecoveryType::Materialize,
    );
    let second = plan(
        "recovery:second",
        "idem:second",
        "worktree:two",
        "same.txt",
        RecoveryType::Materialize,
    );
    let fenced = plan(
        "recovery:fenced",
        "idem:fenced",
        "worktree:one",
        "same.txt",
        RecoveryType::Restore,
    );
    for candidate in [&first, &second, &fenced] {
        recovery.create(candidate).unwrap();
        recovery
            .prepare(candidate.id(), &preparation(Some("same.txt"), false), 11)
            .unwrap();
    }
    let first_claim = recovery.claim(first.id(), "owner:first", 12, 100).unwrap();
    let renewed = recovery
        .heartbeat(first.id(), "owner:first", &first_claim.leases, 13, 100)
        .unwrap();
    assert_eq!(renewed[0].generation, first_claim.leases[0].generation);
    recovery
        .claim(second.id(), "owner:second", 12, 100)
        .unwrap();
    assert!(matches!(
        recovery.claim(fenced.id(), "owner:fenced", 12, 100),
        Err(RecoveryError::LeaseFenced { .. })
    ));
    recovery
        .complete(
            first.id(),
            "owner:first",
            &first_claim.leases,
            json!({"done": true}),
            &[applied("same.txt")],
            14,
        )
        .unwrap();
    let takeover = recovery
        .claim(fenced.id(), "owner:fenced", 15, 100)
        .unwrap();
    assert_eq!(takeover.leases[0].generation, 2);
    drop(recovery);
    drop(store);
    assert_eq!(operation_snapshot(&root.0), before);
}

use super::*;

#[test]
fn every_journal_boundary_is_idempotently_restartable() {
    for (index, point) in [
        RecoveryFaultPoint::AfterCreateCommit,
        RecoveryFaultPoint::AfterPrepareCommit,
        RecoveryFaultPoint::AfterClaimCommit,
        RecoveryFaultPoint::BeforeHandler,
        RecoveryFaultPoint::AfterHandler,
        RecoveryFaultPoint::BeforeTerminalCommit,
        RecoveryFaultPoint::AfterTerminalCommit,
    ]
    .into_iter()
    .enumerate()
    {
        let root = initialize(&format!("fault-{index}"), &["worktree:one"]);
        let mut store = open_exclusive(&root.0).unwrap();
        let faults = Arc::new(FailOnce::new(point));
        let mut recovery = NativeRecovery::with_fault_injector(&mut store, faults).unwrap();
        let candidate = plan(
            &format!("recovery:fault-{index}"),
            &format!("idem:fault-{index}"),
            "worktree:one",
            "fault.txt",
            RecoveryType::Materialize,
        );
        let created = recovery.create(&candidate);
        if point == RecoveryFaultPoint::AfterCreateCommit {
            assert_eq!(created.unwrap_err().code(), "recovery_injected_fault");
            assert_eq!(
                recovery.create(&candidate).unwrap(),
                MutationOutcome::Replayed
            );
        } else {
            created.unwrap();
        }
        let prepared = recovery.prepare(candidate.id(), &preparation(Some("fault.txt"), false), 11);
        if point == RecoveryFaultPoint::AfterPrepareCommit {
            assert_eq!(prepared.unwrap_err().code(), "recovery_injected_fault");
            recovery
                .prepare(candidate.id(), &preparation(Some("fault.txt"), false), 11)
                .unwrap();
        } else {
            prepared.unwrap();
        }
        if point == RecoveryFaultPoint::AfterClaimCommit {
            assert_eq!(
                recovery
                    .claim(candidate.id(), "runner", 12, 100)
                    .unwrap_err()
                    .code(),
                "recovery_injected_fault"
            );
            let replayed_claim = recovery.claim(candidate.id(), "runner", 12, 100).unwrap();
            assert_eq!(replayed_claim.outcome, MutationOutcome::Replayed);
            recovery
                .complete(
                    candidate.id(),
                    "runner",
                    &replayed_claim.leases,
                    json!({"done": true}),
                    &[applied("fault.txt")],
                    13,
                )
                .unwrap();
        } else {
            let mut handler = CompleteHandler::default();
            let first = recovery.recover("runner", 12, 100, &mut handler);
            if matches!(
                point,
                RecoveryFaultPoint::BeforeHandler
                    | RecoveryFaultPoint::AfterHandler
                    | RecoveryFaultPoint::BeforeTerminalCommit
                    | RecoveryFaultPoint::AfterTerminalCommit
            ) {
                assert_eq!(first.unwrap_err().code(), "recovery_injected_fault");
                recovery.recover("runner", 13, 100, &mut handler).unwrap();
            } else {
                first.unwrap();
            }
        }
        assert!(recovery.list_pending().unwrap().is_empty());
    }
}

struct CasHandler {
    entered: mpsc::Sender<()>,
}

impl RecoveryHandler for CasHandler {
    fn recover(
        &mut self,
        _: &RecoveryRecord,
    ) -> layrs_store_local::scm_v1::RecoveryResult<RecoveryDecision> {
        self.entered.send(()).unwrap();
        Ok(RecoveryDecision::RecoverCasStaging {
            evidence: json!({"leaseExclusive": true}),
        })
    }
}

struct ImmediateCasHandler;

impl RecoveryHandler for ImmediateCasHandler {
    fn recover(
        &mut self,
        _: &RecoveryRecord,
    ) -> layrs_store_local::scm_v1::RecoveryResult<RecoveryDecision> {
        Ok(RecoveryDecision::RecoverCasStaging {
            evidence: json!({"leaseExclusive": true}),
        })
    }
}

#[test]
fn cas_cleanup_boundaries_are_idempotently_restartable() {
    for (index, point) in [
        RecoveryFaultPoint::BeforeCasStagingRecovery,
        RecoveryFaultPoint::AfterCasStagingRecovery,
    ]
    .into_iter()
    .enumerate()
    {
        let root = initialize(&format!("cas-fault-{index}"), &["worktree:one"]);
        let mut store = open_exclusive(&root.0).unwrap();
        let faults = Arc::new(FailOnce::new(point));
        let mut recovery = NativeRecovery::with_fault_injector(&mut store, faults).unwrap();
        let candidate = plan(
            &format!("recovery:cas-fault-{index}"),
            &format!("idem:cas-fault-{index}"),
            "worktree:one",
            "unused",
            RecoveryType::CasStagingCleanup,
        );
        recovery.create(&candidate).unwrap();
        recovery
            .prepare(candidate.id(), &preparation(None, false), 11)
            .unwrap();
        let mut handler = ImmediateCasHandler;
        assert_eq!(
            recovery
                .recover("cas-fault-runner", 12, 100, &mut handler)
                .unwrap_err()
                .code(),
            "recovery_injected_fault"
        );
        let report = recovery
            .recover("cas-fault-runner", 13, 100, &mut handler)
            .unwrap();
        assert_eq!(report.completed, 1);
        assert!(recovery.list_pending().unwrap().is_empty());
    }
}

#[test]
fn cas_recovery_waits_for_active_staging_lease() {
    let root = initialize("cas-lease", &["worktree:one"]);
    let exclusive = open_exclusive(&root.0).unwrap();
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let prepared_object = cas
        .stage(&CanonicalObject::ChunkRaw(
            Chunk::new(b"active".to_vec()).unwrap(),
        ))
        .unwrap();
    drop(cas);
    drop(exclusive);

    let candidate = plan(
        "recovery:cas",
        "idem:cas",
        "worktree:one",
        "unused",
        RecoveryType::CasStagingCleanup,
    );
    {
        let mut store = open_exclusive(&root.0).unwrap();
        let mut recovery = NativeRecovery::new(&mut store).unwrap();
        recovery.create(&candidate).unwrap();
        recovery
            .prepare(candidate.id(), &preparation(None, false), 11)
            .unwrap();
    }

    let (entered_tx, entered_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let recovery_root = root.0.clone();
    let thread = std::thread::spawn(move || {
        let mut store = open_exclusive(recovery_root).unwrap();
        let mut recovery = NativeRecovery::new(&mut store).unwrap();
        let mut handler = CasHandler {
            entered: entered_tx,
        };
        let result = recovery.recover("cas-runner", 12, 100, &mut handler);
        done_tx.send(result).unwrap();
    });
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(done_rx.recv_timeout(Duration::from_millis(100)).is_err());
    drop(prepared_object);
    let report = done_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert_eq!(report.completed, 1);
    thread.join().unwrap();
}

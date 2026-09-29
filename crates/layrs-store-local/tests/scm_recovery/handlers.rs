use super::*;

struct FailOneHandler {
    failed_id: RecoveryId,
}

struct HugeErrorHandler;

impl RecoveryHandler for HugeErrorHandler {
    fn recover(
        &mut self,
        _: &RecoveryRecord,
    ) -> layrs_store_local::scm_v1::RecoveryResult<RecoveryDecision> {
        Err(RecoveryError::Handler(
            "界".repeat(MAX_FINDING_MESSAGE_BYTES * 2),
        ))
    }
}

#[test]
fn handler_failure_finding_is_utf8_and_byte_bounded() {
    let root = initialize("handler-bounded", &["worktree:one"]);
    let mut store = open_exclusive(&root.0).unwrap();
    let mut recovery = NativeRecovery::new(&mut store).unwrap();
    let candidate = plan(
        "recovery:handler-bounded",
        "idem:handler-bounded",
        "worktree:one",
        "huge.txt",
        RecoveryType::Materialize,
    );
    recovery.create(&candidate).unwrap();
    recovery
        .prepare(candidate.id(), &preparation(Some("huge.txt"), false), 11)
        .unwrap();

    let report = recovery
        .recover("runner", 12, 100, &mut HugeErrorHandler)
        .unwrap();
    assert_eq!(report.failed, 1);
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].code, RecoveryFindingCode::HandlerFailed);
    assert!(
        report.findings[0]
            .message
            .is_char_boundary(report.findings[0].message.len())
    );
    assert_eq!(report.findings[0].message.len(), MAX_FINDING_MESSAGE_BYTES);
}

impl RecoveryHandler for FailOneHandler {
    fn recover(
        &mut self,
        record: &RecoveryRecord,
    ) -> layrs_store_local::scm_v1::RecoveryResult<RecoveryDecision> {
        if record.id == self.failed_id {
            return Err(RecoveryError::Handler("injected handler failure".into()));
        }
        Ok(RecoveryDecision::Complete {
            evidence: json!({"verified": true}),
            path_results: record
                .paths
                .iter()
                .map(|path| {
                    RecoveryPathResult::new(
                        path.path.clone(),
                        path.role,
                        RecoveryPathState::Applied,
                        json!({"verified": true}),
                    )
                    .unwrap()
                })
                .collect(),
        })
    }
}

#[test]
fn handler_failure_is_reported_without_finalizing_or_blocking_independent_records() {
    let root = initialize("handler-continue", &["worktree:one"]);
    let mut store = open_exclusive(&root.0).unwrap();
    let mut recovery = NativeRecovery::new(&mut store).unwrap();
    let failed = plan(
        "recovery:a-handler-fails",
        "idem:a-handler-fails",
        "worktree:one",
        "failed.txt",
        RecoveryType::Materialize,
    );
    let succeeds = plan(
        "recovery:b-handler-succeeds",
        "idem:b-handler-succeeds",
        "worktree:one",
        "succeeds.txt",
        RecoveryType::Materialize,
    );
    for candidate in [&failed, &succeeds] {
        recovery.create(candidate).unwrap();
        let path = if candidate.id().as_str().contains("a-handler") {
            "failed.txt"
        } else {
            "succeeds.txt"
        };
        recovery
            .prepare(candidate.id(), &preparation(Some(path), false), 11)
            .unwrap();
    }
    let mut handler = FailOneHandler {
        failed_id: failed.id().clone(),
    };
    let report = recovery.recover("runner", 12, 100, &mut handler).unwrap();
    assert_eq!(report.completed, 1);
    assert_eq!(report.failed, 1);
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].code, RecoveryFindingCode::HandlerFailed);
    assert_eq!(report.truncated_findings, 0);
    let pending = recovery.list_pending().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, *failed.id());
    assert_eq!(pending[0].state, RecoveryState::Applying);
    assert_eq!(pending[0].paths[0].state, RecoveryPathState::Verified);
}

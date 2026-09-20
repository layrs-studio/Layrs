use super::*;

#[test]
fn clock_rollback_preserves_lease_order_and_generation_fencing() {
    let root = initialize("clock-rollback", &["worktree:one"]);
    let mut store = open_exclusive(&root.0).unwrap();
    let mut recovery = NativeRecovery::new(&mut store).unwrap();
    let candidate = plan(
        "recovery:clock-rollback",
        "idem:clock-rollback",
        "worktree:one",
        "clock.txt",
        RecoveryType::Restore,
    );
    recovery.create(&candidate).unwrap();
    recovery
        .prepare(candidate.id(), &preparation(Some("clock.txt"), false), 100)
        .unwrap();
    let claim = recovery
        .claim(candidate.id(), "owner:same", 90, 100)
        .unwrap();
    assert!(claim.leases.iter().all(|lease| lease.expires_at_ms == 200));
    let forward = recovery
        .heartbeat(candidate.id(), "owner:same", &claim.leases, 110, 100)
        .unwrap();
    let renewed = recovery
        .heartbeat(candidate.id(), "owner:same", &forward, 90, 100)
        .unwrap();
    assert!(renewed.iter().all(|lease| lease.expires_at_ms >= 210));
    let later = recovery
        .claim(candidate.id(), "owner:same", 211, 100)
        .unwrap();
    assert!(matches!(
        recovery.complete(
            candidate.id(),
            "owner:same",
            &claim.leases,
            json!({"done":true}),
            &[applied("clock.txt")],
            80
        ),
        Err(RecoveryError::LeaseFenced { .. })
    ));
    recovery
        .complete(
            candidate.id(),
            "owner:same",
            &later.leases,
            json!({"done":true}),
            &[applied("clock.txt")],
            70,
        )
        .unwrap();
    drop(recovery);
    drop(store);
    let connection = Connection::open(root.0.join("metadata/layrs.sqlite3")).unwrap();
    let row: (i64,i64,i64) = connection.query_row(
        "SELECT acquired_at_ms,heartbeat_at_ms,released_at_ms FROM materialization_leases WHERE resource_key='repo-path:clock.txt'", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!(row, (211, 211, 211));
    let time: (i64,i64) = connection.query_row(
        "SELECT updated_at_ms,completed_at_ms FROM recovery_operations WHERE recovery_operation_id='recovery:clock-rollback'", [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(time, (211, 211));
}

#[test]
fn failed_retry_requires_explicit_durable_authorization() {
    let root = initialize("retry", &["worktree:one"]);
    let mut store = open_exclusive(&root.0).unwrap();
    let mut recovery = NativeRecovery::new(&mut store).unwrap();
    let candidate = plan(
        "recovery:retry",
        "idem:retry",
        "worktree:one",
        "retry.txt",
        RecoveryType::Restore,
    );
    recovery.create(&candidate).unwrap();
    recovery
        .prepare(candidate.id(), &preparation(Some("retry.txt"), false), 11)
        .unwrap();
    let claim = recovery.claim(candidate.id(), "owner", 12, 100).unwrap();
    recovery
        .fail(
            candidate.id(),
            "owner",
            &claim.leases,
            json!({"retryable": false, "observed": "mismatch"}),
            "verification failed",
            &[RecoveryPathResult::new(
                "retry.txt",
                RecoveryPathRole::Destination,
                RecoveryPathState::RolledBack,
                json!({"restored": true}),
            )
            .unwrap()],
            13,
        )
        .unwrap();
    assert_eq!(
        recovery
            .prepare(candidate.id(), &preparation(Some("retry.txt"), false), 14,)
            .unwrap_err()
            .code(),
        "recovery_missing_evidence"
    );
    recovery
        .prepare(candidate.id(), &preparation(Some("retry.txt"), true), 14)
        .unwrap();
    assert_eq!(
        recovery.list_pending().unwrap()[0].state,
        RecoveryState::Prepared
    );
}

#[test]
fn terminal_requires_live_generation_even_when_owner_token_is_reused() {
    let root = initialize("terminal-aba", &["worktree:one"]);
    let mut store = open_exclusive(&root.0).unwrap();
    let mut recovery = NativeRecovery::new(&mut store).unwrap();
    let candidate = plan(
        "recovery:terminal-aba",
        "idem:terminal-aba",
        "worktree:one",
        "aba.txt",
        RecoveryType::Materialize,
    );
    recovery.create(&candidate).unwrap();
    recovery
        .prepare(candidate.id(), &preparation(Some("aba.txt"), false), 11)
        .unwrap();
    let expired = recovery
        .claim(candidate.id(), "owner:same", 12, 10)
        .unwrap();
    assert!(matches!(
        recovery.complete(
            candidate.id(),
            "owner:same",
            &expired.leases,
            json!({"done": true}),
            &[applied("aba.txt")],
            22,
        ),
        Err(RecoveryError::LeaseFenced { .. })
    ));

    let replacement = recovery
        .claim(candidate.id(), "owner:same", 23, 100)
        .unwrap();
    assert_eq!(
        replacement.leases[0].generation,
        expired.leases[0].generation + 1
    );
    assert!(matches!(
        recovery.complete(
            candidate.id(),
            "owner:same",
            &expired.leases,
            json!({"done": true}),
            &[applied("aba.txt")],
            24,
        ),
        Err(RecoveryError::LeaseFenced { .. })
    ));
    recovery
        .complete(
            candidate.id(),
            "owner:same",
            &replacement.leases,
            json!({"done": true}),
            &[applied("aba.txt")],
            24,
        )
        .unwrap();
}

#[test]
fn heartbeat_rejects_duplicate_resource_generations_before_mutation_in_both_orders() {
    let root = initialize("heartbeat-duplicates", &["worktree:one"]);
    let candidate = plan(
        "recovery:heartbeat-duplicates",
        "idem:heartbeat-duplicates",
        "worktree:one",
        "heartbeat.txt",
        RecoveryType::Materialize,
    );
    let (stale, active) = {
        let mut store = open_exclusive(&root.0).unwrap();
        let mut recovery = NativeRecovery::new(&mut store).unwrap();
        recovery.create(&candidate).unwrap();
        recovery
            .prepare(
                candidate.id(),
                &preparation(Some("heartbeat.txt"), false),
                11,
            )
            .unwrap();
        let stale = recovery
            .claim(candidate.id(), "owner:same", 12, 10)
            .unwrap()
            .leases
            .remove(0);
        let active = recovery
            .claim(candidate.id(), "owner:same", 23, 100)
            .unwrap()
            .leases
            .remove(0);
        assert_eq!(active.generation, stale.generation + 1);

        for expected in [
            vec![stale.clone(), active.clone()],
            vec![active.clone(), stale.clone()],
        ] {
            assert!(matches!(
                recovery.heartbeat(candidate.id(), "owner:same", &expected, 24, 200),
                Err(RecoveryError::OwnerMismatch(id)) if id == *candidate.id()
            ));
        }
        (stale, active)
    };

    let connection = Connection::open(root.0.join("metadata").join("layrs.sqlite3")).unwrap();
    let persisted: (String, u64, i64, i64, Option<i64>) = connection
        .query_row(
            "SELECT recovery_operation_id, generation, heartbeat_at_ms, expires_at_ms,
                    released_at_ms
             FROM materialization_leases
             WHERE worktree_id='worktree:one'
               AND resource_key='repo-path:heartbeat.txt'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        persisted,
        (
            candidate.id().as_str().to_owned(),
            active.generation,
            23,
            active.expires_at_ms,
            None,
        )
    );
    assert_eq!(stale.generation, 1);
    drop(connection);

    let mut store = open_exclusive(&root.0).unwrap();
    let mut recovery = NativeRecovery::new(&mut store).unwrap();
    let renewed = recovery
        .heartbeat(candidate.id(), "owner:same", &[active], 25, 100)
        .unwrap();
    assert_eq!(renewed[0].expires_at_ms, 125);
}

#[test]
fn recovery_id_fences_same_token_across_expiry_takeover_and_terminal_rollback() {
    let root = initialize("inter-record-aba", &["worktree:one"]);
    let first = plan(
        "recovery:inter-record-a",
        "idem:inter-record-a",
        "worktree:one",
        "shared.txt",
        RecoveryType::Materialize,
    );
    let second = plan(
        "recovery:inter-record-b",
        "idem:inter-record-b",
        "worktree:one",
        "shared.txt",
        RecoveryType::Restore,
    );
    let (first_claim, second_claim) = {
        let mut store = open_exclusive(&root.0).unwrap();
        let mut recovery = NativeRecovery::new(&mut store).unwrap();
        for candidate in [&first, &second] {
            recovery.create(candidate).unwrap();
            recovery
                .prepare(candidate.id(), &preparation(Some("shared.txt"), false), 11)
                .unwrap();
        }
        let first_claim = recovery.claim(first.id(), "owner:reused", 12, 10).unwrap();
        let second_claim = recovery
            .claim(second.id(), "owner:reused", 23, 100)
            .unwrap();
        assert_eq!(second_claim.leases[0].generation, 2);

        assert!(matches!(
            recovery.claim(first.id(), "owner:reused", 24, 100),
            Err(RecoveryError::LeaseFenced { .. })
        ));
        assert!(matches!(
            recovery.heartbeat(first.id(), "owner:reused", &first_claim.leases, 24, 100,),
            Err(RecoveryError::LeaseFenced { .. })
        ));
        assert!(matches!(
            recovery.complete(
                first.id(),
                "owner:reused",
                &first_claim.leases,
                json!({"done": true}),
                &[applied("shared.txt")],
                24,
            ),
            Err(RecoveryError::LeaseFenced { .. })
        ));
        assert!(matches!(
            recovery.fail(
                first.id(),
                "owner:reused",
                &first_claim.leases,
                json!({"retryable": true}),
                "stale claimant",
                &[applied("shared.txt")],
                24,
            ),
            Err(RecoveryError::LeaseFenced { .. })
        ));
        (first_claim, second_claim)
    };

    let database = root.0.join("metadata").join("layrs.sqlite3");
    let connection = Connection::open(&database).unwrap();
    let lease: (String, String, u64, Option<i64>) = connection
        .query_row(
            "SELECT recovery_operation_id, owner_token, generation, released_at_ms
             FROM materialization_leases
             WHERE worktree_id='worktree:one' AND resource_key='repo-path:shared.txt'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        lease,
        (
            second.id().as_str().to_owned(),
            "owner:reused".to_owned(),
            second_claim.leases[0].generation,
            None,
        )
    );
    let first_state: (String, String, String) = connection
        .query_row(
            "SELECT r.state, r.evidence_json, p.state
             FROM recovery_operations r
             JOIN recovery_paths p USING (recovery_operation_id)
             WHERE r.recovery_operation_id=?1",
            [first.id().as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        first_state,
        (
            "applying".to_owned(),
            r#"{"backupDurable":true,"retryable":false}"#.to_owned(),
            "verified".to_owned(),
        )
    );
    drop(connection);

    let mut store = open_exclusive(&root.0).unwrap();
    let mut recovery = NativeRecovery::new(&mut store).unwrap();
    recovery
        .complete(
            second.id(),
            "owner:reused",
            &second_claim.leases,
            json!({"done": true}),
            &[applied("shared.txt")],
            25,
        )
        .unwrap();
    assert_eq!(first_claim.leases[0].generation, 1);
}

#[test]
fn terminal_release_requires_exact_rowcount() {
    let root = initialize("terminal-rowcount", &["worktree:one"]);
    let candidate = plan(
        "recovery:terminal-rowcount",
        "idem:terminal-rowcount",
        "worktree:one",
        "rowcount.txt",
        RecoveryType::Restore,
    );
    let claim = {
        let mut store = open_exclusive(&root.0).unwrap();
        let mut recovery = NativeRecovery::new(&mut store).unwrap();
        recovery.create(&candidate).unwrap();
        recovery
            .prepare(
                candidate.id(),
                &preparation(Some("rowcount.txt"), false),
                11,
            )
            .unwrap();
        recovery
            .claim(candidate.id(), "owner:rowcount", 12, 100)
            .unwrap()
    };
    let connection = Connection::open(root.0.join("metadata").join("layrs.sqlite3")).unwrap();
    assert_eq!(
        connection
            .execute(
                "UPDATE materialization_leases SET released_at_ms=13
                 WHERE owner_token='owner:rowcount'",
                [],
            )
            .unwrap(),
        1
    );
    drop(connection);

    let mut store = open_exclusive(&root.0).unwrap();
    let mut recovery = NativeRecovery::new(&mut store).unwrap();
    assert!(matches!(
        recovery.complete(
            candidate.id(),
            "owner:rowcount",
            &claim.leases,
            json!({"done": true}),
            &[applied("rowcount.txt")],
            14,
        ),
        Err(RecoveryError::LeaseFenced { .. })
    ));
    assert_eq!(
        recovery.list_pending().unwrap()[0].state,
        RecoveryState::Applying
    );
}

#[test]
fn terminal_path_results_are_exact_and_persisted() {
    let root = initialize("terminal-paths", &["worktree:one"]);
    let mut store = open_exclusive(&root.0).unwrap();
    let mut recovery = NativeRecovery::new(&mut store).unwrap();
    let candidate = plan(
        "recovery:terminal-paths",
        "idem:terminal-paths",
        "worktree:one",
        "paths.txt",
        RecoveryType::Quarantine,
    );
    recovery.create(&candidate).unwrap();
    recovery
        .prepare(candidate.id(), &preparation(Some("paths.txt"), false), 11)
        .unwrap();
    let claim = recovery
        .claim(candidate.id(), "owner:paths", 12, 100)
        .unwrap();
    assert_eq!(
        recovery
            .complete(
                candidate.id(),
                "owner:paths",
                &claim.leases,
                json!({"done": true}),
                &[],
                13,
            )
            .unwrap_err()
            .code(),
        "recovery_missing_evidence"
    );
    let result = RecoveryPathResult::new(
        "paths.txt",
        RecoveryPathRole::Destination,
        RecoveryPathState::Quarantined,
        json!({"quarantineEntry": "q-1"}),
    )
    .unwrap();
    assert_eq!(
        recovery
            .complete(
                candidate.id(),
                "owner:paths",
                &claim.leases,
                json!({"done": true}),
                &[result.clone(), result.clone()],
                13,
            )
            .unwrap_err()
            .code(),
        "recovery_missing_evidence"
    );
    recovery
        .complete(
            candidate.id(),
            "owner:paths",
            &claim.leases,
            json!({"done": true}),
            std::slice::from_ref(&result),
            13,
        )
        .unwrap();
    assert_eq!(
        recovery
            .complete(
                candidate.id(),
                "owner:paths",
                &claim.leases,
                json!({"done": true}),
                std::slice::from_ref(&result),
                13,
            )
            .unwrap(),
        MutationOutcome::Replayed
    );
    drop(recovery);
    drop(store);
    let connection = Connection::open(root.0.join("metadata").join("layrs.sqlite3")).unwrap();
    let (state, evidence): (String, String) = connection
        .query_row(
            "SELECT state, evidence_json FROM recovery_paths
             WHERE recovery_operation_id=?1",
            [candidate.id().as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, "quarantined");
    assert_eq!(evidence, r#"{"quarantineEntry":"q-1"}"#);
}

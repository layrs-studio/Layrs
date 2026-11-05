//! SQL DATA only: genuine retained Published groups plus declared complete C
//! journals. No private B/C database, physical CAS, owner or durability proof is
//! manufactured here. ROOT runs each bounded child only after source release.
use super::{export_tests, installing_tests as installing, *};
use crate::projection::provider::owned_test as child;

macro_rules! owned {
    ($name:ident, $body:block) => {
        #[test]
        #[ignore = "ROOT source release and audited Protected SQL child120/outer180 required"]
        fn $name() {
            child::run(
                concat!(module_path!(), "::", stringify!($name)),
                120,
                || {
                    child::declare();
                    $body
                },
            );
        }
    };
}

fn expected(target: &ExportReserveOnlyTarget, required: &[FileManifest]) -> ExactProtection {
    let binding = target.binding();
    ExactProtection::new(
        &binding.family_incarnation,
        &binding.owner,
        &binding.database_identity,
        &binding.transaction_id,
        required,
        None,
    )
    .unwrap()
}

fn publish_data(
    c: &Connection,
    family: &[u8],
    retained: &AdmittedFamilyContext,
    target: &ExportReserveOnlyTarget,
    required: &[FileManifest],
) {
    retained
        .reserve_export_only(c, family, target, None)
        .unwrap();
    for manifest in required {
        // Every exact representation already belongs to a retained group. C
        // pays only its new journal metadata, never a second physical debit.
        installing::put(
            c,
            target.binding(),
            manifest,
            &installing::journal(target.binding(), manifest, "published", &[], false),
        );
    }
    let (_, state) = retained
        .validate_export_installing(c, family, target, required, None)
        .unwrap();
    assert_eq!(
        (state.records, state.published),
        (required.len() as u64, required.len() as u64)
    );
}

fn inspect(
    c: &Connection,
    family: &[u8],
    retained: &AdmittedFamilyContext,
    target: &ExportReserveOnlyTarget,
    required: &[FileManifest],
    protection: &ExactProtection,
) -> ExportProtectionState {
    let before = installing::state(c);
    let changes = c.total_changes();
    let (proof, state) = retained
        .validate_export_protection(c, family, target, required, protection, None)
        .unwrap();
    assert_eq!(
        proof
            .manifests()
            .iter()
            .map(|m| (m.file_id().to_string(), m.id().to_string(), m.byte_length()))
            .collect::<BTreeSet<_>>(),
        retained.primary.required
    );
    assert_eq!(c.total_changes(), changes);
    assert_eq!(installing::state(c), before);
    state
}

fn refuse_without_repair(
    c: &Connection,
    family: &[u8],
    retained: &AdmittedFamilyContext,
    target: &ExportReserveOnlyTarget,
    required: &[FileManifest],
    protection: &ExactProtection,
) {
    let before = installing::state(c);
    let changes = c.total_changes();
    assert!(
        retained
            .validate_export_protection(c, family, target, required, protection, None)
            .is_err()
    );
    assert!(
        retained
            .protect_export(c, family, target, required, protection, None)
            .is_err()
    );
    assert_eq!(
        c.total_changes(),
        changes,
        "Protected refusal must never repair"
    );
    assert_eq!(installing::state(c), before);
}

owned!(
    export_protection_inserts_only_exact_root_and_membership_and_replays_without_writes,
    {
        export_tests::fixture(|c, family, retained, target, required| {
            publish_data(c, family, retained, target, required);
            let protection = expected(target, required);
            let before = inspect(c, family, retained, target, required, &protection);
            assert_eq!(before.root, ProtectionPresence::Absent);
            let inventory = installing::state(c);
            let changes = c.total_changes();
            assert_eq!(
                retained
                    .protect_export(c, family, target, required, &protection, None)
                    .unwrap(),
                ProtectionEffect::Inserted
            );
            assert_eq!(c.total_changes() - changes, 1 + required.len() as u64);
            let after = inspect(c, family, retained, target, required, &protection);
            assert_eq!(after.root, ProtectionPresence::Present);
            assert_eq!(after.installation, before.installation);
            let actual = installing::state(c);
            for (table, rows) in &inventory {
                if !matches!(*table, "roots" | "root_manifests") {
                    assert_eq!(&actual[table], rows, "protection changed {table}");
                } else {
                    for row in rows {
                        assert!(
                            actual[table].contains(row),
                            "protection removed retained {table}"
                        );
                    }
                }
            }
            let changes = c.total_changes();
            let durable = installing::state(c);
            assert_eq!(
                retained
                    .protect_export(c, family, target, required, &protection, None)
                    .unwrap(),
                ProtectionEffect::AlreadyPresent
            );
            assert_eq!(c.total_changes(), changes);
            assert_eq!(installing::state(c), durable);
            // The new interpretation must not widen the earlier gates.
            assert!(retained.validate_catalog(c, family, None).is_err());
            assert!(
                retained
                    .validate_export_reserve_only(c, family, target, None)
                    .is_err()
            );
            assert!(
                retained
                    .validate_export_installing(c, family, target, required, None)
                    .is_err()
            );
            assert_eq!(c.total_changes(), changes);
            assert_eq!(installing::state(c), durable);
        });
    }
);

owned!(
    export_protection_refuses_altered_partial_colliding_or_foreign_roots_without_repair,
    {
        export_tests::fixture(|c, family, retained, target, required| {
            publish_data(c, family, retained, target, required);
            let protection = expected(target, required);
            retained
                .protect_export(c, family, target, required, &protection, None)
                .unwrap();
            for corruption in 0..12 {
                c.execute_batch("SAVEPOINT corrupt").unwrap();
                match corruption {
                    0 => {
                        c.execute(
                            "UPDATE roots SET payload=?1 WHERE id=?2",
                            params![b"{}".as_slice(), protection.id()],
                        )
                        .unwrap();
                    }
                    1 => {
                        c.execute(
                            "UPDATE roots SET digest=?1 WHERE id=?2",
                            params!["f".repeat(64), protection.id()],
                        )
                        .unwrap();
                    }
                    2 => {
                        // B exists in the registry but is foreign to C. Keep FK
                        // checks enabled so the fixture reaches the validator.
                        c.execute(
                            "UPDATE roots SET owner=?1 WHERE id=?2",
                            params![retained.binding().owner.as_str(), protection.id()],
                        )
                        .unwrap();
                    }
                    3 => {
                        c.execute(
                            "UPDATE roots SET database_identity='1:0:999' WHERE id=?1",
                            [protection.id()],
                        )
                        .unwrap();
                    }
                    4 => {
                        c.execute(
                            "UPDATE roots SET operation='ordinary-unrelated' WHERE id=?1",
                            [protection.id()],
                        )
                        .unwrap();
                    }
                    5 => {
                        c.execute(
                            "DELETE FROM root_manifests WHERE root_id=?1 AND manifest_id=?2",
                            params![protection.id(), required[0].id().as_str()],
                        )
                        .unwrap();
                    }
                    6 => {
                        let extra = super::tests::manifests(b"extra root member").remove(0);
                        installing::index(c, &extra);
                        c.execute(
                            "INSERT INTO root_manifests VALUES(?1,?2)",
                            params![protection.id(), extra.id().as_str()],
                        )
                        .unwrap();
                    }
                    7 => {
                        c.execute(
                            "INSERT INTO roots VALUES(?1,?2,?3,'ordinary-other',?1,?4)",
                            params![
                                "f".repeat(64),
                                target.binding().owner.as_str(),
                                target.binding().database_identity,
                                b"{}".as_slice()
                            ],
                        )
                        .unwrap();
                    }
                    8 => {
                        c.execute(
                            "INSERT INTO roots VALUES(?1,?2,?3,'provider-import:foreign',?1,?4)",
                            params![
                                "e".repeat(64),
                                retained.binding().owner.as_str(),
                                retained.binding().database_identity,
                                b"{}".as_slice()
                            ],
                        )
                        .unwrap();
                    }
                    9 => {
                        c.execute(
                            "UPDATE worktrees SET state='ready' WHERE id=?1",
                            [target.binding().owner.as_str()],
                        )
                        .unwrap();
                    }
                    10 => {
                        c.execute(
                            "INSERT INTO reservations VALUES('provider-transition:foreign',?1,'provider-transition:foreign',0)",
                            [retained.binding().owner.as_str()],
                        )
                        .unwrap();
                    }
                    11 => {
                        c.execute(
                            "UPDATE worktrees SET database_identity='1:0:999' WHERE id=?1",
                            [target.binding().owner.as_str()],
                        )
                        .unwrap();
                    }
                    _ => unreachable!(),
                }
                refuse_without_repair(c, family, retained, target, required, &protection);
                c.execute_batch("ROLLBACK TO corrupt; RELEASE corrupt")
                    .unwrap();
                assert_eq!(
                    inspect(c, family, retained, target, required, &protection).root,
                    ProtectionPresence::Present
                );
            }
        });
    }
);

owned!(
    export_protection_requires_every_c_journal_published_and_preserves_retained_validation,
    {
        export_tests::fixture(|c, family, retained, target, required| {
            publish_data(c, family, retained, target, required);
            let protection = expected(target, required);
            for phase in ["declared", "allocated", "verified", "publishing"] {
                c.execute_batch("SAVEPOINT incomplete").unwrap();
                installing::put(
                    c,
                    target.binding(),
                    &required[0],
                    &installing::journal(target.binding(), &required[0], phase, &[], false),
                );
                refuse_without_repair(c, family, retained, target, required, &protection);
                c.execute_batch("ROLLBACK TO incomplete; RELEASE incomplete")
                    .unwrap();
            }
            c.execute_batch("SAVEPOINT retained_corrupt").unwrap();
            c.execute(
                "UPDATE reservations SET remaining=remaining+1 WHERE id=?1",
                [&retained.binding().reservation],
            )
            .unwrap();
            refuse_without_repair(c, family, retained, target, required, &protection);
            c.execute_batch("ROLLBACK TO retained_corrupt; RELEASE retained_corrupt")
                .unwrap();
            // A complete canonical union with another binding cannot nominate a
            // retained root to bypass its original full-group interpretation.
            let wrong = ExactProtection::new(
                &retained.binding().family_incarnation,
                &retained.binding().owner,
                &retained.binding().database_identity,
                &retained.binding().transaction_id,
                required,
                None,
            )
            .unwrap();
            refuse_without_repair(c, family, retained, target, required, &wrong);
            let subset = expected(target, &required[..1]);
            refuse_without_repair(c, family, retained, target, &required[..1], &subset);
            let changes = c.total_changes();
            let before = installing::state(c);
            assert!(
                retained
                    .validate_export_protection(
                        c,
                        family,
                        target,
                        &required[..1],
                        &protection,
                        None
                    )
                    .is_err()
            );
            assert_eq!(c.total_changes(), changes);
            assert_eq!(installing::state(c), before);
        });
    }
);

owned!(
    export_protection_refuses_spent_metadata_envelope_even_with_complete_published_c,
    {
        export_tests::fixture(|c, family, retained, original, required| {
            let protection = expected(original, required);
            let margin = 1024 * 1024;
            let root_cost = required.len() as u64 * 256 + protection.payload().len() as u64 + 65536;
            // Deliberately declared underfunded Q0 DATA, never owner-derived plan
            // authority. Constructor funds the initial envelope exactly, while
            // publishing C's metadata spends part of it and must cause refusal.
            let target = export_tests::target_with_funding(
                retained,
                required,
                0,
                margin,
                margin - root_cost,
            );
            publish_data(c, family, retained, &target, required);
            let (_, installing) = retained
                .validate_export_installing(c, family, &target, required, None)
                .unwrap();
            assert!(
                protection
                    .require_envelope(installing.remaining, margin - root_cost)
                    .is_err()
            );
            refuse_without_repair(c, family, retained, &target, required, &protection);
            assert_eq!(
                c.query_row(
                    "SELECT count(*) FROM roots WHERE owner=?1",
                    [target.binding().owner.as_str()],
                    |r| r.get::<_, u64>(0)
                )
                .unwrap(),
                0
            );
        });
    }
);

owned!(
    export_protection_readonly_fk_off_replay_and_precancel_preserve_every_row,
    {
        export_tests::fixture(|c, family, retained, target, required| {
            publish_data(c, family, retained, target, required);
            let protection = expected(target, required);
            retained
                .protect_export(c, family, target, required, &protection, None)
                .unwrap();
            c.execute_batch("COMMIT; PRAGMA foreign_keys=OFF; BEGIN DEFERRED")
                .unwrap();
            assert_eq!(
                inspect(c, family, retained, target, required, &protection).root,
                ProtectionPresence::Present
            );
            let before = installing::state(c);
            let changes = c.total_changes();
            assert!(
                retained
                    .protect_export(c, family, target, required, &protection, None)
                    .is_err()
            );
            let cancel = Cancellation::default();
            cancel.cancel();
            assert!(
                retained
                    .validate_export_protection(
                        c,
                        family,
                        target,
                        required,
                        &protection,
                        Some(&cancel)
                    )
                    .is_err()
            );
            assert!(
                retained
                    .protect_export(c, family, target, required, &protection, Some(&cancel))
                    .is_err()
            );
            assert_eq!(c.total_changes(), changes);
            assert_eq!(installing::state(c), before);
            c.execute_batch("ROLLBACK; PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
                .unwrap();
        });
    }
);

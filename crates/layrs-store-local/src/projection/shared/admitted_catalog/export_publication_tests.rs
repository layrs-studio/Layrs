//! Lower full-family SQL DATA/CAS tests, not private Ready or physical proofs.
//! Public entries additionally require Ready/receipt validated by require_ready;
//! ROOT must connect its real private-Ready fixture for that integration case.
use super::super::{export_tests, installing_tests as installing};
use super::*;
use crate::projection::provider::owned_test as child;
fn protected(
    c: &Connection,
    family: &[u8],
    retained: &AdmittedFamilyContext,
    target: &ExportReserveOnlyTarget,
    required: &[FileManifest],
) -> ExactProtection {
    retained
        .reserve_export_only(c, family, target, None)
        .unwrap();
    for m in required {
        installing::put(
            c,
            target.binding(),
            m,
            &installing::journal(target.binding(), m, "published", &[], false),
        );
    }
    let b = target.binding();
    let p = ExactProtection::new(
        &b.family_incarnation,
        &b.owner,
        &b.database_identity,
        &b.transaction_id,
        required,
        None,
    )
    .unwrap();
    retained
        .protect_export(c, family, target, required, &p, None)
        .unwrap();
    p
}
fn inspect(
    c: &Connection,
    family: &[u8],
    retained: &AdmittedFamilyContext,
    target: &ExportReserveOnlyTarget,
    required: &[FileManifest],
    p: &ExactProtection,
) -> ExportPublicationState {
    let rows = installing::state(c);
    let changes = c.total_changes();
    let member = member_phase(c, target, None).unwrap();
    let parts = publication_parts(retained, c, family, target, required, p, member, None).unwrap();
    let protection = parts.protection.unwrap();
    assert_eq!(protection.root, ProtectionPresence::Present);
    assert_eq!(installing::state(c), rows);
    assert_eq!(c.total_changes(), changes);
    ExportPublicationState { member, protection }
}
#[test]
#[ignore = "ROOT source release and audited family publication DATA child120/outer180 required"]
fn export_publication_fullwalk_cas_changes_only_c_member_and_ro_replay_preserves_accounting() {
    let test = concat!(
        module_path!(),
        "::export_publication_fullwalk_cas_changes_only_c_member_and_ro_replay_preserves_accounting"
    );
    child::run(test, 120, || {
        child::declare();
        export_tests::fixture(|c, family, retained, target, required| {
            let p = protected(c, family, retained, target, required);
            let before = inspect(c, family, retained, target, required, &p);
            assert_eq!(before.member, ExportPublicationMember::Initializing);
            target
                .require_publication_envelope(before.protection, &p, 65536, 16384, 0)
                .unwrap();
            for (floor, journal, bytes) in [
                (0, 16384, 0),
                (65536, u64::MAX, 0),
                (65536, 16384, u64::MAX),
            ] {
                assert!(
                    target
                        .require_publication_envelope(before.protection, &p, floor, journal, bytes)
                        .is_err()
                );
            }
            let rows = installing::state(c);
            let changes = c.total_changes();
            compare_and_publish(c, target).unwrap();
            assert_eq!(c.total_changes() - changes, 1);
            let after = inspect(c, family, retained, target, required, &p);
            assert_eq!(after.member, ExportPublicationMember::Ready);
            assert_eq!(after.protection, before.protection);
            let actual = installing::state(c);
            let owner = installing::Cell::Text(target.binding().owner.as_str().as_bytes().to_vec());
            let old_members = &rows["worktrees"];
            let new_members = &actual["worktrees"];
            assert_eq!(new_members.len(), old_members.len());
            for member in old_members {
                if member.first() != Some(&owner) {
                    assert!(
                        new_members.contains(member),
                        "publication changed another member"
                    );
                } else {
                    let observed = new_members
                        .iter()
                        .find(|row| row.first() == Some(&owner))
                        .unwrap();
                    assert_eq!(&observed[..4], &member[..4]);
                    assert_eq!(observed[4], installing::Cell::Text(b"ready".to_vec()));
                }
            }
            for (table, old) in &rows {
                if *table != "worktrees" {
                    assert_eq!(&actual[table], old, "publication changed {table}");
                }
            }
            let changes = c.total_changes();
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
            assert!(
                retained
                    .validate_export_protection(c, family, target, required, &p, None)
                    .is_err()
            );
            assert_eq!(installing::state(c), actual);
            assert_eq!(c.total_changes(), changes);
            c.execute_batch(
                "COMMIT; PRAGMA foreign_keys=OFF; PRAGMA query_only=ON; BEGIN DEFERRED",
            )
            .unwrap();
            assert_eq!(inspect(c, family, retained, target, required, &p), after);
            let cancel = Cancellation::default();
            cancel.cancel();
            assert!(
                publication_parts(
                    retained,
                    c,
                    family,
                    target,
                    required,
                    &p,
                    ExportPublicationMember::Ready,
                    Some(&cancel)
                )
                .is_err()
            );
            assert_eq!(installing::state(c), actual);
            assert_eq!(c.total_changes(), changes);
            // The outer retained fixture owns the final transaction rollback.
            assert!(!c.is_autocommit());
        });
    });
}
#[test]
#[ignore = "ROOT source release and audited family publication refusal DATA child120/outer180 required"]
fn export_publication_fullwalk_refuses_corrupt_registry_roots_debits_and_incomplete_journals_without_repair()
 {
    let test = concat!(
        module_path!(),
        "::export_publication_fullwalk_refuses_corrupt_registry_roots_debits_and_incomplete_journals_without_repair"
    );
    child::run(test, 120, || {
        child::declare();
        export_tests::fixture(|c, family, retained, target, required| {
            let p = protected(c, family, retained, target, required);
            let owner = target.binding().owner.as_str();
            for sql in [
                "UPDATE worktrees SET metadata_path='foreign' WHERE id=?1",
                "UPDATE worktrees SET database_identity='foreign' WHERE id=?1",
                "UPDATE worktrees SET marker=x'00' WHERE id=?1",
                "UPDATE worktrees SET state='ready' WHERE id=?1",
                "UPDATE roots SET payload=x'00' WHERE owner=?1",
                "UPDATE reservations SET remaining=remaining+1 WHERE owner=?1",
            ] {
                c.execute_batch("SAVEPOINT corrupt").unwrap();
                assert_eq!(c.execute(sql, [owner]).unwrap(), 1);
                let rows = installing::state(c);
                let changes = c.total_changes();
                assert!(
                    publication_parts(
                        retained,
                        c,
                        family,
                        target,
                        required,
                        &p,
                        ExportPublicationMember::Initializing,
                        None
                    )
                    .is_err()
                );
                assert_eq!(installing::state(c), rows);
                assert_eq!(c.total_changes(), changes);
                c.execute_batch("ROLLBACK TO corrupt; RELEASE corrupt")
                    .unwrap();
            }
            for absent in [true, false] {
                c.execute_batch("SAVEPOINT root_corrupt").unwrap();
                if absent {
                    assert_eq!(
                        c.execute("DELETE FROM root_manifests WHERE root_id=?1", [p.id()])
                            .unwrap(),
                        required.len()
                    );
                    assert_eq!(
                        c.execute("DELETE FROM roots WHERE id=?1", [p.id()])
                            .unwrap(),
                        1
                    );
                } else {
                    assert_eq!(c.execute("INSERT INTO roots(id,owner,database_identity,operation,digest,payload) SELECT ?1,owner,database_identity,operation,digest,payload FROM roots WHERE id=?2", params!["f".repeat(64),p.id()]).unwrap(), 1);
                }
                let rows = installing::state(c);
                let changes = c.total_changes();
                assert!(
                    publication_parts(
                        retained,
                        c,
                        family,
                        target,
                        required,
                        &p,
                        ExportPublicationMember::Initializing,
                        None
                    )
                    .is_err()
                );
                assert_eq!(installing::state(c), rows);
                assert_eq!(c.total_changes(), changes);
                c.execute_batch("ROLLBACK TO root_corrupt; RELEASE root_corrupt")
                    .unwrap();
            }
            for phase in ["declared", "allocated", "verified", "publishing"] {
                c.execute_batch("SAVEPOINT incomplete").unwrap();
                installing::put(
                    c,
                    target.binding(),
                    &required[0],
                    &installing::journal(target.binding(), &required[0], phase, &[], false),
                );
                let rows = installing::state(c);
                let changes = c.total_changes();
                assert!(
                    publication_parts(
                        retained,
                        c,
                        family,
                        target,
                        required,
                        &p,
                        ExportPublicationMember::Initializing,
                        None
                    )
                    .is_err()
                );
                assert_eq!(installing::state(c), rows);
                assert_eq!(c.total_changes(), changes);
                c.execute_batch("ROLLBACK TO incomplete; RELEASE incomplete")
                    .unwrap();
            }
            c.execute_batch("SAVEPOINT retained").unwrap();
            assert_eq!(
                c.execute(
                    "UPDATE reservations SET remaining=remaining+1 WHERE id=?1",
                    [&retained.binding().reservation]
                )
                .unwrap(),
                1
            );
            let rows = installing::state(c);
            let changes = c.total_changes();
            assert!(
                publication_parts(
                    retained,
                    c,
                    family,
                    target,
                    required,
                    &p,
                    ExportPublicationMember::Initializing,
                    None
                )
                .is_err()
            );
            assert_eq!(installing::state(c), rows);
            assert_eq!(c.total_changes(), changes);
            c.execute_batch("ROLLBACK TO retained; RELEASE retained; COMMIT; PRAGMA foreign_keys=OFF; BEGIN DEFERRED").unwrap();
            let rows = installing::state(c);
            let changes = c.total_changes();
            assert!(compare_and_publish(c, target).is_err());
            assert_eq!(installing::state(c), rows);
            assert_eq!(c.total_changes(), changes);
            assert_eq!(
                inspect(c, family, retained, target, required, &p).member,
                ExportPublicationMember::Initializing
            );
            // The outer retained fixture owns the final transaction rollback.
            assert!(!c.is_autocommit());
        });
    });
}

//! Genuine schema9 source planning after ordinary capture and restore. The
//! scanned image is an independent DATA witness, not physical export authority.
//! No destination allocation, B-to-C transport or admission is exercised here.
use crate::projection::{
    capture::{self, CapturedFile},
    database, format,
    provider::{
        admitted_ordinary::{self, AdmittedOrdinaryData},
        import_validation::{self, ImportDatabaseKind},
        managed_admission::read_admitted_import,
        process_profile::tests::child,
        records::{
            import::{ImportSource, ImportSourceReferences, ProviderImportPrefixRecord},
            Record,
        },
    },
    shared, ProjectionIdentity, ProjectionStore, RestoreSource, SessionMode,
};
use layrs_core::scm::{FileId, FileManifestId, RepoPath, WorktreeId};
use std::{collections::BTreeMap, fs};

const TARGET: &[u8] = b"admitted export capture A";
const DIRTY: &[u8] = b"admitted export restore-only preimage B";

#[derive(Debug, PartialEq, Eq)]
struct PlannedData {
    prefix: Vec<u8>,
    image: String,
    mappings: BTreeMap<FileId, (FileManifestId, u64)>,
}

fn plan_under_owner(
    store: &ProjectionStore,
    destination: &ProjectionIdentity,
    preimage: &CapturedFile,
    baseline: &AdmittedOrdinaryData,
    expected_committed_budgets: u64,
    refusals: bool,
) -> PlannedData {
    // Access::run positively closes the admitted private handle before its
    // owner and family scopes drop. No SQL proof or ContentScope escapes it.
    format::access(store, SessionMode::Shared)
        .unwrap()
        .run(|access| {
            access.with_content(|c, scope| {
                let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)?;
                let changes: u64 = tx.query_row("SELECT total_changes()", [], |r| r.get(0))?;
                let data_version: u64 = tx.query_row("PRAGMA data_version", [], |r| r.get(0))?;
                assert_eq!(
                    tx.query_row(
                        "SELECT count(*) FROM projection_budgets WHERE state='committed'",
                        [],
                        |r| r.get::<_, u64>(0),
                    )?,
                    expected_committed_budgets,
                    "ordinary reopen settles the completed restore budget"
                );
                let state = database::current(&tx)?;
                let scan = capture::scan::scan_with_scope(
                    store,
                    &tx,
                    &state.snapshot,
                    false,
                    Some(scope),
                )?;
                // Production ordinary retention uses this same complete scan
                // witness. It hashes actual selected checkout bytes and modes;
                // the SQL planner receives only its independently derived DATA.
                let image = shared::digest(&scan.files)?;
                assert_eq!(
                    scan.files["README.md"].as_ref().unwrap().file_id,
                    FileId::from_bytes(TARGET)
                );
                assert!(scan
                    .files
                    .values()
                    .flatten()
                    .all(|file| file.file_id != preimage.file_id));
                let (prefix, proof) = import_validation::plan_admitted_import(
                    &tx,
                    &store.marker,
                    destination.clone(),
                    &image,
                    None,
                )?;
                assert!(!tx.is_autocommit());
                proof.require_source_connection(&tx)?;
                assert_eq!(proof.closure(), &prefix.closure);
                let binding = prefix.source_admission().unwrap();
                let admitted = read_admitted_import(&tx, &store.marker, None)?;
                assert_eq!(binding.admission_digest, admitted.record().digest()?);
                assert_eq!(binding.incoming_prefix_id, admitted.prefix().digest()?);
                assert_eq!(binding.incoming_receipt_id, admitted.receipt().digest()?);
                assert_eq!(
                    binding.database_identity,
                    store.marker.shared.as_ref().unwrap().database_identity
                );
                assert_eq!(
                    prefix.ancestry_root_receipt.as_deref(),
                    Some(binding.incoming_receipt_id.as_str())
                );
                match &prefix.source {
                    ImportSource::Ordinary {
                        private_schema,
                        checkout_identity,
                        version,
                        ..
                    } => {
                        assert_eq!(*private_schema, 9);
                        assert_eq!(
                            checkout_identity.as_deref(),
                            Some(store.marker.checkout_identity())
                        );
                        assert_eq!(version.effective_image_digest, image);
                    }
                    _ => panic!("actual admitted ordinary source must remain ordinary"),
                }
                assert_eq!(prefix.closure.restore_history.as_ref().unwrap().count, 1);
                let mappings = proof.copied_content_mappings(&tx, &prefix, None)?;
                let manifest =
                    capture::storage::file_manifest_with_scope(&tx, store, preimage, Some(scope))?;
                assert_eq!(
                    mappings.get(&preimage.file_id),
                    Some(&(manifest.id().clone(), preimage.bytes))
                );
                let mut retained = Vec::new();
                capture::storage::stream_file_with_scope(
                    &tx,
                    store,
                    preimage,
                    &mut retained,
                    Some(scope),
                )?;
                assert_eq!(retained, DIRTY);

                if refusals {
                    // Each false admission digest is present in both the
                    // prefix and supplied reference DATA. The reference codec
                    // succeeds; the genuine SQL admission must reject it.
                    for digit in ["a", "b"] {
                        let mut forged = ProviderImportPrefixRecord::decode(&prefix.encode()?)?;
                        let mut false_binding = binding.clone();
                        false_binding.admission_digest = format!("blake3:{}", digit.repeat(64));
                        assert_ne!(false_binding.admission_digest, binding.admission_digest);
                        let ImportSource::Ordinary { admission, .. } = &mut forged.source else {
                            unreachable!()
                        };
                        *admission = Some(false_binding.clone());
                        let references = || ImportSourceReferences::AdmittedOrdinary {
                            marker: &store.marker,
                            admission: &false_binding,
                        };
                        forged.require_references(references())?;
                        let failure = import_validation::verify_source(
                            &tx,
                            &store.marker,
                            ImportDatabaseKind::AdmittedOrdinary,
                            &forged,
                            references(),
                            None,
                            &image,
                            None,
                        );
                        assert!(failure.is_err());
                        assert!(failure
                            .err()
                            .unwrap()
                            .to_string()
                            .contains("genuine admission"));
                    }
                    let false_image = "c".repeat(64);
                    assert_ne!(false_image, image);
                    assert!(import_validation::verify_source(
                        &tx,
                        &store.marker,
                        ImportDatabaseKind::AdmittedOrdinary,
                        &prefix,
                        ImportSourceReferences::AdmittedOrdinary {
                            marker: &store.marker,
                            admission: binding,
                        },
                        None,
                        &false_image,
                        None,
                    )
                    .is_err());
                    assert!(import_validation::verify_source(
                        &tx,
                        &store.marker,
                        ImportDatabaseKind::AdmittedOrdinary,
                        &prefix,
                        ImportSourceReferences::Ordinary(&store.marker),
                        None,
                        &image,
                        None,
                    )
                    .is_err());
                }
                baseline.require_same_import(&admitted_ordinary::inspect(
                    &tx,
                    &store.marker,
                    None,
                )?)?;
                let after = capture::scan::scan_with_scope(
                    store,
                    &tx,
                    &state.snapshot,
                    false,
                    Some(scope),
                )?;
                assert_eq!(shared::digest(&after.files)?, image);
                scope.verify(&tx, store)?;
                assert_eq!(
                    tx.query_row("SELECT total_changes()", [], |r| r.get::<_, u64>(0))?,
                    changes,
                    "planning and refused DATA substitutions execute no SQL writes"
                );
                assert_eq!(
                    tx.query_row("PRAGMA data_version", [], |r| r.get::<_, u64>(0))?,
                    data_version
                );
                let prefix = prefix.encode()?;
                tx.rollback()?;
                assert!(c.is_autocommit());
                Ok(PlannedData {
                    prefix,
                    image,
                    mappings,
                })
            })
        })
        .unwrap()
}

#[test]
#[ignore = "ROOT source release and audited owned native child600/outer660 required"]
fn managed_admitted_export_plans_restore_preimage_without_mutating_admission() {
    child::run(
        concat!(
            module_path!(),
            "::managed_admitted_export_plans_restore_preimage_without_mutating_admission"
        ),
        600,
        || {
            child::declare();
            let root = super::managed_restore_recovery_tests::admitted_destination();
            let store = ProjectionStore::open(&root).unwrap();
            let baseline = format::access(&store, SessionMode::Shared)
                .unwrap()
                .run(|access| {
                    admitted_ordinary::inspect(&access.connection, &store.marker, None)
                        .map_err(Into::into)
                })
                .unwrap();
            let state = store.state().unwrap();
            let path = RepoPath::parse("README.md").unwrap();
            fs::write(root.join(path.as_str()), TARGET).unwrap();
            let target = store.capture_patch(state.generation).unwrap();
            fs::write(root.join(path.as_str()), DIRTY).unwrap();
            let preview = store
                .preview_restore(
                    state.generation,
                    &RestoreSource::Capture {
                        sequence: target.sequence,
                        paths: vec![path.clone()],
                    },
                )
                .unwrap();
            let preimage = preview.before[&path].as_ref().unwrap().clone();
            assert_eq!(preimage.file_id, FileId::from_bytes(DIRTY));
            let restored = store.apply_restore(&preview).unwrap();
            assert_eq!(restored.capture.sequence, target.sequence + 1);
            assert_eq!(fs::read(root.join(path.as_str())).unwrap(), TARGET);
            for sequence in 1..=restored.capture.sequence {
                let patch = store.captured_patch(sequence).unwrap();
                assert!(patch
                    .changes
                    .iter()
                    .filter_map(|change| change.replacement.as_ref())
                    .all(|file| file.file_id != preimage.file_id));
            }
            let mut destination = store.identity().clone();
            destination.worktree_id =
                WorktreeId::parse("worktree:admitted-export-planned-C").unwrap();
            let family = store.marker.shared.as_ref().unwrap().root.clone();
            let before = (
                super::managed_ready_reopen_tests::tree(&root),
                super::managed_ready_reopen_tests::tree(&family),
            );
            let first = plan_under_owner(&store, &destination, &preimage, &baseline, 1, true);
            assert_eq!(
                (
                    super::managed_ready_reopen_tests::tree(&root),
                    super::managed_ready_reopen_tests::tree(&family),
                ),
                before,
                "positive planning and all DATA refusals preserve checkout, private SQL and family"
            );
            drop(store);
            let reopened = ProjectionStore::open(&root).unwrap();
            // Ordinary open takes Exclusive access and settles the committed
            // restore budget: it releases the family reservation, then deletes
            // the private budget. Keep full DB byte witnesses around each
            // planner, without attributing authorized reopen writes to it.
            let after_open = (
                super::managed_ready_reopen_tests::tree(&root),
                super::managed_ready_reopen_tests::tree(&family),
            );
            assert_eq!(after_open.0, before.0, "reopen preserves checkout bytes");
            let again = plan_under_owner(&reopened, &destination, &preimage, &baseline, 0, false);
            assert_eq!(
                again, first,
                "planning is stable after positive close and reopen"
            );
            assert_eq!(
                (
                    super::managed_ready_reopen_tests::tree(&root),
                    super::managed_ready_reopen_tests::tree(&family),
                ),
                after_open,
                "reopened planning preserves the complete private and family byte trees"
            );
        },
    );
}

//! Proposed native released-C10 -> D10 export with an explicitly granted
//! nested addition and positive README deletion. Source only until ROOT builds
//! and runs the ignored recipe; no crash, owner-loss or close-failure claim.
use super::{
    managed_ordinary_autonomy_tests::released_source10,
    managed_ordinary_family_ready_tests::{family, text, Close},
    managed_ordinary_installing_tests::{self as witness, Cell, Database},
    managed_ordinary_ready_tests::CURRENT,
    managed_ordinary_source_publication_tests::{self as publication, HISTORY},
};
use crate::projection::{
    provider::{
        admitted_ordinary,
        checkout_journal::{CheckoutFile, ExpectedCheckout},
        destination_ready::ReadyEffect,
        managed_admission::read_admitted_import,
        managed_export::{allocation, AdmissionStep, OrdinaryReadyAdmission},
        ordinary_destination::{staging::StageEffect, CopyEffect},
        process_profile::tests::child,
        records::{
            import::ImportSource,
            transition::{TransitionRole, TransitionState},
        },
        transition_sql::SourceIntentEffect,
    },
    shared::{
        admitted_catalog::{ExportPublicationEffect, ExportReservationEffect},
        provider_access::ImportRegistrationEffect,
    },
    CapturedPatch, ProjectionStore,
};
use layrs_core::{
    projection::EntryState,
    scm::{FileId, FileManifest, RepoPath, TreeEntryTarget, WorktreeId},
};
use layrs_vfs::Cancellation;
use std::{fs, path::Path};

const LEAF: &str = "00000000000000000000000000000141";
const TX: &str = "00000000000000000000000000000142";
const NESTED: &[u8] = b"ordinary C10 explicitly granted nested addition";

fn snapshot(path: &Path) -> Database {
    witness::read(path, witness::database)
}

/// Only the real outgoing intent and its receipt may change. In particular
/// this includes the incoming admission/archive, projection head, selections,
/// capture frontier/payloads, content mappings and restore history unchanged.
fn require_source_rows_unchanged(before: &Database, after: &Database) {
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>()
    );
    for (table, rows) in before {
        if table == "projection_provider_transitions"
            || table == "projection_provider_transition_receipts"
        {
            assert_eq!(after[table].len(), rows.len() + 1);
            assert!(rows.iter().all(|row| after[table].contains(row)));
        } else {
            assert_eq!(&after[table], rows, "C export changed {table}");
        }
    }
}

fn captured_bytes(store: &ProjectionStore, patch: &CapturedPatch) -> Vec<(RepoPath, Vec<u8>)> {
    patch
        .changes
        .iter()
        .filter_map(|change| {
            let replacement = change.replacement.as_ref()?;
            let expected = [HISTORY, CURRENT, NESTED]
                .into_iter()
                .find(|bytes| FileId::from_bytes(bytes) == replacement.file_id)
                .expect("capture must reference genuine B/C fixture bytes");
            let mut bytes = Vec::new();
            store
                .write_captured_file(patch.sequence, &change.path, &mut bytes)
                .unwrap();
            assert_eq!(bytes, expected);
            Some((change.path.clone(), bytes))
        })
        .collect()
}

#[test]
#[ignore = "ROOT source release and audited owned native child5400/outer5460 required; proposed unmeasured"]
fn managed_ordinary_source10_exports_nested_and_deleted_paths_to_d10_then_reopens() {
    // Finite proposal only: fresh B publication/C10 admission, then one complete
    // C10 export/admission and released-holder public D10/C10 readback.
    child::run(
        concat!(
            module_path!(),
            "::managed_ordinary_source10_exports_nested_and_deleted_paths_to_d10_then_reopens"
        ),
        5400,
        source10_export_recipe,
    );
}

fn source10_export_recipe() {
    let (root, admission) = released_source10();
    assert_eq!(admission.private_schema(), 10);
    let b_identity = match &admission.source_published().plan.source_prefix.source {
        ImportSource::Ordinary {
            identity,
            private_schema: 9,
            ..
        } => identity.clone(),
        _ => panic!("fresh C10 must carry genuine B9 incoming provenance"),
    };
    let cancel = Cancellation::default();
    // Fresh public access, after the original B owner and all C proofs/store
    // holders are gone. This recipe never reuses their physical authority.
    let store = ProjectionStore::open(&root).unwrap();
    let original = store.state().unwrap();
    let readme = RepoPath::parse("README.md").unwrap();
    let nested = RepoPath::parse("dir/sub/extra.txt").unwrap();
    let readme_entry = original
        .snapshot
        .entries
        .iter()
        .find(|e| e.path == readme)
        .unwrap();
    assert!(readme_entry.writable);
    let Some(TreeEntryTarget::File {
        file_id: readme_base,
    }) = &readme_entry.target
    else {
        panic!("README deletion requires a known selected regular-file base")
    };
    assert_eq!(fs::read(root.join(readme.as_str())).unwrap(), CURRENT);
    assert!(!root.join(nested.as_str()).try_exists().unwrap());
    // Explicit writable absence is the permission source. Mere physical
    // absence above is only a precondition and cannot authorize creation.
    let state = store
        .observe_absent_paths(original.generation, &original.digest, &[nested.clone()])
        .unwrap();
    let grant = state
        .snapshot
        .entries
        .iter()
        .find(|e| e.path == nested)
        .unwrap();
    assert_eq!(grant.state, EntryState::Absent);
    assert!(grant.writable && grant.target.is_none());
    assert_eq!(state.generation, original.generation + 1);
    fs::create_dir_all(root.join("dir/sub")).unwrap();
    fs::write(root.join(nested.as_str()), NESTED).unwrap();
    let added = store.capture_patch(state.generation).unwrap();
    assert_eq!(added.sequence, admission.ready_capture_sequence() + 1);
    assert_eq!(added.identity, *store.identity());
    assert_eq!(
        added.changes.len(),
        2,
        "capture is a full diff, including existing CURRENT README"
    );
    let addition = added.changes.iter().find(|c| c.path == nested).unwrap();
    assert!(addition.expected.is_none());
    assert_eq!(
        addition.replacement.as_ref().unwrap().file_id,
        FileId::from_bytes(NESTED)
    );
    fs::remove_file(root.join(readme.as_str())).unwrap();
    let deleted = store.capture_patch(state.generation).unwrap();
    assert_eq!(deleted.sequence, added.sequence + 1);
    assert_eq!(deleted.identity, *store.identity());
    assert_eq!(deleted.changes.len(), 2);
    let deletion = deleted.changes.iter().find(|c| c.path == readme).unwrap();
    assert_eq!(&deletion.expected.as_ref().unwrap().file_id, readme_base);
    assert!(deletion.replacement.is_none());
    assert_eq!(store.capture_patch(state.generation).unwrap(), deleted);
    assert_eq!(
        store.state().unwrap(),
        state,
        "captures preserve the granted projection"
    );
    let captures: Vec<_> = (1..=deleted.sequence)
        .map(|sequence| {
            let patch = store.captured_patch(sequence).unwrap();
            let bytes = captured_bytes(&store, &patch);
            (patch, bytes)
        })
        .collect();
    assert!(captures
        .iter()
        .any(|(patch, bytes)| patch.identity == b_identity
            && bytes.iter().any(|(_, bytes)| bytes == HISTORY)));
    assert!(captures
        .iter()
        .any(|(patch, bytes)| patch.identity == *store.identity()
            && bytes.iter().any(|(_, bytes)| bytes == CURRENT)));
    assert!(captures
        .iter()
        .any(|(patch, bytes)| patch.identity == *store.identity()
            && bytes.iter().any(|(_, bytes)| bytes == NESTED)));
    let source_marker = store.marker.clone();
    let source_database = source_marker
        .metadata_path(&root)
        .join("projection.sqlite3");
    let original_ingress = witness::read(&source_database, |c| {
        publication::ingress(c, &source_marker)
    });
    let mut before_source = snapshot(&source_database);
    admission
        .require_same_import(&witness::read(&source_database, |c| {
            admitted_ordinary::inspect(c, &source_marker, None).unwrap()
        }))
        .unwrap();
    let mut identity = store.identity().clone();
    identity.worktree_id = WorktreeId::parse("worktree:ordinary-source10-destination-d").unwrap();
    drop(store);

    let store = ProjectionStore::open_for_managed_export(&root).unwrap();
    assert_eq!(store.state().unwrap(), state);
    assert_eq!(
        snapshot(&source_database),
        before_source,
        "shared discovery must not settle the capture budget"
    );
    let family_root = store.marker.shared.as_ref().unwrap().root.clone();
    let family_database = family_root.join("metadata/catalog.sqlite3");
    // The last public capture leaves a committed ordinary budget. Exclusive
    // begin_managed_export settles it before creating any outgoing intent.
    // Witness that exact cleanup separately, then retain full-table equality
    // for every later export step rather than ignoring the budgets table.
    let budget = &before_source["projection_budgets"];
    assert_eq!(budget.len(), 1);
    let row = &budget[0];
    assert_eq!(row.len(), 6);
    assert_eq!(row[0], Cell::Integer(1));
    assert_eq!(row[1], text(&format!("capture:{}", state.digest)));
    assert_eq!(row[5], text("committed"));
    let Cell::Text(completed_reservation) = &row[2] else {
        panic!("committed capture reservation must be text")
    };
    let mut settled_family = snapshot(&family_database);
    let reservations = settled_family.get_mut("reservations").unwrap();
    let ordinal = reservations
        .iter()
        .position(|row| row.first() == Some(&Cell::Text(completed_reservation.clone())))
        .expect("committed capture reservation exists before settlement");
    let reservation = reservations.remove(ordinal);
    assert_eq!(reservation.len(), 4);
    assert_eq!(
        reservation[1],
        text(source_marker.identity.worktree_id.as_str())
    );
    assert_eq!(reservation[2], row[1]);
    before_source.get_mut("projection_budgets").unwrap().clear();
    let mut owner = store
        .begin_managed_export(state.generation, identity, &cancel)
        .unwrap();
    assert_eq!(
        snapshot(&source_database),
        before_source,
        "owner acquisition only settles the exact committed budget"
    );
    assert_eq!(
        snapshot(&family_database),
        settled_family,
        "settlement only releases its exact family reservation"
    );
    assert!(owner.current_manifest(&readme).is_none());
    let current = owner.current_manifest(&nested).unwrap().clone();
    assert_eq!(current.file_id(), &FileId::from_bytes(NESTED));
    let (planned, effect) = owner
        .begin_plan(root.parent().unwrap(), LEAF, TX, &cancel)
        .unwrap();
    assert_eq!(effect, SourceIntentEffect::Inserted);
    match &planned.plan.source_prefix.source {
        ImportSource::Ordinary {
            private_schema,
            admission: Some(binding),
            version,
            ..
        } => {
            assert_eq!(*private_schema, 10);
            assert_eq!(binding.admission_digest, admission.admission_digest());
            assert_eq!(version.generation, state.generation);
            assert_eq!(version.capture_count, deleted.sequence);
            assert_eq!(
                version.capture_head.as_deref(),
                Some(deleted.digest.as_str())
            );
        }
        _ => panic!("export must authenticate actual admitted C10 as its source"),
    }
    let derived = owner
        .derive_plan(
            &planned.plan.destination_parent,
            &planned.plan.destination_parent_identity,
            LEAF,
            TX,
            &cancel,
        )
        .unwrap();
    assert_eq!(derived.plan, planned.plan);
    let required: Vec<FileManifest> = derived
        .required_manifests
        .iter()
        .map(|row| {
            let manifest = if row.manifest_id == *current.id() {
                current.clone()
            } else {
                owner.retained_manifest(&row.file_id).unwrap().clone()
            };
            assert_eq!(*manifest.id(), row.manifest_id);
            assert_eq!(manifest.byte_length(), row.bytes);
            manifest
        })
        .collect();
    assert_eq!(
        required.len(),
        4,
        "base, B HISTORY, C CURRENT and nested bytes"
    );
    for file_id in [
        readme_base.clone(),
        FileId::from_bytes(HISTORY),
        FileId::from_bytes(CURRENT),
        FileId::from_bytes(NESTED),
    ] {
        assert!(required.iter().any(|m| m.file_id() == &file_id));
    }
    let image = owner.image_digest().to_owned();
    let mut allocated = allocation::allocate(&mut owner, &planned, &cancel).unwrap();
    let (copying, effect) = owner
        .commit_allocation(&planned, &allocated, &cancel)
        .unwrap();
    assert_eq!(effect, SourceIntentEffect::Updated);
    allocated
        .publish_format(&mut owner, &copying, &cancel)
        .unwrap();
    assert_eq!(
        owner
            .register_allocation(&allocated, &copying, &cancel)
            .unwrap(),
        ImportRegistrationEffect::Inserted
    );
    let mut close = Close::default();
    assert_eq!(
        owner
            .copy_metadata(&allocated, &copying, &mut close, &cancel)
            .unwrap(),
        CopyEffect::Copied
    );
    let staged = owner
        .stage_metadata(&allocated, &copying, &mut close, &cancel)
        .unwrap();
    assert_eq!(staged.source, SourceIntentEffect::Updated);
    assert_eq!(staged.destination, StageEffect::Updated);
    let staging = staged.intent;
    assert_eq!(staging.state, TransitionState::Staging);
    assert_eq!(
        owner
            .reserve_content(&allocated, &staging, &mut close, &cancel)
            .unwrap(),
        ExportReservationEffect::Inserted
    );
    for manifest in &required {
        assert_eq!(
            owner
                .install_content(&allocated, &staging, manifest.id(), &mut close, &cancel)
                .unwrap(),
            *manifest
        );
    }
    let protection = owner
        .protect_content(&allocated, &staging, &mut close, &cancel)
        .unwrap();
    let checkout = owner
        .copy_checkout(&allocated, &staging, &protection, &mut close, &cancel)
        .unwrap();
    let destination = allocated.root().to_owned();
    let destination_database = allocated.destination_database().path().to_owned();
    let files = [CheckoutFile {
        path: nested.clone(),
        manifest_id: current.id().clone(),
        file_id: current.file_id().clone(),
        bytes: current.byte_length(),
        executable: false,
    }];
    let expected =
        ExpectedCheckout::ordinary(&staging, &files, &[readme.clone()], &image, None).unwrap();
    assert_eq!(expected.files(), &files);
    assert_eq!(
        expected.directories(),
        &[
            RepoPath::parse("dir").unwrap(),
            RepoPath::parse("dir/sub").unwrap()
        ]
    );
    assert_eq!(expected.deletions(), &[readme.clone()]);
    assert_eq!(fs::read(destination.join(nested.as_str())).unwrap(), NESTED);
    assert!(!destination.join(readme.as_str()).try_exists().unwrap());
    witness::verify_objects(&family_root, &required);
    let ready = owner
        .finalize_destination_ready(
            &allocated,
            &staging,
            &protection,
            &checkout,
            &mut close,
            &cancel,
        )
        .unwrap();
    assert_eq!(ready.effect(), ReadyEffect::Finalized);
    assert_eq!(ready.ready().role, TransitionRole::Destination);
    assert_eq!(ready.ready().state, TransitionState::Ready);
    witness::read(&destination_database, |c| {
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            8
        );
        assert!(read_admitted_import(c, &allocated.proof().format, None).is_err());
    });
    family(&family_database, &staging, &required, "initializing");
    let family_ready = owner
        .publish_destination_family_ready(
            &allocated,
            &staging,
            &protection,
            &checkout,
            &mut close,
            &cancel,
        )
        .unwrap();
    assert_eq!(family_ready.effect(), ExportPublicationEffect::Updated);
    assert_eq!(family_ready.ready(), ready.ready());
    assert_eq!(family_ready.receipt(), ready.receipt());
    family(&family_database, &staging, &required, "ready");
    let source_ready = owner
        .publish_source_ready(
            &allocated,
            &staging,
            &protection,
            &checkout,
            ready.ready(),
            ready.receipt(),
            &cancel,
        )
        .unwrap();
    assert_eq!(source_ready.effect(), SourceIntentEffect::Updated);
    assert_eq!(source_ready.intent().state, TransitionState::Ready);
    let ready_rows = publication::source(
        &source_database,
        &source_marker,
        source_ready.intent(),
        Some(ready.receipt()),
        &original_ingress,
    );
    let published = owner
        .publish_source_published(
            &allocated,
            &staging,
            &protection,
            &checkout,
            ready.ready(),
            ready.receipt(),
            &cancel,
        )
        .unwrap();
    assert_eq!(published.effect(), SourceIntentEffect::Updated);
    assert_eq!(published.intent().state, TransitionState::Published);
    let published_rows = publication::source(
        &source_database,
        &source_marker,
        published.intent(),
        Some(ready.receipt()),
        &original_ingress,
    );
    publication::only_source_rows_changed(
        &ready_rows,
        &published_rows,
        source_ready.intent(),
        published.intent(),
        ready.receipt(),
    );
    require_source_rows_unchanged(&before_source, &published_rows);
    let family_rows = snapshot(&family_database);
    let held =
        OrdinaryReadyAdmission::from_retained(&allocated, &protection, &checkout, &cancel).unwrap();
    let progress = owner
        .admit_ready_destination(held, &mut close, &cancel)
        .unwrap_or_else(|e| panic!("C10 -> D10 admission {:?}: {}", e.step, e.cause));
    assert_eq!(progress.step(), AdmissionStep::FreshVerified);
    let admitted = witness::read(&destination_database, |c| {
        let data = admitted_ordinary::inspect(c, &allocated.proof().format, None).unwrap();
        assert_eq!(data.private_schema(), 10);
        assert_eq!(data.admission_digest(), progress.admission_digest());
        assert_eq!(data.ready(), ready.ready());
        assert_eq!(data.receipt(), ready.receipt());
        assert_eq!(data.source_published(), published.intent());
        assert_eq!(data.checkout().deletions(), expected.deletions());
        assert_eq!(data.checkout().directories(), expected.directories());
        assert_eq!(data.checkout().files().len(), 1);
        assert_eq!(data.checkout().files()[0].path, nested);
        assert_eq!(data.imported_capture_count(), deleted.sequence);
        assert_eq!(data.ready_capture_sequence(), deleted.sequence + 1);
        data
    });
    drop(progress);
    owner.close().unwrap();
    assert_eq!(snapshot(&source_database), published_rows);
    assert_eq!(snapshot(&family_database), family_rows);
    allocated.require_published(&staging.plan).unwrap();
    drop(checkout);
    drop(protection);
    drop(allocated);
    drop(store);

    // This public D opener acquires its own gates and byte proofs after every
    // original C export, D allocation/checkout/CAS and store holder is gone.
    let reopened = ProjectionStore::open(&destination).unwrap();
    let destination_state = reopened.state().unwrap();
    assert_eq!(destination_state.generation, admitted.ready_generation());
    assert_eq!(destination_state.digest, staging.plan.placement_digest);
    assert_eq!(fs::read(destination.join(nested.as_str())).unwrap(), NESTED);
    assert!(!destination.join(readme.as_str()).try_exists().unwrap());
    let mut inherited_b = false;
    let mut inherited_c = false;
    for (patch, bytes) in &captures {
        assert_eq!(reopened.captured_patch(patch.sequence).unwrap(), *patch);
        assert_eq!(captured_bytes(&reopened, patch), *bytes);
        let origin = witness::read(&destination_database, |c| {
            admitted_ordinary::capture_origin(c, &reopened.marker, patch.sequence, None).unwrap()
        });
        assert!(origin.imported());
        assert_eq!(origin.identity(), &patch.identity);
        if patch.identity == source_marker.identity {
            inherited_c = true;
        } else {
            assert_eq!(patch.identity, b_identity);
            inherited_b = true;
        }
    }
    assert!(
        inherited_b && inherited_c,
        "D must retain both B and C authorship/bytes"
    );
    let local = reopened
        .captured_patch(admitted.ready_capture_sequence())
        .unwrap();
    assert_eq!(local.identity, *reopened.identity());
    assert_eq!(local.changes, deleted.changes);
    assert_eq!(
        local.previous_patch_digest.as_deref(),
        Some(deleted.digest.as_str())
    );
    assert_eq!(
        captured_bytes(&reopened, &local),
        captured_bytes(&reopened, &deleted)
    );
    witness::read(&destination_database, |c| {
        admitted
            .require_same_import(&admitted_ordinary::inspect(c, &reopened.marker, None).unwrap())
            .unwrap();
        let origin =
            admitted_ordinary::capture_origin(c, &reopened.marker, local.sequence, None).unwrap();
        assert!(!origin.imported());
        assert_eq!(origin.identity(), reopened.identity());
    });
    drop(reopened);
    let source = ProjectionStore::open(&root).unwrap();
    assert_eq!(source.state().unwrap(), state);
    assert_eq!(source.latest_captured_patch().unwrap(), Some(deleted));
    for (patch, bytes) in &captures {
        assert_eq!(source.captured_patch(patch.sequence).unwrap(), *patch);
        assert_eq!(captured_bytes(&source, patch), *bytes);
    }
    assert_eq!(fs::read(root.join(nested.as_str())).unwrap(), NESTED);
    assert!(!root.join(readme.as_str()).try_exists().unwrap());
    witness::read(&source_database, |c| {
        assert_eq!(publication::ingress(c, &source_marker), original_ingress);
        admission
            .require_same_import(&admitted_ordinary::inspect(c, &source_marker, None).unwrap())
            .unwrap();
        assert_eq!(witness::database(c), published_rows);
    });
    drop(source);
    witness::verify_objects(&family_root, &required);
}

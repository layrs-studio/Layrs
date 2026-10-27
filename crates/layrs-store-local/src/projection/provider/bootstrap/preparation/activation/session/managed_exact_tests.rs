//! Native exact installation with retained owners; ROOT releases each recipe.
use super::*;
use crate::projection::{
    provider::{
        bootstrap::tests::fixture::Fixture,
        database_io::{self, WriterCloseObserver, WriterOpenPoint},
        process_profile::tests::child,
        records::{
            transition::{ProviderTransitionIntentRecord, TransitionState},
            Record,
        },
    },
    shared::exact_content::{validate_installations, ExactRecord, Phase, PREPARE_CHARGE},
};
use layrs_core::scm::{canonical_json, FileManifest};
use rusqlite::{ffi, params, Connection, OpenFlags};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Default)]
struct Close {
    opens: usize,
    closes: usize,
}
impl WriterCloseObserver for Close {
    fn open_point(&mut self, point: WriterOpenPoint, _: *mut ffi::sqlite3) -> CauseResult<()> {
        if point == WriterOpenPoint::BeforeRaw {
            self.opens += 1;
        }
        Ok(())
    }
    fn before_close(&mut self, _: *mut ffi::sqlite3) -> Option<Cause> {
        None
    }
    fn after_close(&mut self, succeeded: bool) -> Option<Cause> {
        assert!(succeeded, "every native exact writer must close physically");
        self.closes += 1;
        None
    }
}

fn witnesses(paths: &[PathBuf]) -> Vec<(String, u64, String)> {
    paths
        .iter()
        .map(|p| database_io::tests::support::witness(p))
        .collect()
}

// Include retained staging, not just CAS: replay must neither rotate attempts
// nor remove duplicate pieces that remain covered by the fixed reservation.
fn content_witness(root: &Path) -> BTreeMap<PathBuf, Option<(String, u64, String)>> {
    fn visit(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Option<(String, u64, String)>>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            assert!(!entry.file_type().unwrap().is_symlink());
            if entry.file_type().unwrap().is_dir() {
                out.insert(path.strip_prefix(root).unwrap().to_owned(), None);
                visit(root, &path, out);
            } else {
                assert!(entry.file_type().unwrap().is_file());
                out.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    Some(database_io::tests::support::witness(&path)),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    for directory in ["objects", "staging"] {
        visit(root, &root.join(directory), &mut out);
    }
    out
}

fn object_name(id: &str) -> &str {
    id.split_once(':').unwrap().1
}

fn assert_bytes(root: &Path, manifest: &FileManifest, expected: &[u8]) {
    let payload = canonical_json(manifest).unwrap();
    assert_eq!(
        fs::read(
            root.join("objects/manifests")
                .join(object_name(manifest.id().as_str()))
        )
        .unwrap(),
        payload
    );
    let mut actual = Vec::new();
    for part in manifest.chunks() {
        let bytes = fs::read(
            root.join("objects/chunks")
                .join(object_name(part.chunk_id().as_str())),
        )
        .unwrap();
        assert_eq!(bytes.len() as u64, part.byte_length());
        actual.extend_from_slice(&bytes);
    }
    assert_eq!(
        actual, expected,
        "CAS reconstructs the retained native file exactly"
    );
}

fn content_bytes(c: &Connection) -> u64 {
    c.query_row(
        "SELECT coalesce((SELECT sum(bytes) FROM chunks),0)+coalesce((SELECT sum(length(payload)) FROM manifests),0)+coalesce((SELECT sum(bytes) FROM sweep),0)+8192*(SELECT count(*) FROM exact_installations)+coalesce((SELECT sum(length(manifest)) FROM exact_installations),0)",
        [], |r| r.get(0),
    ).unwrap()
}

fn assert_published(
    fixture: &Fixture,
    session: &ProviderActiveSession,
    staging: &ProviderTransitionIntentRecord,
    manifests: &[FileManifest],
    installed: &FileManifest,
    base_content: u64,
    expected_debit: u64,
) {
    let held = &session.source.held;
    let _locks = held
        .family
        .locks(
            &held.artifacts.allocation.format.identity.worktree_id,
            false,
        )
        .unwrap();
    let c = Connection::open_with_flags(
        fixture.family.root().join("metadata/catalog.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    c.execute_batch("BEGIN DEFERRED").unwrap();
    let reservation = staging.reservation_operation().unwrap();
    let (payload, stored, published): (Vec<u8>, Vec<u8>, bool) = c.query_row(
        "SELECT payload,manifest,published FROM exact_installations WHERE reservation=?1 AND manifest_id=?2",
        params![reservation, installed.id().as_str()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    ).unwrap();
    let record: ExactRecord = serde_json::from_slice(&payload).unwrap();
    assert_eq!(canonical_json(&record).unwrap(), payload);
    assert_eq!(record.phase, Phase::Published);
    assert!(published && record.charged_manifest);
    assert_eq!(record.manifest_id, *installed.id());
    assert_eq!(record.file_id, *installed.file_id());
    assert_eq!(stored, canonical_json(installed).unwrap());
    let state = validate_installations(&c, &record.binding, manifests, None).unwrap();
    assert_eq!((state.records, state.published), (1, 1));
    assert_eq!(state.debit_bytes, expected_debit);
    assert_eq!(record.debit_bytes, expected_debit);
    assert_eq!(state.remaining, staging.plan.quota_bytes - expected_debit);
    assert_eq!(state.content_bytes, base_content + expected_debit);
    assert_eq!(content_bytes(&c), state.content_bytes);
    assert_eq!(state.reserved_bytes, state.remaining);
    assert_eq!(
        state.content_bytes + state.reserved_bytes,
        base_content + staging.plan.quota_bytes,
        "publication transfers fixed charge; no reservation capacity is freed early"
    );
    let owner = staging
        .plan
        .source_prefix
        .destination_identity
        .worktree_id
        .as_str();
    assert_eq!(
        c.query_row("SELECT state FROM worktrees WHERE id=?1", [owner], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "initializing"
    );
    for sql in [
        "SELECT EXISTS(SELECT 1 FROM roots WHERE owner=?1)",
        "SELECT EXISTS(SELECT 1 FROM installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1)",
    ] {
        assert!(!c.query_row(sql, [owner], |r| r.get::<_,bool>(0)).unwrap());
    }
    let (file, bytes, indexed): (String, u64, Vec<u8>) = c
        .query_row(
            "SELECT file_id,bytes,payload FROM manifests WHERE id=?1",
            [installed.id().as_str()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(file, installed.file_id().as_str());
    assert_eq!(bytes, installed.byte_length());
    assert_eq!(indexed, stored);
    let indexed_chunks = c
        .prepare("SELECT chunk_id FROM manifest_chunks WHERE manifest_id=?1 ORDER BY ordinal")
        .unwrap()
        .query_map([installed.id().as_str()], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        indexed_chunks,
        installed
            .chunks()
            .iter()
            .map(|p| p.chunk_id().as_str().to_owned())
            .collect::<Vec<_>>()
    );
    c.close().unwrap();
}

fn case(reopen: bool) {
    // Two identical complete chunks plus a distinct tail exercise per-ordinal
    // publication while the immutable object is charged only once.
    let mut replacement = vec![b'm'; 2 * 1024 * 1024];
    replacement.extend_from_slice(b"exact-native-tail");
    super::managed_staging_tests::with_content_planned_bytes(
        &replacement,
        |fixture, session, attempt, planned| {
            let cancel = Cancellation::default();
            let (mut destination, copying) =
                super::managed_allocation_tests::bound_holder(session, attempt, planned);
            destination
                .publish_format(
                    &session.source.root,
                    &session.source.record,
                    &session.source.activation,
                    attempt,
                    &copying,
                    &cancel,
                )
                .unwrap();
            let held = &session.source.held;
            let mut setup = Close::default();
            super::super::managed_destination::register_and_copy(
                held,
                &session.source.root,
                &session.source.record,
                &session.source.activation,
                attempt,
                &copying,
                &destination,
                &mut setup,
                &cancel,
            )
            .unwrap();
            let active = {
                let _locks = held
                    .family
                    .locks(
                        &held.artifacts.allocation.format.identity.worktree_id,
                        false,
                    )
                    .unwrap();
                capture_state::read(held, Some(&session.source.root), Some(&cancel))
                    .unwrap_or_else(|e| panic!("active read: {}", e.cause))
            };
            let fresh = active.active.source.initial().clone();
            let mut checkpoint = session.session.export_checkpoint().unwrap();
            let mut image = super::super::managed_image::plan(
                &mut checkpoint,
                session.source.as_ref(),
                &active.active.source,
                &active.active.activated.snapshot,
                &cancel,
            )
            .unwrap();
            let manifests =
                super::managed_staging_tests::manifests_for(&active.active.source, &image);
            let manifest = image
                .files()
                .iter()
                .find(|f| f.manifest.manifest().byte_length() == replacement.len() as u64)
                .unwrap()
                .manifest
                .manifest()
                .clone();
            assert_eq!(manifest.chunks().len(), 3);
            assert_eq!(
                manifest.chunks()[0].chunk_id(),
                manifest.chunks()[1].chunk_id()
            );
            assert_ne!(
                manifest.chunks()[1].chunk_id(),
                manifest.chunks()[2].chunk_id()
            );
            let mut staging =
                ProviderTransitionIntentRecord::decode(&copying.encode().unwrap()).unwrap();
            staging.state = TransitionState::Staging;
            staging.reservation = Some(staging.reservation_operation().unwrap());
            super::super::managed_staging::stage(
                held,
                &session.source.root,
                &session.source.record,
                &session.source.activation,
                attempt,
                &copying,
                &staging,
                &destination,
                &mut image,
                &manifests,
                &mut setup,
                &cancel,
            )
            .unwrap();
            assert_eq!((setup.opens, setup.closes), (4, 4));
            let private_paths = [
                fixture.metadata.join("projection.sqlite3"),
                destination.destination_database().path().to_owned(),
            ];
            let private_before = witnesses(&private_paths);
            let family_path = fixture.family.root().join("metadata/catalog.sqlite3");
            let (base_content, new_chunks) = {
                let _locks = held
                    .family
                    .locks(
                        &held.artifacts.allocation.format.identity.worktree_id,
                        false,
                    )
                    .unwrap();
                let c = Connection::open_with_flags(&family_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                    .unwrap();
                let base = content_bytes(&c);
                assert_eq!(
                    c.query_row("SELECT count(*) FROM reservations", [], |r| r
                        .get::<_, u64>(0))
                        .unwrap(),
                    0
                );
                assert!(!c
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM manifests WHERE id=?1)",
                        [manifest.id().as_str()],
                        |r| r.get::<_, bool>(0)
                    )
                    .unwrap());
                let unique: BTreeMap<_, _> = manifest
                    .chunks()
                    .iter()
                    .map(|p| (p.chunk_id().as_str(), p.byte_length()))
                    .collect();
                let new = unique
                    .into_iter()
                    .filter_map(|(id, len)| {
                        let indexed = c
                            .query_row(
                                "SELECT EXISTS(SELECT 1 FROM chunks WHERE id=?1)",
                                [id],
                                |r| r.get::<_, bool>(0),
                            )
                            .unwrap();
                        (!indexed).then_some(len)
                    })
                    .sum::<u64>();
                c.close().unwrap();
                (base, new)
            };
            assert_eq!(
                new_chunks,
                (replacement.len() - 1024 * 1024) as u64,
                "both distinct fixture chunks start absent; repeated ordinal is not charged twice"
            );
            let expected_debit =
                PREPARE_CHARGE + 2 * canonical_json(&manifest).unwrap().len() as u64 + new_chunks;
            let mut lease = super::super::managed_content::ManagedImportLease::open(
                held,
                &session.source.root,
                &session.source.record,
                &session.source.activation,
                attempt,
                &destination,
                &mut image,
                &staging,
                &manifests,
                &fresh,
                &cancel,
            )
            .unwrap();
            let mut close = Close::default();
            lease.reserve_plan(&mut close, &cancel).unwrap();
            assert_eq!((close.opens, close.closes), (1, 1));
            assert_eq!(
                lease
                    .install_exact(manifest.id(), &mut close, &cancel)
                    .unwrap(),
                manifest
            );
            assert!(close.opens > 1);
            assert_eq!(close.opens, close.closes);
            drop(lease);
            assert_eq!(witnesses(&private_paths), private_before);
            assert_published(
                fixture,
                session,
                &staging,
                &manifests,
                &manifest,
                base_content,
                expected_debit,
            );
            assert_bytes(fixture.family.root(), &manifest, &replacement);
            if reopen {
                let durable_family = database_io::tests::support::witness(&family_path);
                let durable_content = content_witness(fixture.family.root());
                let mut replay = super::super::managed_content::ManagedImportLease::open(
                    held,
                    &session.source.root,
                    &session.source.record,
                    &session.source.activation,
                    attempt,
                    &destination,
                    &mut image,
                    &staging,
                    &manifests,
                    &fresh,
                    &cancel,
                )
                .unwrap_or_else(|e| panic!("Published reopen refused: {e}"));
                let mut replay_close = Close::default();
                assert_eq!(
                    replay
                        .install_exact(manifest.id(), &mut replay_close, &cancel)
                        .unwrap(),
                    manifest
                );
                assert_eq!((replay_close.opens, replay_close.closes), (0, 0));
                drop(replay);
                assert_eq!(
                    database_io::tests::support::witness(&family_path),
                    durable_family
                );
                assert_eq!(content_witness(fixture.family.root()), durable_content);
                assert_eq!(witnesses(&private_paths), private_before);
                assert_published(
                    fixture,
                    session,
                    &staging,
                    &manifests,
                    &manifest,
                    base_content,
                    expected_debit,
                );
            }
            drop(image);
            drop(checkpoint);
        },
    );
}

macro_rules! owned {
    ($name:ident, $reopen:expr) => {
        #[test]
        #[ignore = "UNQUALIFIED until ROOT releases this exact owned native installation recipe"]
        fn $name() {
            child::run(
                concat!(module_path!(), "::", stringify!($name)),
                240,
                || {
                    child::declare();
                    case($reopen);
                },
            );
        }
    };
}
owned!(
    managed_exact_multichunk_charges_unique_objects_and_metadata,
    false
);
owned!(
    managed_exact_published_reopen_replays_without_writer_or_byte_changes,
    true
);

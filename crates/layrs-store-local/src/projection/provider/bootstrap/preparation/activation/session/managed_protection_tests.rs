//! Integrated protection with retained native owners. Acknowledgement refusal
//! below is deliberate DATA after successful close, never real SQLITE_BUSY.
use super::*;
use crate::projection::{
    provider::{
        bootstrap::tests::fixture::Fixture,
        database_io::{self, WriterCloseObserver, WriterOpenPoint},
        process_profile::tests::child,
        records::{
            Record,
            transition::{ProviderTransitionIntentRecord, TransitionState},
        },
    },
    shared::{
        exact_content::{
            ExactInstallState, ExactRecord, PREPARE_CHARGE, Phase, validate_installations,
            validate_protected_installations,
        },
        exact_protection::{ExactProtection, ProtectionPresence},
    },
};
use layrs_core::scm::{FileManifest, canonical_json};
use rusqlite::{Connection, OpenFlags, ffi};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Default)]
struct Close {
    opens: usize,
    closes: usize,
    refuse_acknowledgement: bool,
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
        assert!(succeeded, "managed protection writer must close physically");
        self.closes += 1;
        self.refuse_acknowledgement
            .then(|| invalid("managed protection close acknowledgement withheld"))
    }
}

fn witnesses(paths: &[PathBuf]) -> Vec<(String, u64, String)> {
    paths
        .iter()
        .map(|p| database_io::tests::support::witness(p))
        .collect()
}
fn content_witness(root: &Path) -> BTreeMap<PathBuf, Option<(String, u64, String)>> {
    fn visit(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Option<(String, u64, String)>>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            assert!(!kind.is_symlink());
            if kind.is_dir() {
                out.insert(path.strip_prefix(root).unwrap().to_owned(), None);
                visit(root, &path, out);
            } else {
                assert!(kind.is_file());
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

type JournalRows = Vec<(String, String, u64, Vec<u8>, Vec<u8>, bool)>;

fn read_family(
    fixture: &Fixture,
    session: &ProviderActiveSession,
    staging: &ProviderTransitionIntentRecord,
    required: &[FileManifest],
    expected: &ExactProtection,
    protected: bool,
) -> (ExactInstallState, JournalRows) {
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
    let operation = staging.reservation_operation().unwrap();
    let mut query = c.prepare("SELECT manifest_id,file_id,bytes,payload,manifest,published FROM exact_installations WHERE reservation=?1 ORDER BY manifest_id").unwrap();
    let journals = query
        .query_map([operation], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, u64>(2)?,
                r.get::<_, Vec<u8>>(3)?,
                r.get::<_, Vec<u8>>(4)?,
                r.get::<_, bool>(5)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<JournalRows>>()
        .unwrap();
    drop(query);
    assert_eq!(journals.len(), 2);
    let mut binding = None;
    for (id, file, bytes, payload, manifest, published) in &journals {
        let record: ExactRecord = serde_json::from_slice(payload).unwrap();
        assert_eq!(canonical_json(&record).unwrap(), *payload);
        assert_eq!(record.phase, Phase::Published);
        assert!(*published);
        let expected = required.iter().find(|m| m.id().as_str() == id).unwrap();
        assert_eq!(file, expected.file_id().as_str());
        assert_eq!(*bytes, expected.byte_length());
        assert_eq!(*manifest, canonical_json(expected).unwrap());
        if let Some(previous) = &binding {
            assert_eq!(previous, &record.binding);
        }
        binding = Some(record.binding);
    }
    let binding = binding.unwrap();
    let state = if protected {
        assert_eq!(
            expected.inspect(&c, None).unwrap(),
            ProtectionPresence::Present
        );
        assert!(
            validate_installations(&c, &binding, required, None).is_err(),
            "installation admission remains root-absent strict"
        );
        validate_protected_installations(&c, &binding, required, expected, None).unwrap()
    } else {
        assert_eq!(
            expected.inspect(&c, None).unwrap(),
            ProtectionPresence::Absent
        );
        validate_installations(&c, &binding, required, None).unwrap()
    };
    assert_eq!((state.records, state.published), (2, 2));
    assert_eq!(
        state.remaining,
        staging.plan.quota_bytes - state.debit_bytes
    );
    assert_eq!(
        c.query_row(
            "SELECT state FROM worktrees WHERE id=?1",
            [held
                .artifacts
                .allocation
                .format
                .identity
                .worktree_id
                .as_str()],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "ready"
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
    assert!(!c.query_row("SELECT EXISTS(SELECT 1 FROM installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1)",[owner],|r| r.get::<_,bool>(0)).unwrap());
    let root_count: usize = c
        .query_row("SELECT count(*) FROM roots WHERE owner=?1", [owner], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(root_count, usize::from(protected));
    if protected {
        let (root, digest, operation, payload): (String, String, String, Vec<u8>) = c
            .query_row(
                "SELECT id,digest,operation,payload FROM roots WHERE owner=?1",
                [owner],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(root, expected.id());
        assert_eq!(digest, expected.digest());
        assert_eq!(
            operation,
            format!("provider-import:{}", staging.transaction_id)
        );
        assert_eq!(payload, expected.payload());
        let members = c
            .prepare("SELECT manifest_id FROM root_manifests WHERE root_id=?1 ORDER BY manifest_id")
            .unwrap()
            .query_map([root], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        let mut ids: Vec<_> = required
            .iter()
            .map(|m| m.id().as_str().to_owned())
            .collect();
        ids.sort_unstable();
        assert_eq!(members, ids);
    }
    c.close().unwrap();
    (state, journals)
}

fn case(refuse_acknowledgement: bool) {
    let replacement = b"provider base content";
    super::managed_staging_tests::with_content_planned_bytes(
        replacement,
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
            let required =
                super::managed_staging_tests::manifests_for(&active.active.source, &image);
            assert_eq!(
                required.len(),
                2,
                "source Single and native fixed representation both retained"
            );
            let current = image
                .files()
                .iter()
                .find(|f| f.path.as_str() == "README.md")
                .unwrap()
                .manifest
                .manifest()
                .clone();
            let historical = required.iter().find(|m| m.id() != current.id()).unwrap();
            assert_eq!(historical.file_id(), current.file_id());
            assert_ne!(historical.id(), current.id());
            assert_eq!(historical.chunks().len(), 1);
            assert_eq!(current.chunks().len(), 1);
            assert_eq!(
                historical.chunks()[0].chunk_id(),
                current.chunks()[0].chunk_id()
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
                &required,
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
            let expected = ExactProtection::new(
                &held.artifacts.allocation.family.incarnation,
                &staging.plan.source_prefix.destination_identity.worktree_id,
                &staging
                    .destination
                    .as_ref()
                    .unwrap()
                    .private_database_identity,
                &staging.transaction_id,
                &required,
                Some(&cancel),
            )
            .unwrap();
            let mut lease = super::super::managed_content::ManagedImportLease::open(
                held,
                &session.source.root,
                &session.source.record,
                &session.source.activation,
                attempt,
                &destination,
                &mut image,
                &staging,
                &required,
                &fresh,
                &cancel,
            )
            .unwrap();
            let mut install_close = Close::default();
            lease.reserve_plan(&mut install_close, &cancel).unwrap();
            assert_eq!(
                lease
                    .install_exact(current.id(), &mut install_close, &cancel)
                    .unwrap(),
                current
            );
            // Only the retained current image authorizes production. The historical
            // representation must use the already verified indexed shared chunk.
            for manifest in &required {
                assert_eq!(
                    lease
                        .install_exact(manifest.id(), &mut install_close, &cancel)
                        .unwrap(),
                    *manifest
                );
            }
            assert_eq!(install_close.opens, install_close.closes);
            let before = read_family(fixture, session, &staging, &required, &expected, false);
            let expected_debit = 2 * PREPARE_CHARGE
                + 2 * required
                    .iter()
                    .map(|m| canonical_json(m).unwrap().len() as u64)
                    .sum::<u64>()
                + replacement.len() as u64;
            assert_eq!(
                before.0.debit_bytes, expected_debit,
                "shared immutable chunk is debited by one exact representation only"
            );
            assert_eq!(
                fs::read(
                    fixture.family.root().join("objects/chunks").join(
                        current.chunks()[0]
                            .chunk_id()
                            .as_str()
                            .split_once(':')
                            .unwrap()
                            .1
                    )
                )
                .unwrap(),
                replacement
            );
            for manifest in &required {
                assert_eq!(
                    fs::read(
                        fixture
                            .family
                            .root()
                            .join("objects/manifests")
                            .join(manifest.id().as_str().split_once(':').unwrap().1)
                    )
                    .unwrap(),
                    canonical_json(manifest).unwrap()
                );
            }
            let content_before = content_witness(fixture.family.root());
            let mut protection_close = Close {
                refuse_acknowledgement,
                ..Close::default()
            };
            let outcome = lease.protect_exact(&mut protection_close, &cancel);
            if refuse_acknowledgement {
                let error = outcome
                    .err()
                    .expect("physical close succeeds but acknowledgement must be withheld");
                assert!(
                    error
                        .to_string()
                        .contains("managed protection close acknowledgement withheld"),
                    "{error}"
                );
            } else {
                let protected = outcome.unwrap();
                protected.require(&lease, &cancel).unwrap();
                assert_eq!(protected.root(), &expected);
                drop(protected);
            }
            assert_eq!((protection_close.opens, protection_close.closes), (1, 1));
            drop(lease);
            assert_eq!(
                read_family(fixture, session, &staging, &required, &expected, true),
                before,
                "root publication cannot debit, remove or rewrite installation state"
            );
            assert_eq!(witnesses(&private_paths), private_before);
            assert_eq!(content_witness(fixture.family.root()), content_before);
            let durable_family = database_io::tests::support::witness(&family_path);
            let mut replay = super::super::managed_content::ManagedImportLease::open(
                held,
                &session.source.root,
                &session.source.record,
                &session.source.activation,
                attempt,
                &destination,
                &mut image,
                &staging,
                &required,
                &fresh,
                &cancel,
            )
            .unwrap_or_else(|e| panic!("protected reopen refused: {e}"));
            let mut replay_close = Close::default();
            let protected = replay.protect_exact(&mut replay_close, &cancel).unwrap();
            protected.require(&replay, &cancel).unwrap();
            assert_eq!(protected.root(), &expected);
            assert_eq!((replay_close.opens, replay_close.closes), (0, 0));
            drop(protected);
            drop(replay);
            assert_eq!(
                database_io::tests::support::witness(&family_path),
                durable_family
            );
            assert_eq!(witnesses(&private_paths), private_before);
            assert_eq!(content_witness(fixture.family.root()), content_before);
            assert_eq!(
                read_family(fixture, session, &staging, &required, &expected, true),
                before
            );
            for path in &private_paths {
                let c =
                    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
                assert_eq!(
                    c.query_row(
                        "SELECT state FROM projection_provider_transitions WHERE id=?1",
                        [staging.transaction_id.as_str()],
                        |r| r.get::<_, String>(0)
                    )
                    .unwrap(),
                    "staging"
                );
                c.close().unwrap();
            }
            drop(image);
            drop(checkpoint);
        },
    );
}

macro_rules! owned {
    ($name:ident,$refuse:expr) => {
        #[test]
        #[ignore = "UNQUALIFIED until ROOT releases this exact owned native protection recipe"]
        fn $name() {
            child::run(
                concat!(module_path!(), "::", stringify!($name)),
                240,
                || {
                    child::declare();
                    case($refuse);
                },
            );
        }
    };
}
owned!(
    managed_protection_preserves_two_representations_and_reopens_without_writer,
    false
);
owned!(
    managed_protection_committed_ack_refusal_replays_without_writer,
    true
);

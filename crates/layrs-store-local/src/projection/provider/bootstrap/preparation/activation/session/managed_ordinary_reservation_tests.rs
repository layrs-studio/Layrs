//! Genuine same-owner ReserveOnly replay after withholding acknowledgement of
//! a physically successful catalog writer close. This is no OS-close-failure,
//! owner-loss recovery, content-transfer or autonomous C admission recipe.
use crate::{
    projection::{
        format,
        provider::{
            admitted_ordinary,
            database_io::{self, WriterCloseObserver, WriterOpenPoint},
            managed_admission::read_admitted_import,
            managed_export::allocation,
            ordinary_destination::{staging::StageEffect, CopyEffect},
            process_profile::tests::child,
            records::{
                transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
                Record,
            },
            recovery::error::{invalid, ProviderBootstrapCause as Cause, Result as CauseResult},
            transition_sql::SourceIntentEffect,
        },
        shared::{
            admitted_catalog::{AdmittedFamilyContext, ExportReservationEffect},
            provider_access::ImportRegistrationEffect,
        },
        ProjectionStore,
    },
    secure_fs::PinnedDirectory,
};
use layrs_core::scm::WorktreeId;
use layrs_vfs::Cancellation;
use rusqlite::{ffi, types::ValueRef, Connection, OpenFlags};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

const LEAF: &str = "000000000000000000000000000000d1";
const TX: &str = "000000000000000000000000000000d2";
const WITHHELD_ACK: &str = "family reservation successful close acknowledgement withheld";

#[derive(Default)]
struct Close {
    before_raw: usize,
    opened: usize,
    successful_closes: usize,
    withhold_ack: bool,
    withheld: usize,
}
impl Close {
    fn counts(&self) -> (usize, usize, usize, usize) {
        (
            self.before_raw,
            self.opened,
            self.successful_closes,
            self.withheld,
        )
    }
}
impl WriterCloseObserver for Close {
    fn open_point(&mut self, point: WriterOpenPoint, raw: *mut ffi::sqlite3) -> CauseResult<()> {
        match point {
            WriterOpenPoint::BeforeRaw => {
                assert!(raw.is_null());
                self.before_raw += 1;
            }
            WriterOpenPoint::AfterRaw => {
                assert!(!raw.is_null());
                self.opened += 1;
            }
            _ => {}
        }
        Ok(())
    }
    fn before_close(&mut self, _: *mut ffi::sqlite3) -> Option<Cause> {
        None
    }
    fn after_close(&mut self, succeeded: bool) -> Option<Cause> {
        assert!(
            succeeded,
            "observer requires a physically successful writer close"
        );
        self.successful_closes += 1;
        if std::mem::take(&mut self.withhold_ack) {
            self.withheld += 1;
            Some(invalid(WITHHELD_ACK))
        } else {
            None
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Node {
    Directory(String),
    File((String, u64, String)),
    LockedOwner(String, u64),
}
type Tree = BTreeMap<PathBuf, Node>;

fn tree(root: &Path, locks: [&Path; 2]) -> Tree {
    assert_ne!(locks[0], locks[1]);
    let mut pending = vec![root.to_owned()];
    let mut result = BTreeMap::new();
    let mut bytes = 0u64;
    while let Some(path) = pending.pop() {
        assert!(
            result.len() < 128,
            "tiny reservation fixture inventory bound"
        );
        let metadata = fs::symlink_metadata(&path).unwrap();
        assert!(!metadata.file_type().is_symlink());
        if metadata.is_dir() {
            // Read/traverse access is compatible with retained immutable image
            // handles; this does not request DELETE or alter their sharing.
            let pin = PinnedDirectory::open(&path).unwrap();
            let mut children = Vec::new();
            for child in fs::read_dir(&path).unwrap() {
                assert!(children.len() < 64);
                children.push(child.unwrap().path());
            }
            pin.verify_path_identity().unwrap();
            result.insert(path, Node::Directory(pin.identity_token()));
            pending.extend(children);
        } else {
            assert!(metadata.is_file());
            assert!(metadata.len() <= 2 * 1024 * 1024);
            bytes = bytes.checked_add(metadata.len()).unwrap();
            assert!(bytes <= 16 * 1024 * 1024);
            if locks.contains(&path.as_path()) {
                // Exactly B/C owner coordination files have LockFileEx byte
                // locks. Preserve their identity and length, never mask a DB.
                let pin = crate::projection::file::DatabasePin::open(&path).unwrap();
                let length = pin.file.metadata().unwrap().len();
                pin.verify(&path).unwrap();
                result.insert(path, Node::LockedOwner(pin.identity().to_owned(), length));
            } else {
                result.insert(
                    path.clone(),
                    Node::File(database_io::tests::support::witness(&path)),
                );
            }
        }
    }
    result
}
fn trees(root: &Path, family: &Path, destination: &Path, locks: [&Path; 2]) -> (Tree, Tree, Tree) {
    let result = (
        tree(root, locks),
        tree(family, locks),
        tree(destination, locks),
    );
    for lock in locks {
        assert!(matches!(result.1.get(lock), Some(Node::LockedOwner(..))));
    }
    result
}
fn only_catalog_changed(before: &(Tree, Tree, Tree), after: &(Tree, Tree, Tree), catalog: &Path) {
    assert_eq!(
        before.0, after.0,
        "reservation preserves source checkout bytes"
    );
    assert_eq!(
        before.2, after.2,
        "reservation preserves destination checkout bytes"
    );
    assert_eq!(
        before.1.keys().collect::<Vec<_>>(),
        after.1.keys().collect::<Vec<_>>()
    );
    for (path, previous) in &before.1 {
        let current = &after.1[path];
        if path == catalog {
            let (Node::File((old_id, _, old_hash)), Node::File((new_id, _, new_hash))) =
                (previous, current)
            else {
                panic!("family catalog must remain a regular file");
            };
            assert_eq!(old_id, new_id);
            assert_ne!(old_hash, new_hash, "full reservation must be durable");
        } else {
            assert_eq!(
                previous, current,
                "reservation changed unrelated path {path:?}"
            );
        }
    }
    assert!(before.1.contains_key(catalog));
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Cell {
    Null,
    Integer(i64),
    Real(u64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}
type Rows = Vec<Vec<Cell>>;
type Database = BTreeMap<String, Rows>;

fn read<T>(path: &Path, inspect: impl FnOnce(&Connection) -> T) -> T {
    // Independent read-only SQL observations, without another owner lock.
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    c.execute_batch("BEGIN DEFERRED").unwrap();
    let result = inspect(&c);
    c.execute_batch("ROLLBACK").unwrap();
    c.close().unwrap();
    result
}
fn database(c: &Connection) -> Database {
    let tables = {
        let mut q = c
            .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name LIMIT 65")
            .unwrap();
        let result = q
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(result.len() <= 64);
        result
    };
    let mut result = BTreeMap::new();
    let mut bytes = 0usize;
    for table in tables {
        assert!(table
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_'));
        let mut q = c
            .prepare(&format!("SELECT * FROM \"{table}\" LIMIT 129"))
            .unwrap();
        let columns = q.column_count();
        assert!(columns <= 32);
        let mut query = q.query([]).unwrap();
        let mut rows = Vec::new();
        while let Some(row) = query.next().unwrap() {
            assert!(rows.len() < 128);
            let mut cells = Vec::new();
            for column in 0..columns {
                cells.push(match row.get_ref(column).unwrap() {
                    ValueRef::Null => Cell::Null,
                    ValueRef::Integer(v) => Cell::Integer(v),
                    ValueRef::Real(v) => Cell::Real(v.to_bits()),
                    ValueRef::Text(v) => {
                        bytes += v.len();
                        Cell::Text(v.to_vec())
                    }
                    ValueRef::Blob(v) => {
                        bytes += v.len();
                        Cell::Blob(v.to_vec())
                    }
                });
                assert!(bytes <= 16 * 1024 * 1024);
            }
            rows.push(cells);
        }
        rows.sort();
        result.insert(table, rows);
    }
    result
}
fn source(
    path: &Path,
    marker: &format::Marker,
    expected: Option<&ProviderTransitionIntentRecord>,
) -> Database {
    read(path, |c| {
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            9
        );
        let admitted = read_admitted_import(c, marker, None).unwrap();
        let outgoing = admitted.outgoing();
        if let Some(expected) = expected {
            assert_eq!(outgoing.len(), 1);
            assert_eq!(&outgoing[0], expected);
        } else {
            assert!(outgoing.is_empty());
        }
        database(c)
    })
}
fn destination(path: &Path, expected: &ProviderTransitionIntentRecord) -> Database {
    read(path, |c| {
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            8
        );
        assert!(!c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='projection_managed_admission')", [], |r| r.get::<_, bool>(0)).unwrap());
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM projection_provider_transitions",
                [],
                |r| r.get::<_, u64>(0)
            )
            .unwrap(),
            1
        );
        let actual: Vec<u8> = c
            .query_row(
                "SELECT payload FROM projection_provider_transitions",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut incoming =
            ProviderTransitionIntentRecord::decode(&expected.encode().unwrap()).unwrap();
        incoming.role = TransitionRole::Destination;
        incoming.local_generation = 0;
        assert_eq!(actual, incoming.encode().unwrap());
        assert_eq!(
            c.query_row(
                "SELECT identity FROM projection_meta WHERE singleton=1",
                [],
                |r| r.get::<_, Vec<u8>>(0)
            )
            .unwrap(),
            expected
                .destination
                .as_ref()
                .unwrap()
                .format
                .bytes()
                .unwrap()
        );
        database(c)
    })
}

#[derive(Debug)]
struct Catalog {
    rows: Database,
    base: u64,
    journals: u64,
    reserved: u64,
    limit: u64,
}
fn catalog(path: &Path, staging: &ProviderTransitionIntentRecord, reserved: bool) -> Catalog {
    read(path, |c| {
        let proof = staging.destination.as_ref().unwrap();
        let owner = proof.format.identity.worktree_id.as_str();
        let actual: (String, String, Vec<u8>, String) = c
            .query_row(
                "SELECT metadata_path,database_identity,marker,state FROM worktrees WHERE id=?1",
                [owner],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            actual,
            (
                proof
                    .format
                    .shared
                    .as_ref()
                    .unwrap()
                    .metadata_path
                    .to_str()
                    .unwrap()
                    .to_owned(),
                proof.private_database_identity.clone(),
                proof.format.bytes().unwrap(),
                "initializing".into()
            )
        );
        for sql in [
            "SELECT EXISTS(SELECT 1 FROM roots WHERE owner=?1)",
            "SELECT EXISTS(SELECT 1 FROM installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1)",
            "SELECT EXISTS(SELECT 1 FROM exact_installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1)",
        ] { assert!(!c.query_row(sql, [owner], |r| r.get::<_, bool>(0)).unwrap()); }
        let count: u64 = c
            .query_row(
                "SELECT count(*) FROM reservations WHERE owner=?1",
                [owner],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, u64::from(reserved));
        if reserved {
            let actual: (String, String, u64) = c
                .query_row(
                    "SELECT id,operation,remaining FROM reservations WHERE owner=?1",
                    [owner],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            let operation = staging.reservation_operation().unwrap();
            assert_eq!(
                actual,
                (operation.clone(), operation, staging.plan.quota_bytes)
            );
        }
        let base = c.query_row("SELECT coalesce((SELECT sum(bytes) FROM chunks),0)+coalesce((SELECT sum(length(payload)) FROM manifests),0)+coalesce((SELECT sum(bytes) FROM sweep),0)", [], |r|r.get(0)).unwrap();
        let journals: u64 = c
            .query_row(
                "SELECT count(*)*8192+coalesce(sum(length(manifest)),0) FROM exact_installations",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            journals > 0,
            "genuine retained Published journal charge is present"
        );
        let reserved: u64 = c
            .query_row(
                "SELECT coalesce(sum(remaining),0) FROM reservations",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let limit = c
            .query_row("SELECT bytes FROM quota WHERE singleton=1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(base + journals + reserved <= limit);
        Catalog {
            rows: database(c),
            base,
            journals,
            reserved,
            limit,
        }
    })
}
fn ordinary_catalog_gate(
    source_path: &Path,
    marker: &format::Marker,
    family: &Path,
    allowed: bool,
) {
    let context = read(source_path, |c| {
        AdmittedFamilyContext::new(&admitted_ordinary::inspect(c, marker, None).unwrap()).unwrap()
    });
    let family_marker = fs::read(family.join("FORMAT")).unwrap();
    read(&family.join("metadata/catalog.sqlite3"), |c| {
        assert_eq!(
            context.validate_catalog(c, &family_marker, None).is_ok(),
            allowed,
            "ordinary retained-family gate must refuse the active C reservation"
        );
    });
}

#[test]
#[ignore = "ROOT source release and audited owned native child1500/outer1560 required"]
fn managed_ordinary_reservation_same_owner_replays_withheld_family_close_ack() {
    // r35's genuine fixture plus allocation/copy/Staging and four B owner
    // cycles measured 694.993s. This recipe retains one owner and adds two
    // complete ReserveOnly validations/CAS checks: child1500/outer1560.
    child::run(
        concat!(
            module_path!(),
            "::managed_ordinary_reservation_same_owner_replays_withheld_family_close_ack"
        ),
        1500,
        || {
            child::declare();
            let root = super::managed_restore_recovery_tests::admitted_destination();
            let store = ProjectionStore::open(&root).unwrap();
            let state = store.state().unwrap();
            let family = store.marker.shared.as_ref().unwrap().root.clone();
            let source_database = store.marker.metadata_path(&root).join("projection.sqlite3");
            let original = source(&source_database, &store.marker, None);
            let source_lock = source_database.parent().unwrap().join("layrs.lock");
            let mut identity = store.identity().clone();
            identity.worktree_id =
                WorktreeId::parse("worktree:ordinary-reservation-destination").unwrap();
            let cancel = Cancellation::default();
            let mut owner = store
                .begin_managed_export(state.generation, identity, &cancel)
                .unwrap();
            let (planned, effect) = owner
                .begin_plan(root.parent().unwrap(), LEAF, TX, &cancel)
                .unwrap();
            assert_eq!(effect, SourceIntentEffect::Inserted);
            let mut allocated = allocation::allocate(&mut owner, &planned, &cancel)
                .unwrap_or_else(|error| panic!("ordinary reservation allocation: {error:?}"));
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
            let mut setup = Close::default();
            assert_eq!(
                owner
                    .copy_metadata(&allocated, &copying, &mut setup, &cancel)
                    .unwrap(),
                CopyEffect::Copied
            );
            let staged = owner
                .stage_metadata(&allocated, &copying, &mut setup, &cancel)
                .unwrap();
            assert_eq!(staged.source, SourceIntentEffect::Updated);
            assert_eq!(staged.destination, StageEffect::Updated);
            assert_eq!(setup.counts(), (2, 2, 2, 0));
            let staging = staged.intent;
            assert_eq!(staging.state, TransitionState::Staging);
            assert!(staging.plan.quota_bytes > 0);
            let destination_root = allocated.root().to_owned();
            let destination_database = allocated.destination_database().path().to_owned();
            let destination_lock = destination_database.parent().unwrap().join("layrs.lock");
            let locks = [source_lock.as_path(), destination_lock.as_path()];
            let family_database = family.join("metadata/catalog.sqlite3");
            let before_source = source(&source_database, &store.marker, Some(&staging));
            for table in [
                "projection_managed_admission",
                "projection_restore_history",
                "projection_captures",
            ] {
                assert_eq!(
                    before_source[table], original[table],
                    "setup preserves incoming admission/history"
                );
            }
            let before_destination = destination(&destination_database, &staging);
            let before_catalog = catalog(&family_database, &staging, false);
            ordinary_catalog_gate(&source_database, &store.marker, &family, true);
            let before = trees(&root, &family, &destination_root, locks);
            let mut close = Close::default();
            let cancelled = Cancellation::default();
            cancelled.cancel();
            assert!(owner
                .reserve_content(&allocated, &staging, &mut close, &cancelled)
                .is_err());
            let mut forged =
                ProviderTransitionIntentRecord::decode(&staging.encode().unwrap()).unwrap();
            forged.destination.as_mut().unwrap().format_file_identity = "0:0:0".into();
            forged.encode().unwrap();
            assert!(owner
                .reserve_content(&allocated, &forged, &mut close, &cancel)
                .is_err());
            assert_eq!(close.counts(), (0, 0, 0, 0));
            assert_eq!(trees(&root, &family, &destination_root, locks), before);

            close.withhold_ack = true;
            let error = owner
                .reserve_content(&allocated, &staging, &mut close, &cancel)
                .unwrap_err();
            assert!(error.to_string().contains(WITHHELD_ACK), "{error}");
            assert_eq!(close.counts(), (1, 1, 1, 1));
            assert!(!close.withhold_ack);
            let committed = trees(&root, &family, &destination_root, locks);
            only_catalog_changed(&before, &committed, &family_database);
            assert_eq!(
                source(&source_database, &store.marker, Some(&staging)),
                before_source
            );
            assert_eq!(
                destination(&destination_database, &staging),
                before_destination
            );
            let after_catalog = catalog(&family_database, &staging, true);
            assert_eq!(after_catalog.base, before_catalog.base);
            assert_eq!(after_catalog.journals, before_catalog.journals);
            assert_eq!(after_catalog.limit, before_catalog.limit);
            assert_eq!(
                after_catalog.reserved,
                before_catalog.reserved + staging.plan.quota_bytes
            );
            for (table, rows) in &before_catalog.rows {
                if table != "reservations" {
                    assert_eq!(
                        &after_catalog.rows[table], rows,
                        "reservation changed retained {table}"
                    );
                } else {
                    assert_eq!(after_catalog.rows[table].len(), rows.len() + 1);
                    for row in rows {
                        assert!(
                            after_catalog.rows[table].contains(row),
                            "retained reservation/debit changed"
                        );
                    }
                }
            }
            ordinary_catalog_gate(&source_database, &store.marker, &family, false);

            // Retain this exact owner and independent C holder across the error.
            // Ordinary lease manifest/stream lookups both reject active C; the
            // explicit export callback must still revalidate and replay read-only.
            assert_eq!(
                owner
                    .reserve_content(&allocated, &staging, &mut close, &cancel)
                    .unwrap(),
                ExportReservationEffect::AlreadyPresent
            );
            assert_eq!(
                close.counts(),
                (1, 1, 1, 1),
                "exact replay opens no new family writer"
            );
            assert_eq!(trees(&root, &family, &destination_root, locks), committed);
            assert_eq!(
                source(&source_database, &store.marker, Some(&staging)),
                before_source
            );
            assert_eq!(
                destination(&destination_database, &staging),
                before_destination
            );
            assert_eq!(
                catalog(&family_database, &staging, true).rows,
                after_catalog.rows
            );
            owner.close().unwrap();
            // The refusal now cannot be explained by the retained B owner lock.
            assert!(
                ProjectionStore::open(&root).is_err(),
                "ordinary B open must refuse its live export/active C"
            );
            ordinary_catalog_gate(&source_database, &store.marker, &family, false);
            assert_eq!(trees(&root, &family, &destination_root, locks), committed);
            allocated.require_published(&staging.plan).unwrap();
            drop(allocated);
            drop(store);
        },
    );
}

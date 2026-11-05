//! Genuine same-owner Installing replay after withholding acknowledgement of
//! a physically successful catalog writer close. No OS close failure,
//! owner-loss recovery, crash or autonomous C admission is qualified here.
use crate::{
    projection::{
        ProjectionStore, format,
        provider::{
            database_io::{self, WriterCloseObserver, WriterOpenPoint},
            managed_admission::read_admitted_import,
            managed_export::allocation,
            ordinary_destination::{CopyEffect, staging::StageEffect},
            process_profile::tests::child,
            records::{
                Record,
                transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
            },
            recovery::error::{ProviderBootstrapCause as Cause, Result as CauseResult, invalid},
            transition_sql::SourceIntentEffect,
        },
        shared::{
            admitted_catalog::ExportReservationEffect, provider_access::ImportRegistrationEffect,
        },
    },
    secure_fs::PinnedDirectory,
};
use layrs_core::scm::{Chunk, FileManifest, RepoPath, TreeEntryTarget, WorktreeId, canonical_json};
use layrs_vfs::Cancellation;
use rusqlite::{Connection, OpenFlags, ffi, types::ValueRef};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

const LEAF: &str = "000000000000000000000000000000e1";
const TX: &str = "000000000000000000000000000000e2";
const WITHHELD_ACK: &str = "Installing Declared successful close acknowledgement withheld";

#[derive(Default)]
struct Close {
    before_raw: usize,
    opened: usize,
    successful_closes: usize,
    withhold_ack: bool,
    withheld: usize,
    retained_chunk: Option<PathBuf>,
    pin_checks: usize,
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
                if let Some(path) = &self.retained_chunk {
                    let error = OpenOptions::new()
                        .write(true)
                        .share_mode(7)
                        .open(path)
                        .unwrap_err();
                    assert_eq!(
                        error.raw_os_error(),
                        Some(32),
                        "actual retained CAS pin must forbid write sharing"
                    );
                    self.pin_checks += 1;
                }
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
pub(super) enum Node {
    Directory(String),
    File((String, u64, String)),
    LockedOwner(String, u64),
}
pub(super) type Tree = BTreeMap<PathBuf, Node>;

fn tree(root: &Path, locks: [&Path; 2]) -> Tree {
    assert_ne!(locks[0], locks[1]);
    let mut pending = vec![root.to_owned()];
    let mut result = BTreeMap::new();
    let mut bytes = 0u64;
    while let Some(path) = pending.pop() {
        assert!(
            result.len() < 192,
            "tiny three-manifest Installing fixture inventory bound"
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
pub(super) fn trees(
    root: &Path,
    family: &Path,
    destination: &Path,
    locks: [&Path; 2],
) -> (Tree, Tree, Tree) {
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
pub(super) enum Cell {
    Null,
    Integer(i64),
    Real(u64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}
pub(super) type Rows = Vec<Vec<Cell>>;
pub(super) type Database = BTreeMap<String, Rows>;

pub(super) fn read<T>(path: &Path, inspect: impl FnOnce(&Connection) -> T) -> T {
    // Independent read-only SQL observations, without another owner lock.
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    c.execute_batch("BEGIN DEFERRED").unwrap();
    let result = inspect(&c);
    c.execute_batch("ROLLBACK").unwrap();
    c.close().unwrap();
    result
}
pub(super) fn database(c: &Connection) -> Database {
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
        assert!(
            table
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        );
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
pub(super) fn source(
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
pub(super) fn destination(path: &Path, expected: &ProviderTransitionIntentRecord) -> Database {
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

const HISTORY: &[u8] = b"Installing historical-only B capture";
const CURRENT: &[u8] = b"Installing new current bytes absent from retained CAS";

#[derive(Debug)]
struct Catalog {
    rows: Database,
    records: BTreeMap<String, serde_json::Value>,
    remaining: u64,
    content: u64,
    reserved: u64,
}
fn catalog(path: &Path, stage: &ProviderTransitionIntentRecord) -> Catalog {
    read(path, |c| {
        let proof = stage.destination.as_ref().unwrap();
        let owner = proof.format.identity.worktree_id.as_str();
        let registration: (String, String, Vec<u8>, String) = c
            .query_row(
                "SELECT metadata_path,database_identity,marker,state FROM worktrees WHERE id=?1",
                [owner],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            registration,
            (
                proof
                    .format
                    .shared
                    .as_ref()
                    .unwrap()
                    .metadata_path
                    .to_str()
                    .unwrap()
                    .into(),
                proof.private_database_identity.clone(),
                proof.format.bytes().unwrap(),
                "initializing".into()
            )
        );
        for sql in [
            "SELECT EXISTS(SELECT 1 FROM roots WHERE owner=?1)",
            "SELECT EXISTS(SELECT 1 FROM installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1)",
        ] {
            assert!(!c.query_row(sql, [owner], |r| r.get::<_, bool>(0)).unwrap());
        }
        let operation = stage.reservation_operation().unwrap();
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM reservations WHERE owner=?1",
                [owner],
                |r| r.get::<_, u64>(0)
            )
            .unwrap(),
            1
        );
        let reservation: (String, String, u64) = c
            .query_row(
                "SELECT owner,operation,remaining FROM reservations WHERE id=?1",
                [&operation],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(reservation.0, owner);
        assert_eq!(reservation.1, operation);
        let records = {
            let mut q=c.prepare("SELECT manifest_id,payload,file_id,bytes,manifest,published FROM exact_installations WHERE reservation=?1 ORDER BY manifest_id LIMIT 9").unwrap();
            let mut query = q.query([&operation]).unwrap();
            let mut records = BTreeMap::new();
            while let Some(row) = query.next().unwrap() {
                assert!(records.len() < 8);
                let id: String = row.get(0).unwrap();
                let payload: Vec<u8> = row.get(1).unwrap();
                assert!(payload.len() <= 8192);
                let record: serde_json::Value = serde_json::from_slice(&payload).unwrap();
                assert_eq!(canonical_json(&record).unwrap(), payload);
                assert_eq!(record["manifestId"], id);
                assert_eq!(record["fileId"], row.get::<_, String>(2).unwrap());
                assert_eq!(record["bytes"], row.get::<_, u64>(3).unwrap());
                let phase = record["phase"].as_str().unwrap();
                assert_eq!(row.get::<_, bool>(5).unwrap(), phase == "published");
                let stored: Option<Vec<u8>> = row.get(4).unwrap();
                if matches!(phase, "verified" | "publishing" | "published") {
                    let stored = stored.unwrap();
                    let manifest: FileManifest = serde_json::from_slice(&stored).unwrap();
                    assert_eq!(canonical_json(&manifest).unwrap(), stored);
                    assert_eq!(manifest.id().as_str(), id);
                    assert_eq!(record["fileId"], manifest.file_id().as_str());
                    assert_eq!(record["bytes"], manifest.byte_length());
                } else {
                    assert!(stored.is_none());
                }
                assert_eq!(record["binding"]["owner"], owner);
                assert_eq!(record["binding"]["quotaBytes"], stage.plan.quota_bytes);
                assert_eq!(
                    record["binding"]["planDigest"],
                    stage.plan.digest().unwrap()
                );
                assert_eq!(
                    record["binding"]["databaseIdentity"],
                    proof.private_database_identity
                );
                assert_eq!(record["binding"]["operation"], operation);
                records.insert(id, record);
            }
            records
        };
        let debit: u64 = records
            .values()
            .map(|r| r["debitBytes"].as_u64().unwrap())
            .sum();
        assert_eq!(reservation.2 + debit, stage.plan.quota_bytes);
        let content:u64=c.query_row("SELECT coalesce((SELECT sum(bytes) FROM chunks),0)+coalesce((SELECT sum(length(payload)) FROM manifests),0)+coalesce((SELECT sum(bytes) FROM sweep),0)+(SELECT count(*)*8192+coalesce(sum(length(manifest)),0) FROM exact_installations)",[],|r|r.get(0)).unwrap();
        let reserved: u64 = c
            .query_row(
                "SELECT coalesce(sum(remaining),0) FROM reservations",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let limit: u64 = c
            .query_row("SELECT bytes FROM quota WHERE singleton=1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(content + reserved <= limit);
        Catalog {
            rows: database(c),
            records,
            remaining: reservation.2,
            content,
            reserved,
        }
    })
}

pub(super) fn object(family: &Path, kind: &str, id: &str) -> PathBuf {
    let hex = id.strip_prefix("blake3:").unwrap();
    assert_eq!(hex.len(), 64);
    assert!(hex.bytes().all(|b| b.is_ascii_hexdigit()));
    family.join("objects").join(kind).join(hex)
}
pub(super) fn verify_objects(family: &Path, manifests: &[FileManifest]) {
    for manifest in manifests {
        assert_eq!(
            fs::read(object(family, "manifests", manifest.id().as_str())).unwrap(),
            canonical_json(manifest).unwrap()
        );
        for chunk in manifest.chunks() {
            let bytes = fs::read(object(family, "chunks", chunk.chunk_id().as_str())).unwrap();
            assert_eq!(bytes.len() as u64, chunk.byte_length());
            assert_eq!(Chunk::new(bytes).unwrap().id(), chunk.chunk_id());
        }
    }
}
fn installation_tree_changes(
    before: &(Tree, Tree, Tree),
    after: &(Tree, Tree, Tree),
    family: &Path,
    required: &[FileManifest],
    records: &BTreeMap<String, serde_json::Value>,
) {
    assert_eq!(before.0, after.0);
    assert_eq!(before.2, after.2);
    let catalog = family.join("metadata/catalog.sqlite3");
    let mut allowed_files = BTreeSet::new();
    let mut allowed_directories = BTreeSet::new();
    for manifest in required {
        allowed_files.insert(object(family, "manifests", manifest.id().as_str()));
        for chunk in manifest.chunks() {
            allowed_files.insert(object(family, "chunks", chunk.chunk_id().as_str()));
        }
        let record = &records[manifest.id().as_str()];
        let nonce = record["directory"].as_str().unwrap();
        assert!(
            !nonce.is_empty()
                && nonce.len() <= 128
                && nonce.bytes().all(|b| b.is_ascii_digit() || b == b'-')
        );
        let directory = family.join("staging").join(nonce);
        assert!(
            allowed_directories.insert(directory.clone()),
            "independent exact attempts have distinct directories"
        );
        let Some(Node::Directory(identity)) = after.1.get(&directory) else {
            panic!("published journal must retain bound staging directory");
        };
        assert_eq!(record["directoryIdentity"], identity.as_str());
        allowed_files.insert(directory.join("manifest"));
        if directory.join("manifest").exists() {
            assert_eq!(
                fs::read(directory.join("manifest")).unwrap(),
                canonical_json(manifest).unwrap()
            );
        }
        for n in 0..manifest.chunks().len() {
            let path = directory.join(format!("chunk-{n}"));
            if path.exists() {
                let bytes = fs::read(&path).unwrap();
                assert_eq!(bytes.len() as u64, manifest.chunks()[n].byte_length());
                assert_eq!(
                    Chunk::new(bytes).unwrap().id(),
                    manifest.chunks()[n].chunk_id()
                );
            }
            allowed_files.insert(path);
        }
    }
    for (path, node) in &before.1 {
        let next = after
            .1
            .get(path)
            .expect("installation removed an existing path");
        if path == &catalog {
            let (Node::File((old_id, _, old_hash)), Node::File((new_id, _, new_hash))) =
                (node, next)
            else {
                panic!("catalog must remain file");
            };
            assert_eq!(old_id, new_id);
            assert_ne!(old_hash, new_hash);
        } else {
            assert_eq!(node, next, "installation changed preexisting {path:?}");
        }
    }
    for (path, node) in &after.1 {
        if !before.1.contains_key(path) {
            match node {
                Node::Directory(_) => assert!(
                    allowed_directories.contains(path),
                    "unexpected directory {path:?}"
                ),
                Node::File(_) => assert!(allowed_files.contains(path), "unexpected file {path:?}"),
                Node::LockedOwner(..) => panic!("unexpected owner coordination file"),
            }
        }
    }
}
fn retained_rows_unchanged(before: &Database, after: &Database, operation: &str) {
    let operation = Cell::Text(operation.as_bytes().to_vec());
    for table in ["reservations", "exact_installations"] {
        assert_eq!(
            before[table]
                .iter()
                .filter(|r| r[0] != operation)
                .collect::<Vec<_>>(),
            after[table]
                .iter()
                .filter(|r| r[0] != operation)
                .collect::<Vec<_>>(),
            "retained B journal/debit changed"
        );
    }
}
fn installation_sql_changes(
    before: &Database,
    after: &Database,
    required: &[FileManifest],
    operation: &str,
) {
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>()
    );
    retained_rows_unchanged(before, after, operation);
    let mut allowed: BTreeMap<&str, Rows> = BTreeMap::from([
        ("files", Vec::new()),
        ("manifests", Vec::new()),
        ("chunks", Vec::new()),
        ("manifest_chunks", Vec::new()),
    ]);
    for m in required {
        allowed.get_mut("files").unwrap().push(vec![
            Cell::Text(m.file_id().as_str().as_bytes().to_vec()),
            Cell::Integer(m.byte_length().try_into().unwrap()),
        ]);
        allowed.get_mut("manifests").unwrap().push(vec![
            Cell::Text(m.id().as_str().as_bytes().to_vec()),
            Cell::Text(m.file_id().as_str().as_bytes().to_vec()),
            Cell::Integer(m.byte_length().try_into().unwrap()),
            Cell::Blob(canonical_json(m).unwrap()),
        ]);
        for (n, chunk) in m.chunks().iter().enumerate() {
            allowed.get_mut("chunks").unwrap().push(vec![
                Cell::Text(chunk.chunk_id().as_str().as_bytes().to_vec()),
                Cell::Integer(chunk.byte_length().try_into().unwrap()),
            ]);
            allowed.get_mut("manifest_chunks").unwrap().push(vec![
                Cell::Text(m.id().as_str().as_bytes().to_vec()),
                Cell::Integer(n.try_into().unwrap()),
                Cell::Text(chunk.chunk_id().as_str().as_bytes().to_vec()),
            ]);
        }
    }
    for (table, previous) in before {
        let next = &after[table];
        if matches!(table.as_str(), "reservations" | "exact_installations") {
            continue;
        } else if let Some(expected) = allowed.get(table.as_str()) {
            for row in previous {
                assert!(next.contains(row), "preexisting immutable index changed");
            }
            for row in next {
                assert!(
                    previous.contains(row) || expected.contains(row),
                    "unexpected immutable index insertion"
                );
            }
            for row in expected {
                assert!(
                    next.contains(row),
                    "required representation not fully indexed"
                );
            }
        } else {
            assert_eq!(previous, next, "unexpected catalog mutation {table}");
        }
    }
}

#[test]
#[ignore = "ROOT source release and audited owned native child1800/outer1860 required"]
fn managed_ordinary_installing_replays_declared_close_ack_and_publishes_complete_union() {
    // One genuine fixture, one capture and one retained B owner. r35 measured
    // 694.993s; exact engine cost is not yet measured. Proposed child1800/outer1860.
    child::run(
        concat!(
            module_path!(),
            "::managed_ordinary_installing_replays_declared_close_ack_and_publishes_complete_union"
        ),
        1800,
        || {
            child::declare();
            let root = super::managed_restore_recovery_tests::admitted_destination();
            let store = ProjectionStore::open(&root).unwrap();
            let state = store.state().unwrap();
            let path = RepoPath::parse("README.md").unwrap();
            let base_id = match state
                .snapshot
                .entries
                .iter()
                .find(|e| e.path == path)
                .unwrap()
                .target
                .as_ref()
                .unwrap()
            {
                TreeEntryTarget::File { file_id } => file_id.clone(),
                _ => panic!("fixture requires regular README"),
            };
            fs::write(root.join(path.as_str()), HISTORY).unwrap();
            let historical_capture = store.capture_patch(state.generation).unwrap();
            assert!(!historical_capture.changes.is_empty());
            assert!(historical_capture.changes.iter().any(|change| {
                change.replacement.as_ref().is_some_and(|file| {
                    file.file_id == layrs_core::scm::FileId::from_bytes(HISTORY)
                })
            }));
            drop(store);
            let store = ProjectionStore::open(&root).unwrap();
            assert_eq!(store.state().unwrap(), state);
            fs::write(root.join(path.as_str()), CURRENT).unwrap();
            assert_eq!(fs::read(root.join(path.as_str())).unwrap(), CURRENT);
            let family = store.marker.shared.as_ref().unwrap().root.clone();
            let source_database = store.marker.metadata_path(&root).join("projection.sqlite3");
            let original = source(&source_database, &store.marker, None);
            let source_lock = source_database.parent().unwrap().join("layrs.lock");
            let mut identity = store.identity().clone();
            identity.worktree_id =
                WorktreeId::parse("worktree:ordinary-installing-destination").unwrap();
            let cancel = Cancellation::default();
            let mut owner = store
                .begin_managed_export(state.generation, identity, &cancel)
                .unwrap();
            let current = owner.current_manifest(&path).unwrap().clone();
            let history = owner
                .retained_manifest(&layrs_core::scm::FileId::from_bytes(HISTORY))
                .unwrap()
                .clone();
            let base = owner.retained_manifest(&base_id).unwrap().clone();
            assert!(owner.retained_manifest(current.file_id()).is_none());
            assert_ne!(history.id(), current.id());
            assert_ne!(base.id(), history.id());
            assert_eq!(current.chunks().len(), 1);
            let (planned, effect) = owner
                .begin_plan(root.parent().unwrap(), LEAF, TX, &cancel)
                .unwrap();
            assert_eq!(effect, SourceIntentEffect::Inserted);
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
                    let m = if row.manifest_id == *current.id() {
                        current.clone()
                    } else {
                        owner.retained_manifest(&row.file_id).unwrap().clone()
                    };
                    assert_eq!(*m.id(), row.manifest_id);
                    assert_eq!(m.byte_length(), row.bytes);
                    m
                })
                .collect();
            assert_eq!(
                required.len(),
                3,
                "base, historical-only and new current must all be funded"
            );
            assert_eq!(
                required
                    .iter()
                    .map(FileManifest::id)
                    .collect::<BTreeSet<_>>(),
                [base.id(), history.id(), current.id()]
                    .into_iter()
                    .collect::<BTreeSet<_>>()
            );
            let mut allocated = allocation::allocate(&mut owner, &planned, &cancel)
                .unwrap_or_else(|e| panic!("Installing allocation: {e:?}"));
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
            let outcome = owner
                .stage_metadata(&allocated, &copying, &mut setup, &cancel)
                .unwrap();
            assert_eq!(outcome.source, SourceIntentEffect::Updated);
            assert_eq!(outcome.destination, StageEffect::Updated);
            let staging = outcome.intent;
            assert_eq!(staging.state, TransitionState::Staging);
            assert_eq!(
                owner
                    .reserve_content(&allocated, &staging, &mut setup, &cancel)
                    .unwrap(),
                ExportReservationEffect::Inserted
            );
            assert_eq!(setup.counts(), (3, 3, 3, 0));
            let destination_root = allocated.root().to_owned();
            let destination_database = allocated.destination_database().path().to_owned();
            let destination_lock = destination_database.parent().unwrap().join("layrs.lock");
            let locks = [source_lock.as_path(), destination_lock.as_path()];
            let family_database = family.join("metadata/catalog.sqlite3");
            let private_b = source(&source_database, &store.marker, Some(&staging));
            for table in [
                "projection_managed_admission",
                "projection_captures",
                "projection_restore_history",
            ] {
                assert_eq!(private_b[table], original[table]);
            }
            let private_c = destination(&destination_database, &staging);
            let before = catalog(&family_database, &staging);
            assert!(before.records.is_empty());
            assert_eq!(before.remaining, staging.plan.quota_bytes);
            assert!(
                !before.rows["manifests"]
                    .iter()
                    .any(|r| r[0] == Cell::Text(current.id().as_str().as_bytes().to_vec()))
            );
            assert!(!object(&family, "chunks", current.chunks()[0].chunk_id().as_str()).exists());
            let initial_trees = trees(&root, &family, &destination_root, locks);
            let mut close = Close {
                retained_chunk: Some(object(
                    &family,
                    "chunks",
                    base.chunks()[0].chunk_id().as_str(),
                )),
                ..Close::default()
            };
            let cancelled = Cancellation::default();
            cancelled.cancel();
            assert!(
                owner
                    .install_content(&allocated, &staging, current.id(), &mut close, &cancelled)
                    .is_err()
            );
            let foreign = FileManifest::from_chunks(
                layrs_core::scm::ChunkingDescriptor::single(),
                &[Chunk::new(b"foreign Installing ID".to_vec()).unwrap()],
            )
            .unwrap();
            assert!(
                owner
                    .install_content(&allocated, &staging, foreign.id(), &mut close, &cancel)
                    .is_err()
            );
            assert_eq!(close.counts(), (0, 0, 0, 0));
            assert_eq!(close.pin_checks, 0);
            assert_eq!(
                trees(&root, &family, &destination_root, locks),
                initial_trees
            );
            assert_eq!(catalog(&family_database, &staging).rows, before.rows);
            close.withhold_ack = true;
            let error = owner
                .install_content(&allocated, &staging, current.id(), &mut close, &cancel)
                .unwrap_err();
            assert!(error.to_string().contains(WITHHELD_ACK), "{error}");
            assert_eq!(close.counts(), (1, 1, 1, 1));
            assert_eq!(close.pin_checks, 1);
            let declared = catalog(&family_database, &staging);
            retained_rows_unchanged(
                &before.rows,
                &declared.rows,
                &staging.reservation_operation().unwrap(),
            );
            assert_eq!(declared.records.len(), 1);
            let record = &declared.records[current.id().as_str()];
            assert_eq!(record["phase"], "declared");
            assert_eq!(record["debitBytes"], 8192);
            assert_eq!(record["directoryIdentity"], serde_json::Value::Null);
            assert_eq!(declared.remaining, staging.plan.quota_bytes - 8192);
            assert!(
                !family
                    .join("staging")
                    .join(record["directory"].as_str().unwrap())
                    .exists()
            );
            only_catalog_changed(
                &initial_trees,
                &trees(&root, &family, &destination_root, locks),
                &family_database,
            );
            assert_eq!(
                source(&source_database, &store.marker, Some(&staging)),
                private_b
            );
            assert_eq!(destination(&destination_database, &staging), private_c);
            for (table, rows) in &before.rows {
                if !matches!(table.as_str(), "reservations" | "exact_installations") {
                    assert_eq!(&declared.rows[table], rows);
                }
            }

            // The exact same B owner retries the durable Declared row, then handles
            // every remaining member. No ordinary lease/reopen is used after C reserve.
            assert_eq!(
                owner
                    .install_content(&allocated, &staging, current.id(), &mut close, &cancel)
                    .unwrap(),
                current
            );
            for manifest in &required {
                if manifest.id() != current.id() {
                    assert_eq!(
                        owner
                            .install_content(
                                &allocated,
                                &staging,
                                manifest.id(),
                                &mut close,
                                &cancel
                            )
                            .unwrap(),
                        *manifest
                    );
                }
            }
            assert!(close.pin_checks > 1);
            assert_eq!(close.before_raw, close.opened);
            assert_eq!(close.opened, close.successful_closes);
            assert_eq!(close.withheld, 1);
            let published = catalog(&family_database, &staging);
            assert_eq!(published.records.len(), required.len());
            let mut expected_debit = 0u64;
            for manifest in &required {
                let record = &published.records[manifest.id().as_str()];
                assert_eq!(record["phase"], "published");
                assert_eq!(record["active"], serde_json::Value::Null);
                assert_eq!(record["spoolCursor"], manifest.chunks().len());
                assert_eq!(record["publishCursor"], manifest.chunks().len());
                let new = manifest.id() == current.id();
                assert_eq!(record["chargedManifest"], new);
                assert_eq!(
                    record["chargedChunks"],
                    if new {
                        format!("1{}", "0".repeat(255))
                    } else {
                        "0".repeat(256)
                    }
                );
                let expected = 8192
                    + canonical_json(manifest).unwrap().len() as u64
                    + if new {
                        canonical_json(manifest).unwrap().len() as u64 + manifest.byte_length()
                    } else {
                        0
                    };
                assert_eq!(record["debitBytes"], expected);
                expected_debit += expected;
            }
            assert_eq!(
                published.remaining + expected_debit,
                staging.plan.quota_bytes
            );
            assert_eq!(published.content - before.content, expected_debit);
            assert_eq!(before.reserved - published.reserved, expected_debit);
            installation_sql_changes(
                &before.rows,
                &published.rows,
                &required,
                &staging.reservation_operation().unwrap(),
            );
            verify_objects(&family, &required);
            assert_eq!(
                fs::read(object(
                    &family,
                    "chunks",
                    current.chunks()[0].chunk_id().as_str()
                ))
                .unwrap(),
                CURRENT
            );
            let complete = trees(&root, &family, &destination_root, locks);
            installation_tree_changes(
                &initial_trees,
                &complete,
                &family,
                &required,
                &published.records,
            );
            assert_eq!(
                source(&source_database, &store.marker, Some(&staging)),
                private_b
            );
            assert_eq!(destination(&destination_database, &staging), private_c);
            let counts = close.counts();
            let pin_checks = close.pin_checks;
            assert_eq!(
                owner
                    .install_content(&allocated, &staging, current.id(), &mut close, &cancel)
                    .unwrap(),
                current
            );
            assert_eq!(
                close.counts(),
                counts,
                "completed manifest replay must open no writer"
            );
            assert_eq!(close.pin_checks, pin_checks);
            assert_eq!(trees(&root, &family, &destination_root, locks), complete);
            assert_eq!(catalog(&family_database, &staging).rows, published.rows);
            assert_eq!(
                source(&source_database, &store.marker, Some(&staging)),
                private_b
            );
            assert_eq!(destination(&destination_database, &staging), private_c);
            owner.close().unwrap();
            assert!(ProjectionStore::open(&root).is_err());
            allocated.require_published(&staging.plan).unwrap();
            drop(allocated);
            assert!(
                ProjectionStore::open(&destination_root).is_err(),
                "C8 Initializing remains unready and unadmitted"
            );
            assert_eq!(trees(&root, &family, &destination_root, locks), complete);
            drop(store);
        },
    );
}

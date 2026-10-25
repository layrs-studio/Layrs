//! Inputs come from frozen pure literals; all final bootstrap state is made by
//! the production writer. No direct final receipt/phase fixture construction.
use super::super::super::{
    database_io::tests::support, validation::tests::fixture::Fixture as Pure,
};
use super::*;
use crate::projection::{PartialStoreIdentity, SharedQuota};
use std::{
    fs,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};
pub(super) const TX: &str = "00000000000000000000000000000001";
pub(super) struct Fixture {
    pub outer: PathBuf,
    pub family: PartialStoreFamily,
    pub checkout: PathBuf,
    pub stages: PathBuf,
    pub metadata: PathBuf,
    pub worktree: WorktreeId,
}
fn budget(database: &Path) {
    let units = database.as_os_str().encode_wide().count();
    for suffix in [0usize, 8, 4, 4, 12] {
        assert!(
            units
                .checked_add(suffix)
                .and_then(|n| n.checked_add(1))
                .is_some_and(|n| n <= 260),
            "UNQUALIFIED named SQLite fixture budget: {database:?}, UTF16={units}, suffix={suffix}, NUL=1"
        );
    }
}
impl Fixture {
    pub fn new() -> Self {
        Self::with_family_name("f")
    }
    pub fn with_family_name(name: &str) -> Self {
        assert!(matches!(name, "f" | "family#one"));
        let outer = support::owned_root("w");
        let checkout = outer.join("c");
        let stages = outer.join("s");
        let family_path = outer.join(name);
        for p in [&checkout, &stages, &family_path] {
            fs::create_dir(p).unwrap();
        }
        let input = Pure::new("G1", false);
        let id = input.allocation.format.identity;
        let metadata = family_path
            .join("worktrees")
            .join(
                blake3::hash(id.worktree_id.as_str().as_bytes())
                    .to_hex()
                    .as_str(),
            )
            .join("metadata");
        for p in [
            family_path.join("metadata/catalog.sqlite3"),
            metadata.join("projection.sqlite3"),
            outer.join("external.sqlite3"),
        ] {
            budget(&p);
        }
        let family = PartialStoreFamily::create(
            &family_path,
            PartialStoreIdentity {
                store_id: id.store_id,
                space_id: id.space_id,
                origin: id.origin,
                principal_id: id.principal_id,
            },
            SharedQuota {
                bytes: 64 * 1024 * 1024,
            },
        )
        .unwrap();
        Self {
            outer,
            family,
            checkout,
            stages,
            metadata,
            worktree: id.worktree_id,
        }
    }
    pub fn from_owned(outer: PathBuf) -> Self {
        assert!(outer.is_absolute());
        let owner = crate::layout::open_read_no_follow(&outer.join("fixture-owner.txt")).unwrap();
        assert!(owner.metadata().unwrap().len() <= 1024);
        let input = Pure::new("G1", false);
        let worktree = input.allocation.format.identity.worktree_id;
        let family = PartialStoreFamily::open(&outer.join("f")).unwrap(); // fresh parent-owned setup, never production discovery
        let metadata = family
            .root()
            .join("worktrees")
            .join(blake3::hash(worktree.as_str().as_bytes()).to_hex().as_str())
            .join("metadata");
        for p in [
            family.root().join("metadata/catalog.sqlite3"),
            metadata.join("projection.sqlite3"),
        ] {
            budget(&p);
        }
        Self {
            checkout: outer.join("c"),
            stages: outer.join("s"),
            outer,
            family,
            metadata,
            worktree,
        }
    }
    pub fn source() -> ProviderBootstrapSource {
        Pure::new("G1", false).source
    }
    pub fn begin(&self) -> Result<ProviderControlPublicationOutcome> {
        self.family.begin_provider_bootstrap(
            &self.checkout,
            &self.stages,
            &self.worktree,
            TX,
            Self::source(),
        )
    }
    pub fn published(&self) -> ProviderControlPublished {
        match self
            .begin()
            .unwrap_or_else(|e| panic!("writer begin failed: {e:?}"))
        {
            ProviderControlPublicationOutcome::Published(p) => p,
            _ => panic!("writer did not return a Published capability"),
        }
    }
    pub fn inspect(&self) -> ProviderBootstrapInspection {
        PartialStoreFamily::inspect_provider_bootstrap(self.family.root(), &self.worktree, TX)
            .unwrap()
    }
    pub fn recovery(&self) -> ProviderBootstrapRecovery {
        let i = self.inspect();
        PartialStoreFamily::open_provider_bootstrap_recovery(
            self.family.root(),
            &self.worktree,
            TX,
            &i.binding,
        )
        .unwrap()
    }
    pub fn receipt(&self, kind: ProviderBootstrapReceipt) -> PathBuf {
        self.metadata.join("provider-control").join(kind.names().1)
    }
    pub fn witness(&self) -> Vec<(String, u64, String)> {
        let mut paths = vec![
            self.family.root().join("FORMAT"),
            self.family.root().join("metadata/catalog.sqlite3"),
        ];
        for p in [
            self.metadata.join("projection.sqlite3"),
            self.receipt(ProviderBootstrapReceipt::Allocation),
            self.receipt(ProviderBootstrapReceipt::Prepared),
            self.receipt(ProviderBootstrapReceipt::Abandoned),
            self.checkout.join(".layrs/FORMAT"),
        ] {
            match fs::symlink_metadata(&p) {
                Ok(_) => paths.push(p),
                Err(e) if e.raw_os_error() == Some(2) || e.raw_os_error() == Some(3) => (),
                Err(e) => panic!("witness unavailable {p:?}: {e}"),
            }
        }
        let allocation_path = self.receipt(ProviderBootstrapReceipt::Allocation);
        match fs::symlink_metadata(&allocation_path) {
            Ok(_) => {
                let bytes = fs::read(&allocation_path).unwrap();
                assert!(bytes.len() <= AllocationRecord::MAX_BYTES);
                let allocation = AllocationRecord::decode(&bytes).unwrap();
                let stage = allocation.stage_identity().unwrap();
                assert!(
                    stage.stage().starts_with(&self.outer),
                    "test witness restricted to its actual owned fixture"
                );
                let path = stage.stage().join("FORMAT");
                match fs::symlink_metadata(&path) {
                    Ok(_) => paths.push(path),
                    Err(e) if matches!(e.raw_os_error(), Some(2 | 3)) => (),
                    Err(e) => panic!("stage witness unavailable: {e}"),
                }
            }
            Err(e) if matches!(e.raw_os_error(), Some(2 | 3)) => (),
            Err(e) => panic!("allocation witness unavailable: {e}"),
        }
        for database in [
            self.family.root().join("metadata/catalog.sqlite3"),
            self.metadata.join("projection.sqlite3"),
            self.outer.join("external.sqlite3"),
        ] {
            for suffix in ["-journal", "-wal", "-shm"] {
                let mut name = database.as_os_str().to_owned();
                name.push(suffix);
                let p = PathBuf::from(name);
                match fs::symlink_metadata(&p) {
                    Ok(_) => paths.push(p),
                    Err(e) if matches!(e.raw_os_error(), Some(2 | 3)) => (),
                    Err(e) => panic!("fixed witness unavailable {p:?}: {e}"),
                }
            }
        }
        paths.iter().map(|p| support::witness(p)).collect()
    }
}
pub(super) fn failure<T>(result: Result<T>) -> ProviderBootstrapFailure {
    match result {
        Err(ProjectionStoreError::ProviderBootstrap(f)) => *f,
        Err(e) => panic!("untyped writer failure: {e:?}"),
        Ok(_) => panic!("expected exact writer failure"),
    }
}
pub(super) fn has(
    f: &ProviderBootstrapFailure,
    step: ProviderBootstrapWriteStep,
    state: ProviderBootstrapWriteState,
) -> bool {
    f.write_report
        .as_ref()
        .unwrap()
        .effects
        .iter()
        .any(|e| e.step == step && e.state == state)
}
pub(super) fn native_source(cause: &Cause) -> Option<i32> {
    use std::error::Error;
    let mut next: Option<&(dyn Error + 'static)> = Some(cause);
    while let Some(e) = next {
        if let Some(e) = e.downcast_ref::<std::io::Error>() {
            if e.raw_os_error().is_some() {
                return e.raw_os_error();
            }
        }
        next = e.source();
    }
    None
}
pub(super) fn assert_synthetic(cause: &Cause, expected: &str) {
    use std::error::Error;
    let mut next: Option<&(dyn Error + 'static)> = Some(cause);
    while let Some(e) = next {
        if let Some(e) = e.downcast_ref::<std::io::Error>() {
            if e.to_string() == expected {
                return;
            }
        }
        next = e.source();
    }
    panic!("missing exact synthetic cause {expected}: {cause:?}");
}

impl Fixture {
    /// Cold fixture setup only: keep the family FORMAT and SQL marker consistent.
    /// This never opens a fixture after a hot-journal mutation.
    pub fn with_incarnation(mut self, length: usize) -> Self {
        let path = self.family.root().to_owned();
        let marker_path = path.join("FORMAT");
        let mut marker: serde_json::Value =
            serde_json::from_slice(&fs::read(&marker_path).unwrap()).unwrap();
        marker["incarnation"] = "i".repeat(length).into();
        let bytes = layrs_core::scm::canonical_json(&marker).unwrap();
        {
            use std::io::Write;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&marker_path)
                .unwrap();
            file.write_all(&bytes).unwrap();
            file.sync_all().unwrap();
        }
        let c = rusqlite::Connection::open_with_flags(
            path.join("metadata/catalog.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap();
        assert_eq!(
            c.execute("UPDATE family SET marker=?1 WHERE singleton=1", [&bytes])
                .unwrap(),
            1
        );
        c.close().unwrap();
        self.family = PartialStoreFamily::open(&path).unwrap();
        self
    }
    pub fn assert_no_begin_allocation(&self) {
        assert!(!self.metadata.parent().unwrap().try_exists().unwrap());
        assert!(!self.checkout.join(".layrs").try_exists().unwrap());
        assert_eq!(fs::read_dir(&self.stages).unwrap().count(), 0);
    }
}

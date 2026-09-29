//! Journaled receive/reauthorization. Revoked bytes are quarantined, not erased.
mod files;
mod history;
mod plan;
mod recovery;
use super::*;
use crate::secure_fs::PinnedDirectory;
pub(super) use history::verify_history;
use layrs_core::{
    projection::{EntryState, ProjectionEntry},
    scm::{FileId, TreeEntryTarget},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
const MAX_FILE: u64 = 1024 * 1024 * 1024;
const MAX_TOTAL: u64 = 4 * MAX_FILE;
static SERIAL: AtomicU64 = AtomicU64::new(0);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshPoint {
    IntentDurable,
    Staged,
    JournalDurable,
    PathQuarantined,
    PathInstalled,
    BeforeCommit,
    AfterCommit,
}
pub trait RefreshFaults {
    fn check(&self, point: RefreshPoint) -> io::Result<()>;
}
pub struct NoRefreshFaults;
impl RefreshFaults for NoRefreshFaults {
    fn check(&self, _: RefreshPoint) -> io::Result<()> {
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    path: String,
    file_id: FileId,
    executable: bool,
    bytes: u64,
    identity: String,
    name: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Journal {
    schema_version: String,
    expected_generation: u64,
    expected_digest: String,
    directory: String,
    directory_identity: Option<String>,
    ready: bool,
    target: ProjectionSnapshot,
    old: Vec<Record>,
    new: Vec<Record>,
}
impl ProjectionStore {
    pub fn refresh_projection(
        &self,
        expected_generation: u64,
        target: &ProjectionSnapshot,
        write_file: &mut dyn FnMut(&str, &mut dyn Write) -> io::Result<()>,
    ) -> Result<VersionedProjection> {
        self.refresh_projection_with_faults(
            expected_generation,
            target,
            write_file,
            &NoRefreshFaults,
        )
    }
    pub fn refresh_projection_with_faults(
        &self,
        expected_generation: u64,
        target: &ProjectionSnapshot,
        write_file: &mut dyn FnMut(&str, &mut dyn Write) -> io::Result<()>,
        faults: &dyn RefreshFaults,
    ) -> Result<VersionedProjection> {
        let mut access = format::access(self, SessionMode::Exclusive)?;
        database::require_mutable(&access.connection)?;
        let current = database::current(&access.connection)?;
        if current.generation != expected_generation {
            return Err(ProjectionStoreError::Stale {
                expected: expected_generation,
                actual: current.generation,
            });
        }
        let planned = plan::prepare(self, &current.snapshot, target)?;
        if planned.target == current.snapshot {
            return Ok(current);
        }
        let directory = format!(
            "refresh-{}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(invalid)?
                .as_nanos(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        );
        let mut journal = Journal {
            schema_version: "layrs.projection-refresh.v1".into(),
            expected_generation,
            expected_digest: current.digest,
            directory,
            directory_identity: None,
            ready: false,
            target: planned.target,
            old: planned.old,
            new: vec![],
        };
        access.verify()?;
        let payload = encode(&journal)?;
        access.connection.execute(
            "INSERT INTO projection_refresh_pending VALUES(1,?1)",
            [payload],
        )?;
        faults.check(RefreshPoint::IntentDurable)?;
        let parent = PinnedDirectory::open(&self.root.join(".layrs"))?
            .ensure_private_child("projection-quarantine".as_ref())?;
        let directory = parent.ensure_private_child(journal.directory.as_ref())?;
        journal.directory_identity = Some(directory.identity_token());
        let targets: BTreeMap<_, _> = journal
            .target
            .entries
            .iter()
            .map(|e| (e.path.as_str(), e))
            .collect();
        let mut total = 0;
        for (index, path) in planned.install.iter().enumerate() {
            let entry = targets[path.as_str()];
            let Some(TreeEntryTarget::File { file_id }) = &entry.target else {
                return Err(invalid("refresh file target missing"));
            };
            let name = format!("new-{index:08}");
            let mut staged = directory.create_file(name.as_ref())?;
            let mut writer =
                install::files::FileWriter::new(staged.file_mut(), MAX_FILE.min(MAX_TOTAL - total));
            write_file(path, &mut writer)?;
            let bytes = writer.finish(file_id)?;
            total += bytes;
            install::files::set_mode(staged.file(), entry.executable)?;
            staged.file().sync_all()?;
            journal.new.push(Record {
                path: path.clone(),
                file_id: file_id.clone(),
                executable: entry.executable,
                bytes,
                identity: file::identity(staged.file())?,
                name,
            });
        }
        directory.sync()?;
        parent.sync()?;
        faults.check(RefreshPoint::Staged)?;
        journal.ready = true;
        access.verify()?;
        access.connection.execute(
            "UPDATE projection_refresh_pending SET payload=?1 WHERE singleton=1",
            [encode(&journal)?],
        )?;
        faults.check(RefreshPoint::JournalDurable)?;
        recovery::finish(self, &mut access, &journal, faults)
    }
    pub fn resume_refresh(checkout_root: impl AsRef<Path>) -> Result<VersionedProjection> {
        let root = std::path::absolute(checkout_root)?;
        let marker = format::read_marker(&root)?;
        recovery::resume(&Self { root, marker })
    }
}
fn encode(journal: &Journal) -> Result<Vec<u8>> {
    let bytes = layrs_core::scm::canonical_json(journal).map_err(invalid)?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err(invalid("refresh journal exceeds64MiB"));
    }
    Ok(bytes)
}

use super::super::{
    journal,
    recovery::error::{Result, invalid},
};
use crate::{
    projection::file::{DatabasePin, identity},
    secure_fs::PinnedDirectory,
};
use std::{
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

pub(in crate::projection) struct DatabaseFile {
    path: PathBuf,
    pin: DatabasePin,
    parent: PinnedDirectory,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::projection::provider) struct FileObservation {
    pub(in crate::projection::provider) identity: String,
    pub(in crate::projection::provider) bytes: u64,
}
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Snapshot {
    database: FileObservation,
    header: Option<[u8; 100]>,
    pub(super) journal: Option<FileObservation>,
}
impl Snapshot {
    pub(super) fn empty(&self) -> bool {
        self.database.bytes == 0
    }
    pub(super) fn database_evidence(&self) -> Result<journal::DatabaseHeaderEvidence> {
        Ok(journal::DatabaseHeaderEvidence::from_observed(
            self.header
                .as_ref()
                .ok_or_else(|| invalid("zero database has no replay evidence"))?,
            self.database.bytes,
        )?)
    }
}
impl DatabaseFile {
    pub(in crate::projection) fn open(path: PathBuf, expected: &str) -> Result<Self> {
        let parent = PinnedDirectory::open(
            path.parent()
                .ok_or_else(|| invalid("database parent absent"))?,
        )?;
        let pin = DatabasePin::open(&path)?;
        if pin.identity() != expected {
            return Err(invalid("provider database native identity differs"));
        }
        parent.verify_path_identity()?;
        Ok(Self { path, pin, parent })
    }
    pub(in crate::projection) fn path(&self) -> &Path {
        &self.path
    }
    pub(in crate::projection) fn verify(&self) -> Result<()> {
        self.parent.verify_path_identity()?;
        self.pin.verify(&self.path)?;
        Ok(())
    }
    pub(in crate::projection::provider) fn sidecar_path(&self, suffix: &str) -> PathBuf {
        let mut value = self.path.as_os_str().to_os_string();
        value.push(suffix);
        PathBuf::from(value)
    }
    pub(super) fn snapshot(&self) -> Result<Snapshot> {
        self.verify()?;
        // Presence alone is never a hot-journal verdict. No directory scan and
        // no DELETE handle; each short-lived read handle closes in probe_leaf.
        let journal = self.probe_leaf("-journal")?;
        let wal = self.probe_leaf("-wal")?;
        let shm = self.probe_leaf("-shm")?;
        self.verify()?;
        if wal.is_some() || shm.is_some() {
            return Err(invalid(
                "provider database WAL/SHM sidecar present before SQLite open",
            ));
        }
        let mut file = crate::layout::open_read_no_follow(&self.path)?;
        let observed = FileObservation {
            identity: identity(&file)?,
            bytes: file.metadata()?.len(),
        };
        if observed.identity != self.pin.identity() {
            return Err(invalid("database changed before header probe"));
        }
        let header = if observed.bytes == 0 {
            if journal.is_some() {
                return Err(invalid(
                    "zero database has a sidecar; SQLite open forbidden",
                ));
            }
            None
        } else {
            let mut header = [0; 100];
            file.seek(SeekFrom::Start(0))?;
            file.read_exact(&mut header)?;
            // Reject WAL page-header versions BEFORE SQLite, including RO.
            journal::DatabaseHeaderEvidence::from_observed(&header, observed.bytes)?;
            Some(header)
        };
        if identity(&file)? != observed.identity || file.metadata()?.len() != observed.bytes {
            return Err(invalid("database changed during header probe"));
        }
        drop(file);
        self.verify()?;
        // Bracket the header with fresh fixed-name observations as well.
        if self.probe_leaf("-journal")? != journal
            || self.probe_leaf("-wal")?.is_some()
            || self.probe_leaf("-shm")?.is_some()
        {
            return Err(invalid("sidecars changed during database preflight"));
        }
        self.verify()?;
        Ok(Snapshot {
            database: observed,
            header,
            journal,
        })
    }
    fn probe_leaf(&self, suffix: &str) -> Result<Option<FileObservation>> {
        self.parent.verify_path_identity()?;
        let result = match crate::layout::open_read_no_follow(&self.sidecar_path(suffix)) {
            Ok(file) => Some(FileObservation {
                identity: identity(&file)?,
                bytes: file.metadata()?.len(),
            }),
            // Only leaf FILE_NOT_FOUND, not PATH_NOT_FOUND, unknown, reparse,
            // sharing or generic ErrorKind::NotFound. Parent pins bracket it.
            Err(e) if e.raw_os_error() == Some(2) => None,
            Err(e) => return Err(e.into()),
        };
        self.parent.verify_path_identity()?;
        self.pin.verify(&self.path)?;
        Ok(result)
    }
}

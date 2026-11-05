use crate::{Error, Result, checked_range};
use layrs_core::scm::{FileId, RepoPath};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// User files plus their distinct directories; excludes the root itself and
/// the one provider control entry, which requires separate identity validation.
pub const MAX_ENTRIES: usize = 100_000;
const MAX_CATALOG_BYTES: usize = 32 * 1024 * 1024;

#[cfg(any(windows, test))]
#[derive(Default)]
pub(crate) struct NamespaceBudget {
    user_entries: usize,
    control_seen: bool,
}

#[cfg(any(windows, test))]
impl NamespaceBudget {
    pub(crate) fn user_entry(&mut self) -> Result<()> {
        if self.user_entries == MAX_ENTRIES {
            return Err(Error::Unsupported("user namespace entry budget exceeded"));
        }
        self.user_entries += 1;
        Ok(())
    }

    /// Call only after the exact attached capability has been validated. A
    /// filename alone must never spend this separate internal allowance.
    pub(crate) fn verified_control_entry(&mut self) -> Result<()> {
        if self.control_seen {
            return Err(Error::Unsupported("duplicate provider control entry"));
        }
        self.control_seen = true;
        Ok(())
    }
    pub(crate) fn finish(&self, attached_control: bool) -> Result<()> {
        if self.control_seen != attached_control {
            return Err(Error::IdentityMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub path: RepoPath,
    pub file_id: FileId,
    pub size: u64,
}

/// A complete, immutable regular-file namespace. Directories are derived.
#[derive(Debug, Clone)]
pub struct Catalog {
    version: u64,
    identity: FileId,
    entries: Vec<CatalogEntry>,
    directories: BTreeSet<String>,
}

impl Catalog {
    pub fn new(version: u64, mut entries: Vec<CatalogEntry>) -> Result<Self> {
        if entries.len() > MAX_ENTRIES {
            return Err(Error::InvalidCatalog("entry budget exceeded".into()));
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let mut names = BTreeMap::<String, (String, bool)>::new();
        let mut sizes = BTreeMap::new();
        let mut directories = BTreeSet::new();
        let mut encoded = b"layrs-vfs-catalog-v1\0".to_vec();
        encoded.extend_from_slice(&version.to_le_bytes());
        for entry in &entries {
            validate_path(&entry.path)?;
            if entry.size > i64::MAX as u64 {
                return Err(Error::InvalidRange);
            }
            if let Some(size) = sizes.insert(entry.file_id.clone(), entry.size) {
                if size != entry.size {
                    return Err(Error::InvalidCatalog(
                        "same FileId has different sizes".into(),
                    ));
                }
            }
            let components: Vec<_> = entry.path.as_str().split('/').collect();
            for depth in 1..=components.len() {
                let name = components[..depth].join("/");
                let directory = depth != components.len();
                let key = name.to_ascii_lowercase();
                if let Some((old, was_directory)) = names.get(&key) {
                    if old != &name || !directory || !was_directory {
                        return Err(Error::InvalidCatalog(format!(
                            "alias, duplicate or file/directory collision: {name}"
                        )));
                    }
                } else {
                    names.insert(key, (name.clone(), directory));
                }
                if directory {
                    directories.insert(name);
                }
            }
            encoded.extend_from_slice(&(entry.path.as_str().len() as u32).to_le_bytes());
            encoded.extend_from_slice(entry.path.as_str().as_bytes());
            encoded.extend_from_slice(entry.file_id.as_str().as_bytes());
            encoded.extend_from_slice(&entry.size.to_le_bytes());
            if encoded.len() > MAX_CATALOG_BYTES || names.len() > MAX_ENTRIES {
                return Err(Error::InvalidCatalog(
                    "catalog metadata budget exceeded".into(),
                ));
            }
        }
        Ok(Self {
            version,
            identity: FileId::from_bytes(&encoded),
            entries,
            directories,
        })
    }

    pub fn version(&self) -> u64 {
        self.version
    }
    pub fn identity(&self) -> &FileId {
        &self.identity
    }
    pub fn entries(&self) -> &[CatalogEntry] {
        &self.entries
    }
    pub fn directories(&self) -> &BTreeSet<String> {
        &self.directories
    }
    pub fn entry(&self, path: &RepoPath) -> Option<&CatalogEntry> {
        self.entries
            .binary_search_by(|e| e.path.cmp(path))
            .ok()
            .map(|i| &self.entries[i])
    }

    pub(crate) fn file_identity(&self, index: usize) -> Vec<u8> {
        let e = &self.entries[index];
        let mut bytes = b"LYRFILE1".to_vec();
        bytes.extend_from_slice(self.identity.as_str().as_bytes());
        bytes.extend_from_slice(&(index as u32).to_le_bytes());
        bytes.extend_from_slice(e.file_id.as_str().as_bytes());
        bytes.extend_from_slice(&e.size.to_le_bytes());
        bytes
    }

    pub(crate) fn resolve_identity(&self, bytes: &[u8]) -> Result<&CatalogEntry> {
        if bytes.len() != 162
            || &bytes[..8] != b"LYRFILE1"
            || &bytes[8..79] != self.identity.as_str().as_bytes()
        {
            return Err(Error::IdentityMismatch);
        }
        let index = u32::from_le_bytes(bytes[79..83].try_into().unwrap()) as usize;
        let e = self.entries.get(index).ok_or(Error::IdentityMismatch)?;
        if bytes != self.file_identity(index) {
            return Err(Error::IdentityMismatch);
        }
        Ok(e)
    }
}

/// Deliberately narrow v1 namespace: ASCII avoids pretending Unicode casefold
/// matches NTFS's upcase table. Tilde aliases, DOS devices and ADS are refused.
pub(crate) fn validate_path(path: &RepoPath) -> Result<()> {
    let name = path.as_str();
    if !name.is_ascii() || name.len() > 1024 || name.split('/').count() > 64 {
        return Err(Error::InvalidCatalog(format!("unsupported path: {name}")));
    }
    for part in name.split('/') {
        // RepoPath validates syntax only. Match ordinary checkout/projection
        // metadata fences here; only Session's exact attached control capability
        // can bypass this validator during enumeration.
        if part.eq_ignore_ascii_case(".layrs") || part.eq_ignore_ascii_case(".git") {
            return Err(Error::InvalidCatalog(format!(
                "reserved metadata component: {name}"
            )));
        }
        let stem = part
            .split('.')
            .next()
            .unwrap()
            .trim_end_matches(' ')
            .to_ascii_uppercase();
        if part.ends_with(['.', ' '])
            || part.len() > 255
            || part.bytes().any(|c| c < 32 || b"<>:\"\\|?*~".contains(&c))
            || matches!(
                stem.as_str(),
                "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
            )
            || ((stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.len() == 4
                && stem.as_bytes()[3].is_ascii_digit())
        {
            return Err(Error::InvalidCatalog(format!(
                "unsupported Windows name: {name}"
            )));
        }
    }
    Ok(())
}

#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// The adapter owns a fixed snapshot and pins its verified immutable bytes for
/// the lifetime of all sessions. It must fill the entire buffer or return an
/// error, never synthesize zeros, and honor cancellation with bounded latency.
/// This includes lock/lease acquisition: use cancellable try-acquire/bounded
/// waits even when a queued exclusive owner would starve shared acquisition.
/// Retain lifetime ownership leases until native work has actually drained;
/// cancellation never authorizes early return/drop while that work is live.
/// The provider holds no pending-request mutex across this call. It cannot
/// interrupt a nonconforming arbitrary source or authenticate Store receipts.
/// This synchronous contract is suitable for a local verified CAS adapter.
pub trait SnapshotSource: Send + Sync + 'static {
    fn catalog(&self) -> &Catalog;
    fn read_exact_at(
        &self,
        id: &FileId,
        offset: u64,
        buffer: &mut [u8],
        cancel: &Cancellation,
    ) -> Result<()>;
}

pub(crate) fn read_checked(
    source: &dyn SnapshotSource,
    entry: &CatalogEntry,
    offset: u64,
    buffer: &mut [u8],
    cancel: &Cancellation,
) -> Result<()> {
    checked_range(entry.size, offset, buffer.len() as u64)?;
    cancel.check()?;
    source.read_exact_at(&entry.file_id, offset, buffer, cancel)?;
    cancel.check()
}

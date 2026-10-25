use super::{InspectionPurpose, Session, fs};
use crate::{CaptureEvidence, Error, Inspection, Result};
use layrs_core::scm::RepoPath;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
};
use windows_sys::Win32::Storage::{CloudFilters::*, FileSystem::*};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Node {
    volume: u32,
    index: u64,
    directory: bool,
}
type Namespace = BTreeMap<String, Node>;

/// Retains effective file and directory identity guards. Callers must quiesce
/// namespace writers and keep this value through CAS/ack. Directory handles do
/// NOT make child-name creation atomic; revalidate before accepting a capture.
pub struct Checkpoint<'a> {
    session: &'a Session,
    entries: Vec<Inspection>,
    namespace: Namespace,
    _directories: Vec<File>,
    root_proof: Option<super::provider_root::ProviderRootGuard>,
}

impl Session {
    pub fn checkpoint(&self) -> Result<Checkpoint<'_>> {
        self.checkpoint_for(InspectionPurpose::Capture)
    }

    pub(super) fn checkpoint_for(&self, purpose: InspectionPurpose) -> Result<Checkpoint<'_>> {
        self.verify_registration()?;
        let (namespace, directories) = self.guarded_namespace()?;
        let mut paths: BTreeSet<_> = self
            .catalog
            .entries()
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        for (path, node) in &namespace {
            if !node.directory {
                paths.insert(RepoPath::parse(path).map_err(|_| Error::IdentityMismatch)?);
            }
        }
        if paths.len() > crate::catalog::MAX_ENTRIES {
            return Err(Error::Unsupported("checkpoint entry budget exceeded"));
        }
        let mut entries = Vec::with_capacity(paths.len());
        for path in paths {
            let entry = match namespace.get(path.as_str()) {
                Some(node) if !node.directory => {
                    let file = fs::open(
                        &self.root.root.join(path.as_str()),
                        FILE_SHARE_READ,
                        FILE_READ_DATA | FILE_READ_ATTRIBUTES,
                    )?;
                    // Directory guards are already retained. Native identities
                    // are reconciled with this exact namespace before return.
                    self.inspect_file_for(&path, file, Vec::new(), purpose)?
                }
                Some(_) => return Err(Error::Unsupported("catalog file replaced by directory")),
                // Do not reopen an absent spelling: Win32 would resolve a
                // differently cased ordinary replacement to the same handle.
                None => self.deleted(&path)?,
            };
            entries.push(entry);
        }
        let checkpoint = Checkpoint {
            session: self,
            entries,
            namespace,
            _directories: directories,
            root_proof: self
                .control
                .proof()
                .map(|proof| proof.try_clone())
                .transpose()?,
        };
        checkpoint.revalidate_namespace()?;
        Ok(checkpoint)
    }

    fn guarded_namespace(&self) -> Result<(Namespace, Vec<File>)> {
        self.control.revalidate()?;
        let mut state = BTreeMap::new();
        let mut guards = Vec::new();
        let mut pending = vec![String::new()];
        let mut budget = crate::catalog::NamespaceBudget::default();
        while let Some(relative) = pending.pop() {
            let dir = fs::open(
                &self.root.root.join(&relative),
                FILE_SHARE_READ,
                FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
            )?;
            let attributes = fs::attributes(&dir)?;
            if attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
                || (!relative.is_empty()
                    && attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0)
            {
                return Err(Error::Unsupported(
                    "checkpoint directory changed type or became reparse point",
                ));
            }
            let identity = fs::identity(&dir)?;
            state.insert(
                relative.clone(),
                Node {
                    volume: identity.dwVolumeSerialNumber,
                    index: fs::index(&identity),
                    directory: true,
                },
            );
            guards.push(dir);
            for item in std::fs::read_dir(self.root.root.join(&relative))? {
                let item = item?;
                let leaf = item
                    .file_name()
                    .into_string()
                    .map_err(|_| Error::Unsupported("non-Unicode checkpoint name"))?;
                let path = if relative.is_empty() {
                    leaf
                } else {
                    format!("{relative}/{leaf}")
                };
                // The provider reserves `.layrs` beyond RepoPath's syntax
                // checks. Only this exact attached capability may be excluded.
                if path == ".layrs" && self.control.is_some() {
                    let file = fs::open(
                        &item.path(),
                        super::control_directory_io::ALL_SHARING,
                        self.control.observer_access(),
                    )?;
                    if self.is_control(&path, &file)? {
                        budget.verified_control_entry()?;
                        continue;
                    }
                }
                budget.user_entry()?;
                let repo_path = RepoPath::parse(&path)
                    .map_err(|_| Error::Unsupported("invalid checkpoint path"))?;
                crate::catalog::validate_path(&repo_path)?;
                // This probe is observation only. The next phase acquires the
                // effective data-read guard; identities are reconciled afterward.
                let file = fs::open(
                    &item.path(),
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    FILE_READ_ATTRIBUTES,
                )?;
                let attr = fs::attributes(&file)?;
                let native = fs::identity(&file)?;
                if attr.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                    if attr.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                        return Err(Error::Unsupported("checkpoint reparse directory"));
                    }
                    pending.push(path);
                } else {
                    let placeholder =
                        unsafe { (self.api.state)(attr.FileAttributes, attr.ReparseTag) };
                    if native.nNumberOfLinks != 1
                        || placeholder == CF_PLACEHOLDER_STATE_INVALID
                        || (attr.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
                            && placeholder & CF_PLACEHOLDER_STATE_PLACEHOLDER == 0)
                    {
                        return Err(Error::Unsupported(
                            "checkpoint unsupported file type or hardlink",
                        ));
                    }
                    state.insert(
                        path,
                        Node {
                            volume: native.dwVolumeSerialNumber,
                            index: fs::index(&native),
                            directory: false,
                        },
                    );
                }
            }
        }
        budget.finish(self.control.is_some())?;
        self.control.revalidate()?;
        Ok((state, guards))
    }
}

impl Checkpoint<'_> {
    /// Actual retained V2 proof; legacy checkpoints intentionally return None.
    pub fn root_guard(&self) -> Option<&super::ProviderRootGuard> {
        self.root_proof.as_ref()
    }
    pub(super) fn session(&self) -> &Session {
        self.session
    }

    pub(super) fn validate_export_namespace(&self) -> Result<()> {
        // Casefold is only a collision detector, never a path reconciliation.
        let mut names = BTreeSet::new();
        for path in self.namespace.keys() {
            if !names.insert(path.to_ascii_lowercase()) {
                return Err(Error::Unsupported("export namespace contains case aliases"));
            }
        }
        Ok(())
    }

    pub fn entries(&self) -> &[Inspection] {
        &self.entries
    }

    pub fn revalidate_namespace(&self) -> Result<()> {
        if let Some(proof) = &self.root_proof {
            proof.revalidate()?;
        }
        self.session.verify_registration()?;
        let (current, _guards) = self.session.guarded_namespace()?;
        if current != self.namespace {
            return Err(Error::Busy);
        }
        // Bind the file handles used by consumers to the namespace we observed,
        // including ordinary files that do not have a CFAPI backing identity.
        for entry in &self.entries {
            if let Some(file) = &entry._guard {
                let actual = fs::identity(file)?;
                let expected = current.get(entry.path.as_str()).ok_or(Error::Busy)?;
                if expected.volume != actual.dwVolumeSerialNumber
                    || expected.index != fs::index(&actual)
                {
                    return Err(Error::Busy);
                }
            } else if current.contains_key(entry.path.as_str()) {
                return Err(Error::Busy);
            }
        }
        if let Some(proof) = &self.root_proof {
            proof.revalidate()?;
        }
        Ok(())
    }

    /// Reads only a NeedsHash entry, on its retained handle. A clean immutable
    /// placeholder cannot be implicitly hydrated through this API. The consumer
    /// must read every byte before returning success; no path is reopened.
    pub fn with_reader<T>(
        &mut self,
        path: &RepoPath,
        consume: impl FnOnce(&mut dyn Read) -> Result<T>,
    ) -> Result<T> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.path == *path)
            .ok_or(Error::IdentityMismatch)?;
        let CaptureEvidence::NeedsHash { size, .. } = entry.evidence else {
            return Err(Error::Unsupported(
                "only NeedsHash checkpoint entries may read content",
            ));
        };
        let guard = entry._guard.as_ref().ok_or(Error::IdentityMismatch)?;
        if entry.placeholder.is_some() {
            #[cfg(test)]
            super::stage2_tests::hydration_call();
            super::api::check("CfHydratePlaceholder(checkpoint)", unsafe {
                (self.session.api.hydrate)(
                    fs::handle(guard),
                    0,
                    -1,
                    CF_HYDRATE_FLAG_NONE,
                    std::ptr::null_mut(),
                )
            })?;
        }
        let mut file = guard.try_clone()?;
        file.seek(SeekFrom::Start(0))?;
        let mut reader = Counted {
            inner: file.take(size),
            count: 0,
        };
        let result = consume(&mut reader)?;
        if reader.count != size {
            return Err(Error::InvalidRange);
        }
        Ok(result)
    }
}

struct Counted<R> {
    inner: R,
    count: u64,
}
impl<R: Read> Read for Counted<R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(bytes)?;
        self.count += count as u64;
        Ok(count)
    }
}

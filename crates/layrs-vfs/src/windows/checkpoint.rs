use super::{ChangeSentinel, InspectionPurpose, Session, fs};
use crate::{CaptureEvidence, Error, Inspection, ListedFile, ListedPlaceholder, Result, Vouch};
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
type Listing = BTreeMap<String, ListedFile>;
/// Decides, from listing facts alone, whether a file can be admitted unopened.
pub type TrustHook<'t> = &'t mut dyn FnMut(&RepoPath, &ListedFile) -> Option<Vouch>;
/// `(path, is_directory)` that a capture leaves out (ignore rules). A path of
/// the catalog, or a directory holding one, is never left out.
pub type CaptureFilter = std::sync::Arc<dyn Fn(&str, bool) -> bool + Send + Sync>;

/// Retains effective file and directory identity guards. Callers must quiesce
/// namespace writers and keep this value through CAS/ack. Directory handles do
/// NOT make child-name creation atomic; revalidate before accepting a capture.
pub struct Checkpoint<'a> {
    session: &'a Session,
    entries: Vec<Inspection>,
    namespace: Namespace,
    listing: Listing,
    filter: Option<CaptureFilter>,
    _directories: Vec<File>,
    root_proof: Option<super::provider_root::ProviderRootGuard>,
}

impl Session {
    pub fn checkpoint(&self) -> Result<Checkpoint<'_>> {
        self.checkpoint_for(InspectionPurpose::Capture, None, false, None)
    }

    /// The verification required before deleting a root (ADR 0025): every
    /// placeholder listed in sync is inspected live through its own handle
    /// (CFAPI in-sync state, modified bytes, identity and native number), with
    /// attribute access only so no content is read or scanned; any other file
    /// is guarded and its bytes are hashed by the consumer.
    pub fn checkpoint_verifying(&self, filter: Option<CaptureFilter>) -> Result<Checkpoint<'_>> {
        self.checkpoint_for(InspectionPurpose::Capture, None, true, filter)
    }

    /// As `checkpoint`, where `trust` may admit a file from its directory
    /// listing alone (ADR 0025): an in-sync placeholder as its catalog base,
    /// or content the caller proved earlier. Such a file is not opened and
    /// holds no guard; revalidation requires its same listed version. Every
    /// other file is inspected through an effective guard exactly as before.
    /// `filter` leaves ignored paths out of the walk entirely (not listed,
    /// not opened, not captured), as an ordinary capture does.
    pub fn checkpoint_trusting(
        &self,
        trust: TrustHook<'_>,
        filter: Option<CaptureFilter>,
    ) -> Result<Checkpoint<'_>> {
        self.checkpoint_for(InspectionPurpose::Capture, Some(trust), false, filter)
    }

    /// Listing facts of every regular file, from guarded directory
    /// enumerations only. Observation, not a capture.
    pub fn list_files(&self) -> Result<BTreeMap<String, ListedFile>> {
        self.verify_registration()?;
        Ok(self.guarded_namespace(None)?.1)
    }

    pub(super) fn checkpoint_for(
        &self,
        purpose: InspectionPurpose,
        mut trust: Option<TrustHook<'_>>,
        live_placeholders: bool,
        filter: Option<CaptureFilter>,
    ) -> Result<Checkpoint<'_>> {
        self.verify_registration()?;
        let filter = filter.filter(|_| purpose == InspectionPurpose::Capture);
        let (namespace, listing, directories) = self.guarded_namespace(filter.as_ref())?;
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
        // The live probes of in-sync placeholders (attribute-only opens, no
        // data access, nothing hydrated) are independent: run them on a few
        // threads (council r5, D1). Each verdict is used exactly as before;
        // a refused probe still sends the file to the guarded path.
        let mut probed: BTreeMap<String, Option<Inspection>> = BTreeMap::new();
        if live_placeholders && purpose == InspectionPurpose::Capture && trust.is_none() {
            let candidates: Vec<(&RepoPath, &ListedFile)> = paths
                .iter()
                .filter_map(|path| {
                    listing
                        .get(path.as_str())
                        .filter(|listed| listed.placeholder.is_some_and(|p| p.in_sync))
                        .map(|listed| (path, listed))
                })
                .collect();
            let threads = std::thread::available_parallelism()
                .map_or(4, |n| n.get())
                .clamp(1, 8)
                .min(candidates.len().max(1));
            let per = candidates.len().div_ceil(threads).max(1);
            std::thread::scope(|scope| {
                let workers: Vec<_> = candidates
                    .chunks(per)
                    .map(|chunk| {
                        scope.spawn(move || {
                            chunk
                                .iter()
                                .map(|(path, listed)| {
                                    (path.as_str().to_owned(), self.live_base(path, listed))
                                })
                                .collect::<Vec<_>>()
                        })
                    })
                    .collect();
                for worker in workers {
                    if let Ok(results) = worker.join() {
                        probed.extend(results);
                    }
                }
            });
        }
        let mut entries = Vec::with_capacity(paths.len());
        let mut vouched = 0u64;
        for path in paths {
            let admitted = match (namespace.get(path.as_str()), listing.get(path.as_str())) {
                (Some(node), Some(listed))
                    if !node.directory && purpose == InspectionPurpose::Capture =>
                {
                    trust
                        .as_mut()
                        .and_then(|trust| trust(&path, listed))
                        .and_then(|vouch| self.vouched(&path, listed, vouch))
                }
                _ => None,
            };
            if let Some(entry) = admitted {
                vouched += 1;
                entries.push(entry);
                continue;
            }
            if live_placeholders && purpose == InspectionPurpose::Capture {
                let live = match probed.remove(path.as_str()) {
                    Some(verdict) => verdict,
                    None => listing
                        .get(path.as_str())
                        .filter(|listed| listed.placeholder.is_some_and(|p| p.in_sync))
                        .and_then(|listed| self.live_base(&path, listed)),
                };
                if let Some(entry) = live {
                    entries.push(entry);
                    continue;
                }
            }
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
        let _ = vouched;
        let checkpoint = Checkpoint {
            session: self,
            entries,
            namespace,
            listing,
            filter,
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

    /// A placeholder still at its catalog path, live in sync with no modified
    /// bytes, inspected through an attribute-only handle (no data access, so
    /// nothing is hydrated or scanned). `None` sends it to the guarded path.
    fn live_base(&self, path: &RepoPath, listed: &ListedFile) -> Option<Inspection> {
        let probe = fs::open(
            &self.root.root.join(path.as_str()),
            super::control_directory_io::ALL_SHARING,
            FILE_READ_ATTRIBUTES,
        )
        .ok()?;
        let mut inspection = self
            .inspect_file_for(path, probe, Vec::new(), InspectionPurpose::Capture)
            .ok()?;
        let live = inspection.placeholder.as_ref()?;
        if !matches!(inspection.evidence, CaptureEvidence::Immutable { .. })
            || !live.in_sync
            || live.modified_bytes != 0
            || live.file_id as u64 != listed.native_index
        {
            return None;
        }
        inspection._guard = None;
        inspection.listed = Some(*listed);
        Some(inspection)
    }

    /// A catalog file, or a directory holding catalog content.
    fn tracked(&self, path: &str, directory: bool) -> bool {
        if directory {
            return self.catalog.directories().contains(path);
        }
        RepoPath::parse(path).is_ok_and(|path| self.catalog.entry(&path).is_some())
    }

    /// The provider's own checks on a hook's verdict.
    fn vouched(&self, path: &RepoPath, listed: &ListedFile, vouch: Vouch) -> Option<Inspection> {
        let evidence = match vouch {
            Vouch::Base => {
                let entry = self.catalog.entry(path)?;
                if !listed.placeholder.is_some_and(|p| p.in_sync) || listed.size != entry.size {
                    return None;
                }
                CaptureEvidence::Immutable {
                    file_id: entry.file_id.clone(),
                    size: entry.size,
                    original_path: path.clone(),
                }
            }
            Vouch::Content(file_id) => CaptureEvidence::Vouched {
                file_id,
                size: listed.size,
            },
        };
        Some(Inspection {
            path: path.clone(),
            evidence,
            placeholder: None,
            _guard: None,
            _parents: Vec::new(),
            listed: Some(*listed),
        })
    }

    /// Walks the namespace through retained directory guards, enumerating
    /// each directory by its handle: no child file is opened (the control
    /// entry excepted). Files must be ordinary or CFAPI placeholders.
    fn guarded_namespace(
        &self,
        filter: Option<&CaptureFilter>,
    ) -> Result<(Namespace, Listing, Vec<File>)> {
        self.control.revalidate()?;
        let mut state = BTreeMap::new();
        let mut listing = BTreeMap::new();
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
            let volume = identity.dwVolumeSerialNumber;
            let listed = fs::enumerate(&dir)?;
            guards.push(dir);
            for item in listed {
                let path = if relative.is_empty() {
                    item.name.clone()
                } else {
                    format!("{relative}/{}", item.name)
                };
                // The provider reserves `.layrs` beyond RepoPath's syntax
                // checks. Only this exact attached capability may be excluded.
                if path == ".layrs" && self.control.is_some() {
                    let file = fs::open(
                        &self.root.root.join(&path),
                        super::control_directory_io::ALL_SHARING,
                        self.control.observer_access(),
                    )?;
                    if self.is_control(&path, &file)? {
                        budget.verified_control_entry()?;
                        continue;
                    }
                }
                let directory = item.attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
                if filter.is_some_and(|ignored| ignored(&path, directory))
                    && !self.tracked(&path, directory)
                {
                    continue;
                }
                budget.user_entry()?;
                let repo_path = RepoPath::parse(&path)
                    .map_err(|_| Error::Unsupported("invalid checkpoint path"))?;
                crate::catalog::validate_path(&repo_path)?;
                if directory {
                    if item.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                        return Err(Error::Unsupported("checkpoint reparse directory"));
                    }
                    pending.push(path);
                } else {
                    let placeholder = unsafe { (self.api.state)(item.attributes, item.tag) };
                    if placeholder == CF_PLACEHOLDER_STATE_INVALID
                        || (item.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
                            && placeholder & CF_PLACEHOLDER_STATE_PLACEHOLDER == 0)
                    {
                        return Err(Error::Unsupported("checkpoint unsupported file type"));
                    }
                    listing.insert(
                        path.clone(),
                        ListedFile {
                            native_index: item.index,
                            size: item.size,
                            attributes: item.attributes,
                            last_write: item.last_write,
                            creation: item.creation,
                            change: item.change,
                            placeholder: (placeholder & CF_PLACEHOLDER_STATE_PLACEHOLDER != 0)
                                .then_some(ListedPlaceholder {
                                    in_sync: placeholder & CF_PLACEHOLDER_STATE_IN_SYNC != 0,
                                    partial: placeholder & CF_PLACEHOLDER_STATE_PARTIAL != 0,
                                }),
                        },
                    );
                    state.insert(
                        path,
                        Node {
                            volume,
                            index: item.index,
                            directory: false,
                        },
                    );
                }
            }
        }
        budget.finish(self.control.is_some())?;
        self.control.revalidate()?;
        Ok((state, listing, guards))
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

    /// The listing facts observed for `path` when this checkpoint was taken.
    pub fn listed(&self, path: &RepoPath) -> Option<&ListedFile> {
        self.listing.get(path.as_str())
    }

    /// `revalidate_namespace`, without listing the folder again when
    /// `sentinel` (armed before this checkpoint listed it) proves that
    /// nothing below the root changed since: a new sentinel is armed first,
    /// so that the two leave no gap, then the old one settles. The provider
    /// checks (root proof, registration) run either way. Any reported change,
    /// an overflow or no sentinel lists the folder as `revalidate_namespace`.
    /// Returns whether the listing was skipped.
    pub fn revalidate_namespace_watched(
        &self,
        sentinel: &mut Option<ChangeSentinel>,
    ) -> Result<bool> {
        let next = self.session.arm_change_sentinel().ok();
        let quiet = next.is_some() && sentinel.take().is_some_and(ChangeSentinel::settle);
        *sentinel = next;
        if !quiet {
            self.revalidate_namespace()?;
            return Ok(false);
        }
        if let Some(proof) = &self.root_proof {
            proof.revalidate()?;
        }
        self.session.verify_registration()?;
        Ok(true)
    }

    pub fn revalidate_namespace(&self) -> Result<()> {
        if let Some(proof) = &self.root_proof {
            proof.revalidate()?;
        }
        self.session.verify_registration()?;
        let (current, listing, _guards) = self.session.guarded_namespace(self.filter.as_ref())?;
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
            } else if let Some(listed) = &entry.listed {
                // Admitted unopened: still that very version at that path.
                if !listing
                    .get(entry.path.as_str())
                    .is_some_and(|now| now.same_version(listed))
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

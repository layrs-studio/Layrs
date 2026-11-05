//! Retained ordinary checkout bytes. Immutable Windows leaf handles exclude
//! conflicting ordinary write/delete opens; repeated namespace observations do
//! not claim quiescence against arbitrary writers or future absent-path writes.
use crate::{
    projection::{
        ProjectionStore, Result,
        capture::{
            CapturedFile, MAX_CAPTURE_BYTES, MAX_CAPTURE_FILE_BYTES,
            scan::{executable, scan_selected_readonly},
        },
        file, format, invalid,
        provider::{
            bootstrap::{
                self, ProviderRecoveryExportClass, ProviderRecoveryExportDeletion,
                ProviderRecoveryExportFile,
            },
            recovery::ProviderBootstrapCause as Cause,
        },
        selection,
    },
    secure_fs::{PinnedDirectory, PinnedFile},
};
use layrs_core::{
    projection::{EntryState, ProjectionSelection, ProjectionSnapshot},
    scm::{FileId, FileManifest, RepoPath, TreeEntryTarget, canonical_json},
};
use layrs_vfs::Cancellation;
use rusqlite::Connection;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

const MAX_PATHS: usize = 100_000;
const MAX_PATH_BYTES: usize = 8 * 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 32 * 1024 * 1024;

pub(super) struct OrdinaryImageFile {
    pub(super) path: RepoPath,
    pub(super) manifest: FileManifest,
    pub(super) executable: bool,
    pub(super) expected: Option<(FileId, bool)>,
}

pub(super) struct OrdinaryImageDeletion {
    pub(super) path: RepoPath,
    pub(super) base: FileId,
    pub(super) executable: bool,
}

/// Owns no SQL connection, content scope or family lease. The operation owner
/// must authenticate its SQL/owner separately and call full `revalidate` under
/// its content scope, or `revalidate_parts` under its fresh source capability,
/// before/after effects and each copy. Copy methods check retained leaf bytes.
pub(super) struct OrdinaryExportImage {
    root: PinnedDirectory,
    root_path: PathBuf,
    marker: format::Marker,
    snapshot: ProjectionSnapshot,
    selection: ProjectionSelection,
    scanned_files: BTreeMap<String, Option<CapturedFile>>,
    files: Vec<OrdinaryImageFile>,
    // Same order as files; callers never receive or replace retained handles.
    pins: Vec<PinnedFile>,
    deletions: Vec<OrdinaryImageDeletion>,
    digest: String,
}

impl OrdinaryExportImage {
    pub(super) fn build(
        store: &ProjectionStore,
        c: &Connection,
        scope: &format::ContentScope<'_>,
        snapshot: &ProjectionSnapshot,
        cancel: &Cancellation,
    ) -> Result<Self> {
        check(cancel)?;
        scope.validate(c, store)?;
        let selection = selection::for_scan(c, snapshot)?;
        let image = Self::build_parts(&store.root, &store.marker, snapshot, &selection, cancel)?;
        if selection::for_scan(c, snapshot)? != selection {
            return Err(invalid(
                "ordinary export selection changed during image build",
            ));
        }
        scope.verify(c, store)?;
        check(cancel)?;
        Ok(image)
    }

    /// Physical image proof only. These bounded DATA arguments do not stand
    /// in for authenticated source SQL, a content scope or an exclusive owner.
    /// The fresh source caller must retain and revalidate those independently.
    pub(super) fn build_parts(
        root_path: &Path,
        marker: &format::Marker,
        snapshot: &ProjectionSnapshot,
        selection: &ProjectionSelection,
        cancel: &Cancellation,
    ) -> Result<Self> {
        check(cancel)?;
        selection.validate_snapshot(snapshot)?;
        bound_paths(snapshot, selection, cancel)?;
        let root = PinnedDirectory::open(root_path)?;
        if root.identity_token() != marker.checkout_identity() {
            return Err(invalid("ordinary export root differs from FORMAT checkout"));
        }
        let scan = scan_selected_readonly(root_path, snapshot, selection, cancel)?;
        if scan.files.len() > MAX_PATHS {
            return Err(invalid("ordinary export image exceeds path bound"));
        }
        let entries: BTreeMap<_, _> = snapshot
            .entries
            .iter()
            .filter(|entry| scan.files.contains_key(entry.path.as_str()))
            .map(|entry| (entry.path.as_str(), entry))
            .collect();
        // Complete byte/count preflight precedes retained leaf hashing.
        let total = scan.files.values().flatten().try_fold(0u64, |sum, file| {
            if file.bytes > MAX_CAPTURE_FILE_BYTES {
                return Err(invalid("ordinary export file exceeds 1GiB"));
            }
            sum.checked_add(file.bytes)
                .filter(|sum| *sum <= MAX_CAPTURE_BYTES)
                .ok_or_else(|| invalid("ordinary export image exceeds 4GiB"))
        })?;
        let mut files = Vec::new();
        let mut pins = Vec::new();
        let mut deletions = Vec::new();
        let mut manifest_bytes = 0usize;
        let mut retained_bytes = 0u64;
        for (path, actual) in &scan.files {
            check(cancel)?;
            let entry = entries
                .get(path.as_str())
                .ok_or_else(|| invalid("ordinary export scan path lacks selected entry"))?;
            let expected = match &entry.target {
                Some(TreeEntryTarget::File { file_id }) => {
                    Some((file_id.clone(), entry.executable))
                }
                None if entry.state == EntryState::Absent => None,
                _ => {
                    return Err(invalid(
                        "ordinary export image requires regular-file leaves",
                    ));
                }
            };
            let Some(actual) = actual else {
                if let Some((base, executable)) = expected {
                    if !entry.writable {
                        return Err(invalid("ordinary export deletion lacks write authority"));
                    }
                    deletions.push(OrdinaryImageDeletion {
                        path: entry.path.clone(),
                        base,
                        executable,
                    });
                }
                continue;
            };
            if expected.as_ref().map(|(id, mode)| (id, *mode))
                != Some((&actual.file_id, actual.executable))
                && !entry.writable
            {
                return Err(invalid("ordinary export changed a protected file"));
            }
            let (parent, name) = root.existing_parent(Path::new(path))?;
            let pin = parent.open_immutable_file(&name)?;
            file::require_single_link(pin.file())?;
            let metadata = pin.file().metadata()?;
            if metadata.len() != actual.bytes || executable(&metadata) != actual.executable {
                return Err(invalid("ordinary export file changed after initial scan"));
            }
            let manifest = hash_pin(&pin, actual.bytes, cancel)?;
            if manifest.file_id() != &actual.file_id || manifest.byte_length() != actual.bytes {
                return Err(invalid("ordinary export retained bytes differ from scan"));
            }
            manifest_bytes = manifest_bytes
                .checked_add(canonical_json(&manifest).map_err(invalid)?.len())
                .filter(|bytes| *bytes <= MAX_MANIFEST_BYTES)
                .ok_or_else(|| invalid("ordinary export manifests exceed 32MiB"))?;
            retained_bytes = retained_bytes
                .checked_add(manifest.byte_length())
                .filter(|bytes| *bytes <= MAX_CAPTURE_BYTES)
                .ok_or_else(|| invalid("ordinary export retained bytes exceed bound"))?;
            pin.verify_parent()?;
            parent.verify_path_identity()?;
            if !pin.same_file(&parent.open_readonly_file(&name)?) {
                return Err(invalid("ordinary export selected file was replaced"));
            }
            files.push(OrdinaryImageFile {
                path: entry.path.clone(),
                manifest,
                executable: actual.executable,
                expected,
            });
            pins.push(pin);
        }
        if retained_bytes != total {
            return Err(invalid("ordinary export retained image length differs"));
        }
        let digest = image_digest(&files, &deletions)?;
        let mut image = Self {
            root,
            root_path: root_path.to_owned(),
            marker: marker.clone(),
            snapshot: snapshot.clone(),
            selection: selection.clone(),
            scanned_files: scan.files,
            files,
            pins,
            deletions,
            digest,
        };
        image.revalidate_parts(root_path, marker, snapshot, selection, cancel)?;
        Ok(image)
    }

    pub(super) fn revalidate(
        &mut self,
        store: &ProjectionStore,
        c: &Connection,
        scope: &format::ContentScope<'_>,
        snapshot: &ProjectionSnapshot,
        cancel: &Cancellation,
    ) -> Result<()> {
        check(cancel)?;
        scope.validate(c, store)?;
        let selection = selection::for_scan(c, snapshot)?;
        self.revalidate_parts(&store.root, &store.marker, snapshot, &selection, cancel)?;
        if selection::for_scan(c, snapshot)? != selection {
            return Err(invalid(
                "ordinary export selection changed during image validation",
            ));
        }
        scope.verify(c, store)?;
        check(cancel)
    }

    /// Complete namespace and retained-byte proof without SQL or borrowed
    /// Access. The enclosing fresh owner must authenticate the exact arguments.
    pub(super) fn revalidate_parts(
        &mut self,
        root_path: &Path,
        marker: &format::Marker,
        snapshot: &ProjectionSnapshot,
        selection: &ProjectionSelection,
        cancel: &Cancellation,
    ) -> Result<()> {
        check(cancel)?;
        if root_path != self.root_path.as_path()
            || marker != &self.marker
            || snapshot != &self.snapshot
            || selection != &self.selection
        {
            return Err(invalid(
                "ordinary export owner, snapshot or selection changed",
            ));
        }
        let before = scan_selected_readonly(root_path, snapshot, selection, cancel)?;
        if before.files != self.scanned_files {
            return Err(invalid("ordinary export selected checkout image changed"));
        }
        self.verify_pins(cancel)?;
        let after = scan_selected_readonly(root_path, snapshot, selection, cancel)?;
        if after.files != self.scanned_files {
            return Err(invalid(
                "ordinary export selected checkout changed during validation",
            ));
        }
        check(cancel)
    }

    pub(super) fn digest(&self) -> &str {
        &self.digest
    }

    pub(super) fn files(&self) -> &[OrdinaryImageFile] {
        &self.files
    }

    pub(super) fn deletions(&self) -> &[OrdinaryImageDeletion] {
        &self.deletions
    }

    /// The output may contain a prefix on failure. It remains unpublished until
    /// this and the owner's post-copy full scoped revalidation both succeed.
    pub(super) fn copy_file(
        &mut self,
        path: &RepoPath,
        manifest: &FileManifest,
        output: &mut dyn Write,
        cancel: &Cancellation,
    ) -> Result<()> {
        check(cancel)?;
        let index = self.exact_file(path, manifest)?;
        self.verify_pins(cancel)?;
        let mut input = self.pins[index].file();
        input.seek(SeekFrom::Start(0))?;
        let copied = bootstrap::copy_managed_file(manifest, output, cancel, |part| {
            input.read_exact(part)?;
            Ok(())
        });
        let after = self.verify_pins(cancel);
        finish_copy(copied, after)
    }

    /// A chunk copy authenticates the selected exact representation's chunk;
    /// full retained image checks still run before and after sink callbacks.
    pub(super) fn copy_chunk(
        &mut self,
        path: &RepoPath,
        manifest: &FileManifest,
        ordinal: usize,
        output: &mut dyn Write,
        cancel: &Cancellation,
    ) -> Result<()> {
        check(cancel)?;
        let index = self.exact_file(path, manifest)?;
        self.verify_pins(cancel)?;
        let mut input = self.pins[index].file();
        let copied =
            bootstrap::copy_managed_chunk(manifest, ordinal, output, cancel, |offset, part| {
                input.seek(SeekFrom::Start(offset))?;
                input.read_exact(part)?;
                Ok(())
            });
        let after = self.verify_pins(cancel);
        finish_copy(copied, after)
    }

    fn exact_file(&self, path: &RepoPath, manifest: &FileManifest) -> Result<usize> {
        let index = self
            .files
            .binary_search_by(|file| file.path.cmp(path))
            .map_err(|_| invalid("ordinary export path is outside retained image"))?;
        if self.files[index].manifest != *manifest {
            return Err(invalid("ordinary export exact manifest differs"));
        }
        Ok(index)
    }

    fn verify_pins(&self, cancel: &Cancellation) -> Result<()> {
        check(cancel)?;
        self.root.verify_path_identity()?;
        if self.root.identity_token() != self.marker.checkout_identity()
            || self.pins.len() != self.files.len()
        {
            return Err(invalid(
                "ordinary export retained root or file inventory differs",
            ));
        }
        for (file, pin) in self.files.iter().zip(&self.pins) {
            check(cancel)?;
            file::require_single_link(pin.file())?;
            pin.verify_parent()?;
            let (parent, name) = self.root.existing_parent(Path::new(file.path.as_str()))?;
            parent.verify_path_identity()?;
            let current = parent.open_readonly_file(&name)?;
            if !pin.same_file(&current) {
                return Err(invalid("ordinary export retained path identity changed"));
            }
            let metadata = pin.file().metadata()?;
            if metadata.len() != file.manifest.byte_length()
                || executable(&metadata) != file.executable
                || hash_pin(pin, file.manifest.byte_length(), cancel)? != file.manifest
            {
                return Err(invalid(
                    "ordinary export retained file bytes or mode changed",
                ));
            }
            file::require_single_link(pin.file())?;
            parent.verify_path_identity()?;
            if !pin.same_file(&parent.open_readonly_file(&name)?) {
                return Err(invalid(
                    "ordinary export retained path changed during hashing",
                ));
            }
        }
        self.root.verify_path_identity()?;
        check(cancel)
    }

    /// No content reads: usable inside a catalog transaction while the full
    /// image scans and hashes run before entry and after positive close.
    pub(super) fn verify_identity(&self, cancel: &Cancellation) -> Result<()> {
        check(cancel)?;
        self.root.verify_path_identity()?;
        if self.root.identity_token() != self.marker.checkout_identity()
            || self.pins.len() != self.files.len()
        {
            return Err(invalid("ordinary install image identity differs"));
        }
        for (file, pin) in self.files.iter().zip(&self.pins) {
            check(cancel)?;
            file::require_single_link(pin.file())?;
            pin.verify_parent()?;
            let (parent, name) = self.root.existing_parent(Path::new(file.path.as_str()))?;
            parent.verify_path_identity()?;
            let metadata = pin.file().metadata()?;
            if !pin.same_file(&parent.open_readonly_file(&name)?)
                || metadata.len() != file.manifest.byte_length()
                || executable(&metadata) != file.executable
            {
                return Err(invalid("ordinary install retained leaf identity differs"));
            }
        }
        check(cancel)
    }
}

fn bound_paths(
    snapshot: &ProjectionSnapshot,
    selection: &ProjectionSelection,
    cancel: &Cancellation,
) -> Result<()> {
    let mut count = 0usize;
    let mut bytes = 0usize;
    let mut aliases = BTreeSet::new();
    for entry in &snapshot.entries {
        check(cancel)?;
        if !selection.includes(&entry.path)
            || matches!(entry.state, EntryState::Reserved | EntryState::Unavailable)
            || matches!(entry.target, Some(TreeEntryTarget::Tree { .. }))
        {
            continue;
        }
        count = count
            .checked_add(1)
            .filter(|count| *count <= MAX_PATHS)
            .ok_or_else(|| invalid("ordinary export selected paths exceed bound"))?;
        bytes = bytes
            .checked_add(entry.path.as_str().len())
            .filter(|bytes| *bytes <= MAX_PATH_BYTES)
            .ok_or_else(|| invalid("ordinary export selected path metadata exceeds 8MiB"))?;
        if entry.path.as_str().len() > 4096
            || !aliases.insert(entry.path.as_str().to_ascii_lowercase())
        {
            return Err(invalid("ordinary export path is oversized or aliased"));
        }
    }
    Ok(())
}

fn hash_pin(pin: &PinnedFile, size: u64, cancel: &Cancellation) -> Result<FileManifest> {
    check(cancel)?;
    file::require_single_link(pin.file())?;
    if size > MAX_CAPTURE_FILE_BYTES || pin.file().metadata()?.len() != size {
        return Err(invalid("ordinary export retained file length differs"));
    }
    let mut input = pin.file();
    input.seek(SeekFrom::Start(0))?;
    let manifest = bootstrap::hash_managed_file(&mut input, size, cancel).map_err(Cause::from)?;
    let mut extra = [0u8; 1];
    if input.read(&mut extra)? != 0 || pin.file().metadata()?.len() != size {
        return Err(invalid(
            "ordinary export retained file changed during hashing",
        ));
    }
    file::require_single_link(pin.file())?;
    check(cancel)?;
    Ok(manifest)
}

fn image_digest(
    files: &[OrdinaryImageFile],
    deletions: &[OrdinaryImageDeletion],
) -> Result<String> {
    let files: Vec<_> = files
        .iter()
        .map(|file| ProviderRecoveryExportFile {
            path: file.path.clone(),
            file_id: file.manifest.file_id().clone(),
            length: file.manifest.byte_length(),
            executable: Some(file.executable),
            class: ProviderRecoveryExportClass::ManagedCandidate,
        })
        .collect();
    let deletions: Vec<_> = deletions
        .iter()
        .map(|deletion| ProviderRecoveryExportDeletion {
            path: deletion.path.clone(),
            base: deletion.base.clone(),
            expected_executable: Some(deletion.executable),
            class: ProviderRecoveryExportClass::ManagedCandidate,
        })
        .collect();
    let digest = bootstrap::recovery_image_digest(&files, &deletions, &[])?;
    digest
        .strip_prefix("blake3:")
        .filter(|raw| raw.len() == 64)
        .map(str::to_owned)
        .ok_or_else(|| invalid("ordinary export image digest shape differs"))
}

fn check(cancel: &Cancellation) -> Result<()> {
    cancel.check().map_err(Cause::from)?;
    Ok(())
}

fn finish_copy(copied: std::result::Result<(), Cause>, after: Result<()>) -> Result<()> {
    match (copied, after) {
        (Ok(()), after) => after,
        (Err(error), Ok(())) => Err(error.into()),
        (Err(primary), Err(postcheck)) => Err(Cause::Cleanup {
            primary: Box::new(primary),
            close: Box::new(postcheck.into()),
        }
        .into()),
    }
}

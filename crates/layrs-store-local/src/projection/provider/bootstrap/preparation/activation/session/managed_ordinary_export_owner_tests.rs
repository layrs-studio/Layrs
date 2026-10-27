//! Native ordinary export ownership and byte reads on a genuine admission.
//! Derivation returns DATA only: no destination reservation, physical transfer,
//! publication or admission is performed by this recipe.
use crate::{
    projection::{
        provider::{
            import_validation::content_mapping_rowset_data,
            process_profile::tests::child,
            records::import::{ImportSource, ProviderImportKind},
        },
        ProjectionStore, ProjectionStoreError, RestoreSource,
    },
    secure_fs::PinnedDirectory,
};
use layrs_core::scm::{FileId, RepoPath, TreeEntryTarget, WorktreeId};
use layrs_vfs::Cancellation;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{self, Write},
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

const TARGET: &[u8] = b"ordinary owner captured A";
const PREIMAGE: &[u8] = b"ordinary owner restore-only B";
const CURRENT: &[u8] = b"ordinary owner unrecorded C";
const LEAF_NONCE: &str = "00000000000000000000000000000071";
const TRANSACTION: &str = "00000000000000000000000000000072";
// Windows DELETE access and ordinary read/write/delete sharing. Requesting
// DELETE alone observes the pin's sharing contract without deleting the file.
const DELETE_ACCESS: u32 = 0x0001_0000;
const SHARE_ALL: u32 = 0x0000_0007;

type Tree = BTreeMap<PathBuf, super::managed_ready_reopen_tests::Node>;

fn trees(root: &Path, family: &Path) -> (Tree, Tree) {
    (
        super::managed_ready_reopen_tests::tree(root),
        super::managed_ready_reopen_tests::tree(family),
    )
}

#[derive(Default)]
struct RecordingSink {
    writes: usize,
    bytes: Vec<u8>,
}
impl Write for RecordingSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.writes += 1;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct FailingSink {
    bytes: Vec<u8>,
}
impl Write for FailingSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.bytes.is_empty() && !bytes.is_empty() {
            self.bytes.push(bytes[0]);
            return Ok(1);
        }
        Err(io::Error::other("owned ordinary export sink refusal"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
#[ignore = "ROOT source release and audited owned native child900/outer960 required"]
fn managed_ordinary_export_owner_retains_current_and_restore_only_bytes() {
    child::run(
        concat!(
            module_path!(),
            "::managed_ordinary_export_owner_retains_current_and_restore_only_bytes"
        ),
        900,
        || {
            child::declare();
            let root = super::managed_restore_recovery_tests::admitted_destination();
            let store = ProjectionStore::open(&root).unwrap();
            let state = store.state().unwrap();
            let path = RepoPath::parse("README.md").unwrap();
            let checkout_file = root.join(path.as_str());
            let base_id = match state
                .snapshot
                .entries
                .iter()
                .find(|entry| entry.path == path)
                .unwrap()
                .target
                .as_ref()
                .unwrap()
            {
                TreeEntryTarget::File { file_id } => file_id.clone(),
                _ => panic!("genuine fixture requires a regular README base"),
            };
            // The genuine incoming provider had no captures. Its destination
            // Ready capture is an empty diff, so this fresh admitted source has
            // a private base representation but no historical content mapping.
            {
                let ready_capture = store.latest_captured_patch().unwrap().unwrap();
                assert_eq!(ready_capture.sequence, 1);
                assert!(ready_capture.changes.is_empty());
                let initial_bytes = fs::read(&checkout_file).unwrap();
                assert_eq!(FileId::from_bytes(&initial_bytes), base_id);
                let family = store.marker.shared.as_ref().unwrap().root.clone();
                let initial_witness = trees(&root, &family);
                let mut destination = store.identity().clone();
                destination.worktree_id =
                    WorktreeId::parse("worktree:ordinary-owner-fresh-destination").unwrap();
                let cancel = Cancellation::default();
                let mut owner = store
                    .begin_managed_export(state.generation, destination.clone(), &cancel)
                    .unwrap();
                let current = owner.current_manifest(&path).unwrap().clone();
                let base = owner
                    .retained_manifest(&base_id)
                    .expect("selected private base is retained without historical membership")
                    .clone();
                assert_eq!(current.file_id(), &base_id);
                assert_eq!(base.file_id(), &base_id);
                let mut bytes = Vec::new();
                owner
                    .copy_retained_file(&base, &mut bytes, &cancel)
                    .unwrap();
                assert_eq!(bytes, initial_bytes);
                let parent = root.parent().unwrap();
                let parent_pin = PinnedDirectory::open(parent).unwrap();
                let derived = owner
                    .derive_plan(
                        parent.to_str().unwrap(),
                        &parent_pin.identity_token(),
                        LEAF_NONCE,
                        TRANSACTION,
                        &cancel,
                    )
                    .unwrap();
                parent_pin.verify_path_identity().unwrap();
                let empty_history =
                    content_mapping_rowset_data(&BTreeMap::new(), Some(&cancel)).unwrap();
                assert_eq!(
                    derived.plan.source_prefix.closure.content_mappings,
                    empty_history
                );
                assert!(derived.capture.changes.is_empty());
                assert_eq!(
                    derived.capture.previous_patch_digest.as_deref(),
                    Some(ready_capture.digest.as_str())
                );
                assert_eq!(derived.capture.identity, destination);
                let required: BTreeSet<_> = derived
                    .required_manifests
                    .iter()
                    .map(|row| (row.file_id.clone(), row.manifest_id.clone(), row.bytes))
                    .collect();
                let expected: BTreeSet<_> = [&base, &current]
                    .into_iter()
                    .map(|manifest| {
                        (
                            manifest.file_id().clone(),
                            manifest.id().clone(),
                            manifest.byte_length(),
                        )
                    })
                    .collect();
                assert_eq!(
                    required, expected,
                    "fresh base protection is independent of historical closure"
                );
                owner.revalidate(&cancel).unwrap();
                owner.close().unwrap();
                assert_eq!(
                    store.latest_captured_patch().unwrap().unwrap(),
                    ready_capture
                );
                assert_eq!(
                    trees(&root, &family),
                    initial_witness,
                    "fresh base transfer and plan DATA preserve source/family witnesses"
                );
            }
            fs::write(&checkout_file, TARGET).unwrap();
            let target = store.capture_patch(state.generation).unwrap();
            fs::write(&checkout_file, PREIMAGE).unwrap();
            let preview = store
                .preview_restore(
                    state.generation,
                    &RestoreSource::Capture {
                        sequence: target.sequence,
                        paths: vec![path.clone()],
                    },
                )
                .unwrap();
            let preimage = preview.before[&path].as_ref().unwrap().clone();
            assert_eq!(preimage.file_id, FileId::from_bytes(PREIMAGE));
            let restored = store.apply_restore(&preview).unwrap();
            assert_eq!(fs::read(&checkout_file).unwrap(), TARGET);
            for sequence in 1..=restored.capture.sequence {
                assert!(store
                    .captured_patch(sequence)
                    .unwrap()
                    .changes
                    .iter()
                    .filter_map(|change| change.replacement.as_ref())
                    .all(|file| file.file_id != preimage.file_id));
            }
            drop(store);
            // Ordinary reopen settles the committed restore reservation and
            // budget before any complete byte witness is attributed to export.
            let store = ProjectionStore::open(&root).unwrap();
            assert_eq!(store.state().unwrap(), state);
            fs::write(&checkout_file, CURRENT).unwrap();
            assert_eq!(
                store.latest_captured_patch().unwrap().unwrap(),
                restored.capture
            );
            let family = store.marker.shared.as_ref().unwrap().root.clone();
            let mut destination = store.identity().clone();
            destination.worktree_id =
                WorktreeId::parse("worktree:ordinary-owner-destination").unwrap();
            let cancel = Cancellation::default();
            let before = trees(&root, &family);

            // One extra acquisition on this same fixture proves that an
            // existing writable handle prevents immutable image ownership.
            let writable = OpenOptions::new()
                .write(true)
                .share_mode(SHARE_ALL)
                .open(&checkout_file)
                .unwrap();
            let refusal =
                match store.begin_managed_export(state.generation, destination.clone(), &cancel) {
                    Err(error) => error,
                    Ok(owner) => {
                        owner.close().unwrap();
                        panic!("ordinary export accepted an existing writable checkout handle")
                    }
                };
            assert!(
                matches!(&refusal, ProjectionStoreError::Io(error) if error.raw_os_error() == Some(32)),
                "expected immutable-open sharing refusal, got {refusal}"
            );
            drop(writable);
            assert_eq!(trees(&root, &family), before);

            let mut owner = store
                .begin_managed_export(state.generation, destination.clone(), &cancel)
                .unwrap();
            assert_eq!(owner.source_generation(), state.generation);
            assert_eq!(owner.image_digest().len(), 64);
            let current = owner.current_manifest(&path).unwrap().clone();
            assert_eq!(current.file_id(), &FileId::from_bytes(CURRENT));
            assert_eq!(current.byte_length(), CURRENT.len() as u64);
            let historical = owner.retained_manifest(&preimage.file_id).unwrap().clone();
            let base = owner.retained_manifest(&base_id).unwrap().clone();
            let captured = owner
                .retained_manifest(&FileId::from_bytes(TARGET))
                .unwrap()
                .clone();
            assert!(owner.retained_manifest(current.file_id()).is_none());

            // Do not call the old tree witness while the exclusive owner lock
            // and immutable pins are held: that witness requests DELETE access.
            let write_error = OpenOptions::new()
                .write(true)
                .share_mode(SHARE_ALL)
                .open(&checkout_file)
                .unwrap_err();
            assert_eq!(write_error.raw_os_error(), Some(32));
            let delete_error = OpenOptions::new()
                .read(true)
                .access_mode(DELETE_ACCESS)
                .share_mode(SHARE_ALL)
                .open(&checkout_file)
                .unwrap_err();
            assert_eq!(delete_error.raw_os_error(), Some(32));

            let mut bytes = Vec::new();
            owner
                .copy_current_file(&path, &current, &mut bytes, &cancel)
                .unwrap();
            assert_eq!(bytes, CURRENT);
            bytes.clear();
            assert_eq!(current.chunks().len(), 1);
            owner
                .copy_current_chunk(&path, &current, 0, &mut bytes, &cancel)
                .unwrap();
            assert_eq!(bytes, CURRENT);
            bytes.clear();
            owner
                .copy_retained_file(&historical, &mut bytes, &cancel)
                .unwrap();
            assert_eq!(bytes, PREIMAGE);
            bytes.clear();
            assert_eq!(historical.chunks().len(), 1);
            owner
                .copy_retained_chunk(&historical, 0, &mut bytes, &cancel)
                .unwrap();
            assert_eq!(bytes, PREIMAGE);

            let parent = root.parent().unwrap();
            let parent_pin = PinnedDirectory::open(parent).unwrap();
            let derived = owner
                .derive_plan(
                    parent.to_str().unwrap(),
                    &parent_pin.identity_token(),
                    LEAF_NONCE,
                    TRANSACTION,
                    &cancel,
                )
                .unwrap();
            parent_pin.verify_path_identity().unwrap();
            assert_eq!(derived.plan.kind, ProviderImportKind::ManagedOrdinary);
            assert_eq!(derived.plan.source_prefix.destination_identity, destination);
            assert_eq!(derived.capture.identity, destination);
            assert_eq!(derived.capture.changes.len(), 1);
            assert_eq!(derived.capture.changes[0].path, path);
            assert_eq!(
                derived.capture.changes[0]
                    .expected
                    .as_ref()
                    .unwrap()
                    .file_id,
                base_id
            );
            assert_eq!(
                derived.capture.changes[0]
                    .replacement
                    .as_ref()
                    .unwrap()
                    .file_id,
                *current.file_id()
            );
            assert_eq!(derived.plan.working_overlay.count, 1);
            assert!(derived.validated_metadata_floor.is_none());
            assert!(matches!(
                &derived.plan.source_prefix.source,
                ImportSource::Ordinary {
                    private_schema: 9,
                    admission: Some(_),
                    ..
                }
            ));
            let required: BTreeSet<_> = derived
                .required_manifests
                .iter()
                .map(|row| (row.file_id.clone(), row.manifest_id.clone(), row.bytes))
                .collect();
            let expected: BTreeSet<_> = [&base, &captured, &historical, &current]
                .into_iter()
                .map(|manifest| {
                    (
                        manifest.file_id().clone(),
                        manifest.id().clone(),
                        manifest.byte_length(),
                    )
                })
                .collect();
            assert_eq!(
                required, expected,
                "plan preserves exact base, capture, restore and current representations"
            );

            let mut untouched = RecordingSink::default();
            assert!(owner
                .copy_current_file(&path, &historical, &mut untouched, &cancel)
                .is_err());
            assert!(owner
                .copy_current_file(
                    &RepoPath::parse("not-in-retained-image.md").unwrap(),
                    &current,
                    &mut untouched,
                    &cancel,
                )
                .is_err());
            assert!(owner
                .copy_current_chunk(
                    &path,
                    &current,
                    current.chunks().len(),
                    &mut untouched,
                    &cancel
                )
                .is_err());
            // This current-only manifest has never joined historical authority.
            assert!(owner
                .copy_retained_file(&current, &mut untouched, &cancel)
                .is_err());
            assert!(owner
                .copy_retained_chunk(
                    &historical,
                    historical.chunks().len(),
                    &mut untouched,
                    &cancel
                )
                .is_err());
            let cancelled = Cancellation::default();
            cancelled.cancel();
            assert!(owner
                .copy_current_file(&path, &current, &mut untouched, &cancelled)
                .is_err());
            assert!(owner
                .copy_current_chunk(&path, &current, 0, &mut untouched, &cancelled)
                .is_err());
            assert!(owner
                .copy_retained_file(&historical, &mut untouched, &cancelled)
                .is_err());
            assert!(owner
                .copy_retained_chunk(&historical, 0, &mut untouched, &cancelled)
                .is_err());
            assert_eq!(untouched.writes, 0, "all refusals precede sink writes");
            assert!(untouched.bytes.is_empty());

            let mut failing = FailingSink::default();
            let failure = owner
                .copy_current_file(&path, &current, &mut failing, &cancel)
                .unwrap_err();
            assert!(failure
                .to_string()
                .contains("owned ordinary export sink refusal"));
            assert_eq!(
                failing.bytes,
                CURRENT[..1],
                "failed output is only an unpublished prefix"
            );
            owner.revalidate(&cancel).unwrap();
            // Cancel inside real retained-content verification, after its first
            // verified chunk. No timing, extra fixture or pre-cancel shortcut.
            let during_verification = Cancellation::default();
            {
                let observer =
                    crate::projection::provider::managed_export::verification_cancellation::arm(
                        &during_verification,
                    );
                let failure = owner.revalidate(&during_verification).unwrap_err();
                assert_eq!(observer.fired(), 1);
                assert!(matches!(
                    during_verification.check(),
                    Err(layrs_vfs::Error::Cancelled)
                ));
                assert!(
                    matches!(
                        &failure,
                        ProjectionStoreError::Io(error)
                            if matches!(
                                error.get_ref().and_then(|inner| inner.downcast_ref::<layrs_vfs::Error>()),
                                Some(layrs_vfs::Error::Cancelled)
                            )
                    ),
                    "retained verification preserves typed cancellation: {failure:?}"
                );
            }
            owner.close().unwrap();
            assert_eq!(trees(&root, &family), before, "failed acquisition, reads, DATA derivation and refusals preserve complete source/family bytes");
            drop(store);

            let reopened = ProjectionStore::open(&root).unwrap();
            assert_eq!(reopened.state().unwrap(), state);
            assert_eq!(
                reopened.latest_captured_patch().unwrap().unwrap(),
                restored.capture
            );
            assert_eq!(fs::read(&checkout_file).unwrap(), CURRENT);
            assert_eq!(trees(&root, &family), before);
            let mut next_destination = reopened.identity().clone();
            next_destination.worktree_id =
                WorktreeId::parse("worktree:ordinary-owner-after-cancel").unwrap();
            let next_owner = reopened
                .begin_managed_export(state.generation, next_destination, &Cancellation::default())
                .unwrap();
            next_owner.close().unwrap();
            assert_eq!(
                trees(&root, &family),
                before,
                "cancelled verification releases ownership and preserves source/family bytes"
            );
            // These ordinary writes happen after the no-effect byte comparison.
            // They prove explicit close released both owner and immutable pins.
            fs::write(&checkout_file, CURRENT).unwrap();
            let ordinary = reopened.capture_patch(state.generation).unwrap();
            assert_eq!(ordinary.sequence, restored.capture.sequence + 1);
            assert_eq!(
                ordinary
                    .changes
                    .iter()
                    .find(|change| change.path == path)
                    .unwrap()
                    .replacement
                    .as_ref()
                    .unwrap()
                    .file_id,
                FileId::from_bytes(CURRENT)
            );
        },
    );
}

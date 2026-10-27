//! Native end-to-end inspection after every original holder is gone. Only the
//! owned destination PathBuf escapes setup; CFAPI disconnect completes first.
//! No ordinary admission, ACK transfer, cleanup or process-crash claim here.
use super::*;
use crate::projection::provider::{
    database_io::{self, WriterCloseObserver, WriterOpenPoint},
    managed_ready_reopen::ManagedReadyInspection,
    process_profile::tests::child,
    records::{
        transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
        Record,
    },
};
use crate::secure_fs::PinnedDirectory;
use rusqlite::ffi;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Default)]
struct Close {
    opens: usize,
    closes: usize,
}
impl WriterCloseObserver for Close {
    fn open_point(&mut self, point: WriterOpenPoint, _: *mut ffi::sqlite3) -> CauseResult<()> {
        if point == WriterOpenPoint::BeforeRaw {
            self.opens += 1;
        }
        Ok(())
    }
    fn before_close(&mut self, _: *mut ffi::sqlite3) -> Option<Cause> {
        None
    }
    fn after_close(&mut self, succeeded: bool) -> Option<Cause> {
        assert!(succeeded, "setup requires positive writer closes");
        self.closes += 1;
        None
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Node {
    Directory(String),
    File((String, u64, String)),
}
fn tree(root: &Path) -> BTreeMap<PathBuf, Node> {
    let mut pending = vec![root.to_owned()];
    let mut result = BTreeMap::new();
    let mut bytes = 0u64;
    while let Some(path) = pending.pop() {
        assert!(result.len() < 128, "tiny native fixture inventory bound");
        let metadata = fs::symlink_metadata(&path).unwrap();
        assert!(!metadata.file_type().is_symlink());
        if metadata.is_dir() {
            let pin = PinnedDirectory::open(&path).unwrap();
            let identity = pin.identity_token();
            let mut children = Vec::new();
            for child in fs::read_dir(&path).unwrap() {
                assert!(children.len() < 64);
                children.push(child.unwrap().path());
            }
            pin.verify_path_identity().unwrap();
            result.insert(path, Node::Directory(identity));
            pending.extend(children);
        } else {
            assert!(metadata.is_file());
            assert!(metadata.len() <= 2 * 1024 * 1024, "fixture file bound");
            bytes = bytes.checked_add(metadata.len()).unwrap();
            assert!(bytes <= 16 * 1024 * 1024, "fixture aggregate bound");
            result.insert(
                path.clone(),
                Node::File(database_io::tests::support::witness(&path)),
            );
        }
    }
    result
}

#[test]
#[ignore = "UNQUALIFIED standalone physical Ready inspection; native owned child240 / ROOT outer300 and exact source release required"]
fn managed_ready_reopens_after_all_holders_and_source_disconnect_without_effects() {
    child::run(
        concat!(
            module_path!(),
            "::managed_ready_reopens_after_all_holders_and_source_disconnect_without_effects"
        ),
        240,
        || {
            child::declare();
            let mut destination_root = None;
            super::managed_staging_tests::with_content_planned_bytes(
                b"provider base content",
                |_, session, attempt, planned| {
                    let cancel = Cancellation::default();
                    let (mut destination, copying) =
                        super::managed_allocation_tests::bound_holder(session, attempt, planned);
                    destination
                        .publish_format(
                            &session.source.root,
                            &session.source.record,
                            &session.source.activation,
                            attempt,
                            &copying,
                            &cancel,
                        )
                        .unwrap();
                    let held = &session.source.held;
                    let mut setup = Close::default();
                    super::super::managed_destination::register_and_copy(
                        held,
                        &session.source.root,
                        &session.source.record,
                        &session.source.activation,
                        attempt,
                        &copying,
                        &destination,
                        &mut setup,
                        &cancel,
                    )
                    .unwrap();
                    let active = {
                        let _locks = held
                            .family
                            .locks(
                                &held.artifacts.allocation.format.identity.worktree_id,
                                false,
                            )
                            .unwrap();
                        capture_state::read(held, Some(&session.source.root), Some(&cancel))
                            .unwrap_or_else(|error| panic!("active read: {}", error.cause))
                    };
                    let fresh = active.active.source.initial().clone();
                    let mut checkpoint = session.session.export_checkpoint().unwrap();
                    let mut image = super::super::managed_image::plan(
                        &mut checkpoint,
                        session.source.as_ref(),
                        &active.active.source,
                        &active.active.activated.snapshot,
                        &cancel,
                    )
                    .unwrap();
                    assert_eq!(image.files().len(), 1);
                    let current = image.files()[0].manifest.manifest().id().clone();
                    let required =
                        super::managed_staging_tests::manifests_for(&active.active.source, &image);
                    assert_eq!(required.len(), 2);
                    assert_eq!(required[0].file_id(), required[1].file_id());
                    assert_ne!(required[0].id(), required[1].id());
                    let mut staging =
                        ProviderTransitionIntentRecord::decode(&copying.encode().unwrap()).unwrap();
                    staging.state = TransitionState::Staging;
                    staging.reservation = Some(staging.reservation_operation().unwrap());
                    super::super::managed_staging::stage(
                        held,
                        &session.source.root,
                        &session.source.record,
                        &session.source.activation,
                        attempt,
                        &copying,
                        &staging,
                        &destination,
                        &mut image,
                        &required,
                        &mut setup,
                        &cancel,
                    )
                    .unwrap();
                    let mut lease = super::super::managed_content::ManagedImportLease::open(
                        held,
                        &session.source.root,
                        &session.source.record,
                        &session.source.activation,
                        attempt,
                        &destination,
                        &mut image,
                        &staging,
                        &required,
                        &fresh,
                        &cancel,
                    )
                    .unwrap();
                    lease.reserve_plan(&mut setup, &cancel).unwrap();
                    let mut installed = BTreeSet::new();
                    for id in std::iter::once(&current)
                        .chain(required.iter().map(|manifest| manifest.id()))
                    {
                        if installed.insert(id.clone()) {
                            lease.install_exact(id, &mut setup, &cancel).unwrap();
                        }
                    }
                    let protected = lease.protect_exact(&mut setup, &cancel).unwrap();
                    let checkout = lease
                        .copy_checkout(&protected, &mut setup, &cancel)
                        .unwrap();
                    let private = lease
                        .finalize_private_ready(&protected, &checkout, &mut setup, &cancel)
                        .unwrap();
                    let family = lease
                        .publish_family_ready(&private, &mut setup, &cancel)
                        .unwrap();
                    let published = lease.publish_source(&family, &mut setup, &cancel).unwrap();
                    assert_eq!(
                        published.source_published().state,
                        TransitionState::Published
                    );
                    assert_eq!(setup.opens, setup.closes);
                    destination_root = Some(destination.root().to_owned());
                    // No DATA proof, allocation, lease, image, checkpoint, registration
                    // or source holder escapes this callback or its enclosing helper.
                },
            );
            let destination_root = destination_root.expect("setup returned only owned root path");
            let marker = crate::projection::format::read_marker(&destination_root).unwrap();
            let family_root = marker.shared.as_ref().unwrap().root.clone();
            let before = (tree(&destination_root), tree(&family_root));
            let cancel = Cancellation::default();
            let inspection = ManagedReadyInspection::open(&destination_root, &cancel).unwrap();
            inspection.require(&cancel).unwrap();
            assert_eq!(inspection.root(), destination_root);
            assert_eq!(inspection.ready().role, TransitionRole::Destination);
            assert_eq!(inspection.ready().state, TransitionState::Ready);
            assert_eq!(inspection.receipt().destination_identity, marker.identity);
            assert_eq!(inspection.required().len(), 2);
            assert_eq!(
                inspection.required()[0].file_id(),
                inspection.required()[1].file_id()
            );
            assert_ne!(inspection.required()[0].id(), inspection.required()[1].id());
            assert!(inspection.journal().is_complete());
            assert_eq!(inspection.expected().files().len(), 1);
            let receipt = inspection.receipt().encode().unwrap();
            let rows = inspection
                .journal()
                .rows()
                .iter()
                .map(|row| row.encode().unwrap())
                .collect::<Vec<_>>();
            assert!(
                fs::OpenOptions::new()
                    .write(true)
                    .open(destination_root.join("README.md"))
                    .is_err(),
                "fresh inspection retains immutable working file pins"
            );
            assert_eq!((tree(&destination_root), tree(&family_root)), before);
            drop(inspection);
            let inspection = ManagedReadyInspection::open(&destination_root, &cancel).unwrap();
            inspection.require(&cancel).unwrap();
            assert_eq!(inspection.receipt().encode().unwrap(), receipt);
            assert_eq!(
                inspection
                    .journal()
                    .rows()
                    .iter()
                    .map(|row| row.encode().unwrap())
                    .collect::<Vec<_>>(),
                rows
            );
            assert_eq!((tree(&destination_root), tree(&family_root)), before);
            drop(inspection);
            let cancelled = Cancellation::default();
            cancelled.cancel();
            assert!(ManagedReadyInspection::open(&destination_root, &cancelled).is_err());
            assert_eq!((tree(&destination_root), tree(&family_root)), before);
        },
    );
}

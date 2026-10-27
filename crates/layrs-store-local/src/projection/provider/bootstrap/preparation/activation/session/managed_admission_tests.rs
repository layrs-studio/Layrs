//! Native ACK refusal after a real admission COMMIT, followed by destination-
//! only resolution after source disconnect and release of every setup holder.
//! No ordinary open/edit, process crash, source deletion or cleanup is tested.
use super::*;
use crate::{
    lock::StoreLock,
    projection::{
        format,
        provider::{
            admitted_ordinary::{self, AdmittedOrdinaryData},
            database_io::{self, DatabaseFile, ReadState, WriterCloseObserver, WriterOpenPoint},
            managed_ready_reopen::{AdmissionStep, ManagedReadyAdmission},
            process_profile::tests::child,
            records::transition::{TransitionRole, TransitionState},
        },
        shared::{admitted_catalog::AdmittedFamilyContext, provider_access::FamilyAccess},
        SessionMode,
    },
};
use rusqlite::ffi;
use std::path::Path;

fn witnesses(
    destination: &Path,
    family: &Path,
) -> (
    std::collections::BTreeMap<std::path::PathBuf, super::managed_ready_reopen_tests::Node>,
    std::collections::BTreeMap<std::path::PathBuf, super::managed_ready_reopen_tests::Node>,
) {
    let marker = format::read_marker(destination).unwrap();
    let owner = marker
        .shared
        .as_ref()
        .unwrap()
        .metadata_path
        .join("layrs.lock");
    (
        super::managed_ready_reopen_tests::tree_with_owner_lock(destination, &owner),
        super::managed_ready_reopen_tests::tree_with_owner_lock(family, &owner),
    )
}

#[derive(Default)]
struct Close {
    opens: usize,
    closes: usize,
    refuse_acknowledgement: bool,
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
        assert!(
            succeeded,
            "admission recipe requires successful physical close"
        );
        self.closes += 1;
        self.refuse_acknowledgement
            .then(|| invalid("owned admission acknowledgement withheld after positive close"))
    }
}

fn valid<T>(state: ReadState<T>) -> T {
    match state {
        ReadState::Valid(value) => value,
        ReadState::Rejected { cause, .. }
        | ReadState::ReadButUnavailable(cause)
        | ReadState::Unavailable(cause) => panic!("owned admission RO read unavailable: {cause}"),
        ReadState::UnboundRollback { trigger, cause } => {
            panic!("owned admission unexpected unbound rollback: {trigger}; {cause}")
        }
        ReadState::Hot(_) => panic!("owned admission unexpectedly has a hot journal"),
        ReadState::Uninitialized => panic!("owned admission database unexpectedly empty"),
    }
}

/// Caller retains the genuine destination owner/lifecycle: either the opaque
/// ManagedReadyAdmission failure, or explicit fresh shared scopes below. The
/// helper acquires no owner recursively and opens no ordinary store or source.
fn inspect_under_owner(root: &Path, cancel: &Cancellation) -> AdmittedOrdinaryData {
    let marker = format::read_marker(root).unwrap();
    let shared = marker.shared.as_ref().unwrap();
    let family = FamilyAccess::open(&shared.root).unwrap();
    let _lifecycle = family.lifecycle_lease().unwrap();
    let private = DatabaseFile::open(
        shared.metadata_path.join("projection.sqlite3"),
        &shared.database_identity,
    )
    .unwrap();
    let read = || {
        valid(
            database_io::read_cancellable(&private, cancel, |c| {
                admitted_ordinary::inspect(c, &marker, Some(cancel))
            })
            .unwrap(),
        )
    };
    let data = read(); // private snapshot and positive close precede catalog gate
    let context = AdmittedFamilyContext::new(&data).unwrap();
    {
        let _catalog = StoreLock::acquire_existing(
            &family.root().join("metadata/catalog.lock"),
            SessionMode::Shared,
        )
        .unwrap();
        let manifests = valid(
            database_io::read_cancellable(&family.catalog, cancel, |c| {
                let proof = context.validate_catalog(c, family.marker_bytes(), Some(cancel))?;
                Ok(proof
                    .manifests()
                    .iter()
                    .map(|m| (m.file_id().to_string(), m.id().to_string(), m.byte_length()))
                    .collect::<Vec<_>>())
            })
            .unwrap(),
        );
        assert_eq!(manifests.len(), 2);
        assert_eq!(manifests[0].0, manifests[1].0);
        assert_ne!(manifests[0].1, manifests[1].1);
        let mut expected: Vec<_> = data
            .required()
            .iter()
            .map(|m| (m.file_id.to_string(), m.manifest_id.to_string(), m.bytes))
            .collect();
        expected.sort();
        let mut manifests = manifests;
        manifests.sort();
        assert_eq!(manifests, expected);
    }
    data.require_same_import(&read()).unwrap();
    private.verify().unwrap();
    family.verify().unwrap();
    data
}

fn version_under_owner(root: &Path, cancel: &Cancellation) -> u32 {
    let marker = format::read_marker(root).unwrap();
    let shared = marker.shared.as_ref().unwrap();
    let private = DatabaseFile::open(
        shared.metadata_path.join("projection.sqlite3"),
        &shared.database_identity,
    )
    .unwrap();
    let version = valid(
        database_io::read_cancellable(&private, cancel, |c| {
            Ok(c.query_row("PRAGMA user_version", [], |r| r.get(0))?)
        })
        .unwrap(),
    );
    private.verify().unwrap();
    version
}

#[test]
#[ignore = "UNQUALIFIED physical managed admission ACK refusal/source-disconnected resolve; ROOT exact source review and owned child240 / outer300 required"]
fn managed_admission_commit_ack_refusal_resolves_after_source_disconnect_without_writer() {
    child::run(
        concat!(
            module_path!(),
            "::managed_admission_commit_ack_refusal_resolves_after_source_disconnect_without_writer"
        ),
        240,
        || {
            child::declare();
            let mut retained = None;
            let mut close = Close::default();
            super::managed_staging_tests::with_content_planned_bytes_then(
                b"provider base content",
                |_, session, attempt, planned| {
                    super::managed_ready_reopen_tests::publish_destination(
                        session, attempt, planned,
                    )
                },
                |_, session, destination| {
                    let cancel = Cancellation::default();
                    let marker = format::read_marker(&destination).unwrap();
                    let family_root = marker.shared.as_ref().unwrap().root.clone();
                    let held = ManagedReadyAdmission::open(&destination, &cancel).unwrap();
                    assert_eq!(version_under_owner(&destination, &cancel), 8);
                    let before_cancel = witnesses(&destination, &family_root);
                    let cancelled = Cancellation::default();
                    cancelled.cancel();
                    let withheld = session
                        .admit_ready_destination(held, &mut close, &cancelled)
                        .expect_err("pre-admission cancellation must refuse");
                    assert_eq!(withheld.step, AdmissionStep::Validating);
                    assert_eq!((close.opens, close.closes), (0, 0));
                    assert_eq!(version_under_owner(&destination, &cancel), 8);
                    assert_eq!(witnesses(&destination, &family_root), before_cancel);
                    close.refuse_acknowledgement = true;
                    let failure = session
                        .admit_ready_destination(withheld.held, &mut close, &cancel)
                        .expect_err("after-close observer must withhold admission ACK");
                    assert_eq!(failure.step, AdmissionStep::CommitReturned);
                    assert!(failure
                        .cause
                        .to_string()
                        .contains("acknowledgement withheld"));
                    assert_eq!((close.opens, close.closes), (1, 1));
                    assert_eq!(version_under_owner(&destination, &cancel), 9);
                    let data = inspect_under_owner(&destination, &cancel);
                    assert_eq!(data.required().len(), 2);
                    assert_eq!(data.required()[0].file_id, data.required()[1].file_id);
                    assert_ne!(
                        data.required()[0].manifest_id,
                        data.required()[1].manifest_id
                    );
                    assert_eq!(data.ready().role, TransitionRole::Destination);
                    assert_eq!(data.ready().state, TransitionState::Ready);
                    assert_eq!(data.source_published().role, TransitionRole::Source);
                    assert_eq!(data.source_published().state, TransitionState::Published);
                    let after_commit = witnesses(&destination, &family_root);
                    retained = Some((failure.held, destination, family_root, data, after_commit));
                },
            );
            // This line is after helper return, explicit Session::disconnect and
            // drop of session/source/allocation/checkpoint/old transfer holders.
            // Only independent destination pins from the failure remain live.
            let (held, destination, family_root, committed, after_commit) =
                retained.expect("failure must retain independent destination authority");
            let cancel = Cancellation::default();
            let digest = held.resolve(&cancel).unwrap_or_else(|failure| {
                panic!(
                    "source-free resolution {:?}: {}",
                    failure.step, failure.cause
                )
            });
            assert_eq!(digest, committed.admission_digest());
            assert_eq!((close.opens, close.closes), (1, 1));
            let marker = format::read_marker(&destination).unwrap();
            let shared = marker.shared.as_ref().unwrap();
            let family = FamilyAccess::open(&shared.root).unwrap();
            let _lifecycle = family.lifecycle_lease().unwrap();
            let _owner = StoreLock::acquire_existing(
                &shared.metadata_path.join("layrs.lock"),
                SessionMode::Shared,
            )
            .unwrap();
            committed
                .require_same_import(&inspect_under_owner(&destination, &cancel))
                .unwrap();
            assert_eq!(witnesses(&destination, &family_root), after_commit);
        },
    );
}

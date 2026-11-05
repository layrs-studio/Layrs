//! C13 negative native integration, not a companion conversion success recipe.
//! Uses the existing allocation/copier/Staging boundary and a real checkpoint.
//! No online authority is fabricated; the legacy guard must keep C unready.
use super::*;
use crate::projection::provider::{
    database_io::{self, WriterCloseObserver, WriterOpenPoint},
    process_profile::tests::child,
    records::{
        Record,
        transition::{ProviderTransitionIntentRecord, TransitionState},
    },
};
use layrs_core::scm::{Chunk, ChunkingDescriptor, FileManifest, RepoPath};
use rusqlite::{Connection, ffi};
use std::fs;

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
    fn after_close(&mut self, success: bool) -> Option<Cause> {
        assert!(
            success,
            "requires positively observed SQLite connection close"
        );
        self.closes += 1;
        None
    }
}

#[test]
#[ignore = "UNQUALIFIED C13 negative native integration; ROOT must compile/review/release this exact recipe"]
fn c13_unknown_creation_preserved_complete_reader_and_legacy_staging_stays_unready() {
    child::run(
        concat!(
            module_path!(),
            "::c13_unknown_creation_preserved_complete_reader_and_legacy_staging_stays_unready"
        ),
        240,
        || {
            child::declare();
            super::managed_staging_tests::with_content_planned(
                true,
                false,
                |fixture, session, attempt, planned| {
                    let cancel = Cancellation::default();
                    let held = &session.source.held;
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
                            .unwrap_or_else(|e| panic!("active read: {}", e.cause))
                    };
                    let mut checkpoint = session.session.export_checkpoint().unwrap();
                    let mut legacy = super::super::managed_image::plan(
                        &mut checkpoint,
                        session.source.as_ref(),
                        &active.active.source,
                        &active.active.activated.snapshot,
                        &cancel,
                    )
                    .unwrap();
                    let manifests =
                        super::managed_staging_tests::manifests_for(&active.active.source, &legacy);
                    let path = fixture.checkout.join("c13-unknown.txt");
                    let bytes = b"C13 unknown creation must remain outside ordinary checkout";
                    fs::write(&path, bytes).unwrap();
                    let unknown_before = database_io::tests::support::witness(&path);
                    let source_file = fixture.metadata.join("projection.sqlite3");
                    let family_file = fixture.family.root().join("metadata/catalog.sqlite3");
                    let before = [
                        source_file.as_path(),
                        destination.destination_database().path(),
                        family_file.as_path(),
                    ]
                    .map(|p| database_io::tests::support::witness(p));
                    let mut staging =
                        ProviderTransitionIntentRecord::decode(&copying.encode().unwrap()).unwrap();
                    staging.state = TransitionState::Staging;
                    staging.reservation = Some(staging.reservation_operation().unwrap());
                    let mut refused = Close::default();
                    assert!(
                        super::super::managed_staging::stage(
                            held,
                            &session.source.root,
                            &session.source.record,
                            &session.source.activation,
                            attempt,
                            &copying,
                            &staging,
                            &destination,
                            &mut legacy,
                            &manifests,
                            &mut refused,
                            &cancel
                        )
                        .is_err()
                    );
                    assert_eq!(
                        (refused.opens, refused.closes),
                        (0, 0),
                        "namespace/preservation refusal precedes a writer"
                    );
                    drop(legacy);
                    drop(checkpoint);
                    let mut checkpoint = session.session.export_checkpoint().unwrap();
                    let mut complete = super::super::managed_image::complete::plan(
                        &mut checkpoint,
                        session.source.as_ref(),
                        &active.active.source,
                        &active.active.activated.snapshot,
                        &cancel,
                    )
                    .unwrap();
                    let manifest = FileManifest::from_chunks(
                        ChunkingDescriptor::deterministic_fixed(1024 * 1024).unwrap(),
                        &[Chunk::new(bytes.to_vec()).unwrap()],
                    )
                    .unwrap();
                    let logical = RepoPath::parse("c13-unknown.txt").unwrap();
                    assert_eq!(complete.current_path(&manifest), Some(&logical));
                    let mut copied = Vec::new();
                    complete
                        .copy_exact_file(&logical, &manifest, &mut copied, &cancel)
                        .unwrap();
                    assert_eq!(copied, bytes);
                    complete.revalidate(&cancel).unwrap();
                    drop(complete);
                    drop(checkpoint);
                    assert_eq!(
                        [
                            source_file.as_path(),
                            destination.destination_database().path(),
                            family_file.as_path()
                        ]
                        .map(|p| database_io::tests::support::witness(p)),
                        before
                    );
                    assert_eq!(database_io::tests::support::witness(&path), unknown_before);
                    assert_eq!(fs::read(&path).unwrap(), bytes);
                    super::super::managed_destination::valid(database_io::read_cancellable(destination.destination_database(), &cancel, |c: &Connection| {
                let state: String = c.query_row("SELECT state FROM projection_provider_transitions", [], |r| r.get(0))?;
                assert_eq!(state, "copying");
                assert!(!c.query_row("SELECT EXISTS(SELECT 1 FROM projection_provider_transitions WHERE state IN ('ready','published'))", [], |r| r.get::<_, bool>(0))?);
                assert!(!c.query_row("SELECT EXISTS(SELECT 1 FROM projection_provider_transition_receipts WHERE transaction_id=?1)", [copying.transaction_id.as_str()], |r| r.get::<_, bool>(0))?);
                Ok(())
            }).unwrap()).unwrap();
                },
            );
        },
    );
}

//! Actual C14 publication with authorized checkout and no companion header/s.
//! Local observation remains fixture DATA, never a continuous server lease.
use super::super::super::{
    managed_allocation::{self, ManagedDestinationAllocation},
    managed_content::{self, DenialPins},
    managed_destination,
    managed_image::complete::VerifiedCompleteManagedImage,
    managed_journal::denial_start::PendingDenialStart,
};
use super::*;
use crate::projection::{
    provider::{
        bootstrap::outcome::PublicationPins,
        denied_coverage_storage, destination_ready,
        destination_sql::CopiedStagingEffect,
        managed_ready_reopen::{ManagedReadyAdmission, ManagedReadyInspection},
        records::{
            import::{ImportSource, ProviderTransitionReceiptRecord},
            transition::{TransitionRole, TransitionState},
        },
        transition_reconcile::source_sql,
    },
    shared::{
        source_companion::SourceCompanionFamilyContext,
        source_denial::SourceDeniedCoverageReservation,
    },
};
use std::path::PathBuf;

pub(super) struct Completed {
    pub(super) root: PathBuf,
    pub(super) receipt: ProviderTransitionReceiptRecord,
    database: PathBuf,
    database_witness: (String, u64, String),
}

#[derive(Debug, PartialEq)]
struct Snapshot {
    source_file: (String, u64, String),
    destination_file: (String, u64, String),
    family_file: (String, u64, String),
    format_file: (String, u64, String),
    source: CatalogSnapshot,
    destination: CatalogSnapshot,
    family: CatalogSnapshot,
}

#[derive(Clone, Copy, Debug)]
enum AckPhase {
    PrivateReady,
    FamilyReady,
    SourceReady,
    SourcePublished,
}

/// Recognize only committed rows in the writer that production actually opened.
/// The borrowed rusqlite wrapper neither owns nor closes that connection.
struct RetryClose {
    normal: Close,
    phase: AckPhase,
    transaction: String,
    destination_owner: String,
    armed: bool,
    refused: Rc<Cell<bool>>,
}
impl WriterCloseObserver for RetryClose {
    fn open_point(
        &mut self,
        point: WriterOpenPoint,
        raw: *mut rusqlite::ffi::sqlite3,
    ) -> CauseResult<()> {
        assert!(
            !self.refused.get(),
            "plain denial opened another writer after withheld ACK"
        );
        self.normal.open_point(point, raw)
    }

    fn before_close(&mut self, raw: *mut rusqlite::ffi::sqlite3) -> Option<Cause> {
        self.armed = false;
        let Ok(c) = (unsafe { rusqlite::Connection::from_handle(raw) }) else {
            return None;
        };
        if !c.is_autocommit() {
            return None;
        }
        let table = match self.phase {
            AckPhase::FamilyReady => "worktrees",
            _ => "projection_provider_transitions",
        };
        let exists: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
                [table],
                |row| row.get(0),
            )
            .unwrap_or(false);
        self.armed = exists
            && match self.phase {
                AckPhase::FamilyReady => c
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM worktrees WHERE id=?1 AND state='ready')",
                        [&self.destination_owner],
                        |row| row.get(0),
                    )
                    .unwrap_or(false),
                phase => c
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM projection_provider_transitions WHERE id=?1 AND role=?2 AND state=?3)",
                        [
                            self.transaction.as_str(),
                            if matches!(phase, AckPhase::PrivateReady) { "destination" } else { "source" },
                            if matches!(phase, AckPhase::SourcePublished) { "published" } else { "ready" },
                        ],
                        |row| row.get(0),
                    )
                    .unwrap_or(false),
            };
        None
    }

    fn after_close(&mut self, success: bool) -> Option<Cause> {
        // Require the real SQLite close first. This fault only withholds its
        // acknowledgement; it supplies no OS-close or power-loss evidence.
        self.normal.after_close(success);
        if self.armed {
            self.armed = false;
            self.refused.set(true);
            Some(invalid("C14 plain durable phase SQLite close ACK withheld"))
        } else {
            None
        }
    }
}

fn require_ack_gap(
    session: &ProviderActiveSession,
    destination: &ManagedDestinationAllocation<'_>,
    staging: &ProviderTransitionIntentRecord,
    coverage: &DerivedDeniedCoverage,
    phase: AckPhase,
    cancel: &Cancellation,
) {
    let held = &session.source.held;
    let _locks = held
        .family
        .locks(
            &held.artifacts.allocation.format.identity.worktree_id,
            false,
        )
        .unwrap();
    for (file, role, state) in [
        (
            &held.artifacts.database,
            TransitionRole::Source,
            match phase {
                AckPhase::PrivateReady | AckPhase::FamilyReady => TransitionState::Staging,
                AckPhase::SourceReady => TransitionState::Ready,
                AckPhase::SourcePublished => TransitionState::Published,
            },
        ),
        (
            destination.destination_database(),
            TransitionRole::Destination,
            TransitionState::Ready,
        ),
    ] {
        database_io::read_cancellable_strict(file, cancel, |c| {
            let payload: Vec<u8> = c.query_row(
                "SELECT payload FROM projection_provider_transitions WHERE id=?1",
                [staging.transaction_id.as_str()],
                |row| row.get(0),
            )?;
            let actual = ProviderTransitionIntentRecord::decode(&payload)?;
            assert_eq!(actual.role, role);
            assert_eq!(actual.state, state);
            assert_eq!(actual.plan, staging.plan);
            require_no_companion_rows(c)?;
            let body = denied_coverage_storage::read_required(
                c,
                actual.plan.denial_coverage.as_ref().unwrap(),
                Some(cancel),
            )?;
            assert_eq!(&body, coverage.record());
            body.require_intent(&actual, Some(cancel))?;
            if state != TransitionState::Staging {
                let receipt: Vec<u8> = c.query_row(
                    "SELECT payload FROM projection_provider_transition_receipts WHERE transaction_id=?1",
                    [staging.transaction_id.as_str()],
                    |row| row.get(0),
                )?;
                let receipt = ProviderTransitionReceiptRecord::decode(&receipt)?;
                actual.require_receipt(&receipt)?;
                assert!(receipt.companion_id.is_none());
                assert_eq!(receipt.denial_coverage, staging.plan.denial_coverage);
            }
            Ok(())
        })
        .and_then(managed_destination::valid)
        .unwrap();
    }
    database_io::read_cancellable_strict(&held.family.catalog, cancel, |c| {
        let state: String = c.query_row(
            "SELECT state FROM worktrees WHERE id=?1",
            [destination.proof().format.identity.worktree_id.as_str()],
            |row| row.get(0),
        )?;
        assert_eq!(
            state,
            if matches!(phase, AckPhase::PrivateReady) { "initializing" } else { "ready" },
        );
        let s: u64 = c.query_row(
            "SELECT (SELECT count(*) FROM reservations WHERE owner=?1 AND operation LIKE 'provider-companion:%')+(SELECT count(*) FROM roots WHERE owner=?1 AND operation LIKE 'provider-companion:%')",
            [held.artifacts.allocation.format.identity.worktree_id.as_str()],
            |row| row.get(0),
        )?;
        assert_eq!(s, 0, "plain ACK retry created source companion s");
        Ok(())
    })
    .and_then(managed_destination::valid)
    .unwrap();
}
impl Snapshot {
    fn read(
        f: &Fixture,
        held: &PublicationPins,
        destination: &ManagedDestinationAllocation<'_>,
        cancel: &Cancellation,
    ) -> Self {
        let read = |file: &database_io::DatabaseFile| {
            database_io::read_cancellable_strict(file, cancel, table_snapshot)
                .and_then(managed_destination::valid)
                .unwrap()
        };
        Self {
            source_file: database_io::tests::support::witness(held.artifacts.database.path()),
            destination_file: database_io::tests::support::witness(
                destination.destination_database().path(),
            ),
            family_file: database_io::tests::support::witness(held.family.catalog.path()),
            format_file: database_io::tests::support::witness(
                &destination.root().join(".layrs/FORMAT"),
            ),
            source: read(&held.artifacts.database),
            destination: read(destination.destination_database()),
            family: catalog_snapshot(f, cancel),
        }
    }
}

fn require_no_companion_rows(c: &rusqlite::Connection) -> CauseResult<()> {
    for table in [
        "projection_provider_companions",
        "projection_provider_companion_files",
        "projection_provider_companion_deletions",
    ] {
        let count: u64 = c.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })?;
        assert_eq!(count, 0, "plain denial created {table} rows");
    }
    let claims: u64 = c.query_row(
        "SELECT count(*) FROM projection_claims WHERE kind='provider-companion'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(claims, 0, "plain denial created a source companion claim");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn require_terminal(
    session: &ProviderActiveSession,
    destination: &ManagedDestinationAllocation<'_>,
    staging: &ProviderTransitionIntentRecord,
    coverage: &DerivedDeniedCoverage,
    receipt: &ProviderTransitionReceiptRecord,
    cancel: &Cancellation,
) {
    let held = &session.source.held;
    let a = &held.artifacts.allocation;
    let _locks = held
        .family
        .locks(&a.format.identity.worktree_id, false)
        .unwrap();
    database_io::read_cancellable_strict(&held.artifacts.database, cancel, |source| {
        let full = validation::private_active_publication_with_source(
            source, a, held.artifacts.prepared.as_ref().unwrap(), &held.binding,
            Some(session.source.root.registration().root_id()), Some(cancel),
        )?;
        let actual = full.transitions.iter().find(|row| row.transaction_id == staging.transaction_id)
            .ok_or_else(|| invalid("plain C14 actual Published source absent"))?;
        assert_eq!(actual.role, TransitionRole::Source);
        assert_eq!(actual.state, TransitionState::Published);
        assert_eq!(actual.plan, staging.plan);
        assert!(actual.plan.companion_id.is_none());
        actual.require_receipt(receipt)?;
        require_no_companion_rows(source)?;
        let source_receipt: Vec<u8> = source.query_row(
            "SELECT payload FROM projection_provider_transition_receipts WHERE transaction_id=?1",
            [staging.transaction_id.as_str()], |row| row.get(0),
        )?;
        assert_eq!(source_receipt, receipt.encode()?);
        let source_body = denied_coverage_storage::read_required(
            source, actual.plan.denial_coverage.as_ref().unwrap(), Some(cancel),
        )?;
        assert_eq!(&source_body, coverage.record());
        source_body.require_intent(actual, Some(cancel))?;
        let funding = SourceDeniedCoverageReservation::new(a, staging, &source_body, Some(cancel))?;
        assert_eq!(full.capture.denial_reservations(), std::slice::from_ref(&funding));
        database_io::read_cancellable_strict(&held.family.catalog, cancel, |family| {
            held.family.validate_source_companion_catalog(family, a,
                &SourceCompanionFamilyContext { publication: &full, pending: None }, Some(cancel))?;
            funding.require_retained(family, Some(cancel))?;
            let charged: (u64, u64) = family.query_row(
                "SELECT (SELECT count(*) FROM reservations WHERE owner=?1 AND operation LIKE 'provider-denial:%'),(SELECT coalesce(sum(remaining),0) FROM reservations WHERE owner=?1 AND operation LIKE 'provider-denial:%')",
                [a.format.identity.worktree_id.as_str()], |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            assert_eq!(charged, (1, funding.reservation_bytes()));
            let s: u64 = family.query_row(
                "SELECT (SELECT count(*) FROM reservations WHERE owner=?1 AND operation LIKE 'provider-companion:%')+(SELECT count(*) FROM roots WHERE owner=?1 AND operation LIKE 'provider-companion:%')",
                [a.format.identity.worktree_id.as_str()], |row| row.get(0),
            )?;
            assert_eq!(s, 0, "plain denial created source companion s");
            let (state, identity): (String, String) = family.query_row(
                "SELECT state,database_identity FROM worktrees WHERE id=?1",
                [destination.proof().format.identity.worktree_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            assert_eq!(state, "ready");
            assert_eq!(identity, destination.proof().private_database_identity);
            database_io::read_cancellable_strict(destination.destination_database(), cancel, |c| {
                let ready = destination_ready::read_managed_ready(c, &destination.proof().format, family, Some(cancel))?;
                assert_eq!(ready.receipt(), receipt);
                assert_eq!(ready.ready().role, TransitionRole::Destination);
                assert_eq!(ready.ready().state, TransitionState::Ready);
                assert_eq!(ready.ready().plan, staging.plan);
                assert!(ready.receipt().companion_id.is_none());
                require_no_companion_rows(c)?;
                let body = denied_coverage_storage::read_required(c,
                    ready.ready().plan.denial_coverage.as_ref().unwrap(), Some(cancel))?;
                assert_eq!(body, source_body);
                body.require_intent(ready.ready(), Some(cancel))?;
                let published = crate::projection::provider::published_handoff::read_published_for_ready(
                    source, crate::projection::provider::published_handoff::PublicationSource {
                        allocation: a, prepared: held.artifacts.prepared.as_ref().unwrap(), binding: &held.binding,
                        root_id: session.source.root.registration().root_id(),
                    }, ready.ready(), ready.receipt(), Some(cancel),
                )?;
                let changes = c.total_changes();
                let error = crate::projection::provider::managed_admission::admit_ready(
                    c, &destination.proof().format, family, source, &ready, &published, Some(cancel),
                ).unwrap_err();
                require_validation_message(error, "admitted denial coverage writer is not integrated");
                assert_eq!(c.total_changes(), changes);
                assert_eq!(c.query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))?, 8);
                Ok(())
            }).and_then(managed_destination::valid)
        }).and_then(managed_destination::valid)
    }).and_then(managed_destination::valid).unwrap();
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run(
    f: &Fixture,
    session: &ProviderActiveSession,
    attempt: &CaptureAttemptLock,
    planned: &ProviderTransitionIntentRecord,
    coverage: &DerivedDeniedCoverage,
    observed: &TransitionObservationData,
    image: &mut VerifiedCompleteManagedImage<'_, '_>,
    required: &[FileManifest],
    metadata: u64,
    ranges: &RangeWatch,
    retry: bool,
    cancel: &Cancellation,
) -> Completed {
    let held = &session.source.held;
    let a = &held.artifacts.allocation;
    let prepared = held.artifacts.prepared.as_ref().unwrap();
    assert!(planned.plan.denial_coverage.is_some());
    assert!(planned.plan.companion_id.is_none());
    assert_eq!(required.len(), 3);
    let observed_readme = observed
        .snapshot()
        .entries
        .iter()
        .find(|row| row.path.as_str() == "README.md")
        .unwrap();
    assert!(observed_readme.writable);
    assert_ne!(observed_readme.state, EntryState::Reserved);
    let anchor = session.transition_source(cancel).unwrap();
    let parent = session
        .session
        .guard_export_parent(std::path::Path::new(&planned.plan.destination_parent))
        .unwrap();
    let mut pending = PendingDenialStart::prepare(
        held,
        parent,
        &session.source.root,
        &session.source.record,
        &session.source.activation,
        attempt,
        planned,
        coverage,
        observed,
        image,
        required,
        cancel,
    )
    .unwrap();
    assert!(pending.prospective_companion_cost().unwrap().is_none());
    let mut family_close = Close {
        funded: Some(ranges.funded.clone()),
        ..Close::default()
    };
    let mut source_close = Close::default();
    assert_eq!(
        pending
            .begin(&mut family_close, &mut source_close, cancel)
            .unwrap(),
        SourceIntentEffect::Inserted
    );
    assert_eq!((family_close.opened, family_close.closed), (1, 1));
    assert_eq!((source_close.opened, source_close.closed), (1, 1));
    let mut destination =
        managed_allocation::allocate_denial(&session.session, held, &mut pending, planned, cancel)
            .unwrap_or_else(|error| panic!("plain C14 allocation: {error:?}"));
    let mut close = Close::default();
    let (copying, effect) = pending
        .bind_denial_allocation(&destination, &mut close, cancel)
        .unwrap();
    assert_eq!(effect, SourceIntentEffect::Updated);
    assert_eq!((close.opened, close.closed), (1, 1));
    destination
        .publish_format_denial(&mut pending, &copying, cancel)
        .unwrap();
    destination.require_published(&copying.plan).unwrap();
    let before = (close.opened, close.closed);
    let copied = managed_destination::register_and_copy_denial(
        held,
        &mut pending,
        &copying,
        &destination,
        &mut close,
        cancel,
    )
    .unwrap();
    assert_eq!(
        copied.family,
        crate::projection::shared::provider_access::ImportRegistrationEffect::Inserted
    );
    assert_eq!(
        copied.destination,
        managed_destination::DestinationSqlState::FreshCopied
    );
    assert_eq!((close.opened - before.0, close.closed - before.1), (2, 2));
    require_denial_copy_state(
        held,
        &pending,
        &copying,
        &destination,
        coverage,
        true,
        cancel,
    );
    let before = (close.opened, close.closed);
    let (staging, effect) = pending
        .stage_denial(&destination, &mut close, cancel)
        .unwrap();
    assert_eq!(effect.source, SourceIntentEffect::Updated);
    assert_eq!(effect.destination, CopiedStagingEffect::Updated);
    assert_eq!((close.opened - before.0, close.closed - before.1), (2, 2));
    drop(pending);
    let rows = {
        let _locks = held
            .family
            .locks(&a.format.identity.worktree_id, false)
            .unwrap();
        database_io::read_cancellable_strict(&held.artifacts.database, cancel, |c| {
            let full = validation::private_active_publication_with_source(
                c,
                a,
                prepared,
                &held.binding,
                Some(session.source.root.registration().root_id()),
                Some(cancel),
            )?;
            anchor.require_transition(held, &session.source.root, &full, &staging, cancel)?;
            let ImportSource::Provider { version, .. } = &staging.plan.source_prefix.source else {
                panic!("plain C14 requires Provider source");
            };
            let closure = import_validation::verify_source_publication(
                c,
                &a.format,
                &staging.plan.source_prefix,
                ImportSourceReferences::Provider {
                    allocation: a,
                    prepared,
                    activation: &full.active.activation,
                },
                &full,
                &version.effective_image_digest,
                Some(cancel),
            )?;
            source_sql::with_source(
                c,
                &a.format,
                &staging.plan.source_prefix,
                &closure,
                &full,
                observed,
                Some(cancel),
                |source| {
                    image.reconcile_denial_publication_retained(
                        source, observed, &full, &staging, coverage, required, metadata, cancel,
                    )
                },
            )
        })
        .and_then(managed_destination::valid)
        .unwrap()
    };
    assert!(rows.header().is_none());
    assert_eq!(rows.manifests(), required);
    assert!(
        rows.authorized_files()
            .iter()
            .any(|row| row.path.as_str() == "README.md")
    );
    let derived = super::super::super::managed_plan::derive_reconciled_denial_data(
        &staging.plan.source_prefix,
        &rows,
        observed,
        staging.plan.source_prefix.destination_identity.clone(),
        &staging.plan.destination_parent,
        &staging.plan.destination_parent_identity,
        &staging.plan.destination_leaf_nonce,
        &staging.transaction_id,
        coverage,
    )
    .unwrap();
    assert_eq!(derived.plan, staging.plan);
    let mut pins = DenialPins::default();
    let requests = session.hydration_requests().len();
    if retry {
        let mut previous_gap: Option<Snapshot> = None;
        for phase in [
            AckPhase::PrivateReady,
            AckPhase::FamilyReady,
            AckPhase::SourceReady,
            AckPhase::SourcePublished,
        ] {
            eprintln!("C14_PLAIN_ACK_PHASE {phase:?}");
            let refused = Rc::new(Cell::new(false));
            let probes = Rc::new(Cell::new(0));
            let watch_refused = refused.clone();
            let watch_probes = probes.clone();
            let mut fault = RetryClose {
                normal: Close::default(),
                phase,
                transaction: staging.transaction_id.as_str().into(),
                destination_owner: destination
                    .proof()
                    .format
                    .identity
                    .worktree_id
                    .as_str()
                    .into(),
                armed: false,
                refused: refused.clone(),
            };
            // Keep exactly the original pins, image, source anchor, allocation
            // and attempt throughout all four real same-owner ACK gaps.
            let error = with_observer(
                move |_, _| {
                    if watch_refused.get() {
                        watch_probes.set(watch_probes.get() + 1);
                        return Err(invalid("forbidden DB probe after plain denial ACK"));
                    }
                    Ok(())
                },
                || {
                    managed_content::commit_denial(
                        held,
                        &session.source.root,
                        &session.source.record,
                        &session.source.activation,
                        attempt,
                        &destination,
                        image,
                        &staging,
                        &rows,
                        &anchor,
                        observed,
                        metadata,
                        coverage,
                        &mut pins,
                        &mut fault,
                        cancel,
                    )
                },
            )
            .err()
            .unwrap_or_else(|| panic!("{phase:?}: selected durable phase did not withhold ACK"));
            assert!(
                refused.get(),
                "{phase:?} failed before its selected ACK: {error}"
            );
            require_validation_message(error, "C14 plain durable phase SQLite close ACK withheld");
            assert_eq!(probes.get(), 0, "{phase:?} probed after withheld ACK");
            assert!(fault.normal.opened > 0);
            assert_eq!(fault.normal.opened, fault.normal.closed);
            if !matches!(phase, AckPhase::PrivateReady) {
                assert_eq!(
                    fault.normal.opened, 1,
                    "{phase:?} rewrote a durable predecessor"
                );
            }
            require_ack_gap(session, &destination, &staging, coverage, phase, cancel);
            let gap = Snapshot::read(f, held, &destination, cancel);
            if let Some(previous) = &previous_gap {
                assert_eq!(gap.destination_file, previous.destination_file);
                assert_eq!(gap.destination, previous.destination);
                assert_eq!(gap.format_file, previous.format_file);
                if matches!(phase, AckPhase::FamilyReady) {
                    assert_eq!(gap.source_file, previous.source_file);
                    assert_eq!(gap.source, previous.source);
                } else {
                    assert_eq!(gap.family_file, previous.family_file);
                    assert_eq!(gap.family, previous.family);
                }
            }
            previous_gap = Some(gap);
            assert_eq!(session.hydration_requests().len(), requests);
            assert_eq!(ranges.late.load(Ordering::SeqCst), 0);
        }
    }
    let published_gap = retry.then(|| Snapshot::read(f, held, &destination, cancel));
    let mut commit = Close::default();
    let receipt = managed_content::commit_denial(
        held,
        &session.source.root,
        &session.source.record,
        &session.source.activation,
        attempt,
        &destination,
        image,
        &staging,
        &rows,
        &anchor,
        observed,
        metadata,
        coverage,
        &mut pins,
        &mut commit,
        cancel,
    )
    .unwrap();
    if let Some(before) = published_gap {
        assert_eq!(
            (commit.opened, commit.closed),
            (0, 0),
            "Published ACK recovery opened a writer"
        );
        assert_eq!(Snapshot::read(f, held, &destination, cancel), before);
    } else {
        assert!(commit.opened > 0);
    }
    assert_eq!(commit.opened, commit.closed);
    assert_eq!(
        receipt.schema_version,
        "layrs.projection-provider-transition-receipt.v2"
    );
    assert_eq!(receipt.denial_coverage, staging.plan.denial_coverage);
    assert!(receipt.companion_id.is_none());
    assert_eq!(receipt.companion_files.count, 0);
    assert_eq!(receipt.companion_deletions.count, 0);
    require_terminal(session, &destination, &staging, coverage, &receipt, cancel);
    let before_replay = Snapshot::read(f, held, &destination, cancel);
    let mut replay = Close::default();
    let repeated = managed_content::commit_denial(
        held,
        &session.source.root,
        &session.source.record,
        &session.source.activation,
        attempt,
        &destination,
        image,
        &staging,
        &rows,
        &anchor,
        observed,
        metadata,
        coverage,
        &mut pins,
        &mut replay,
        cancel,
    )
    .unwrap();
    assert_eq!(repeated, receipt);
    assert_eq!((replay.opened, replay.closed), (0, 0));
    assert_eq!(Snapshot::read(f, held, &destination, cancel), before_replay);
    require_terminal(session, &destination, &staging, coverage, &receipt, cancel);
    assert_eq!(
        fs::read(destination.root().join("README.md")).unwrap(),
        b"C14 current native image"
    );
    assert_eq!(
        fs::read(destination.root().join("unchanged.txt")).unwrap(),
        b"C14 distinct immutable bytes"
    );
    assert_eq!(
        fs::read(f.checkout.join("README.md")).unwrap(),
        b"C14 current native image"
    );
    assert_eq!(session.hydration_requests().len(), requests);
    assert_eq!(ranges.late.load(Ordering::SeqCst), 0);
    let completed = Completed {
        root: destination.root().to_path_buf(),
        receipt,
        database: destination.destination_database().path().to_path_buf(),
        database_witness: database_io::tests::support::witness(
            destination.destination_database().path(),
        ),
    };
    let witness = || {
        (
            database_io::tests::support::witness(&completed.database),
            database_io::tests::support::witness(held.artifacts.database.path()),
            database_io::tests::support::witness(held.family.catalog.path()),
        )
    };
    let before_admission = witness();
    drop(pins);
    drop(destination);
    drop(anchor);
    let admission = ManagedReadyAdmission::open(&completed.root, cancel).unwrap();
    let mut admission_close = Close::default();
    let failure = session
        .admit_ready_destination(admission, &mut admission_close, cancel)
        .expect_err("plain C14 admission must still refuse before its first writer");
    assert_eq!(
        failure.step,
        crate::projection::provider::managed_ready_reopen::AdmissionStep::Validating
    );
    require_validation_message(
        failure.cause,
        "admitted denial coverage writer is not integrated",
    );
    assert_eq!((admission_close.opened, admission_close.closed), (0, 0));
    assert_eq!(witness(), before_admission);
    drop(failure.held);
    completed
}

pub(super) fn require_reopened(
    completed: &Completed,
    coverage: &DerivedDeniedCoverage,
    cancel: &Cancellation,
) {
    // Caller has dropped Pending, image/checkpoint, attempt, range source and
    // the connected Store session; this is a real physical Ready re-open.
    let ready = ManagedReadyInspection::open(&completed.root, cancel).unwrap();
    ready.require(cancel).unwrap();
    assert_eq!(ready.receipt(), &completed.receipt);
    assert!(ready.receipt().companion_id.is_none());
    assert_eq!(ready.ready().state, TransitionState::Ready);
    assert_eq!(
        ready.ready().plan.denial_coverage.as_ref(),
        Some(&coverage.record().reference().unwrap())
    );
    assert_eq!(
        database_io::tests::support::witness(&completed.database),
        completed.database_witness
    );
    assert_eq!(
        fs::read(completed.root.join("README.md")).unwrap(),
        b"C14 current native image"
    );
    assert_eq!(
        fs::read(completed.root.join("unchanged.txt")).unwrap(),
        b"C14 distinct immutable bytes"
    );
    drop(ready);
}

#[test]
#[ignore = "owned native C14 without companion; actual Ready/Published/replay/reopen, admission refused"]
fn c14_plain_denial_commits_ready_and_published_without_source_companion_then_reopens() {
    child::run(
        concat!(
            module_path!(),
            "::c14_plain_denial_commits_ready_and_published_without_source_companion_then_reopens"
        ),
        1800,
        || {
            child::declare();
            run_start_configuration(
                false,
                false,
                None,
                PreimageCase::DescriptorOnly,
                Some(AllocationCase::ContentPlain),
            );
        },
    );
}

#[test]
#[ignore = "UNQUALIFIED native C14 plain same-owner ACK matrix; ROOT serial child3600/outer3660 release required; no OS-close failure claim"]
fn c14_plain_denial_same_owner_retries_private_family_source_ready_and_published_acks() {
    child::run(
        concat!(
            module_path!(),
            "::c14_plain_denial_same_owner_retries_private_family_source_ready_and_published_acks"
        ),
        3600,
        || {
            child::declare();
            run_start_configuration(
                false,
                false,
                None,
                PreimageCase::DescriptorOnly,
                Some(AllocationCase::ContentPlainRetry),
            );
        },
    );
}

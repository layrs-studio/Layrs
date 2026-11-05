//! Proposed owned native recipes for the retained Ordinary C8 -> C10 driver.
//! Fresh fixtures only. ACK withholding below follows a successful physical
//! C writer close; it does not simulate a failing OS xClose. B/family close
//! failures, crashes, owner loss and general reopening remain unqualified.
use super::{
    managed_ordinary_family_ready_tests::text,
    managed_ordinary_installing_tests::{self as witness, Cell, Database},
    managed_ordinary_source_publication_tests::{
        AdmissionHooks, PublicationFixture, with_admission_hooks,
    },
};
use crate::projection::{
    ProjectionStore,
    provider::{
        database_io::{WriterCloseObserver, WriterOpenPoint},
        managed_admission::read_admitted_import,
        managed_export::{
            AdmissionStep, OrdinaryAdmissionFailure, OrdinaryManagedExport, OrdinaryReadyAdmission,
        },
        process_profile::tests::child,
        records::Record,
        recovery::error::{ProviderBootstrapCause as Cause, Result as CauseResult, invalid},
    },
};
use layrs_vfs::Cancellation;
use rusqlite::ffi;
use std::{
    collections::BTreeSet,
    fs::OpenOptions,
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

const WITHHELD_ACK: &str = "ordinary C10 successful writer close acknowledgement withheld";

#[derive(Clone)]
struct Pins {
    file: PathBuf,
    chunk: PathBuf,
}
impl Pins {
    fn from_fixture(f: &PublicationFixture<'_>) -> Self {
        let file = &f.expected.files()[0];
        let manifest = f
            .required
            .iter()
            .find(|m| m.id() == &file.manifest_id)
            .unwrap();
        Self {
            file: f.destination_root.join(file.path.as_str()),
            chunk: witness::object(
                f.family_root,
                "chunks",
                manifest.chunks()[0].chunk_id().as_str(),
            ),
        }
    }
    fn require(&self) {
        // Identity/pin exclusion only: these callbacks execute under the
        // catalog gate, so no byte hash or namespace scan is permitted here.
        for path in [&self.file, &self.chunk] {
            let error = OpenOptions::new()
                .write(true)
                .share_mode(7)
                .open(path)
                .unwrap_err();
            assert_eq!(
                error.raw_os_error(),
                Some(32),
                "retained immutable pin {path:?}"
            );
        }
    }
}

struct Close {
    opens: usize,
    raw: usize,
    closes: usize,
    withheld: usize,
    withhold: bool,
    pins: Pins,
}
impl Close {
    fn new(f: &PublicationFixture<'_>, withhold: bool) -> Self {
        Self {
            opens: 0,
            raw: 0,
            closes: 0,
            withheld: 0,
            withhold,
            pins: Pins::from_fixture(f),
        }
    }
    fn counts(&self) -> (usize, usize, usize, usize) {
        (self.opens, self.raw, self.closes, self.withheld)
    }
}
impl WriterCloseObserver for Close {
    fn open_point(&mut self, point: WriterOpenPoint, raw: *mut ffi::sqlite3) -> CauseResult<()> {
        match point {
            WriterOpenPoint::BeforeRaw => {
                assert!(raw.is_null());
                self.opens += 1;
                self.pins.require();
            }
            WriterOpenPoint::AfterRaw => {
                assert!(!raw.is_null());
                self.raw += 1;
            }
            _ => {}
        }
        Ok(())
    }
    fn before_close(&mut self, _: *mut ffi::sqlite3) -> Option<Cause> {
        self.pins.require();
        None
    }
    fn after_close(&mut self, succeeded: bool) -> Option<Cause> {
        assert!(
            succeeded,
            "recipe requires a real successful C writer close"
        );
        self.closes += 1;
        self.pins.require();
        if std::mem::take(&mut self.withhold) {
            self.withheld += 1;
            Some(invalid(WITHHELD_ACK))
        } else {
            None
        }
    }
}

fn snapshot(path: &Path) -> Database {
    witness::read(path, witness::database)
}

fn refuse_before_published(
    owner: &mut OrdinaryManagedExport<'_>,
    f: &PublicationFixture<'_>,
    cancel: &Cancellation,
) {
    let before_b = snapshot(f.source_database);
    let before_c = snapshot(f.destination_database);
    let before_family = snapshot(f.family_database);
    witness::read(f.source_database, |c| {
        let admitted = read_admitted_import(c, f.source_marker, None).unwrap();
        let intent = admitted
            .outgoing()
            .iter()
            .find(|row| row.transaction_id == f.ready.transaction_id)
            .unwrap();
        assert_eq!(
            intent.state,
            crate::projection::provider::records::transition::TransitionState::Ready
        );
    });
    let held =
        OrdinaryReadyAdmission::from_retained(f.destination, f.protection, f.checkout, cancel)
            .unwrap_or_else(|e| panic!("retained C8 Ready {e}"));
    let mut close = Close::new(f, false);
    let failure = owner
        .admit_ready_destination(held, &mut close, cancel)
        .err()
        .expect("actual B Ready cannot admit C before Published");
    assert_eq!(failure.step, AdmissionStep::Validating);
    assert_eq!(
        close.counts(),
        (0, 0, 0, 0),
        "refuse before C writable open"
    );
    assert_eq!(snapshot(f.source_database), before_b);
    assert_eq!(snapshot(f.destination_database), before_c);
    assert_eq!(snapshot(f.family_database), before_family);
    witness::read(f.destination_database, |c| {
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            8
        );
        assert!(read_admitted_import(c, &f.destination.proof().format, None).is_err());
    });
    close.pins.require();
    drop(failure.held);
}

fn published_body(f: &PublicationFixture<'_>) -> Vec<u8> {
    witness::read(f.source_database, |c| {
        let admitted = read_admitted_import(c, f.source_marker, None).unwrap();
        let published = admitted
            .outgoing()
            .iter()
            .find(|row| row.transaction_id == f.ready.transaction_id)
            .unwrap();
        assert_eq!(
            published.state,
            crate::projection::provider::records::transition::TransitionState::Published
        );
        published.encode().unwrap()
    })
}

fn require_c10(f: &PublicationFixture<'_>, before: &Database, actual_published: &[u8]) -> String {
    witness::read(f.destination_database, |c| {
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            10
        );
        let admitted = read_admitted_import(c, &f.destination.proof().format, None).unwrap();
        assert_eq!(admitted.private_schema(), 10);
        assert_eq!(admitted.ready(), f.ready);
        assert_eq!(admitted.receipt(), f.receipt);
        assert_eq!(
            admitted.source_published().encode().unwrap(),
            actual_published
        );
        assert_eq!(admitted.expected().header(), f.expected.header());
        assert_eq!(
            admitted.witness_column_bytes(),
            f.required.len() as u64 * 150
        );
        let mut triples: Vec<_> = f
            .required
            .iter()
            .map(|m| {
                vec![
                    text(m.file_id().as_str()),
                    text(m.id().as_str()),
                    Cell::Integer(i64::try_from(m.byte_length()).unwrap()),
                ]
            })
            .collect();
        triples.sort();
        let after = witness::database(c);
        assert_eq!(after["projection_import_representations"], triples);
        assert_eq!(
            after["projection_import_checkout"],
            before["projection_prejournal"]
        );
        assert!(after["projection_prejournal"].is_empty());
        let expected_tables: BTreeSet<_> = before
            .keys()
            .map(String::as_str)
            .chain([
                "projection_managed_admission",
                "projection_import_checkout",
                "projection_import_representations",
            ])
            .collect();
        assert_eq!(
            after.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            expected_tables
        );
        for (table, rows) in before {
            if table != "projection_prejournal" {
                assert_eq!(
                    &after[table], rows,
                    "unexpected C8 history mutation in {table}"
                );
            }
        }
        admitted.record().digest().unwrap()
    })
}

fn admit_and_close_owner(
    mut owner: OrdinaryManagedExport<'_>,
    f: &PublicationFixture<'_>,
    cancel: &Cancellation,
) {
    let before_b = snapshot(f.source_database);
    let before_c = snapshot(f.destination_database);
    let before_family = snapshot(f.family_database);
    let published = published_body(f);
    let held =
        OrdinaryReadyAdmission::from_retained(f.destination, f.protection, f.checkout, cancel)
            .unwrap_or_else(|e| panic!("retained C8 Ready {e}"));
    let mut close = Close::new(f, false);
    let progress = owner
        .admit_ready_destination(held, &mut close, cancel)
        .unwrap_or_else(|e| panic!("ordinary admission {:?}: {}", e.step, e.cause));
    assert_eq!(progress.step(), AdmissionStep::FreshVerified);
    assert_eq!(close.counts(), (1, 1, 1, 0));
    let digest = require_c10(f, &before_c, &published);
    assert_eq!(progress.admission_digest(), digest);
    assert_eq!(require_c10(f, &before_c, &published), digest);
    assert_eq!(snapshot(f.source_database), before_b);
    assert_eq!(snapshot(f.family_database), before_family);
    witness::verify_objects(f.family_root, f.required);
    close.pins.require();
    // Progress alone is not this separately acknowledged native owner close.
    owner.close().unwrap();
    let reopened = ProjectionStore::open(f.source_root).unwrap();
    drop(reopened);
    assert_eq!(snapshot(f.source_database), before_b);
    assert_eq!(require_c10(f, &before_c, &published), digest);
    close.pins.require();
}

fn withhold_c_ack_then_resolve_without_b(
    mut owner: OrdinaryManagedExport<'_>,
    f: &PublicationFixture<'_>,
    cancel: &Cancellation,
) {
    let before_b = snapshot(f.source_database);
    let before_c = snapshot(f.destination_database);
    let before_family = snapshot(f.family_database);
    let published = published_body(f);
    let held =
        OrdinaryReadyAdmission::from_retained(f.destination, f.protection, f.checkout, cancel)
            .unwrap_or_else(|e| panic!("retained C8 Ready {e}"));
    let mut close = Close::new(f, true);
    let OrdinaryAdmissionFailure { step, cause, held } = owner
        .admit_ready_destination(held, &mut close, cancel)
        .err()
        .expect("withheld acknowledgement must retain the admission holder");
    assert_eq!(step, AdmissionStep::CommitReturned);
    assert_eq!(close.counts(), (1, 1, 1, 1));
    assert!(cause.to_string().contains(WITHHELD_ACK));
    let primary = cause.to_string();
    close.pins.require();
    // After a failed enclosing close, original B cleanup is error-only: no
    // Access::run success validation or implicit retry/readback is permitted.
    let cleanup = owner.finish::<()>(Err(cause.into())).unwrap_err();
    assert_eq!(
        cleanup.to_string(),
        primary,
        "original B close must not add a cleanup failure"
    );
    // This is a new explicit caller action after B has closed. Resolution
    // borrows C/family pins and has neither a B opener nor a writer parameter.
    let progress = held
        .resolve(cancel)
        .unwrap_or_else(|e| panic!("held C10 resolution {:?}: {}", e.step, e.cause));
    assert_eq!(progress.step(), AdmissionStep::FreshVerified);
    assert_eq!(close.counts(), (1, 1, 1, 1));
    assert_eq!(
        require_c10(f, &before_c, &published),
        progress.admission_digest()
    );
    assert_eq!(snapshot(f.source_database), before_b);
    assert_eq!(snapshot(f.family_database), before_family);
    witness::verify_objects(f.family_root, f.required);
    close.pins.require();
}

#[test]
#[ignore = "ROOT source release and audited owned native child3600/outer3660 required; proposed unmeasured"]
fn managed_ordinary_admission_commits_exact_c10_after_published_and_positive_closes() {
    // Same finite proposal as the fresh B publication setup: four source calls
    // plus retained admission, exact private double read and owner close.
    child::run(
        concat!(
            module_path!(),
            "::managed_ordinary_admission_commits_exact_c10_after_published_and_positive_closes"
        ),
        3600,
        || {
            with_admission_hooks(AdmissionHooks {
                before_published: refuse_before_published,
                after_published: admit_and_close_owner,
            });
        },
    );
}

#[test]
#[ignore = "ROOT source release and audited owned native child3600/outer3660 required; proposed unmeasured"]
fn managed_ordinary_admission_withheld_c_ack_retains_holder_for_sourceless_resolution() {
    child::run(
        concat!(
            module_path!(),
            "::managed_ordinary_admission_withheld_c_ack_retains_holder_for_sourceless_resolution"
        ),
        3600,
        || {
            with_admission_hooks(AdmissionHooks {
                before_published: refuse_before_published,
                after_published: withhold_c_ack_then_resolve_without_b,
            });
        },
    );
}

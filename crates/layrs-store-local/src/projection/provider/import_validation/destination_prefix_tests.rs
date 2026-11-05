//! Audited memory-SQL DATA recipes. The live source and family connections
//! positively close before destination-only reads. No physical authority or
//! complete Ready suffix/admission is asserted by these recipes.
use super::*;
use crate::projection::capture::CapturedPatch;
use crate::projection::provider::{
    database_io::BareConnection,
    destination_ready::*,
    destination_sql,
    process_profile::tests::child,
    records::{
        ProviderBootstrapSource, Receipt, Record,
        activation::ProviderActivationRecord,
        import::{ImportSource, ProviderImportPrefixRecord, ProviderTransitionReceiptRecord},
        transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
    },
    validation::tests::fixture::Fixture as SourceFixture,
};
use layrs_core::{
    projection::ProjectionSnapshot,
    scm::{FileId, FileManifest, canonical_json},
};
use rusqlite::params;

// Only the fixture's canonical family marker DATA is needed. Its production
// interpreter remains private to destination_ready; no visibility is widened.
mod family {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    pub(super) struct FamilyMarker {
        pub schema_version: String,
        pub magic: String,
        pub format_version: u32,
        pub min_reader_version: u32,
        pub incarnation: String,
        pub identity: crate::projection::shared::PartialStoreIdentity,
        pub root_identity: String,
        pub metadata_identity: String,
        pub database_identity: String,
    }
}

#[path = "../destination_ready/tests/fixture.rs"]
mod fixture;
#[path = "../destination_ready/tests/source.rs"]
mod source;
use fixture::Fixture;

macro_rules! owned {
    ($name:ident, $body:block) => {
        #[test]
        #[ignore = "UNQUALIFIED owned destination-only historical prefix SQL recipe; ROOT reviews/releases child120"]
        fn $name() {
            child::run(concat!(module_path!(), "::", stringify!($name)), 120, || {
                child::declare();
                $body
            });
        }
    };
}

fn with_destination(body: impl FnOnce(&Connection, &Fixture)) {
    let source = BareConnection::reference().unwrap();
    let destination = BareConnection::reference().unwrap();
    let family = BareConnection::reference().unwrap();
    let (f, closure) = Fixture::new(source.connection());
    f.stage(source.connection(), destination.connection(), &closure);
    f.populate_family(family.connection());
    destination
        .connection()
        .execute_batch("BEGIN IMMEDIATE")
        .unwrap();
    let data = f.data();
    finalize_staged_or_replay(
        source.connection(),
        destination.connection(),
        family.connection(),
        &f.inputs(&closure, &data),
        None,
    )
    .unwrap();
    destination
        .connection()
        .execute_batch("COMMIT; BEGIN DEFERRED")
        .unwrap();
    // The new reader gets neither connection nor a forged live-source closure.
    drop(closure);
    source.connection().execute_batch("ROLLBACK").unwrap();
    family.connection().execute_batch("ROLLBACK").unwrap();
    source.finish(Ok(())).unwrap();
    family.finish(Ok(())).unwrap();
    body(destination.connection(), &f);
    if !destination.connection().is_autocommit() {
        destination.connection().execute_batch("ROLLBACK").unwrap();
    }
    destination.finish(Ok(())).unwrap();
}

owned!(
    destination_prefix_reads_without_source_after_ready_suffix,
    {
        with_destination(|c, f| {
            c.execute_batch("COMMIT; PRAGMA query_only=ON; BEGIN DEFERRED")
                .unwrap();
            let changes = c.total_changes();
            let prefix = read_destination_prefix(c, &f.marker, None).unwrap();
            assert_eq!(prefix.prefix(), &f.prefix);
            assert_eq!(prefix.receipt(), &f.receipt);
            assert_eq!(prefix.ready(), &f.ready);
            assert_eq!(prefix.activation().unwrap(), &f.activation);
            assert_eq!(
                prefix.source().unwrap().commitment(),
                f.source.source.commitment()
            );
            assert_eq!(prefix.source().unwrap().descriptors().len(), 1);
            let mappings = prefix.copied_content_mappings(c, None).unwrap();
            assert_eq!(mappings.len(), 1);
            assert_eq!(
                mappings.get(f.historical.file_id()),
                Some(&(f.historical.id().clone(), f.historical.byte_length()))
            );
            assert_ne!(f.historical.id(), f.current.id());
            assert_eq!(
                c.query_row("SELECT generation FROM projection_head", [], |r| r
                    .get::<_, u64>(0))
                    .unwrap(),
                3
            );
            assert_eq!(
                c.query_row("SELECT count(*) FROM projection_captures", [], |r| r
                    .get::<_, u64>(0))
                    .unwrap(),
                2
            );
            let version = match &prefix.prefix().source {
                ImportSource::Provider { version, .. } | ImportSource::Ordinary { version, .. } => {
                    version
                }
            };
            assert_eq!(version.generation, 1);
            assert_eq!(version.capture_count, 1);
            prefix.require_prefix_scope(c, None).unwrap();
            let cancel = Cancellation::default();
            cancel.cancel();
            assert!(read_destination_prefix(c, &f.marker, Some(&cancel)).is_err());
            assert_eq!(c.total_changes(), changes);
            // Even a borrow cannot keep a SQLite transaction alive by itself.
            c.execute_batch("COMMIT").unwrap();
            assert!(prefix.require_destination_connection(c).is_err());
            assert!(prefix.require_prefix_scope(c, None).is_err());
        });
    }
);

owned!(
    destination_prefix_refuses_mapping_entry_and_schema_substitution,
    {
        with_destination(|c, f| {
            c.execute_batch("SAVEPOINT probe").unwrap();
            c.execute(
                "UPDATE projection_content SET manifest_id=?1 WHERE file_id=?2",
                params![f.current.id().as_str(), f.historical.file_id().as_str()],
            )
            .unwrap();
            let changes = c.total_changes();
            assert!(read_destination_prefix(c, &f.marker, None).is_err());
            assert_eq!(c.total_changes(), changes);
            c.execute_batch("ROLLBACK TO probe; RELEASE probe").unwrap();
            let retained = read_destination_prefix(c, &f.marker, None).unwrap();
            // Deliberately defer the owned fixture's FK while constructing a
            // dangling receipt projection. Otherwise SQLite rejects the test
            // setup before the bounded reader can inspect the corruption.
            c.execute_batch("SAVEPOINT probe; PRAGMA defer_foreign_keys=ON")
                .unwrap();
            c.execute(
                "UPDATE projection_provider_transitions SET receipt_id=?1",
                [format!("blake3:{}", "f".repeat(64))],
            )
            .unwrap();
            let changes = c.total_changes();
            assert!(retained.require_prefix_scope(c, None).is_err());
            assert_eq!(c.total_changes(), changes);
            c.execute_batch("ROLLBACK TO probe; PRAGMA defer_foreign_keys=OFF; RELEASE probe")
                .unwrap();
            retained.require_prefix_scope(c, None).unwrap();
            c.execute_batch("SAVEPOINT probe; DROP TABLE projection_imports; CREATE VIEW projection_imports AS SELECT 1 AS singleton, 'forged' AS digest, X'' AS payload").unwrap();
            let changes = c.total_changes();
            let failure = match read_destination_prefix(c, &f.marker, None) {
                Ok(_) => panic!("schema substitution accepted"),
                Err(e) => e,
            };
            assert!(format!("{failure:?}").contains("import private schema fingerprint differs"));
            assert_eq!(c.total_changes(), changes);
            c.execute_batch("ROLLBACK TO probe; RELEASE probe").unwrap();
            retained.require_prefix_scope(c, None).unwrap();
            // An entry envelope alone is not a claim to validate the Ready suffix.
            c.execute("UPDATE projection_head SET generation=2", [])
                .unwrap();
            read_destination_prefix(c, &f.marker, None).unwrap();
        });
    }
);

/// Small tails exercise the same SQLite TEXT/BLOB discrepancy as oversized
/// allocations. Every refusal must come from the preflight, before semantics.
fn require_scalar_refusal(c: &Connection, f: &Fixture, table: &str) {
    let changes = c.total_changes();
    let error = match read_destination_prefix(c, &f.marker, None) {
        Ok(_) => panic!("NUL-tail accepted for {table}"),
        Err(error) => error,
    };
    assert!(
        format!("{error:?}").contains(&format!(
            "destination prefix historical scalar byte bound: {table}"
        )),
        "unexpected refusal: {error:?}"
    );
    assert_eq!(c.total_changes(), changes);
}

owned!(
    destination_prefix_bounds_sql_text_bytes_before_semantic_loaders,
    {
        with_destination(|c, f| {
            // These columns have SQL CHECK(length(TEXT)=64/71) or <=128. The
            // complete value is longer despite satisfying that schema constraint.
            for (table, column, width) in [
                ("projection_selections", "digest", 64),
                ("projection_provider_catalogs", "source_digest", 64),
                ("projection_provider_catalogs", "selection_digest", 64),
                ("projection_captures", "digest", 71),
                ("projection_provider_activations", "source_digest", 64),
                ("projection_provider_activations", "activated_digest", 64),
                (
                    "projection_provider_activations",
                    "owner_identity_digest",
                    71,
                ),
                ("projection_provider_activations", "mount_id", 128),
            ] {
                c.execute_batch("SAVEPOINT scalar_probe").unwrap();
                let value = format!("{}\0tail", "a".repeat(width));
                // Capture has two rows; constrain the update to the imported row.
                let predicate = if table == "projection_captures" {
                    " WHERE sequence=1"
                } else {
                    ""
                };
                c.execute(
                    &format!("UPDATE {table} SET {column}=?1{predicate}"),
                    [&value],
                )
                .unwrap();
                let (characters, bytes): (u64,u64) = c.query_row(&format!("SELECT length({column}),length(CAST({column} AS BLOB)) FROM {table}{predicate} LIMIT 1"), [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
                assert_eq!(characters, width as u64);
                assert!(bytes > width as u64);
                require_scalar_refusal(c, f, table);
                c.execute_batch("ROLLBACK TO scalar_probe; RELEASE scalar_probe")
                    .unwrap();
            }
            let token = format!("blake3:{}", "b".repeat(64));
            for column in ["receipt_digest", "projection_digest", "request_digest"] {
                c.execute_batch("SAVEPOINT scalar_probe").unwrap();
                c.execute(
                    "INSERT INTO projection_publications VALUES(1,?1,?1,X'',NULL,1,?2,X'')",
                    params![token, "a".repeat(64)],
                )
                .unwrap();
                let width = if column == "projection_digest" {
                    64
                } else {
                    71
                };
                c.execute(
                    &format!("UPDATE projection_publications SET {column}=?1"),
                    [format!("{}\0tail", "a".repeat(width))],
                )
                .unwrap();
                require_scalar_refusal(c, f, "projection_publications");
                c.execute_batch("ROLLBACK TO scalar_probe; RELEASE scalar_probe")
                    .unwrap();
            }
            for column in ["files_digest", "deletions_digest"] {
                c.execute_batch("SAVEPOINT scalar_probe").unwrap();
                c.execute(
                    "INSERT INTO projection_provider_companions VALUES(?1,1,1,?1,?1,X'')",
                    [&token],
                )
                .unwrap();
                c.execute(
                    &format!("UPDATE projection_provider_companions SET {column}=?1"),
                    [format!("{}\0tail", "a".repeat(71))],
                )
                .unwrap();
                require_scalar_refusal(c, f, "projection_provider_companions");
                c.execute_batch("ROLLBACK TO scalar_probe; RELEASE scalar_probe")
                    .unwrap();
            }
            for (table, columns) in [
                (
                    "projection_provider_companion_files",
                    &["base_file_id", "digest"][..],
                ),
                (
                    "projection_provider_companion_deletions",
                    &["base_manifest_id", "digest"][..],
                ),
            ] {
                for column in columns {
                    c.execute_batch("SAVEPOINT scalar_probe").unwrap();
                    c.execute(
                        "INSERT INTO projection_provider_companions VALUES(?1,1,1,?1,?1,X'')",
                        [&token],
                    )
                    .unwrap();
                    if table == "projection_provider_companion_files" {
                        c.execute("INSERT INTO projection_provider_companion_files VALUES(?1,'README.md',?1,1,0,?1,?1,0,'excludedPath',?1)", [&token]).unwrap();
                    } else {
                        c.execute("INSERT INTO projection_provider_companion_deletions VALUES(?1,'README.md',?1,0,?1,'excludedPath',?1)", [&token]).unwrap();
                    }
                    c.execute(
                        &format!("UPDATE {table} SET {column}=?1"),
                        [format!("{}\0tail", "a".repeat(71))],
                    )
                    .unwrap();
                    require_scalar_refusal(c, f, table);
                    c.execute_batch("ROLLBACK TO scalar_probe; RELEASE scalar_probe")
                        .unwrap();
                }
            }
            read_destination_prefix(c, &f.marker, None).unwrap();
        });
    }
);

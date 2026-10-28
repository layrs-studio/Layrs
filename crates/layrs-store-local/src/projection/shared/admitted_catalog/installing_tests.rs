//! Owned SQL DATA: complete retained Published groups coexist with one C
//! Installing group. Canonical C journals are declared test data, not evidence
//! of private admission, physical spools, CAS bytes or successful transfers.
use super::{export_tests, *};
use crate::projection::{
    provider::owned_test as child,
    shared::exact_content::{ExactInstallState, PREPARE_CHARGE},
};
use layrs_core::scm::{Chunk, ChunkingDescriptor};
use rusqlite::{types::ValueRef, OptionalExtension};
use serde_json::{json, Value};

macro_rules! owned {
    ($name:ident, $body:block) => {
        #[test]
        #[ignore = "ROOT source release and audited owned Installing SQL child120/outer180 required"]
        fn $name() {
            child::run(concat!(module_path!(), "::", stringify!($name)), 120, || {
                child::declare();
                $body
            });
        }
    };
}

const TABLES: [&str; 13] = [
    "family",
    "quota",
    "worktrees",
    "files",
    "manifests",
    "chunks",
    "manifest_chunks",
    "roots",
    "root_manifests",
    "reservations",
    "installations",
    "exact_installations",
    "sweep",
];
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Cell {
    Null,
    Integer(i64),
    Real(u64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}
pub(super) type Rows = Vec<Vec<Cell>>;
pub(super) type State = BTreeMap<&'static str, Rows>;
pub(super) fn state(c: &Connection) -> State {
    let mut result = BTreeMap::new();
    let mut bytes = 0usize;
    for table in TABLES {
        let mut q = c
            .prepare(&format!("SELECT * FROM \"{table}\" LIMIT 257"))
            .unwrap();
        let columns = q.column_count();
        assert!(columns <= 16);
        let mut query = q.query([]).unwrap();
        let mut rows = Vec::new();
        while let Some(row) = query.next().unwrap() {
            assert!(rows.len() < 256);
            rows.push(
                (0..columns)
                    .map(|n| match row.get_ref(n).unwrap() {
                        ValueRef::Null => Cell::Null,
                        ValueRef::Integer(v) => Cell::Integer(v),
                        ValueRef::Real(v) => Cell::Real(v.to_bits()),
                        ValueRef::Text(v) => {
                            bytes += v.len();
                            Cell::Text(v.to_vec())
                        }
                        ValueRef::Blob(v) => {
                            bytes += v.len();
                            Cell::Blob(v.to_vec())
                        }
                    })
                    .collect(),
            );
            assert!(bytes <= 16 * 1024 * 1024);
        }
        rows.sort();
        result.insert(table, rows);
    }
    result
}

fn inspect(
    c: &Connection,
    family: &[u8],
    retained: &AdmittedFamilyContext,
    target: &ExportReserveOnlyTarget,
    required: &[FileManifest],
) -> ExactInstallState {
    let before = state(c);
    let changes = c.total_changes();
    let (proof, actual) = retained
        .validate_export_installing(c, family, target, required, None)
        .unwrap();
    assert_eq!(
        proof
            .manifests()
            .iter()
            .map(|m| (m.file_id().to_string(), m.id().to_string(), m.byte_length()))
            .collect::<BTreeSet<_>>(),
        retained.primary.required
    );
    assert_eq!(actual.content_bytes, global_content(c));
    assert_eq!(
        actual.reserved_bytes,
        c.query_row(
            "SELECT coalesce(sum(remaining),0) FROM reservations",
            [],
            |r| r.get::<_, u64>(0)
        )
        .unwrap()
    );
    assert_eq!(
        actual.limit_bytes,
        c.query_row("SELECT bytes FROM quota WHERE singleton=1", [], |r| r
            .get::<_, u64>(0))
            .unwrap()
    );
    assert!(actual.content_bytes + actual.reserved_bytes <= actual.limit_bytes);
    assert_eq!(
        c.total_changes(),
        changes,
        "Installing reader must never repair"
    );
    assert_eq!(state(c), before);
    actual
}
fn refusal(
    c: &Connection,
    family: &[u8],
    retained: &AdmittedFamilyContext,
    target: &ExportReserveOnlyTarget,
    required: &[FileManifest],
) {
    let before = state(c);
    let changes = c.total_changes();
    assert!(retained
        .validate_export_installing(c, family, target, required, None)
        .is_err());
    assert_eq!(c.total_changes(), changes);
    assert_eq!(state(c), before);
}
fn global_content(c: &Connection) -> u64 {
    c.query_row("SELECT coalesce((SELECT sum(bytes) FROM chunks),0)+coalesce((SELECT sum(length(payload)) FROM manifests),0)+coalesce((SELECT sum(bytes) FROM sweep),0)+(SELECT count(*)*8192+coalesce(sum(length(manifest)),0) FROM exact_installations)", [], |r|r.get(0)).unwrap()
}
fn retained_rows_unchanged(before: &State, after: &State, target: &ExportReserveOnlyTarget) {
    let c_reservation = Cell::Text(target.binding().reservation.as_bytes().to_vec());
    for table in ["reservations", "exact_installations"] {
        assert_eq!(
            before[table]
                .iter()
                .filter(|row| row[0] != c_reservation)
                .collect::<Vec<_>>(),
            after[table]
                .iter()
                .filter(|row| row[0] != c_reservation)
                .collect::<Vec<_>>(),
            "Installing changed a retained group's {table}"
        );
    }
}
fn reserve(
    c: &Connection,
    family: &[u8],
    retained: &AdmittedFamilyContext,
    target: &ExportReserveOnlyTarget,
) {
    assert_eq!(
        retained
            .reserve_export_only(c, family, target, None)
            .unwrap(),
        ExportReservationEffect::Inserted
    );
}

pub(super) fn journal(
    binding: &ExactBinding,
    manifest: &FileManifest,
    phase: &str,
    charged: &[usize],
    charge_manifest: bool,
) -> Value {
    let verified = matches!(phase, "verified" | "publishing" | "published");
    let published = phase == "published";
    let payload = canonical_json(manifest).unwrap();
    let mut bits = vec![b'0'; 256];
    let mut debit = PREPARE_CHARGE + if verified { payload.len() as u64 } else { 0 };
    for &n in charged {
        let before = (bits[n / 4] as char).to_digit(16).unwrap() as usize;
        bits[n / 4] = b"0123456789abcdef"[before | (1 << (n % 4))];
        debit += manifest.chunks()[n].byte_length();
    }
    if charge_manifest {
        debit += payload.len() as u64;
    }
    json!({"schemaVersion":"layrs.shared-exact-installation.v1", "binding":binding,
        "manifestId":manifest.id(), "fileId":manifest.file_id(), "bytes":manifest.byte_length(),
        "directory":"5-6-7", "directoryIdentity":if phase == "declared" { None } else { Some("fs:v1:0000000000000001:0000000000000030") },
        "phase":phase, "spoolCursor":if verified {manifest.chunks().len()} else {0},
        "publishCursor":if published {manifest.chunks().len()} else {0}, "active":null,
        "chargedChunks":String::from_utf8(bits).unwrap(), "residentChunks":"0".repeat(256),
        "chargedManifest":charge_manifest, "debitBytes":debit})
}
pub(super) fn put(c: &Connection, binding: &ExactBinding, manifest: &FileManifest, record: &Value) {
    let previous: Option<Vec<u8>> = c
        .query_row(
            "SELECT payload FROM exact_installations WHERE reservation=?1 AND manifest_id=?2",
            params![binding.reservation, manifest.id().as_str()],
            |r| r.get(0),
        )
        .optional()
        .unwrap();
    let previous = previous.map_or(0, |p| {
        serde_json::from_slice::<Value>(&p).unwrap()["debitBytes"]
            .as_u64()
            .unwrap()
    });
    let debit = record["debitBytes"].as_u64().unwrap();
    let phase = record["phase"].as_str().unwrap();
    let stored = matches!(phase, "verified" | "publishing" | "published")
        .then(|| canonical_json(manifest).unwrap());
    c.execute("INSERT INTO exact_installations VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(reservation,manifest_id) DO UPDATE SET payload=excluded.payload,manifest=excluded.manifest,published=excluded.published",
        params![binding.reservation, manifest.id().as_str(), manifest.file_id().as_str(), manifest.byte_length(), canonical_json(record).unwrap(), stored, phase == "published"]).unwrap();
    c.execute(
        "UPDATE reservations SET remaining=remaining+?1-?2 WHERE id=?3",
        params![previous, debit, binding.reservation],
    )
    .unwrap();
}
pub(super) fn index(c: &Connection, manifest: &FileManifest) {
    c.execute(
        "INSERT INTO files VALUES(?1,?2) ON CONFLICT DO NOTHING",
        params![manifest.file_id().as_str(), manifest.byte_length()],
    )
    .unwrap();
    c.execute(
        "INSERT INTO manifests VALUES(?1,?2,?3,?4) ON CONFLICT DO NOTHING",
        params![
            manifest.id().as_str(),
            manifest.file_id().as_str(),
            manifest.byte_length(),
            canonical_json(manifest).unwrap()
        ],
    )
    .unwrap();
    for (n, chunk) in manifest.chunks().iter().enumerate() {
        c.execute(
            "INSERT INTO chunks VALUES(?1,?2) ON CONFLICT DO NOTHING",
            params![chunk.chunk_id().as_str(), chunk.byte_length()],
        )
        .unwrap();
        c.execute(
            "INSERT INTO manifest_chunks VALUES(?1,?2,?3) ON CONFLICT DO NOTHING",
            params![manifest.id().as_str(), n, chunk.chunk_id().as_str()],
        )
        .unwrap();
    }
}
fn new_manifest() -> FileManifest {
    FileManifest::from_chunks(
        ChunkingDescriptor::deterministic_fixed(3).unwrap(),
        &[
            Chunk::new(b"abc".to_vec()).unwrap(),
            Chunk::new(b"XYZ".to_vec()).unwrap(),
        ],
    )
    .unwrap()
}

owned!(
    export_installing_reuses_retained_objects_but_charges_each_c_journal_through_all_phases,
    {
        export_tests::fixture(|c, family, retained, target, required| {
            reserve(c, family, retained, target);
            let baseline = inspect(c, family, retained, target, required);
            assert_eq!(
                (baseline.records, baseline.debit_bytes, baseline.published),
                (0, 0, 0)
            );
            let before = state(c);
            let manifest = &required[0];
            for phase in [
                "declared",
                "allocated",
                "verified",
                "publishing",
                "published",
            ] {
                let record = journal(target.binding(), manifest, phase, &[], false);
                put(c, target.binding(), manifest, &record);
                let actual = inspect(c, family, retained, target, required);
                assert_eq!(actual.records, 1);
                assert_eq!(actual.published, u64::from(phase == "published"));
                assert_eq!(actual.debit_bytes, record["debitBytes"].as_u64().unwrap());
                assert_eq!(
                    actual.remaining + actual.debit_bytes,
                    target.binding().quota_bytes
                );
                assert_eq!(
                    actual.content_bytes - baseline.content_bytes,
                    actual.debit_bytes,
                    "shared immutable bytes cost zero again; C journal metadata is still charged"
                );
                assert_eq!(
                    baseline.reserved_bytes - actual.reserved_bytes,
                    actual.debit_bytes
                );
            }
            put(
                c,
                target.binding(),
                &required[1],
                &journal(target.binding(), &required[1], "published", &[], false),
            );
            let actual = inspect(c, family, retained, target, required);
            assert_eq!((actual.records, actual.published), (2, 2));
            let expected: u64 = required
                .iter()
                .map(|m| PREPARE_CHARGE + canonical_json(m).unwrap().len() as u64)
                .sum();
            assert_eq!(actual.debit_bytes, expected);
            let after = state(c);
            retained_rows_unchanged(&before, &after, target);
            for table in TABLES {
                if !matches!(table, "reservations" | "exact_installations") {
                    assert_eq!(after[table], before[table], "reuse changed {table}");
                }
            }
            assert!(retained.validate_catalog(c, family, None).is_err());
            assert!(retained
                .validate_export_reserve_only(c, family, target, None)
                .is_err());
        });
    }
);

owned!(
    export_installing_charges_only_the_new_chunk_and_new_manifest_across_retained_groups,
    {
        export_tests::fixture(|c, family, retained, _, required| {
            let fresh = new_manifest();
            assert_eq!(
                fresh.chunks()[0].chunk_id(),
                required[1].chunks()[0].chunk_id()
            );
            let mut complete = required.to_vec();
            complete.push(fresh.clone());
            let target = export_tests::target(retained, &complete);
            reserve(c, family, retained, &target);
            let baseline = inspect(c, family, retained, &target, &complete);
            let before = state(c);
            index(c, &fresh);
            put(
                c,
                target.binding(),
                &fresh,
                &journal(target.binding(), &fresh, "published", &[1], true),
            );
            let actual = inspect(c, family, retained, &target, &complete);
            let expected = PREPARE_CHARGE + 2 * canonical_json(&fresh).unwrap().len() as u64 + 3;
            assert_eq!(actual.debit_bytes, expected);
            assert_eq!(actual.content_bytes - baseline.content_bytes, expected);
            assert_eq!(baseline.reserved_bytes - actual.reserved_bytes, expected);
            assert_eq!((actual.records, actual.published), (1, 1));
            let after = state(c);
            retained_rows_unchanged(&before, &after, &target);
            assert_eq!(after["chunks"].len(), before["chunks"].len() + 1);
            assert_eq!(after["manifests"].len(), before["manifests"].len() + 1);
            for row in &before["exact_installations"] {
                assert!(after["exact_installations"].contains(row));
            }
            for table in [
                "family",
                "quota",
                "worktrees",
                "roots",
                "root_manifests",
                "installations",
                "sweep",
            ] {
                assert_eq!(after[table], before[table]);
            }
            for row in &before["reservations"] {
                if row[0] != Cell::Text(target.binding().reservation.as_bytes().to_vec()) {
                    assert!(after["reservations"].contains(row));
                }
            }
        });
    }
);

owned!(export_installing_refuses_double_physical_debits_outside_union_and_foreign_groups_without_repair, {
    export_tests::fixture(|c,family,retained,target,required| {
        reserve(c,family,retained,target);
        for mutation in ["chunk-debit","manifest-debit","outside-union","foreign-binding","foreign-group","c-other-reservation","retained-debit","retained-published","full-quota"] {
            c.execute_batch("SAVEPOINT corrupt_installing").unwrap();
            match mutation {
                "chunk-debit"|"manifest-debit" => {
                    let record=journal(target.binding(),&required[0],"published",if mutation=="chunk-debit" { &[0] } else { &[] },mutation=="manifest-debit");
                    put(c,target.binding(),&required[0],&record);
                }
                "outside-union" => {
                    let extra=new_manifest();
                    put(c,target.binding(),&extra,&journal(target.binding(),&extra,"declared",&[],false));
                }
                "foreign-binding" => {
                    let mut record=journal(target.binding(),&required[0],"declared",&[],false);
                    record["binding"]["planDigest"]=format!("blake3:{}","e".repeat(64)).into();
                    put(c,target.binding(),&required[0],&record);
                }
                "foreign-group" => {
                    let mut foreign=target.binding().clone();
                    foreign.operation=format!("provider-transition:{}","f".repeat(64));
                    foreign.reservation=foreign.operation.clone();
                    // This other owner's orphan group reaches the parent's
                    // complete classification, rather than C's owner preflight.
                    foreign.owner=retained.binding().owner.clone();
                    foreign.database_identity=retained.binding().database_identity.clone();
                    c.execute("INSERT INTO reservations VALUES(?1,?2,?1,?3)",params![foreign.reservation,foreign.owner.as_str(),foreign.quota_bytes]).unwrap();
                    put(c,&foreign,&required[0],&journal(&foreign,&required[0],"declared",&[],false));
                }
                "c-other-reservation" => {
                    c.execute("INSERT INTO reservations VALUES('ordinary-c-other',?1,'capture',1)",[target.binding().owner.as_str()]).unwrap();
                }
                "retained-debit" => { c.execute("UPDATE reservations SET remaining=remaining+1 WHERE id=?1",[&retained.binding().reservation]).unwrap(); }
                "retained-published" => { c.execute("UPDATE exact_installations SET published=0 WHERE reservation=?1",[&retained.binding().reservation]).unwrap(); }
                "full-quota" => {
                    put(c,target.binding(),&required[0],&journal(target.binding(),&required[0],"declared",&[],false));
                    c.execute("UPDATE reservations SET remaining=?1 WHERE id=?2",params![target.binding().quota_bytes,target.binding().reservation]).unwrap();
                }
                _=>unreachable!(),
            }
            refusal(c,family,retained,target,required);
            c.execute_batch("ROLLBACK TO corrupt_installing; RELEASE corrupt_installing").unwrap();
            inspect(c,family,retained,target,required);
        }
    });
});

owned!(
    export_installing_keeps_global_retained_journal_ordinary_and_sweep_usage_at_exact_capacity,
    {
        export_tests::fixture(|c, family, retained, target, required| {
            c.execute("INSERT INTO reservations VALUES('ordinary-installing-background',?1,'capture',777)",[retained.binding().owner.as_str()]).unwrap();
            c.execute(
                "INSERT INTO sweep VALUES('chunks','ordinary-installing-sweep',123)",
                [],
            )
            .unwrap();
            reserve(c, family, retained, target);
            let before = inspect(c, family, retained, target, required);
            let exact = before.content_bytes + before.reserved_bytes;
            c.execute("UPDATE quota SET bytes=?1 WHERE singleton=1", [exact])
                .unwrap();
            put(
                c,
                target.binding(),
                &required[0],
                &journal(target.binding(), &required[0], "verified", &[], false),
            );
            let actual = inspect(c, family, retained, target, required);
            assert_eq!(actual.content_bytes + actual.reserved_bytes, exact);
            assert_eq!(actual.limit_bytes, exact);
            c.execute("UPDATE quota SET bytes=?1 WHERE singleton=1", [exact - 1])
                .unwrap();
            refusal(c, family, retained, target, required);
            c.execute("UPDATE quota SET bytes=?1 WHERE singleton=1", [exact])
                .unwrap();
            let cancel = Cancellation::default();
            cancel.cancel();
            let snapshot = state(c);
            let changes = c.total_changes();
            assert!(retained
                .validate_export_installing(c, family, target, required, Some(&cancel))
                .is_err());
            assert_eq!(c.total_changes(), changes);
            assert_eq!(state(c), snapshot);
            inspect(c, family, retained, target, required);
            assert_eq!(
                c.query_row(
                    "SELECT remaining FROM reservations WHERE id='ordinary-installing-background'",
                    [],
                    |r| r.get::<_, u64>(0)
                )
                .unwrap(),
                777
            );
            assert_eq!(
                c.query_row(
                    "SELECT bytes FROM sweep WHERE id='ordinary-installing-sweep'",
                    [],
                    |r| r.get::<_, u64>(0)
                )
                .unwrap(),
                123
            );
        });
    }
);

//! Owned catalog SQL DATA only. Existing fixtures supply completely validated
//! retained Published journals/debits. B/C Staging, image and metadata-floor
//! inputs are explicitly declared DATA: no private admission or physical C is
//! authenticated by these recipes, and no lease/content authority is granted.
use super::tests as retained;
use super::*;
use crate::projection::provider::{
    owned_test as child,
    records::{
        Record,
        import::{ImportSourceReferences, ProviderImportPrefixRecord},
        transition::ProviderTransitionIntentRecord,
    },
};
use layrs_core::scm::WorktreeId;
use rusqlite::types::ValueRef;
use serde_json::json;

const TX: &str = "000000000000000000000000000000c1";
const LEAF: &str = "000000000000000000000000000000c2";
const RAW: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TOKEN: &str = "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DECLARED_PRIVATE_FLOOR: u64 = 64 * 1024;
const DECLARED_MARGIN: u64 = 1024 * 1024;

macro_rules! owned {
    ($name:ident, $body:block) => {
        #[test]
        #[ignore = "ROOT source release and audited owned export-family SQL child120/outer180 required"]
        fn $name() {
            child::run(concat!(module_path!(), "::", stringify!($name)), 120, || {
                child::declare();
                $body
            });
        }
    };
}

fn token(domain: &'static str, bytes: &[u8]) -> String {
    format!(
        "blake3:{}",
        blake3::hash(&layrs_core::scm::domain_separated_bytes(domain, bytes))
    )
}

fn protected(required: &[FileManifest]) -> String {
    let ordered: BTreeMap<_, _> = required
        .iter()
        .map(|manifest| ((manifest.file_id(), manifest.id()), manifest))
        .collect();
    assert_eq!(ordered.len(), required.len());
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-provider-managed-protected-manifests-v1\0");
    hash.update(&(ordered.len() as u64).to_be_bytes());
    for manifest in ordered.into_values() {
        let row = canonical_json(&json!({
            "fileId": manifest.file_id(), "manifestId": manifest.id(),
            "bytes": manifest.byte_length(),
        }))
        .unwrap();
        hash.update(&(row.len() as u64).to_be_bytes());
        hash.update(&row);
    }
    format!("blake3:{}", hash.finalize())
}

fn baseline_quota(required: &[FileManifest]) -> u64 {
    // Descriptive Q0 for this declared DATA image: both exact representations,
    // and one current file. This does not establish owner rederivation.
    let mut quota = 65536;
    for manifest in required {
        let payload = canonical_json(manifest).unwrap().len() as u64;
        quota += manifest.byte_length()
            + 2 * payload
            + 8192
            + 2048
            + manifest.chunks().len() as u64 * 256;
    }
    quota + required.first().map_or(0, FileManifest::byte_length)
}

pub(super) fn target(
    retained: &AdmittedFamilyContext,
    required: &[FileManifest],
) -> ExportReserveOnlyTarget {
    target_with_funding(
        retained,
        required,
        baseline_quota(required),
        DECLARED_MARGIN,
        DECLARED_PRIVATE_FLOOR,
    )
}

// Declared DATA only: alternate funding lets protection tests isolate the
// late remaining-envelope check without weakening a production constructor.
pub(super) fn target_with_funding(
    retained: &AdmittedFamilyContext,
    required: &[FileManifest],
    baseline: u64,
    margin: u64,
    private_floor: u64,
) -> ExportReserveOnlyTarget {
    let marker = retained.marker();
    let shared = marker.shared.as_ref().unwrap();
    let mut identity = marker.identity.clone();
    identity.worktree_id = WorktreeId::parse("worktree:export-reserve-only-c").unwrap();
    let rowset = |count| json!({"count": count, "digest": TOKEN});
    let admission = json!({
        "admissionDigest": retained.admission_digest(),
        "incomingPrefixId": retained.binding().prefix_id,
        "incomingReceiptId": TOKEN,
        "databaseIdentity": retained.binding().database_identity,
    });
    let prefix: ProviderImportPrefixRecord = serde_json::from_value(json!({
        "schemaVersion": "layrs.projection-provider-import-prefix.v2",
        "kind": "managedOrdinary",
        "source": {
            "kind": "ordinary", "identity": marker.identity,
            "format_digest": token("layrs-projection-provider-import-format-v1", &marker.bytes().unwrap()),
            "private_schema": 9,
            "version": {"generation": 3, "snapshotDigest": RAW,
                "selectionDigest": RAW, "captureCount": 1, "captureHead": TOKEN,
                "effectiveImageDigest": RAW},
            "checkout_identity": marker.checkout_identity(), "admission": admission,
        },
        "destinationIdentity": identity,
        "inheritedSegments": rowset(1), "ancestryRootReceipt": TOKEN,
        "closure": {"snapshots": rowset(4), "selections": rowset(1),
            "captures": rowset(1), "publications": rowset(0), "catalogs": rowset(0),
            "descriptors": rowset(0), "activationMaps": rowset(0),
            "transitionReceipts": rowset(1), "companions": rowset(0),
            "companionFiles": rowset(0), "companionDeletions": rowset(0),
            "contentMappings": rowset(1), "restoreHistory": rowset(0)},
    })).unwrap();
    prefix
        .require_references(ImportSourceReferences::AdmittedOrdinary {
            marker,
            admission: prefix.source_admission().unwrap(),
        })
        .unwrap();
    let mut destination_marker = serde_json::to_value(marker).unwrap();
    destination_marker["identity"] = serde_json::to_value(&identity).unwrap();
    destination_marker["checkoutRoot"] = std::path::Path::new("C:\\export-reserve-parent")
        .join(LEAF)
        .to_str()
        .unwrap()
        .into();
    destination_marker["checkoutIdentity"] = "fs:v1:0000000000000001:0000000000000011".into();
    destination_marker["storeIdentity"] = "fs:v1:0000000000000001:0000000000000012".into();
    destination_marker["metadataIdentity"] = "fs:v1:0000000000000001:0000000000000013".into();
    destination_marker["importDigest"] = prefix.digest().unwrap().into();
    destination_marker["shared"]["metadataPath"] = shared
        .root
        .join("worktrees")
        .join(
            blake3::hash(identity.worktree_id.as_str().as_bytes())
                .to_hex()
                .to_string(),
        )
        .join("metadata")
        .to_str()
        .unwrap()
        .into();
    destination_marker["shared"]["databaseIdentity"] = "1:0:800".into();
    let quota = baseline.checked_add(margin).unwrap();
    let mut source: ProviderTransitionIntentRecord = serde_json::from_value(json!({
        "schemaVersion": "layrs.projection-provider-transition-intent.v1",
        "transactionId": TX, "role": "source", "state": "staging", "localGeneration": 3,
        "plan": {"kind": "managedOrdinary", "sourcePrefixId": prefix.digest().unwrap(),
            "sourcePrefix": prefix, "destinationParent": "C:\\export-reserve-parent",
            "destinationParentIdentity": "fs:v1:0000000000000001:0000000000000010",
            "destinationLeafNonce": LEAF,
            "observation": {"stepId": TOKEN, "rootTreeId": TOKEN, "policySetId": TOKEN,
                "authorizationEpoch": 1, "generation": 4, "snapshotDigest": RAW,
                "selectionDigest": RAW},
            "placementGeneration": 5, "placementDigest": RAW,
            "captureSequence": 2, "captureDigest": TOKEN,
            "workingOverlay": rowset(0), "companionId": null,
            "companionFiles": rowset(0), "companionDeletions": rowset(0),
            "protectedManifests": {"count": required.len(), "digest": protected(required)},
            "quotaBytes": quota},
        "destination": {"backend": "ordinary", "format": destination_marker,
            "formatFileIdentity": "1:0:801", "privateDatabaseIdentity": "1:0:800",
            "familyMarkerDigest": retained.binding().family_marker_digest,
            "nativeControlIdentityHex": null, "nativeRootIdentity": null},
        "reservation": null, "receiptId": null,
    }))
    .unwrap();
    source.reservation = Some(source.reservation_operation().unwrap());
    source.encode().unwrap();
    let mut destination =
        ProviderTransitionIntentRecord::decode(&source.encode().unwrap()).unwrap();
    destination.role =
        crate::projection::provider::records::transition::TransitionRole::Destination;
    destination.local_generation = 0;
    destination.encode().unwrap();
    let encoded_pair = source.encode().unwrap().len() + destination.encode().unwrap().len();
    assert!((encoded_pair as u64) < private_floor);
    ExportReserveOnlyTarget::new(
        retained,
        &source,
        &destination,
        required,
        baseline,
        private_floor,
        None,
    )
    .unwrap()
}

pub(super) fn fixture(
    body: impl FnOnce(
        &Connection,
        &[u8],
        &AdmittedFamilyContext,
        &ExportReserveOnlyTarget,
        &[FileManifest],
    ),
) {
    retained::fixture(|c, family, _, manifests| {
        // Add a second genuinely Published SQL group using the existing debit
        // fixture. Its declared checkout identities are canonical for the v2
        // ordinary source codec; no private B/C database is represented here.
        let mut source = retained::context(family, 4, manifests);
        let mut marker = serde_json::to_value(source.marker()).unwrap();
        marker["checkoutIdentity"] = "fs:v1:0000000000000001:0000000000000007".into();
        marker["storeIdentity"] = "fs:v1:0000000000000001:0000000000000008".into();
        marker["metadataIdentity"] = "fs:v1:0000000000000001:0000000000000009".into();
        marker["shared"]["metadataPath"] = source
            .marker()
            .shared
            .as_ref()
            .unwrap()
            .root
            .join("worktrees")
            .join(
                blake3::hash(source.binding().owner.as_str().as_bytes())
                    .to_hex()
                    .to_string(),
            )
            .join("metadata")
            .to_str()
            .unwrap()
            .into();
        source.primary.marker =
            format::parse_marker_bytes(&canonical_json(&marker).unwrap()).unwrap();
        retained::install(c, &source, manifests);
        source.validate_catalog(c, family, None).unwrap();
        let target = target(&source, manifests);
        let binding = target.binding();
        c.execute(
            "INSERT INTO worktrees VALUES(?1,?2,?3,?4,'initializing')",
            params![
                binding.owner.as_str(),
                target
                    .marker()
                    .shared
                    .as_ref()
                    .unwrap()
                    .metadata_path
                    .to_str()
                    .unwrap(),
                binding.database_identity,
                target.marker().bytes().unwrap(),
            ],
        )
        .unwrap();
        body(c, family, &source, &target, manifests);
    });
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
enum Cell {
    Null,
    Integer(i64),
    Real(u64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}
type Rows = Vec<Vec<Cell>>;
fn rows(c: &Connection, table: &str) -> Rows {
    assert!(TABLES.contains(&table));
    let mut statement = c
        .prepare(&format!("SELECT * FROM \"{table}\" LIMIT 257"))
        .unwrap();
    let columns = statement.column_count();
    assert!(columns <= 16);
    let mut query = statement.query([]).unwrap();
    let mut rows = Vec::new();
    let mut bytes = 0usize;
    while let Some(row) = query.next().unwrap() {
        assert!(rows.len() < 256);
        rows.push(
            (0..columns)
                .map(|column| match row.get_ref(column).unwrap() {
                    ValueRef::Null => Cell::Null,
                    ValueRef::Integer(value) => Cell::Integer(value),
                    ValueRef::Real(value) => Cell::Real(value.to_bits()),
                    ValueRef::Text(value) => {
                        bytes += value.len();
                        assert!(bytes <= 16 * 1024 * 1024);
                        Cell::Text(value.to_vec())
                    }
                    ValueRef::Blob(value) => {
                        bytes += value.len();
                        assert!(bytes <= 16 * 1024 * 1024);
                        Cell::Blob(value.to_vec())
                    }
                })
                .collect(),
        );
    }
    rows.sort();
    rows
}
fn state(c: &Connection) -> BTreeMap<&'static str, Rows> {
    TABLES
        .into_iter()
        .map(|table| (table, rows(c, table)))
        .collect()
}
fn refuses_without_writes(
    c: &Connection,
    family: &[u8],
    retained: &AdmittedFamilyContext,
    target: &ExportReserveOnlyTarget,
) {
    let before = state(c);
    let changes = c.total_changes();
    assert!(
        retained
            .validate_export_reserve_only(c, family, target, None)
            .is_err()
    );
    assert!(
        retained
            .reserve_export_only(c, family, target, None)
            .is_err()
    );
    assert_eq!(c.total_changes(), changes);
    assert_eq!(state(c), before);
}

owned!(
    export_reserve_only_inserts_full_quota_retries_and_rolls_back_without_changing_retained_groups,
    {
        fixture(|c, family, retained, target, manifests| {
            let (proof, before_usage) = retained
                .validate_export_reserve_only(c, family, target, None)
                .unwrap();
            assert_eq!(before_usage.reservation, ExportReservation::Absent);
            assert_eq!(
                proof
                    .manifests()
                    .iter()
                    .map(|manifest| manifest.id())
                    .collect::<BTreeSet<_>>(),
                manifests
                    .iter()
                    .map(|manifest| manifest.id())
                    .collect::<BTreeSet<_>>()
            );
            assert_eq!(manifests[0].file_id(), manifests[1].file_id());
            assert_ne!(manifests[0].id(), manifests[1].id());
            let before = state(c);
            c.execute_batch("SAVEPOINT reserve_target").unwrap();
            let changes = c.total_changes();
            assert_eq!(
                retained
                    .reserve_export_only(c, family, target, None)
                    .unwrap(),
                ExportReservationEffect::Inserted
            );
            assert_eq!(c.total_changes(), changes + 1);
            let (_, after_usage) = retained
                .validate_export_reserve_only(c, family, target, None)
                .unwrap();
            assert_eq!(
                after_usage.reservation,
                ExportReservation::Present {
                    remaining: target.binding().quota_bytes
                }
            );
            assert_eq!(
                after_usage.usage.content_bytes,
                before_usage.usage.content_bytes
            );
            assert_eq!(
                after_usage.usage.reserved_bytes,
                before_usage.usage.reserved_bytes + target.binding().quota_bytes
            );
            let after = state(c);
            for (table, records) in &before {
                if *table != "reservations" {
                    assert_eq!(
                        after.get(table),
                        Some(records),
                        "reservation changed {table}"
                    );
                }
            }
            assert_eq!(
                after["reservations"].len(),
                before["reservations"].len() + 1
            );
            for record in &before["reservations"] {
                assert!(
                    after["reservations"].contains(record),
                    "retained remaining/debit changed"
                );
            }
            let changes = c.total_changes();
            assert_eq!(
                retained
                    .reserve_export_only(c, family, target, None)
                    .unwrap(),
                ExportReservationEffect::AlreadyPresent
            );
            assert!(
                retained.validate_catalog(c, family, None).is_err(),
                "ordinary policy must reject active C"
            );
            assert_eq!(c.total_changes(), changes);
            assert_eq!(state(c), after);
            c.execute_batch("ROLLBACK TO reserve_target; RELEASE reserve_target")
                .unwrap();
            assert_eq!(state(c), before);
            retained.validate_catalog(c, family, None).unwrap();
            assert_eq!(
                retained
                    .validate_export_reserve_only(c, family, target, None)
                    .unwrap()
                    .1
                    .reservation,
                ExportReservation::Absent
            );
        });
    }
);

owned!(
    export_reserve_only_refuses_partial_foreign_progress_and_retained_corruption_without_repair,
    {
        fixture(|c, family, retained, target, manifests| {
            retained
                .reserve_export_only(c, family, target, None)
                .unwrap();
            for mutation in [
                "remaining",
                "owner",
                "operation",
                "unknown-group",
                "root",
                "legacy-install",
                "exact-install",
                "retained-debit",
                "foreign-published",
                "duplicate-charge",
            ] {
                c.execute_batch("SAVEPOINT corrupt_export").unwrap();
                let b = target.binding();
                match mutation {
                    "remaining" => {
                        c.execute(
                            "UPDATE reservations SET remaining=remaining-1 WHERE id=?1",
                            [&b.reservation],
                        )
                        .unwrap();
                    }
                    "owner" => {
                        c.execute(
                            "UPDATE reservations SET owner=?1 WHERE id=?2",
                            params![retained.binding().owner.as_str(), b.reservation],
                        )
                        .unwrap();
                    }
                    "operation" => {
                        c.execute(
                            "UPDATE reservations SET operation='capture' WHERE id=?1",
                            [&b.reservation],
                        )
                        .unwrap();
                    }
                    "unknown-group" => {
                        c.execute("INSERT INTO reservations VALUES('provider-transition:unknown',?1,'provider-transition:unknown',0)", [retained.binding().owner.as_str()]).unwrap();
                    }
                    "root" => {
                        c.execute("INSERT INTO roots VALUES('unexpected-c-root',?1,?2,'capture','unexpected-c-root',x'00')", params![b.owner.as_str(), b.database_identity]).unwrap();
                    }
                    "legacy-install" => {
                        c.execute(
                            "INSERT INTO installations VALUES(?1,?2,x'00',NULL,0)",
                            params![b.reservation, manifests[0].file_id().as_str()],
                        )
                        .unwrap();
                    }
                    "exact-install" => {
                        c.execute(
                            "INSERT INTO exact_installations VALUES(?1,?2,?3,?4,x'00',?5,0)",
                            params![
                                b.reservation,
                                manifests[0].id().as_str(),
                                manifests[0].file_id().as_str(),
                                manifests[0].byte_length(),
                                canonical_json(&manifests[0]).unwrap()
                            ],
                        )
                        .unwrap();
                    }
                    "retained-debit" => {
                        c.execute(
                            "UPDATE reservations SET remaining=remaining+1 WHERE id=?1",
                            [&retained.binding().reservation],
                        )
                        .unwrap();
                    }
                    "foreign-published" => {
                        c.execute("UPDATE exact_installations SET published=0 WHERE reservation=(SELECT reservation FROM exact_installations WHERE reservation<>?1 LIMIT 1)", [&retained.binding().reservation]).unwrap();
                    }
                    "duplicate-charge" => {
                        let id = manifests[0].id().as_str();
                        let (payload, manifest): (Vec<u8>, Vec<u8>) = c.query_row("SELECT payload,manifest FROM exact_installations WHERE reservation=?1 AND manifest_id=?2", params![retained.binding().reservation, id], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
                        let mut record: serde_json::Value =
                            serde_json::from_slice(&payload).unwrap();
                        assert_eq!(record["chargedManifest"], false);
                        record["chargedManifest"] = true.into();
                        record["debitBytes"] =
                            (record["debitBytes"].as_u64().unwrap() + manifest.len() as u64).into();
                        c.execute("UPDATE exact_installations SET payload=?1 WHERE reservation=?2 AND manifest_id=?3", params![canonical_json(&record).unwrap(), retained.binding().reservation, id]).unwrap();
                        c.execute(
                            "UPDATE reservations SET remaining=remaining-?1 WHERE id=?2",
                            params![manifest.len() as u64, retained.binding().reservation],
                        )
                        .unwrap();
                    }
                    _ => unreachable!(),
                }
                if mutation == "duplicate-charge" {
                    let refusal = retained
                        .validate_export_reserve_only(c, family, target, None)
                        .err()
                        .expect("a second manifest debit must refuse");
                    assert!(refusal.to_string().contains("duplicate debit"), "{refusal}");
                }
                refuses_without_writes(c, family, retained, target);
                c.execute_batch("ROLLBACK TO corrupt_export; RELEASE corrupt_export")
                    .unwrap();
                retained
                    .validate_export_reserve_only(c, family, target, None)
                    .unwrap();
            }
        });
    }
);

owned!(
    export_reserve_only_counts_retained_journals_ordinary_reservations_and_sweep_at_exact_capacity,
    {
        fixture(|c, family, retained, target, _| {
            c.execute(
                "INSERT INTO reservations VALUES('ordinary-background',?1,'capture',777)",
                [retained.binding().owner.as_str()],
            )
            .unwrap();
            c.execute(
                "INSERT INTO sweep VALUES('chunks','ordinary-pending-sweep',123)",
                [],
            )
            .unwrap();
            let (_, usage) = retained
                .validate_export_reserve_only(c, family, target, None)
                .unwrap();
            let (records, manifest_bytes): (u64, u64) = c
                .query_row(
                    "SELECT count(*),sum(length(manifest)) FROM exact_installations",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(
                records, 4,
                "two complete retained groups share two representations"
            );
            let physical: u64 = c.query_row("SELECT coalesce((SELECT sum(bytes) FROM chunks),0)+coalesce((SELECT sum(length(payload)) FROM manifests),0)+coalesce((SELECT sum(bytes) FROM sweep),0)", [], |row| row.get(0)).unwrap();
            assert_eq!(
                usage.usage.content_bytes,
                physical + records * 8192 + manifest_bytes
            );
            let exact = usage.usage.content_bytes
                + usage.usage.reserved_bytes
                + target.binding().quota_bytes;
            c.execute("UPDATE quota SET bytes=?1 WHERE singleton=1", [exact - 1])
                .unwrap();
            let before = state(c);
            let changes = c.total_changes();
            let refusal = retained
                .reserve_export_only(c, family, target, None)
                .unwrap_err();
            assert!(
                refusal.to_string().contains("partial_store_quota"),
                "{refusal}"
            );
            assert_eq!(c.total_changes(), changes);
            assert_eq!(state(c), before);
            c.execute("UPDATE quota SET bytes=?1 WHERE singleton=1", [exact])
                .unwrap();
            assert_eq!(
                retained
                    .reserve_export_only(c, family, target, None)
                    .unwrap(),
                ExportReservationEffect::Inserted
            );
            let (_, after) = retained
                .validate_export_reserve_only(c, family, target, None)
                .unwrap();
            assert_eq!(
                after.usage.content_bytes + after.usage.reserved_bytes,
                exact
            );
            assert_eq!(after.usage.limit_bytes, exact);
            let before = state(c);
            let changes = c.total_changes();
            assert_eq!(
                retained
                    .reserve_export_only(c, family, target, None)
                    .unwrap(),
                ExportReservationEffect::AlreadyPresent
            );
            assert_eq!(c.total_changes(), changes);
            assert_eq!(state(c), before);
        });
    }
);

owned!(
    export_reserve_only_precancel_and_fk_off_refuse_without_effects,
    {
        fixture(|c, family, retained, target, _| {
            let cancel = Cancellation::default();
            cancel.cancel();
            let before = state(c);
            let changes = c.total_changes();
            assert!(
                retained
                    .validate_export_reserve_only(c, family, target, Some(&cancel))
                    .is_err()
            );
            assert!(
                retained
                    .reserve_export_only(c, family, target, Some(&cancel))
                    .is_err()
            );
            assert_eq!(c.total_changes(), changes);
            assert_eq!(state(c), before);
            c.execute_batch("COMMIT; PRAGMA foreign_keys=OFF; BEGIN DEFERRED")
                .unwrap();
            assert_eq!(
                c.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, u64>(0))
                    .unwrap(),
                0
            );
            let before = state(c);
            let changes = c.total_changes();
            retained
                .validate_export_reserve_only(c, family, target, None)
                .unwrap();
            let refusal = retained
                .reserve_export_only(c, family, target, None)
                .unwrap_err();
            assert!(refusal.to_string().contains("FK-enabled"), "{refusal}");
            assert_eq!(c.total_changes(), changes);
            assert_eq!(state(c), before);
        });
    }
);

owned!(
    export_reserve_only_refuses_oversized_exact_inventory_before_retained_walk,
    {
        fixture(|c, family, retained, target, manifests| {
            // This is bounded corrupt inventory, not 100k valid Published groups.
            // The separate export arithmetic test covers retained+future ==100k
            // and one-over; this recipe checks the real SQL count preflight.
            c.execute("WITH RECURSIVE n(value) AS (VALUES(1) UNION ALL SELECT value+1 FROM n WHERE value<100001) INSERT OR IGNORE INTO exact_installations SELECT ?1,'blake3:'||printf('%064x',value),?2,0,x'00',NULL,0 FROM n", params![retained.binding().reservation, manifests[0].file_id().as_str()]).unwrap();
            let count: u64 = c
                .query_row("SELECT count(*) FROM exact_installations", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert!(count > 100_000);
            let changes = c.total_changes();
            let refusal = retained
                .validate_export_reserve_only(c, family, target, None)
                .err()
                .expect("oversized SQL inventory must refuse");
            assert!(
                refusal
                    .to_string()
                    .contains("exact journal count exceeds bound"),
                "{refusal}"
            );
            assert!(
                retained
                    .reserve_export_only(c, family, target, None)
                    .is_err()
            );
            assert_eq!(c.total_changes(), changes);
            assert_eq!(
                c.query_row("SELECT count(*) FROM exact_installations", [], |row| row
                    .get::<_, u64>(0))
                    .unwrap(),
                count
            );
            assert!(
                !c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM reservations WHERE id=?1)",
                    [&target.binding().reservation],
                    |row| row.get::<_, bool>(0)
                )
                .unwrap()
            );
        });
    }
);

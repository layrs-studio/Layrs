//! Synthetic DATA/SQL fixtures derived from the independently frozen R4 source
//! literals. Native tokens are deliberately synthetic; no filesystem authority.
use super::super::*;
use layrs_core::{
    projection::{PathSemantics, ProjectionSelection},
    scm::{FileId, FileManifest},
};
use rusqlite::params;
use serde::Deserialize;

pub(in crate::projection::provider) const TX: &str = "00000000000000000000000000000001";
pub(in crate::projection::provider) struct Fixture {
    pub(in crate::projection::provider) source: ProviderBootstrapSource,
    pub(in crate::projection::provider) allocation: AllocationRecord,
    pub(in crate::projection::provider) prepared: PreparedRecord,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Golden {
    label: String,
    field: String,
    byte_length: usize,
    hex: String,
}
fn literal(label: &str, field: &str) -> Vec<u8> {
    let bytes = include_bytes!("../../records/golden-r4.json");
    let rows: Vec<Golden> =
        serde_json::from_slice(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap()).unwrap();
    let mut found = rows
        .into_iter()
        .filter(|r| r.label == label && r.field == field);
    let value = found.next().unwrap();
    assert!(found.next().is_none());
    let result: Vec<_> = value
        .hex
        .as_bytes()
        .chunks_exact(2)
        .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
        .collect();
    assert_eq!(result.len(), value.byte_length);
    result
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl Fixture {
    pub(in crate::projection::provider) fn new(label: &str, executable: bool) -> Self {
        let mut initial: ProjectionSnapshot = serde_json::from_slice(&literal(label, "S")).unwrap();
        if executable {
            assert!(!initial.entries.is_empty());
            initial.entries[0].executable = true;
        }
        let selection = if label == "G2" {
            ProjectionSelection::selected(
                vec![RepoPath::parse("README.md").unwrap()],
                vec![],
                PathSemantics::PortableWindows,
            )
            .unwrap()
        } else {
            ProjectionSelection::All
        };
        let inputs = if label == "G0" {
            vec![]
        } else {
            vec![ProviderSourceDescriptor {
                path: RepoPath::parse("README.md").unwrap(),
                manifest: serde_json::from_slice::<FileManifest>(&literal(label, "Fb")).unwrap(),
            }]
        };
        let source = ProviderBootstrapSource::validate(initial, selection, inputs).unwrap();
        let mut marker: serde_json::Value =
            serde_json::from_slice(&literal(label, "FORMAT")).unwrap();
        marker["initialSnapshotDigest"] = source.commitment().snapshot_digest.clone().into();
        let marker: crate::projection::format::Marker = serde_json::from_value(marker).unwrap();
        let format = marker.bytes().unwrap();
        let metadata = marker
            .shared
            .as_ref()
            .unwrap()
            .metadata_path
            .to_str()
            .unwrap();
        let allocation: AllocationRecord = serde_json::from_value(serde_json::json!({
            "schemaVersion":"layrs.projection-provider-allocation.v1", "transactionId":TX, "receiptFileIdentity":"1:0:24",
            "family":{"root":"C:\\family","canonicalRoot":"C:\\family","incarnation":"test-incarnation",
                "rootIdentity":"fs:v1:0000000000000001:0000000000000014","markerFileIdentity":"1:0:23",
                "markerDigest":"blake3:0000000000000000000000000000000000000000000000000000000000000000",
                "metadataIdentity":"fs:v1:0000000000000001:0000000000000015","catalogFileIdentity":"1:0:22"},
            "private":{"metadataPath":metadata,"canonicalMetadataPath":metadata,
                "metadataIdentity":"fs:v1:0000000000000001:0000000000000009",
                "providerControlIdentity":"fs:v1:0000000000000001:000000000000000a","databaseFileIdentity":"1:0:11"},
            "stageIdentityHex":hex(&literal(label,"LYRCSTG1")), "format":marker,
            "formatBytes":format.len(), "formatFileId":FileId::from_bytes(&format), "source":source.commitment()
        })).unwrap();
        allocation.validate_fields().unwrap();
        let stage = allocation.stage_identity().unwrap().encode();
        let mut control = b"LYRCDIR1".to_vec();
        control.extend_from_slice(&(stage.len() as u32).to_le_bytes());
        control.extend_from_slice(&stage);
        control.extend_from_slice(&(format.len() as u32).to_le_bytes());
        control.extend_from_slice(allocation.format_file_id.as_str().as_bytes());
        let prepared = PreparedRecord {
            schema_version: "layrs.projection-provider-prepared.v1".into(),
            transaction_id: TX.into(),
            receipt_file_identity: "1:0:25".into(),
            allocation: allocation.data_reference().unwrap(),
            control_identity_hex: hex(&control),
        };
        prepared.require_allocation(&allocation).unwrap();
        Self {
            source,
            allocation,
            prepared,
        }
    }
    pub(in crate::projection::provider) fn mount(&self, phase: MountPhase) -> MountRecord {
        MountRecord {
            schema_version: "layrs.projection-provider-mount.v1".into(),
            transaction_id: self.allocation.transaction_id.clone(),
            catalog_id: self.source.commitment().catalog_id.clone(),
            phase,
            allocation: self.allocation.data_reference().unwrap(),
            prepared: if phase == MountPhase::Allocating {
                None
            } else {
                Some(self.prepared.data_reference().unwrap())
            },
        }
    }
    pub(in crate::projection::provider) fn populate(&self, c: &Connection, phase: MountPhase) {
        let begin = c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE;");
        if let Err(error) = &begin {
            // Capture SQLite's exposed state after execute_batch/statement cleanup,
            // before the filename accessor or output. This is not the first Win32 error.
            // SAFETY: c remains alive and is used only by this fixture thread.
            let sqlite_exposed_system_errno =
                unsafe { rusqlite::ffi::sqlite3_system_errno(c.handle()) };
            let (variant, base_name, base_code, extended_code) = match error {
                rusqlite::Error::SqliteFailure(code, _) => (
                    "rusqlite::Error::SqliteFailure",
                    Some(format!("{:?}", code.code)),
                    Some(code.extended_code & 0xff),
                    Some(code.extended_code),
                ),
                _ => ("rusqlite::Error (non-SqliteFailure)", None, None, None),
            };
            // rusqlite 0.32.1 path() borrows sqlite3_db_filename(main): no SQL or
            // filesystem access. None also includes unavailable/non-UTF-8 names;
            // an empty name denotes memory/temporary, not a missing file.
            let observed_filename = c.path();
            let (filename_state, filename, database_utf16_units, journal_utf16_units) =
                match observed_filename {
                    Some("") => ("memoryOrTemporary", Some(""), Some(0), None),
                    Some(name) if name.len() <= 4096 => {
                        let units = name.encode_utf16().count();
                        (
                            "available",
                            Some(name),
                            Some(units),
                            units.checked_add("-journal".encode_utf16().count()),
                        )
                    }
                    Some(_) => ("omittedOver4096Utf8Bytes", None, None, None),
                    None => ("unavailableOrNonUtf8", None, None, None),
                };
            let diagnostic = serde_json::json!({
                "event": "providerFixtureBeginFailure",
                "errorType": variant,
                "sqliteBaseCodeName": base_name,
                "sqliteBaseCode": base_code,
                "sqliteExtendedCode": extended_code,
                "sqliteExposedSystemErrnoAfterExecuteBatch": sqlite_exposed_system_errno,
                "filenameApi": "rusqlite::Connection::path/sqlite3_db_filename(main)",
                "filenameState": filename_state,
                "filenameUtf8Bytes": observed_filename.map(str::len),
                "connectionFilename": filename,
                "databaseUtf16UnitsExcludingNul": database_utf16_units,
                "derivedJournalUtf16UnitsExcludingNul": journal_utf16_units,
                "derivedJournalSuffix": "-journal",
                "journalNameIsDerivedNotObserved": true,
            });
            // Only bounded strings/scalars are serialized: <32 KiB including
            // worst-case JSON escaping. Output/encoding errors must not mask begin.
            let mut diagnostic = serde_json::to_vec(&diagnostic).unwrap_or_else(|_| {
                br#"{"event":"providerFixtureBeginFailure","diagnosticEncoding":"unavailable"}"#
                    .to_vec()
            });
            diagnostic.push(b'\n');
            let _ = std::io::Write::write_all(&mut std::io::stderr().lock(), &diagnostic);
        }
        begin.unwrap();
        c.execute_batch(
            &[
                database::SQL,
                selection::SQL,
                retention::SQL,
                super::super::super::schema::SQL,
                "PRAGMA user_version=8;",
            ]
            .concat(),
        )
        .unwrap();
        c.execute(
            "INSERT INTO projection_meta VALUES(1,?1)",
            [self.allocation.format.bytes().unwrap()],
        )
        .unwrap();
        c.execute(
            "INSERT INTO projection_snapshots VALUES(0,?1,NULL,?2)",
            params![
                self.source.commitment().snapshot_digest,
                self.source.initial().canonical_bytes().unwrap()
            ],
        )
        .unwrap();
        c.execute("INSERT INTO projection_head VALUES(1,0)", [])
            .unwrap();
        selection::insert(c, 0, self.source.selection()).unwrap();
        let q = self.source.catalog();
        c.execute(
            "INSERT INTO projection_provider_catalogs VALUES(?1,0,?2,?3,?4)",
            params![
                self.source.commitment().catalog_id,
                q.source_digest,
                q.selection_digest,
                q.encode().unwrap()
            ],
        )
        .unwrap();
        for d in self.source.descriptors() {
            c.execute(
                "INSERT INTO projection_provider_descriptors VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    self.source.commitment().catalog_id,
                    d.path.as_str(),
                    d.file_id.as_str(),
                    d.bytes,
                    d.executable,
                    d.manifest_id.as_str(),
                    d.manifest_bytes().unwrap()
                ],
            )
            .unwrap();
        }
        let m = self.mount(phase);
        let phase = match phase {
            MountPhase::Allocating => "allocating",
            MountPhase::ControlPrepared => "control_prepared",
            MountPhase::ControlPublished => "control_published",
        };
        c.execute(
            "INSERT INTO projection_provider_mounts VALUES(?1,?2,?3,?4,?5)",
            params![
                m.transaction_id,
                m.catalog_id,
                phase,
                m.encode().unwrap(),
                m.digest().unwrap()
            ],
        )
        .unwrap();
        c.execute(
            "INSERT INTO projection_backend VALUES(1,'provider',?1)",
            [TX],
        )
        .unwrap();
        c.execute_batch("COMMIT;").unwrap();
        let bad: u64 = c
            .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(bad, 0, "positive fixture must have valid foreign keys");
    }
}

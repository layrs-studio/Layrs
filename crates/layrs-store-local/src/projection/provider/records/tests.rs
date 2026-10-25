//! Windows test-only draft tests. The 58 reviewed R4 candidate values are frozen
//! in a compile-time fixture; fixtureInputs/synthetic extras document inputs,
//! not observed identities or authenticated family authority.
//! Expected JSON below is hand-written JCS, and hashing/framing is independent
//! of records::codec, database::digest and the source validator under test.
//! Receipt roundtrips below are dynamic DAG tests, not literal receipt goldens.
use super::receipts::{ABANDONED_DOMAIN, ALLOCATION_DOMAIN, PREPARED_DOMAIN, Receipt};
use super::*;
use crate::projection::{RecoveryBinding, format};
use layrs_core::{
    projection::{
        EntryState, PathSemantics, ProjectionEntry, ProjectionSelection, ProjectionSnapshot,
    },
    scm::{
        ChunkId, ChunkingDescriptor, FileChunk, FileId, FileManifest, ObjectId, RepoPath, StepId,
        TreeEntryTarget, TreeId,
    },
};

const ZERO: &str = "blake3:0000000000000000000000000000000000000000000000000000000000000000";
const STEP: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";
const TREE: &str = "blake3:2222222222222222222222222222222222222222222222222222222222222222";
const POLICY: &str = "blake3:3333333333333333333333333333333333333333333333333333333333333333";
const TX: &str = "00000000000000000000000000000001";
const WORKTREE: &str = "worktree:stage1b-golden";
const STORE: &str = "store:stage1b-golden";
const SPACE: &str = "space:stage1b-golden";
const ORIGIN: &str = "https://example.invalid";
const PRINCIPAL: &str = "principal:golden";
const CHECKOUT: &str = r"C:\g\checkout";
const STAGE_PARENT: &str = r"C:\g\stages";
const STAGE: &str = r"C:\g\stages\s1";
const VOLUME: u32 = 1;
const CHECKOUT_INDEX: u64 = 0x10;
const STAGE_PARENT_INDEX: u64 = 0x20;
const STAGE_INDEX: u64 = 0x30;
const FORMAT_INDEX: u64 = 0x40;
// R4 leaves these extra comparison-data values open. Name the retained choices
// explicitly; none is an observed filesystem identity or authenticated digest.
const SYNTHETIC_FAMILY: &str = r"C:\family";
const SYNTHETIC_INCARNATION: &str = "test-incarnation";
const PRIVATE_METADATA_INDEX: u64 = 9;
const PROVIDER_CONTROL_INDEX: u64 = 10;
const PRIVATE_DB_INDEX: u64 = 11;
const FAMILY_ROOT_INDEX: u64 = 20;
const FAMILY_METADATA_INDEX: u64 = 21;
const FAMILY_CATALOG_INDEX: u64 = 22;
const FAMILY_MARKER_INDEX: u64 = 23;
const ALLOCATION_RECEIPT_INDEX: u64 = 24;
const PREPARED_RECEIPT_INDEX: u64 = 25;
const TERMINAL_RECEIPT_INDEX: u64 = 26;
const ALL: &[u8] =
    br#"{"schemaVersion":"layrs.projection-selection.v1","selection":{"kind":"all"}}"#;
const SELECTED: &[u8] = br#"{"schemaVersion":"layrs.projection-selection.v1","selection":{"kind":"selected","paths":["README.md"],"subtrees":[]}}"#;

fn independent_hash(prefix: &[u8], bytes: &[u8]) -> String {
    let mut h = blake3::Hasher::new();
    h.update(prefix);
    h.update(bytes);
    format!("blake3:{}", h.finalize())
}
fn independent_history(parent: Option<&str>, bytes: &[u8]) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"layrs-projection-state-v1\0");
    h.update(&[0; 8]);
    h.update(parent.unwrap_or("").as_bytes());
    h.update(&[0]);
    h.update(bytes);
    h.finalize().to_hex().to_string()
}
fn p(s: &str) -> RepoPath {
    RepoPath::parse(s).unwrap()
}
fn snapshot(entries: Vec<ProjectionEntry>) -> ProjectionSnapshot {
    ProjectionSnapshot {
        schema_version: "layrs.projection-snapshot.v1".into(),
        base_step_id: StepId::parse(STEP).unwrap(),
        base_root_tree_id: TreeId::parse(TREE).unwrap(),
        observed_policy_set_id: ObjectId::parse(POLICY).unwrap(),
        observed_authorization_epoch: 1,
        path_semantics: PathSemantics::PortableWindows,
        entries,
    }
}
fn literal_snapshot(entries: &str) -> Vec<u8> {
    format!(r#"{{"baseRootTreeId":"{TREE}","baseStepId":"{STEP}","entries":{entries},"observedAuthorizationEpoch":1,"observedPolicySetId":"{POLICY}","pathSemantics":"portable_windows","schemaVersion":"layrs.projection-snapshot.v1"}}"#).into_bytes()
}
fn fixture_directory(index: u64) -> String {
    format!("fs:v1:{VOLUME:016x}:{index:016x}")
}
fn fixture_file(index: u64) -> String {
    format!("{VOLUME}:{}:{}", index >> 32, index as u32)
}
fn fixture_input_map(selected: bool) -> Vec<u8> {
    let selection = if selected {
        "selected:README.md"
    } else {
        "all"
    };
    format!(r#"{{"epoch":1,"familyLocator":"C:\\family","familyMarkerDigest":"{ZERO}","familyMarkerDigestIsSynthetic":true,"incarnation":"test-incarnation","nativeIndices":{{"allocationReceipt":24,"checkout":16,"familyCatalog":22,"familyMarker":23,"familyMetadata":21,"familyRoot":20,"format":64,"preparedReceipt":25,"privateDatabase":11,"privateMetadata":9,"providerControl":10,"stage":48,"stageParent":32,"terminalReceipt":26}},"origin":"{ORIGIN}","pathSemantics":"portable_windows","policyId":"{POLICY}","principalId":"{PRINCIPAL}","root":"C:\\g\\checkout","selection":"{selection}","sourceGeneration":0,"spaceId":"{SPACE}","stage":"C:\\g\\stages\\s1","stageParent":"C:\\g\\stages","stepId":"{STEP}","storeId":"{STORE}","transactionId":"{TX}","treeId":"{TREE}","vfsCatalogVersion":0,"volume":1,"worktreeId":"{WORKTREE}"}}"#).into_bytes()
}
fn literal_marker(snapshot_digest: &str, selection_payload_digest: Option<&str>) -> Vec<u8> {
    let member = blake3::hash(WORKTREE.as_bytes()).to_hex();
    let selected = selection_payload_digest.map_or_else(String::new, |digest| {
        format!(r#""initialSelectionDigest":"{digest}","#)
    });
    // Handwritten expected JSON, including independent literal native tokens
    // and path escaping. No Marker/record/serde encoder generates these bytes.
    format!(r#"{{"checkoutIdentity":"fs:v1:0000000000000001:0000000000000010","checkoutRoot":"C:\\g\\checkout","formatVersion":4,"identity":{{"origin":"{ORIGIN}","principalId":"{PRINCIPAL}","spaceId":"{SPACE}","storeId":"{STORE}","worktreeId":"{WORKTREE}"}},{selected}"initialSnapshotDigest":"{snapshot_digest}","magic":"LAYRS_NATIVE_STORE","metadataIdentity":"fs:v1:0000000000000001:0000000000000009","minReaderVersion":4,"schemaVersion":"layrs.projection-store-format.v1","shared":{{"databaseIdentity":"1:0:11","incarnation":"test-incarnation","metadataPath":"C:\\family\\worktrees\\{member}\\metadata","root":"C:\\family","rootIdentity":"fs:v1:0000000000000001:0000000000000014"}},"storeIdentity":"fs:v1:0000000000000001:0000000000000030"}}"#).into_bytes()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LiteralGolden {
    label: String,
    field: String,
    byte_length: usize,
    hex: String,
}
fn literal_goldens() -> std::collections::BTreeMap<(String, String), Vec<u8>> {
    const FIXTURE: &[u8] = include_bytes!("golden-r4.json");
    const BASE_FIELDS: &[&str] = &[
        "fixtureInputs",
        "S",
        "E",
        "descriptorHashPreimage",
        "vfsFileIdPreimage",
        "Q",
        "SourceCommitment",
        "Sd",
        "Ed",
        "Ep",
        "D",
        "catalogId",
        "vfsCatalogId",
        "LYRCSTG1",
        "FORMAT",
        "formatFileId",
        "markerDigest",
        "LYRCDIR1",
    ];
    // Exact byte-for-byte copy of the independently reviewed coordinator output,
    // including its UTF-8 BOM. Tests do no filesystem reads or writes.
    assert!(FIXTURE.len() <= 64 * 1024);
    let json = FIXTURE
        .strip_prefix(&[0xef, 0xbb, 0xbf])
        .expect("reviewed fixture BOM");
    let values: Vec<LiteralGolden> = serde_json::from_slice(json).unwrap();
    assert_eq!(values.len(), 58);
    let mut expected = std::collections::BTreeSet::new();
    for label in ["G0", "G1", "G2"] {
        for field in BASE_FIELDS {
            expected.insert((label.to_owned(), (*field).to_owned()));
        }
        if label != "G0" {
            for field in ["Fb", "R"] {
                expected.insert((label.to_owned(), field.to_owned()));
            }
        }
    }
    let mut result = std::collections::BTreeMap::new();
    for value in values {
        assert!(value.byte_length <= 4096);
        assert_eq!(value.hex.len(), value.byte_length.checked_mul(2).unwrap());
        assert!(
            value
                .hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
        let bytes: Vec<_> = (0..value.hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&value.hex[i..i + 2], 16).unwrap())
            .collect();
        assert_eq!(bytes.len(), value.byte_length);
        assert!(
            result.insert((value.label, value.field), bytes).is_none(),
            "duplicate golden field"
        );
    }
    assert_eq!(
        result
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        expected
    );
    result
}

#[test]
fn literal_fixture_has_exact_unique_58_fieldset_and_lengths() {
    assert_eq!(literal_goldens().len(), 58);
}

fn emit_candidates(label: &str, fields: &[(&str, &[u8])]) {
    use std::fmt::Write as _;
    assert!(matches!(label, "G0" | "G1" | "G2"));
    assert!(fields.len() <= 24);
    let frozen = literal_goldens();
    let expected: std::collections::BTreeMap<_, _> = frozen
        .iter()
        .filter(|((fixture, _), _)| fixture == label)
        .map(|((_, field), bytes)| (field.as_str(), bytes.as_slice()))
        .collect();
    let mut actual = std::collections::BTreeMap::new();
    for (field, bytes) in fields {
        assert!(
            actual.insert(*field, *bytes).is_none(),
            "duplicate computed golden field"
        );
    }
    assert_eq!(
        actual.keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>(),
        "literal fieldset {label} changed"
    );
    for (field, bytes) in &actual {
        let frozen = expected[field];
        assert!(
            *bytes == frozen,
            "literal {label}/{field} differs: lengths {}/{}; blake3 {}/{}",
            bytes.len(),
            frozen.len(),
            blake3::hash(bytes),
            blake3::hash(frozen)
        );
    }
    let mut output = String::new();
    let mut total = 0usize;
    for (name, bytes) in fields {
        assert!(bytes.len() <= 4096);
        total = total.checked_add(bytes.len()).unwrap();
        assert!(total <= 16 * 1024);
        write!(
            output,
            "R4_GOLDEN_CANDIDATE {label} {name} bytes={} hex=",
            bytes.len()
        )
        .unwrap();
        for byte in *bytes {
            write!(output, "{byte:02x}").unwrap();
        }
        output.push('\n');
    }
    // At most 48KiB per fixture/144KiB for the three. Build and check before
    // stdout; no file writes or output derived from an implementation result.
    assert!(output.len() <= 48 * 1024);
    print!("{output}");
}
fn file_source(selected: bool) -> ProviderBootstrapSource {
    let f = FileManifest::empty(ChunkingDescriptor::single()).unwrap();
    let entry = ProjectionEntry {
        path: p("README.md"),
        target: Some(TreeEntryTarget::file(f.file_id().clone())),
        executable: false,
        writable: false,
        state: EntryState::Unavailable,
    };
    let selection = if selected {
        ProjectionSelection::selected(vec![p("README.md")], vec![], PathSemantics::PortableWindows)
            .unwrap()
    } else {
        ProjectionSelection::All
    };
    ProviderBootstrapSource::validate(
        snapshot(vec![entry]),
        selection,
        vec![ProviderSourceDescriptor {
            path: p("README.md"),
            manifest: f,
        }],
    )
    .unwrap()
}
fn empty_source() -> ProviderBootstrapSource {
    ProviderBootstrapSource::validate(snapshot(vec![]), ProjectionSelection::All, vec![]).unwrap()
}

fn assert_source_golden(
    label: &str,
    source: &ProviderBootstrapSource,
    s: &[u8],
    e: &[u8],
    r: Option<&[u8]>,
    f: Option<&[u8]>,
    file_id: Option<&str>,
) {
    assert_eq!(source.initial().canonical_bytes().unwrap(), s);
    assert_eq!(
        crate::projection::selection::encode(source.selection()).unwrap(),
        e
    );
    let sd = independent_history(None, s);
    let ed = independent_history(Some(&sd), e);
    let ep = blake3::hash(e).to_hex().to_string();
    let mut descriptors = Vec::new();
    descriptors.extend_from_slice(&(r.is_some() as u64).to_le_bytes());
    if let Some(r) = r {
        descriptors.extend_from_slice(&(r.len() as u64).to_le_bytes());
        descriptors.extend_from_slice(r);
    }
    let dd = independent_hash(b"layrs-projection-provider-descriptors-v1\0", &descriptors);
    let mut vfs = b"layrs-vfs-catalog-v1\0".to_vec();
    vfs.extend_from_slice(&[0; 8]);
    if let Some(file_id) = file_id {
        vfs.extend_from_slice(&9u32.to_le_bytes());
        vfs.extend_from_slice(b"README.md");
        vfs.extend_from_slice(file_id.as_bytes());
        vfs.extend_from_slice(&[0; 8]);
    }
    let vi = independent_hash(b"layrs-file-v1\0", &vfs);
    let count = r.is_some() as u64;
    let wire = r.map_or(0, |r| r.len());
    let logical = f.map_or(0, |f| 231 + f.len());
    let q = format!(r#"{{"descriptorBytes":{logical},"descriptorCount":{count},"descriptorWireBytes":{wire},"descriptorsDigest":"{dd}","schemaVersion":"layrs.projection-provider-catalog.v1","selectionDigest":"{ed}","sourceDigest":"{sd}","sourceGeneration":0,"vfsCatalogId":"{vi}","vfsCatalogVersion":0}}"#).into_bytes();
    assert_eq!(source.catalog().encode().unwrap(), q);
    let qi = independent_hash(b"layrs-projection-provider-catalog-v1\0", &q);
    assert_ne!(qi, vi);
    assert_eq!(source.catalog().id().unwrap(), qi);
    let aggregate = q.len() + logical;
    let qlen = q.len();
    let slen = s.len();
    let elen = e.len();
    let commitment_json = format!(r#"{{"catalogAggregateBytes":{aggregate},"catalogBytes":{qlen},"catalogId":"{qi}","descriptorBytes":{logical},"descriptorCount":{count},"descriptorWireBytes":{wire},"descriptorsDigest":"{dd}","selectionBytes":{elen},"selectionDigest":"{ed}","selectionPayloadDigest":"{ep}","snapshotBytes":{slen},"snapshotDigest":"{sd}","sourceGeneration":0,"vfsCatalogId":"{vi}","vfsCatalogVersion":0}}"#).into_bytes();
    let commitment: SourceCommitment = serde_json::from_slice(&commitment_json).unwrap();
    assert_eq!(source.commitment(), &commitment);
    assert_eq!(
        canonical_json(source.commitment()).unwrap(),
        commitment_json
    );
    if let Some(r) = r {
        assert_eq!(source.descriptors()[0].encode().unwrap(), r);
        assert_eq!(
            source.descriptors()[0].manifest_bytes().unwrap(),
            f.unwrap()
        );
    }
    let selected = e == SELECTED;
    let inputs = fixture_input_map(selected);
    serde_json::from_slice::<serde_json::Value>(&inputs).unwrap();
    let marker = literal_marker(&sd, selected.then_some(ep.as_str()));
    let format_id = independent_hash(b"layrs-file-v1\0", &marker);
    let marker_digest = independent_hash(b"layrs-projection-recovery-marker-v1\0", &marker);
    let stage = stage_bytes();
    let mut control = b"LYRCDIR1".to_vec();
    control.extend_from_slice(&(stage.len() as u32).to_le_bytes());
    control.extend_from_slice(&stage);
    control.extend_from_slice(&(marker.len() as u32).to_le_bytes());
    control.extend_from_slice(format_id.as_bytes());
    let a = allocation(source);
    assert_eq!(a.format.bytes().unwrap(), marker);
    assert_eq!(a.format_file_id.as_str(), format_id);
    assert_eq!(a.stage_identity().unwrap().encode(), stage);
    assert_eq!(prepared(&a).control_identity().unwrap().encode(), control);
    let mut descriptor_preimage = b"layrs-projection-provider-descriptors-v1\0".to_vec();
    descriptor_preimage.extend_from_slice(&descriptors);
    let mut vfs_preimage = b"layrs-file-v1\0".to_vec();
    vfs_preimage.extend_from_slice(&vfs);
    let mut fields = vec![
        ("fixtureInputs", inputs.as_slice()),
        ("S", s),
        ("E", e),
        ("descriptorHashPreimage", descriptor_preimage.as_slice()),
        ("vfsFileIdPreimage", vfs_preimage.as_slice()),
        ("Q", q.as_slice()),
        ("SourceCommitment", commitment_json.as_slice()),
        ("Sd", sd.as_bytes()),
        ("Ed", ed.as_bytes()),
        ("Ep", ep.as_bytes()),
        ("D", dd.as_bytes()),
        ("catalogId", qi.as_bytes()),
        ("vfsCatalogId", vi.as_bytes()),
        ("LYRCSTG1", stage.as_slice()),
        ("FORMAT", marker.as_slice()),
        ("formatFileId", format_id.as_bytes()),
        ("markerDigest", marker_digest.as_bytes()),
        ("LYRCDIR1", control.as_slice()),
    ];
    if let Some(f) = f {
        fields.push(("Fb", f));
    }
    if let Some(r) = r {
        fields.push(("R", r));
    }
    emit_candidates(label, &fields);
}

#[test]
fn g0_empty_source_literal_json_and_independent_framing() {
    let s = empty_source();
    assert_source_golden("G0", &s, &literal_snapshot("[]"), ALL, None, None, None);
    let a = allocation(&s);
    a.require_source_and_member(&s, std::path::Path::new(&a.private.metadata_path))
        .unwrap();
    assert!(
        serde_json::to_value(&a.format)
            .unwrap()
            .get("initialSelectionDigest")
            .is_none()
    );
}

fn g1_literals() -> (String, Vec<u8>, Vec<u8>, Vec<u8>) {
    let file_id = independent_hash(b"layrs-file-v1\0", b"");
    let payload = format!(
        r#"{{"byteLength":0,"chunking":{{"strategy":"single","version":1}},"chunks":[],"fileId":"{file_id}","schemaVersion":"layrs.file-manifest.v1"}}"#
    );
    let manifest_id = independent_hash(b"layrs-file-manifest-v1\0", payload.as_bytes());
    let f = format!(
        r#"{{"byteLength":0,"chunking":{{"strategy":"single","version":1}},"chunks":[],"fileId":"{file_id}","manifestId":"{manifest_id}","schemaVersion":"layrs.file-manifest.v1"}}"#
    );
    let r = format!(
        r#"{{"bytes":0,"executable":false,"fileId":"{file_id}","manifest":{f},"manifestId":"{manifest_id}","path":"README.md","schemaVersion":"layrs.projection-provider-descriptor.v1"}}"#
    );
    let entry = format!(
        r#"[{{"executable":false,"path":"README.md","state":"unavailable","target":{{"fileId":"{file_id}","kind":"file"}},"writable":false}}]"#
    );
    (
        file_id,
        f.into_bytes(),
        r.into_bytes(),
        literal_snapshot(&entry),
    )
}
#[test]
fn g1_manifest_is_object_sql_blob_is_full_manifest_bytes() {
    let (id, f, r, s) = g1_literals();
    let source = file_source(false);
    assert_source_golden("G1", &source, &s, ALL, Some(&r), Some(&f), Some(&id));
    let row = &source.descriptors()[0];
    assert_eq!(row.file_id.as_str(), id);
    assert_eq!(row.manifest.file_id(), &row.file_id);
    assert_eq!(row.bytes, source.vfs_catalog().entries()[0].size);
    assert_eq!(row.bytes, row.manifest.byte_length());
    let mut value: serde_json::Value = serde_json::from_slice(&r).unwrap();
    assert!(value["manifest"].is_object());
    value["manifest"] = serde_json::to_value(&f).unwrap();
    let wrong = canonical_json(&value).unwrap();
    assert!(wrong.len() < DescriptorRecord::MAX_BYTES);
    assert!(
        DescriptorRecord::decode(&wrong).is_err(),
        "array must not become a manifest object"
    );
    let wrong_blob = canonical_json(&f).unwrap();
    assert!(codec::decode::<FileManifest>(&wrong_blob, 256 * 1024).is_err());
}
#[test]
fn g2_selected_golden_keeps_linked_and_payload_digests_distinct() {
    let (id, f, r, s) = g1_literals();
    let source = file_source(true);
    assert_source_golden("G2", &source, &s, SELECTED, Some(&r), Some(&f), Some(&id));
    assert_ne!(
        source.commitment().selection_payload_digest,
        source.commitment().selection_digest
    );
    let a = allocation(&source);
    a.require_source_and_member(&source, std::path::Path::new(&a.private.metadata_path))
        .unwrap();
    let value = serde_json::to_value(&a.format).unwrap();
    assert_eq!(
        value["initialSelectionDigest"],
        source.commitment().selection_payload_digest
    );
    assert_ne!(
        value["initialSelectionDigest"],
        source.commitment().selection_digest
    );
}

fn stage_bytes() -> Vec<u8> {
    let mut bytes = b"LYRCSTG1".to_vec();
    bytes.extend_from_slice(&VOLUME.to_le_bytes());
    for id in [
        CHECKOUT_INDEX,
        STAGE_PARENT_INDEX,
        STAGE_INDEX,
        FORMAT_INDEX,
    ] {
        bytes.extend_from_slice(&id.to_le_bytes());
    }
    for path in [CHECKOUT, STAGE_PARENT, STAGE] {
        bytes.extend_from_slice(&(path.len() as u32).to_le_bytes());
        bytes.extend_from_slice(path.as_bytes());
    }
    bytes
}
fn clone_data<T: Serialize + DeserializeOwned>(value: &T) -> T {
    serde_json::from_slice(&canonical_json(value).unwrap()).unwrap()
}
fn allocation(source: &ProviderBootstrapSource) -> AllocationRecord {
    let member = blake3::hash(WORKTREE.as_bytes()).to_hex();
    let metadata = format!(r"{SYNTHETIC_FAMILY}\worktrees\{member}\metadata");
    let mut marker = serde_json::json!({
        "schemaVersion":"layrs.projection-store-format.v1","magic":"LAYRS_NATIVE_STORE",
        "formatVersion":4,"minReaderVersion":4,
        "identity":{"storeId":STORE,"worktreeId":WORKTREE,"spaceId":SPACE,"origin":ORIGIN,"principalId":PRINCIPAL},
        "checkoutRoot":CHECKOUT,"checkoutIdentity":fixture_directory(CHECKOUT_INDEX),
        "storeIdentity":fixture_directory(STAGE_INDEX),
        "metadataIdentity":fixture_directory(PRIVATE_METADATA_INDEX),
        "initialSnapshotDigest":source.commitment().snapshot_digest,
        "shared":{"root":SYNTHETIC_FAMILY,"incarnation":SYNTHETIC_INCARNATION,"rootIdentity":fixture_directory(FAMILY_ROOT_INDEX),"metadataPath":metadata,"databaseIdentity":fixture_file(PRIVATE_DB_INDEX)}
    });
    if !matches!(source.selection(), ProjectionSelection::All) {
        marker["initialSelectionDigest"] =
            source.commitment().selection_payload_digest.clone().into();
    }
    let bytes = canonical_json(&marker).unwrap();
    let format = format::parse_marker_bytes(&bytes).unwrap();
    AllocationRecord {
        schema_version: "layrs.projection-provider-allocation.v1".into(),
        transaction_id: TX.into(),
        receipt_file_identity: fixture_file(ALLOCATION_RECEIPT_INDEX),
        family: FamilyRecord {
            root: SYNTHETIC_FAMILY.into(),
            canonical_root: SYNTHETIC_FAMILY.into(),
            incarnation: SYNTHETIC_INCARNATION.into(),
            root_identity: fixture_directory(FAMILY_ROOT_INDEX),
            marker_file_identity: fixture_file(FAMILY_MARKER_INDEX),
            marker_digest: ZERO.into(),
            metadata_identity: fixture_directory(FAMILY_METADATA_INDEX),
            catalog_file_identity: fixture_file(FAMILY_CATALOG_INDEX),
        },
        private: PrivateRecord {
            metadata_path: metadata.clone(),
            canonical_metadata_path: metadata,
            metadata_identity: fixture_directory(PRIVATE_METADATA_INDEX),
            provider_control_identity: fixture_directory(PROVIDER_CONTROL_INDEX),
            database_file_identity: fixture_file(PRIVATE_DB_INDEX),
        },
        stage_identity_hex: codec::encode_hex(&stage_bytes()),
        format,
        format_bytes: bytes.len() as u64,
        format_file_id: FileId::from_bytes(&bytes),
        source: clone_data(source.commitment()),
    }
}
fn prepared(a: &AllocationRecord) -> PreparedRecord {
    // Literal protocol fixture, never a callback/physical expected identity.
    let stage = stage_bytes();
    let mut wire = b"LYRCDIR1".to_vec();
    wire.extend_from_slice(&(stage.len() as u32).to_le_bytes());
    wire.extend_from_slice(&stage);
    wire.extend_from_slice(&(a.format_bytes as u32).to_le_bytes());
    wire.extend_from_slice(a.format_file_id.as_str().as_bytes());
    PreparedRecord {
        schema_version: "layrs.projection-provider-prepared.v1".into(),
        transaction_id: TX.into(),
        receipt_file_identity: fixture_file(PREPARED_RECEIPT_INDEX),
        allocation: a.data_reference().unwrap(),
        control_identity_hex: codec::encode_hex(&wire),
    }
}
fn mount(a: &AllocationRecord) -> MountRecord {
    MountRecord {
        schema_version: "layrs.projection-provider-mount.v1".into(),
        transaction_id: TX.into(),
        catalog_id: a.source.catalog_id.clone(),
        phase: MountPhase::Allocating,
        allocation: a.data_reference().unwrap(),
        prepared: None,
    }
}
fn terminal(a: &AllocationRecord, p: Option<&PreparedRecord>) -> AbandonedRecord {
    let eligibility = EligibilityRecord {
        schema_version: "layrs.projection-provider-bootstrap-eligibility.v1".into(),
        registry_at_decision: RegistryAtDecision::Absent,
        private_state: PrivateState::Uninitialized,
    };
    AbandonedRecord {
        schema_version: "layrs.projection-provider-abandoned.v1".into(),
        transaction_id: TX.into(),
        receipt_file_identity: fixture_file(TERMINAL_RECEIPT_INDEX),
        allocation: a.data_reference().unwrap(),
        prepared: p.map(|p| p.data_reference().unwrap()),
        eligibility_digest: eligibility.digest().unwrap(),
        eligibility,
    }
}
fn binding(
    a: &AllocationRecord,
    p: Option<&PreparedRecord>,
    t: Option<&AbandonedRecord>,
) -> ProviderBootstrapBinding {
    ProviderBootstrapBinding {
        schema_version: "layrs.projection-provider-bootstrap-binding.v1".into(),
        recovery: RecoveryBinding {
            identity: a.format.identity.clone(),
            checkout_identity: a.format.checkout_identity().into(),
            marker_digest: independent_hash(
                b"layrs-projection-recovery-marker-v1\0",
                &a.format.bytes().unwrap(),
            ),
            database_identity: a.private.database_file_identity.clone(),
            family_marker_digest: Some(a.family.marker_digest.clone()),
        },
        transaction_id: TX.into(),
        allocation: a.data_reference().unwrap(),
        prepared: p.map(|p| PreparedBinding {
            receipt: p.data_reference().unwrap(),
            control_identity_hex: p.control_identity_hex.clone(),
        }),
        abandoned: t.map(|t| t.data_reference().unwrap()),
    }
}

#[test]
fn all_three_receipts_roundtrip_with_distinct_domain_references() {
    let a = allocation(&empty_source());
    let p = prepared(&a);
    let t = terminal(&a, Some(&p));
    assert_eq!(AllocationRecord::decode(&a.encode().unwrap()).unwrap(), a);
    assert_eq!(PreparedRecord::decode(&p.encode().unwrap()).unwrap(), p);
    assert_eq!(AbandonedRecord::decode(&t.encode().unwrap()).unwrap(), t);
    p.require_allocation(&a).unwrap();
    t.require_references(&a, Some(&p), None).unwrap();
    for (domain, bytes, reference) in [
        (
            ALLOCATION_DOMAIN,
            a.encode().unwrap(),
            a.data_reference().unwrap(),
        ),
        (
            PREPARED_DOMAIN,
            p.encode().unwrap(),
            p.data_reference().unwrap(),
        ),
        (
            ABANDONED_DOMAIN,
            t.encode().unwrap(),
            t.data_reference().unwrap(),
        ),
    ] {
        let mut prefix = domain.as_bytes().to_vec();
        prefix.push(0);
        assert_eq!(reference.digest, independent_hash(&prefix, &bytes));
        assert_eq!(reference.byte_length, bytes.len() as u64);
    }
}
#[test]
fn allocating_mount_never_hides_existing_prepared_in_binding_or_terminal() {
    let a = allocation(&empty_source());
    let p = prepared(&a);
    let m = mount(&a);
    m.require_references(&a, Some(&p)).unwrap();
    assert!(m.prepared.is_none());
    let t = terminal(&a, Some(&p));
    let b = binding(&a, Some(&p), Some(&t));
    assert_eq!(
        b.require_references(&a, Some(&p), Some(&t)).unwrap(),
        ReceiptBindingCheck::TerminalPendingValidation
    );
    assert_eq!(
        binding(&a, Some(&p), None)
            .require_references(&a, Some(&p), None)
            .unwrap(),
        ReceiptBindingCheck::NoTerminal
    );
    assert!(b.prepared.is_some());
    assert!(t.prepared.is_some());
    assert!(
        b.require_references(&a, None, Some(&t))
            .unwrap_err()
            .to_string()
            .contains("rebind")
    );
    assert!(t.require_references(&a, None, None).is_err());
    assert!(
        binding(&a, None, None)
            .require_references(&a, Some(&p), None)
            .is_err()
    );
}
#[test]
fn nullable_fields_are_required_not_missing_and_unknown_duplicate_noncanonical_refuse() {
    let a = allocation(&empty_source());
    let m = mount(&a);
    let valid = m.encode().unwrap();
    assert!(MountRecord::decode(&valid).is_ok());
    let mut value = serde_json::to_value(&m).unwrap();
    value.as_object_mut().unwrap().remove("prepared");
    assert!(
        MountRecord::decode(&canonical_json(&value).unwrap())
            .unwrap_err()
            .to_string()
            .contains("missing field")
    );
    let mut value = serde_json::to_value(&m).unwrap();
    value["unknown"] = true.into();
    assert!(
        MountRecord::decode(&canonical_json(&value).unwrap())
            .unwrap_err()
            .to_string()
            .contains("unknown field")
    );
    let mut duplicate = b"{\"prepared\":null,".to_vec();
    duplicate.extend_from_slice(&valid[1..]);
    assert!(
        MountRecord::decode(&duplicate)
            .unwrap_err()
            .to_string()
            .contains("duplicate field")
    );
    let mut newline = valid.clone();
    newline.push(b'\n');
    assert!(MountRecord::decode(&newline).is_err());
    let t = terminal(&a, None);
    let mut value = serde_json::to_value(&t).unwrap();
    value.as_object_mut().unwrap().remove("prepared");
    assert!(AbandonedRecord::decode(&canonical_json(&value).unwrap()).is_err());
    let b = binding(&a, None, None);
    for field in ["prepared", "abandoned"] {
        let mut value = serde_json::to_value(&b).unwrap();
        value.as_object_mut().unwrap().remove(field);
        assert!(ProviderBootstrapBinding::decode(&canonical_json(&value).unwrap()).is_err());
    }
    assert!(AllocationRecord::decode(&vec![b' '; AllocationRecord::MAX_BYTES + 1]).is_err());
}
#[test]
fn canonical_numeric_roundtrip_preserves_exact_u64_literals_and_limits() {
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Number {
        value: u64,
    }
    // Pinned serde_jcs 0.1.0 keeps the integer path: no blanket 2^53 policy.
    let cases: &[(u64, &[u8])] = &[
        (0, br#"{"value":0}"#),
        (255, br#"{"value":255}"#),
        (256, br#"{"value":256}"#),
        (9_007_199_254_740_991, br#"{"value":9007199254740991}"#),
        (9_007_199_254_740_992, br#"{"value":9007199254740992}"#),
        (9_007_199_254_740_993, br#"{"value":9007199254740993}"#),
        (i64::MAX as u64, br#"{"value":9223372036854775807}"#),
        (u64::MAX, br#"{"value":18446744073709551615}"#),
    ];
    for &(value, literal) in cases {
        let number = Number { value };
        assert_eq!(canonical_json(&number).unwrap(), literal);
        assert_eq!(codec::encode(&number, literal.len()).unwrap(), literal);
        assert_eq!(
            codec::decode::<Number>(literal, literal.len()).unwrap(),
            number
        );
        assert!(codec::encode(&number, literal.len() - 1).is_err());
        assert!(codec::decode::<Number>(literal, literal.len() - 1).is_err());
    }
    for bad in [
        br#"{"value":-1}"#.as_slice(),
        br#"{"value":1.5}"#.as_slice(),
        br#"{"value":18446744073709551616}"#.as_slice(),
        br#"{"value":1e0}"#.as_slice(),
    ] {
        assert!(codec::decode::<Number>(bad, 128).is_err());
    }
    assert_eq!(codec::add(u64::MAX - 1, 1).unwrap(), u64::MAX);
    assert_eq!(
        codec::add(u64::MAX, 1).unwrap_err().to_string(),
        "record accounting overflow"
    );
}
#[test]
fn canonical_roundtrip_guard_rejects_controlled_lossy_serialization() {
    #[derive(Debug, PartialEq, Deserialize)]
    struct LossyNumber {
        value: u64,
    }
    impl Serialize for LossyNumber {
        fn serialize<S: serde::Serializer>(
            &self,
            serializer: S,
        ) -> std::result::Result<S::Ok, S::Error> {
            use serde::ser::SerializeStruct;
            let mut record = serializer.serialize_struct("LossyNumber", 1)?;
            // Deliberately faulty TEST serializer, not ordinary u64 behavior.
            record.serialize_field("value", &(self.value as f64))?;
            record.end()
        }
    }
    let exact = LossyNumber {
        value: 9_007_199_254_740_992,
    };
    let literal = br#"{"value":9007199254740992}"#;
    assert_eq!(codec::encode(&exact, 128).unwrap(), literal);
    assert_eq!(codec::decode::<LossyNumber>(literal, 128).unwrap(), exact);
    let lossy = LossyNumber {
        value: 9_007_199_254_740_993,
    };
    assert_eq!(
        codec::encode(&lossy, 128).unwrap_err().to_string(),
        "canonical record changes typed value"
    );
}
#[test]
fn descriptor_and_vfs_size_limits_are_independent_of_u64_encoding() {
    for bytes in [i64::MAX as u64, i64::MAX as u64 + 1] {
        let manifest = FileManifest::new(
            FileId::parse(ZERO).unwrap(),
            bytes,
            ChunkingDescriptor::single(),
            vec![FileChunk::new(ChunkId::parse(ZERO).unwrap(), 0, bytes).unwrap()],
        )
        .unwrap();
        let row = DescriptorRecord {
            schema_version: "layrs.projection-provider-descriptor.v1".into(),
            path: p("sized-file"),
            file_id: manifest.file_id().clone(),
            bytes,
            executable: false,
            manifest_id: manifest.id().clone(),
            manifest,
        };
        // All redundant fields match; only the independent size bound differs.
        assert_eq!(row.file_id, *row.manifest.file_id());
        assert_eq!(row.manifest_id, *row.manifest.id());
        assert_eq!(row.bytes, row.manifest.byte_length());
        let catalog = layrs_vfs::Catalog::new(
            0,
            vec![layrs_vfs::CatalogEntry {
                path: row.path.clone(),
                file_id: row.file_id.clone(),
                size: bytes,
            }],
        );
        if bytes == i64::MAX as u64 {
            row.validate_fields().unwrap();
            assert!(catalog.is_ok());
        } else {
            assert_eq!(
                row.validate_fields().unwrap_err().to_string(),
                "descriptor projection differs from manifest or exceeds bounds"
            );
            assert!(matches!(catalog, Err(layrs_vfs::Error::InvalidRange)));
        }
    }
}
#[test]
fn native_transaction_and_identity_hex_tokens_are_exact_data_only() {
    for t in [
        "",
        "0123456789ABCDEF0123456789abcdef",
        "0123456789abcdef0123456789abcdef00",
    ] {
        assert!(codec::transaction(t).is_err());
    }
    for f in ["01:0:1", "1:00:1", "1:0:4294967296", "1:0", "1:0:1:2"] {
        assert!(codec::file_identity(f).is_err());
    }
    for d in ["fs:v1:7:1", "fs:v1:000000000000000A:0000000000000001"] {
        assert!(codec::directory_identity(d).is_err());
    }
    assert!(codec::file_identity("4294967295:4294967295:4294967295").is_ok());
    let a = allocation(&empty_source());
    a.stage_identity().unwrap();
    let mut bad: AllocationRecord = clone_data(&a);
    bad.stage_identity_hex.push_str("00");
    assert!(bad.validate_fields().is_err());
    let mut bad: AllocationRecord = clone_data(&a);
    bad.stage_identity_hex = bad.stage_identity_hex.to_uppercase();
    assert!(bad.validate_fields().is_err());
    let mut bad = prepared(&a);
    bad.control_identity_hex.push_str("00");
    assert!(bad.require_allocation(&a).is_err());
    let mut bad = prepared(&a);
    bad.allocation.file_identity = fixture_file(99);
    assert!(bad.require_allocation(&a).is_err());
}
#[test]
fn source_refuses_materialized_wrong_missing_duplicate_and_unselected_descriptors() {
    let f = FileManifest::empty(ChunkingDescriptor::single()).unwrap();
    let base = ProjectionEntry {
        path: p("README.md"),
        target: Some(TreeEntryTarget::file(f.file_id().clone())),
        executable: false,
        writable: false,
        state: EntryState::Unavailable,
    };
    let mut dirty = base.clone();
    dirty.state = EntryState::Materialized;
    let error =
        ProviderBootstrapSource::validate(snapshot(vec![dirty]), ProjectionSelection::All, vec![])
            .err()
            .unwrap();
    assert!(error.to_string().contains("Materialized"));
    assert!(
        ProviderBootstrapSource::validate(
            snapshot(vec![base.clone()]),
            ProjectionSelection::All,
            vec![]
        )
        .is_err()
    );
    let descriptor = || ProviderSourceDescriptor {
        path: p("README.md"),
        manifest: f.clone(),
    };
    assert!(
        ProviderBootstrapSource::validate(
            snapshot(vec![base.clone()]),
            ProjectionSelection::All,
            vec![descriptor(), descriptor()]
        )
        .is_err()
    );
    let none =
        ProjectionSelection::selected(vec![], vec![], PathSemantics::PortableWindows).unwrap();
    assert!(
        ProviderBootstrapSource::validate(snapshot(vec![base.clone()]), none, vec![descriptor()])
            .is_err()
    );
    let mut wrong = base;
    wrong.target = Some(TreeEntryTarget::file(FileId::parse(ZERO).unwrap()));
    assert!(
        ProviderBootstrapSource::validate(
            snapshot(vec![wrong]),
            ProjectionSelection::All,
            vec![descriptor()]
        )
        .is_err()
    );
}
#[test]
fn selection_absence_and_real_vfs_namespace_bounds_are_used() {
    let absent = ProjectionEntry {
        path: p("gone"),
        target: None,
        executable: false,
        writable: false,
        state: EntryState::Absent,
    };
    assert!(
        ProviderBootstrapSource::validate(snapshot(vec![absent]), ProjectionSelection::All, vec![])
            .is_ok()
    );
    let tree = ProjectionEntry {
        path: p("dir"),
        target: Some(TreeEntryTarget::tree(TreeId::parse(ZERO).unwrap())),
        executable: false,
        writable: false,
        state: EntryState::Unavailable,
    };
    assert!(
        ProviderBootstrapSource::validate(snapshot(vec![tree]), ProjectionSelection::All, vec![])
            .is_err()
    );
    // Valid portable core name, deliberately unsupported by the real VFS v1
    // ASCII namespace; proves that a Store-only substitute did not accept it.
    let f = FileManifest::empty(ChunkingDescriptor::single()).unwrap();
    let entry = ProjectionEntry {
        path: p("é.txt"),
        target: Some(TreeEntryTarget::file(f.file_id().clone())),
        executable: false,
        writable: false,
        state: EntryState::Unavailable,
    };
    let s = snapshot(vec![entry]);
    assert!(s.validate().is_ok());
    assert!(
        ProviderBootstrapSource::validate(
            s,
            ProjectionSelection::All,
            vec![ProviderSourceDescriptor {
                path: p("é.txt"),
                manifest: f
            }]
        )
        .is_err()
    );
}
#[test]
fn budgets_and_commitment_recomputation_do_not_trust_counter_fields() {
    let source = file_source(false);
    let mut c: SourceCommitment = clone_data(source.commitment());
    c.descriptor_bytes += 1;
    c.catalog_aggregate_bytes += 1;
    assert!(c.validate_fields().is_ok());
    assert!(source.require_commitment(&c).is_err());
    c.catalog_aggregate_bytes = u64::MAX;
    assert!(c.validate_fields().is_err());
    assert!(codec::add(u64::MAX, 1).is_err());
    let mut c: SourceCommitment = clone_data(source.commitment());
    c.descriptor_count = 100_001;
    assert!(c.validate_fields().is_err());
    let mut c: SourceCommitment = clone_data(source.commitment());
    c.source_generation = 1;
    assert!(c.validate_fields().is_err());
    let mut c: SourceCommitment = clone_data(source.commitment());
    c.catalog_bytes = 64 * 1024 * 1024 + 1;
    assert!(c.validate_fields().is_err());
    // Preflight stops serialization before materializing a second huge encoded
    // value. Small discriminating limit exercises that mechanism, not a large
    // synthetic allocation that would disturb the coordinator's quiet slot.
    assert!(codec::preflight(&"x".repeat(129), 128).is_err());
}
#[test]
fn real_manifest_over_row_budget_refuses_before_descriptor_retention() {
    let chunks = (0..4096u64)
        .map(|offset| FileChunk::new(ChunkId::parse(ZERO).unwrap(), offset, 1).unwrap())
        .collect();
    // A structurally valid metadata manifest with deliberately unverified
    // content IDs. The test asserts metadata size refusal, not content safety.
    let f = FileManifest::new(
        FileId::parse(ZERO).unwrap(),
        4096,
        ChunkingDescriptor::single(),
        chunks,
    )
    .unwrap();
    assert!(canonical_json(&f).unwrap().len() > 256 * 1024);
    let e = ProjectionEntry {
        path: p("large-meta"),
        target: Some(TreeEntryTarget::file(f.file_id().clone())),
        executable: false,
        writable: false,
        state: EntryState::Unavailable,
    };
    let err = ProviderBootstrapSource::validate(
        snapshot(vec![e]),
        ProjectionSelection::All,
        vec![ProviderSourceDescriptor {
            path: p("large-meta"),
            manifest: f,
        }],
    )
    .err()
    .unwrap();
    assert!(err.to_string().contains("budget"));
}
#[test]
fn prepared_codec_comparison_is_exact_including_path_case() {
    let a = allocation(&empty_source());
    let mut p = prepared(&a);
    let mut wire = codec::decode_hex(
        &p.control_identity_hex,
        layrs_vfs::CONTROL_DIRECTORY_MAX_ENCODED_BYTES,
    )
    .unwrap();
    let at = wire
        .windows(b"checkout".len())
        .position(|s| s == b"checkout")
        .unwrap();
    wire[at] = b'C';
    p.control_identity_hex = codec::encode_hex(&wire);
    p.control_identity().unwrap(); // valid syntax, different comparison data
    assert!(
        p.require_allocation(&a)
            .unwrap_err()
            .to_string()
            .contains("stage binding")
    );
}
#[test]
fn later_mount_phases_and_terminal_digest_changes_refuse() {
    let a = allocation(&empty_source());
    let m = mount(&a);
    for phase in [
        "provider_prepared",
        "active",
        "retiring",
        "retired",
        "abandoned",
    ] {
        let mut v = serde_json::to_value(&m).unwrap();
        v["phase"] = phase.into();
        assert!(MountRecord::decode(&canonical_json(&v).unwrap()).is_err());
    }
    let mut m = mount(&a);
    m.phase = MountPhase::ControlPrepared;
    assert!(m.encode().is_err());
    let mut t = terminal(&a, None);
    t.eligibility.registry_at_decision = RegistryAtDecision::Initializing;
    assert!(t.encode().is_err());
}

#[test]
fn bounded_row_data_checks_cardinality_and_backend_before_phase() {
    let s = empty_source();
    let a = allocation(&s);
    let mut m = mount(&a);
    // Deliberately invalid phase data: wrong backend must be detected first.
    m.phase = MountPhase::ControlPrepared;
    let catalogs = [CatalogRow {
        id: &s.commitment().catalog_id,
        source_generation: 0,
        source_digest: &s.commitment().snapshot_digest,
        selection_digest: &s.commitment().selection_digest,
        record: s.catalog(),
    }];
    let mounts = [MountRow {
        id: TX,
        catalog_id: &s.commitment().catalog_id,
        phase: m.phase,
        digest: ZERO,
        record: &m,
    }];
    let wrong = [BackendRow {
        singleton: 1,
        backend: BackendKind::Ordinary,
        mount_id: None,
    }];
    let error =
        require_bootstrap_row_data(&wrong, &catalogs, &[], &mounts, &s, &a, None).unwrap_err();
    assert!(error.to_string().contains("backend/mount binding"));
    assert!(
        require_bootstrap_row_data(&[], &catalogs, &[], &mounts, &s, &a, None)
            .unwrap_err()
            .to_string()
            .contains("one backend/catalog/mount")
    );
    let m = mount(&a);
    let digest = m.digest().unwrap();
    let mounts = [MountRow {
        id: TX,
        catalog_id: &s.commitment().catalog_id,
        phase: m.phase,
        digest: &digest,
        record: &m,
    }];
    let right = [BackendRow {
        singleton: 1,
        backend: BackendKind::Provider,
        mount_id: Some(TX.into()),
    }];
    require_bootstrap_row_data(&right, &catalogs, &[], &mounts, &s, &a, None).unwrap();
    // An extra backend row cannot occur in exact SQL (PK/CHECK). Do not pretend
    // this pure slice fixture proves a physically possible extra-SQL-row case.
    // G2's FK-valid second catalog+mount fixture remains later SQL test work.
}

fn assert_invalid<T>(result: Result<T>, expected: &str) {
    match result {
        Err(crate::projection::ProjectionStoreError::Invalid(cause)) => assert_eq!(cause, expected),
        Err(other) => panic!("expected Invalid({expected:?}), got {other}"),
        Ok(_) => panic!("expected Invalid({expected:?}), got success"),
    }
}
fn edit_marker(a: &mut AllocationRecord, edit: impl FnOnce(&mut serde_json::Value)) {
    let mut value = serde_json::to_value(&a.format).unwrap();
    edit(&mut value);
    a.format = serde_json::from_value(value).unwrap();
    let bytes = a.format.bytes().unwrap();
    a.format_bytes = bytes.len() as u64;
    a.format_file_id = FileId::from_bytes(&bytes);
}
fn refresh_terminal(t: &mut AbandonedRecord) {
    t.eligibility_digest = t.eligibility.digest().unwrap();
}
fn initialized_terminal(
    a: &AllocationRecord,
    p: Option<&PreparedRecord>,
    m: &MountRecord,
) -> AbandonedRecord {
    let mut t = terminal(a, p);
    t.eligibility.registry_at_decision = RegistryAtDecision::Initializing;
    t.eligibility.private_state = PrivateState::InitialSchema8 {
        catalog_id: a.source.catalog_id.clone(),
        snapshot_digest: a.source.snapshot_digest.clone(),
        selection_digest: a.source.selection_digest.clone(),
        mount_digest: m.digest().unwrap(),
    };
    refresh_terminal(&mut t);
    t
}
fn descriptor_rows<'a>(
    source: &'a ProviderBootstrapSource,
    blobs: &'a [Vec<u8>],
) -> Vec<DescriptorRow<'a>> {
    assert_eq!(source.descriptors().len(), blobs.len());
    source
        .descriptors()
        .iter()
        .zip(blobs)
        .map(|(r, bytes)| DescriptorRow {
            catalog_id: &source.commitment().catalog_id,
            path: r.path.as_str(),
            file_id: r.file_id.as_str(),
            bytes: r.bytes,
            executable: r.executable,
            manifest_id: r.manifest_id.as_str(),
            manifest: bytes,
        })
        .collect()
}
fn check_rows(
    source: &ProviderBootstrapSource,
    a: &AllocationRecord,
    p: Option<&PreparedRecord>,
    m: &MountRecord,
    descriptors: &[DescriptorRow<'_>],
) -> Result<()> {
    let digest = m.digest()?;
    require_bootstrap_row_data(
        &[BackendRow {
            singleton: 1,
            backend: BackendKind::Provider,
            mount_id: Some(a.transaction_id.clone()),
        }],
        &[CatalogRow {
            id: &source.commitment().catalog_id,
            source_generation: 0,
            source_digest: &source.commitment().snapshot_digest,
            selection_digest: &source.commitment().selection_digest,
            record: source.catalog(),
        }],
        descriptors,
        &[MountRow {
            id: &a.transaction_id,
            catalog_id: &source.commitment().catalog_id,
            phase: m.phase,
            digest: &digest,
            record: m,
        }],
        source,
        a,
        p,
    )
}

#[test]
fn allocation_self_contained_all_ep_and_member_derivation_refuse_specific_mismatches() {
    let all = allocation(&empty_source());
    let selected = allocation(&file_source(true));
    all.encode().unwrap();
    selected.encode().unwrap();
    let selection_error = "allocation Marker/source selection commitment differs";
    let mut wrong = clone_data(&selected);
    let ed = wrong.source.selection_digest.clone();
    edit_marker(&mut wrong, |v| v["initialSelectionDigest"] = ed.into());
    assert_invalid(wrong.encode(), selection_error);
    let mut wrong = clone_data(&selected);
    edit_marker(&mut wrong, |v| {
        v.as_object_mut().unwrap().remove("initialSelectionDigest");
    });
    assert_invalid(wrong.encode(), selection_error);
    let mut wrong = clone_data(&all);
    let ep = wrong.source.selection_payload_digest.clone();
    edit_marker(&mut wrong, |v| v["initialSelectionDigest"] = ep.into());
    assert_invalid(wrong.encode(), selection_error);
    for field in [
        "selectionBytes",
        "selectionDigest",
        "selectionPayloadDigest",
    ] {
        let mut value = serde_json::to_value(&all).unwrap();
        value["source"][field] = if field == "selectionBytes" {
            (ALL.len() as u64 + 1).into()
        } else {
            STEP[7..].into()
        };
        let wrong: AllocationRecord = serde_json::from_value(value).unwrap();
        assert_invalid(wrong.encode(), selection_error);
    }
    // Both serialized locators agree with each other, but not the independently
    // derived member. Updating FORMAT length/id excludes a stale-outer-hash cause.
    let changed_case = all
        .private
        .metadata_path
        .replace("C:\\family", "c:\\family");
    for metadata in [r"C:\family\worktrees\wrong\metadata", changed_case.as_str()] {
        let mut wrong = clone_data(&all);
        wrong.private.metadata_path = metadata.into();
        edit_marker(&mut wrong, |v| {
            v["shared"]["metadataPath"] = metadata.into()
        });
        assert_invalid(
            wrong.encode(),
            "allocation deterministic member metadata differs",
        );
    }
}

#[test]
fn allocation_identity_import_and_legacy_marker_refuse_with_refreshed_format_commitment() {
    let a = allocation(&empty_source());
    let mut wrong = clone_data(&a);
    edit_marker(&mut wrong, |v| v["identity"]["principalId"] = "".into());
    assert_invalid(wrong.encode(), "invalid projection Principal identity");
    let mut wrong = clone_data(&a);
    edit_marker(&mut wrong, |v| v["importDigest"] = STEP.into());
    assert_invalid(
        wrong.encode(),
        "allocation FORMAT/source/native comparison fields differ",
    );
    let mut wrong = clone_data(&a);
    edit_marker(&mut wrong, |v| {
        v["formatVersion"] = 3.into();
        v["minReaderVersion"] = 3.into();
        v.as_object_mut().unwrap().remove("shared");
    });
    assert_invalid(wrong.encode(), "provider allocation requires format4");
}

#[test]
fn prepared_published_and_initialized_receipts_compare_successfully_but_binding_stays_pending() {
    for selected in [false, true] {
        let source = file_source(selected);
        let a = allocation(&source);
        let p = prepared(&a);
        let blobs: Vec<_> = source
            .descriptors()
            .iter()
            .map(|r| r.manifest_bytes().unwrap())
            .collect();
        let rows = descriptor_rows(&source, &blobs);
        for phase in [
            MountPhase::Allocating,
            MountPhase::ControlPrepared,
            MountPhase::ControlPublished,
        ] {
            let mut m = mount(&a);
            m.phase = phase;
            if phase != MountPhase::Allocating {
                m.prepared = Some(p.data_reference().unwrap());
            }
            assert_eq!(MountRecord::decode(&m.encode().unwrap()).unwrap(), m);
            m.require_references(&a, Some(&p)).unwrap();
            check_rows(&source, &a, Some(&p), &m, &rows).unwrap();
            let t = initialized_terminal(&a, Some(&p), &m);
            assert_eq!(AbandonedRecord::decode(&t.encode().unwrap()).unwrap(), t);
            t.require_references(&a, Some(&p), Some(&m)).unwrap();
            assert_eq!(
                binding(&a, Some(&p), Some(&t))
                    .require_references(&a, Some(&p), Some(&t))
                    .unwrap(),
                ReceiptBindingCheck::TerminalPendingValidation
            );
            assert_eq!(
                binding(&a, Some(&p), None)
                    .require_references(&a, Some(&p), None)
                    .unwrap(),
                ReceiptBindingCheck::NoTerminal
            );
        }
    }
}

#[test]
fn hot_terminal_rejects_a_derived_contradictions_but_never_synthesizes_unknown_mount() {
    let a = allocation(&file_source(true));
    let p = prepared(&a);
    let mut m = mount(&a);
    m.phase = MountPhase::ControlPrepared;
    m.prepared = Some(p.data_reference().unwrap());
    let t = initialized_terminal(&a, Some(&p), &m);
    for field in [
        "catalogId",
        "snapshotDigest",
        "selectionDigest",
        "mountDigest",
    ] {
        let mut value = serde_json::to_value(&t).unwrap();
        value["eligibility"]["privateState"][field] =
            if matches!(field, "snapshotDigest" | "selectionDigest") {
                STEP[7..].into()
            } else {
                STEP.into()
            };
        let mut changed: AbandonedRecord = serde_json::from_value(value).unwrap();
        refresh_terminal(&mut changed);
        // Refresh T's outer reference too: an obsolete receipt hash cannot be
        // the reason these independent source/mount comparisons refuse.
        let b = binding(&a, Some(&p), Some(&changed));
        if field == "mountDigest" {
            assert_eq!(
                b.require_references(&a, Some(&p), Some(&changed)).unwrap(),
                ReceiptBindingCheck::TerminalPendingValidation
            );
            assert_invalid(
                changed.require_references(&a, Some(&p), Some(&m)),
                "terminal initialized mount commitment differs",
            );
            assert_invalid(
                changed.require_references(&a, Some(&p), None),
                "initialized eligibility lacks mount data",
            );
        } else {
            assert_invalid(
                b.require_references(&a, Some(&p), Some(&changed)),
                "terminal allocation source commitment differs",
            );
            assert_invalid(
                changed.require_references(&a, Some(&p), None),
                "terminal allocation source commitment differs",
            );
        }
    }
    assert_eq!(
        binding(&a, Some(&p), Some(&t))
            .require_references(&a, Some(&p), Some(&t))
            .unwrap(),
        ReceiptBindingCheck::TerminalPendingValidation
    );
    assert_invalid(
        t.require_references(&a, Some(&p), None),
        "initialized eligibility lacks mount data",
    );
    let empty = terminal(&a, Some(&p));
    assert_eq!(
        binding(&a, Some(&p), Some(&empty))
            .require_references(&a, Some(&p), Some(&empty))
            .unwrap(),
        ReceiptBindingCheck::TerminalPendingValidation
    );
    assert_invalid(
        empty.require_references(&a, Some(&p), Some(&m)),
        "uninitialized eligibility has mount data",
    );
    empty.require_references(&a, Some(&p), None).unwrap(); // DATA for proven-uninitialized only.
}

#[test]
fn prepared_and_binding_commitment_mutations_have_specific_refusals() {
    let a = allocation(&empty_source());
    let p = prepared(&a);
    let t = terminal(&a, Some(&p));
    let b = binding(&a, Some(&p), Some(&t));
    for which in ["allocation", "transaction", "content"] {
        let mut changed: PreparedRecord = clone_data(&p);
        let expected = match which {
            "allocation" => {
                changed.allocation.digest = STEP.into();
                "prepared allocation reference differs"
            }
            "transaction" => {
                changed.transaction_id = "00000000000000000000000000000002".into();
                "prepared allocation reference differs"
            }
            _ => {
                let mut bytes = changed.control_identity().unwrap().encode();
                let last = bytes.last_mut().unwrap();
                *last = if *last == b'0' { b'1' } else { b'0' };
                changed.control_identity_hex = codec::encode_hex(&bytes);
                "prepared FORMAT size/content differs"
            }
        };
        changed.encode().unwrap();
        assert_invalid(changed.require_allocation(&a), expected);
    }
    for field in [
        "markerDigest",
        "familyMarkerDigest",
        "databaseIdentity",
        "checkoutIdentity",
    ] {
        let mut value = serde_json::to_value(&b).unwrap();
        value["recovery"][field] = match field {
            "databaseIdentity" => "1:0:999".into(),
            "checkoutIdentity" => fixture_directory(999).into(),
            _ => STEP.into(),
        };
        let changed: ProviderBootstrapBinding = serde_json::from_value(value).unwrap();
        changed.encode().unwrap();
        assert_invalid(
            changed.require_references(&a, Some(&p), Some(&t)),
            "bootstrap binding allocation differs",
        );
    }
    let mut changed: ProviderBootstrapBinding = clone_data(&b);
    changed.prepared.as_mut().unwrap().receipt.digest = STEP.into();
    assert_invalid(
        changed.require_references(&a, Some(&p), Some(&t)),
        "bootstrap prepared binding changed; rebind required",
    );
    let mut changed: ProviderBootstrapBinding = clone_data(&b);
    changed.abandoned.as_mut().unwrap().digest = STEP.into();
    assert_invalid(
        changed.require_references(&a, Some(&p), Some(&t)),
        "bootstrap terminal changed; rebind required",
    );
}

fn regular_entry(name: RepoPath, manifest: &FileManifest) -> ProjectionEntry {
    ProjectionEntry {
        path: name,
        target: Some(TreeEntryTarget::file(manifest.file_id().clone())),
        executable: false,
        writable: false,
        state: EntryState::Unavailable,
    }
}
fn files_source(names: &[&str]) -> ProviderBootstrapSource {
    let f = FileManifest::empty(ChunkingDescriptor::single()).unwrap();
    ProviderBootstrapSource::validate(
        snapshot(
            names
                .iter()
                .map(|name| regular_entry(p(name), &f))
                .collect(),
        ),
        ProjectionSelection::All,
        names
            .iter()
            .map(|name| ProviderSourceDescriptor {
                path: p(name),
                manifest: f.clone(),
            })
            .collect(),
    )
    .unwrap()
}

#[test]
fn descriptor_rows_require_all_seven_projections_canonical_bytes_count_and_order() {
    let source = files_source(&["a", "b"]);
    let a = allocation(&source);
    let m = mount(&a);
    let blobs: Vec<_> = source
        .descriptors()
        .iter()
        .map(|r| r.manifest_bytes().unwrap())
        .collect();
    let original = descriptor_rows(&source, &blobs);
    check_rows(&source, &a, None, &m, &original).unwrap();
    assert_invalid(
        check_rows(&source, &a, None, &m, &original[..1]),
        "provider descriptor row count differs",
    );
    let mut changed = original.clone();
    changed.push(original[0]);
    assert_invalid(
        check_rows(&source, &a, None, &m, &changed),
        "provider descriptor row count differs",
    );
    for field in [
        "catalog",
        "path",
        "file",
        "bytes",
        "mode",
        "manifest_id",
        "order",
        "duplicate",
    ] {
        let mut changed = original.clone();
        match field {
            "catalog" => changed[0].catalog_id = STEP,
            "path" => changed[0].path = "other",
            "file" => changed[0].file_id = STEP,
            "bytes" => changed[0].bytes = 1,
            "mode" => changed[0].executable = true,
            "manifest_id" => changed[0].manifest_id = STEP,
            "order" => changed.swap(0, 1),
            _ => changed[1] = changed[0],
        }
        assert_invalid(
            check_rows(&source, &a, None, &m, &changed),
            "provider descriptor row projection/order differs",
        );
    }
    let array_blob = canonical_json(&blobs[0]).unwrap();
    let mut whitespace_blob = blobs[0].clone();
    whitespace_blob.push(b' ');
    for bytes in [&array_blob, &whitespace_blob] {
        let mut changed = original.clone();
        changed[0].manifest = bytes;
        assert_invalid(
            check_rows(&source, &a, None, &m, &changed),
            "provider descriptor manifest bytes differ",
        );
    }
    let oversized =
        vec![b' '; crate::projection::provider::PROVIDER_DESCRIPTOR_MAX_BYTES as usize + 1];
    let mut changed = original.clone();
    changed[0].manifest = &oversized;
    assert_invalid(
        check_rows(&source, &a, None, &m, &changed),
        "provider descriptor row exceeds bounds",
    );
    // Source metadata IDs are also checked independently of SQL-shaped slices.
    for field in ["fileId", "manifestId"] {
        let mut value = serde_json::to_value(&source.descriptors()[0]).unwrap();
        value[field] = STEP.into();
        let changed: DescriptorRecord = serde_json::from_value(value).unwrap();
        assert_invalid(
            changed.encode(),
            "descriptor projection differs from manifest or exceeds bounds",
        );
    }
}

#[test]
fn catalog_mount_and_actual_selection_data_are_checked_before_acceptance() {
    let source = file_source(true);
    let a = allocation(&source);
    let p = prepared(&a);
    let mut m = mount(&a);
    m.phase = MountPhase::ControlPrepared;
    m.prepared = Some(p.data_reference().unwrap());
    let blobs: Vec<_> = source
        .descriptors()
        .iter()
        .map(|r| r.manifest_bytes().unwrap())
        .collect();
    let rows = descriptor_rows(&source, &blobs);
    let backend = [BackendRow {
        singleton: 1,
        backend: BackendKind::Provider,
        mount_id: Some(TX.into()),
    }];
    let digest = m.digest().unwrap();
    check_rows(&source, &a, Some(&p), &m, &rows).unwrap();
    a.format
        .validate_initial_selection(source.selection())
        .unwrap();
    assert_invalid(
        a.format
            .validate_initial_selection(&ProjectionSelection::All),
        "initial selection differs from FORMAT",
    );
    for field in [
        "catalog_id",
        "generation",
        "snapshot",
        "selection",
        "payload",
        "mount_id",
        "mount_catalog",
        "phase",
        "digest",
    ] {
        let mut payload: CatalogRecord = clone_data(source.catalog());
        if field == "payload" {
            payload.descriptors_digest = STEP.into();
        }
        let mut q = CatalogRow {
            id: &source.commitment().catalog_id,
            source_generation: 0,
            source_digest: &source.commitment().snapshot_digest,
            selection_digest: &source.commitment().selection_digest,
            record: &payload,
        };
        let mut row = MountRow {
            id: TX,
            catalog_id: &source.commitment().catalog_id,
            phase: m.phase,
            digest: &digest,
            record: &m,
        };
        let expected = match field {
            "catalog_id" => {
                q.id = STEP;
                "provider catalog row projection differs"
            }
            "generation" => {
                q.source_generation = 1;
                "provider catalog row projection differs"
            }
            "snapshot" => {
                q.source_digest = &STEP[7..];
                "provider catalog row projection differs"
            }
            "selection" => {
                q.selection_digest = &STEP[7..];
                "provider catalog row projection differs"
            }
            "payload" => "provider catalog row projection differs",
            "mount_id" => {
                row.id = "00000000000000000000000000000002";
                "provider mount row projection differs"
            }
            "mount_catalog" => {
                row.catalog_id = STEP;
                "provider mount row projection differs"
            }
            "phase" => {
                row.phase = MountPhase::ControlPublished;
                "provider mount row projection differs"
            }
            _ => {
                row.digest = STEP;
                "provider mount row projection differs"
            }
        };
        assert_invalid(
            require_bootstrap_row_data(&backend, &[q], &rows, &[row], &source, &a, Some(&p)),
            expected,
        );
    }
    // A self-consistent non-All Ep substitution still cannot replace the actual
    // validated source. Refresh Marker to isolate source commitment comparison.
    let mut changed: AllocationRecord = clone_data(&a);
    changed.source.selection_payload_digest = STEP[7..].into();
    edit_marker(&mut changed, |v| {
        v["initialSelectionDigest"] = STEP[7..].into()
    });
    changed.encode().unwrap();
    assert_invalid(
        check_rows(&source, &changed, Some(&p), &m, &rows),
        "source commitment differs from recomputed rows",
    );
}

#[test]
fn source_order_reserved_alias_selection_and_descriptor_negatives_have_valid_controls() {
    let f = FileManifest::empty(ChunkingDescriptor::single()).unwrap();
    let descriptors = || {
        vec![
            ProviderSourceDescriptor {
                path: p("a"),
                manifest: f.clone(),
            },
            ProviderSourceDescriptor {
                path: p("b"),
                manifest: f.clone(),
            },
        ]
    };
    let entries = || vec![regular_entry(p("a"), &f), regular_entry(p("b"), &f)];
    ProviderBootstrapSource::validate(snapshot(entries()), ProjectionSelection::All, descriptors())
        .unwrap();
    let mut out_of_order = entries();
    out_of_order.swap(0, 1);
    assert_invalid(
        ProviderBootstrapSource::validate(
            snapshot(out_of_order),
            ProjectionSelection::All,
            descriptors(),
        ),
        "projection entries must be sorted and unique",
    );
    let mut reserved = regular_entry(p("a"), &f);
    reserved.state = EntryState::Reserved;
    ProviderBootstrapSource::validate(
        snapshot(vec![reserved.clone()]),
        ProjectionSelection::All,
        vec![ProviderSourceDescriptor {
            path: p("a"),
            manifest: f.clone(),
        }],
    )
    .unwrap();
    reserved.writable = true;
    assert_invalid(
        ProviderBootstrapSource::validate(
            snapshot(vec![reserved]),
            ProjectionSelection::All,
            vec![],
        ),
        "reserved content cannot be writable",
    );
    assert_invalid(
        ProviderBootstrapSource::validate(
            snapshot(vec![regular_entry(p("A"), &f), regular_entry(p("a"), &f)]),
            ProjectionSelection::All,
            vec![],
        ),
        "projection paths alias on the checkout platform",
    );
    let unnormalized = ProjectionSelection::Selected {
        paths: vec![p("b"), p("a")],
        subtrees: vec![],
    };
    assert_invalid(
        ProviderBootstrapSource::validate(snapshot(entries()), unnormalized, descriptors()),
        "selection is not normalized",
    );
    assert_invalid(
        ProviderBootstrapSource::validate(snapshot(entries()), ProjectionSelection::All, vec![]),
        "missing or unsupported selected source descriptor",
    );
    let duplicated = vec![
        ProviderSourceDescriptor {
            path: p("a"),
            manifest: f.clone(),
        },
        ProviderSourceDescriptor {
            path: p("a"),
            manifest: f.clone(),
        },
    ];
    assert_invalid(
        ProviderBootstrapSource::validate(
            snapshot(entries()),
            ProjectionSelection::All,
            duplicated,
        ),
        "duplicate source descriptor path",
    );
    let extra = vec![ProviderSourceDescriptor {
        path: p("a"),
        manifest: f.clone(),
    }];
    assert_invalid(
        ProviderBootstrapSource::validate(snapshot(vec![]), ProjectionSelection::All, extra),
        "extra source descriptor",
    );
    let mut wrong = regular_entry(p("a"), &f);
    wrong.target = Some(TreeEntryTarget::file(FileId::parse(STEP).unwrap()));
    assert_invalid(
        ProviderBootstrapSource::validate(
            snapshot(vec![wrong]),
            ProjectionSelection::All,
            vec![ProviderSourceDescriptor {
                path: p("a"),
                manifest: f,
            }],
        ),
        "source descriptor does not match selected snapshot target",
    );
}

fn long_path(bytes: usize, index: usize) -> RepoPath {
    let leaf = format!("f{index:06}");
    let mut remaining = bytes.checked_sub(leaf.len()).unwrap();
    let mut parts = Vec::new();
    while remaining != 0 {
        let part = (remaining - 1).min(200);
        assert!(part > 0, "fixture length must leave a nonempty component");
        parts.push("a".repeat(part));
        remaining -= part + 1;
    }
    parts.push(leaf);
    let name = parts.join("/");
    assert_eq!(name.len(), bytes);
    p(&name)
}
fn single_path_source(name: RepoPath) -> Result<ProviderBootstrapSource> {
    let f = FileManifest::empty(ChunkingDescriptor::single()).unwrap();
    ProviderBootstrapSource::validate(
        snapshot(vec![regular_entry(name.clone(), &f)]),
        ProjectionSelection::All,
        vec![ProviderSourceDescriptor {
            path: name,
            manifest: f,
        }],
    )
}
fn manifest_with_chunks(count: usize) -> FileManifest {
    FileManifest::new(
        FileId::parse(ZERO).unwrap(),
        count as u64,
        ChunkingDescriptor::single(),
        (0..count)
            .map(|offset| FileChunk::new(ChunkId::parse(ZERO).unwrap(), offset as u64, 1).unwrap())
            .collect(),
    )
    .unwrap()
}
fn many_file_source(count: usize, manifest: &FileManifest) -> Result<ProviderBootstrapSource> {
    let mut entries = Vec::with_capacity(count);
    let mut descriptors = Vec::with_capacity(count);
    for i in 0..count {
        let name = p(&format!("f{i:06}"));
        entries.push(regular_entry(name.clone(), manifest));
        descriptors.push(ProviderSourceDescriptor {
            path: name,
            manifest: manifest.clone(),
        });
    }
    ProviderBootstrapSource::validate(snapshot(entries), ProjectionSelection::All, descriptors)
}

#[test]
fn reachable_vfs_path_depth_and_descriptor_only_path_bounds_are_distinct() {
    single_path_source(long_path(1024, 0)).unwrap();
    let too_long = long_path(1025, 0);
    let f = FileManifest::empty(ChunkingDescriptor::single()).unwrap();
    snapshot(vec![regular_entry(too_long.clone(), &f)])
        .validate()
        .unwrap();
    let cause = format!(
        "InvalidCatalog({:?})",
        format!("unsupported path: {too_long}")
    );
    assert_invalid(single_path_source(too_long), &cause);
    let name64 = p(&std::iter::repeat_n("d", 63)
        .chain(std::iter::once("f"))
        .collect::<Vec<_>>()
        .join("/"));
    single_path_source(name64).unwrap();
    let name65 = p(&std::iter::repeat_n("d", 64)
        .chain(std::iter::once("f"))
        .collect::<Vec<_>>()
        .join("/"));
    let cause = format!(
        "InvalidCatalog({:?})",
        format!("unsupported path: {name65}")
    );
    assert_invalid(single_path_source(name65), &cause);
    // 4096 is a record-data bound only: the selected VFS source limit is 1024.
    let source = file_source(false);
    let mut descriptor: DescriptorRecord = clone_data(&source.descriptors()[0]);
    descriptor.path = long_path(4096, 0);
    descriptor.encode().unwrap();
    descriptor.path = long_path(4097, 0);
    assert_invalid(
        descriptor.encode(),
        "descriptor projection differs from manifest or exceeds bounds",
    );
}

#[test]
fn encoded_commitment_counter_bounds_are_not_whole_source_capacity_claims() {
    let source = file_source(false);
    for (field, limit) in [
        ("snapshotBytes", 64 * 1024 * 1024u64),
        ("selectionBytes", 16 * 1024 * 1024u64),
    ] {
        let mut value = serde_json::to_value(source.commitment()).unwrap();
        value[field] = limit.into();
        let at: SourceCommitment = serde_json::from_value(value.clone()).unwrap();
        at.validate_fields().unwrap(); // Counter shape only, not matching source.
        assert_invalid(
            source.require_commitment(&at),
            "source commitment differs from recomputed rows",
        );
        value[field] = (limit + 1).into();
        let over: SourceCommitment = serde_json::from_value(value).unwrap();
        assert_invalid(
            over.validate_fields(),
            "source commitment budget/accounting mismatch",
        );
    }
    let cap = crate::projection::provider::PROVIDER_CATALOG_AGGREGATE_MAX_BYTES;
    let mut at: SourceCommitment = clone_data(source.commitment());
    at.descriptor_bytes = cap - at.catalog_bytes;
    at.catalog_aggregate_bytes = cap;
    at.validate_fields().unwrap();
    assert_invalid(
        source.require_commitment(&at),
        "source commitment differs from recomputed rows",
    );
    at.descriptor_bytes += 1;
    at.catalog_aggregate_bytes += 1;
    assert_invalid(
        at.validate_fields(),
        "source commitment budget/accounting mismatch",
    );
    let mut count: SourceCommitment = clone_data(source.commitment());
    count.descriptor_count = 100_000;
    count.validate_fields().unwrap();
    assert_invalid(
        source.require_commitment(&count),
        "source commitment differs from recomputed rows",
    );
    count.descriptor_count += 1;
    assert_invalid(
        count.validate_fields(),
        "invalid fresh source generation or descriptor accounting",
    );
}

#[test]
fn real_manifest_row_budget_has_valid_control_and_specific_refusal() {
    let under = manifest_with_chunks(1024);
    assert!(canonical_json(&under).unwrap().len() <= 256 * 1024);
    many_file_source(1, &under).unwrap();
    let over = manifest_with_chunks(4096);
    assert!(canonical_json(&over).unwrap().len() > 256 * 1024);
    // The existing counting writer fails before a canonical over-budget buffer
    // is retained. Match its exact cause, not an unrelated invalid manifest.
    assert_invalid(
        many_file_source(1, &over),
        "record serialization budget exceeded",
    );
}

#[test]
#[ignore = "PURE resource case: about 256MiB manifest JSON / 0.5-1GiB estimated heap; root runs serially"]
fn heavy_whole_source_descriptor_aggregate_crossing() {
    let manifest = manifest_with_chunks(1024);
    let one = many_file_source(1, &manifest).unwrap();
    let row_bytes = one.commitment().descriptor_bytes;
    let cap = crate::projection::provider::PROVIDER_CATALOG_AGGREGATE_MAX_BYTES;
    let below = ((cap - 4096) / row_bytes) as usize;
    let over = (cap / row_bytes + 1) as usize;
    assert!(below > 1 && over <= 4096 && over - below <= 2);
    // 4096 is conservative headroom for the compact scalar catalog, NOT a
    // bypass of actual source validation. Never retain both large sources.
    let source = many_file_source(below, &manifest).unwrap();
    assert!(source.commitment().catalog_bytes <= 4096);
    assert!(source.commitment().catalog_aggregate_bytes <= cap);
    assert!(cap - source.commitment().catalog_aggregate_bytes < 2 * row_bytes);
    drop(source);
    assert_invalid(
        many_file_source(over, &manifest),
        "descriptor aggregate budget exceeded",
    );
}

#[test]
#[ignore = "PURE resource case: roughly 10k 1024-byte paths; estimated peak below256MiB; root runs serially"]
fn heavy_snapshot_conservative_size_boundary() {
    let max = 64 * 1024 * 1024usize;
    let count = (max - 1024) / (6 * 1024 + 512);
    let build = |count| {
        let f = FileManifest::empty(ChunkingDescriptor::single()).unwrap();
        let entries = (0..count)
            .map(|i| regular_entry(long_path(1024, i), &f))
            .collect();
        // Empty selection isolates snapshot validation from selected VFS bytes.
        ProviderBootstrapSource::validate(
            snapshot(entries),
            ProjectionSelection::Selected {
                paths: vec![],
                subtrees: vec![],
            },
            vec![],
        )
    };
    let source = build(count).unwrap();
    assert!(source.commitment().snapshot_bytes < max as u64);
    drop(source);
    assert_invalid(build(count + 1), "projection encoded size budget exceeded");
}

#[test]
#[ignore = "PURE resource case: 8MiB rule paths plus100k-rule control; estimated peak below256MiB; root runs serially"]
fn heavy_selection_reachable_raw_size_and_rule_count_boundaries() {
    let build = |paths| {
        ProviderBootstrapSource::validate(
            snapshot(vec![]),
            ProjectionSelection::Selected {
                paths,
                subtrees: vec![],
            },
            vec![],
        )
    };
    let source = build((0..1024).map(|i| long_path(8192, i)).collect()).unwrap();
    assert!(source.commitment().selection_bytes < 16 * 1024 * 1024);
    drop(source);
    let mut paths: Vec<_> = (0..1024).map(|i| long_path(8192, i)).collect();
    paths[1023] = long_path(8193, 1023);
    assert_invalid(build(paths), "selection exceeds8MiB");
    let source = build((0..100_000).map(|i| p(&format!("f{i:06}"))).collect()).unwrap();
    drop(source);
    assert_invalid(
        build((0..100_001).map(|i| p(&format!("f{i:06}"))).collect()),
        "selection exceeds100000 rules",
    );
}

#[test]
#[ignore = "PURE resource case: 100000 selected files then100001 early refusal; estimated250-600MiB; root runs serially"]
fn heavy_source_100000_descriptor_count_boundary() {
    let empty = FileManifest::empty(ChunkingDescriptor::single()).unwrap();
    let source = many_file_source(100_000, &empty).unwrap();
    assert_eq!(source.descriptors().len(), 100_000);
    assert_eq!(source.vfs_catalog().entries().len(), 100_000);
    drop(source);
    assert_invalid(
        many_file_source(100_001, &empty),
        "source descriptor count exceeded",
    );
}

#[test]
#[ignore = "PURE direct VFS case: 32MiB encoded catalog; estimated150-300MiB; not reachable Store source capacity"]
fn heavy_direct_vfs_32mib_catalog_boundary_is_not_store_capacity() {
    let header = b"layrs-vfs-catalog-v1\0".len() + 8;
    let count = (32 * 1024 * 1024 - header) / (1024 + 4 + 71 + 8);
    let build = |count| {
        layrs_vfs::Catalog::new(
            0,
            (0..count)
                .map(|i| layrs_vfs::CatalogEntry {
                    path: long_path(1024, i),
                    file_id: FileId::parse(ZERO).unwrap(),
                    size: 0,
                })
                .collect(),
        )
    };
    let catalog = build(count).unwrap();
    assert_eq!(catalog.entries().len(), count);
    drop(catalog);
    match build(count + 1) {
        Err(layrs_vfs::Error::InvalidCatalog(cause)) => {
            assert_eq!(cause, "catalog metadata budget exceeded")
        }
        _ => panic!("expected direct VFS encoded catalog budget refusal"),
    }
    // Core's conservative bound is stricter: 6*pathBytes+512 per source entry,
    // versus pathBytes+83 per VFS record. Do not call this a Store capacity test.
    assert!(1024 + count * (6 * 1024 + 512) > 64 * 1024 * 1024);
}

#[test]
#[ignore = "PURE direct VFS case: 50000 files plus50000 directories; estimated below128MiB; root runs serially"]
fn heavy_direct_vfs_namespace_counts_implicit_directories() {
    let build = |count| {
        layrs_vfs::Catalog::new(
            0,
            (0..count)
                .map(|i| layrs_vfs::CatalogEntry {
                    path: p(&format!("d{i:06}/f")),
                    file_id: FileId::parse(ZERO).unwrap(),
                    size: 0,
                })
                .collect(),
        )
    };
    let catalog = build(50_000).unwrap();
    assert_eq!(catalog.entries().len(), 50_000);
    drop(catalog);
    match build(50_001) {
        Err(layrs_vfs::Error::InvalidCatalog(cause)) => {
            assert_eq!(cause, "catalog metadata budget exceeded")
        }
        _ => panic!("expected implicit-directory namespace budget refusal"),
    }
}

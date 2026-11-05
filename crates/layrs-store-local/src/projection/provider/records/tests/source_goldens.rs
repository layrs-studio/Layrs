//! Source golden emitters and assertions of the record codec tests.
use super::*;

pub(super) fn emit_candidates(label: &str, fields: &[(&str, &[u8])]) {
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
pub(super) fn file_source(selected: bool) -> ProviderBootstrapSource {
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
pub(super) fn empty_source() -> ProviderBootstrapSource {
    ProviderBootstrapSource::validate(snapshot(vec![]), ProjectionSelection::All, vec![]).unwrap()
}

pub(super) fn assert_source_golden(
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

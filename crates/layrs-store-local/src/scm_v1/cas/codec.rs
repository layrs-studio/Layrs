//! Canonical CAS envelope and streaming content validation.
use super::*;
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EnvelopeRef<'a, T: Serialize> {
    schema_version: &'static str,
    kind: &'static str,
    object_id: &'a str,
    value: &'a T,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EnvelopeWire {
    schema_version: String,
    kind: String,
    object_id: String,
    value: serde_json::Value,
}

/// The canonical envelope of a Tree, written directly: byte for byte what
/// `encode_envelope` gives (JCS key order: kind, objectId, schemaVersion,
/// value; the value's keys entries, schemaVersion, treeId), the entries
/// taken from the Tree's own canonical payload. Pinned by
/// `tree_envelope_matches_the_general_encoder`.
fn tree_envelope(tree: &Tree) -> CasResult<Vec<u8>> {
    let id = tree.id().as_str();
    let payload = tree
        .canonical_payload()
        .map_err(|error| CasError::Corruption {
            kind: StoreObjectKind::Tree,
            id: id.into(),
            reason: format!("canonical envelope serialization failed: {error}"),
        })?;
    // The payload is `{"entries":[...],"schemaVersion":"layrs.tree.v1"}`.
    let Some(open) = payload.strip_suffix(b"}") else {
        return Err(corruption(
            &StoreObjectId::Tree(tree.id().clone()),
            "Tree payload shape",
        ));
    };
    let mut out = Vec::with_capacity(payload.len() + 2 * id.len() + 96);
    out.extend_from_slice(b"{\"kind\":\"tree\",\"objectId\":\"");
    out.extend_from_slice(id.as_bytes());
    out.extend_from_slice(b"\",\"schemaVersion\":");
    serde_json::to_writer(&mut out, ENVELOPE_VERSION).map_err(|error| CasError::Corruption {
        kind: StoreObjectKind::Tree,
        id: id.into(),
        reason: format!("canonical envelope serialization failed: {error}"),
    })?;
    out.extend_from_slice(b",\"value\":");
    out.extend_from_slice(open);
    out.extend_from_slice(b",\"treeId\":\"");
    out.extend_from_slice(id.as_bytes());
    out.extend_from_slice(b"\"}}");
    Ok(out)
}

pub(super) fn encode_object(object: &CanonicalObject) -> CasResult<Vec<u8>> {
    match object {
        CanonicalObject::ChunkRaw(value) => Ok(value.bytes().to_vec()),
        CanonicalObject::FileManifest(value) => {
            encode_envelope(StoreObjectKind::FileManifest, value.id().as_str(), value)
        }
        CanonicalObject::Tree(value) => tree_envelope(value),
        CanonicalObject::Symlink(value) => {
            encode_envelope(StoreObjectKind::Symlink, value.id().as_str(), value)
        }
        CanonicalObject::Tombstone(value) => {
            encode_envelope(StoreObjectKind::Tombstone, value.id().as_str(), value)
        }
        CanonicalObject::Step(value) => {
            encode_envelope(StoreObjectKind::Step, value.id().as_str(), value)
        }
        CanonicalObject::RepositoryView(value) => {
            encode_envelope(StoreObjectKind::RepositoryView, value.id().as_str(), value)
        }
        CanonicalObject::Conflict(value) => {
            encode_envelope(StoreObjectKind::Conflict, value.id().as_str(), value)
        }
        CanonicalObject::Product(value) => {
            encode_envelope(StoreObjectKind::Product, value.id().as_str(), value)
        }
    }
}

pub(super) fn encode_envelope<T: Serialize>(
    kind: StoreObjectKind,
    object_id: &str,
    value: &T,
) -> CasResult<Vec<u8>> {
    serde_jcs::to_vec(&EnvelopeRef {
        schema_version: ENVELOPE_VERSION,
        kind: kind_name(kind),
        object_id,
        value,
    })
    .map_err(|error| CasError::Corruption {
        kind,
        id: object_id.into(),
        reason: format!("canonical envelope serialization failed: {error}"),
    })
}

pub(super) fn validate_stored_bytes(
    expected: &StoreObjectId,
    bytes: &[u8],
) -> CasResult<CanonicalObject> {
    if let StoreObjectId::ChunkRaw(expected_id) = expected {
        let chunk = Chunk::new(bytes.to_vec())
            .map_err(|error| corruption(expected, format!("invalid raw Chunk: {error}")))?;
        if chunk.id() != expected_id {
            return Err(CasError::IdMismatch {
                expected: expected.clone(),
                actual: chunk.id().to_string(),
            });
        }
        return Ok(CanonicalObject::ChunkRaw(chunk));
    }

    if let StoreObjectId::Tree(_) = expected {
        return validate_tree_bytes(expected, bytes);
    }
    let wire: EnvelopeWire = serde_json::from_slice(bytes)
        .map_err(|error| corruption(expected, format!("invalid JSON envelope: {error}")))?;
    if wire.schema_version != ENVELOPE_VERSION {
        return Err(corruption(
            expected,
            format!("unsupported envelope version {}", wire.schema_version),
        ));
    }
    let actual_kind = parse_kind(&wire.kind).ok_or_else(|| CasError::KindMismatch {
        expected: expected.kind(),
        actual: wire.kind.clone(),
    })?;
    if actual_kind != expected.kind() {
        return Err(CasError::KindMismatch {
            expected: expected.kind(),
            actual: wire.kind,
        });
    }
    if wire.object_id != expected.as_str() {
        return Err(CasError::IdMismatch {
            expected: expected.clone(),
            actual: wire.object_id,
        });
    }

    let object = decode_value(expected, wire.value)?;
    let actual_id = object.object_id();
    if actual_id != *expected {
        return Err(CasError::IdMismatch {
            expected: expected.clone(),
            actual: actual_id.to_string(),
        });
    }
    let canonical = encode_object(&object)?;
    if canonical != bytes {
        return Err(corruption(expected, "JSON envelope is not canonical JCS"));
    }
    Ok(object)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EnvelopeHeader {
    schema_version: String,
    kind: String,
    object_id: String,
    #[allow(dead_code)]
    value: serde::de::IgnoredAny,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TreeEnvelopeWire {
    #[allow(dead_code)]
    schema_version: String,
    #[allow(dead_code)]
    kind: String,
    #[allow(dead_code)]
    object_id: String,
    value: Tree,
}

/// `validate_stored_bytes` for a Tree, with the same checks in the same
/// order: the envelope header is read without building the value, then the
/// value is decoded straight into a Tree (which recomputes its id) and the
/// bytes must be exactly its canonical envelope.
fn validate_tree_bytes(expected: &StoreObjectId, bytes: &[u8]) -> CasResult<CanonicalObject> {
    let header: EnvelopeHeader = serde_json::from_slice(bytes)
        .map_err(|error| corruption(expected, format!("invalid JSON envelope: {error}")))?;
    if header.schema_version != ENVELOPE_VERSION {
        return Err(corruption(
            expected,
            format!("unsupported envelope version {}", header.schema_version),
        ));
    }
    let actual_kind = parse_kind(&header.kind).ok_or_else(|| CasError::KindMismatch {
        expected: expected.kind(),
        actual: header.kind.clone(),
    })?;
    if actual_kind != expected.kind() {
        return Err(CasError::KindMismatch {
            expected: expected.kind(),
            actual: header.kind,
        });
    }
    if header.object_id != expected.as_str() {
        return Err(CasError::IdMismatch {
            expected: expected.clone(),
            actual: header.object_id,
        });
    }
    let wire: TreeEnvelopeWire = serde_json::from_slice(bytes)
        .map_err(|error| corruption(expected, format!("invalid object value: {error}")))?;
    let object = CanonicalObject::Tree(wire.value);
    let actual_id = object.object_id();
    if actual_id != *expected {
        return Err(CasError::IdMismatch {
            expected: expected.clone(),
            actual: actual_id.to_string(),
        });
    }
    let canonical = encode_object(&object)?;
    if canonical != bytes {
        return Err(corruption(expected, "JSON envelope is not canonical JCS"));
    }
    Ok(object)
}

pub(super) fn decode_value(
    expected: &StoreObjectId,
    value: serde_json::Value,
) -> CasResult<CanonicalObject> {
    macro_rules! decode {
        ($type:ty, $variant:path) => {
            serde_json::from_value::<$type>(value)
                .map($variant)
                .map_err(|error| corruption(expected, format!("invalid object value: {error}")))
        };
    }
    match expected {
        StoreObjectId::ChunkRaw(_) => {
            Err(corruption(expected, "ChunkRaw must not use an envelope"))
        }
        StoreObjectId::FileManifest(_) => decode!(FileManifest, CanonicalObject::FileManifest),
        StoreObjectId::Tree(_) => decode!(Tree, CanonicalObject::Tree),
        StoreObjectId::Symlink(_) => decode!(Symlink, CanonicalObject::Symlink),
        StoreObjectId::Tombstone(_) => decode!(Tombstone, CanonicalObject::Tombstone),
        StoreObjectId::Step(_) => decode!(Step, CanonicalObject::Step),
        StoreObjectId::RepositoryView(_) => {
            decode!(RepositoryView, CanonicalObject::RepositoryView)
        }
        StoreObjectId::Conflict(id) => {
            let bytes = canonical_json(&value).map_err(|error| {
                corruption(expected, format!("invalid opaque Conflict value: {error}"))
            })?;
            CanonicalConflict::new(id.clone(), bytes)
                .map(CanonicalObject::Conflict)
                .map_err(|error| corruption(expected, format!("invalid opaque Conflict: {error}")))
        }
        StoreObjectId::Product(id) => {
            let bytes =
                canonical_json(&value).map_err(|error| corruption(expected, error.to_string()))?;
            layrs_store::scm::CanonicalProduct::new(id.clone(), bytes)
                .map(CanonicalObject::Product)
                .map_err(|error| corruption(expected, error.to_string()))
        }
    }
}

pub(super) fn verify_chunk_stream(
    reader: &mut impl Read,
    expected: &StoreObjectId,
    path: &Path,
) -> CasResult<()> {
    stream_and_verify_chunk(reader, expected, path, &mut |_| {}).map(|_| ())
}

pub(super) fn stream_and_verify_chunk(
    reader: &mut impl Read,
    expected: &StoreObjectId,
    path: &Path,
    consume: &mut impl FnMut(&[u8]),
) -> CasResult<u64> {
    let StoreObjectId::ChunkRaw(expected_id) = expected else {
        return Err(corruption(expected, "stream verifier expected ChunkRaw"));
    };
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"layrs-chunk-v1\0");
    let mut buffer = vec![0_u8; IO_BUFFER_SIZE];
    let mut length = 0_u64;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| io_error("stream chunk", path, error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        consume(&buffer[..read]);
        length = length.saturating_add(read as u64);
    }
    let actual = format!("blake3:{}", hasher.finalize().to_hex());
    if actual != expected_id.as_str() {
        return Err(CasError::IdMismatch {
            expected: expected.clone(),
            actual,
        });
    }
    Ok(length)
}

#[cfg(test)]
mod tree_envelope_tests {
    use super::*;
    use layrs_core::scm::{FileId, RepoPathComponent, TreeEntry, TreeEntryTarget, TreeId};

    #[test]
    fn tree_envelope_matches_the_general_encoder() {
        let names = [
            "a",
            "B",
            "f1.txt",
            "caf\u{e9}",
            "\u{1f600}x",
            "quo\"te",
            "back\\slash",
            "tab\tname",
            "ctl\u{1}x",
            "z\u{7f}",
            "\u{2028}sep",
            "slash/no",
            "\u{fffd}",
            "space name",
        ];
        let mut entries = Vec::new();
        for (index, name) in names.iter().enumerate() {
            let Ok(component) = RepoPathComponent::parse(*name) else {
                continue;
            };
            let id = format!("blake3:{}", blake3::hash(name.as_bytes()).to_hex());
            let target = if index % 2 == 0 {
                TreeEntryTarget::file(FileId::parse(id).unwrap())
            } else {
                TreeEntryTarget::tree(TreeId::parse(id).unwrap())
            };
            entries.push(TreeEntry::new(component, target, index % 3 == 0));
        }
        assert!(entries.len() >= 10);
        for count in [0, 1, entries.len()] {
            let tree = Tree::new(entries[..count].to_vec()).unwrap();
            let general =
                encode_envelope(StoreObjectKind::Tree, tree.id().as_str(), &tree).unwrap();
            assert_eq!(tree_envelope(&tree).unwrap(), general);
            let id = StoreObjectId::Tree(tree.id().clone());
            assert_eq!(
                validate_stored_bytes(&id, &general).unwrap(),
                CanonicalObject::Tree(tree.clone())
            );
            // One altered byte (a space) is not canonical.
            let mut spaced = general.clone();
            spaced.insert(1, b' ');
            assert!(validate_stored_bytes(&id, &spaced).is_err());
        }
    }
}

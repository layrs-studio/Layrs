//! Canonical CAS envelope and streaming content validation.
use super::*;
use layrs_core::scm::{
    ConflictId, FileId, RepoPathComponent, SymlinkId, TombstoneId, TreeEntry, TreeEntryTarget,
    TreeId,
};
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

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn take_varint(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0_u64;
    let mut shift = 0;
    loop {
        let byte = *bytes.get(*at)?;
        *at += 1;
        let bits = u64::from(byte & 0x7f);
        if shift > 63 || (shift == 63 && bits > 1) {
            return None;
        }
        value |= bits << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
}

/// Target kinds of a binary Tree entry (the low three bits of its tag; bit
/// 3 is the executable flag, the others are zero).
const TREE_FILE: u8 = 0;
const TREE_TREE: u8 = 1;
const TREE_CONFLICT: u8 = 2;
const TREE_SYMLINK: u8 = 3;
const TREE_TOMBSTONE: u8 = 4;
const TREE_EXECUTABLE: u8 = 8;
/// The smallest encoded entry: shared length, suffix length, one name
/// byte, the tag, the digest.
const TREE_ENTRY_MIN: usize = 1 + 1 + 1 + 1 + 32;

/// Packs v4 (format 8, phase 3b, council s30): a Tree record's body. The
/// record's id is its key; the body is the entry count, then per entry the
/// length its name shares with the previous one, the rest of the name, a
/// tag (target kind, executable bit) and the target's 32-byte digest. Names
/// are in the Tree's order (strictly increasing bytes), so the encoding is
/// one per Tree.
pub(super) fn encode_tree_binary(tree: &Tree) -> Vec<u8> {
    let entries = tree.entries();
    let mut out = Vec::with_capacity(4 + entries.len() * 40);
    put_varint(&mut out, entries.len() as u64);
    let mut previous: &[u8] = &[];
    for entry in entries {
        let name = entry.name().as_str().as_bytes();
        let shared = previous
            .iter()
            .zip(name)
            .take_while(|(left, right)| left == right)
            .count();
        put_varint(&mut out, shared as u64);
        put_varint(&mut out, (name.len() - shared) as u64);
        out.extend_from_slice(&name[shared..]);
        let (kind, digest) = match entry.target() {
            TreeEntryTarget::File { file_id } => (TREE_FILE, file_id.digest_bytes()),
            TreeEntryTarget::Tree { tree_id } => (TREE_TREE, tree_id.digest_bytes()),
            TreeEntryTarget::Conflict { conflict_id } => {
                (TREE_CONFLICT, conflict_id.digest_bytes())
            }
            TreeEntryTarget::Symlink { symlink_id } => (TREE_SYMLINK, symlink_id.digest_bytes()),
            TreeEntryTarget::Tombstone { tombstone_id } => {
                (TREE_TOMBSTONE, tombstone_id.digest_bytes())
            }
        };
        out.push(
            kind | if entry.executable() {
                TREE_EXECUTABLE
            } else {
                0
            },
        );
        out.extend_from_slice(&digest);
        previous = name;
    }
    out
}

/// Decodes a binary Tree record as `expected`: every entry valid, names
/// strictly increasing, the id the key names, and the body the one
/// encoding of that Tree (no padded varint, no shorter shared prefix, no
/// trailing byte).
pub(super) fn decode_tree_binary(
    expected: &StoreObjectId,
    body: &[u8],
) -> CasResult<CanonicalObject> {
    let bad = |reason: &str| corruption(expected, format!("binary Tree record: {reason}"));
    let mut at = 0;
    let count = take_varint(body, &mut at).ok_or_else(|| bad("truncated count"))?;
    // Bounded before anything is allocated for it.
    if count > (body.len() / TREE_ENTRY_MIN) as u64 {
        return Err(bad("more entries than its bytes hold"));
    }
    let mut entries = Vec::with_capacity(count as usize);
    let mut previous: Vec<u8> = Vec::new();
    for index in 0..count {
        let shared = take_varint(body, &mut at).ok_or_else(|| bad("truncated entry"))?;
        let suffix = take_varint(body, &mut at).ok_or_else(|| bad("truncated entry"))?;
        if shared > previous.len() as u64 || suffix == 0 {
            return Err(bad("entry name out of bounds"));
        }
        let end = usize::try_from(suffix)
            .ok()
            .and_then(|suffix| at.checked_add(suffix))
            .filter(|end| *end <= body.len())
            .ok_or_else(|| bad("truncated name"))?;
        let mut name = previous[..shared as usize].to_vec();
        name.extend_from_slice(&body[at..end]);
        at = end;
        if index > 0 && name <= previous {
            return Err(bad("names are not strictly increasing"));
        }
        let tag = *body.get(at).ok_or_else(|| bad("truncated tag"))?;
        at += 1;
        let digest: [u8; 32] = body
            .get(at..at + 32)
            .and_then(|digest| digest.try_into().ok())
            .ok_or_else(|| bad("truncated digest"))?;
        at += 32;
        let target = match tag & !TREE_EXECUTABLE {
            TREE_FILE => TreeEntryTarget::file(FileId::from_digest_bytes(&digest)),
            TREE_TREE => TreeEntryTarget::tree(TreeId::from_digest_bytes(&digest)),
            TREE_CONFLICT => TreeEntryTarget::conflict(ConflictId::from_digest_bytes(&digest)),
            TREE_SYMLINK => TreeEntryTarget::symlink(SymlinkId::from_digest_bytes(&digest)),
            TREE_TOMBSTONE => TreeEntryTarget::tombstone(TombstoneId::from_digest_bytes(&digest)),
            _ => return Err(bad("unknown entry tag")),
        };
        let text = std::str::from_utf8(&name).map_err(|_| bad("name is not UTF-8"))?;
        let component = RepoPathComponent::parse(text)
            .map_err(|error| bad(&format!("invalid name: {error}")))?;
        entries.push(TreeEntry::new(
            component,
            target,
            tag & TREE_EXECUTABLE != 0,
        ));
        previous = name;
    }
    if at != body.len() {
        return Err(bad("trailing bytes"));
    }
    let tree = Tree::new(entries).map_err(|error| bad(&error.to_string()))?;
    let object = CanonicalObject::Tree(tree);
    let actual = object.object_id();
    if actual != *expected {
        return Err(CasError::IdMismatch {
            expected: expected.clone(),
            actual: actual.to_string(),
        });
    }
    let CanonicalObject::Tree(tree) = &object else {
        unreachable!("built as a Tree");
    };
    if encode_tree_binary(tree) != body {
        return Err(bad("not its canonical encoding"));
    }
    Ok(object)
}

/// The first and last entry names of a binary Tree record, as stored (an
/// ordering key only: `None` for a malformed body).
pub(super) fn tree_binary_bounds(body: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut at = 0;
    let count = take_varint(body, &mut at)?;
    let mut first = None;
    let mut previous: Vec<u8> = Vec::new();
    for _ in 0..count.min((body.len() / TREE_ENTRY_MIN) as u64) {
        let shared = usize::try_from(take_varint(body, &mut at)?).ok()?;
        let suffix = usize::try_from(take_varint(body, &mut at)?).ok()?;
        let end = at.checked_add(suffix).filter(|end| *end <= body.len())?;
        let mut name = previous.get(..shared)?.to_vec();
        name.extend_from_slice(&body[at..end]);
        at = end + 1 + 32;
        if first.is_none() {
            first = Some(name.clone());
        }
        previous = name;
    }
    Some((first.unwrap_or_default(), previous))
}

/// A pack record's object, decoded from its body as the kind's codec in a
/// pack of `version` gives it (`pack2::record_codec`): a binary Tree in
/// packs v4, canonical bytes otherwise.
pub(super) fn validate_pack_record(
    version: u32,
    expected: &StoreObjectId,
    body: &[u8],
    whole_files: bool,
) -> CasResult<CanonicalObject> {
    if matches!(expected, StoreObjectId::Tree(_)) && pack2::binary_trees(version) {
        return decode_tree_binary(expected, body);
    }
    validate_stored_bytes(expected, body, whole_files)
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

/// Decodes and checks stored bytes as `expected`. A `whole` manifest (council
/// s13 Q5) is content only in a Store that keeps whole files
/// (`whole_files`, format 7); anywhere else it is corruption (council s16
/// r2). A raw chunk decoded here is always a v1 chunk.
pub(super) fn validate_stored_bytes(
    expected: &StoreObjectId,
    bytes: &[u8],
    whole_files: bool,
) -> CasResult<CanonicalObject> {
    let object = validate_stored_bytes_any(expected, bytes)?;
    if !whole_files {
        if let CanonicalObject::FileManifest(manifest) = &object {
            if manifest.chunks_are_whole() {
                return Err(corruption(
                    expected,
                    "a whole file belongs only in a Store of format 7",
                ));
            }
        }
    }
    Ok(object)
}

fn validate_stored_bytes_any(expected: &StoreObjectId, bytes: &[u8]) -> CasResult<CanonicalObject> {
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
                validate_stored_bytes(&id, &general, false).unwrap(),
                CanonicalObject::Tree(tree.clone())
            );
            // One altered byte (a space) is not canonical.
            let mut spaced = general.clone();
            spaced.insert(1, b' ');
            assert!(validate_stored_bytes(&id, &spaced, false).is_err());
        }
    }
}

#[cfg(test)]
mod whole_gate_tests {
    use super::*;

    /// A whole manifest decodes as content only for a Store of format 7
    /// (council s16 r2); any other read refuses it as corruption.
    #[test]
    fn a_whole_manifest_is_content_only_in_format_7() {
        let chunk = Chunk::whole(b"alpha".to_vec()).unwrap();
        let manifest = layrs_core::scm::FileManifest::from_chunks(
            layrs_core::scm::ChunkingDescriptor::whole(),
            std::slice::from_ref(&chunk),
        )
        .unwrap();
        let object = CanonicalObject::FileManifest(manifest.clone());
        let bytes = encode_object(&object).unwrap();
        let id = object.object_id();
        assert_eq!(validate_stored_bytes(&id, &bytes, true).unwrap(), object);
        assert!(matches!(
            validate_stored_bytes(&id, &bytes, false),
            Err(CasError::Corruption { .. })
        ));
        // A v1 manifest is content everywhere.
        let v1 = layrs_core::scm::FileManifest::from_chunks(
            layrs_core::scm::fixed_chunking(5),
            &[Chunk::new(b"alpha".to_vec()).unwrap()],
        )
        .unwrap();
        let object = CanonicalObject::FileManifest(v1);
        let bytes = encode_object(&object).unwrap();
        assert!(validate_stored_bytes(&object.object_id(), &bytes, false).is_ok());
    }
}

#[cfg(test)]
mod tree_binary_tests {
    use super::*;

    fn digest(seed: &str) -> String {
        format!("blake3:{}", blake3::hash(seed.as_bytes()).to_hex())
    }

    fn tree(names: &[&str]) -> Tree {
        let entries = names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                let id = digest(name);
                let target = match index % 5 {
                    0 => TreeEntryTarget::file(FileId::parse(id).unwrap()),
                    1 => TreeEntryTarget::tree(TreeId::parse(id).unwrap()),
                    2 => TreeEntryTarget::conflict(ConflictId::parse(id).unwrap()),
                    3 => TreeEntryTarget::symlink(SymlinkId::parse(id).unwrap()),
                    _ => TreeEntryTarget::tombstone(TombstoneId::parse(id).unwrap()),
                };
                TreeEntry::new(
                    RepoPathComponent::parse(*name).unwrap(),
                    target,
                    index % 2 == 0,
                )
            })
            .collect();
        Tree::new(entries).unwrap()
    }

    fn id(tree: &Tree) -> StoreObjectId {
        StoreObjectId::Tree(tree.id().clone())
    }

    /// Phase 3b: a binary Tree record decodes to the same Tree (every
    /// target kind, executable bits, shared prefixes, non-ASCII names), its
    /// bounds are its first and last names, and the empty Tree encodes too.
    #[test]
    fn binary_trees_round_trip() {
        for names in [
            vec![],
            vec!["a"],
            vec!["f0.txt", "f1.txt", "f10.txt", "f2.txt", "g"],
            vec!["caf\u{e9}", "caf\u{e9}s", "\u{1f600}x", "z"],
            vec!["src", "src.rs", "srcs", "t"],
        ] {
            let tree = tree(&names);
            let body = encode_tree_binary(&tree);
            assert_eq!(
                decode_tree_binary(&id(&tree), &body).unwrap(),
                CanonicalObject::Tree(tree.clone()),
                "{names:?}"
            );
            let (first, last) = tree_binary_bounds(&body).unwrap();
            let sorted: Vec<&str> = tree.entries().iter().map(|e| e.name().as_str()).collect();
            assert_eq!(first, sorted.first().copied().unwrap_or("").as_bytes());
            assert_eq!(last, sorted.last().copied().unwrap_or("").as_bytes());
        }
        // Shared prefixes are stored once: "f10.txt" shares "f1" with
        // "f1.txt" and stores "0.txt".
        let body = encode_tree_binary(&tree(&["f1.txt", "f10.txt"]));
        assert_eq!(body.len(), 1 + (2 + 6 + 1 + 32) + (2 + 5 + 1 + 32));
    }

    /// Only the one encoding of a Tree decodes, and only as its own id.
    #[test]
    fn binary_trees_refuse_other_encodings() {
        let tree = tree(&["ab", "abc", "b"]);
        let body = encode_tree_binary(&tree);
        let expected = id(&tree);
        let entry = |shared: u8, suffix: &[u8], tag: u8, seed: &str| {
            let mut out = vec![shared, suffix.len() as u8];
            out.extend_from_slice(suffix);
            out.push(tag);
            out.extend_from_slice(blake3::hash(seed.as_bytes()).as_bytes());
            out
        };
        // The same entries, re-encoded by hand: decodes.
        let manual = [
            vec![3],
            entry(0, b"ab", 8, "ab"),
            entry(2, b"c", 1, "abc"),
            entry(0, b"b", 8 | 2, "b"),
        ]
        .concat();
        assert_eq!(manual, body);
        let damaged = [
            // A shorter shared prefix than the names share.
            [
                vec![3],
                entry(0, b"ab", 8, "ab"),
                entry(1, b"bc", 1, "abc"),
                entry(0, b"b", 10, "b"),
            ]
            .concat(),
            // Names out of order, and a repeated name.
            [
                vec![3],
                entry(0, b"abc", 1, "abc"),
                entry(0, b"ab", 8, "ab"),
                entry(0, b"b", 10, "b"),
            ]
            .concat(),
            [
                vec![3],
                entry(0, b"ab", 8, "ab"),
                entry(0, b"ab", 1, "abc"),
                entry(0, b"b", 10, "b"),
            ]
            .concat(),
            // An empty suffix, a reserved tag bit, an unknown kind.
            [
                vec![3],
                entry(0, b"ab", 8, "ab"),
                entry(2, b"", 1, "abc"),
                entry(0, b"b", 10, "b"),
            ]
            .concat(),
            [
                vec![3],
                entry(0, b"ab", 8 | 16, "ab"),
                entry(2, b"c", 1, "abc"),
                entry(0, b"b", 10, "b"),
            ]
            .concat(),
            [
                vec![3],
                entry(0, b"ab", 5, "ab"),
                entry(2, b"c", 1, "abc"),
                entry(0, b"b", 10, "b"),
            ]
            .concat(),
            // A padded count, a trailing byte, a truncated digest.
            [vec![0x83, 0], body[1..].to_vec()].concat(),
            [body.clone(), vec![0]].concat(),
            body[..body.len() - 1].to_vec(),
            // A count past what the bytes could hold.
            [vec![0xff, 0xff, 0xff, 0xff, 0x0f], body[1..].to_vec()].concat(),
            // An invalid name.
            [vec![1], entry(0, b"a/b", 0, "x")].concat(),
        ];
        for bytes in damaged {
            assert!(decode_tree_binary(&expected, &bytes).is_err(), "{bytes:?}");
        }
        // Another Tree's bytes under this id: an id mismatch.
        let other = encode_tree_binary(&super::tree_binary_tests::tree(&["ab"]));
        assert!(matches!(
            decode_tree_binary(&expected, &other),
            Err(CasError::IdMismatch { .. })
        ));
    }
}

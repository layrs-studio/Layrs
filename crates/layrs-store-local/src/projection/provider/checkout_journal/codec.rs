use super::*;
use std::io::{self, Write};

struct Count {
    bytes: usize,
}
impl Write for Count {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .filter(|n| *n <= MAX_ROW_BYTES)
            .ok_or_else(|| io::Error::other("checkout row serialization exceeds bound"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub(super) fn canonical<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_writer(Count { bytes: 0 }, value).map_err(|e| invalid(e.to_string()))?;
    let bytes = canonical_json(value).map_err(|e| invalid(e.to_string()))?;
    if bytes.len() > MAX_ROW_BYTES {
        return Err(invalid("checkout canonical row exceeds bound"));
    }
    Ok(bytes)
}
pub(super) fn hash(domain: &'static str, bytes: &[u8]) -> String {
    super::super::records::codec::hash(domain, bytes)
}
fn lower_hex(value: &str, n: usize) -> bool {
    value.len() == n
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) fn token(value: &str) -> Result<()> {
    if !value
        .strip_prefix("blake3:")
        .is_some_and(|raw| lower_hex(raw, 64))
    {
        return Err(invalid("checkout digest token invalid"));
    }
    Ok(())
}
pub(super) fn raw_digest(value: &str) -> Result<()> {
    if !lower_hex(value, 64) {
        return Err(invalid("checkout image digest invalid"));
    }
    Ok(())
}
pub(super) fn file_identity(value: &str) -> Result<()> {
    if value.len() > 32 {
        return Err(invalid("checkout file identity exceeds bound"));
    }
    let mut count = 0;
    for part in value.split(':') {
        let n = part
            .parse::<u32>()
            .map_err(|_| invalid("checkout file identity invalid"))?;
        if n.to_string() != part {
            return Err(invalid("checkout file identity noncanonical"));
        }
        count += 1;
    }
    if count != 3 {
        return Err(invalid("checkout file identity needs three components"));
    }
    Ok(())
}
pub(super) fn directory_identity(value: &str) -> Result<()> {
    if value.len() != 39 {
        return Err(invalid("checkout directory identity invalid"));
    }
    let mut parts = value.split(':');
    if parts.next() != Some("fs")
        || parts.next() != Some("v1")
        || !parts.next().is_some_and(|p| lower_hex(p, 16))
        || !parts.next().is_some_and(|p| lower_hex(p, 16))
        || parts.next().is_some()
    {
        return Err(invalid("checkout directory identity invalid"));
    }
    Ok(())
}
pub(super) fn file_as_directory_identity(value: &str) -> Result<String> {
    file_identity(value)?;
    let n: Vec<_> = value
        .split(':')
        .map(|s| s.parse::<u32>().unwrap())
        .collect();
    Ok(format!(
        "fs:v1:{:016x}:{:016x}",
        n[0],
        ((n[1] as u64) << 32) | n[2] as u64
    ))
}
pub(super) fn path(path: &RepoPath) -> Result<()> {
    if path.as_str().len() > 4096 {
        return Err(invalid("checkout path exceeds bound"));
    }
    ReservedPaths::new(std::iter::empty::<&str>(), PathSemantics::PortableWindows)
        .and_then(|r| r.check_path(path.as_str()))
        .map_err(|e| invalid(e.to_string()))
}
pub(super) fn file(file: &CheckoutFile) -> Result<()> {
    path(&file.path)?;
    if file.bytes > 1073741824 {
        return Err(invalid("checkout file exceeds 1GiB"));
    }
    if file.executable {
        return Err(invalid(
            "Windows cannot faithfully materialize executable checkout mode",
        ));
    }
    Ok(())
}
pub(super) fn header(h: &CheckoutHeader) -> Result<()> {
    if h.schema_version != HEADER_SCHEMA {
        return Err(invalid("checkout header schema differs"));
    }
    super::super::records::codec::transaction(&h.transaction_id)?;
    for value in [
        &h.plan_digest,
        &h.prefix_id,
        &h.destination_proof_digest,
        &h.inventory_digest,
    ] {
        token(value)?;
    }
    raw_digest(&h.image_digest)?;
    for id in [
        &h.checkout_identity,
        &h.control_identity,
        &h.metadata_identity,
        &h.parent_identity,
        &h.family_root_identity,
    ] {
        directory_identity(id)?;
    }
    file_identity(&h.database_identity)?;
    file_identity(&h.format_file_identity)?;
    let mut unique: BTreeSet<_> = [
        &h.checkout_identity,
        &h.control_identity,
        &h.metadata_identity,
        &h.parent_identity,
        &h.family_root_identity,
    ]
    .into_iter()
    .cloned()
    .collect();
    if unique.len() != 5
        || !unique.insert(file_as_directory_identity(&h.database_identity)?)
        || !unique.insert(file_as_directory_identity(&h.format_file_identity)?)
    {
        return Err(invalid("checkout header identities alias"));
    }
    if h.files > MAX_FILES as u64
        || h.directories > MAX_DIRECTORIES as u64
        || h.deletions > MAX_FILES as u64
        || h.files
            .checked_add(h.deletions)
            .is_none_or(|n| n > MAX_FILES as u64)
        || h.file_bytes > 4 * 1073741824
        || h.path_bytes > MAX_PATH_BYTES as u64
    {
        return Err(invalid("checkout header inventory exceeds bound"));
    }
    Ok(())
}
fn directory_state(state: &DirectoryState) -> Result<()> {
    if let DirectoryState::Owned { identity } = state {
        directory_identity(identity)?;
    }
    Ok(())
}
fn file_state(state: &FileState) -> Result<()> {
    if let Some(inode) = state.inode() {
        file_identity(inode)?;
    }
    if let FileState::Publishing {
        parent_identity, ..
    }
    | FileState::Published {
        parent_identity, ..
    } = state
    {
        directory_identity(parent_identity)?;
    }
    Ok(())
}
pub(super) fn validate(row: &CheckoutRow) -> Result<()> {
    if row.schema_version != ROW_SCHEMA {
        return Err(invalid("checkout row schema differs"));
    }
    token(&row.header_digest)?;
    match &row.entry {
        CheckoutEntry::Header { header: h } => {
            header(h)?;
            if hash("layrs-provider-checkout-header-v1", &canonical(h)?) != row.header_digest {
                return Err(invalid("checkout row header commitment differs"));
            }
        }
        CheckoutEntry::Stage { state } => directory_state(state)?,
        CheckoutEntry::Directory {
            ordinal,
            path: p,
            state,
        } => {
            if *ordinal as usize >= MAX_DIRECTORIES {
                return Err(invalid("checkout directory ordinal exceeds bound"));
            }
            path(p)?;
            directory_state(state)?;
        }
        CheckoutEntry::File {
            ordinal,
            file: f,
            state,
        } => {
            if *ordinal as usize >= MAX_FILES {
                return Err(invalid("checkout file ordinal exceeds bound"));
            }
            file(f)?;
            file_state(state)?;
        }
    }
    Ok(())
}
pub(super) fn encode(row: &CheckoutRow) -> Result<Vec<u8>> {
    validate(row)?;
    let bytes = canonical(row)?;
    let roundtrip: CheckoutRow =
        serde_json::from_slice(&bytes).map_err(|e| invalid(e.to_string()))?;
    if roundtrip != *row {
        return Err(invalid("checkout canonical encoding changes DATA"));
    }
    Ok(bytes)
}
pub(super) fn decode(bytes: &[u8]) -> Result<CheckoutRow> {
    if bytes.is_empty() || bytes.len() > MAX_ROW_BYTES {
        return Err(invalid("checkout row bytes exceed bound"));
    }
    let row: CheckoutRow = serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
    if encode(&row)? != bytes {
        return Err(invalid("checkout row is not canonical"));
    }
    Ok(row)
}

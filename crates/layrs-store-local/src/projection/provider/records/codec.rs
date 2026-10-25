//! Bounded in-memory encodings; `Write` below only counts, never performs I/O.
use super::*;
use std::io::{self, Write};

pub(super) const SNAPSHOT_MAX: usize = 64 * 1024 * 1024;
pub(super) const SELECTION_MAX: usize = 16 * 1024 * 1024;
pub(super) const FORMAT_MAX: usize = 64 * 1024;
pub(super) const PATH_MAX: usize = 4096;

struct Count {
    bytes: usize,
    max: usize,
}
impl Write for Count {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let total = self
            .bytes
            .checked_add(bytes.len())
            .filter(|n| *n <= self.max)
            .ok_or_else(|| io::Error::other("record serialization budget exceeded"))?;
        self.bytes = total;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Bound traversal before `canonical_json` constructs its temporary Value/Vec.
/// A count is not a canonical encoding; canonical length is checked separately.
pub(super) fn preflight<T: Serialize + ?Sized>(value: &T, max: usize) -> Result<()> {
    serde_json::to_writer(Count { bytes: 0, max }, value).map_err(invalid)
}

pub(super) fn encode<T: Serialize + DeserializeOwned + PartialEq>(
    value: &T,
    max: usize,
) -> Result<Vec<u8>> {
    preflight(value, max)?;
    let bytes = canonical_json(value).map_err(invalid)?;
    if bytes.len() > max {
        return Err(invalid("canonical record exceeds byte budget"));
    }
    // JCS may round large integer values. Never silently commit different data.
    let roundtrip: T = serde_json::from_slice(&bytes).map_err(invalid)?;
    if &roundtrip != value {
        return Err(invalid("canonical record changes typed value"));
    }
    Ok(bytes)
}

pub(in crate::projection::provider) fn decode<T: Serialize + DeserializeOwned + PartialEq>(
    bytes: &[u8],
    max: usize,
) -> Result<T> {
    if bytes.is_empty() || bytes.len() > max {
        return Err(invalid("record bytes empty or over budget"));
    }
    // Deserialize directly into typed structs: duplicate/unknown fields must
    // not disappear inside an intermediate serde_json::Value.
    let value: T = serde_json::from_slice(bytes).map_err(invalid)?;
    if encode(&value, max)? != bytes {
        return Err(invalid("record is not canonical"));
    }
    Ok(value)
}

pub(super) fn required_option<'de, D, T>(d: D) -> std::result::Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(d)
}

pub(super) fn schema(actual: &str, expected: &str) -> Result<()> {
    if actual != expected {
        return Err(invalid("record schemaVersion mismatch"));
    }
    Ok(())
}

pub(super) fn lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(in crate::projection::provider) fn transaction(value: &str) -> Result<()> {
    if !lower_hex(value, 32) {
        return Err(invalid("transactionId must be 32 lowercase hex characters"));
    }
    Ok(())
}
pub(super) fn raw_digest(value: &str) -> Result<()> {
    if !lower_hex(value, 64) {
        return Err(invalid("raw digest must be 64 lowercase hex characters"));
    }
    Ok(())
}
pub(super) fn digest_token(value: &str) -> Result<()> {
    value
        .strip_prefix("blake3:")
        .ok_or_else(|| invalid("missing digest domain prefix"))
        .and_then(raw_digest)
}
pub(in crate::projection::provider) fn hash(domain: &'static str, bytes: &[u8]) -> String {
    format!(
        "blake3:{}",
        blake3::hash(&layrs_core::scm::domain_separated_bytes(domain, bytes))
    )
}
pub(super) fn add(left: u64, right: u64) -> Result<u64> {
    left.checked_add(right)
        .ok_or_else(|| invalid("record accounting overflow"))
}

pub(super) fn file_identity(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 512 {
        return Err(invalid("invalid file identity length"));
    }
    let mut count = 0;
    for part in value.split(':') {
        let parsed = part.parse::<u32>().map_err(invalid)?;
        if parsed.to_string() != part {
            return Err(invalid("noncanonical file identity"));
        }
        count += 1;
    }
    if count != 3 {
        return Err(invalid("file identity needs three u32 components"));
    }
    Ok(())
}
pub(super) fn directory_identity(value: &str) -> Result<()> {
    if value.len() != 39 {
        return Err(invalid("noncanonical directory identity length"));
    }
    let fields: Vec<_> = value.split(':').collect();
    if fields.len() != 4
        || fields[0] != "fs"
        || fields[1] != "v1"
        || !lower_hex(fields[2], 16)
        || !lower_hex(fields[3], 16)
    {
        return Err(invalid("noncanonical directory identity"));
    }
    Ok(())
}

/// Lexical metadata checks, not canonical-path/native ancestry verification.
/// Stage/control canonical path checks are delegated to the real VFS codec.
pub(super) fn locator(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > PATH_MAX
        || value.contains('\0')
        || !std::path::Path::new(value).is_absolute()
    {
        return Err(invalid("bounded absolute metadata locator required"));
    }
    Ok(())
}

pub(super) fn decode_hex(value: &str, max_bytes: usize) -> Result<Vec<u8>> {
    if value.len() % 2 != 0 || value.len() / 2 > max_bytes || !lower_hex(value, value.len()) {
        return Err(invalid("noncanonical or oversized identity hex"));
    }
    let nibble = |b: u8| {
        if b.is_ascii_digit() {
            b - b'0'
        } else {
            b - b'a' + 10
        }
    };
    Ok(value
        .as_bytes()
        .chunks_exact(2)
        .map(|c| nibble(c[0]) * 16 + nibble(c[1]))
        .collect())
}
pub(super) fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        result.push(DIGITS[(b >> 4) as usize] as char);
        result.push(DIGITS[(b & 15) as usize] as char);
    }
    result
}

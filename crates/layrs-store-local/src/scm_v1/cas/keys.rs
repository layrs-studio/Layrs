//! The keys of a registered pack (council s17, B1b): what lets a Store with
//! an unreadable pack still answer "absent" for the keys that pack never
//! held, and what a later exact repair proves a rebuilt pack against.
//!
//! Written once, in the transaction that registers the pack (a pack that is
//! lost can no longer give them), from what the pack builder holds when it
//! seals the pack:
//!
//! - a Golomb-Rice coded set of fingerprints of every key the pack answers
//!   for, tagged by form: explicit records (kind and id), file records'
//!   FileIds, their ChunkId and FileManifestId aliases, and the FileIds of
//!   the explicit manifests' section. A key it excludes is surely not in the
//!   pack; a key it may include stays an error while the pack is
//!   unreadable (never absent, never present). One false "may include" per
//!   key asked costs about 2^-rice_bits.
//! - `keys_digest`: the pack's logical content in ordinal order (the order
//!   of its admission bitmaps), independent of its layout, and a digest per
//!   256 ordinals.
//!
//! The filter is decoded only for a pack the catalog found unreadable.
use std::io;

/// Tags of the keys a pack answers for (one byte before the 32-byte id).
pub(crate) const EXPLICIT_TAG: u8 = 0x10;
pub(crate) const CHUNK_ALIAS_TAG: u8 = 0x01;
pub(crate) const MANIFEST_ALIAS_TAG: u8 = 0x02;
pub(crate) const FILEID_SECTION_TAG: u8 = 0x03;
pub(crate) const FILE_RECORD_TAG: u8 = 0x04;

/// The parameter new rows use: about 21.5 bits per key, one false "may
/// include" in about a million keys asked.
pub(crate) const RICE_BITS: u8 = 20;
const MAX_RICE_BITS: u8 = 40;
/// Ordinals per group digest.
pub(crate) const GROUP: usize = 256;

pub(crate) type Key = [u8; 33];

pub(crate) fn key(tag: u8, id: &[u8; 32]) -> Key {
    let mut key = [0_u8; 33];
    key[0] = tag;
    key[1..].copy_from_slice(id);
    key
}

/// An explicit record's key: its kind code in the tag.
pub(crate) fn explicit_key(kind_code: u8, id: &[u8; 32]) -> Key {
    key(EXPLICIT_TAG.wrapping_add(kind_code), id)
}

/// What one logical ordinal is, as `keys_digest` commits to it.
pub(crate) enum Ordinal<'a> {
    Explicit {
        kind_code: u8,
        id: &'a [u8; 32],
    },
    ChunkAlias {
        file_id: &'a [u8; 32],
        chunk_id: &'a [u8; 32],
        recipe: u8,
        length: u64,
    },
    ManifestAlias {
        file_id: &'a [u8; 32],
        manifest_id: &'a [u8; 32],
    },
}

impl Ordinal<'_> {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::Explicit { kind_code, id } => {
                out.extend_from_slice(&[1, *kind_code]);
                out.extend_from_slice(*id);
            }
            Self::ChunkAlias {
                file_id,
                chunk_id,
                recipe,
                length,
            } => {
                out.push(2);
                out.extend_from_slice(*file_id);
                out.extend_from_slice(*chunk_id);
                out.push(*recipe);
                out.extend_from_slice(&length.to_le_bytes());
            }
            Self::ManifestAlias {
                file_id,
                manifest_id,
            } => {
                out.push(3);
                out.extend_from_slice(*file_id);
                out.extend_from_slice(*manifest_id);
            }
        }
    }
}

/// One entry of the explicit manifests' FileId section.
pub(crate) struct SectionEntry<'a> {
    pub(crate) file_id: &'a [u8; 32],
    pub(crate) manifest_position: u32,
    pub(crate) length: u64,
}

/// The keys row of one pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackKeys {
    pub(crate) key_count: u64,
    pub(crate) rice_bits: u8,
    pub(crate) keys_digest: [u8; 32],
    /// 32 bytes per `GROUP` ordinals, in order.
    pub(crate) group_digests: Vec<u8>,
    pub(crate) filter: Vec<u8>,
}

impl PackKeys {
    /// From the pack's ordinals in order, its FileId section, and every key
    /// it answers for (duplicates allowed).
    pub(crate) fn build(
        ordinals: &[Ordinal<'_>],
        section: &[SectionEntry<'_>],
        mut keys: Vec<Key>,
        rice_bits: u8,
    ) -> io::Result<Self> {
        let mut digest = blake3::Hasher::new_derive_key("layrs pack keys v1");
        digest.update(&(ordinals.len() as u64).to_le_bytes());
        let mut group_digests = Vec::with_capacity(ordinals.len().div_ceil(GROUP) * 32);
        let mut encoded = Vec::with_capacity(128);
        for (number, group) in ordinals.chunks(GROUP).enumerate() {
            let mut group_digest = blake3::Hasher::new_derive_key("layrs pack keys group v1");
            group_digest.update(&(number as u64).to_le_bytes());
            group_digest.update(&(group.len() as u64).to_le_bytes());
            for ordinal in group {
                encoded.clear();
                ordinal.encode(&mut encoded);
                digest.update(&encoded);
                group_digest.update(&encoded);
            }
            group_digests.extend_from_slice(group_digest.finalize().as_bytes());
        }
        digest.update(&(section.len() as u64).to_le_bytes());
        for entry in section {
            digest.update(entry.file_id);
            digest.update(&entry.manifest_position.to_le_bytes());
            digest.update(&entry.length.to_le_bytes());
        }
        let keys_digest = *digest.finalize().as_bytes();
        keys.sort_unstable();
        keys.dedup();
        let (filter, values) = encode(&keys_digest, &keys, rice_bits)?;
        let built = Self {
            key_count: keys.len() as u64,
            rice_bits,
            keys_digest,
            group_digests,
            filter,
        };
        // Never a false "absent": the row decodes to every key's
        // fingerprint before it is written.
        if built.decode()?.values != values {
            return Err(io::Error::other("pack keys filter misses a key"));
        }
        Ok(built)
    }

    /// The row's checksum, bound to the pack's registered index digest.
    pub(crate) fn checksum(&self, layout_digest: &[u8; 32]) -> [u8; 16] {
        let mut hasher = blake3::Hasher::new_derive_key("layrs pack keys row v1");
        hasher.update(layout_digest);
        hasher.update(&self.keys_digest);
        hasher.update(&self.key_count.to_le_bytes());
        hasher.update(&[self.rice_bits]);
        hasher.update(&(self.group_digests.len() as u64).to_le_bytes());
        hasher.update(&self.group_digests);
        hasher.update(&(self.filter.len() as u64).to_le_bytes());
        hasher.update(&self.filter);
        let mut checksum = [0_u8; 16];
        checksum.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
        checksum
    }

    /// Inserts this pack's keys row in the transaction registering pack
    /// `seq` under `layout_digest` (its registered index digest).
    pub(crate) fn insert(
        &self,
        connection: &rusqlite::Connection,
        seq: i64,
        layout_digest: &[u8; 32],
    ) -> rusqlite::Result<()> {
        connection.execute(
            "INSERT INTO cas_pack_keys(pack_seq,layout_digest,keys_digest,key_count,rice_bits,group_digests,filter,checksum) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![
                seq,
                &layout_digest[..],
                &self.keys_digest[..],
                i64::try_from(self.key_count).unwrap_or(i64::MAX),
                i64::from(self.rice_bits),
                &self.group_digests,
                &self.filter,
                &self.checksum(layout_digest)[..],
            ],
        )?;
        Ok(())
    }

    /// The filter of a stored row, when the row is sound and bound to
    /// `registered` (its pack's registered index digest); `None` otherwise
    /// (a missing or damaged row excludes nothing).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn filter_of_row(
        registered: &[u8],
        layout_digest: &[u8],
        keys_digest: &[u8],
        key_count: i64,
        rice_bits: i64,
        group_digests: Vec<u8>,
        filter: Vec<u8>,
        checksum: &[u8],
    ) -> Option<KeyFilter> {
        let layout: [u8; 32] = layout_digest.try_into().ok()?;
        if registered != layout_digest {
            return None;
        }
        let row = Self {
            key_count: u64::try_from(key_count).ok()?,
            rice_bits: u8::try_from(rice_bits).ok()?,
            keys_digest: keys_digest.try_into().ok()?,
            group_digests,
            filter,
        };
        if row.checksum(&layout)[..] != *checksum {
            return None;
        }
        row.decode().ok()
    }

    /// The filter, decoded for lookups.
    pub(crate) fn decode(&self) -> io::Result<KeyFilter> {
        let range = range(self.key_count, self.rice_bits)?;
        let values = decode(&self.filter, self.key_count, self.rice_bits)?;
        if values.last().is_some_and(|last| *last >= range) {
            return Err(io::Error::other("pack keys filter value out of range"));
        }
        Ok(KeyFilter {
            keys_digest: self.keys_digest,
            range,
            values,
        })
    }
}

/// A decoded filter: sorted fingerprints.
#[derive(Debug, Clone)]
pub(crate) struct KeyFilter {
    keys_digest: [u8; 32],
    range: u64,
    values: Vec<u64>,
}

impl KeyFilter {
    /// Whether the pack may hold `key`; `false` is exact.
    pub(crate) fn may_contain(&self, key: &Key) -> bool {
        if self.values.is_empty() {
            return false;
        }
        self.values
            .binary_search(&fingerprint(&self.keys_digest, key, self.range))
            .is_ok()
    }
}

fn range(count: u64, rice_bits: u8) -> io::Result<u64> {
    if rice_bits == 0 || rice_bits > MAX_RICE_BITS {
        return Err(io::Error::other("pack keys filter parameter out of bounds"));
    }
    count
        .checked_shl(u32::from(rice_bits))
        .filter(|range| range >> rice_bits == count)
        .ok_or_else(|| io::Error::other("pack keys filter range overflows"))
}

fn fingerprint(keys_digest: &[u8; 32], key: &Key, range: u64) -> u64 {
    let hash = blake3::keyed_hash(keys_digest, key);
    let mut word = [0_u8; 8];
    word.copy_from_slice(&hash.as_bytes()[..8]);
    ((u128::from(u64::from_le_bytes(word)) * u128::from(range)) >> 64) as u64
}

/// The coded set of `keys`, and the sorted fingerprints it codes.
fn encode(keys_digest: &[u8; 32], keys: &[Key], rice_bits: u8) -> io::Result<(Vec<u8>, Vec<u64>)> {
    if keys.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let range = range(keys.len() as u64, rice_bits)?;
    let mut values: Vec<u64> = keys
        .iter()
        .map(|key| fingerprint(keys_digest, key, range))
        .collect();
    values.sort_unstable();
    let mut bits = BitWriter::default();
    let mut previous = 0_u64;
    for &value in &values {
        let delta = value - previous;
        previous = value;
        let quotient = delta >> rice_bits;
        for _ in 0..quotient {
            bits.push(true);
        }
        bits.push(false);
        for bit in (0..rice_bits).rev() {
            bits.push((delta >> bit) & 1 == 1);
        }
    }
    Ok((bits.finish(), values))
}

fn decode(bytes: &[u8], count: u64, rice_bits: u8) -> io::Result<Vec<u64>> {
    let invalid = || io::Error::other("pack keys filter is malformed");
    if count == 0 {
        return if bytes.is_empty() {
            Ok(Vec::new())
        } else {
            Err(invalid())
        };
    }
    // Each value takes at least `rice_bits + 1` bits.
    if count > (bytes.len() as u64 * 8) / (u64::from(rice_bits) + 1) {
        return Err(invalid());
    }
    let mut reader = BitReader { bytes, at: 0 };
    let mut values = Vec::with_capacity(count as usize);
    let mut previous = 0_u64;
    for _ in 0..count {
        let mut quotient = 0_u64;
        while reader.next().ok_or_else(invalid)? {
            quotient += 1;
            if quotient > bytes.len() as u64 * 8 {
                return Err(invalid());
            }
        }
        let mut remainder = 0_u64;
        for _ in 0..rice_bits {
            remainder = (remainder << 1) | u64::from(reader.next().ok_or_else(invalid)?);
        }
        let delta = quotient
            .checked_shl(u32::from(rice_bits))
            .and_then(|high| high.checked_add(remainder))
            .ok_or_else(invalid)?;
        previous = previous.checked_add(delta).ok_or_else(invalid)?;
        values.push(previous);
    }
    // Only zero padding after the last value, within one byte.
    if bytes.len() * 8 - reader.at >= 8 || (reader.at..bytes.len() * 8).any(|at| bit(bytes, at)) {
        return Err(invalid());
    }
    Ok(values)
}

fn bit(bytes: &[u8], at: usize) -> bool {
    bytes[at / 8] & (0x80 >> (at % 8)) != 0
}

#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    len: usize,
}

impl BitWriter {
    fn push(&mut self, value: bool) {
        if self.len % 8 == 0 {
            self.bytes.push(0);
        }
        if value {
            let last = self.bytes.len() - 1;
            self.bytes[last] |= 0x80 >> (self.len % 8);
        }
        self.len += 1;
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

struct BitReader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl BitReader<'_> {
    fn next(&mut self) -> Option<bool> {
        if self.at >= self.bytes.len() * 8 {
            return None;
        }
        let value = bit(self.bytes, self.at);
        self.at += 1;
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u32) -> [u8; 32] {
        *blake3::hash(&n.to_le_bytes()).as_bytes()
    }

    fn sample(n: u32) -> (Vec<[u8; 32]>, PackKeys) {
        let ids: Vec<[u8; 32]> = (0..n).map(id).collect();
        let ordinals: Vec<Ordinal<'_>> = ids
            .iter()
            .map(|id| Ordinal::Explicit { kind_code: 1, id })
            .collect();
        let keys = ids.iter().map(|id| explicit_key(1, id)).collect();
        let built = PackKeys::build(&ordinals, &[], keys, RICE_BITS).unwrap();
        (ids, built)
    }

    #[test]
    fn every_key_is_found_and_others_almost_never() {
        let (ids, built) = sample(20_000);
        let filter = built.decode().unwrap();
        assert!(
            ids.iter()
                .all(|id| filter.may_contain(&explicit_key(1, id)))
        );
        // Same ids in another form are other keys.
        let others = (20_000..120_000)
            .map(|n| explicit_key(1, &id(n)))
            .chain(ids.iter().map(|id| key(FILE_RECORD_TAG, id)))
            .filter(|key| filter.may_contain(key))
            .count();
        assert!(others <= 2, "{others} false positives");
        // About 21.5 bits per key.
        let bits = built.filter.len() as f64 * 8.0 / 20_000.0;
        assert!((20.0..23.5).contains(&bits), "{bits} bits per key");
        assert_eq!(built.group_digests.len(), 20_000_usize.div_ceil(GROUP) * 32);
    }

    #[test]
    fn the_encoding_is_deterministic_and_bound_to_the_content() {
        let (_, first) = sample(300);
        let (_, again) = sample(300);
        assert_eq!(first, again);
        let (_, other) = sample(301);
        assert_ne!(first.keys_digest, other.keys_digest);
        let layout = [7_u8; 32];
        assert_ne!(first.checksum(&layout), first.checksum(&[8; 32]));
    }

    #[test]
    fn an_empty_pack_excludes_everything() {
        let built = PackKeys::build(&[], &[], Vec::new(), RICE_BITS).unwrap();
        assert!(built.filter.is_empty());
        assert!(!built.decode().unwrap().may_contain(&key(1, &id(1))));
    }

    #[test]
    fn a_malformed_filter_is_refused_not_read_as_absence() {
        let (_, built) = sample(1_000);
        let mut truncated = built.clone();
        truncated.filter.truncate(built.filter.len() / 2);
        assert!(truncated.decode().is_err());
        let mut padded = built.clone();
        padded.filter.push(0);
        assert!(padded.decode().is_err());
        let mut bounded = built;
        bounded.rice_bits = 0;
        assert!(bounded.decode().is_err());
    }
}

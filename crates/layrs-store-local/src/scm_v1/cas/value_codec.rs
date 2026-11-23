//! Packs v4 (format 8, phase 3c, council s30): the binary codec of Step,
//! RepositoryView, Operation and StepObservation records.
//!
//! A record's canonical JSON (its JCS envelope for a content record, its
//! JCS value for a history record) is spelled as a tree of tagged values:
//! object keys and common words by dictionary index, content ids as their
//! 32-byte digests, `<prefix>:<uuid>` ids as a prefix code and 16 bytes,
//! RFC 3339 UTC times as integers with their fraction digits, the record's
//! own id (its key names it) as one byte, anything else as text. Decoding
//! rebuilds the JSON value and its JCS bytes, which the existing validators
//! check; only the one encoding of a value decodes (`decode_value_binary`
//! re-encodes and compares). `value_binary_expanded_len` gives the exact
//! JCS length without decoding, for budgets.
use serde_json::{Map, Number, Value};

const NULL: u8 = 0;
const FALSE: u8 = 1;
const TRUE: u8 = 2;
const INTEGER: u8 = 3;
const NUMBER: u8 = 4;
const TEXT: u8 = 5;
const WORD: u8 = 6;
const CONTENT_ID: u8 = 7;
const UUID_ID: u8 = 8;
const TIME: u8 = 9;
const ARRAY: u8 = 10;
const OBJECT: u8 = 11;
const OWN_ID: u8 = 12;

/// Deepest nesting a record decodes (the record types nest 4 levels).
const MAX_DEPTH: usize = 32;
/// Integers spelled as such: those every JSON number renders exactly.
const INTEGER_MAX: i64 = (1 << 53) - 1;

/// Object keys by index + 1 (0: the key follows as text). These three
/// tables are part of the format: any change to them changes the one
/// encoding of existing values (a key or word once spelled as text would
/// now have an index), so it needs a new draft; `tables_are_pinned` holds
/// their hash.
const KEYS: &[&str] = &[
    "activeLayerId",
    "actorId",
    "baseStepId",
    "captureReason",
    "createdAt",
    "deviceId",
    "key",
    "kind",
    "layerHeads",
    "layerId",
    "markerIds",
    "materializedTreeId",
    "metadata",
    "newStepIds",
    "objectId",
    "observationId",
    "observedAt",
    "oldStepId",
    "operationId",
    "parentOperationIds",
    "parentStepIds",
    "releaseIds",
    "repositoryViewId",
    "resultingViewId",
    "rewriteMap",
    "rootTreeId",
    "schemaVersion",
    "spaceId",
    "status",
    "stepId",
    "stepIds",
    "summary",
    "value",
    "worktreeId",
    "worktrees",
];

/// Common string values by index (part of the format, as `KEYS`).
const WORDS: &[&str] = &[
    "layrs.cas-envelope.v1",
    "layrs.step.v1",
    "layrs.repository-view.v1",
    "layrs.operation.v1",
    "layrs.step-observation.v1",
    "step",
    "repository_view",
    "ready",
    "conflicted",
    "needs_repair",
    "stale",
    "initialize",
    "capture_worktree",
    "switch_layer",
    "move_layer_heads",
    "create_marker",
    "hide_marker",
    "create_release",
    "weave",
    "resolve_conflict",
    "rewrite_steps",
    "receive_remote_state",
    "integrate_operations",
    "undo",
    "redo",
    "restore",
    "revert",
    "prune",
    "automatic_idle",
    "before_layer_switch",
    "before_worktree_switch",
    "before_operation",
    "manual_snapshot",
    "weave_result",
    "conflict_resolution",
    "rewrite",
    "received",
    "automation",
    "Capture Worktree",
    "Initialize",
];

/// `<prefix>:<uuid>` ids by prefix code (part of the format, as `KEYS`).
const PREFIXES: &[&str] = &[
    "operation",
    "observation",
    "transaction",
    "space",
    "layer",
    "worktree",
    "store",
    "marker",
    "release",
    "actor",
    "device",
];

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

fn zigzag(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

fn unzigzag(value: u64) -> i64 {
    ((value >> 1) as i64) ^ -((value & 1) as i64)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn push_hex(out: &mut String, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 15)] as char);
    }
}

/// `blake3:<64 lowercase hex>` as its digest.
fn content_digest(text: &str) -> Option<[u8; 32]> {
    let hex = text.strip_prefix("blake3:")?.as_bytes();
    if hex.len() != 64 {
        return None;
    }
    let mut digest = [0_u8; 32];
    for (index, slot) in digest.iter_mut().enumerate() {
        *slot = (hex_digit(hex[2 * index])? << 4) | hex_digit(hex[2 * index + 1])?;
    }
    Some(digest)
}

/// `<prefix>:<lowercase canonical uuid>` as its prefix code and bytes.
fn uuid_id(text: &str) -> Option<(u8, [u8; 16])> {
    let (prefix, uuid) = text.split_once(':')?;
    let code = PREFIXES.iter().position(|known| *known == prefix)? as u8;
    let uuid = uuid.as_bytes();
    if uuid.len() != 36 {
        return None;
    }
    let mut bytes = [0_u8; 16];
    let mut nibbles = 0;
    for (index, byte) in uuid.iter().enumerate() {
        if matches!(index, 8 | 13 | 18 | 23) {
            if *byte != b'-' {
                return None;
            }
            continue;
        }
        let digit = hex_digit(*byte)?;
        bytes[nibbles / 2] |= if nibbles % 2 == 0 { digit << 4 } else { digit };
        nibbles += 1;
    }
    Some((code, bytes))
}

fn uuid_text(code: u8, bytes: &[u8; 16]) -> Option<String> {
    let prefix = PREFIXES.get(usize::from(code))?;
    let mut text = String::with_capacity(prefix.len() + 37);
    text.push_str(prefix);
    text.push(':');
    for (index, range) in [0..4, 4..6, 6..8, 8..10, 10..16].into_iter().enumerate() {
        if index > 0 {
            text.push('-');
        }
        push_hex(&mut text, &bytes[range]);
    }
    Some(text)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Hinnant).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// `YYYY-MM-DDTHH:MM:SS[.f]Z` (1 to 9 fraction digits) as seconds since
/// 1970, the fraction's digit count and its value; `None` for anything
/// that would not render back to the same text.
fn time_parts(text: &str) -> Option<(i64, u8, u64)> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    if bytes[13] != b':' || bytes[16] != b':' || *bytes.last()? != b'Z' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let slice = bytes.get(range)?;
        slice
            .iter()
            .all(u8::is_ascii_digit)
            .then(|| std::str::from_utf8(slice).ok()?.parse().ok())?
    };
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    let (digits, fraction) = if bytes.len() == 20 {
        (0, 0)
    } else {
        if bytes[19] != b'.' || bytes.len() > 30 || bytes.len() < 22 {
            return None;
        }
        let digits = bytes.len() - 21;
        (digits as u8, number(20..20 + digits)? as u64)
    };
    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second;
    let parts = (seconds, digits, fraction);
    (time_text(parts.0, parts.1, parts.2)? == text).then_some(parts)
}

fn time_text(seconds: i64, digits: u8, fraction: u64) -> Option<String> {
    if digits > 9 || fraction >= 10_u64.pow(u32::from(digits)) || (digits == 0 && fraction != 0) {
        return None;
    }
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    if !(0..=9999).contains(&year) {
        return None;
    }
    let mut text = format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    );
    if digits > 0 {
        text.push('.');
        text.push_str(&format!("{fraction:0width$}", width = usize::from(digits)));
    }
    text.push('Z');
    Some(text)
}

fn put_text(out: &mut Vec<u8>, tag: u8, text: &str) {
    out.push(tag);
    put_varint(out, text.len() as u64);
    out.extend_from_slice(text.as_bytes());
}

fn encode_string(text: &str, own: Option<&str>, out: &mut Vec<u8>) {
    if own == Some(text) {
        out.push(OWN_ID);
    } else if let Some(digest) = content_digest(text) {
        out.push(CONTENT_ID);
        out.extend_from_slice(&digest);
    } else if let Some((code, bytes)) = uuid_id(text) {
        out.push(UUID_ID);
        out.push(code);
        out.extend_from_slice(&bytes);
    } else if let Some((seconds, digits, fraction)) = time_parts(text) {
        out.push(TIME);
        put_varint(out, zigzag(seconds));
        out.push(digits);
        put_varint(out, fraction);
    } else if let Some(index) = WORDS.iter().position(|word| *word == text) {
        out.push(WORD);
        put_varint(out, index as u64);
    } else {
        put_text(out, TEXT, text);
    }
}

fn encode_value(value: &Value, own: Option<&str>, out: &mut Vec<u8>) -> Option<()> {
    match value {
        Value::Null => out.push(NULL),
        Value::Bool(false) => out.push(FALSE),
        Value::Bool(true) => out.push(TRUE),
        Value::Number(number) => match number.as_i64() {
            Some(integer) if (-INTEGER_MAX..=INTEGER_MAX).contains(&integer) => {
                out.push(INTEGER);
                put_varint(out, zigzag(integer));
            }
            _ => {
                let text = layrs_core::scm::canonical_json(number).ok()?;
                put_text(out, NUMBER, std::str::from_utf8(&text).ok()?);
            }
        },
        Value::String(text) => encode_string(text, own, out),
        Value::Array(values) => {
            out.push(ARRAY);
            put_varint(out, values.len() as u64);
            for value in values {
                encode_value(value, own, out)?;
            }
        }
        Value::Object(map) => {
            out.push(OBJECT);
            put_varint(out, map.len() as u64);
            for (key, value) in map {
                match KEYS.iter().position(|known| known == key) {
                    Some(index) => put_varint(out, index as u64 + 1),
                    None => {
                        put_varint(out, 0);
                        put_varint(out, key.len() as u64);
                        out.extend_from_slice(key.as_bytes());
                    }
                }
                encode_value(value, own, out)?;
            }
        }
    }
    Some(())
}

/// A record's binary body from its canonical JSON bytes; `own` is the
/// record's id text when its key names it (content records). `None` when
/// the bytes are not JSON.
pub(super) fn encode_value_binary(canonical: &[u8], own: Option<&str>) -> Option<Vec<u8>> {
    let value: Value = serde_json::from_slice(canonical).ok()?;
    let mut out = Vec::with_capacity(canonical.len() / 2);
    encode_value(&value, own, &mut out)?;
    Some(out)
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    own: Option<&'a str>,
}

impl Reader<'_> {
    fn varint(&mut self) -> Option<u64> {
        take_varint(self.bytes, &mut self.at)
    }

    fn take(&mut self, length: usize) -> Option<&[u8]> {
        let end = self
            .at
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())?;
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Some(slice)
    }

    fn text(&mut self) -> Option<&str> {
        let length = usize::try_from(self.varint()?).ok()?;
        std::str::from_utf8(self.take(length)?).ok()
    }

    /// The number of elements a count may name: each takes a byte at least.
    fn count(&mut self) -> Option<usize> {
        let count = usize::try_from(self.varint()?).ok()?;
        (count <= self.bytes.len() - self.at).then_some(count)
    }

    fn string(&mut self, tag: u8) -> Option<String> {
        Some(match tag {
            TEXT => self.text()?.to_owned(),
            WORD => (*WORDS.get(usize::try_from(self.varint()?).ok()?)?).to_owned(),
            CONTENT_ID => {
                let mut text = String::with_capacity(71);
                text.push_str("blake3:");
                push_hex(&mut text, self.take(32)?);
                text
            }
            UUID_ID => {
                let code = *self.take(1)?.first()?;
                let bytes: [u8; 16] = self.take(16)?.try_into().ok()?;
                uuid_text(code, &bytes)?
            }
            TIME => {
                let seconds = unzigzag(self.varint()?);
                let digits = *self.take(1)?.first()?;
                let fraction = self.varint()?;
                time_text(seconds, digits, fraction)?
            }
            OWN_ID => self.own?.to_owned(),
            _ => return None,
        })
    }

    fn value(&mut self, depth: usize) -> Option<Value> {
        if depth > MAX_DEPTH {
            return None;
        }
        let tag = *self.take(1)?.first()?;
        Some(match tag {
            NULL => Value::Null,
            FALSE => Value::Bool(false),
            TRUE => Value::Bool(true),
            INTEGER => Value::Number(Number::from(unzigzag(self.varint()?))),
            NUMBER => Value::Number(canonical_number(self.text()?)?),
            ARRAY => {
                let count = self.count()?;
                // The walk already bounded the size: no large reservation
                // on a count alone.
                let mut values = Vec::with_capacity(count.min(1024));
                for _ in 0..count {
                    values.push(self.value(depth + 1)?);
                }
                Value::Array(values)
            }
            OBJECT => {
                let count = self.count()?;
                let mut map = Map::new();
                let mut previous: Option<String> = None;
                for _ in 0..count {
                    let key = match usize::try_from(self.varint()?).ok()? {
                        0 => self.text()?.to_owned(),
                        index => (*KEYS.get(index - 1)?).to_owned(),
                    };
                    // Keys strictly increasing by bytes: one order whatever
                    // the map keeps (council s33).
                    if previous.as_ref().is_some_and(|previous| key <= *previous) {
                        return None;
                    }
                    previous = Some(key.clone());
                    let value = self.value(depth + 1)?;
                    map.insert(key, value);
                }
                Value::Object(map)
            }
            tag => Value::String(self.string(tag)?),
        })
    }
}

/// The number a `NUMBER` text spells, if it is the one encoding of it: its
/// JCS spelling, and not an integer the `INTEGER` tag carries. The walk
/// checks it too, so a body's size is the size of its canonical form
/// (council s33).
fn canonical_number(text: &str) -> Option<Number> {
    let number: Number = serde_json::from_str(text).ok()?;
    if number
        .as_i64()
        .is_some_and(|integer| (-INTEGER_MAX..=INTEGER_MAX).contains(&integer))
    {
        return None;
    }
    (layrs_core::scm::canonical_json(&number).ok()? == text.as_bytes()).then_some(number)
}

/// The canonical JSON bytes a binary record body decodes to, or `None`:
/// malformed, larger than `limit` once canonical, or not the one encoding
/// of its value (re-encoded and compared). The whole body is walked and
/// sized before any value is built (council s33). The caller validates the
/// bytes as the record they claim.
pub(super) fn decode_value_binary(body: &[u8], own: Option<&str>, limit: u64) -> Option<Vec<u8>> {
    if value_binary_expanded_len(body, own)? > limit {
        return None;
    }
    let mut reader = Reader {
        bytes: body,
        at: 0,
        own,
    };
    let value = reader.value(0)?;
    if reader.at != body.len() {
        return None;
    }
    let mut again = Vec::with_capacity(body.len());
    encode_value(&value, own, &mut again)?;
    if again != body {
        return None;
    }
    layrs_core::scm::canonical_json_value(&value).ok()
}

/// A string's bytes once escaped by serde_json (and so by JCS), quotes
/// included.
fn quoted_len(text: &[u8]) -> u64 {
    2 + text
        .iter()
        .map(|byte| match byte {
            b'"' | b'\\' | 0x08 | 0x09 | 0x0a | 0x0c | 0x0d => 2,
            0..=0x1f => 6,
            _ => 1,
        })
        .sum::<u64>()
}

fn decimal_len(value: i64) -> u64 {
    let mut length = u64::from(value < 0) + 1;
    let mut rest = value.unsigned_abs() / 10;
    while rest > 0 {
        length += 1;
        rest /= 10;
    }
    length
}

impl Reader<'_> {
    /// The JCS length of the next value, read without building it.
    fn expanded(&mut self, depth: usize) -> Option<u64> {
        if depth > MAX_DEPTH {
            return None;
        }
        let tag = *self.take(1)?.first()?;
        Some(match tag {
            NULL | TRUE => 4,
            FALSE => 5,
            INTEGER => decimal_len(unzigzag(self.varint()?)),
            NUMBER => {
                let text = self.text()?;
                canonical_number(text)?;
                text.len() as u64
            }
            TEXT => quoted_len(self.text()?.as_bytes()),
            WORD => quoted_len(WORDS.get(usize::try_from(self.varint()?).ok()?)?.as_bytes()),
            CONTENT_ID => {
                self.take(32)?;
                2 + 71
            }
            UUID_ID => {
                let code = *self.take(1)?.first()?;
                self.take(16)?;
                2 + PREFIXES.get(usize::from(code))?.len() as u64 + 1 + 36
            }
            TIME => {
                self.varint()?;
                let digits = *self.take(1)?.first()?;
                self.varint()?;
                if digits > 9 {
                    return None;
                }
                2 + 20 + if digits > 0 { 1 + u64::from(digits) } else { 0 }
            }
            OWN_ID => quoted_len(self.own?.as_bytes()),
            ARRAY => {
                let count = self.count()?;
                let mut length = 2 + count.saturating_sub(1) as u64;
                for _ in 0..count {
                    length = length.saturating_add(self.expanded(depth + 1)?);
                }
                length
            }
            OBJECT => {
                let count = self.count()?;
                let mut length = 2 + count.saturating_sub(1) as u64;
                for _ in 0..count {
                    let key = match usize::try_from(self.varint()?).ok()? {
                        0 => quoted_len(self.text()?.as_bytes()),
                        index => quoted_len(KEYS.get(index - 1)?.as_bytes()),
                    };
                    length = length
                        .saturating_add(key + 1)
                        .saturating_add(self.expanded(depth + 1)?);
                }
                length
            }
            _ => return None,
        })
    }
}

/// The exact length of the canonical JSON a well-formed binary record
/// decodes to, read without decoding it (budgets charge it before a
/// decode). `None` for a body that does not walk.
pub(super) fn value_binary_expanded_len(body: &[u8], own: Option<&str>) -> Option<u64> {
    let mut reader = Reader {
        bytes: body,
        at: 0,
        own,
    };
    let length = reader.expanded(0)?;
    (reader.at == body.len()).then_some(length)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(canonical: &[u8], own: Option<&str>) {
        let body = encode_value_binary(canonical, own).unwrap();
        assert_eq!(
            decode_value_binary(&body, own, u64::MAX).unwrap(),
            canonical,
            "{}",
            String::from_utf8_lossy(canonical)
        );
        assert_eq!(
            value_binary_expanded_len(&body, own).unwrap(),
            canonical.len() as u64
        );
        assert!(body.len() < canonical.len() || canonical.len() < 40);
    }

    /// Records of each kind with their own ids, then numbers, unknown keys,
    /// near-miss ids and times that must stay exact.
    fn samples() -> [(&'static [u8], Option<&'static str>); 5] {
        [
            (
                br#"{"kind":"step","objectId":"blake3:001f1407f734b7f52ab6def76d4977725e2fdf7094cd091de83ea2a325a4398e","schemaVersion":"layrs.cas-envelope.v1","value":{"parentStepIds":["blake3:bfa0540d101d5dee98cb52b007c368090b20b50f825e5a0df80fd4fe95f0e2d3"],"rootTreeId":"blake3:a396a13a4d763d78239604d0ecf53c1f0e8fef2d4d1c916f8e26091c551f575a","schemaVersion":"layrs.step.v1","stepId":"blake3:001f1407f734b7f52ab6def76d4977725e2fdf7094cd091de83ea2a325a4398e"}}"#,
                Some("blake3:001f1407f734b7f52ab6def76d4977725e2fdf7094cd091de83ea2a325a4398e"),
            ),
            (
                br#"{"kind":"repository_view","objectId":"blake3:0101c59eb26307fb40c0061730f72ce42ee7199e8c9a96129e3a23a8b27a0853","schemaVersion":"layrs.cas-envelope.v1","value":{"layerHeads":[{"layerId":"layer:6ccbc9ff-adb0-4197-9558-03206afcf1e5","stepIds":["blake3:13c7a1f07ebde9afae22d49a94c5422c7c4f92b5a747c1e564bb2d1b66290abf"]}],"markerIds":[],"releaseIds":[],"repositoryViewId":"blake3:0101c59eb26307fb40c0061730f72ce42ee7199e8c9a96129e3a23a8b27a0853","schemaVersion":"layrs.repository-view.v1","spaceId":"space:6ccbc9ff-adb0-4197-9558-03206afcf1e5","worktrees":[{"activeLayerId":"layer:6ccbc9ff-adb0-4197-9558-03206afcf1e5","baseStepId":"blake3:13c7a1f07ebde9afae22d49a94c5422c7c4f92b5a747c1e564bb2d1b66290abf","materializedTreeId":"blake3:14a1b632a54b71edc7e67796ba4131de9c7bbff105d066c5e0d1576d9acd4f9e","status":"ready","worktreeId":"worktree:d4ae26ee-1ef6-4c32-97a7-3b723da2e370"}]}}"#,
                Some("blake3:0101c59eb26307fb40c0061730f72ce42ee7199e8c9a96129e3a23a8b27a0853"),
            ),
            (br#"{"actorId":null,"createdAt":"2026-11-22T23:51:55.6826368Z","deviceId":null,"kind":"capture_worktree","metadata":[],"operationId":"operation:d9ca3a5f-9d45-4681-9fdc-6049560969ff","parentOperationIds":["operation:c6916971-2ddc-40ad-a074-bbb5c68c74d2"],"resultingViewId":"blake3:01b39dd5257187e509a5218de1182954fc83de3d381a9f8ce0f5e29fc848cc64","rewriteMap":[],"schemaVersion":"layrs.operation.v1","spaceId":"space:6ccbc9ff-adb0-4197-9558-03206afcf1e5","summary":"Capture Worktree"}"#, None),
            (br#"{"actorId":null,"captureReason":"manual_snapshot","deviceId":null,"metadata":[{"key":"a.b","value":"x\ny\u0001\"q\\"}],"observationId":"observation:ec315d7c-129a-4aea-8d20-f5ea12e6ef00","observedAt":"2026-11-22T23:51:57Z","operationId":"operation:ec315d7c-129a-4aea-8d20-f5ea12e6ef00","schemaVersion":"layrs.step-observation.v1","spaceId":"space:6ccbc9ff-adb0-4197-9558-03206afcf1e5","stepId":"blake3:5163cb173f212b026677770c1e971c0eb87f5b0ddbd41b82f4c4db5f1a7c8905","worktreeId":"worktree:d4ae26ee-1ef6-4c32-97a7-3b723da2e370"}"#, None),
            (
                r#"{"b":[false,true,null],"n":[0,-1,9007199254740991,9007199254740992,1.5,-0.25,1e+21],"odd":["blake3:ABC","space:6CCBC9FF-adb0-4197-9558-03206afcf1e5","unknown:6ccbc9ff-adb0-4197-9558-03206afcf1e5","2026-02-30T00:00:00Z","2026-11-22T23:51:57.0Z","2026-11-22T23:51:57.123456789Z","1969-12-31T23:59:59Z","é"],"z":{}}"#.as_bytes(),
                None,
            ),
        ]
    }

    /// Phase 3c: the records' canonical JSON round-trips through the
    /// binary form, which is smaller; its expansion is exact.
    #[test]
    fn records_round_trip() {
        for (canonical, own) in samples() {
            round_trip(canonical, own);
        }
    }

    /// The dictionaries, tag codes, varints and time layout are part of the
    /// format (council s33): a change to any needs a new draft, and these
    /// hashes with it.
    #[test]
    fn tables_are_pinned() {
        let mut hasher = blake3::Hasher::new();
        for table in [KEYS, WORDS, PREFIXES] {
            for entry in table {
                hasher.update(entry.as_bytes());
                hasher.update(&[0]);
            }
            hasher.update(&[1]);
        }
        assert_eq!(hasher.finalize().to_hex().as_str(), PINNED_TABLES);
        let mut hasher = blake3::Hasher::new();
        for (canonical, own) in samples() {
            let body = encode_value_binary(canonical, own).unwrap();
            hasher.update(&(body.len() as u64).to_le_bytes());
            hasher.update(&body);
        }
        assert_eq!(hasher.finalize().to_hex().as_str(), PINNED_ENCODINGS);
        assert_eq!(
            crate::scm_v1::schema::BASELINE_V4_NAME,
            "native_scm_v4_history_out_draft10",
            "a new draft pins its tables and encodings again"
        );
    }
    const PINNED_TABLES: &str = "b3b858ffa5bed20f6aacd9abe6db78b448391872666f4f1030cbcd188bffadb9";
    const PINNED_ENCODINGS: &str =
        "7770c42eb5a62562c0c0e52ec1e00ddeae1553f7db3931a5ac54e1cbca596cf1";

    /// Only the one encoding of a value decodes; random damage never
    /// panics, and whatever decodes re-encodes to itself.
    #[test]
    fn binary_values_refuse_other_encodings() {
        let operation = br#"{"actorId":null,"createdAt":"2026-11-22T23:51:55.6826368Z","kind":"capture_worktree","operationId":"operation:d9ca3a5f-9d45-4681-9fdc-6049560969ff","summary":"Capture Worktree"}"#;
        let body = encode_value_binary(operation, None).unwrap();
        // A word spelled as text, a padded varint, a trailing byte, a
        // duplicated key: refused.
        let mut text_word = body.clone();
        let word = text_word
            .windows(2)
            .position(|pair| {
                pair == [
                    WORD,
                    WORDS.iter().position(|w| *w == "Capture Worktree").unwrap() as u8,
                ]
            })
            .unwrap();
        text_word.splice(word..word + 2, {
            let mut text = vec![TEXT, 16];
            text.extend_from_slice(b"Capture Worktree");
            text
        });
        assert!(decode_value_binary(&text_word, None, u64::MAX).is_none());
        assert!(decode_value_binary(&[body.as_slice(), &[0]].concat(), None, u64::MAX).is_none());
        let padded = [vec![OBJECT, 0x85, 0x00], body[2..].to_vec()].concat();
        assert!(decode_value_binary(&padded, None, u64::MAX).is_none());
        let duplicate = [OBJECT, 2, 8, NULL, 8, NULL];
        // Keys out of order.
        let reversed = [OBJECT, 2, 9, NULL, 8, NULL];
        assert!(decode_value_binary(&reversed, None, u64::MAX).is_none());
        // Many values then a trailing byte: refused by the walk, before any
        // value is built (council s33); and a body past its limit.
        let mut many = vec![ARRAY];
        put_varint(&mut many, 100_000);
        many.extend(std::iter::repeat_n(OWN_ID, 100_000));
        let own = Some("blake3:0000000000000000000000000000000000000000000000000000000000000000");
        assert!(value_binary_expanded_len(&many, own).is_some());
        assert!(decode_value_binary(&[many.as_slice(), &[0]].concat(), own, u64::MAX).is_none());
        let canonical = value_binary_expanded_len(&many, own).unwrap();
        assert!(decode_value_binary(&many, own, canonical - 1).is_none());
        assert!(decode_value_binary(&many, own, canonical).is_some());
        assert!(decode_value_binary(&duplicate, None, u64::MAX).is_none());
        // A NUMBER not in its JCS spelling, or an integer the INTEGER tag
        // carries, does not walk: the walk sizes canonical forms only
        // (council s33 r2).
        for text in ["1e20", "1.50", "+1.5", "01.5", "1E+21", "5", "-0", "1.0"] {
            let mut number = vec![ARRAY];
            put_varint(&mut number, 3);
            for _ in 0..3 {
                put_text(&mut number, NUMBER, text);
            }
            assert_eq!(value_binary_expanded_len(&number, None), None, "{text}");
            assert!(
                decode_value_binary(&number, None, u64::MAX).is_none(),
                "{text}"
            );
        }
        let mut number = vec![ARRAY, 1];
        put_text(&mut number, NUMBER, "100000000000000000000");
        assert_eq!(value_binary_expanded_len(&number, None), Some(23));
        assert_eq!(
            decode_value_binary(&number, None, u64::MAX).unwrap(),
            b"[100000000000000000000]"
        );
        // Too deep.
        let deep: Vec<u8> = std::iter::repeat_n([ARRAY, 1], 40).flatten().collect();
        assert!(decode_value_binary(&[deep, vec![NULL]].concat(), None, u64::MAX).is_none());
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state.wrapping_mul(0x2545_f491_4f6c_dd1d)
        };
        // Every sample's encoding is damaged, so integers, numbers, ids
        // and own ids are fuzzed too (council s33 r3).
        let bodies: Vec<(Vec<u8>, Option<&str>)> = std::iter::once((body.clone(), None))
            .chain(
                samples()
                    .into_iter()
                    .map(|(canonical, own)| (encode_value_binary(canonical, own).unwrap(), own)),
            )
            .collect();
        for round in 0..30_000 {
            let (body, own) = &bodies[round % bodies.len()];
            let own = *own;
            let mut damaged = body.clone();
            let at = (next() % damaged.len() as u64) as usize;
            match next() % 3 {
                0 => damaged[at] ^= 1 << (next() % 8),
                1 => {
                    damaged.remove(at);
                }
                _ => damaged.insert(at, next() as u8),
            }
            if let Some(canonical) = decode_value_binary(&damaged, own, u64::MAX) {
                assert_eq!(encode_value_binary(&canonical, own).unwrap(), damaged);
                assert_eq!(
                    value_binary_expanded_len(&damaged, own).unwrap(),
                    canonical.len() as u64
                );
            }
            let _ = value_binary_expanded_len(&damaged, own);
        }
    }
}

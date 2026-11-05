use super::*;
use journal::Piece;
use rusqlite::{OptionalExtension, params};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) struct ExactInstallState {
    pub remaining: u64,
    pub debit_bytes: u64,
    pub records: u64,
    pub published: u64,
    pub content_bytes: u64,
    pub reserved_bytes: u64,
    pub limit_bytes: u64,
}

fn transaction(c: &Connection, write: bool) -> Result<()> {
    if c.is_autocommit()
        || c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))? != 2
        || write && c.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, u32>(0))? != 1
    {
        return Err(invalid(
            "exact catalog operation requires schema2 caller transaction",
        ));
    }
    Ok(())
}

pub(super) fn remaining(c: &Connection, binding: &ExactBinding) -> Result<u64> {
    let bounds: Option<(u64, u64)> = c.query_row("SELECT length(CAST(owner AS BLOB)),length(CAST(operation AS BLOB)) FROM reservations WHERE id=?1", [&binding.reservation], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    if !bounds.is_some_and(|(owner, operation)| owner <= 512 && operation <= 128) {
        return Err(invalid("exact reservation absent or exceeds bound"));
    }
    let (owner, operation, remaining): (String, String, u64) = c.query_row(
        "SELECT owner,operation,remaining FROM reservations WHERE id=?1",
        [&binding.reservation],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if owner != binding.owner.as_str()
        || operation != binding.operation
        || remaining > binding.quota_bytes
    {
        return Err(invalid(
            "exact reservation owner, operation or charge differs",
        ));
    }
    Ok(remaining)
}

pub(super) fn load(
    c: &Connection,
    binding: &ExactBinding,
    manifest: &FileManifest,
) -> Result<Option<ExactRecord>> {
    transaction(c, false)?;
    let bounds: Option<(u64, u64, u64, u64, bool)> = c.query_row(
        "SELECT length(CAST(file_id AS BLOB)),length(payload),coalesce(length(manifest),0),bytes,published FROM exact_installations WHERE reservation=?1 AND manifest_id=?2",
        params![binding.reservation, manifest.id().as_str()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    ).optional()?;
    let Some((file_len, payload_len, manifest_len, bytes, published)) = bounds else {
        return Ok(None);
    };
    if file_len != 71
        || payload_len > 8192
        || manifest_len > 262144
        || bytes != manifest.byte_length()
    {
        return Err(invalid("exact installation SQL bounds differ"));
    }
    let (file, payload, stored): (String, Vec<u8>, Option<Vec<u8>>) = c.query_row(
        "SELECT file_id,payload,manifest FROM exact_installations WHERE reservation=?1 AND manifest_id=?2", params![binding.reservation, manifest.id().as_str()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let record = ExactRecord::decode(&payload, manifest)?;
    let expected = record
        .verified()
        .then(|| journal::manifest_bytes(manifest))
        .transpose()?;
    if record.binding != *binding
        || record.manifest_id != *manifest.id()
        || file != manifest.file_id().as_str()
        || expected != stored
        || published != (record.phase == Phase::Published)
    {
        return Err(invalid("exact installation SQL projection differs"));
    }
    Ok(Some(record))
}

fn debit(c: &Connection, binding: &ExactBinding, bytes: u64) -> Result<()> {
    let before = remaining(c, binding)?;
    if before < bytes {
        return Err(invalid(
            "partial_store_quota: exact installation exceeds fixed reservation",
        ));
    }
    if bytes == 0 {
        return Ok(());
    }
    if c.execute("UPDATE reservations SET remaining=remaining-?1 WHERE id=?2 AND owner=?3 AND operation=?4 AND remaining=?5", params![bytes, binding.reservation, binding.owner.as_str(), binding.operation, before])? != 1 {
        return Err(invalid("exact reservation debit CAS differs"));
    }
    Ok(())
}

pub(super) fn insert(c: &Connection, record: &ExactRecord, manifest: &FileManifest) -> Result<()> {
    transaction(c, true)?;
    let payload = record.encode(manifest)?;
    if record.phase != Phase::Declared {
        return Err(invalid("exact initial journal is not Declared"));
    }
    if let Some(actual) = load(c, &record.binding, manifest)? {
        if actual == *record {
            return Ok(());
        }
        return Err(invalid(
            "exact installation already belongs to another attempt",
        ));
    }
    debit(c, &record.binding, PREPARE_CHARGE)?;
    if c.execute(
        "INSERT INTO exact_installations VALUES(?1,?2,?3,?4,?5,NULL,0)",
        params![
            record.binding.reservation,
            record.manifest_id.as_str(),
            record.file_id.as_str(),
            record.bytes,
            payload
        ],
    )? != 1
    {
        return Err(invalid("exact journal insertion count differs"));
    }
    Ok(())
}

pub(super) fn replace(
    c: &Connection,
    previous: &ExactRecord,
    next: &ExactRecord,
    manifest: &FileManifest,
) -> Result<()> {
    transaction(c, true)?;
    previous.validate(manifest)?;
    next.validate(manifest)?;
    let legal_phase = previous.phase == next.phase
        || matches!(
            (previous.phase, next.phase),
            (Phase::Declared, Phase::Allocated)
                | (Phase::Allocated, Phase::Verified)
                | (Phase::Verified, Phase::Publishing)
                | (Phase::Publishing, Phase::Published)
        );
    if !legal_phase
        || previous.binding != next.binding
        || previous.manifest_id != next.manifest_id
        || previous.file_id != next.file_id
        || previous.bytes != next.bytes
        || previous.directory != next.directory
        || previous.directory_identity.is_some()
            && previous.directory_identity != next.directory_identity
        || next.spool_cursor < previous.spool_cursor
        || next.spool_cursor > previous.spool_cursor + 1
        || next.publish_cursor < previous.publish_cursor
        || next.publish_cursor > previous.publish_cursor + 1
        || next.debit_bytes < previous.debit_bytes
    {
        return Err(invalid("exact journal transition differs"));
    }
    for n in 0..1024 {
        if previous.charged_chunk(n) && !next.charged_chunk(n)
            || previous.resident_chunk(n) && !next.resident_chunk(n)
        {
            return Err(invalid("exact journal bitmap regressed"));
        }
    }
    if previous.charged_manifest && !next.charged_manifest {
        return Err(invalid("exact manifest debit regressed"));
    }
    let shape = match (&previous.active, &next.active) {
        (None, Some(_)) => {
            previous.phase == next.phase
                && previous.spool_cursor == next.spool_cursor
                && previous.publish_cursor == next.publish_cursor
                && previous.debit_bytes == next.debit_bytes
        }
        (Some(before), Some(after)) => before == after && previous == next,
        (Some(active), None) => match (previous.phase, next.phase, active.piece) {
            (Phase::Allocated, Phase::Allocated, Piece::Chunk(n)) => {
                n == previous.spool_cursor
                    && next.spool_cursor == n + 1
                    && previous.publish_cursor == next.publish_cursor
            }
            (Phase::Allocated, Phase::Verified, Piece::Manifest) => {
                previous.spool_cursor == next.spool_cursor
                    && previous.publish_cursor == next.publish_cursor
            }
            (Phase::Publishing, Phase::Publishing, Piece::Chunk(n)) => {
                n == previous.publish_cursor
                    && next.publish_cursor == n + 1
                    && previous.spool_cursor == next.spool_cursor
            }
            (Phase::Publishing, Phase::Published, Piece::Manifest) => {
                previous.spool_cursor == next.spool_cursor
                    && previous.publish_cursor == next.publish_cursor
            }
            _ => false,
        },
        (None, None) => {
            previous == next
                || matches!(
                    (previous.phase, next.phase),
                    (Phase::Declared, Phase::Allocated) | (Phase::Verified, Phase::Publishing)
                ) && previous.spool_cursor == next.spool_cursor
                    && previous.publish_cursor == next.publish_cursor
                    && previous.debit_bytes == next.debit_bytes
        }
    };
    if !shape {
        return Err(invalid("exact journal active-inode transition differs"));
    }
    if load(c, &previous.binding, manifest)?.as_ref() != Some(previous) {
        return Err(invalid("exact journal predecessor differs"));
    }
    if previous == next {
        return Ok(());
    }
    let before = previous.encode(manifest)?;
    let after = next.encode(manifest)?;
    let old_manifest = previous
        .verified()
        .then(|| journal::manifest_bytes(manifest))
        .transpose()?;
    let new_manifest = next
        .verified()
        .then(|| journal::manifest_bytes(manifest))
        .transpose()?;
    debit(c, &next.binding, next.debit_bytes - previous.debit_bytes)?;
    if c.execute(
        "UPDATE exact_installations SET payload=?1,manifest=?2,published=?3 WHERE reservation=?4 AND manifest_id=?5 AND file_id=?6 AND bytes=?7 AND payload=?8 AND manifest IS ?9 AND published=?10",
        params![after, new_manifest, next.phase == Phase::Published, previous.binding.reservation, previous.manifest_id.as_str(), previous.file_id.as_str(), previous.bytes, before, old_manifest, previous.phase == Phase::Published],
    )? != 1 { return Err(invalid("exact journal complete row CAS differs")); }
    Ok(())
}

/// Check an indexed chunk's exact length, and refuse sweep ownership ambiguity.
pub(super) fn chunk_indexed(c: &Connection, id: &str, bytes: u64) -> Result<bool> {
    if c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sweep WHERE kind='chunks' AND id=?1)",
        [id],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(invalid("exact publication refuses pending chunk sweep"));
    }
    let actual: Option<u64> = c
        .query_row("SELECT bytes FROM chunks WHERE id=?1", [id], |r| r.get(0))
        .optional()?;
    match actual {
        Some(value) if value != bytes => Err(invalid("exact chunk catalog length collision")),
        Some(_) => Ok(true),
        None => Ok(false),
    }
}
pub(super) fn manifest_indexed(c: &Connection, manifest: &FileManifest) -> Result<bool> {
    if c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sweep WHERE kind='manifests' AND id=?1)",
        [manifest.id().as_str()],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(invalid("exact publication refuses pending manifest sweep"));
    }
    let bounds: Option<(u64, u64, u64)> = c
        .query_row(
            "SELECT length(CAST(file_id AS BLOB)),bytes,length(payload) FROM manifests WHERE id=?1",
            [manifest.id().as_str()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((file_len, bytes, payload_len)) = bounds else {
        return Ok(false);
    };
    if file_len != 71 || bytes != manifest.byte_length() || payload_len > 262144 {
        return Err(invalid("exact manifest catalog bound differs"));
    }
    let (file, payload): (String, Vec<u8>) = c.query_row(
        "SELECT file_id,payload FROM manifests WHERE id=?1",
        [manifest.id().as_str()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if file != manifest.file_id().as_str() || payload != journal::manifest_bytes(manifest)? {
        return Err(invalid("exact manifest catalog collision"));
    }
    let count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM manifest_chunks WHERE manifest_id=?1 LIMIT 1025)",
        [manifest.id().as_str()],
        |r| r.get(0),
    )?;
    if count != manifest.chunks().len() as u64 {
        return Err(invalid("exact manifest index count differs"));
    }
    for (n, chunk) in manifest.chunks().iter().enumerate() {
        let matches: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM manifest_chunks WHERE manifest_id=?1 AND ordinal=?2 AND chunk_id=?3)", params![manifest.id().as_str(), n as u64, chunk.chunk_id().as_str()], |r| r.get(0))?;
        if !matches || !chunk_indexed(c, chunk.chunk_id().as_str(), chunk.byte_length())? {
            return Err(invalid("exact manifest indexed chunk differs"));
        }
    }
    Ok(true)
}

pub(super) fn require_file_length(c: &Connection, manifest: &FileManifest) -> Result<()> {
    let bytes: Option<u64> = c
        .query_row(
            "SELECT bytes FROM files WHERE file_id=?1",
            [manifest.file_id().as_str()],
            |r| r.get(0),
        )
        .optional()?;
    if bytes.is_some_and(|bytes| bytes != manifest.byte_length()) {
        return Err(invalid("exact FileId length collision"));
    }
    Ok(())
}

pub(in crate::projection) fn validate_installations(
    c: &Connection,
    binding: &ExactBinding,
    required: &[FileManifest],
    cancel: Option<&Cancellation>,
) -> Result<ExactInstallState> {
    validate_installations_inner(c, binding, required, None, cancel)
}

/// Separate complete-Published reader for the protection phase. The ordinary
/// installation reader still requires absence of every destination root.
pub(in crate::projection) fn validate_protected_installations(
    c: &Connection,
    binding: &ExactBinding,
    required: &[FileManifest],
    protection: &crate::projection::shared::exact_protection::ExactProtection,
    cancel: Option<&Cancellation>,
) -> Result<ExactInstallState> {
    let state = validate_installations_inner(c, binding, required, Some(protection), cancel)?;
    if state.records != required.len() as u64 || state.published != required.len() as u64 {
        return Err(invalid(
            "exact protection requires complete Published manifest union",
        ));
    }
    for manifest in required {
        crate::projection::provider::cancellation::check(cancel)?;
        if !c.query_row(
            "SELECT EXISTS(SELECT 1 FROM files WHERE file_id=?1 AND bytes=?2)",
            params![manifest.file_id().as_str(), manifest.byte_length()],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(invalid("exact protection FileId length metadata differs"));
        }
    }
    Ok(state)
}

impl ExactBinding {
    /// Complete immutable Published group, for the typed admitted-family gate.
    /// Unlike the installation/replay reader this permits other groups and an
    /// ordinary suffix. It grants neither installation nor release authority.
    pub(in crate::projection) fn validate_retained_installations(
        &self,
        c: &Connection,
        required: &[FileManifest],
        protection: &crate::projection::shared::exact_protection::ExactProtection,
        charged: &mut BTreeSet<String>,
        charged_manifests: &mut BTreeSet<String>,
        cancel: Option<&Cancellation>,
    ) -> Result<ExactInstallState> {
        transaction(c, false)?;
        self.validate()?;
        protection.require_binding(self, required, cancel)?;
        if protection.inspect_retained(c, cancel)?
            != crate::projection::shared::exact_protection::ProtectionPresence::Present
        {
            return Err(invalid("retained exact protection root absent"));
        }
        let count: u64 = c.query_row(
            "SELECT count(*) FROM (SELECT 1 FROM exact_installations WHERE reservation=?1 LIMIT 100001)",
            [&self.reservation], |r| r.get(0),
        )?;
        if required.len() > 100_000
            || count != required.len() as u64
            || required
                .iter()
                .map(|m| m.id())
                .collect::<BTreeSet<_>>()
                .len()
                != required.len()
            || c.query_row(
                "SELECT EXISTS(SELECT 1 FROM installations WHERE reservation=?1)",
                [&self.reservation],
                |r| r.get::<_, bool>(0),
            )?
        {
            return Err(invalid("retained exact group membership differs"));
        }
        let mut debit_bytes = 0u64;
        for manifest in required {
            crate::projection::provider::cancellation::check(cancel)?;
            let record =
                load(c, self, manifest)?.ok_or_else(|| invalid("retained exact journal absent"))?;
            if record.phase != Phase::Published
                || !manifest_indexed(c, manifest)?
                || !c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM files WHERE file_id=?1 AND bytes=?2)",
                    params![manifest.file_id().as_str(), manifest.byte_length()],
                    |r| r.get::<_, bool>(0),
                )?
            {
                return Err(invalid("retained exact journal is not fully Published"));
            }
            if record.charged_manifest && !charged_manifests.insert(manifest.id().to_string()) {
                return Err(invalid("retained exact manifest has duplicate debit"));
            }
            for (n, chunk) in manifest.chunks().iter().enumerate() {
                crate::projection::provider::cancellation::check(cancel)?;
                if record.charged_chunk(n) {
                    // Bound aggregate memory across all retained groups before
                    // allocating another identifier. Per-manifest bounds alone
                    // permit 100000 * 1024 charged chunks across a family.
                    if charged.len() >= 1_000_000 {
                        return Err(invalid("retained exact charged chunk union exceeds bound"));
                    }
                    if !charged.insert(chunk.chunk_id().to_string()) {
                        return Err(invalid("retained exact chunk has duplicate debit"));
                    }
                }
            }
            debit_bytes = debit_bytes
                .checked_add(record.debit_bytes)
                .ok_or_else(|| invalid("retained exact debit overflow"))?;
        }
        let remaining = remaining(c, self)?;
        if self.quota_bytes.checked_sub(debit_bytes) != Some(remaining) {
            return Err(invalid(
                "retained exact reservation differs from durable debit",
            ));
        }
        // Include ALL groups in the family's usage; charging just this group's
        // journals would incorrectly create capacity when groups coexist.
        let base: u64 = c.query_row("SELECT coalesce((SELECT sum(bytes) FROM chunks),0)+coalesce((SELECT sum(length(payload)) FROM manifests),0)+coalesce((SELECT sum(bytes) FROM sweep),0)", [], |r| r.get(0))?;
        let (records, journal_bytes): (u64, u64) = c.query_row(
            "SELECT count(*),coalesce(sum(length(manifest)),0) FROM exact_installations",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let content_bytes = records
            .checked_mul(PREPARE_CHARGE)
            .and_then(|n| n.checked_add(journal_bytes))
            .and_then(|n| n.checked_add(base))
            .ok_or_else(|| invalid("retained exact content charge overflow"))?;
        let reserved_bytes: u64 = c.query_row(
            "SELECT coalesce(sum(remaining),0) FROM reservations",
            [],
            |r| r.get(0),
        )?;
        let limit_bytes: u64 =
            c.query_row("SELECT bytes FROM quota WHERE singleton=1", [], |r| {
                r.get(0)
            })?;
        if content_bytes
            .checked_add(reserved_bytes)
            .is_none_or(|n| n > limit_bytes)
        {
            return Err(invalid(
                "partial_store_quota: retained exact family is overcommitted",
            ));
        }
        Ok(ExactInstallState {
            remaining,
            debit_bytes,
            records: count,
            published: count,
            content_bytes,
            reserved_bytes,
            limit_bytes,
        })
    }
}

fn validate_installations_inner(
    c: &Connection,
    binding: &ExactBinding,
    required: &[FileManifest],
    protection: Option<&crate::projection::shared::exact_protection::ExactProtection>,
    cancel: Option<&Cancellation>,
) -> Result<ExactInstallState> {
    transaction(c, false)?;
    binding.validate()?;
    if required.len() > 100_000 {
        return Err(invalid("exact manifest union exceeds bound"));
    }
    let by_id: BTreeMap<_, _> = required
        .iter()
        .map(|manifest| (manifest.id().as_str(), manifest))
        .collect();
    if by_id.len() != required.len() {
        return Err(invalid("exact manifest union contains duplicate ID"));
    }
    if let Some(protection) = protection {
        protection.require_binding(binding, required, cancel)?;
        protection.inspect(c, cancel)?;
    } else if c.query_row(
        "SELECT EXISTS(SELECT 1 FROM roots WHERE owner=?1)",
        [binding.owner.as_str()],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(invalid(
            "exact family contains unsupported owner effect or foreign attempt",
        ));
    }
    for (sql, args) in [
        (
            "SELECT EXISTS(SELECT 1 FROM installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1)",
            binding.owner.as_str(),
        ),
        (
            "SELECT EXISTS(SELECT 1 FROM exact_installations WHERE reservation!=?1)",
            binding.reservation.as_str(),
        ),
    ] {
        if c.query_row(sql, [args], |r| r.get::<_, bool>(0))? {
            return Err(invalid(
                "exact family contains unsupported owner effect or foreign attempt",
            ));
        }
    }
    if c.query_row(
        "SELECT EXISTS(SELECT 1 FROM reservations WHERE owner=?1 AND id!=?2)",
        params![binding.owner.as_str(), binding.reservation],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(invalid("exact owner has foreign reservation"));
    }
    let group = validate_group_records(
        c,
        binding,
        &by_id,
        &mut BTreeSet::new(),
        &mut BTreeSet::new(),
        cancel,
    )?;
    let GroupState {
        remaining,
        debit_bytes,
        records: count,
        published,
        journal_manifests,
    } = group;
    let base: u64 = c.query_row("SELECT coalesce((SELECT sum(bytes) FROM chunks),0)+coalesce((SELECT sum(length(payload)) FROM manifests),0)+coalesce((SELECT sum(bytes) FROM sweep),0)", [], |r| r.get(0))?;
    let content_bytes = base
        .checked_add(
            count
                .checked_mul(PREPARE_CHARGE)
                .ok_or_else(|| invalid("exact metadata charge overflow"))?,
        )
        .and_then(|bytes| bytes.checked_add(journal_manifests))
        .ok_or_else(|| invalid("exact content charge overflow"))?;
    let reserved_bytes: u64 = c.query_row(
        "SELECT coalesce(sum(remaining),0) FROM reservations",
        [],
        |r| r.get(0),
    )?;
    let limit_bytes: u64 = c.query_row("SELECT bytes FROM quota WHERE singleton=1", [], |r| {
        r.get(0)
    })?;
    if content_bytes
        .checked_add(reserved_bytes)
        .is_none_or(|bytes| bytes > limit_bytes)
    {
        return Err(invalid(
            "partial_store_quota: exact family is overcommitted",
        ));
    }
    Ok(ExactInstallState {
        remaining,
        debit_bytes,
        records: count,
        published,
        content_bytes,
        reserved_bytes,
        limit_bytes,
    })
}

// Only typed group interpretation. Full-family classification stays with the
// strict wrapper or admitted_catalog's retained-plus-active walk.
struct GroupState {
    remaining: u64,
    debit_bytes: u64,
    records: u64,
    published: u64,
    journal_manifests: u64,
}

fn validate_group_records(
    c: &Connection,
    binding: &ExactBinding,
    by_id: &BTreeMap<&str, &FileManifest>,
    charged_chunks: &mut BTreeSet<String>,
    charged_manifests: &mut BTreeSet<String>,
    cancel: Option<&Cancellation>,
) -> Result<GroupState> {
    let (count, max_id, max_file, max_payload, max_manifest): (u64,u64,u64,u64,u64) = c.query_row("SELECT count(*),coalesce(max(length(CAST(manifest_id AS BLOB))),0),coalesce(max(length(CAST(file_id AS BLOB))),0),coalesce(max(length(payload)),0),coalesce(max(length(manifest)),0) FROM (SELECT * FROM exact_installations WHERE reservation=?1 LIMIT 100001)", [&binding.reservation], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
    if count > by_id.len() as u64
        || max_id > 71
        || max_file > 71
        || max_payload > 8192
        || max_manifest > 262144
    {
        return Err(invalid("exact installation rowset exceeds bound"));
    }
    let mut debit_bytes = 0u64;
    let mut published = 0u64;
    let mut journal_manifests = 0u64;
    let mut query =
        c.prepare("SELECT manifest_id FROM exact_installations WHERE reservation=?1 ORDER BY manifest_id LIMIT 100001")?;
    let mut rows = query.query([&binding.reservation])?;
    while let Some(row) = rows.next()? {
        crate::projection::provider::cancellation::check(cancel)?;
        let id: String = row.get(0)?;
        let manifest = by_id
            .get(id.as_str())
            .ok_or_else(|| invalid("exact journal is outside complete manifest union"))?;
        let record =
            load(c, binding, manifest)?.ok_or_else(|| invalid("exact installation disappeared"))?;
        debit_bytes = debit_bytes
            .checked_add(record.debit_bytes)
            .ok_or_else(|| invalid("exact aggregate debit overflow"))?;
        if record.charged_manifest && !charged_manifests.insert(manifest.id().to_string()) {
            return Err(invalid(
                "exact manifest was debited by multiple installation rows",
            ));
        }
        if record.verified() {
            journal_manifests = journal_manifests
                .checked_add(journal::manifest_bytes(manifest)?.len() as u64)
                .ok_or_else(|| invalid("exact metadata charge overflow"))?;
        }
        for (n, chunk) in manifest.chunks().iter().enumerate() {
            crate::projection::provider::cancellation::check(cancel)?;
            if n < record.publish_cursor as usize
                && !chunk_indexed(c, chunk.chunk_id().as_str(), chunk.byte_length())?
            {
                return Err(invalid("exact published prefix lost its chunk"));
            }
            if record.charged_chunk(n) && charged_chunks.len() >= 1_000_000 {
                return Err(invalid("exact charged chunk union exceeds bound"));
            }
            if record.charged_chunk(n) && !charged_chunks.insert(chunk.chunk_id().to_string()) {
                return Err(invalid(
                    "exact chunk was debited by multiple installation rows",
                ));
            }
            if record.resident_chunk(n)
                && !chunk_indexed(c, chunk.chunk_id().as_str(), chunk.byte_length())?
            {
                return Err(invalid("exact resident spool chunk lost its index"));
            }
        }
        if let Some(active) = &record.active {
            if active.from_cas {
                let Piece::Chunk(n) = active.piece else {
                    return Err(invalid("exact resident active object is not a chunk"));
                };
                let chunk = &manifest.chunks()[n as usize];
                if !chunk_indexed(c, chunk.chunk_id().as_str(), chunk.byte_length())? {
                    return Err(invalid("exact active resident chunk lost its index"));
                }
            }
        }
        if record.phase == Phase::Published {
            if !manifest_indexed(c, manifest)? {
                return Err(invalid(
                    "exact Published journal lacks exact indexed manifest",
                ));
            }
            published += 1;
        }
    }
    let remaining = remaining(c, binding)?;
    if binding.quota_bytes.checked_sub(debit_bytes) != Some(remaining) {
        return Err(invalid(
            "exact reservation does not equal quota minus durable debits",
        ));
    }
    Ok(GroupState {
        remaining,
        debit_bytes,
        records: count,
        published,
        journal_manifests,
    })
}

impl ExactBinding {
    /// SQL DATA only for one active group, after the caller's complete retained
    /// walk. Share both debit sets across ALL retained and active groups; this
    /// method alone neither classifies foreign groups nor grants byte authority.
    pub(in crate::projection) fn validate_export_installations(
        &self,
        c: &Connection,
        required: &[FileManifest],
        charged_chunks: &mut BTreeSet<String>,
        charged_manifests: &mut BTreeSet<String>,
        cancel: Option<&Cancellation>,
    ) -> Result<ExactInstallState> {
        transaction(c, false)?;
        self.validate()?;
        crate::projection::provider::cancellation::check(cancel)?;
        if required.len() > 100_000 {
            return Err(invalid("exact export manifest union exceeds bound"));
        }
        let mut by_id = BTreeMap::new();
        for manifest in required {
            crate::projection::provider::cancellation::check(cancel)?;
            journal::manifest_bytes(manifest)?;
            if by_id.insert(manifest.id().as_str(), manifest).is_some() {
                return Err(invalid("exact export manifest union contains duplicate ID"));
            }
        }
        let group =
            validate_group_records(c, self, &by_id, charged_chunks, charged_manifests, cancel)?;
        // ALL groups contribute metadata, even if C has not started yet.
        let records: u64 = c.query_row(
            "SELECT count(*) FROM (SELECT 1 FROM exact_installations LIMIT 100001)",
            [],
            |r| r.get(0),
        )?;
        if records > 100_000 {
            return Err(invalid("exact export global journal count exceeds bound"));
        }
        let journal_bytes: u64 = c.query_row(
            "SELECT coalesce(sum(length(manifest)),0) FROM exact_installations",
            [],
            |r| r.get(0),
        )?;
        let base: u64 = c.query_row("SELECT coalesce((SELECT sum(bytes) FROM chunks),0)+coalesce((SELECT sum(length(payload)) FROM manifests),0)+coalesce((SELECT sum(bytes) FROM sweep),0)", [], |r| r.get(0))?;
        let content_bytes = records
            .checked_mul(PREPARE_CHARGE)
            .and_then(|n| n.checked_add(journal_bytes))
            .and_then(|n| n.checked_add(base))
            .ok_or_else(|| invalid("exact export global content charge overflow"))?;
        let reserved_bytes: u64 = c.query_row(
            "SELECT coalesce(sum(remaining),0) FROM reservations",
            [],
            |r| r.get(0),
        )?;
        let limit_bytes: u64 =
            c.query_row("SELECT bytes FROM quota WHERE singleton=1", [], |r| {
                r.get(0)
            })?;
        if content_bytes
            .checked_add(reserved_bytes)
            .is_none_or(|n| n > limit_bytes)
        {
            return Err(invalid(
                "partial_store_quota: exact export family is overcommitted",
            ));
        }
        crate::projection::provider::cancellation::check(cancel)?;
        Ok(ExactInstallState {
            remaining: group.remaining,
            debit_bytes: group.debit_bytes,
            records: group.records,
            published: group.published,
            content_bytes,
            reserved_bytes,
            limit_bytes,
        })
    }
}

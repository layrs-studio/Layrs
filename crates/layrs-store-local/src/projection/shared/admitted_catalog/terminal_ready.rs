//! Autonomous terminal ordinary Ready catalog DATA. The caller authenticates
//! the actual C8 Ready/receipt, complete private union and retained physical
//! family pins. No source owner, admission, private opener or byte authority is
//! inferred here; every retained SQL group is interpreted in this snapshot.
use super::*;

/// Complete family proof for one exact terminal Ready member. Caller retains
/// its catalog gate/transaction and authenticates C's private commitments and
/// metadata/checkout envelope independently, before and after physical effects.
#[allow(clippy::too_many_arguments)]
pub(in crate::projection) fn validate_terminal_ready_catalog(
    c: &Connection,
    family_marker: &[u8],
    marker: &format::Marker,
    binding: &ExactBinding,
    required: &[FileManifest],
    protection: &ExactProtection,
    cancel: Option<&Cancellation>,
) -> Result<ExactInstallState> {
    cancellation::check(cancel)?;
    if c.is_autocommit()
        || family_marker.len() > 65536
        || c.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, u64>(0))? != 1
    {
        return Err(invalid(
            "terminal Ready requires bounded marker and FK-enabled caller transaction",
        ));
    }
    if super::super::provider_access::require_provider_catalog_schema(c, cancel)?
        != super::super::CatalogSchema::V2
    {
        return Err(invalid("terminal Ready requires schema2"));
    }
    binding.validate()?;
    marker.validate().map_err(|e| invalid(e.to_string()))?;
    protection.require_binding(binding, required, cancel)?;
    let family: FamilyMarker =
        serde_json::from_slice(family_marker).map_err(|e| invalid(e.to_string()))?;
    let stored: Vec<u8> = c.query_row(
        "SELECT marker FROM family WHERE singleton=1 AND length(marker)<=65536",
        [],
        |r| r.get(0),
    )?;
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-projection-recovery-family-v1\0");
    hash.update(family_marker);
    let digest = format!("blake3:{}", hash.finalize());
    if canonical_json(&family).map_err(|e| invalid(e.to_string()))? != family_marker
        || stored != family_marker
        || digest != binding.family_marker_digest
        || family.incarnation != binding.family_incarnation
        || family.root_identity != binding.family_root_identity
        || family.database_identity != binding.family_catalog_identity
    {
        return Err(invalid("terminal Ready family marker differs"));
    }
    if c.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(invalid("terminal Ready family foreign key violation"));
    }
    // C is known from genuine terminal private DATA, including an empty union.
    // This does not construct an admitted-family context or admission digest.
    require_member(c, &family, marker, binding, marker)?;
    let marker_bytes = marker.bytes().map_err(|e| invalid(e.to_string()))?;
    if marker_bytes.len() > 65536
        || !c.query_row(
            "SELECT EXISTS(SELECT 1 FROM worktrees WHERE id=?1 AND state='ready' AND marker=?2 AND length(marker)<=65536 AND length(CAST(metadata_path AS BLOB))<=32768 AND length(CAST(database_identity AS BLOB))<=512)",
            params![binding.owner.as_str(), marker_bytes],
            |r| r.get::<_, bool>(0),
        )?
    {
        return Err(invalid("terminal Ready exact member FORMAT differs"));
    }
    if protection.inspect(c, cancel)? != ProtectionPresence::Present {
        return Err(invalid("terminal Ready exact protection absent"));
    }
    if c.query_row(
        "SELECT EXISTS(SELECT 1 FROM reservations WHERE owner=?1 AND id!=?2)",
        params![binding.owner.as_str(), binding.reservation],
        |r| r.get::<_, bool>(0),
    )? || c.query_row(
        "SELECT EXISTS(SELECT 1 FROM installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1)",
        [binding.owner.as_str()],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(invalid("terminal Ready has additional reservation or legacy installation effects"));
    }
    let journal_count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM exact_installations LIMIT 100001)",
        [],
        |r| r.get(0),
    )?;
    if journal_count > MAX_MANIFESTS {
        return Err(invalid("terminal Ready family journal count exceeds bound"));
    }
    // Classify every B companion namespace without opening B. It neither
    // supplies C authority nor participates in C's exact debit/charged sets.
    super::super::source_companion::retained_namespace(c, cancel, |source| {
        let b = &source.binding.destination;
        if b.family_incarnation != family.incarnation
            || b.family_root_identity != family.root_identity
            || b.family_catalog_identity != family.database_identity
            || b.family_marker_digest != digest
        {
            return Err(invalid("terminal Ready source retention family differs"));
        }
        let id = super::source_destination_root_id(c, b)?;
        if id == protection.id() {
            if *b != *binding {
                return Err(invalid(
                    "terminal Ready B retention names a different C binding",
                ));
            }
            source.require_union(required, cancel)?;
        } else {
            let (actual, manifests, member) = read_group(c, &id, None, cancel)?;
            if actual != *b {
                return Err(invalid("terminal Ready foreign B/C retention differs"));
            }
            source.require_union(&manifests, cancel)?;
            require_member(c, &family, marker, b, &member)?;
        }
        Ok(())
    })?;
    let mut roots = BTreeSet::new();
    let mut groups = BTreeSet::new();
    let mut charged_chunks = BTreeSet::new();
    let mut charged_manifests = BTreeSet::new();
    let mut total_manifests = 0u64;
    let mut owned = None;
    let mut query = c.prepare(
        "SELECT id FROM roots WHERE operation LIKE 'provider-import%' ORDER BY id LIMIT 4097",
    )?;
    let mut rows = query.query([])?;
    while let Some(row) = rows.next()? {
        cancellation::check(cancel)?;
        let id = row
            .get_ref(0)?
            .as_str()
            .map_err(|e| invalid(e.to_string()))?;
        if id.len() != 64 || roots.len() >= MAX_GROUPS || !roots.insert(id.to_owned()) {
            return Err(invalid("terminal Ready import root key/count differs"));
        }
        if id == protection.id() {
            total_manifests = add_manifests(total_manifests, required.len())?;
            if !groups.insert(binding.reservation.clone()) {
                return Err(invalid("terminal Ready import reservation repeats"));
            }
            let state = binding.validate_retained_installations(
                c,
                required,
                protection,
                &mut charged_chunks,
                &mut charged_manifests,
                cancel,
            )?;
            owned = Some(state);
        } else {
            // Unknown empty retained imports remain fail-closed. Nonempty
            // foreign roots identify their binding through Published journals.
            let (other, manifests, member) = read_group(c, id, None, cancel)?;
            require_member(c, &family, marker, &other, &member)?;
            if other.family_marker_digest != digest || !groups.insert(other.reservation.clone()) {
                return Err(invalid(
                    "terminal Ready retained import binding repeats or differs",
                ));
            }
            total_manifests = add_manifests(total_manifests, manifests.len())?;
            let root = ExactProtection::new(
                &other.family_incarnation,
                &other.owner,
                &other.database_identity,
                &other.transaction_id,
                &manifests,
                cancel,
            )?;
            other.validate_retained_installations(
                c,
                &manifests,
                &root,
                &mut charged_chunks,
                &mut charged_manifests,
                cancel,
            )?;
        }
    }
    // Every journal reservation and every transition reservation must have one
    // validated root. Ordinary reservations of other owners coexist unchanged.
    let mut query = c.prepare("SELECT id FROM reservations WHERE operation LIKE 'provider-transition%' OR id IN (SELECT reservation FROM exact_installations) LIMIT 4097")?;
    let mut rows = query.query([])?;
    let mut seen = BTreeSet::new();
    while let Some(row) = rows.next()? {
        cancellation::check(cancel)?;
        let id = row
            .get_ref(0)?
            .as_str()
            .map_err(|e| invalid(e.to_string()))?;
        if id.len() > 128 || seen.len() >= MAX_GROUPS || !groups.contains(id) {
            return Err(invalid("terminal Ready has unknown retained exact group"));
        }
        seen.insert(id.to_owned());
    }
    if seen != groups {
        return Err(invalid("terminal Ready retained reservation absent"));
    }
    let state = owned.ok_or_else(|| invalid("terminal Ready exact import root absent"))?;
    if state.records != required.len() as u64 || state.published != required.len() as u64 {
        return Err(invalid("terminal Ready complete Published union differs"));
    }
    // Independently include all CAS, sweep, journal metadata and ordinary
    // reservations in accounting; no group-local capacity is manufactured.
    let usage = export::global_usage(c, 0, 0, cancel)?;
    if state.content_bytes != usage.content_bytes
        || state.reserved_bytes != usage.reserved_bytes
        || state.limit_bytes != usage.limit_bytes
    {
        return Err(invalid("terminal Ready complete family accounting differs"));
    }
    cancellation::check(cancel)?;
    Ok(state)
}

fn add_manifests(total: u64, count: usize) -> Result<u64> {
    total
        .checked_add(count as u64)
        .filter(|n| *n <= MAX_MANIFESTS)
        .ok_or_else(|| invalid("terminal Ready family manifest union exceeds bound"))
}

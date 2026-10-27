use super::*;

pub(super) fn read_group(
    c: &Connection,
    id: &str,
    known: Option<&ImportContext>,
    cancel: Option<&Cancellation>,
) -> Result<(ExactBinding, Vec<FileManifest>, format::Marker)> {
    if c.query_row("SELECT EXISTS(SELECT 1 FROM roots WHERE id=?1 AND (length(CAST(owner AS BLOB))>512 OR length(CAST(database_identity AS BLOB))>512 OR length(CAST(operation AS BLOB))>128 OR length(CAST(digest AS BLOB))>128 OR length(payload)>16777216))", [id], |r| r.get::<_,bool>(0))? {
        return Err(invalid("admitted family import root metadata exceeds bound"));
    }
    let payload: Vec<u8> =
        c.query_row("SELECT payload FROM roots WHERE id=?1", [id], |r| r.get(0))?;
    let root: super::super::content::Protection =
        serde_json::from_slice(&payload).map_err(|e| invalid(e.to_string()))?;
    let transaction = root
        .operation
        .strip_prefix("provider-import:")
        .ok_or_else(|| invalid("admitted family import root operation differs"))?;
    let expected = ExactProtection::new(
        &root.incarnation,
        &root.owner,
        &root.database_identity,
        transaction,
        &root.manifests,
        cancel,
    )?;
    if expected.id() != id || expected.payload() != payload {
        return Err(invalid("admitted family import root is not canonical"));
    }
    let binding = if let Some(known) = known {
        let actual: BTreeSet<_> = root
            .manifests
            .iter()
            .map(|m| (m.file_id().to_string(), m.id().to_string(), m.byte_length()))
            .collect();
        if actual != known.required || actual.len() != root.manifests.len() {
            return Err(invalid(
                "admitted family import root differs from private exact union",
            ));
        }
        known.binding.clone()
    } else {
        // A nonempty foreign root can identify its group via a retained exact
        // Published record. This proves its family retention only, never that
        // another member has schema9 admission or ordinary mutation authority.
        let first = root.manifests.first().ok_or_else(|| {
            invalid("unknown empty retained import requires prevalidated admitted context")
        })?;
        let mut query = c.prepare("SELECT e.payload FROM exact_installations e JOIN reservations r ON r.id=e.reservation WHERE r.owner=?1 AND e.manifest_id=?2 AND length(e.payload)<=8192 ORDER BY e.reservation LIMIT 2")?;
        let mut rows = query.query(params![root.owner.as_str(), first.id().as_str()])?;
        let row = rows
            .next()?
            .ok_or_else(|| invalid("retained import group journal absent"))?;
        let payload: Vec<u8> = row.get(0)?;
        let record: ExactRecord =
            serde_json::from_slice(&payload).map_err(|e| invalid(e.to_string()))?;
        if rows.next()?.is_some() {
            return Err(invalid("retained import group journal is ambiguous"));
        }
        record.binding
    };
    expected.require_binding(&binding, &root.manifests, cancel)?;
    // Bound external registration before materializing the FORMAT/path strings.
    if !c.query_row("SELECT EXISTS(SELECT 1 FROM worktrees WHERE id=?1 AND length(marker)<=65536 AND length(CAST(metadata_path AS BLOB))<=32768 AND length(CAST(database_identity AS BLOB))<=512 AND state='ready')", [binding.owner.as_str()], |r| r.get::<_,bool>(0))? {
        return Err(invalid("admitted family import member is not bounded Ready"));
    }
    let bytes: Vec<u8> = c.query_row(
        "SELECT marker FROM worktrees WHERE id=?1",
        [binding.owner.as_str()],
        |r| r.get(0),
    )?;
    let member: format::Marker =
        serde_json::from_slice(&bytes).map_err(|e| invalid(e.to_string()))?;
    member.validate().map_err(|e| invalid(e.to_string()))?;
    if member.bytes().map_err(|e| invalid(e.to_string()))? != bytes
        || known.is_some_and(|known| known.marker != member)
    {
        return Err(invalid("admitted family import member FORMAT differs"));
    }
    Ok((binding, root.manifests, member))
}

pub(super) fn require_member(
    c: &Connection,
    family: &FamilyMarker,
    primary: &format::Marker,
    binding: &ExactBinding,
    member: &format::Marker,
) -> Result<()> {
    binding.validate()?;
    let shared = member
        .shared
        .as_ref()
        .ok_or_else(|| invalid("admitted family member shared binding absent"))?;
    let primary_shared = primary
        .shared
        .as_ref()
        .ok_or_else(|| invalid("admitted primary shared binding absent"))?;
    if binding.owner != member.identity.worktree_id
        || binding.database_identity != shared.database_identity
        || binding.family_incarnation != family.incarnation
        || binding.family_incarnation != shared.incarnation
        || binding.family_root_identity != family.root_identity
        || binding.family_root_identity != shared.root_identity
        || binding.family_catalog_identity != family.database_identity
        || shared.root != primary_shared.root
        || member.import_digest.as_deref() != Some(binding.prefix_id.as_str())
        || member.identity.store_id != family.identity.store_id
        || member.identity.space_id != family.identity.space_id
        || member.identity.origin != family.identity.origin
        || member.identity.principal_id != family.identity.principal_id
    {
        return Err(invalid(
            "admitted family member differs from exact group or family",
        ));
    }
    if !c.query_row("SELECT EXISTS(SELECT 1 FROM worktrees WHERE id=?1 AND metadata_path=?2 AND database_identity=?3 AND state='ready')",
        params![binding.owner.as_str(), shared.metadata_path.to_string_lossy(), binding.database_identity], |r| r.get::<_,bool>(0))?
    { return Err(invalid("admitted family member registration differs from FORMAT")); }
    Ok(())
}

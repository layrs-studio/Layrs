//! Journal decoding, lease checks and evidence validation.
use super::*;

/// Wall time may move backwards (VM clock correction, restart). Journal writes
/// retain the latest recorded instant; owner/generation/expiry checks still fence
/// every transition. This does not authorize takeover of another live lease.
pub(super) fn ordered_time(
    transaction: &Transaction<'_>,
    record: &RecoveryRecord,
    requested: i64,
) -> RecoveryResult<i64> {
    let heartbeat: Option<i64> = transaction.query_row(
        "SELECT MAX(heartbeat_at_ms) FROM materialization_leases WHERE recovery_operation_id=?1 AND worktree_id=?2",
        params![record.id.as_str(), record.worktree_id.as_str()], |row| row.get(0))?;
    Ok(requested
        .max(record.updated_at_ms)
        .max(heartbeat.unwrap_or(0)))
}
pub(super) fn load_record(
    connection: &rusqlite::Connection,
    id: &RecoveryId,
) -> RecoveryResult<RecoveryRecord> {
    type Row = (
        String,
        String,
        String,
        Option<String>,
        String,
        String,
        String,
        String,
        u64,
        Option<String>,
        Option<String>,
        i64,
        i64,
        Option<i64>,
    );
    let row: Option<Row> = connection
        .query_row(
            "SELECT idempotency_key, request_digest, worktree_id, operation_id,
                    recovery_type, state, payload_json, evidence_json, attempt_count,
                    owner_token, last_error, created_at_ms, updated_at_ms, completed_at_ms
             FROM recovery_operations WHERE recovery_operation_id = ?1",
            [id.as_str()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                    row.get(13)?,
                ))
            },
        )
        .optional()?;
    let Some((
        idempotency_key,
        request_digest,
        worktree_id,
        operation_id,
        recovery_type,
        state,
        payload_json,
        evidence_json,
        attempt_count,
        owner_token,
        last_error,
        created_at_ms,
        updated_at_ms,
        completed_at_ms,
    )) = row
    else {
        return Err(RecoveryError::NotFound(id.clone()));
    };
    let mut statement = connection.prepare(
        "SELECT path, path_role, expected_object_id, state, evidence_json
         FROM recovery_paths WHERE recovery_operation_id = ?1 ORDER BY path, path_role",
    )?;
    let path_rows = statement
        .query_map([id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let paths = path_rows
        .into_iter()
        .map(|(path, role, expected_object_id, state, evidence)| {
            Ok(RecoveryPathRecord {
                path,
                role: RecoveryPathRole::parse(&role)?,
                expected_object_id,
                state: RecoveryPathState::parse(&state)?,
                evidence: serde_json::from_str(&evidence)
                    .map_err(|error| RecoveryError::Corruption(error.to_string()))?,
            })
        })
        .collect::<RecoveryResult<Vec<_>>>()?;
    Ok(RecoveryRecord {
        id: id.clone(),
        idempotency_key,
        request_digest,
        worktree_id: WorktreeId::parse(worktree_id)
            .map_err(|error| RecoveryError::Corruption(error.to_string()))?,
        operation_id: operation_id
            .map(OperationId::parse)
            .transpose()
            .map_err(|error| RecoveryError::Corruption(error.to_string()))?,
        recovery_type: RecoveryType::parse(&recovery_type)?,
        state: RecoveryState::parse(&state)?,
        payload: serde_json::from_str(&payload_json)
            .map_err(|error| RecoveryError::Corruption(error.to_string()))?,
        evidence: serde_json::from_str(&evidence_json)
            .map_err(|error| RecoveryError::Corruption(error.to_string()))?,
        paths,
        attempt_count,
        owner_token,
        last_error,
        created_at_ms,
        updated_at_ms,
        completed_at_ms,
    })
}

pub(super) fn resource_keys(record: &RecoveryRecord) -> Vec<String> {
    let mut resources: BTreeSet<String> = record
        .paths
        .iter()
        .map(|path| format!("repo-path:{}", path.path))
        .collect();
    if record.recovery_type == RecoveryType::CasStagingCleanup {
        resources.insert("cas:staging".into());
    }
    resources.into_iter().collect()
}

pub(super) fn load_owner_leases(
    transaction: &Transaction<'_>,
    record: &RecoveryRecord,
    owner: &str,
) -> RecoveryResult<Vec<ResourceLease>> {
    let mut leases = Vec::new();
    for resource in resource_keys(record) {
        let (generation, expires_at_ms): (u64, i64) = transaction.query_row(
            "SELECT generation, expires_at_ms FROM materialization_leases
             WHERE worktree_id = ?1 AND resource_key = ?2
               AND recovery_operation_id = ?3 AND owner_token = ?4
               AND released_at_ms IS NULL",
            params![
                record.worktree_id.as_str(),
                resource,
                record.id.as_str(),
                owner
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        leases.push(ResourceLease {
            resource_key: resource,
            generation,
            expires_at_ms,
        });
    }
    Ok(leases)
}

pub(super) fn leases_are_active(
    transaction: &Transaction<'_>,
    record: &RecoveryRecord,
    owner: &str,
    now_ms: i64,
) -> RecoveryResult<bool> {
    for resource in resource_keys(record) {
        let expiry: Option<i64> = transaction
            .query_row(
                "SELECT expires_at_ms FROM materialization_leases
                 WHERE worktree_id = ?1 AND resource_key = ?2
                   AND recovery_operation_id = ?3 AND owner_token = ?4
                   AND released_at_ms IS NULL",
                params![
                    record.worktree_id.as_str(),
                    resource,
                    record.id.as_str(),
                    owner
                ],
                |row| row.get(0),
            )
            .optional()?;
        if expiry.is_none_or(|expiry| expiry <= now_ms) {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn release_leases(
    transaction: &Transaction<'_>,
    record: &RecoveryRecord,
    owner: &str,
    expected: &[ResourceLease],
    now_ms: i64,
) -> RecoveryResult<()> {
    let expected_map: BTreeMap<_, _> = expected
        .iter()
        .map(|lease| (lease.resource_key.as_str(), lease.generation))
        .collect();
    let resources = resource_keys(record);
    if expected_map.len() != expected.len()
        || expected_map.len() != resources.len()
        || resources
            .iter()
            .any(|resource| !expected_map.contains_key(resource.as_str()))
    {
        return Err(RecoveryError::OwnerMismatch(record.id.clone()));
    }
    for resource in resources {
        let generation = expected_map[resource.as_str()];
        let changed = transaction.execute(
            "UPDATE materialization_leases SET released_at_ms = ?1
             WHERE worktree_id = ?2 AND resource_key = ?3
               AND recovery_operation_id = ?4 AND owner_token = ?5
               AND generation = ?6 AND released_at_ms IS NULL AND expires_at_ms > ?1",
            params![
                now_ms,
                record.worktree_id.as_str(),
                resource,
                record.id.as_str(),
                owner,
                generation,
            ],
        )?;
        if changed != 1 {
            return Err(RecoveryError::LeaseFenced {
                worktree_id: record.worktree_id.clone(),
                resource_key: resource,
            });
        }
    }
    Ok(())
}

pub(super) fn bounded_finding_message(mut message: String) -> String {
    if message.len() <= MAX_FINDING_MESSAGE_BYTES {
        return message;
    }
    let mut boundary = MAX_FINDING_MESSAGE_BYTES;
    while !message.is_char_boundary(boundary) {
        boundary -= 1;
    }
    message.truncate(boundary);
    message
}

pub(super) fn require_exact_path_results(
    paths: &[RecoveryPathRecord],
    results: &[RecoveryPathResult],
    id: &RecoveryId,
) -> RecoveryResult<()> {
    let expected: BTreeSet<_> = paths
        .iter()
        .map(|path| PathKey {
            path: path.path.clone(),
            role: path.role,
        })
        .collect();
    let actual: BTreeSet<_> = results
        .iter()
        .map(|result| PathKey {
            path: result.path.clone(),
            role: result.role,
        })
        .collect();
    if expected.len() == paths.len()
        && actual.len() == results.len()
        && expected == actual
        && results.iter().all(|result| result.state.is_terminal())
    {
        Ok(())
    } else {
        Err(RecoveryError::MissingEvidence(id.clone()))
    }
}

pub(super) fn persist_path_results(
    transaction: &Transaction<'_>,
    id: &RecoveryId,
    results: &[RecoveryPathResult],
) -> RecoveryResult<()> {
    for result in results {
        let changed = transaction.execute(
            "UPDATE recovery_paths SET evidence_json = ?1, state = ?2
             WHERE recovery_operation_id = ?3 AND path = ?4 AND path_role = ?5
               AND state = 'verified'",
            params![
                result.evidence_json,
                result.state.as_str(),
                id.as_str(),
                result.path,
                result.role.as_str(),
            ],
        )?;
        if changed != 1 {
            return Err(RecoveryError::Corruption(format!(
                "terminal path result did not update exactly one row for {} ({})",
                result.path,
                result.role.as_str()
            )));
        }
    }
    Ok(())
}

pub(super) fn path_results_match(
    paths: &[RecoveryPathRecord],
    results: &[RecoveryPathResult],
) -> bool {
    let expected: BTreeMap<_, _> = paths
        .iter()
        .map(|path| {
            (
                PathKey {
                    path: path.path.clone(),
                    role: path.role,
                },
                (path.state, canonical_text(&path.evidence).ok()),
            )
        })
        .collect();
    let actual: BTreeMap<_, _> = results
        .iter()
        .map(|result| {
            (
                PathKey {
                    path: result.path.clone(),
                    role: result.role,
                },
                (result.state, Some(result.evidence_json.clone())),
            )
        })
        .collect();
    expected.len() == paths.len() && actual.len() == results.len() && expected == actual
}

pub(super) fn require_exact_path_evidence(
    paths: &[RecoveryPathRecord],
    evidence: &[PathEvidence],
    id: &RecoveryId,
) -> RecoveryResult<()> {
    let expected: BTreeSet<_> = paths
        .iter()
        .map(|path| PathKey {
            path: path.path.clone(),
            role: path.role,
        })
        .collect();
    let actual: BTreeSet<_> = evidence
        .iter()
        .map(|path| PathKey {
            path: path.path.clone(),
            role: path.role,
        })
        .collect();
    if expected == actual {
        Ok(())
    } else {
        Err(RecoveryError::MissingEvidence(id.clone()))
    }
}

pub(super) fn path_evidence_matches(
    paths: &[RecoveryPathRecord],
    evidence: &[PathEvidence],
) -> bool {
    let expected: BTreeMap<_, _> = paths
        .iter()
        .map(|path| {
            (
                PathKey {
                    path: path.path.clone(),
                    role: path.role,
                },
                canonical_text(&path.evidence).ok(),
            )
        })
        .collect();
    let actual: BTreeMap<_, _> = evidence
        .iter()
        .map(|path| {
            (
                PathKey {
                    path: path.path.clone(),
                    role: path.role,
                },
                Some(path.canonical_json.clone()),
            )
        })
        .collect();
    expected == actual
}

pub(super) fn required_evidence(value: Value) -> RecoveryResult<String> {
    if value.is_null() || value.as_object().is_some_and(serde_json::Map::is_empty) {
        return Err(RecoveryError::InvalidInput {
            field: "evidence",
            reason: "must contain explicit durable evidence".into(),
        });
    }
    const MAX_EVIDENCE_BYTES: usize = 1024 * 1024;
    let canonical = canonical_text(&value)?;
    if canonical.len() > MAX_EVIDENCE_BYTES {
        return Err(RecoveryError::InvalidInput {
            field: "evidence",
            reason: format!("must not exceed {MAX_EVIDENCE_BYTES} canonical JSON bytes"),
        });
    }
    Ok(canonical)
}

pub(super) fn canonical_text(value: &impl Serialize) -> RecoveryResult<String> {
    String::from_utf8(canonical_json(value).map_err(json_error)?).map_err(json_error)
}

pub(super) fn canonical_repo_path(path: String) -> RecoveryResult<String> {
    if path.is_empty()
        || path.contains('\0')
        || path.contains('\\')
        || path.starts_with('/')
        || path.ends_with('/')
        || path
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(RecoveryError::InvalidInput {
            field: "path",
            reason: "must be a canonical slash-separated repository-relative path".into(),
        });
    }
    Ok(path)
}

pub(super) fn validated_token(field: &'static str, value: String) -> RecoveryResult<String> {
    if value.is_empty() || value.len() > 255 || value.contains('\0') {
        return Err(RecoveryError::InvalidInput {
            field,
            reason: "must be non-empty, NUL-free, and at most 255 bytes".into(),
        });
    }
    Ok(value)
}

pub(super) fn valid_time(field: &'static str, value: i64) -> RecoveryResult<()> {
    if value < 0 {
        Err(RecoveryError::InvalidInput {
            field,
            reason: "must be non-negative".into(),
        })
    } else {
        Ok(())
    }
}

pub(super) fn json_error(error: impl fmt::Display) -> RecoveryError {
    RecoveryError::InvalidInput {
        field: "json",
        reason: error.to_string(),
    }
}

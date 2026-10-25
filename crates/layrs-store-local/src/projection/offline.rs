//! Exact local versions and explicit preparation; no transport or inferred grants.
use super::*;
use layrs_core::{
    projection::{
        AvailabilityBlocker as Block, AvailabilityRequest, EntryState, OfflineOperation as Op,
        ProjectionAvailability, ProjectionSelection, ProjectionVersion, VersionPathAvailability,
    },
    scm::{FileId, RepoPath, TreeEntryTarget, canonical_json},
};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::BTreeMap,
    io::{Seek, SeekFrom, Write},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfflinePreparation {
    pub id: String,
    pub request: AvailabilityRequest,
    pub status: String,
}
const MAX_ACTIVE_PREPARATIONS: u64 = 1024;
const RETAINED_RELEASED_PREPARATIONS: u64 = 64;

fn preparation_record(c: &Connection, id: &str) -> Result<Option<(OfflinePreparation, String)>> {
    if id.is_empty() || id.len() > 256 {
        return Err(invalid("invalid preparation identifier"));
    }
    let row: Option<(Vec<u8>, String, String)> = c.query_row(
        "SELECT payload,status,reservation FROM projection_preparations WHERE id=?1 AND length(payload)<=16777216",
        [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).optional()?;
    row.map(|(payload, status, reservation)| {
        let p: OfflinePreparation = serde_json::from_slice(&payload).map_err(invalid)?;
        if p.id != id || p.status != status || canonical_json(&p).map_err(invalid)? != payload {
            return Err(invalid("preparation identity mismatch"));
        }
        p.request.validate()?;
        Ok((p, reservation))
    })
    .transpose()
}

fn prune_released(c: &Connection, store: &ProjectionStore, lease: &shared::Lease) -> Result<()> {
    // Bounded maintenance also drains histories written by the earlier draft.
    // Cleanup precedes deletion: a crash keeps the exact reservation owner
    // available for retry, rather than turning it into an untracked attempt.
    let mut q = c.prepare("SELECT id FROM projection_preparations WHERE status='released' ORDER BY rowid DESC LIMIT ?1 OFFSET ?1")?;
    let ids = q
        .query_map([RETAINED_RELEASED_PREPARATIONS], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(q);
    for id in ids {
        let (p, reservation) = preparation_record(c, &id)?
            .ok_or_else(|| invalid("released preparation disappeared"))?;
        if p.status != "released" || c.query_row(
            "SELECT EXISTS(SELECT 1 FROM projection_claims WHERE kind='preparation' AND owner=?1)",
            [&id], |r| r.get::<_, bool>(0),
        )? {
            return Err(invalid("released preparation still has accepted claims"));
        }
        lease.cancel_installations(&reservation, &store.identity().worktree_id)?;
        lease.release_reservation(&reservation, &store.identity().worktree_id)?;
        c.execute(
            "DELETE FROM projection_preparations WHERE id=?1 AND status='released'",
            [&id],
        )?;
    }
    Ok(())
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreparationPoint {
    IntentDurable,
    ContentVerified,
    ProtectionDurable,
    ReadyDurable,
}
pub trait PreparationFaults {
    fn check(&self, point: PreparationPoint) -> io::Result<()>;
}
pub struct NoPreparationFaults;
impl PreparationFaults for NoPreparationFaults {
    fn check(&self, _: PreparationPoint) -> io::Result<()> {
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VersionDifference {
    pub path: RepoPath,
    pub before: Option<FileId>,
    pub after: Option<FileId>,
    pub before_executable: bool,
    pub after_executable: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Observation {
    snapshot: ProjectionSnapshot,
    selection: ProjectionSelection,
}
pub(super) struct VersionState {
    pub snapshot: ProjectionSnapshot,
    pub selection: ProjectionSelection,
}

struct ChargedSpool<'a> {
    spool: &'a mut std::fs::File,
    lease: &'a shared::Lease,
    reservation: &'a str,
    owner: &'a WorktreeId,
    file_id: &'a FileId,
    bytes: u64,
}
impl Write for ChargedSpool<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let end = self
            .bytes
            .checked_add(bytes.len() as u64)
            .filter(|v| *v <= MAX_CAPTURE_FILE_BYTES)
            .ok_or_else(|| io::Error::other("preparation file exceeds 1GiB"))?;
        let file = CapturedFile {
            file_id: self.file_id.clone(),
            bytes: end,
            executable: false,
        };
        // Both the anonymous spool and its eventual shared staging copy coexist.
        // Persist their charge before either receives the corresponding bytes.
        let need = retention::budget_for(&[file], end).map_err(io::Error::other)?;
        self.lease
            .ensure_reservation_capacity(self.reservation, self.owner, need)
            .map_err(io::Error::other)?;
        if fs2::available_space(std::env::temp_dir())?
            < self.lease.usage().map_err(io::Error::other)?.reserved_bytes
        {
            return Err(io::Error::other(
                "partial_store_quota: insufficient temporary spool volume capacity",
            ));
        }
        let written = self.spool.write(bytes)?;
        self.bytes += written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.spool.flush()
    }
}

pub(super) fn snapshot_at(
    c: &Connection,
    generation: u64,
    expected: Option<&str>,
) -> Result<VersionedProjection> {
    let (digest,parent,payload):(String,Option<String>,Vec<u8>)=c.query_row("SELECT digest,parent_digest,payload FROM projection_snapshots WHERE generation=?1 AND length(payload)<=67108864",[generation],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    if expected.is_some_and(|e| e != digest)
        || database::digest(generation, parent.as_deref(), &payload) != digest
    {
        return Err(invalid("version generation digest mismatch"));
    }
    let snapshot: ProjectionSnapshot = serde_json::from_slice(&payload).map_err(invalid)?;
    if snapshot.canonical_bytes()? != payload {
        return Err(invalid("version snapshot is not canonical"));
    }
    Ok(VersionedProjection {
        generation,
        digest,
        snapshot,
    })
}
pub(super) fn resolve(
    c: &Connection,
    store: &ProjectionStore,
    version: &ProjectionVersion,
) -> Result<VersionState> {
    match version {
        ProjectionVersion::Generation {
            worktree_id,
            generation,
            digest,
        } => {
            if worktree_id != &store.identity().worktree_id {
                return Err(invalid("version belongs to another Worktree"));
            }
            let snapshot = snapshot_at(c, *generation, Some(digest))?.snapshot;
            let selection = selection::at(c, *generation)?;
            selection.validate_snapshot(&snapshot)?;
            Ok(VersionState {
                snapshot,
                selection,
            })
        }
        ProjectionVersion::Capture {
            worktree_id,
            sequence,
            digest,
        } => {
            if worktree_id != &store.identity().worktree_id {
                return Err(invalid("capture version belongs to another Worktree"));
            }
            // Accepted replacements are durable local history. Missing or damaged
            // replacement bytes are corruption, never a remote acquisition miss.
            let patch = capture::storage::load_metadata(c, store, *sequence)?;
            let lease = shared::for_store(store)?
                .ok_or_else(|| invalid("capture version requires shared family"))?;
            let roots = retention::root_metadata(c, store, &lease)?;
            for file in patch.changes.iter().filter_map(|v| v.replacement.as_ref()) {
                let manifest = retention::lookup_with_roots(c, &lease, &file.file_id, &roots)?
                    .ok_or_else(|| {
                        invalid(
                            "captured replacement missing; corruption is not an acquisition miss",
                        )
                    })?;
                if manifest.byte_length() != file.bytes {
                    return Err(invalid("capture replacement length mismatch"));
                }
            }
            if patch.digest != *digest {
                return Err(invalid("capture version digest mismatch"));
            }
            let mut state = snapshot_at(
                c,
                patch.projection_generation,
                Some(&patch.projection_digest),
            )?
            .snapshot;
            for change in patch.changes {
                let e = state
                    .entries
                    .iter_mut()
                    .find(|e| e.path == change.path)
                    .ok_or_else(|| invalid("capture path missing from its base"))?;
                if let Some(file) = change.replacement {
                    e.target = Some(TreeEntryTarget::file(file.file_id));
                    e.executable = file.executable;
                    e.state = EntryState::Materialized;
                } else {
                    e.target = None;
                    e.executable = false;
                    e.state = EntryState::Absent;
                }
            }
            state.validate()?;
            Ok(VersionState {
                snapshot: state,
                selection: selection::at(c, patch.projection_generation)?,
            })
        }
        ProjectionVersion::Remote {
            step_id,
            root_tree_id,
            policy_set_id,
            authorization_epoch,
            observation_digest,
        } => {
            let payload:Vec<u8>=c.query_row("SELECT payload FROM projection_observations WHERE digest=?1 AND length(payload)<=67108864",[observation_digest],|r|r.get(0))?;
            let value: Observation = serde_json::from_slice(&payload).map_err(invalid)?;
            if shared::digest(&value)? != *observation_digest
                || canonical_json(&value).map_err(invalid)? != payload
                || value.snapshot.base_step_id != *step_id
                || value.snapshot.base_root_tree_id != *root_tree_id
                || value.snapshot.observed_policy_set_id != *policy_set_id
                || value.snapshot.observed_authorization_epoch != *authorization_epoch
            {
                return Err(invalid("remote version observation mismatch"));
            }
            value.selection.validate_snapshot(&value.snapshot)?;
            Ok(VersionState {
                snapshot: value.snapshot,
                selection: value.selection,
            })
        }
    }
}
fn require_shared(store: &ProjectionStore) -> Result<()> {
    if store.marker.shared.is_none() {
        Err(invalid(
            "offline version API requires explicit format4 Store",
        ))
    } else {
        Ok(())
    }
}
fn require_generation(c: &Connection, expected: Option<u64>) -> Result<()> {
    if let Some(expected) = expected {
        let actual = database::current(c)?.generation;
        if actual != expected {
            return Err(ProjectionStoreError::Stale { expected, actual });
        }
    }
    Ok(())
}
fn entry<'a>(
    state: &'a VersionState,
    path: &RepoPath,
) -> Option<&'a layrs_core::projection::ProjectionEntry> {
    state
        .snapshot
        .entries
        .binary_search_by(|e| e.path.cmp(path))
        .ok()
        .map(|n| &state.snapshot.entries[n])
}
fn pending(c: &Connection) -> Result<bool> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM projection_install) OR EXISTS(SELECT 1 FROM projection_refresh_pending) OR EXISTS(SELECT 1 FROM projection_restore_pending) OR EXISTS(SELECT 1 FROM projection_rebase_pending) OR EXISTS(SELECT 1 FROM projection_namespace_pending) OR EXISTS(SELECT 1 FROM projection_publication_pending)",[],|r|r.get(0))?)
}
fn needs_content(operation: Op, same_content: bool) -> bool {
    matches!(operation, Op::Read | Op::Edit | Op::Restore)
        || (operation == Op::CompareContent && !same_content)
}

/// This stage prepares immutable content dependencies. Mutating whole-checkout
/// plans use the existing capture/restore preflight, not a path-only ready receipt.
fn preparation_dependencies(
    c: &Connection,
    store: &ProjectionStore,
    lease: &shared::Lease,
    request: &AvailabilityRequest,
) -> Result<Vec<(ProjectionVersion, RepoPath, FileId, bool)>> {
    if matches!(request.operation, Op::Capture | Op::Restore) {
        return Err(invalid(
            "offline_preparation_unsupported: capture and restore require their whole-checkout preflight; prepare immutable restore target bytes with Read",
        ));
    }
    let coverage = availability(c, store, lease, request)?;
    if !coverage.blockers.is_empty()
        || coverage
            .paths
            .iter()
            .any(|row| row.blockers.iter().any(|b| *b != Block::MissingContent))
    {
        return Err(invalid(format!(
            "offline_preparation_unavailable: {}",
            serde_json::to_string(&coverage).map_err(invalid)?
        )));
    }
    let first = resolve(c, store, &request.version)?;
    let second = request
        .other
        .as_ref()
        .map(|v| resolve(c, store, v))
        .transpose()?;
    let mut dependencies = Vec::new();
    for path in &request.paths {
        let a = entry(&first, path);
        let b = second.as_ref().and_then(|state| entry(state, path));
        if !needs_content(
            request.operation,
            matches!((a, b), (Some(a), Some(b)) if a.target == b.target),
        ) {
            continue;
        }
        for (version, state) in std::iter::once((&request.version, &first))
            .chain(request.other.as_ref().zip(second.as_ref()))
        {
            let value = entry(state, path).ok_or_else(|| invalid("preparation path unobserved"))?;
            if let Some(TreeEntryTarget::File { file_id }) = &value.target {
                dependencies.push((
                    version.clone(),
                    path.clone(),
                    file_id.clone(),
                    value.executable,
                ));
            }
        }
    }
    Ok(dependencies)
}

pub(super) fn availability(
    c: &Connection,
    store: &ProjectionStore,
    lease: &shared::Lease,
    request: &AvailabilityRequest,
) -> Result<ProjectionAvailability> {
    request.validate()?;
    let first = resolve(c, store, &request.version)?;
    let second = request
        .other
        .as_ref()
        .map(|v| resolve(c, store, v))
        .transpose()?;
    let current = database::current(c)?;
    let roots = retention::root_metadata(c, store, lease)?;
    let effective_paths = if request.operation == Op::Capture {
        selection::current(c)?.validate_snapshot(&current.snapshot)?;
        current
            .snapshot
            .entries
            .iter()
            .filter(|e| {
                first.selection.includes(&e.path)
                    && matches!(e.state, EntryState::Materialized | EntryState::Absent)
            })
            .map(|e| e.path.clone())
            .collect()
    } else {
        request.paths.clone()
    };
    let mut result = ProjectionAvailability {
        request: request.clone(),
        effective_paths,
        paths: Vec::new(),
        blockers: Vec::new(),
        available: false,
        prepared: false,
    };
    if matches!(request.operation, Op::Edit | Op::Capture | Op::Restore) && pending(c)? {
        result.blockers.push(Block::PendingOperation);
    }
    if matches!(request.operation, Op::Edit | Op::Capture)
        && request.version
            != (ProjectionVersion::Generation {
                worktree_id: store.identity().worktree_id.clone(),
                generation: current.generation,
                digest: current.digest.clone(),
            })
    {
        result.blockers.push(Block::UnsupportedTarget);
    }
    for path in &result.effective_paths {
        let a = entry(&first, path);
        let b = second.as_ref().and_then(|s| entry(s, path));
        let same_content = matches!((a,b),(Some(a),Some(b)) if a.target==b.target);
        for (operand, state) in std::iter::once((0, &first)).chain(second.as_ref().map(|s| (1, s)))
        {
            let e = entry(state, path);
            let mut row = VersionPathAvailability {
                path: path.clone(),
                operand,
                absent: false,
                file_id: None,
                manifest_id: None,
                selected: state.selection.includes(path),
                observed_writable: e.is_some_and(|e| e.writable),
                materialized: e.is_some_and(|e| e.state == EntryState::Materialized),
                retained: false,
                blockers: Vec::new(),
            };
            match e {
                None => row.blockers.push(Block::UnknownPath),
                Some(e) => {
                    if e.state == EntryState::Reserved {
                        row.blockers.push(Block::ProtectedPath);
                    }
                    match &e.target {
                        None if e.state == EntryState::Absent => {
                            row.absent = true;
                            row.retained = true;
                        }
                        Some(TreeEntryTarget::File { file_id }) => {
                            row.file_id = Some(file_id.clone());
                            if let Some(m) =
                                retention::lookup_with_roots(c, lease, file_id, &roots)?
                            {
                                row.retained = roots.contains_key(m.id());
                                row.manifest_id = Some(m.id().clone());
                            }
                            let needs_bytes = needs_content(request.operation, same_content);
                            if needs_bytes && row.manifest_id.is_none() {
                                row.blockers.push(Block::MissingContent);
                            }
                        }
                        _ => row.blockers.push(Block::UnsupportedTarget),
                    }
                    // Capture checks write eligibility only for reconciled changes
                    // in scan below; untouched read-only leaves remain valid refs.
                    if matches!(request.operation, Op::Edit | Op::Restore) {
                        let live = current.snapshot.entries.iter().find(|v| &v.path == path);
                        if !live.is_some_and(|v| {
                            v.writable
                                && !matches!(
                                    v.state,
                                    EntryState::Reserved | EntryState::Unavailable
                                )
                        }) {
                            row.blockers.push(Block::ProtectedPath);
                        }
                        if !row.selected {
                            row.blockers.push(Block::ExcludedPath);
                        }
                        if request.operation == Op::Edit && !row.materialized {
                            row.blockers.push(Block::NotMaterialized);
                        }
                    }
                }
            }
            result.paths.push(row);
        }
    }
    if matches!(request.operation, Op::Capture | Op::Restore)
        && !pending(c)?
        && result.paths.iter().all(|row| row.blockers.is_empty())
    {
        match capture::scan::scan(store, c, &current.snapshot, false) {
            Ok(scan) => {
                let mut changes = BTreeMap::new();
                if request.operation == Op::Restore {
                    for row in &result.paths {
                        let file = match &row.manifest_id {
                            Some(id) => {
                                let manifest = lease.manifest(id)?;
                                Some(CapturedFile {
                                    file_id: manifest.file_id().clone(),
                                    bytes: manifest.byte_length(),
                                    executable: entry(&first, &row.path)
                                        .is_some_and(|e| e.executable),
                                })
                            }
                            None => None,
                        };
                        changes.insert(row.path.to_string(), file);
                    }
                }
                let (files, staging) =
                    retention::checkout_requirements(&scan.files, &current.snapshot, &changes)?;
                if !retention::capacity_available(store, lease, &files, staging)? {
                    result.blockers.push(Block::Quota);
                }
                if c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM projection_budgets WHERE state='active')",
                    [],
                    |r| r.get::<_, bool>(0),
                )? {
                    result.blockers.push(Block::PendingOperation);
                }
                if capture::scan::scan(store, c, &current.snapshot, false)?.files != scan.files {
                    result.blockers.push(Block::ChangedCheckout);
                }
            }
            Err(_) => result.blockers.push(Block::ChangedCheckout),
        }
    }
    result.available =
        result.blockers.is_empty() && result.paths.iter().all(|p| p.blockers.is_empty());
    let payload = canonical_json(request).map_err(invalid)?;
    let mut q=c.prepare("SELECT payload FROM projection_preparations WHERE status='ready' AND length(payload)<=16777216")?;
    for row in q.query_map([], |r| r.get::<_, Vec<u8>>(0))? {
        let p: OfflinePreparation = serde_json::from_slice(&row?).map_err(invalid)?;
        if canonical_json(&p.request).map_err(invalid)? == payload {
            result.prepared = result.available
                && result.paths.iter().all(|p| {
                    let same = matches!((entry(&first, &p.path), second.as_ref().and_then(|s| entry(s, &p.path))), (Some(a), Some(b)) if a.target == b.target);
                    p.file_id.is_none() || p.retained || !needs_content(request.operation, same)
                        || request.operation == Op::CompareContent && p.manifest_id.is_none()
                });
            break;
        }
    }
    Ok(result)
}

impl ProjectionStore {
    pub fn generation_version(&self, generation: u64) -> Result<ProjectionVersion> {
        let access = format::access(self, SessionMode::Shared)?;
        let state = snapshot_at(&access.connection, generation, None)?;
        access.verify()?;
        Ok(ProjectionVersion::Generation {
            worktree_id: self.identity().worktree_id.clone(),
            generation,
            digest: state.digest,
        })
    }
    pub fn current_version(&self) -> Result<ProjectionVersion> {
        let state = self.state()?;
        Ok(ProjectionVersion::Generation {
            worktree_id: self.identity().worktree_id.clone(),
            generation: state.generation,
            digest: state.digest,
        })
    }
    pub fn availability(&self, request: &AvailabilityRequest) -> Result<ProjectionAvailability> {
        require_shared(self)?;
        let access = format::access(self, SessionMode::Shared)?;
        let lease = access
            .family
            .as_ref()
            .ok_or_else(|| invalid("missing shared reader lease"))?;
        let result = availability(&access.connection, self, lease, request)?;
        access.verify()?;
        Ok(result)
    }
    /// Resolve immutable metadata only from the named version. Capture changes
    /// overlay their own recorded base; this does not supply acquisition authority.
    pub fn version_metadata(
        &self,
        version: &ProjectionVersion,
    ) -> Result<(ProjectionSnapshot, ProjectionSelection)> {
        require_shared(self)?;
        let access = format::access(self, SessionMode::Shared)?;
        let state = resolve(&access.connection, self, version)?;
        access.verify()?;
        Ok((state.snapshot, state.selection))
    }
    /// Caller must supply a complete correlated server observation. This API
    /// records evidence only; it cannot authorize acquisition or publication.
    pub fn record_version_observation(
        &self,
        snapshot: &ProjectionSnapshot,
        selection: &ProjectionSelection,
    ) -> Result<ProjectionVersion> {
        self.record_version_observation_inner(None, snapshot, selection)
    }
    /// Accept server evidence only while the caller's private generation remains
    /// current. The caller must separately validate fresh acquisition authority.
    pub fn record_version_observation_at_generation(
        &self,
        expected_generation: u64,
        snapshot: &ProjectionSnapshot,
        selection: &ProjectionSelection,
    ) -> Result<ProjectionVersion> {
        self.record_version_observation_inner(Some(expected_generation), snapshot, selection)
    }
    fn record_version_observation_inner(
        &self,
        expected_generation: Option<u64>,
        snapshot: &ProjectionSnapshot,
        selection: &ProjectionSelection,
    ) -> Result<ProjectionVersion> {
        require_shared(self)?;
        selection.validate_snapshot(snapshot)?;
        let mut access = format::access(self, SessionMode::Exclusive)?;
        database::require_mutable(&access.connection)?;
        let tx = access
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        require_generation(&tx, expected_generation)?;
        let value = Observation {
            snapshot: snapshot.clone(),
            selection: selection.clone(),
        };
        let digest = shared::digest(&value)?;
        let payload = canonical_json(&value).map_err(invalid)?;
        if payload.len() > 64 * 1024 * 1024 {
            return Err(invalid("offline observation exceeds bound"));
        }
        tx.execute(
            "INSERT INTO projection_observations VALUES(?1,?2) ON CONFLICT(digest) DO NOTHING",
            params![digest, payload],
        )?;
        tx.commit()?;
        access.verify()?;
        Ok(ProjectionVersion::Remote {
            step_id: snapshot.base_step_id.clone(),
            root_tree_id: snapshot.base_root_tree_id.clone(),
            policy_set_id: snapshot.observed_policy_set_id.clone(),
            authorization_epoch: snapshot.observed_authorization_epoch,
            observation_digest: digest,
        })
    }
    pub fn read_version_file(
        &self,
        version: &ProjectionVersion,
        path: &RepoPath,
        out: &mut dyn Write,
    ) -> Result<()> {
        require_shared(self)?;
        let access = format::access(self, SessionMode::Shared)?;
        let lease = access
            .family
            .as_ref()
            .ok_or_else(|| invalid("missing version reader lease"))?;
        let state = resolve(&access.connection, self, version)?;
        let e = entry(&state, path)
            .ok_or_else(|| invalid(format!("offline_missing_metadata: {path}")))?;
        if e.state == EntryState::Reserved {
            return Err(invalid("offline path is protected"));
        }
        let Some(TreeEntryTarget::File { file_id }) = &e.target else {
            return Err(invalid("version path is absent or is not a regular file"));
        };
        let manifest = retention::lookup(&access.connection, self, lease, file_id)?
            .ok_or_else(|| invalid(format!("offline_missing_content: {path} {file_id}")))?;
        lease.stream(&manifest, out)?;
        access.verify()
    }
    pub fn compare_versions(
        &self,
        request: &AvailabilityRequest,
    ) -> Result<Vec<VersionDifference>> {
        if !matches!(request.operation, Op::CompareMetadata | Op::CompareContent) {
            return Err(invalid("compare requires comparison operation"));
        }
        require_shared(self)?;
        let access = format::access(self, SessionMode::Shared)?;
        let lease = access
            .family
            .as_ref()
            .ok_or_else(|| invalid("missing compare lease"))?;
        let coverage = availability(&access.connection, self, lease, request)?;
        if !coverage.available {
            return Err(invalid(format!(
                "offline_compare_unavailable: {}",
                serde_json::to_string(&coverage).map_err(invalid)?
            )));
        }
        let first = resolve(&access.connection, self, &request.version)?;
        let second = resolve(
            &access.connection,
            self,
            request
                .other
                .as_ref()
                .ok_or_else(|| invalid("missing comparison operand"))?,
        )?;
        let mut rows = Vec::new();
        for path in &request.paths {
            let a = entry(&first, path).ok_or_else(|| invalid("unknown comparison path"))?;
            let b = entry(&second, path).ok_or_else(|| invalid("unknown comparison path"))?;
            if a.target != b.target || a.executable != b.executable {
                let id = |e: &layrs_core::projection::ProjectionEntry| match &e.target {
                    Some(TreeEntryTarget::File { file_id }) => Some(file_id.clone()),
                    _ => None,
                };
                rows.push(VersionDifference {
                    path: path.clone(),
                    before: id(a),
                    after: id(b),
                    before_executable: a.executable,
                    after_executable: b.executable,
                });
            }
        }
        access.verify()?;
        Ok(rows)
    }
    pub fn begin_preparation(&self, request: &AvailabilityRequest) -> Result<OfflinePreparation> {
        self.begin_preparation_inner(None, request)
    }
    pub fn begin_preparation_at_generation(
        &self,
        expected_generation: u64,
        request: &AvailabilityRequest,
    ) -> Result<OfflinePreparation> {
        self.begin_preparation_inner(Some(expected_generation), request)
    }
    fn begin_preparation_inner(
        &self,
        expected_generation: Option<u64>,
        request: &AvailabilityRequest,
    ) -> Result<OfflinePreparation> {
        require_shared(self)?;
        request.validate()?;
        let mut access = format::access(self, SessionMode::Exclusive)?;
        database::require_mutable(&access.connection)?;
        require_generation(&access.connection, expected_generation)?;
        let active: u64 = access.connection.query_row(
            "SELECT COUNT(*) FROM projection_preparations WHERE status!='released'",
            [],
            |r| r.get(0),
        )?;
        if active >= MAX_ACTIVE_PREPARATIONS {
            return Err(invalid(
                "active preparation limit reached; release an existing preparation",
            ));
        }
        let lease = access
            .family
            .as_ref()
            .ok_or_else(|| invalid("missing preparation lease"))?;
        let dependencies = preparation_dependencies(&access.connection, self, lease, request)?;
        // Retain existing representations now. Missing files acquire their
        // spool plus installation capacity before every write, by actual length.
        let reserve = 65536u64;
        let mut seen = std::collections::BTreeSet::new();
        let mut retained = Vec::new();
        for (_, _, file_id, _) in &dependencies {
            if seen.insert(file_id) {
                if let Some(manifest) = lease.manifest_for(file_id)? {
                    lease.check_manifest_resident(&manifest)?;
                    retained.push(manifest);
                }
            }
        }
        let id = shared::unique_id()?;
        let reservation = lease.reserve(
            &self.identity().worktree_id,
            &format!("preparation:{id}"),
            reserve,
        )?;
        let preparation = OfflinePreparation {
            id: id.clone(),
            request: request.clone(),
            status: "acquiring".into(),
        };
        let payload = canonical_json(&preparation).map_err(invalid)?;
        let tx = access
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        require_generation(&tx, expected_generation)?;
        retention::accept_claim(&tx, self, lease, "preparation", &id, &retained)?;
        tx.execute(
            "INSERT INTO projection_preparations VALUES(?1,?2,'acquiring',NULL,?3)",
            params![id, payload, reservation],
        )?;
        tx.commit()?;
        access.verify()?;
        Ok(preparation)
    }
    pub fn prepare_offline(
        &self,
        request: &AvailabilityRequest,
        write_file: &mut dyn FnMut(&ProjectionVersion, &RepoPath, &mut dyn Write) -> io::Result<()>,
    ) -> Result<OfflinePreparation> {
        let preparation = self.begin_preparation(request)?;
        self.resume_preparation(&preparation.id, write_file)
    }
    /// The writer must only copy already authorized, verified local input. Both
    /// intent creation and ready acceptance reject an intervening generation change.
    pub fn prepare_offline_at_generation(
        &self,
        expected_generation: u64,
        request: &AvailabilityRequest,
        write_file: &mut dyn FnMut(&ProjectionVersion, &RepoPath, &mut dyn Write) -> io::Result<()>,
    ) -> Result<OfflinePreparation> {
        let preparation = self.begin_preparation_at_generation(expected_generation, request)?;
        self.resume_preparation_at_generation(expected_generation, &preparation.id, write_file)
    }
    pub fn resume_preparation(
        &self,
        id: &str,
        write_file: &mut dyn FnMut(&ProjectionVersion, &RepoPath, &mut dyn Write) -> io::Result<()>,
    ) -> Result<OfflinePreparation> {
        self.resume_preparation_with_faults(id, write_file, &NoPreparationFaults)
    }
    /// Uses the stored immutable request, never a caller-supplied replacement.
    /// Fresh remote authorization belongs to the caller on every resume attempt.
    pub fn resume_preparation_at_generation(
        &self,
        expected_generation: u64,
        id: &str,
        write_file: &mut dyn FnMut(&ProjectionVersion, &RepoPath, &mut dyn Write) -> io::Result<()>,
    ) -> Result<OfflinePreparation> {
        self.resume_preparation_inner(
            Some(expected_generation),
            id,
            write_file,
            &NoPreparationFaults,
        )
    }
    pub fn resume_preparation_with_faults(
        &self,
        id: &str,
        write_file: &mut dyn FnMut(&ProjectionVersion, &RepoPath, &mut dyn Write) -> io::Result<()>,
        faults: &dyn PreparationFaults,
    ) -> Result<OfflinePreparation> {
        self.resume_preparation_inner(None, id, write_file, faults)
    }
    fn resume_preparation_inner(
        &self,
        expected_generation: Option<u64>,
        id: &str,
        write_file: &mut dyn FnMut(&ProjectionVersion, &RepoPath, &mut dyn Write) -> io::Result<()>,
        faults: &dyn PreparationFaults,
    ) -> Result<OfflinePreparation> {
        require_shared(self)?;
        let mut access = format::access(self, SessionMode::Exclusive)?;
        database::require_mutable(&access.connection)?;
        require_generation(&access.connection, expected_generation)?;
        let lease = access
            .family
            .as_ref()
            .ok_or_else(|| invalid("missing preparation lease"))?;
        let (payload,status,reservation):(Vec<u8>,String,String)=access.connection.query_row("SELECT payload,status,reservation FROM projection_preparations WHERE id=?1 AND length(payload)<=16777216",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        let mut p: OfflinePreparation = serde_json::from_slice(&payload).map_err(invalid)?;
        if p.id != id || p.status != status || canonical_json(&p).map_err(invalid)? != payload {
            return Err(invalid("preparation identity mismatch"));
        }
        if status == "released" {
            return Err(invalid("preparation released"));
        }
        let dependencies = preparation_dependencies(&access.connection, self, lease, &p.request)?;
        if status == "ready" {
            lease.release_reservation(&reservation, &self.identity().worktree_id)?;
            access.verify()?;
            return Ok(p);
        }
        faults.check(PreparationPoint::IntentDurable)?;
        // An interrupted previous attempt may own a spool-sized installation.
        // Resolve only its authenticated stages before charging a new spool.
        lease.cancel_installations(&reservation, &self.identity().worktree_id)?;
        let mut manifests = Vec::new();
        for (version, path, file_id, executable) in dependencies {
            let manifest = if let Some(manifest) = lease.manifest_for(&file_id)? {
                lease.verify_manifest(&manifest)?;
                manifest
            } else {
                let mut spool = crate::private_spool_file()?;
                let mut charged = ChargedSpool {
                    spool: &mut spool,
                    lease,
                    reservation: &reservation,
                    owner: &self.identity().worktree_id,
                    file_id: &file_id,
                    bytes: 0,
                };
                let mut writer =
                    install::files::FileWriter::new(&mut charged, MAX_CAPTURE_FILE_BYTES);
                write_file(&version, &path, &mut writer)?;
                let bytes = writer.finish(&file_id)?;
                drop(charged);
                spool.seek(SeekFrom::Start(0))?;
                lease.install_file(
                    &mut spool,
                    &CapturedFile {
                        file_id: file_id.clone(),
                        executable,
                        bytes,
                    },
                    &reservation,
                )?
            };
            // Preserve successful acquisition progress across a later file's
            // failure. Release removes these exact private preparation claims.
            retention::accept_claim(
                &access.connection,
                self,
                lease,
                "preparation",
                &p.id,
                std::slice::from_ref(&manifest),
            )?;
            manifests.push(manifest);
        }
        faults.check(PreparationPoint::ContentVerified)?;
        let (root, digest) =
            lease.protect(&self.marker, &format!("preparation:{}", p.id), &manifests)?;
        faults.check(PreparationPoint::ProtectionDurable)?;
        let tx = access
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        require_generation(&tx, expected_generation)?;
        // Root membership and private ready receipt are accepted atomically.
        tx.execute("INSERT INTO projection_claims VALUES(?1,?2,'preparation',?3) ON CONFLICT(root_id) DO NOTHING",params![root,digest,p.id])?;
        for m in &manifests {
            tx.execute(
                "INSERT INTO projection_content VALUES(?1,?2,?3) ON CONFLICT(file_id) DO NOTHING",
                params![m.file_id().as_str(), m.id().as_str(), m.byte_length()],
            )?;
        }
        p.status = "ready".into();
        tx.execute(
            "UPDATE projection_preparations SET payload=?1,status='ready',root_id=?2 WHERE id=?3",
            params![canonical_json(&p).map_err(invalid)?, root, p.id],
        )?;
        tx.commit()?;
        faults.check(PreparationPoint::ReadyDurable)?;
        lease.release_reservation(&reservation, &self.identity().worktree_id)?;
        access.verify()?;
        Ok(p)
    }
    /// Exact lookup is independent of listing/history bounds. Pruned released
    /// receipts return None; active preparations are never pruned.
    pub fn preparation(&self, id: &str) -> Result<Option<OfflinePreparation>> {
        require_shared(self)?;
        let access = format::access(self, SessionMode::Shared)?;
        let result = preparation_record(&access.connection, id)?.map(|(p, _)| p);
        access.verify()?;
        Ok(result)
    }
    /// At most 1024 active entries plus the last 64 created released entries.
    /// Oversized histories from earlier drafts cannot block exact lookup.
    pub fn preparations(&self) -> Result<Vec<OfflinePreparation>> {
        require_shared(self)?;
        let access = format::access(self, SessionMode::Shared)?;
        let mut out = Vec::new();
        for (released, limit) in [
            (false, MAX_ACTIVE_PREPARATIONS),
            (true, RETAINED_RELEASED_PREPARATIONS),
        ] {
            let mut q = access.connection.prepare("SELECT id FROM projection_preparations WHERE (status='released')=?1 ORDER BY rowid DESC LIMIT ?2")?;
            for id in q.query_map(params![released, limit], |r| r.get::<_, String>(0))? {
                out.push(
                    preparation_record(&access.connection, &id?)?
                        .ok_or_else(|| invalid("listed preparation disappeared"))?
                        .0,
                );
            }
        }
        access.verify()?;
        Ok(out)
    }
    pub fn release_preparation(&self, id: &str) -> Result<()> {
        require_shared(self)?;
        let mut access = format::access(self, SessionMode::Exclusive)?;
        database::require_mutable(&access.connection)?;
        let (mut p, reservation) = preparation_record(&access.connection, id)?
            .ok_or_else(|| invalid("preparation unknown or released history expired"))?;
        let lease = access
            .family
            .as_ref()
            .ok_or_else(|| invalid("missing cancellation lease"))?;
        lease.cancel_installations(&reservation, &self.identity().worktree_id)?;
        p.status = "released".into();
        let tx = access
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM projection_claims WHERE kind='preparation' AND owner=?1",
            [id],
        )?;
        tx.execute(
            "UPDATE projection_preparations SET payload=?1,status='released' WHERE id=?2",
            params![canonical_json(&p).map_err(invalid)?, id],
        )?;
        tx.commit()?;
        access
            .family
            .as_ref()
            .ok_or_else(|| invalid("missing release lease"))?
            .release_reservation(&reservation, &self.identity().worktree_id)?;
        prune_released(&access.connection, self, lease)?;
        // Shared root removal requires exclusive reconciliation of every owner.
        access.verify()
    }
}

pub(super) fn restore_targets(
    c: &Connection,
    store: &ProjectionStore,
    version: &ProjectionVersion,
    paths: &[RepoPath],
) -> Result<BTreeMap<RepoPath, Option<CapturedFile>>> {
    require_shared(store)?;
    if paths.is_empty() || paths.len() > 1024 || paths.windows(2).any(|p| p[0] >= p[1]) {
        return Err(invalid("restore version paths must be sorted and unique"));
    }
    let lease =
        shared::for_store(store)?.ok_or_else(|| invalid("missing restore content lease"))?;
    let state = resolve(c, store, version)?;
    let mut out = BTreeMap::new();
    for path in paths {
        let e = entry(&state, path).ok_or_else(|| invalid("offline_restore_missing_metadata"))?;
        if e.state == EntryState::Reserved {
            return Err(invalid("offline restore source protected"));
        }
        let file = match &e.target {
            None if e.state == EntryState::Absent => None,
            Some(TreeEntryTarget::File { file_id }) => {
                let m = retention::lookup(c, store, &lease, file_id)?.ok_or_else(|| {
                    invalid(format!("offline_restore_missing_content: {path} {file_id}"))
                })?;
                lease.verify_manifest(&m)?;
                Some(CapturedFile {
                    file_id: file_id.clone(),
                    executable: e.executable,
                    bytes: m.byte_length(),
                })
            }
            _ => return Err(invalid("offline restore target unsupported")),
        };
        out.insert(path.clone(), file);
    }
    Ok(out)
}

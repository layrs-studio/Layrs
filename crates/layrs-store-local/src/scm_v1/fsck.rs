//! Bounded, strictly read-only integrity inspection for native SCM stores.

use super::cas::NativeCas;
use super::format::{
    MAX_FORMAT_BYTES, NativeStoreError, NativeStoreInspection, inspect, open_audit,
};
use super::layout::{NativeStoreAudit, NativeStoreLayout};
use super::schema::{
    BASELINE_NAME, REQUIRED_TABLES, SCHEMA_CHECKSUM, SCHEMA_FINGERPRINT, SCHEMA_MAGIC,
    SCHEMA_VERSION,
};
use super::sqlite;
use layrs_core::scm::{
    ChunkId, ConflictId, FileManifestId, Layer, Marker, Operation, Release, ReleasePublication,
    RepositoryViewId, Space, StepId, StepObservation, SymlinkId, TombstoneId, TreeId, Worktree,
    canonical_json,
};
use layrs_store::scm::{CanonicalObject, StoreObjectId, StoreObjectKind};
use rusqlite::{Connection, OptionalExtension};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::layout::open_read_no_follow;

const MAX_ENTITY_PAYLOAD_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FsckSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FsckFindingCode {
    FormatInvalid,
    StoreIdMismatch,
    SchemaMetadataMismatch,
    SchemaFingerprintMismatch,
    SqliteIntegrityFailure,
    ForeignKeyViolation,
    MissingCasObject,
    CasCorruption,
    ObjectKindMismatch,
    ObjectPathMismatch,
    ObjectLengthMismatch,
    ManifestReconstructionFailure,
    ObjectEdgeMismatch,
    TreeClosureBroken,
    StepParentMissing,
    StepCycle,
    OperationParentMissing,
    OperationCycle,
    OperationHistoryInvalid,
    AdmissionClosureBroken,
    ViewHeadsNotAntichain,
    CrossSpaceReference,
    CurrentReferenceInvalid,
    ProjectionDivergence,
    RecoveryOperationMixing,
    GcRootInvalid,
    UncheckpointedWal,
    WorkBudgetExceeded,
}

impl FsckFindingCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FormatInvalid => "format_invalid",
            Self::StoreIdMismatch => "store_id_mismatch",
            Self::SchemaMetadataMismatch => "schema_metadata_mismatch",
            Self::SchemaFingerprintMismatch => "schema_fingerprint_mismatch",
            Self::SqliteIntegrityFailure => "sqlite_integrity_failure",
            Self::ForeignKeyViolation => "foreign_key_violation",
            Self::MissingCasObject => "missing_cas_object",
            Self::CasCorruption => "cas_corruption",
            Self::ObjectKindMismatch => "object_kind_mismatch",
            Self::ObjectPathMismatch => "object_path_mismatch",
            Self::ObjectLengthMismatch => "object_length_mismatch",
            Self::ManifestReconstructionFailure => "manifest_reconstruction_failure",
            Self::ObjectEdgeMismatch => "object_edge_mismatch",
            Self::TreeClosureBroken => "tree_closure_broken",
            Self::StepParentMissing => "step_parent_missing",
            Self::StepCycle => "step_cycle",
            Self::OperationParentMissing => "operation_parent_missing",
            Self::OperationCycle => "operation_cycle",
            Self::OperationHistoryInvalid => "operation_history_invalid",
            Self::AdmissionClosureBroken => "admission_closure_broken",
            Self::ViewHeadsNotAntichain => "view_heads_not_antichain",
            Self::CrossSpaceReference => "cross_space_reference",
            Self::CurrentReferenceInvalid => "current_reference_invalid",
            Self::ProjectionDivergence => "projection_divergence",
            Self::RecoveryOperationMixing => "recovery_operation_mixing",
            Self::GcRootInvalid => "gc_root_invalid",
            Self::UncheckpointedWal => "uncheckpointed_wal",
            Self::WorkBudgetExceeded => "work_budget_exceeded",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsckFinding {
    pub code: FsckFindingCode,
    pub severity: FsckSeverity,
    pub path: Option<PathBuf>,
    pub object_id: Option<String>,
    pub entity_id: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsckOptions {
    pub max_findings: usize,
}

impl Default for FsckOptions {
    fn default() -> Self {
        Self { max_findings: 1024 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionAction {
    pub table: String,
    pub entity_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectionRebuildPlan {
    pub actions: Vec<ProjectionAction>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GcReachability {
    pub roots: u64,
    pub reachable_objects: u64,
    pub unreachable_objects: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FsckReport {
    pub findings: Vec<FsckFinding>,
    pub checked_objects: u64,
    pub checked_rows: u64,
    pub truncated_count: u64,
    pub projection_rebuild_plan: ProjectionRebuildPlan,
    pub gc_reachability: GcReachability,
}

impl FsckReport {
    pub fn is_clean(&self) -> bool {
        self.findings
            .iter()
            .all(|finding| finding.severity != FsckSeverity::Error)
    }

    pub fn is_truncated(&self) -> bool {
        self.truncated_count != 0
    }

    /// Describes projection rows that can be regenerated from already
    /// verified canonical payloads. It never includes synthetic history or a
    /// replacement for missing CAS bytes.
    pub fn projection_rebuild_plan(&self) -> &ProjectionRebuildPlan {
        &self.projection_rebuild_plan
    }

    pub fn gc_reachability(&self) -> &GcReachability {
        &self.gc_reachability
    }
}

#[derive(Debug)]
pub enum FsckError {
    Io(std::io::Error),
    Sqlite(rusqlite::Error),
    Native(String),
}

impl fmt::Display for FsckError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "native fsck I/O failed: {error}"),
            Self::Sqlite(error) => write!(formatter, "native fsck SQLite failed: {error}"),
            Self::Native(message) => write!(formatter, "native fsck failed: {message}"),
        }
    }
}

impl Error for FsckError {}

impl From<std::io::Error> for FsckError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<rusqlite::Error> for FsckError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sqlite(value)
    }
}

pub struct NativeFsck;

impl NativeFsck {
    pub fn run(audit: &NativeStoreAudit, options: FsckOptions) -> Result<FsckReport, FsckError> {
        let connection = match audit.try_connection() {
            Ok(connection) => connection,
            Err(super::ScmSchemaError::UncheckpointedWal(path)) => {
                let mut report = FsckReport::default();
                bounded_finding(
                    &mut report,
                    options.max_findings,
                    FsckFinding {
                        code: FsckFindingCode::UncheckpointedWal,
                        severity: FsckSeverity::Error,
                        path: Some(path),
                        object_id: None,
                        entity_id: None,
                        message: "read-only inspection refused an uncheckpointed WAL".into(),
                    },
                );
                return Ok(report);
            }
            Err(error) => return Err(FsckError::Native(error.to_string())),
        };
        let mut context = Context::new(audit, &connection, options.max_findings);
        context.check_format()?;
        context.check_schema();
        if !context.has_required_tables() {
            return Ok(context.finish());
        }
        context.check_work_budget()?;
        if context.work_truncated {
            return Ok(context.finish());
        }
        context.check_sqlite_integrity();
        context.check_foreign_keys();
        context.scan_objects()?;
        if context.work_truncated {
            return Ok(context.finish());
        }
        context.check_manifest_projections()?;
        context.check_object_edges()?;
        if context.work_truncated {
            return Ok(context.finish());
        }
        context.check_graphs()?;
        if context.work_truncated {
            return Ok(context.finish());
        }
        context.check_admissions()?;
        context.check_views_and_spaces()?;
        if context.work_truncated {
            return Ok(context.finish());
        }
        context.check_current_references()?;
        if context.work_truncated {
            return Ok(context.finish());
        }
        context.check_payload_projections()?;
        if context.work_truncated {
            return Ok(context.finish());
        }
        context.check_recovery_separation()?;
        context.compute_gc_reachability()?;
        Ok(context.finish())
    }
}

pub fn fsck(audit: &NativeStoreAudit, options: FsckOptions) -> Result<FsckReport, FsckError> {
    NativeFsck::run(audit, options)
}

/// Read-only convenience entry point. Unsupported roots produce a bounded
/// format finding instead of being initialized or migrated.
pub fn fsck_existing(
    root: impl AsRef<Path>,
    options: FsckOptions,
) -> Result<FsckReport, FsckError> {
    let root = root.as_ref();
    // `inspect()` parses FORMAT to classify the store. Preflight its size
    // without following links so this public fsck entry point never asks the
    // format classifier to allocate an attacker-controlled marker. The shared
    // format reader independently enforces the same cap against races and all
    // non-fsck callers.
    if let Ok(layout) = NativeStoreLayout::new(root) {
        if let Ok(file) = open_read_no_follow(layout.format_path()) {
            if file.metadata()?.len() > MAX_FORMAT_BYTES {
                let mut report = FsckReport {
                    truncated_count: 1,
                    ..FsckReport::default()
                };
                bounded_finding(
                    &mut report,
                    options.max_findings,
                    FsckFinding {
                        code: FsckFindingCode::WorkBudgetExceeded,
                        severity: FsckSeverity::Error,
                        path: Some(layout.format_path().to_path_buf()),
                        object_id: None,
                        entity_id: None,
                        message: "fsck byte budget exhausted before reading FORMAT".into(),
                    },
                );
                return Ok(report);
            }
        }
    }
    match inspect(root).map_err(|error| FsckError::Native(error.to_string()))? {
        NativeStoreInspection::NativeReady { .. } => match open_audit(root) {
            Ok(audit) => NativeFsck::run(&audit, options),
            Err(NativeStoreError::Schema(super::ScmSchemaError::UncheckpointedWal(path))) => {
                let mut report = FsckReport::default();
                bounded_finding(
                    &mut report,
                    options.max_findings,
                    FsckFinding {
                        code: FsckFindingCode::UncheckpointedWal,
                        severity: FsckSeverity::Error,
                        path: Some(path),
                        object_id: None,
                        entity_id: None,
                        message: "read-only inspection refused an uncheckpointed WAL".into(),
                    },
                );
                Ok(report)
            }
            Err(error) => Err(FsckError::Native(error.to_string())),
        },
        inspection => {
            let mut report = FsckReport::default();
            if options.max_findings == 0 {
                report.truncated_count = 1;
            } else {
                report.findings.push(FsckFinding {
                    code: FsckFindingCode::FormatInvalid,
                    severity: FsckSeverity::Error,
                    path: Some(inspection.layout().root().to_path_buf()),
                    object_id: None,
                    entity_id: None,
                    message: format!("unsupported Store format: {}", inspection.kind()),
                });
            }
            Ok(report)
        }
    }
}

fn bounded_finding(report: &mut FsckReport, max_findings: usize, finding: FsckFinding) {
    if report.findings.len() < max_findings {
        report.findings.push(finding);
    } else {
        report.truncated_count = report.truncated_count.saturating_add(1);
    }
}

struct Context<'a> {
    audit: &'a NativeStoreAudit,
    connection: &'a Connection,
    max_findings: usize,
    report: FsckReport,
    objects: BTreeMap<String, CanonicalObject>,
    object_ids: BTreeMap<String, StoreObjectId>,
    row_limit: usize,
    byte_budget_remaining: u64,
    chunk_byte_budget_remaining: u64,
    file_byte_budget_remaining: u64,
    cpu_budget_remaining: u64,
    work_budget_reported: bool,
    work_truncated: bool,
}

impl<'a> Context<'a> {
    fn new(audit: &'a NativeStoreAudit, connection: &'a Connection, max_findings: usize) -> Self {
        let row_limit = max_findings.max(1).saturating_mul(64).min(1_000_000);
        Self {
            audit,
            connection,
            max_findings,
            report: FsckReport::default(),
            objects: BTreeMap::new(),
            object_ids: BTreeMap::new(),
            row_limit,
            byte_budget_remaining: (row_limit as u64)
                .saturating_mul(64 * 1024)
                .min(256 * 1024 * 1024),
            // Chunk auditing is streaming and does not populate objects RAM.
            // Keep its I/O budget separate from retained canonical metadata.
            chunk_byte_budget_remaining: (row_limit as u64)
                .saturating_mul(64 * 1024)
                .min(4 * 1024 * 1024 * 1024),
            file_byte_budget_remaining: (row_limit as u64)
                .saturating_mul(64 * 1024)
                .min(4 * 1024 * 1024 * 1024),
            cpu_budget_remaining: (max_findings.max(1) as u64)
                .saturating_mul(256)
                .min(16_000_000),
            work_budget_reported: false,
            work_truncated: false,
        }
    }

    fn finish(self) -> FsckReport {
        self.report
    }

    fn finding(
        &mut self,
        code: FsckFindingCode,
        severity: FsckSeverity,
        path: Option<PathBuf>,
        object_id: Option<String>,
        entity_id: Option<String>,
        message: impl Into<String>,
    ) {
        if self.report.findings.len() < self.max_findings {
            self.report.findings.push(FsckFinding {
                code,
                severity,
                path,
                object_id,
                entity_id,
                message: message.into(),
            });
        } else {
            self.report.truncated_count = self.report.truncated_count.saturating_add(1);
        }
    }

    fn error(
        &mut self,
        code: FsckFindingCode,
        entity_id: Option<String>,
        message: impl Into<String>,
    ) {
        self.finding(code, FsckSeverity::Error, None, None, entity_id, message);
    }

    fn consume_cpu(&mut self, amount: usize) -> bool {
        let amount = amount as u64;
        if self.cpu_budget_remaining >= amount {
            self.cpu_budget_remaining -= amount;
            return true;
        }
        self.exhaust_cpu();
        false
    }

    fn exhaust_cpu(&mut self) {
        self.cpu_budget_remaining = 0;
        self.work_truncated = true;
        self.report.truncated_count = self.report.truncated_count.saturating_add(1);
        if !self.work_budget_reported {
            self.work_budget_reported = true;
            // The budget finding explains why the rest of the report is
            // incomplete, so reserve a bounded slot for it even when earlier
            // corruption already filled the caller's finding cap.
            if self.max_findings > 0 && self.report.findings.len() >= self.max_findings {
                self.report.findings.pop();
                self.report.truncated_count = self.report.truncated_count.saturating_add(1);
            }
            self.finding(
                FsckFindingCode::WorkBudgetExceeded,
                FsckSeverity::Error,
                None,
                None,
                None,
                "fsck CPU budget exhausted while traversing graph edges, visits, or pairs",
            );
        }
    }

    fn exhaust_bytes(&mut self, context: &str) {
        self.byte_budget_remaining = 0;
        self.work_truncated = true;
        self.report.truncated_count = self.report.truncated_count.saturating_add(1);
        if !self.work_budget_reported {
            self.work_budget_reported = true;
            if self.max_findings > 0 && self.report.findings.len() >= self.max_findings {
                self.report.findings.pop();
                self.report.truncated_count = self.report.truncated_count.saturating_add(1);
            }
            self.finding(
                FsckFindingCode::WorkBudgetExceeded,
                FsckSeverity::Error,
                None,
                None,
                None,
                format!("fsck byte budget exhausted before reading {context}"),
            );
        }
    }

    fn reserve_bytes(&mut self, length: u64, per_value_limit: u64, context: &str) -> bool {
        if length > per_value_limit || length > self.byte_budget_remaining {
            self.exhaust_bytes(context);
            return false;
        }
        self.byte_budget_remaining -= length;
        true
    }

    fn check_work_budget(&mut self) -> Result<(), FsckError> {
        const WORK_ROWS: &str = "
            SELECT 1 FROM objects UNION ALL SELECT 1 FROM object_edges
            UNION ALL SELECT 1 FROM file_manifests UNION ALL SELECT 1 FROM file_manifest_chunks
            UNION ALL SELECT 1 FROM space_object_admissions UNION ALL SELECT 1 FROM steps
            UNION ALL SELECT 1 FROM step_parents UNION ALL SELECT 1 FROM repository_views
            UNION ALL SELECT 1 FROM repository_view_layer_heads
            UNION ALL SELECT 1 FROM repository_view_markers
            UNION ALL SELECT 1 FROM repository_view_releases
            UNION ALL SELECT 1 FROM repository_view_worktrees
            UNION ALL SELECT 1 FROM layers UNION ALL SELECT 1 FROM layer_parents
            UNION ALL SELECT 1 FROM step_observations UNION ALL SELECT 1 FROM operations
            UNION ALL SELECT 1 FROM operation_parents UNION ALL SELECT 1 FROM operation_rewrites
            UNION ALL SELECT 1 FROM operation_rewrite_targets UNION ALL SELECT 1 FROM worktrees
            UNION ALL SELECT 1 FROM markers UNION ALL SELECT 1 FROM marker_proofs
            UNION ALL SELECT 1 FROM releases UNION ALL SELECT 1 FROM release_proofs
            UNION ALL SELECT 1 FROM release_publications
            UNION ALL SELECT 1 FROM current_repository_views UNION ALL SELECT 1 FROM operation_heads
            UNION ALL SELECT 1 FROM object_pins UNION ALL SELECT 1 FROM gc_roots
            UNION ALL SELECT 1 FROM recovery_operations UNION ALL SELECT 1 FROM recovery_paths
        ";
        let sql = format!("SELECT 1 FROM ({WORK_ROWS}) LIMIT ?1");
        let mut statement = self.connection.prepare(&sql)?;
        let count = statement
            .query_map([self.row_limit.saturating_add(1) as u64], |_| Ok(()))?
            .take(self.row_limit.saturating_add(1))
            .collect::<rusqlite::Result<Vec<_>>>()?
            .len();
        if count > self.row_limit {
            self.exhaust_cpu();
        }
        Ok(())
    }
}

mod entities;
mod graphs;
mod integrity;
mod projections;

fn decode_core_projection(
    table: &str,
    payload: &str,
) -> Result<serde_json::Value, serde_json::Error> {
    macro_rules! decode {
        ($type:ty) => {{
            let value: $type = serde_json::from_str(payload)?;
            serde_json::to_value(value)
        }};
    }
    match table {
        "spaces" => decode!(Space),
        "layers" => decode!(Layer),
        "step_observations" => decode!(StepObservation),
        "operations" => decode!(Operation),
        "worktrees" => decode!(Worktree),
        "markers" => decode!(Marker),
        "releases" => decode!(Release),
        "release_publications" => decode!(ReleasePublication),
        // Store identity is format-owned and has no independent Core entity;
        // its exact marker invariant is checked by check_format.
        _ => serde_json::from_str(payload),
    }
}

fn parse_kind(value: &str) -> Option<StoreObjectKind> {
    match value {
        "chunk_raw" => Some(StoreObjectKind::ChunkRaw),
        "file_manifest" => Some(StoreObjectKind::FileManifest),
        "tree" => Some(StoreObjectKind::Tree),
        "symlink" => Some(StoreObjectKind::Symlink),
        "tombstone" => Some(StoreObjectKind::Tombstone),
        "step" => Some(StoreObjectKind::Step),
        "repository_view" => Some(StoreObjectKind::RepositoryView),
        "conflict" => Some(StoreObjectKind::Conflict),
        "product" => Some(StoreObjectKind::Product),
        _ => None,
    }
}

fn parse_object_id(kind: StoreObjectKind, value: &str) -> Option<StoreObjectId> {
    macro_rules! parse {
        ($type:ty, $variant:path) => {
            <$type>::parse(value.to_owned()).ok().map($variant)
        };
    }
    match kind {
        StoreObjectKind::ChunkRaw => parse!(ChunkId, StoreObjectId::ChunkRaw),
        StoreObjectKind::FileManifest => parse!(FileManifestId, StoreObjectId::FileManifest),
        StoreObjectKind::Tree => parse!(TreeId, StoreObjectId::Tree),
        StoreObjectKind::Symlink => parse!(SymlinkId, StoreObjectId::Symlink),
        StoreObjectKind::Tombstone => parse!(TombstoneId, StoreObjectId::Tombstone),
        StoreObjectKind::Step => parse!(StepId, StoreObjectId::Step),
        StoreObjectKind::RepositoryView => {
            parse!(RepositoryViewId, StoreObjectId::RepositoryView)
        }
        StoreObjectKind::Conflict => parse!(ConflictId, StoreObjectId::Conflict),
        StoreObjectKind::Product => parse!(layrs_core::scm::ObjectId, StoreObjectId::Product),
    }
}

fn query_strings(
    connection: &Connection,
    sql: &str,
    parameter: &str,
) -> rusqlite::Result<Vec<String>> {
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map([parameter], |row| row.get::<_, String>(0))?;
    rows.collect()
}

fn query_strings_no_param(connection: &Connection, sql: &str) -> rusqlite::Result<Vec<String>> {
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    rows.collect()
}

fn query_pairs(connection: &Connection, sql: &str) -> rusqlite::Result<Vec<(String, String)>> {
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect()
}

fn query_triples(
    connection: &Connection,
    sql: &str,
) -> rusqlite::Result<Vec<(String, String, String)>> {
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    rows.collect()
}

fn query_edges(connection: &Connection, sql: &str) -> rusqlite::Result<Vec<(String, String)>> {
    query_pairs(connection, sql)
}

fn query_node_spaces(
    connection: &Connection,
    sql: &str,
) -> rusqlite::Result<BTreeMap<String, String>> {
    Ok(query_pairs(connection, sql)?.into_iter().collect())
}

fn adjacency(edges: &[(String, String)]) -> BTreeMap<String, BTreeSet<String>> {
    let mut result: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (source, target) in edges {
        result
            .entry(source.clone())
            .or_default()
            .insert(target.clone());
    }
    result
}

fn cyclic_nodes(
    nodes: impl IntoIterator<Item = String>,
    child_parent_edges: &[(String, String)],
) -> BTreeSet<String> {
    let nodes: BTreeSet<_> = nodes.into_iter().collect();
    let mut indegree: BTreeMap<_, usize> = nodes.iter().cloned().map(|node| (node, 0)).collect();
    let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (child, parent) in child_parent_edges {
        if nodes.contains(child) && nodes.contains(parent) {
            *indegree.entry(child.clone()).or_default() += 1;
            children
                .entry(parent.clone())
                .or_default()
                .push(child.clone());
        }
    }
    let mut queue: VecDeque<_> = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(node, _)| node.clone())
        .collect();
    while let Some(node) = queue.pop_front() {
        if let Some(values) = children.get(&node) {
            for child in values {
                if let Some(degree) = indegree.get_mut(child) {
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push_back(child.clone());
                    }
                }
            }
        }
        indegree.remove(&node);
    }
    indegree.into_keys().collect()
}

fn graph_reaches_bounded(
    adjacency: &BTreeMap<String, BTreeSet<String>>,
    start: &str,
    target: &str,
    budget: &mut u64,
) -> Option<bool> {
    let mut seen = BTreeSet::new();
    let mut queue: VecDeque<_> = adjacency
        .get(start)
        .into_iter()
        .flatten()
        .map(String::as_str)
        .collect();
    while let Some(node) = queue.pop_front() {
        if *budget == 0 {
            return None;
        }
        *budget -= 1;
        if node == target {
            return Some(true);
        }
        if !seen.insert(node) {
            continue;
        }
        if let Some(more) = adjacency.get(node) {
            queue.extend(more.iter().map(String::as_str));
        }
    }
    Some(false)
}

fn cross_space_violations(connection: &Connection) -> rusqlite::Result<Vec<(String, String)>> {
    const CHECKS: &[(&str, &str)] = &[
        (
            "SELECT p.layer_id FROM layer_parents p JOIN layers c ON c.layer_id=p.layer_id JOIN layers a ON a.layer_id=p.parent_layer_id WHERE c.space_id<>a.space_id",
            "Layer parent belongs to another Space",
        ),
        (
            "SELECT o.operation_id FROM operations o JOIN repository_views v ON v.repository_view_id=o.resulting_view_id WHERE v.space_id<>o.space_id",
            "Operation resulting View belongs to another Space",
        ),
        (
            "SELECT w.worktree_id FROM worktrees w JOIN operations o ON o.operation_id=w.active_operation_id WHERE o.space_id<>w.space_id",
            "Worktree active Operation belongs to another Space",
        ),
        (
            "SELECT a.object_id FROM space_object_admissions a JOIN operations o ON o.operation_id=a.admitted_by_operation_id WHERE o.space_id<>a.space_id",
            "Object admission cites an Operation from another Space",
        ),
        (
            "SELECT s.observation_id FROM step_observations s JOIN operations o ON o.operation_id=s.operation_id WHERE s.space_id IS NOT NULL AND o.space_id<>s.space_id",
            "StepObservation Operation belongs to another Space",
        ),
        (
            "SELECT s.observation_id FROM step_observations s JOIN worktrees w ON w.worktree_id=s.worktree_id WHERE s.space_id IS NOT NULL AND w.space_id<>s.space_id",
            "StepObservation Worktree belongs to another Space",
        ),
        (
            "SELECT s.observation_id FROM step_observations s WHERE s.space_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM space_object_admissions a WHERE a.space_id=s.space_id AND a.object_id=s.step_id)",
            "StepObservation Step is not admitted to its Space",
        ),
        (
            "SELECT w.worktree_id FROM worktrees w JOIN stores s ON s.store_id=w.store_id WHERE w.store_id<>(SELECT store_id FROM stores ORDER BY store_id LIMIT 1)",
            "Worktree references another Store identity",
        ),
        (
            "SELECT m.marker_id FROM markers m WHERE NOT EXISTS (SELECT 1 FROM space_object_admissions a WHERE a.space_id=m.space_id AND a.object_id=m.target_step_id)",
            "Marker target Step is not admitted to its Space",
        ),
        (
            "SELECT r.release_id FROM releases r WHERE NOT EXISTS (SELECT 1 FROM space_object_admissions a WHERE a.space_id=r.space_id AND a.object_id=r.target_step_id)",
            "Release target Step is not admitted to its Space",
        ),
        (
            "SELECT p.release_publication_id FROM release_publications p JOIN releases r ON r.release_id=p.release_id WHERE p.space_id<>r.space_id",
            "ReleasePublication references a Release from another Space",
        ),
        (
            "SELECT p.release_publication_id FROM release_publications p JOIN operations o ON o.operation_id=p.operation_id WHERE p.space_id<>o.space_id",
            "ReleasePublication references an Operation from another Space",
        ),
        (
            "SELECT r.operation_id FROM operation_rewrites r JOIN operations o ON o.operation_id=r.operation_id WHERE NOT EXISTS (SELECT 1 FROM space_object_admissions a WHERE a.space_id=o.space_id AND a.object_id=r.old_step_id)",
            "Operation rewrite source Step is not admitted to its Space",
        ),
        (
            "SELECT t.operation_id FROM operation_rewrite_targets t JOIN operations o ON o.operation_id=t.operation_id WHERE NOT EXISTS (SELECT 1 FROM space_object_admissions a WHERE a.space_id=o.space_id AND a.object_id=t.new_step_id)",
            "Operation rewrite target Step is not admitted to its Space",
        ),
        (
            "SELECT h.repository_view_id FROM repository_view_layer_heads h JOIN repository_views v ON v.repository_view_id=h.repository_view_id JOIN layers l ON l.layer_id=h.layer_id WHERE v.space_id<>l.space_id",
            "RepositoryView contains a Layer from another Space",
        ),
        (
            "SELECT h.repository_view_id FROM repository_view_layer_heads h JOIN repository_views v ON v.repository_view_id=h.repository_view_id WHERE NOT EXISTS (SELECT 1 FROM space_object_admissions a WHERE a.space_id=v.space_id AND a.object_id=h.step_id)",
            "RepositoryView head Step is not admitted to its Space",
        ),
        (
            "SELECT x.repository_view_id FROM repository_view_markers x JOIN repository_views v ON v.repository_view_id=x.repository_view_id JOIN markers m ON m.marker_id=x.marker_id WHERE v.space_id<>m.space_id",
            "RepositoryView contains a Marker from another Space",
        ),
        (
            "SELECT x.repository_view_id FROM repository_view_releases x JOIN repository_views v ON v.repository_view_id=x.repository_view_id JOIN releases r ON r.release_id=x.release_id WHERE v.space_id<>r.space_id",
            "RepositoryView contains a Release from another Space",
        ),
        (
            "SELECT x.repository_view_id FROM repository_view_worktrees x JOIN repository_views v ON v.repository_view_id=x.repository_view_id JOIN worktrees w ON w.worktree_id=x.worktree_id WHERE v.space_id<>w.space_id",
            "RepositoryView contains a Worktree from another Space",
        ),
        (
            "SELECT x.repository_view_id FROM repository_view_worktrees x JOIN repository_views v ON v.repository_view_id=x.repository_view_id JOIN layers l ON l.layer_id=x.active_layer_id WHERE v.space_id<>l.space_id",
            "RepositoryView Worktree state uses an active Layer from another Space",
        ),
        (
            "SELECT x.repository_view_id FROM repository_view_worktrees x JOIN repository_views v ON v.repository_view_id=x.repository_view_id WHERE x.base_step_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM space_object_admissions a WHERE a.space_id=v.space_id AND a.object_id=x.base_step_id)",
            "RepositoryView Worktree base Step is not admitted to its Space",
        ),
        (
            "SELECT x.repository_view_id FROM repository_view_worktrees x JOIN repository_views v ON v.repository_view_id=x.repository_view_id WHERE x.materialized_tree_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM space_object_admissions a WHERE a.space_id=v.space_id AND a.object_id=x.materialized_tree_id)",
            "RepositoryView Worktree materialized Tree is not admitted to its Space",
        ),
    ];
    let mut findings = Vec::new();
    for (sql, reason) in CHECKS {
        findings.extend(
            query_strings_no_param(connection, sql)?
                .into_iter()
                .map(|id| (id, (*reason).to_owned())),
        );
    }
    Ok(findings)
}

fn query_gc_expected(connection: &Connection) -> rusqlite::Result<Vec<(String, String, String)>> {
    query_triples(
        connection,
        "SELECT 'current_view', space_id, repository_view_id FROM current_repository_views
         UNION ALL
         SELECT 'operation_head', h.operation_id, o.resulting_view_id
           FROM operation_heads h JOIN operations o ON o.operation_id=h.operation_id
         UNION ALL
         SELECT 'retained_operation', operation_id, resulting_view_id FROM operations
         UNION ALL
         SELECT 'retained_operation', operation_id, old_step_id FROM operation_rewrites
         UNION ALL
         SELECT 'retained_operation', operation_id, new_step_id FROM operation_rewrite_targets
         UNION ALL
         SELECT 'worktree', w.worktree_id, o.resulting_view_id
           FROM worktrees w JOIN operations o ON o.operation_id=w.active_operation_id
         UNION ALL
         SELECT 'worktree', w.worktree_id, x.base_step_id
           FROM worktrees w
           JOIN operations o ON o.operation_id=w.active_operation_id
           JOIN repository_view_worktrees x
             ON x.repository_view_id=o.resulting_view_id AND x.worktree_id=w.worktree_id
          WHERE x.base_step_id IS NOT NULL
         UNION ALL
         SELECT 'worktree', w.worktree_id, x.materialized_tree_id
           FROM worktrees w
           JOIN operations o ON o.operation_id=w.active_operation_id
           JOIN repository_view_worktrees x
             ON x.repository_view_id=o.resulting_view_id AND x.worktree_id=w.worktree_id
          WHERE x.materialized_tree_id IS NOT NULL
         UNION ALL
         SELECT 'marker', marker_id, target_step_id FROM markers
         UNION ALL
         SELECT 'release', release_id, target_step_id FROM releases
         UNION ALL
         SELECT 'observation', observation_id, step_id FROM step_observations
         UNION ALL
         SELECT 'pin', pin_id, object_id FROM object_pins UNION ALL SELECT 'product',p.product_object_id,p.product_object_id FROM products p JOIN space_object_admissions a ON a.object_id=p.product_object_id",
    )
}

fn value_as_projection_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        serde_json::Value::Bool(value) => Some(if *value { "1" } else { "0" }.to_owned()),
        serde_json::Value::Null => None,
        _ => Some(value.to_string()),
    }
}

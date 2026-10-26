//! Complete bootstrap SQL interpretation. Pure row comparisons alone are not authority.
use super::{
    database_io,
    records::{
        self,
        activation::{ActivationRowData, ProviderActivationRecord, ProviderActiveMountRecord},
        preparation::{MountRowData, ProviderPreparationRecord},
        *,
    },
    recovery::error::{Result, invalid},
};
use crate::projection::{VersionedProjection, database, retention, selection};
use layrs_core::{
    projection::ProjectionSnapshot,
    scm::{FileManifest, RepoPath},
};
use layrs_vfs::Cancellation;
use rusqlite::Connection;
mod capture;
pub(in crate::projection) use capture::{CaptureContentProof, PrivateCaptureProof};

pub(super) enum PrivateValidation {
    Uninitialized,
    Initialized(MountRecord),
}
pub(super) enum PrivateSourceValidation {
    Uninitialized,
    Initialized {
        mount: MountRecord,
        source: ProviderBootstrapSource,
    },
}
pub(super) struct PrivatePreparationValidation {
    pub(super) mount: ProviderPreparationRecord,
    pub(super) source: ProviderBootstrapSource,
}
pub(super) struct PrivateActiveValidation {
    pub(super) mount: ProviderActiveMountRecord,
    pub(super) activation: ProviderActivationRecord,
    pub(super) source: ProviderBootstrapSource,
    pub(super) activated: VersionedProjection,
}
pub(super) struct PrivateActiveCaptureValidation {
    pub(super) active: PrivateActiveValidation,
    pub(super) capture: PrivateCaptureProof,
}
pub(super) enum PrivateActivationStateValidation {
    Prepared(PrivatePreparationValidation),
    Active(PrivateActiveValidation),
}

#[derive(Clone, Copy)]
enum PrivateMode<'a> {
    Bootstrap,
    Preparation {
        expected_binding: &'a ProviderBootstrapBinding,
        expected_root_id: Option<[u8; 16]>,
    },
    Active {
        expected_binding: &'a ProviderBootstrapBinding,
        expected_root_id: Option<[u8; 16]>,
    },
    ActiveCapture {
        expected_binding: &'a ProviderBootstrapBinding,
        expected_root_id: Option<[u8; 16]>,
    },
}

impl PrivateMode<'_> {
    fn active(self) -> bool {
        matches!(self, Self::Active { .. } | Self::ActiveCapture { .. })
    }
    fn capture(self) -> bool {
        matches!(self, Self::ActiveCapture { .. })
    }
}

enum ValidatedMount {
    Bootstrap(MountRecord),
    Preparation(ProviderPreparationRecord),
    Active(
        ProviderActiveMountRecord,
        ProviderActivationRecord,
        VersionedProjection,
    ),
}

pub(super) const EMPTY_TABLES: [&str; 31] = [
    "projection_install",
    "projection_refresh_pending",
    "projection_refresh_history",
    "projection_capture_files",
    "projection_capture_chunks",
    "projection_captures",
    "projection_publication_pending",
    "projection_publications",
    "projection_namespace_pending",
    "projection_namespace_operations",
    "projection_restore_pending",
    "projection_restore_history",
    "projection_rebase_pending",
    "projection_rebase_history",
    "projection_rebase_previews",
    "projection_content",
    "projection_claims",
    "projection_budgets",
    "projection_prejournal",
    "projection_preparations",
    "projection_observations",
    "projection_imports",
    "projection_conversion",
    "projection_provider_activations",
    "projection_provider_capture_transactions",
    "projection_provider_capture_receipts",
    "projection_provider_transitions",
    "projection_provider_transition_receipts",
    "projection_provider_companions",
    "projection_provider_companion_files",
    "projection_provider_companion_deletions",
];
pub(super) const SINGLE_TABLES: [&str; 7] = [
    "projection_meta",
    "projection_snapshots",
    "projection_head",
    "projection_selections",
    "projection_backend",
    "projection_provider_catalogs",
    "projection_provider_mounts",
];

pub(super) fn private(
    c: &Connection,
    allocation: &AllocationRecord,
    prepared: Option<&PreparedRecord>,
) -> Result<PrivateValidation> {
    match private_with_source(c, allocation, prepared)? {
        PrivateSourceValidation::Uninitialized => Ok(PrivateValidation::Uninitialized),
        PrivateSourceValidation::Initialized { mount, .. } => {
            Ok(PrivateValidation::Initialized(mount))
        }
    }
}

pub(super) fn private_with_source(
    c: &Connection,
    allocation: &AllocationRecord,
    prepared: Option<&PreparedRecord>,
) -> Result<PrivateSourceValidation> {
    match private_interpret(c, allocation, prepared, PrivateMode::Bootstrap, None)? {
        None => Ok(PrivateSourceValidation::Uninitialized),
        Some((ValidatedMount::Bootstrap(mount), source)) => {
            Ok(PrivateSourceValidation::Initialized { mount, source })
        }
        Some((ValidatedMount::Preparation(_) | ValidatedMount::Active(..), _)) => {
            unreachable!("bootstrap mode is v1 only")
        }
    }
}

/// Full schema8/source validation for an already authenticated post-bootstrap
/// operation. No SQL-shaped DATA returned here establishes native READY.
pub(super) fn private_preparation_with_source(
    c: &Connection,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    expected_binding: &ProviderBootstrapBinding,
    expected_root_id: [u8; 16],
) -> Result<PrivatePreparationValidation> {
    match private_interpret(
        c,
        allocation,
        Some(prepared),
        PrivateMode::Preparation {
            expected_binding,
            expected_root_id: Some(expected_root_id),
        },
        None,
    )? {
        Some((ValidatedMount::Preparation(mount), source)) => {
            Ok(PrivatePreparationValidation { mount, source })
        }
        _ => unreachable!("post-bootstrap mode is initialized v2 only"),
    }
}

pub(super) fn private_preparation_with_source_cancellable(
    c: &Connection,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    expected_binding: &ProviderBootstrapBinding,
    expected_root_id: [u8; 16],
    cancel: &Cancellation,
) -> Result<PrivatePreparationValidation> {
    match private_interpret(
        c,
        allocation,
        Some(prepared),
        PrivateMode::Preparation {
            expected_binding,
            expected_root_id: Some(expected_root_id),
        },
        Some(cancel),
    )? {
        Some((ValidatedMount::Preparation(mount), source)) => {
            Ok(PrivatePreparationValidation { mount, source })
        }
        _ => unreachable!("post-bootstrap mode is initialized v2 only"),
    }
}

/// Derive the root ID from the bounded mount materialized by the full schema8
/// interpreter, after its schema/cardinality/length fences and before native use.
pub(super) fn private_preparation_reopen_with_source(
    c: &Connection,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    expected_binding: &ProviderBootstrapBinding,
) -> Result<PrivatePreparationValidation> {
    match private_interpret(
        c,
        allocation,
        Some(prepared),
        PrivateMode::Preparation {
            expected_binding,
            expected_root_id: None,
        },
        None,
    )? {
        Some((ValidatedMount::Preparation(mount), source)) => {
            Ok(PrivatePreparationValidation { mount, source })
        }
        _ => unreachable!("post-bootstrap reopen mode is initialized v2 only"),
    }
}

/// Complete schema8 Active interpretation. Returns DATA; the caller must
/// independently authenticate family READY, retained pins and native READY.
pub(super) fn private_active_with_source(
    c: &Connection,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    expected_binding: &ProviderBootstrapBinding,
    expected_root_id: Option<[u8; 16]>,
    cancel: Option<&Cancellation>,
) -> Result<PrivateActiveValidation> {
    match private_interpret(
        c,
        allocation,
        Some(prepared),
        PrivateMode::Active {
            expected_binding,
            expected_root_id,
        },
        cancel,
    )? {
        Some((ValidatedMount::Active(mount, activation, activated), source)) => {
            Ok(PrivateActiveValidation {
                mount,
                activation,
                source,
                activated,
            })
        }
        _ => unreachable!("active mode is initialized v3 only"),
    }
}

/// A separate interpreter for mutable Active capture state. Callers must also
/// validate the returned private proof against the pinned family database.
/// The strict Active admission above remains unchanged.
pub(super) fn private_active_capture_with_source(
    c: &Connection,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    expected_binding: &ProviderBootstrapBinding,
    expected_root_id: Option<[u8; 16]>,
    cancel: Option<&Cancellation>,
) -> Result<PrivateActiveCaptureValidation> {
    let Some((ValidatedMount::Active(mount, activation, activated), source)) = private_interpret(
        c,
        allocation,
        Some(prepared),
        PrivateMode::ActiveCapture {
            expected_binding,
            expected_root_id,
        },
        cancel,
    )?
    else {
        unreachable!("capture mode is initialized Active only")
    };
    let capture = capture::validate(c, allocation, &mount, &activation, &activated, cancel)?;
    Ok(PrivateActiveCaptureValidation {
        active: PrivateActiveValidation {
            mount,
            activation,
            source,
            activated,
        },
        capture,
    })
}

/// Select a prepared or active interpreter only after the exact schema and a
/// bounded singleton phase are known. The selected interpreter repeats every
/// fence and returns no authority without its full source/history validation.
pub(super) fn private_activation_state_with_source(
    c: &Connection,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    expected_binding: &ProviderBootstrapBinding,
    expected_root_id: [u8; 16],
) -> Result<PrivateActivationStateValidation> {
    let version: u32 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version != 8 {
        return Err(invalid(
            "provider activation requires exact private schema8",
        ));
    }
    require_exact_schema(&database_io::schema_inventory(c)?)?;
    let count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM projection_provider_mounts LIMIT 2)",
        [],
        |r| r.get(0),
    )?;
    if count != 1 {
        return Err(invalid("provider activation requires one mount row"));
    }
    let valid: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_provider_mounts WHERE typeof(phase)='text' AND length(CAST(phase AS BLOB))<=32 AND phase IN ('provider_prepared','active'))",
        [], |r| r.get(0),
    )?;
    if !valid {
        return Err(invalid(
            "provider activation mount phase is not bounded prepared/active",
        ));
    }
    let phase: String = c.query_row("SELECT phase FROM projection_provider_mounts", [], |r| {
        r.get(0)
    })?;
    match phase.as_str() {
        "provider_prepared" => Ok(PrivateActivationStateValidation::Prepared(
            private_preparation_with_source(
                c,
                allocation,
                prepared,
                expected_binding,
                expected_root_id,
            )?,
        )),
        "active" => Ok(PrivateActivationStateValidation::Active(
            private_active_with_source(
                c,
                allocation,
                prepared,
                expected_binding,
                Some(expected_root_id),
                None,
            )?,
        )),
        _ => Err(invalid(
            "provider activation phase changed after bounded selection",
        )),
    }
}

fn require_exact_schema(observed: &[(String, String, String)]) -> Result<()> {
    let ddl = [
        database::SQL,
        selection::SQL,
        retention::SQL,
        super::schema::SQL,
    ]
    .concat();
    database_io::with_reference(&ddl, |reference| {
        if observed != database_io::schema_inventory(reference)? {
            return Err(invalid("provider private schema fingerprint differs"));
        }
        Ok(())
    })
}

fn private_interpret(
    c: &Connection,
    allocation: &AllocationRecord,
    prepared: Option<&PreparedRecord>,
    mode: PrivateMode<'_>,
    cancel: Option<&Cancellation>,
) -> Result<Option<(ValidatedMount, ProviderBootstrapSource)>> {
    super::cancellation::check(cancel)?;
    let version: u32 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let observed = database_io::schema_inventory(c)?;
    if version == 0 && observed.is_empty() {
        return match mode {
            PrivateMode::Bootstrap => Ok(None),
            PrivateMode::Preparation { .. }
            | PrivateMode::Active { .. }
            | PrivateMode::ActiveCapture { .. } => {
                Err(invalid("post-bootstrap private database is uninitialized"))
            }
        };
    }
    if version != 8 {
        return Err(invalid("provider bootstrap requires exact private schema8"));
    }
    require_exact_schema(&observed)?;
    super::cancellation::check(cancel)?;
    // Account for every table before any potentially unbounded FK traversal.
    for table in EMPTY_TABLES {
        if (mode.active() && table == "projection_provider_activations")
            || (mode.capture()
                && matches!(
                    table,
                    "projection_provider_capture_transactions"
                        | "projection_provider_capture_receipts"
                        | "projection_captures"
                        | "projection_content"
                        | "projection_claims"
                ))
        {
            continue;
        }
        super::cancellation::check(cancel)?;
        let nonempty: bool = c.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} LIMIT 1)"),
            [],
            |r| r.get(0),
        )?;
        if nonempty {
            return Err(invalid(format!(
                "provider bootstrap requires empty {table}"
            )));
        }
    }
    for table in SINGLE_TABLES {
        super::cancellation::check(cancel)?;
        let active_snapshots = mode.active() && table == "projection_snapshots";
        let limit = if active_snapshots { 3 } else { 2 };
        let count: u64 = c.query_row(
            &format!("SELECT count(*) FROM (SELECT 1 FROM {table} LIMIT {limit})"),
            [],
            |r| r.get(0),
        )?;
        let expected_count = if active_snapshots { 2 } else { 1 };
        if count != expected_count {
            return Err(invalid(format!(
                "provider bootstrap requires exactly one {table} row"
            )));
        }
    }
    if mode.active() {
        let count: u64 = c.query_row(
            "SELECT count(*) FROM (SELECT 1 FROM projection_provider_activations LIMIT 2)",
            [],
            |r| r.get(0),
        )?;
        if count != 1 {
            return Err(invalid("active provider requires one activation row"));
        }
    }
    let count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM projection_provider_descriptors LIMIT 100001)",
        [],
        |r| r.get(0),
    )?;
    if count > super::PROVIDER_CATALOG_MAX_DESCRIPTORS {
        return Err(invalid("provider descriptor count exceeds bound"));
    }
    // Exact DDL alone is not evidence that historical writes honored CHECKs.
    // Bound every materialized TEXT projection before fetching Rust Strings or
    // sorting descriptor rows; corrupt/ignore_check_constraints data is refused.
    let mount_phase_bound = match mode {
        PrivateMode::Bootstrap => {
            "length(CAST(id AS BLOB))>128 OR length(CAST(catalog_id AS BLOB))!=71 OR length(CAST(digest AS BLOB))!=71 OR phase NOT IN ('allocating','control_prepared','control_published')"
        }
        PrivateMode::Preparation { .. } => {
            "length(CAST(id AS BLOB))>128 OR length(CAST(catalog_id AS BLOB))!=71 OR length(CAST(digest AS BLOB))!=71 OR phase NOT IN ('control_published','provider_prepared')"
        }
        PrivateMode::Active { .. } | PrivateMode::ActiveCapture { .. } => {
            "length(CAST(id AS BLOB))>128 OR length(CAST(catalog_id AS BLOB))!=71 OR length(CAST(digest AS BLOB))!=71 OR phase!='active'"
        }
    };
    let snapshot_projection_bound = match mode {
        PrivateMode::Active { .. } | PrivateMode::ActiveCapture { .. } => {
            "generation NOT IN (0,1) OR length(CAST(digest AS BLOB))!=64 OR (generation=0 AND parent_digest IS NOT NULL) OR (generation=1 AND (parent_digest IS NULL OR length(CAST(parent_digest AS BLOB))!=64))"
        }
        _ => "length(CAST(digest AS BLOB))!=64 OR parent_digest IS NOT NULL",
    };
    for (table, predicate) in [
        ("projection_snapshots", snapshot_projection_bound),
        ("projection_selections", "length(CAST(digest AS BLOB))!=64"),
        (
            "projection_backend",
            "backend NOT IN ('ordinary','provider') OR length(CAST(mount_id AS BLOB))>128",
        ),
        (
            "projection_provider_catalogs",
            "length(CAST(id AS BLOB))!=71 OR length(CAST(source_digest AS BLOB))!=64 OR length(CAST(selection_digest AS BLOB))!=64",
        ),
        ("projection_provider_mounts", mount_phase_bound),
        (
            "projection_provider_descriptors",
            "length(CAST(catalog_id AS BLOB))!=71 OR length(CAST(path AS BLOB))=0 OR length(CAST(path AS BLOB))>4096 OR length(CAST(file_id AS BLOB))!=71 OR length(CAST(manifest_id AS BLOB))!=71 OR executable NOT IN (0,1) OR bytes<0",
        ),
    ] {
        super::cancellation::check(cancel)?;
        let bad: bool = c.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE {predicate})"),
            [],
            |r| r.get(0),
        )?;
        if bad {
            return Err(invalid(format!(
                "provider {table} text/projection bound differs"
            )));
        }
    }
    if mode.active() {
        super::cancellation::check(cancel)?;
        let bad: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM projection_provider_activations WHERE length(CAST(id AS BLOB))!=71 OR length(CAST(owner_identity_digest AS BLOB))!=71 OR length(CAST(mount_id AS BLOB))=0 OR length(CAST(mount_id AS BLOB))>128 OR length(CAST(catalog_id AS BLOB))!=71 OR source_generation!=0 OR length(CAST(source_digest AS BLOB))!=64 OR activated_generation!=1 OR length(CAST(activated_digest AS BLOB))!=64)",
            [], |r| r.get(0),
        )?;
        if bad {
            return Err(invalid(
                "active provider activation projection bound differs",
            ));
        }
    }
    let (logical, largest): (u64, u64) = c.query_row(
        "SELECT coalesce(sum(length(CAST(path AS BLOB))+length(manifest)+222),0),coalesce(max(length(manifest)),0) FROM projection_provider_descriptors",
        [], |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    // 71+71+8+1+71 = 222, the frozen descriptor logical accounting.
    if logical > super::PROVIDER_CATALOG_AGGREGATE_MAX_BYTES
        || largest > super::PROVIDER_DESCRIPTOR_MAX_BYTES
    {
        return Err(invalid("provider descriptor aggregate exceeds bound"));
    }
    let mount_payload_bound = match mode {
        PrivateMode::Bootstrap => 1024 * 1024,
        PrivateMode::Preparation { .. } => ProviderPreparationRecord::MAX_BYTES as u64,
        PrivateMode::Active { .. } | PrivateMode::ActiveCapture { .. } => {
            ProviderActiveMountRecord::MAX_BYTES as u64
        }
    };
    for (table, column, bound) in [
        ("projection_meta", "identity", 65536u64),
        ("projection_snapshots", "payload", 64 * 1024 * 1024),
        ("projection_selections", "payload", 16 * 1024 * 1024),
        (
            "projection_provider_catalogs",
            "payload",
            super::PROVIDER_CATALOG_MAX_BYTES,
        ),
        ("projection_provider_mounts", "payload", mount_payload_bound),
        (
            "projection_provider_activations",
            "payload",
            ProviderActivationRecord::MAX_BYTES as u64,
        ),
    ] {
        if table == "projection_provider_activations" && !mode.active() {
            continue;
        }
        super::cancellation::check(cancel)?;
        let bytes: u64 = c.query_row(
            &format!("SELECT coalesce(max(length({column})),0) FROM {table}"),
            [],
            |r| r.get(0),
        )?;
        if bytes > bound {
            return Err(invalid(format!("provider {table} payload exceeds bound")));
        }
    }
    let (singleton, identity): (u64, Vec<u8>) =
        c.query_row("SELECT singleton,identity FROM projection_meta", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    if singleton != 1 || identity != allocation.format.bytes()? {
        return Err(invalid(
            "provider SQL identity differs from authenticated allocation FORMAT",
        ));
    }
    let (generation, digest, parent, payload): (u64, String, Option<String>, Vec<u8>) = c
        .query_row(
            "SELECT generation,digest,parent_digest,payload FROM projection_snapshots WHERE generation=0",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
    if generation != 0
        || parent.is_some()
        || digest != allocation.source.snapshot_digest
        || database::digest(0, None, &payload) != digest
    {
        return Err(invalid("provider bootstrap snapshot anchor differs"));
    }
    let initial: ProjectionSnapshot = serde_json::from_slice(&payload)
        .map_err(|e| invalid(format!("provider snapshot encoding: {e}")))?;
    if initial
        .canonical_bytes()
        .map_err(|e| invalid(e.to_string()))?
        != payload
    {
        return Err(invalid("provider snapshot is not canonical"));
    }
    let head: (u64, u64) = c.query_row(
        "SELECT singleton,generation FROM projection_head",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let selection_generation: u64 =
        c.query_row("SELECT generation FROM projection_selections", [], |r| {
            r.get(0)
        })?;
    let expected_head = if mode.active() { (1, 1) } else { (1, 0) };
    if head != expected_head || selection_generation != 0 {
        return Err(invalid(
            "provider bootstrap head/selection generation differs",
        ));
    }
    let selection = selection::at(c, 0)?;
    super::cancellation::check(cancel)?;
    allocation.format.validate_initial_selection(&selection)?;
    let (backend_singleton, backend_name, backend_mount): (u64, String, Option<String>) = c
        .query_row(
            "SELECT singleton,backend,mount_id FROM projection_backend",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
    let backend = BackendRow {
        singleton: backend_singleton,
        backend: match backend_name.as_str() {
            "provider" => BackendKind::Provider,
            "ordinary" => BackendKind::Ordinary,
            _ => return Err(invalid("unknown provider backend")),
        },
        mount_id: backend_mount,
    };
    let (catalog_id, source_generation, source_digest, selection_digest, catalog_payload): (String,u64,String,String,Vec<u8>) = c.query_row(
        "SELECT id,source_generation,source_digest,selection_digest,payload FROM projection_provider_catalogs", [],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
    )?;
    let catalog = CatalogRecord::decode(&catalog_payload)?;
    super::cancellation::check(cancel)?;
    if logical
        .checked_add(catalog_payload.len() as u64)
        .is_none_or(|bytes| bytes > super::PROVIDER_CATALOG_AGGREGATE_MAX_BYTES)
    {
        return Err(invalid(
            "provider descriptor/catalog aggregate exceeds bound",
        ));
    }
    struct OwnedRow {
        catalog: String,
        path: String,
        file: String,
        bytes: u64,
        executable: bool,
        manifest_id: String,
        manifest: Vec<u8>,
    }
    let mut statement = c.prepare("SELECT catalog_id,path,file_id,bytes,executable,manifest_id,manifest FROM projection_provider_descriptors ORDER BY catalog_id COLLATE BINARY,path COLLATE BINARY")?;
    let mut rows = statement.query([])?;
    let mut owned = Vec::with_capacity(count as usize);
    let mut inputs = Vec::with_capacity(count as usize);
    while let Some(r) = rows.next()? {
        super::cancellation::check(cancel)?;
        let row = OwnedRow {
            catalog: r.get(0)?,
            path: r.get(1)?,
            file: r.get(2)?,
            bytes: r.get(3)?,
            executable: r.get(4)?,
            manifest_id: r.get(5)?,
            manifest: r.get(6)?,
        };
        let manifest: FileManifest =
            records::codec::decode(&row.manifest, super::PROVIDER_DESCRIPTOR_MAX_BYTES as usize)?;
        let path = RepoPath::parse(&row.path).map_err(|e| invalid(e.to_string()))?;
        inputs.push(ProviderSourceDescriptor { path, manifest });
        owned.push(row);
    }
    drop(rows);
    drop(statement);
    let source = match cancel {
        Some(cancel) => {
            ProviderBootstrapSource::validate_cancellable(initial, selection, inputs, cancel)
                .map_err(|error| {
                    if cancel.is_cancelled() && records::source::is_source_cancelled(&error) {
                        super::recovery::error::ProviderBootstrapCause::Vfs(
                            layrs_vfs::Error::Cancelled,
                        )
                    } else {
                        error.into()
                    }
                })?
        }
        None => ProviderBootstrapSource::validate(initial, selection, inputs)?,
    };
    super::cancellation::check(cancel)?;
    allocation.require_source_and_member(
        &source,
        std::path::Path::new(&allocation.private.metadata_path),
    )?;
    let descriptors: Vec<_> = owned
        .iter()
        .map(|r| DescriptorRow {
            catalog_id: &r.catalog,
            path: &r.path,
            file_id: &r.file,
            bytes: r.bytes,
            executable: r.executable,
            manifest_id: &r.manifest_id,
            manifest: &r.manifest,
        })
        .collect();
    let (mount_id, mount_catalog, phase, mount_payload, mount_digest): (
        String,
        String,
        String,
        Vec<u8>,
        String,
    ) = c.query_row(
        "SELECT id,catalog_id,phase,payload,digest FROM projection_provider_mounts",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )?;
    let catalogs = [CatalogRow {
        id: &catalog_id,
        source_generation,
        source_digest: &source_digest,
        selection_digest: &selection_digest,
        record: &catalog,
    }];
    let mount = match mode {
        PrivateMode::Bootstrap => {
            let phase = match phase.as_str() {
                "allocating" => MountPhase::Allocating,
                "control_prepared" => MountPhase::ControlPrepared,
                "control_published" => MountPhase::ControlPublished,
                _ => return Err(invalid("unsupported provider bootstrap mount phase")),
            };
            let mount = MountRecord::decode(&mount_payload)?;
            require_bootstrap_row_data(
                &[backend],
                &catalogs,
                &descriptors,
                &[MountRow {
                    id: &mount_id,
                    catalog_id: &mount_catalog,
                    phase,
                    digest: &mount_digest,
                    record: &mount,
                }],
                &source,
                allocation,
                prepared,
            )?;
            ValidatedMount::Bootstrap(mount)
        }
        PrivateMode::Preparation {
            expected_binding,
            expected_root_id,
        } => {
            let prepared = prepared.ok_or_else(|| invalid("post-bootstrap P is absent"))?;
            let mount = ProviderPreparationRecord::decode(&mount_payload)?;
            let root_id = match expected_root_id {
                Some(root_id) => root_id,
                None => mount.root_id()?,
            };
            require_source_rows(
                &backend,
                &catalogs,
                &descriptors,
                &source,
                allocation,
                cancel,
            )?;
            if &mount.binding != expected_binding {
                return Err(invalid("post-bootstrap expected binding differs"));
            }
            mount.require_references(allocation, prepared, &source, root_id)?;
            mount.require_row(MountRowData {
                id: &mount_id,
                catalog_id: &mount_catalog,
                phase: &phase,
                payload: &mount_payload,
                digest: &mount_digest,
            })?;
            ValidatedMount::Preparation(mount)
        }
        PrivateMode::Active {
            expected_binding,
            expected_root_id,
        }
        | PrivateMode::ActiveCapture {
            expected_binding,
            expected_root_id,
        } => {
            let prepared = prepared.ok_or_else(|| invalid("active provider P is absent"))?;
            let mount = ProviderActiveMountRecord::decode(&mount_payload)?;
            let root_id = match expected_root_id {
                Some(root_id) => root_id,
                None => mount.prepared.root_id()?,
            };
            require_source_rows(
                &backend,
                &catalogs,
                &descriptors,
                &source,
                allocation,
                cancel,
            )?;
            if &mount.prepared.binding != expected_binding {
                return Err(invalid("active provider expected binding differs"));
            }
            super::cancellation::check(cancel)?;
            let (id, owner_identity_digest, activation_mount, activation_catalog,
                activation_source_generation, activation_source_digest,
                activated_generation, activated_digest, activation_payload):
                (String,String,String,String,u64,String,u64,String,Vec<u8>) = c.query_row(
                    "SELECT id,owner_identity_digest,mount_id,catalog_id,source_generation,source_digest,activated_generation,activated_digest,payload FROM projection_provider_activations",
                    [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?)),
                )?;
            let activation = ProviderActivationRecord::decode(&activation_payload)?;
            activation.require_row(ActivationRowData {
                id: &id,
                owner_identity_digest: &owner_identity_digest,
                mount_id: &activation_mount,
                catalog_id: &activation_catalog,
                source_generation: activation_source_generation,
                source_digest: &activation_source_digest,
                activated_generation,
                activated_digest: &activated_digest,
                payload: &activation_payload,
            })?;
            let (generation1, digest1, parent1, payload1): (u64, String, Option<String>, Vec<u8>) = c.query_row(
                "SELECT generation,digest,parent_digest,payload FROM projection_snapshots WHERE generation=1",
                [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
            )?;
            if generation1 != 1
                || parent1.as_deref() != Some(source.commitment().snapshot_digest.as_str())
                || digest1 != database::digest(1, parent1.as_deref(), &payload1)
            {
                return Err(invalid("active provider generation-one chain differs"));
            }
            let snapshot1: ProjectionSnapshot = serde_json::from_slice(&payload1)
                .map_err(|e| invalid(format!("active provider snapshot encoding: {e}")))?;
            if snapshot1
                .canonical_bytes()
                .map_err(|e| invalid(e.to_string()))?
                != payload1
            {
                return Err(invalid("active provider snapshot is not canonical"));
            }
            let activated = VersionedProjection {
                generation: generation1,
                digest: digest1,
                snapshot: snapshot1,
            };
            mount
                .prepared
                .require_references(allocation, prepared, &source, root_id)?;
            mount.require_references(
                allocation,
                prepared,
                &source,
                &activation,
                &activated.snapshot,
            )?;
            if activation.activated_digest != activated.digest {
                return Err(invalid(
                    "active provider activation digest differs from generation one",
                ));
            }
            mount.require_row(MountRowData {
                id: &mount_id,
                catalog_id: &mount_catalog,
                phase: &phase,
                payload: &mount_payload,
                digest: &mount_digest,
            })?;
            super::cancellation::check(cancel)?;
            ValidatedMount::Active(mount, activation, activated)
        }
    };
    super::cancellation::check(cancel)?;
    if !mode.capture() {
        let failures: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
            [],
            |r| r.get(0),
        )?;
        if failures {
            return Err(invalid("provider private foreign-key corruption"));
        }
    }
    super::cancellation::check(cancel)?;
    Ok(Some((mount, source)))
}

fn require_source_rows(
    backend: &BackendRow,
    catalogs: &[CatalogRow<'_>],
    descriptors: &[DescriptorRow<'_>],
    source: &ProviderBootstrapSource,
    allocation: &AllocationRecord,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    match cancel {
        Some(cancel) => require_source_row_data_cancellable(
            std::slice::from_ref(backend),
            catalogs,
            descriptors,
            source,
            allocation,
            cancel,
        )
        .map_err(|error| {
            if cancel.is_cancelled() && records::source::is_source_cancelled(&error) {
                super::recovery::error::ProviderBootstrapCause::Vfs(layrs_vfs::Error::Cancelled)
            } else {
                error.into()
            }
        }),
        None => require_source_row_data(
            std::slice::from_ref(backend),
            catalogs,
            descriptors,
            source,
            allocation,
        )
        .map_err(Into::into),
    }
}

#[cfg(test)]
#[path = "validation/active_tests.rs"]
mod active_tests;
#[cfg(test)]
#[path = "validation/preparation_tests.rs"]
mod preparation_tests;
#[cfg(test)]
pub(super) mod tests;

//! Nonmutating discovery and explicit, typed SQLite rollback recovery.
use super::*;
use rusqlite::OptionalExtension;
mod handle;
pub(in crate::projection) use handle::access;
pub(super) use handle::verify_binding;
pub use handle::{ProjectionRecovery, RecoveryBinding};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProjectionRecoveryReason {
    FamilyRollback,
    WorktreeRollback,
    Initializing,
    Conversion,
    FilesystemOperation,
    Publication,
}

/// Evidence for routing to PARTIAL recovery, not a capability to read or mutate
/// content. Rollback reasons mean SQL identity/schema validation is still pending.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectionInspection {
    pub binding: RecoveryBinding,
    pub identity: ProjectionIdentity,
    pub shared: bool,
    pub recovery: Vec<ProjectionRecoveryReason>,
    pub conversion_eligible: bool,
    pub conversion_blocker: Option<String>,
}

impl ProjectionStore {
    pub fn inspect_recovery(root: impl AsRef<Path>) -> Result<ProjectionInspection> {
        inspect(root.as_ref(), false, None)
    }
    pub fn open_bound_recovery(
        root: impl AsRef<Path>,
        expected: &RecoveryBinding,
    ) -> Result<ProjectionRecovery> {
        ProjectionRecovery::open(root.as_ref(), expected)
    }
    pub fn recover_databases(
        root: impl AsRef<Path>,
        expected: &RecoveryBinding,
    ) -> Result<ProjectionInspection> {
        inspect(root.as_ref(), true, Some(expected))
    }
    pub fn resume_initialization(
        root: impl AsRef<Path>,
        expected: &RecoveryBinding,
    ) -> Result<ProjectionInspection> {
        Self::open_bound_recovery(root, expected)?.resume_initialization()
    }
}

fn inspect(
    root: &Path,
    recover: bool,
    expected: Option<&RecoveryBinding>,
) -> Result<ProjectionInspection> {
    if let Some(expected) = expected {
        expected.validate()?;
    }
    let requested = std::path::absolute(root)?;
    let requested_pin = PinnedDirectory::open(&requested)?;
    let marker = read_marker(&requested)?;
    if requested_pin.identity_token() != marker.checkout_identity {
        return Err(invalid("projection checkout identity changed"));
    }
    // Relative spelling and Windows case aliases are accepted only for the
    // exact pinned root. The original bound path must still name that root.
    let store = ProjectionStore {
        root: PathBuf::from(&marker.checkout_root),
        marker,
    };
    let family = shared::for_store_discovery(&store)?;
    let path = store.marker.metadata_path(&store.root);
    let _lock = StoreLock::acquire_existing(
        &path.join("layrs.lock"),
        if recover {
            SessionMode::Exclusive
        } else {
            SessionMode::Shared
        },
    )
    .map_err(invalid)?;
    let metadata = PinnedDirectory::open(&path)?;
    let pin = super::super::file::DatabasePin::open(&path.join(DATABASE))?;
    let boundaries = preliminary_pins(&store.root, &store.marker, &metadata, &pin)?;
    let binding = RecoveryBinding::from_parts(&store.marker, &pin, family.as_ref())?;
    let verify = || -> Result<()> {
        requested_pin.verify_path_identity()?;
        verify_binding(
            &store.root,
            &store.marker,
            &pin,
            family.as_ref(),
            expected.unwrap_or(&binding),
        )?;
        if read_marker(&requested)? != store.marker || read_marker(&store.root)? != store.marker {
            return Err(invalid("projection identity changed during inspection"));
        }
        boundaries.verify(&store.root, &store.marker)?;
        metadata.verify_path_identity()?;
        pin.verify(&path.join(DATABASE))?;
        if let Some(binding) = &store.marker.shared {
            if pin.identity() != binding.database_identity {
                return Err(invalid("shared Worktree database replaced"));
            }
        }
        if let Some(family) = &family {
            family.verify()?;
        }
        Ok(())
    };
    // All filesystem boundaries, including both database inodes, precede any
    // SQLite query and therefore any classification of its typed error.
    verify()?;
    let mut reasons = Vec::new();
    if let Some(lease) = &family {
        let catalog = match lease.inspection_catalog(recover) {
            Err(e) if !recover && database::readonly_rollback(&e) => {
                verify()?;
                return Ok(result(
                    &store,
                    binding.clone(),
                    vec![ProjectionRecoveryReason::FamilyRollback],
                ));
            }
            other => other?,
        };
        let (bytes, state, registered_path, identity): (Vec<u8>, String, String, String) =
            catalog.connection.query_row(
                "SELECT marker,state,metadata_path,database_identity FROM worktrees WHERE id=?1 AND length(marker)<=65536",
                [store.identity().worktree_id.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?;
        if bytes != store.marker.bytes()?
            || Path::new(&registered_path) != path
            || identity != pin.identity()
            || !matches!(state.as_str(), "ready" | "initializing")
        {
            return Err(invalid("Worktree registration differs from locator"));
        }
        if state == "initializing" {
            reasons.push(ProjectionRecoveryReason::Initializing);
        }
    }
    let read = || -> Result<rusqlite::Connection> {
        if recover {
            database::read_owner_with_recovery(&path.join(DATABASE), &store.marker, &pin)
        } else {
            let c = database::connect(&path.join(DATABASE), false)?;
            database::validate(&c, &store.marker)?;
            Ok(c)
        }
    };
    let c = match read() {
        Err(e) if !recover && database::readonly_rollback(&e) => {
            reasons.push(ProjectionRecoveryReason::WorktreeRollback);
            verify()?;
            return Ok(result(&store, binding.clone(), reasons));
        }
        other => other?,
    };
    verify_boundaries(&store.root, &store.marker, &metadata, &pin)?;
    if store.marker.import_digest.is_some() {
        let imported: bool =
            c.query_row("SELECT EXISTS(SELECT 1 FROM projection_imports)", [], |r| {
                r.get(0)
            })?;
        let ready: Option<bool> = c
            .query_row(
                "SELECT ready FROM projection_conversion WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if imported {
            conversion::provenance(&c, &store)?;
            if ready.is_none() { return Err(invalid("conversion receipt missing")); }
        } else if ready.is_some() || !reasons.contains(&ProjectionRecoveryReason::Initializing)
            || database::current(&c)?.generation != 0
            || c.query_row("SELECT EXISTS(SELECT 1 FROM projection_captures) OR EXISTS(SELECT 1 FROM projection_claims) OR EXISTS(SELECT 1 FROM projection_preparations) OR EXISTS(SELECT 1 FROM projection_budgets) OR EXISTS(SELECT 1 FROM projection_prejournal)", [], |r| r.get::<_, bool>(0))?
        {
            return Err(invalid("unverified conversion bootstrap; retain data and restore backup"));
        }
        if ready != Some(true) {
            reasons.push(ProjectionRecoveryReason::Conversion);
        }
    }
    validate_platform(&database::current(&c)?.snapshot)?;
    let filesystem: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM projection_install) OR EXISTS(SELECT 1 FROM projection_refresh_pending) OR EXISTS(SELECT 1 FROM projection_namespace_pending) OR EXISTS(SELECT 1 FROM projection_restore_pending) OR EXISTS(SELECT 1 FROM projection_rebase_pending)", [], |r| r.get(0))?;
    let publication: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_publication_pending)",
        [],
        |r| r.get(0),
    )?;
    if filesystem {
        reasons.push(ProjectionRecoveryReason::FilesystemOperation);
    }
    if publication {
        reasons.push(ProjectionRecoveryReason::Publication);
    }
    verify()?;
    Ok(result(&store, binding.clone(), reasons))
}

fn result(
    store: &ProjectionStore,
    binding: RecoveryBinding,
    recovery: Vec<ProjectionRecoveryReason>,
) -> ProjectionInspection {
    let shared = store.marker.shared.is_some();
    let conversion_blocker = if recovery.iter().any(|r| {
        matches!(
            r,
            ProjectionRecoveryReason::FamilyRollback | ProjectionRecoveryReason::WorktreeRollback
        )
    }) {
        Some("database_recovery_required")
    } else if recovery.contains(&ProjectionRecoveryReason::FilesystemOperation) {
        Some("pending_filesystem_operation")
    } else if recovery.contains(&ProjectionRecoveryReason::Publication) {
        Some("pending_publication")
    } else if recovery.contains(&ProjectionRecoveryReason::Conversion) {
        Some("conversion_pending")
    } else if recovery.contains(&ProjectionRecoveryReason::Initializing) {
        Some("initializing")
    } else if shared {
        Some("already_shared")
    } else {
        None
    };
    ProjectionInspection {
        binding,
        identity: store.identity().clone(),
        shared,
        conversion_eligible: conversion_blocker.is_none(),
        conversion_blocker: conversion_blocker.map(str::to_owned),
        recovery,
    }
}

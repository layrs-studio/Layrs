//! Local exact-version operations shared by CLI and Desktop. These entrypoints
//! never acquire bytes from a server; online preparation is a separate action.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use layrs_core::{
    projection::{AvailabilityRequest, ProjectionVersion},
    scm::{RepoPath, StoreId, WorktreeId},
};
use layrs_store_local::projection::{
    PartialStoreFamily, PartialStoreIdentity, ProjectionInspection, ProjectionStore,
    RecoveryBinding, RestorePreview, RestoreSource, SharedQuota,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ProjectionOfflineAction {
    InspectConversionSource {},
    InspectRecovery {},
    RecoverDatabases {
        expected: RecoveryBinding,
    },
    ResumeInitialization {
        expected: RecoveryBinding,
    },
    CurrentVersion {},
    Availability {
        request: AvailabilityRequest,
    },
    Compare {
        request: AvailabilityRequest,
    },
    ReadFile {
        version: ProjectionVersion,
        path: RepoPath,
        max_bytes: u32,
    },
    PreviewRestore {
        expected_generation: u64,
        version: ProjectionVersion,
        paths: Vec<RepoPath>,
    },
    ApplyRestore {
        preview: RestorePreview,
    },
    Preparations {},
    ReleasePreparation {
        id: String,
    },
    CancelSharedReservation {
        expected: RecoveryBinding,
    },
    ReleaseGenerationRetention {
        generation: u64,
    },
    FamilyUsage {},
    SetFamilyQuota {
        bytes: u64,
    },
    EvictUnretained {},
    CreateFamily {
        path: PathBuf,
        quota_bytes: u64,
    },
    ConvertIntoFamily {
        family_root: PathBuf,
        destination: PathBuf,
    },
    ResumeConversion {
        destination: PathBuf,
    },
    AbandonConversion {
        family_root: PathBuf,
        destination: PathBuf,
    },
}

pub fn execute_projection_offline(
    root: impl AsRef<Path>,
    action: ProjectionOfflineAction,
) -> Result<Value, String> {
    // Discovery and conversion previews must not migrate a legacy database or
    // resume an interrupted operation just to render the Desktop surface.
    let inspection = match &action {
        ProjectionOfflineAction::InspectConversionSource {}
        | ProjectionOfflineAction::InspectRecovery {} => {
            Some(ProjectionStore::inspect_recovery(root.as_ref()))
        }
        ProjectionOfflineAction::RecoverDatabases { expected } => Some(
            ProjectionStore::open_bound_recovery(root.as_ref(), expected)
                .and_then(|recovery| recovery.recover_databases()),
        ),
        ProjectionOfflineAction::ResumeInitialization { expected } => Some(
            ProjectionStore::open_bound_recovery(root.as_ref(), expected)
                .and_then(|recovery| recovery.resume_initialization()),
        ),
        _ => None,
    };
    if let Some(inspection) = inspection {
        return inspection.map(inspection_result).map_err(error);
    }
    if let ProjectionOfflineAction::CancelSharedReservation { expected } = &action {
        ProjectionStore::open_bound_recovery(root.as_ref(), expected)
            .and_then(|recovery| recovery.cancel_shared_reservation())
            .map_err(error)?;
        return Ok(json!({"reservationCancelled":true}));
    }
    // Recovery binds the explicit family and destination itself. The original
    // checkout may have changed or become unavailable after conversion failed.
    if let ProjectionOfflineAction::AbandonConversion {
        family_root,
        destination,
    } = &action
    {
        let family = PartialStoreFamily::open(family_root).map_err(error)?;
        family.abandon_conversion(destination).map_err(error)?;
        return Ok(json!({"destination":destination,"abandoned":true,"filesRetained":true}));
    }
    let store = match &action {
        ProjectionOfflineAction::CreateFamily { .. }
        | ProjectionOfflineAction::ConvertIntoFamily { .. }
        | ProjectionOfflineAction::ResumeConversion { .. } => {
            ProjectionStore::open_conversion_source(root).map_err(error)?
        }
        _ => ProjectionStore::open(root).map_err(error)?,
    };
    match action {
        ProjectionOfflineAction::InspectConversionSource {}
        | ProjectionOfflineAction::InspectRecovery {}
        | ProjectionOfflineAction::RecoverDatabases { .. }
        | ProjectionOfflineAction::ResumeInitialization { .. } => {
            unreachable!("handled before opening mutable source")
        }
        ProjectionOfflineAction::CurrentVersion {} => Ok(
            json!({"version": store.current_version().map_err(error)?, "shared": store.is_shared()}),
        ),
        ProjectionOfflineAction::Availability { request } => {
            Ok(json!(store.availability(&request).map_err(error)?))
        }
        ProjectionOfflineAction::Compare { request } => {
            Ok(json!({"differences": store.compare_versions(&request).map_err(error)?}))
        }
        ProjectionOfflineAction::ReadFile {
            version,
            path,
            max_bytes,
        } => {
            if max_bytes == 0 || max_bytes > 1024 * 1024 {
                return Err("offline_preview_limit: maxBytes must be between 1 and 1048576".into());
            }
            let mut output = BoundedOutput {
                bytes: Vec::new(),
                limit: max_bytes as usize,
            };
            store
                .read_version_file(&version, &path, &mut output)
                .map_err(error)?;
            Ok(json!({"version":version,"path":path,"encoding":"base64",
                "bytes":output.bytes.len(),"content":STANDARD.encode(&output.bytes)}))
        }
        ProjectionOfflineAction::Preparations {} => {
            Ok(json!({"preparations": store.preparations().map_err(error)?}))
        }
        ProjectionOfflineAction::PreviewRestore {
            expected_generation,
            version,
            paths,
        } => {
            let preview = store
                .preview_restore(
                    expected_generation,
                    &RestoreSource::Version { version, paths },
                )
                .map_err(error)?;
            Ok(json!({"preview":preview}))
        }
        ProjectionOfflineAction::ApplyRestore { preview } => {
            if !matches!(&preview.source, RestoreSource::Version { .. }) {
                return Err("offline_restore_requires_exact_version".into());
            }
            // Recomputes the preview under exclusive access, checking current
            // eligibility, generation and checkout bytes before staging writes.
            let receipt = store.apply_restore(&preview).map_err(error)?;
            Ok(json!({"restoration":receipt}))
        }
        ProjectionOfflineAction::ReleasePreparation { id } => {
            if id.is_empty() || id.len() > 256 {
                return Err("invalid_preparation_id".into());
            }
            store.release_preparation(&id).map_err(error)?;
            Ok(json!({"released":id}))
        }
        ProjectionOfflineAction::CancelSharedReservation { .. } => {
            unreachable!("handled before ordinary open")
        }
        ProjectionOfflineAction::ReleaseGenerationRetention { generation } => {
            let released = store
                .release_generation_retention(generation)
                .map_err(error)?;
            Ok(json!({"generation":generation,"releasedClaims":released}))
        }
        ProjectionOfflineAction::FamilyUsage {} => {
            let family = family(&store)?;
            Ok(
                json!({"identity":family.identity(),"path":family.root(),"usage":family.usage().map_err(error)?}),
            )
        }
        ProjectionOfflineAction::SetFamilyQuota { bytes } => {
            let family = family(&store)?;
            family.set_quota(SharedQuota { bytes }).map_err(error)?;
            Ok(json!({"usage":family.usage().map_err(error)?}))
        }
        ProjectionOfflineAction::EvictUnretained {} => {
            let family = family(&store)?;
            // The Store reconciles durable private claims before reclaiming.
            let reclaimed = family.evict_unretained().map_err(error)?;
            Ok(json!({"evictedObjects":reclaimed,"usage":family.usage().map_err(error)?}))
        }
        ProjectionOfflineAction::CreateFamily { path, quota_bytes } => {
            let source = store.identity();
            let identity = PartialStoreIdentity {
                store_id: StoreId::parse(format!("store:{}", uuid::Uuid::new_v4()))
                    .map_err(error)?,
                space_id: source.space_id.clone(),
                origin: source.origin.clone(),
                principal_id: source.principal_id.clone(),
            };
            let family =
                PartialStoreFamily::create(path, identity, SharedQuota { bytes: quota_bytes })
                    .map_err(error)?;
            Ok(
                json!({"identity":family.identity(),"path":family.root(),"usage":family.usage().map_err(error)?}),
            )
        }
        ProjectionOfflineAction::ConvertIntoFamily {
            family_root,
            destination,
        } => {
            let family = PartialStoreFamily::open(family_root).map_err(error)?;
            let id =
                WorktreeId::parse(format!("worktree:{}", uuid::Uuid::new_v4())).map_err(error)?;
            let (converted, receipt) = store
                .convert_into_family(&family, &destination, id)
                .map_err(error)?;
            conversion_result(&converted, &destination, receipt)
        }
        ProjectionOfflineAction::ResumeConversion { destination } => {
            let (converted, receipt) = store.resume_conversion_into(&destination).map_err(error)?;
            conversion_result(&converted, &destination, receipt)
        }
        ProjectionOfflineAction::AbandonConversion { .. } => {
            unreachable!("handled before opening source")
        }
    }
}

fn family(store: &ProjectionStore) -> Result<PartialStoreFamily, String> {
    store.shared_family().map_err(error)?.ok_or_else(|| {
        "shared_partial_store_required: convert a copy into a shared family first".into()
    })
}

fn conversion_result(
    store: &ProjectionStore,
    destination: &Path,
    receipt: layrs_store_local::projection::ConversionReceipt,
) -> Result<Value, String> {
    let (projection, selection) = store.state_with_selection().map_err(error)?;
    Ok(
        json!({"path":destination,"identity":store.identity(),"projection":projection,
        "selection":selection,"receipt":receipt}),
    )
}

fn error(value: impl std::fmt::Display) -> String {
    value.to_string()
}

fn inspection_result(inspection: ProjectionInspection) -> Value {
    json!({
        "identity":inspection.identity,
        "binding":inspection.binding,
        "shared":inspection.shared,
        "recovery":inspection.recovery,
        "conversion":{
            "allowed":inspection.conversion_eligible,
            "reason":inspection.conversion_blocker,
        },
    })
}

struct BoundedOutput {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for BoundedOutput {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        if input.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other(
                "offline_preview_limit: file exceeds requested maximum",
            ));
        }
        self.bytes.extend_from_slice(input);
        Ok(input.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_limit_never_returns_a_truncated_success() {
        let mut out = BoundedOutput {
            bytes: Vec::new(),
            limit: 4,
        };
        out.write_all(&[0, 255]).unwrap();
        assert!(out.write_all(&[1, 2, 3]).is_err());
        assert_eq!(out.bytes, [0, 255]);
        out.write_all(&[1, 2]).unwrap();
        assert!(out.write_all(&[3]).is_err());
    }
    #[test]
    fn local_actions_reject_network_flags_and_unknown_parameters() {
        for action in [
            "inspectConversionSource",
            "inspectRecovery",
            "currentVersion",
            "preparations",
            "familyUsage",
            "evictUnretained",
        ] {
            assert!(serde_json::from_value::<ProjectionOfflineAction>(json!({
                "action": action
            }))
            .is_ok());
            assert!(serde_json::from_value::<ProjectionOfflineAction>(json!({
                "action": action, "hydrate": true
            }))
            .is_err());
        }
        let action: ProjectionOfflineAction = serde_json::from_value(json!({
            "action":"convertIntoFamily","familyRoot":"family","destination":"copy"
        }))
        .unwrap();
        assert!(matches!(
            action,
            ProjectionOfflineAction::ConvertIntoFamily { .. }
        ));
    }
}

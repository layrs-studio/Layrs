//! Shared CLI/Desktop remote actions. Secrets live only in the OS credential
//! store and process memory. Authentication responses omit bearer/device codes;
//! explicit administration can return newly issued service/webhook secrets once.
use super::{NativeDevicePoll, NativeRemote, NativeWorkspace};
use crate::secret_store::{OsSecretStore, SecretStore};
use layrs_core::scm::{LayerId, ObjectId, SpaceId, StepId};
use layrs_sync::v3::LayerMutationMode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeRemoteRequest {
    pub origin: String,
    pub ca_pem: Option<String>,
    pub action: NativeRemoteAction,
}

#[derive(Deserialize, Serialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum NativeRemoteAction {
    StartLogin,
    PollLogin,
    Whoami,
    /// One page of the Spaces this principal may open (`GET /v3/spaces`).
    /// Independent of the local checkout; `root` is ignored.
    ListSpaces {
        #[serde(default)]
        limit: Option<u16>,
        #[serde(default)]
        cursor: Option<String>,
    },
    /// UI capability hint for one Space (`GET /v3/spaces/:space/capabilities`).
    SpaceCapabilities {
        space: SpaceId,
    },
    Logout,
    Provision {
        workspace: String,
        slug: String,
    },
    Publish {
        #[serde(default)]
        integrate: bool,
    },
    PublishCandidates,
    PublishCancellable {
        #[serde(default)]
        integrate: bool,
        transfer_id: String,
    },
    PublishCandidatesCancellable {
        transfer_id: String,
    },
    Receive,
    ReceiveCancellable {
        transfer_id: String,
    },
    CloneCancellable {
        space: SpaceId,
        name: String,
        transfer_id: String,
    },
    CloneProjection {
        space: SpaceId,
        step: StepId,
    },
    PublishProjection {
        patch_id: u64,
        layer_id: LayerId,
    },
    PublishProjectionCandidate {
        patch_id: u64,
        layer_id: LayerId,
    },
    ScopedReview {
        command: super::remote::ScopedReviewCommand,
    },
    ReceiveProjection {
        layer_id: LayerId,
    },
    PreviewProjectionRebase {
        patch_id: u64,
        layer_id: LayerId,
    },
    PreviewProjectionHistoricalRestore {
        patch_id: u64,
        layer_id: LayerId,
        selection: super::remote::ProjectionHistoricalSelection,
    },
    ApplyProjectionRebase {
        preview: Box<super::remote::NativeProjectionRebasePreview>,
        choices: std::collections::BTreeMap<
            layrs_core::scm::RepoPath,
            super::remote::ProjectionMergeChoice,
        >,
    },
    MigrateProjection {
        destination: String,
        layer_id: LayerId,
    },
    PreviewProjectionNamespace {
        expected_generation: u64,
        mutation: super::NativeProjectionNamespaceMutation,
    },
    ApplyProjectionNamespace {
        preview: Box<super::NativeProjectionNamespacePreview>,
    },
    PreviewProjectionDirectoryMove {
        expected_generation: u64,
        source: layrs_core::scm::RepoPath,
        destination: layrs_core::scm::RepoPath,
    },
    ApplyProjectionDirectoryMove {
        preview: Box<super::remote::NativeProjectionDirectoryPreview>,
    },
    PreviewProjectionRestore {
        expected_generation: u64,
        source: layrs_store_local::projection::RestoreSource,
    },
    ApplyProjectionRestore {
        preview: Box<super::remote::NativeProjectionRestorePreview>,
    },
    CancelTransfer {
        transfer_id: String,
    },
    PreviewChoose {
        layer: LayerId,
        chosen_heads: Vec<StepId>,
        reason: String,
    },
    ApplyChoose {
        preview: Box<super::NativeChoosePreview>,
    },
    PreviewPolicy {
        policy_set_id: ObjectId,
    },
    PreviewPolicyRules {
        rules: serde_json::Value,
    },
    PreviewReplacePolicyRules {
        policy_id: layrs_sync::v3::PolicyId,
        rules: serde_json::Value,
    },
    ReadPolicyRules {},
    ApplyPolicy {
        preview: Box<super::NativePolicyPreview>,
    },
    ObserveStep {
        step: StepId,
        observed_at: layrs_core::scm::Timestamp,
        reason: layrs_core::scm::CaptureReason,
        idempotency_key: layrs_sync::v3::IdempotencyKey,
    },
    ReadFile {
        step: StepId,
        path: layrs_core::scm::RepoPath,
        output: std::path::PathBuf,
    },
    Clone {
        space: SpaceId,
        name: String,
    },
    Collaboration {
        command: super::CollaborationCommand,
    },
    /// Newly issued service/webhook secrets are returned once to the explicit
    /// administration screen; callers must not persist or log the response.
    Administration {
        command: super::AdministrationCommand,
    },
}

pub fn execute_remote(
    root: impl AsRef<Path>,
    request: NativeRemoteRequest,
) -> Result<Value, String> {
    execute_remote_with_store(root.as_ref(), request, &OsSecretStore::new())
}

fn execute_remote_with_store(
    root: &Path,
    request: NativeRemoteRequest,
    secrets: &impl SecretStore,
) -> Result<Value, String> {
    let ca = request.ca_pem.as_deref().map(str::as_bytes);
    let anonymous = NativeRemote::unauthenticated(&request.origin, ca).map_err(remote_error)?;
    let key = credential_key(anonymous.origin());
    let device_key = format!("{key}:pending-device");
    match request.action {
        NativeRemoteAction::StartLogin => {
            if !secrets.status().available {
                return Err("OS credential store unavailable".into());
            }
            let device = anonymous
                .device_start("Layrs CLI / Desktop")
                .map_err(remote_error)?;
            secrets
                .set_token(&device_key, &device.device_code)
                .map_err(|e| e.to_string())?;
            return Ok(
                json!({"status":"pending","userCode":device.user_code,"verificationUri":device.verification_uri,"expiresIn":device.expires_in,"interval":device.interval}),
            );
        }
        NativeRemoteAction::PollLogin => {
            let code = secrets
                .get_token(&device_key)
                .map_err(|e| e.to_string())?
                .ok_or("No device login in progress for this server")?;
            return match anonymous.device_poll(&code).map_err(remote_error)? {
                NativeDevicePoll::Pending { interval } => {
                    Ok(json!({"status":"pending","interval":interval}))
                }
                NativeDevicePoll::Authorized(session) => {
                    secrets
                        .set_token(&key, &session.access_token)
                        .map_err(|e| e.to_string())?;
                    secrets
                        .delete_token(&device_key)
                        .map_err(|e| e.to_string())?;
                    Ok(
                        json!({"status":"authorized","principalId":session.principal_id,"expiresIn":session.expires_in}),
                    )
                }
            };
        }
        _ => {}
    }
    let token = secrets
        .get_token(&key)
        .map_err(|e| e.to_string())?
        .ok_or("Sign in to this server first with remote login")?;
    let remote = match ca {
        Some(ca) => NativeRemote::with_trusted_ca(anonymous.origin(), &token, ca),
        None => NativeRemote::new(anonymous.origin(), &token),
    }
    .map_err(remote_error)?;
    match request.action {
        NativeRemoteAction::Whoami => remote.whoami().map_err(remote_error),
        NativeRemoteAction::ListSpaces { limit, cursor } => remote
            .list_spaces(limit, cursor.as_deref())
            .map_err(remote_error),
        NativeRemoteAction::SpaceCapabilities { space } => {
            remote.space_capabilities(&space).map_err(remote_error)
        }
        NativeRemoteAction::CancelTransfer { transfer_id } => Ok(
            json!({"transferId":transfer_id,"status":super::transfer_control::cancel(&transfer_id,remote.origin(),root,&token).map_err(remote_error)?}),
        ),
        NativeRemoteAction::CloneCancellable {
            space,
            name,
            transfer_id,
        } => {
            let registration =
                super::transfer_control::register(&transfer_id, remote.origin(), root, &token)
                    .map_err(remote_error)?;
            let workspace = remote
                .clone_into_cancellable(root, &space, &name, &registration.control)
                .map_err(remote_error)?;
            Ok(
                json!({"status":"cloned","transferId":transfer_id,"state":workspace.state().map_err(|e|e.to_string())?}),
            )
        }
        NativeRemoteAction::Administration { command } => {
            let space = if command.needs_space() {
                Some(
                    NativeWorkspace::discover(root)
                        .map_err(|e| e.to_string())?
                        .state()
                        .map_err(|e| e.to_string())?
                        .space
                        .id()
                        .clone(),
                )
            } else {
                None
            };
            super::admin::execute(&remote, space.as_ref(), command).map_err(remote_error)
        }
        NativeRemoteAction::Logout => {
            // Keep the local credential if revocation fails so it can be retried.
            remote.logout().map_err(remote_error)?;
            secrets.delete_token(&key).map_err(|e| e.to_string())?;
            secrets
                .delete_token(&device_key)
                .map_err(|e| e.to_string())?;
            Ok(json!({"status":"signedOut"}))
        }
        NativeRemoteAction::Clone { space, name } => {
            let workspace = remote
                .clone_into(root, &space, &name)
                .map_err(remote_error)?;
            Ok(json!({"status":"cloned","state":workspace.state().map_err(|e|e.to_string())?}))
        }
        NativeRemoteAction::CloneProjection { space, step } => {
            let store = remote
                .clone_projection_into(root, &space, &step)
                .map_err(remote_error)?;
            Ok(
                json!({"status":"projectionCloned","identity":store.identity(),"projection":store.state().map_err(|e|e.to_string())?}),
            )
        }
        NativeRemoteAction::PublishProjection { patch_id, layer_id } => {
            let store = layrs_store_local::projection::ProjectionStore::open(root)
                .map_err(|e| format!("projection_store_error: {e}"))?;
            serde_json::to_value(
                remote
                    .publish_projection(&store, patch_id, &layer_id)
                    .map_err(remote_error)?,
            )
            .map_err(|_| "invalid projection publication response".into())
        }
        NativeRemoteAction::PublishProjectionCandidate { patch_id, layer_id } => {
            let store = layrs_store_local::projection::ProjectionStore::open(root)
                .map_err(|e| format!("projection_store_error: {e}"))?;
            serde_json::to_value(
                remote
                    .publish_projection_candidate(&store, patch_id, &layer_id)
                    .map_err(remote_error)?,
            )
            .map_err(|_| "invalid candidate publication response".into())
        }
        NativeRemoteAction::ScopedReview { command } => {
            let store = layrs_store_local::projection::ProjectionStore::open(root)
                .map_err(|e| format!("projection_store_error: {e}"))?;
            if remote.origin().trim_end_matches('/') != store.identity().origin {
                return Err("projection origin mismatch".into());
            }
            let principal = remote.whoami().map_err(remote_error)?;
            if principal["principal_id"].as_str() != Some(store.identity().principal_id.as_str()) {
                return Err("projection review principal changed".into());
            }
            remote
                .scoped_review(&store.identity().space_id, &command)
                .map_err(remote_error)
        }
        NativeRemoteAction::PreviewProjectionHistoricalRestore {
            patch_id,
            layer_id,
            selection,
        } => {
            let store = layrs_store_local::projection::ProjectionStore::open(root)
                .map_err(|e| format!("projection_store_error: {e}"))?;
            serde_json::to_value(
                remote
                    .preview_projection_historical_restore(&store, patch_id, &layer_id, &selection)
                    .map_err(remote_error)?,
            )
            .map_err(|_| "invalid partial historical restoration preview".into())
        }
        NativeRemoteAction::PreviewProjectionRebase { patch_id, layer_id } => {
            let store = layrs_store_local::projection::ProjectionStore::open(root)
                .map_err(|e| format!("projection_store_error: {e}"))?;
            serde_json::to_value(
                remote
                    .preview_projection_rebase(&store, patch_id, &layer_id)
                    .map_err(remote_error)?,
            )
            .map_err(|_| "invalid partial merge preview".into())
        }
        NativeRemoteAction::ApplyProjectionRebase { preview, choices } => {
            let store = layrs_store_local::projection::ProjectionStore::open(root)
                .map_err(|e| format!("projection_store_error: {e}"))?;
            serde_json::to_value(
                remote
                    .apply_projection_rebase(&store, &preview, &choices)
                    .map_err(remote_error)?,
            )
            .map_err(|_| "invalid partial merge result".into())
        }
        NativeRemoteAction::ReceiveProjection { layer_id } => {
            let store = layrs_store_local::projection::ProjectionStore::open(root)
                .map_err(|e| format!("projection_store_error: {e}"))?;
            serde_json::to_value(
                remote
                    .receive_projection(&store, &layer_id)
                    .map_err(remote_error)?,
            )
            .map_err(|_| "invalid projection receive response".into())
        }
        NativeRemoteAction::MigrateProjection {
            destination,
            layer_id,
        } => {
            let workspace = NativeWorkspace::discover(root).map_err(|e| e.to_string())?;
            serde_json::to_value(
                remote
                    .migrate_projection_into(&workspace, Path::new(&destination), &layer_id)
                    .map_err(remote_error)?,
            )
            .map_err(|_| "invalid projection migration response".into())
        }
        NativeRemoteAction::PreviewProjectionNamespace {
            expected_generation,
            mutation,
        } => {
            let store = layrs_store_local::projection::ProjectionStore::open(root)
                .map_err(|e| format!("projection_store_error: {e}"))?;
            serde_json::to_value(
                remote
                    .preview_projection_namespace(&store, expected_generation, &mutation)
                    .map_err(remote_error)?,
            )
            .map_err(|_| "invalid namespace preview response".into())
        }
        NativeRemoteAction::ApplyProjectionNamespace { preview } => {
            let store = layrs_store_local::projection::ProjectionStore::open(root)
                .map_err(|e| format!("projection_store_error: {e}"))?;
            serde_json::to_value(
                remote
                    .apply_projection_namespace(&store, &preview)
                    .map_err(remote_error)?,
            )
            .map_err(|_| "invalid namespace application response".into())
        }
        NativeRemoteAction::PreviewProjectionDirectoryMove {
            expected_generation,
            source,
            destination,
        } => {
            let store = layrs_store_local::projection::ProjectionStore::open(root)
                .map_err(|e| format!("projection_store_error: {e}"))?;
            serde_json::to_value(
                remote
                    .preview_projection_directory_move(
                        &store,
                        expected_generation,
                        &source,
                        &destination,
                    )
                    .map_err(remote_error)?,
            )
            .map_err(|_| "invalid directory preview response".into())
        }
        NativeRemoteAction::ApplyProjectionDirectoryMove { preview } => {
            let store = layrs_store_local::projection::ProjectionStore::open(root)
                .map_err(|e| format!("projection_store_error: {e}"))?;
            serde_json::to_value(
                remote
                    .apply_projection_directory_move(&store, &preview)
                    .map_err(remote_error)?,
            )
            .map_err(|_| "invalid directory application response".into())
        }
        NativeRemoteAction::PreviewProjectionRestore {
            expected_generation,
            source,
        } => {
            let store = layrs_store_local::projection::ProjectionStore::open(root)
                .map_err(|e| format!("projection_store_error: {e}"))?;
            serde_json::to_value(
                remote
                    .preview_projection_restore(&store, expected_generation, &source)
                    .map_err(remote_error)?,
            )
            .map_err(|_| "invalid restoration preview response".into())
        }
        NativeRemoteAction::ApplyProjectionRestore { preview } => {
            let store = layrs_store_local::projection::ProjectionStore::open(root)
                .map_err(|e| format!("projection_store_error: {e}"))?;
            serde_json::to_value(
                remote
                    .apply_projection_restore(&store, &preview)
                    .map_err(remote_error)?,
            )
            .map_err(|_| "invalid restoration application response".into())
        }
        action => {
            let workspace = NativeWorkspace::discover(root).map_err(|e| e.to_string())?;
            match action {
                NativeRemoteAction::Provision {
                    workspace: slug,
                    slug: space,
                } => Ok(
                    json!({"provisioned":remote.provision(&workspace, &slug, &space).map_err(remote_error)?}),
                ),
                NativeRemoteAction::Publish { integrate } => Ok(
                    json!({"receipt":remote.publish(&workspace, if integrate {LayerMutationMode::Integrate}else{LayerMutationMode::Advance}).map_err(remote_error)?}),
                ),
                NativeRemoteAction::PublishCandidates => Ok(
                    json!({"receipt": remote.publish_candidates(&workspace).map_err(remote_error)?}),
                ),
                NativeRemoteAction::PublishCancellable {
                    integrate,
                    transfer_id,
                } => {
                    let registration = super::transfer_control::register(
                        &transfer_id,
                        remote.origin(),
                        root,
                        &token,
                    )
                    .map_err(remote_error)?;
                    let mode = if integrate {
                        LayerMutationMode::Integrate
                    } else {
                        LayerMutationMode::Advance
                    };
                    Ok(
                        json!({"transferId":transfer_id,"receipt":remote.publish_cancellable(&workspace,mode,&registration.control).map_err(remote_error)?}),
                    )
                }
                NativeRemoteAction::PublishCandidatesCancellable { transfer_id } => {
                    let registration = super::transfer_control::register(
                        &transfer_id,
                        remote.origin(),
                        root,
                        &token,
                    )
                    .map_err(remote_error)?;
                    Ok(
                        json!({"transferId":transfer_id,"receipt":remote.publish_candidates_cancellable(&workspace,&registration.control).map_err(remote_error)?}),
                    )
                }
                NativeRemoteAction::Receive => {
                    Ok(json!({"received":remote.receive(&workspace).map_err(remote_error)?}))
                }
                NativeRemoteAction::ReceiveCancellable { transfer_id } => {
                    let registration = super::transfer_control::register(
                        &transfer_id,
                        remote.origin(),
                        root,
                        &token,
                    )
                    .map_err(remote_error)?;
                    Ok(
                        json!({"transferId":transfer_id,"received":remote.receive_cancellable(&workspace,&registration.control).map_err(remote_error)?}),
                    )
                }
                NativeRemoteAction::PreviewChoose {
                    layer,
                    chosen_heads,
                    reason,
                } => {
                    let space = remote
                        .choose_bound_space(&workspace)
                        .map_err(remote_error)?;
                    Ok(
                        json!({"preview":remote.preview_choose(&space,&layer,&chosen_heads,&reason).map_err(remote_error)?}),
                    )
                }
                NativeRemoteAction::ApplyChoose { preview } => {
                    let space = remote
                        .choose_bound_space(&workspace)
                        .map_err(remote_error)?;
                    if space != preview.space_id {
                        return Err("choose preview belongs to another workspace Space".into());
                    }
                    Ok(json!({"receipt":remote.apply_choose(&preview).map_err(remote_error)?}))
                }
                NativeRemoteAction::PreviewPolicy { policy_set_id } => Ok(
                    json!({"preview":remote.preview_stored_policy(&workspace, &policy_set_id).map_err(remote_error)?}),
                ),
                NativeRemoteAction::PreviewPolicyRules { rules } => Ok(
                    json!({"preview":remote.preview_policy_rules(&workspace,&serde_json::to_vec(&rules).map_err(|_|"invalid policy rules JSON")?).map_err(remote_error)?}),
                ),
                NativeRemoteAction::PreviewReplacePolicyRules { policy_id, rules } => Ok(
                    json!({"preview":remote.preview_replace_policy_rules(&workspace,&policy_id,&serde_json::to_vec(&rules).map_err(|_|"invalid policy rules JSON")?).map_err(remote_error)?}),
                ),
                NativeRemoteAction::ReadPolicyRules {} => serde_json::to_value(
                    remote.read_policy_rules(&workspace).map_err(remote_error)?,
                )
                .map_err(|_| "invalid policy rules response".into()),
                NativeRemoteAction::ApplyPolicy { preview } => Ok(
                    json!({"receipt":remote.apply_policy_preview(&workspace, &preview).map_err(remote_error)?}),
                ),
                NativeRemoteAction::ObserveStep {
                    step,
                    observed_at,
                    reason,
                    idempotency_key,
                } => Ok(
                    json!({"receipt":remote.publish_observation(&workspace, &step, &observed_at, reason, &idempotency_key).map_err(remote_error)?}),
                ),
                NativeRemoteAction::ReadFile { step, path, output } => {
                    let (id, length) = remote
                        .download_remote_file(
                            &workspace,
                            &step,
                            &path,
                            std::path::Path::new(&output),
                        )
                        .map_err(remote_error)?;
                    Ok(
                        json!({"fileId":id,"stepId":step,"path":path,"output":output,"byteLength":length}),
                    )
                }
                NativeRemoteAction::Collaboration { command } => super::collaboration::execute(
                    &remote,
                    workspace.state().map_err(|e| e.to_string())?.space.id(),
                    command,
                )
                .map_err(remote_error),
                _ => unreachable!("handled without a checkout"),
            }
        }
    }
}

fn credential_key(canonical_origin: &str) -> String {
    format!(
        "native-v3:{}",
        blake3::hash(canonical_origin.as_bytes()).to_hex()
    )
}
fn remote_error(e: super::NativeRemoteError) -> String {
    format!("{}: {e}", e.code())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn publication_cancellation_actions_require_a_scoped_transfer_id() {
        let publish: NativeRemoteAction = serde_json::from_value(
            json!({"action":"publishCancellable","transferId":"fixture-transfer-1234"}),
        )
        .unwrap();
        assert!(matches!(
            publish,
            NativeRemoteAction::PublishCancellable {
                integrate: false,
                ..
            }
        ));
        let candidates: NativeRemoteAction = serde_json::from_value(
            json!({"action":"publishCandidatesCancellable","transferId":"fixture-transfer-1234"}),
        )
        .unwrap();
        assert!(matches!(
            candidates,
            NativeRemoteAction::PublishCandidatesCancellable { .. }
        ));
        assert!(serde_json::from_value::<NativeRemoteAction>(
            json!({"action":"publishCancellable"})
        )
        .is_err());
    }
    #[test]
    fn catalog_actions_decode_with_optional_page_and_exact_space() {
        let first: NativeRemoteAction =
            serde_json::from_value(json!({"action":"listSpaces"})).unwrap();
        assert!(matches!(
            first,
            NativeRemoteAction::ListSpaces {
                limit: None,
                cursor: None
            }
        ));
        let next: NativeRemoteAction =
            serde_json::from_value(json!({"action":"listSpaces","limit":20,"cursor":"c2VhbGVk"}))
                .unwrap();
        assert!(matches!(
            next,
            NativeRemoteAction::ListSpaces {
                limit: Some(20),
                cursor: Some(_)
            }
        ));
        let capabilities: NativeRemoteAction =
            serde_json::from_value(json!({"action":"spaceCapabilities","space":"space:fixture"}))
                .unwrap();
        assert!(matches!(
            capabilities,
            NativeRemoteAction::SpaceCapabilities { .. }
        ));
        for invalid in [
            json!({"action":"listSpaces","offset":3}),
            json!({"action":"spaceCapabilities"}),
            json!({"action":"spaceCapabilities","space":"not-a-space"}),
        ] {
            assert!(serde_json::from_value::<NativeRemoteAction>(invalid).is_err());
        }
    }
    #[test]
    fn catalog_page_arguments_are_bounded_before_any_request() {
        let remote = NativeRemote::new("https://example.invalid", "fixture-token").unwrap();
        for (limit, cursor) in [
            (Some(0), None),
            (Some(101), None),
            (None, Some("")),
            (None, Some("has space")),
            (None, Some("a&limit=1")),
        ] {
            assert!(matches!(
                remote.list_spaces(limit, cursor),
                Err(super::super::NativeRemoteError::Invalid(_))
            ));
        }
        let too_long = "A".repeat(2049);
        assert!(remote.list_spaces(None, Some(&too_long)).is_err());
    }
    #[test]
    fn credentials_are_scoped_to_the_exact_canonical_origin() {
        let a = NativeRemote::unauthenticated("https://example.com", None).unwrap();
        let b = NativeRemote::unauthenticated("https://EXAMPLE.com:443/", None).unwrap();
        assert_eq!(credential_key(a.origin()), credential_key(b.origin()));
        assert_ne!(
            credential_key(a.origin()),
            credential_key("https://example.com:444/")
        );
        assert!(NativeRemote::unauthenticated("http://example.com", None).is_err());
        assert!(NativeRemote::unauthenticated("https://secret@example.com", None).is_err());
    }
}

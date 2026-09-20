//! Durable publication and object transfer.
use super::*;
impl NativeRemote {
    pub(crate) fn choose_bound_space(&self, workspace: &NativeWorkspace) -> RemoteResult<SpaceId> {
        let binding = self.binding(workspace)?;
        if binding.clone_pending {
            return Err(NativeRemoteError::Invalid(
                "clone incomplete before administrative selection",
            ));
        }
        Ok(binding.space_id)
    }
    /// Provision a new Workspace using the existing native Space and base Layer IDs.
    pub fn provision(
        &self,
        workspace: &NativeWorkspace,
        workspace_slug: &str,
        space_slug: &str,
    ) -> RemoteResult<RemoteProvisionedSpace> {
        let state = workspace.state()?;
        let bases: Vec<_> = state
            .layers
            .iter()
            .filter(|p| p.kind() == LayerKind::Base && p.parent_layer_ids().is_empty())
            .collect();
        if bases.len() != 1 {
            return Err(NativeRemoteError::Invalid(
                "initial publication requires one base Layer",
            ));
        }
        let response:RemoteProvisionedSpace=self.post("v3/workspaces/provision",&serde_json::json!({
            "workspaceSlug":workspace_slug,"spaceSlug":space_slug,"name":state.space.name(),"initialLayer":bases[0]
        }))?;
        if response.space_id != *state.space.id() || response.layer_id != *bases[0].id() {
            return Err(NativeRemoteError::Invalid(
                "provisioned identities differ from native identities",
            ));
        }
        Ok(response)
    }
    pub(super) fn binding(&self, workspace: &NativeWorkspace) -> RemoteResult<Binding> {
        let path = workspace.store_root().join("native-remote.json");
        let state = workspace.state()?;
        if path.try_exists()? {
            let value: Binding = serde_json::from_slice(&bounded_file(&path, MAX_OBJECT)?)?;
            if value.origin != self.origin.as_str() || value.space_id != *state.space.id() {
                return Err(NativeRemoteError::Invalid("remote binding mismatch"));
            }
            Ok(value)
        } else {
            Ok(Binding {
                origin: self.origin.to_string(),
                space_id: state.space.id().clone(),
                client_id: ClientId::parse(format!("client:{}", uuid::Uuid::new_v4()))
                    .expect("UUID"),
                clone_pending: false,
                observed: None,
                pending: None,
                last_receipt: None,
            })
        }
    }
    /// Capture local files, upload the complete immutable closure, then persist a
    /// stable transaction before sending it. A retry replays the exact request.
    pub fn publish(
        &self,
        workspace: &NativeWorkspace,
        mode: LayerMutationMode,
    ) -> RemoteResult<SyncTransactionReceipt> {
        self.publish_internal(workspace, Some(mode))
    }
    /// Admit immutable local candidates without moving any remote head or named
    /// reference. Rewritten published heads require a separate authorized Choose.
    /// As with publish, the persisted pending request is retried exactly.
    pub fn publish_candidates(
        &self,
        workspace: &NativeWorkspace,
    ) -> RemoteResult<SyncTransactionReceipt> {
        self.publish_internal(workspace, None)
    }
    fn publish_internal(
        &self,
        workspace: &NativeWorkspace,
        mode: Option<LayerMutationMode>,
    ) -> RemoteResult<SyncTransactionReceipt> {
        if self.transfer.is_some() {
            return self.publish_controlled(workspace, mode);
        }
        let _lock = remote_lock(workspace)?;
        let mut binding = self.binding(workspace)?;
        if binding.clone_pending {
            return Err(NativeRemoteError::Invalid(
                "finish clone with receive before publication",
            ));
        }
        if mode.is_none()
            && binding.pending.as_ref().is_some_and(|request| {
                request.view_mutation.is_some() || request.policy_root_mutation.is_some()
            })
        {
            return Err(NativeRemoteError::Invalid(
                "retry the pending reference publication before admitting candidates",
            ));
        }
        if binding.pending.is_none() {
            if workspace
                .state()?
                .view
                .layer_heads()
                .iter()
                .any(|h| h.step_ids().len() > 1)
                || workspace.state()?.view.worktrees().iter().any(|w| {
                    w.worktree_id() == workspace.worktree_id()
                        && w.status() == WorktreeStatus::Conflicted
                })
            {
                if projection_dirty(workspace, &workspace.state()?)? {
                    return Err(NativeRemoteError::Invalid(
                        "resolve dirty divergent checkout before publication",
                    ));
                }
            } else {
                workspace.capture()?;
            }
            let state = workspace.state()?;
            let (remote, request) = self.prepare_publication(workspace, &state, &binding, mode)?;
            binding.observed = Some(remote.clone());
            binding.pending = Some(request);
            save_binding(workspace, &binding)?;
        }
        self.finish_publication(workspace, &mut binding)
    }
    pub(super) fn upload(&self, binding: &Binding, object: &CanonicalObject) -> RemoteResult<()> {
        self.transfer_check()?;
        let (kind, bytes) = encode(object)?;
        let descriptor = ObjectDescriptor {
            kind,
            object_id: ObjectId::parse(object.object_id().as_str())?,
            canonical_byte_len: bytes.len() as u64,
            encoded_byte_len: bytes.len() as u64,
            compression: Compression::Identity,
        };
        let request = ObjectPlanRequest {
            protocol: SyncV3Protocol::V3,
            message_type: ObjectPlanRequestType::Request,
            space_id: binding.space_id.clone(),
            client_id: binding.client_id.clone(),
            request_id: new_request_id(),
            objects: vec![descriptor.clone()],
        };
        let plan: ObjectPlanResponse = self.post(
            &format!("v3/spaces/{}/object-plans", binding.space_id),
            &request,
        )?;
        plan.validate_v3()
            .map_err(|_| NativeRemoteError::Invalid("invalid object plan"))?;
        if plan.space_id != binding.space_id
            || plan.client_id != binding.client_id
            || plan.request_id != request.request_id
        {
            return Err(NativeRemoteError::Invalid("object plan binding mismatch"));
        }
        if plan.present_objects == [descriptor.clone()] && plan.missing_objects.is_empty() {
            return Ok(());
        }
        if plan.missing_objects != [descriptor.clone()]
            || !plan.present_objects.is_empty()
            || plan.upload_capabilities.len() != 1
        {
            return Err(NativeRemoteError::Invalid(
                "object plan descriptor mismatch",
            ));
        }
        let capability = &plan.upload_capabilities[0];
        if capability.descriptor != descriptor {
            return Err(NativeRemoteError::Invalid("upload descriptor mismatch"));
        }
        self.transfer_check()?;
        let length = bytes.len() as u64;
        let body = match &self.transfer {
            Some(control) => reqwest::blocking::Body::sized(
                super::publication::UploadReader::new(bytes, control.clone()),
                length,
            ),
            None => reqwest::blocking::Body::from(bytes),
        };
        self.transfer_result(
            self.http
                .put(self.transfer_url(&capability.upload_url)?)
                .timeout(self.transfer_timeout())
                .bearer_auth(&capability.token)
                .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                .body(body)
                .send()
                .map_err(NativeRemoteError::from)
                .and_then(checked),
        )?;
        Ok(())
    }
}

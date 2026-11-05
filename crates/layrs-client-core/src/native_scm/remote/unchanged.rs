//! A fast receive is derived from an admitted Operation, never from the mutable
//! remote binding alone. No stored value grants remote authorization.
use super::*;

const KEY: &str = "layrs.native-receive.v1";
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Witness {
    origin: String,
    store_id: StoreId,
    worktree_id: WorktreeId,
    operation_id: OperationId,
    view_id: RepositoryViewId,
    bootstrap_digest: String,
}
impl NativeRemote {
    fn witness(
        &self,
        workspace: &NativeWorkspace,
        state: &WorkspaceState,
        operation_id: &OperationId,
        view_id: &RepositoryViewId,
        bootstrap: &SpaceBootstrap,
    ) -> RemoteResult<Witness> {
        Ok(Witness {
            origin: self.origin.to_string(),
            store_id: state.worktree.store_id().clone(),
            worktree_id: workspace.worktree_id().clone(),
            operation_id: operation_id.clone(),
            view_id: view_id.clone(),
            // Includes principal, epoch, policies, all Product roots and cursor.
            bootstrap_digest: blake3::hash(&serde_jcs::to_vec(bootstrap)?)
                .to_hex()
                .to_string(),
        })
    }

    pub(super) fn receive_metadata(
        &self,
        workspace: &NativeWorkspace,
        state: &WorkspaceState,
        operation_id: &OperationId,
        view_id: &RepositoryViewId,
        bootstrap: &SpaceBootstrap,
    ) -> RemoteResult<Metadata> {
        Ok(Metadata::new(vec![MetadataEntry::new(
            KEY,
            serde_json::to_string(&self.witness(
                workspace,
                state,
                operation_id,
                view_id,
                bootstrap,
            )?)?,
        )?])?)
    }

    /// Reads and verifies the current local closure, then drops every Store
    /// capability before the caller obtains a second authenticated bootstrap.
    pub(super) fn unchanged_candidate(
        &self,
        workspace: &NativeWorkspace,
        source: &WorkspaceState,
        binding: &Binding,
        bootstrap: &SpaceBootstrap,
    ) -> RemoteResult<bool> {
        self.transfer_check()?;
        if binding.clone_pending
            || binding.pending.is_some()
            || binding.observed.as_ref() != Some(bootstrap)
            || source.operation.kind() != OperationKind::ReceiveRemoteState
            || source.operation.resulting_view_id() != source.view.id()
        {
            return Ok(false);
        }
        let Some(value) = source
            .operation
            .metadata()
            .entries()
            .iter()
            .find(|entry| entry.key() == KEY)
        else {
            return Ok(false);
        };
        let Ok(witness) = serde_json::from_str::<Witness>(value.value()) else {
            return Ok(false);
        };
        if witness
            != self.witness(
                workspace,
                source,
                source.operation.id(),
                source.view.id(),
                bootstrap,
            )?
        {
            return Ok(false);
        }
        let graph = GraphObjects::local(workspace, source)?;
        for id in &bootstrap.graph_root_step_ids {
            if !graph.steps.contains_key(id) {
                return Ok(false);
            }
        }
        graph.verify_all_files(|| self.transfer_check())?;
        rename_validation::validate(&graph)?;
        for product in &bootstrap.product_roots {
            // These are the canonical Product roots consumed by download_graph.
            // All other root identities still participate in exact bootstrap equality.
            if matches!(product, ProductRoot::Weave { .. })
                && !graph
                    .objects
                    .contains_key(&StoreObjectId::Product(product.object_id().clone()))
            {
                return Ok(false);
            }
        }
        let store = workspace.open_store()?;
        let reader = NativeScmReader::new(&store)?;
        if reader.operation_heads(source.space.id())? != [source.operation.id().clone()] {
            return Ok(false);
        }
        for layer in &bootstrap.layers {
            if reader.get_layer(layer.record.id())?.as_ref() != Some(&layer.record) {
                return Ok(false);
            }
        }
        for marker in &bootstrap.markers {
            if reader.get_marker(marker.record.id())?.as_ref() != Some(&marker.record) {
                return Ok(false);
            }
        }
        for release in &bootstrap.releases {
            if reader.get_release(release.record.id())?.as_ref() != Some(&release.record) {
                return Ok(false);
            }
        }
        self.transfer_check()?;
        Ok(true)
    }

    pub(super) fn verify_unchanged_checkout(
        &self,
        workspace: &NativeWorkspace,
        source: &WorkspaceState,
    ) -> RemoteResult<()> {
        let current = workspace.state()?;
        if current.operation.id() != source.operation.id()
            || current.view.id() != source.view.id()
            || current.worktree != source.worktree
        {
            return Err(NativeRemoteError::Local(NativeClientError::StaleCapture));
        }
        if projection_dirty(workspace, &current)? {
            return Err(NativeRemoteError::Invalid(
                "checkout changed during receive verification",
            ));
        }
        self.transfer_check()
    }
}

pub(super) fn report(source: &WorkspaceState) -> ReceiveReport {
    ReceiveReport {
        operation_id: source.operation.id().clone(),
        divergent: source
            .view
            .layer_heads()
            .iter()
            .any(|heads| heads.step_ids().len() > 1),
        materialized: false,
    }
}

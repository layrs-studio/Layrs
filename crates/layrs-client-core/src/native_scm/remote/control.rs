use super::*;

struct Finish(NativeTransferControl);
impl Drop for Finish {
    fn drop(&mut self) {
        self.0.finish();
    }
}
impl NativeRemote {
    /// No implicit Capture. Cancellation only confirms that no transaction was
    /// journaled/sent by this attempt; immutable uploaded objects may remain.
    pub fn publish_cancellable(
        &self,
        workspace: &NativeWorkspace,
        mode: LayerMutationMode,
        control: &NativeTransferControl,
    ) -> RemoteResult<SyncTransactionReceipt> {
        let _finish = Finish(control.clone());
        self.controlled(control).publish(workspace, mode)
    }
    pub fn publish_candidates_cancellable(
        &self,
        workspace: &NativeWorkspace,
        control: &NativeTransferControl,
    ) -> RemoteResult<SyncTransactionReceipt> {
        let _finish = Finish(control.clone());
        self.controlled(control).publish_candidates(workspace)
    }
    fn controlled(&self, control: &NativeTransferControl) -> Self {
        Self {
            http: self.http.clone(),
            origin: self.origin.clone(),
            token: self.token.clone(),
            transfer: Some(control.clone()),
            download_cache: self.download_cache.clone(),
        }
    }
    pub fn clone_into_cancellable(
        &self,
        root: impl AsRef<Path>,
        space: &SpaceId,
        name: &str,
        control: &NativeTransferControl,
    ) -> RemoteResult<NativeWorkspace> {
        let _finish = Finish(control.clone());
        self.controlled(control).clone_into(root, space, name)
    }
    pub fn receive_cancellable(
        &self,
        workspace: &NativeWorkspace,
        control: &NativeTransferControl,
    ) -> RemoteResult<ReceiveReport> {
        let _finish = Finish(control.clone());
        self.controlled(control).receive(workspace)
    }
    pub(super) fn transfer_check(&self) -> RemoteResult<()> {
        if let Some(control) = &self.transfer {
            control.check()?;
        }
        Ok(())
    }
    pub(super) fn transfer_begin_commit(&self) -> RemoteResult<()> {
        if let Some(control) = &self.transfer {
            control.begin_commit()?;
        }
        Ok(())
    }
    pub(super) fn transfer_timeout(&self) -> Duration {
        Duration::from_secs(if self.transfer.is_some() { 30 } else { 120 })
    }
    pub(super) fn transfer_result<T>(&self, result: RemoteResult<T>) -> RemoteResult<T> {
        self.transfer_check()?;
        result
    }
    pub(super) fn receive_controlled(
        &self,
        workspace: &NativeWorkspace,
        faults: &dyn scm_v1::materialize::MaterializationFaults,
    ) -> RemoteResult<ReceiveReport> {
        self.transfer_check()?;
        let lock = remote_lock(workspace)?;
        let mut binding = self.binding(workspace)?;
        if binding.pending.is_some() {
            return Err(NativeRemoteError::Invalid(
                "retry pending publication before receive",
            ));
        }
        let source = workspace.state()?;
        if projection_dirty(workspace, &source)? {
            return Err(NativeRemoteError::Invalid(
                "capture or preserve edits before cancellable receive",
            ));
        }
        let binding_path = workspace.store_root().join("native-remote.json");
        let before = if binding_path.try_exists()? {
            Some(bounded_file(&binding_path, MAX_OBJECT)?)
        } else {
            None
        };
        drop(lock);
        // Only in-memory snapshots remain here: no writer capability, database
        // transaction or per-checkout remote journal lock spans network I/O.
        let mut bootstrap = self.bootstrap(source.space.id())?;
        receive::require_complete_access(&bootstrap)?;
        let unchanged = if self.unchanged_candidate(workspace, &source, &binding, &bootstrap)? {
            let refreshed = self.bootstrap(source.space.id())?;
            receive::require_complete_access(&refreshed)?;
            let same = refreshed == bootstrap;
            bootstrap = refreshed;
            same
        } else {
            false
        };
        let graph = if unchanged {
            None
        } else {
            Some(self.download_graph(&binding, &bootstrap)?)
        };
        self.transfer_check()?;
        let _lock = remote_lock(workspace)?;
        let after = if binding_path.try_exists()? {
            Some(bounded_file(&binding_path, MAX_OBJECT)?)
        } else {
            None
        };
        let current = workspace.state()?;
        if before != after
            || current.operation.id() != source.operation.id()
            || current.view.id() != source.view.id()
            || current.worktree != source.worktree
        {
            return Err(NativeRemoteError::Local(NativeClientError::StaleCapture));
        }
        if projection_dirty(workspace, &current)? {
            return Err(NativeRemoteError::Invalid(
                "checkout changed during download; capture or preserve edits before cancellable receive",
            ));
        }
        self.transfer_begin_commit()?;
        let Some(graph) = graph else {
            return Ok(unchanged::report(&source));
        };
        let report = self.accept_remote(
            workspace,
            &source,
            &bootstrap,
            graph,
            binding.clone_pending,
            faults,
        )?;
        binding.clone_pending = false;
        binding.observed = Some(bootstrap);
        save_binding(workspace, &binding)?;
        Ok(report)
    }
}

use super::{
    prepare, MutationStamp, NativeClient, NativeClientError, NativeWorkspace, PreparedMutation,
    Result, WorkspaceState,
};
use layrs_core::scm::*;
use layrs_store::scm::*;
use layrs_store_local::scm_v1::NativeScmReader;

impl NativeClient {
    /// Metadata-only mutation. Worktree materialization claims cannot change here.
    pub(super) fn prepare_metadata_change(
        &self,
        source: &WorkspaceState,
        next: RepositoryView,
        records: Vec<EntityRecord>,
        kind: OperationKind,
        summary: &str,
        stamp: MutationStamp,
    ) -> Result<PreparedMutation> {
        if next.space_id() != source.space.id() || next.worktrees() != source.view.worktrees() {
            return Err(NativeClientError::InvalidState(
                "metadata change cannot materialize a Worktree",
            ));
        }
        let store = self.open_store()?;
        let reader = NativeScmReader::new(&store)?;
        let active = reader
            .worktree_active_operation(source.worktree.id())?
            .ok_or(NativeClientError::Missing("active Operation"))?;
        let current = reader
            .current_repository_view(source.space.id())?
            .ok_or(NativeClientError::Missing("current View"))?;
        let heads = reader.operation_heads(source.space.id())?;
        if active != *source.operation.id() {
            return Err(NativeClientError::StaleCapture);
        }
        let operation = prepare::operation(
            &stamp,
            source.space.id().clone(),
            vec![active.clone()],
            &next,
            kind,
            summary,
        )?;
        let objects = vec![CanonicalObject::RepositoryView(next.clone())];
        let admissions = prepare::admissions(source.space.id(), &objects);
        let mut records = records;
        records.push(EntityRecord::Operation(operation.clone()));
        let step_id = source
            .view
            .worktrees()
            .iter()
            .find(|w| w.worktree_id() == source.worktree.id())
            .and_then(|w| w.base_step_id())
            .ok_or(NativeClientError::Missing("Worktree base Step"))?
            .clone();
        let batch = WriteBatch::new(
            stamp.transaction_id,
            objects,
            records,
            admissions,
            vec![
                StoreMutation::RepositoryView(RepositoryViewCas::new(
                    source.space.id().clone(),
                    current.generation(),
                    current.view_id().cloned(),
                    next.id().clone(),
                )?),
                StoreMutation::OperationHeads(OperationHeadsCas::new(
                    source.space.id().clone(),
                    heads.clone(),
                    operation.id().clone(),
                    super::worktree_operations::advance_heads(&heads, &active, operation.id()),
                )?),
                StoreMutation::WorktreeActiveOperation(WorktreeActiveOperationCas::new(
                    source.worktree.id().clone(),
                    active,
                    operation.id().clone(),
                )?),
            ],
        )?;
        Ok(PreparedMutation {
            batch,
            step_id,
            chunks: None,
        })
    }
}

impl NativeWorkspace {
    pub fn create_layer(&self, name: &str) -> Result<Layer> {
        self.capture()?;
        let source = self.state()?;
        if source.layers.iter().any(|l| l.name() == name) {
            return Err(NativeClientError::InvalidState(
                "Layer name already visible",
            ));
        }
        let state = source
            .view
            .worktrees()
            .iter()
            .find(|w| w.worktree_id() == self.worktree_id())
            .ok_or(NativeClientError::Missing("Worktree state"))?;
        let stamp = MutationStamp::fresh()?;
        let layer = Layer::new(
            LayerId::parse(format!("layer:{}", uuid::Uuid::new_v4()))?,
            source.space.id().clone(),
            name,
            LayerKind::Exploration,
            LayerStatus::Active,
            state.active_layer_id().cloned().into_iter().collect(),
            LayerSettings::new(true, 500)?,
            stamp.timestamp.clone(),
            Metadata::empty(),
        )?;
        let mut heads = source.view.layer_heads().to_vec();
        heads.push(LayerHeadSet::new(
            layer.id().clone(),
            vec![state
                .base_step_id()
                .ok_or(NativeClientError::Missing("base Step"))?
                .clone()],
        )?);
        let next = RepositoryView::new(
            source.space.id().clone(),
            heads,
            source.view.marker_ids().to_vec(),
            source.view.release_ids().to_vec(),
            source.view.worktrees().to_vec(),
        )?;
        let change = self.client().prepare_metadata_change(
            &source,
            next,
            vec![EntityRecord::Layer(layer.clone())],
            OperationKind::MoveLayerHeads,
            "Create Layer",
            stamp,
        )?;
        self.client().accept(&change)?;
        Ok(layer)
    }

    pub fn markers(&self) -> Result<Vec<Marker>> {
        let state = self.state()?;
        let store = self.open_store()?;
        let reader = NativeScmReader::new(&store)?;
        state
            .view
            .marker_ids()
            .iter()
            .map(|id| {
                reader
                    .get_marker(id)?
                    .ok_or(NativeClientError::Missing("Marker"))
            })
            .collect()
    }

    pub fn releases(&self) -> Result<Vec<Release>> {
        let state = self.state()?;
        let store = self.open_store()?;
        let reader = NativeScmReader::new(&store)?;
        state
            .view
            .release_ids()
            .iter()
            .map(|id| {
                reader
                    .get_release(id)?
                    .ok_or(NativeClientError::Missing("Release"))
            })
            .collect()
    }

    pub fn create_marker(&self, name: &str, notes: &str) -> Result<Marker> {
        let stamp = MutationStamp::fresh()?;
        let id = MarkerId::parse(format!("marker:{}", uuid::Uuid::new_v4()))?;
        let make = |source: &WorkspaceState| -> Result<Marker> {
            Ok(Marker::new(
                id.clone(),
                source.space.id().clone(),
                base_step(source)?,
                MarkerKind::Checkpoint,
                name,
                name,
                notes,
                None,
                stamp.timestamp.clone(),
                vec![],
                Metadata::empty(),
            )?)
        };
        make(&self.state()?)?; // Invalid input cannot trigger an automatic capture.
        self.capture()?;
        let source = self.state()?;
        let marker = make(&source)?;
        let mut ids = source.view.marker_ids().to_vec();
        ids.push(marker.id().clone());
        let next = RepositoryView::new(
            source.space.id().clone(),
            source.view.layer_heads().to_vec(),
            ids,
            source.view.release_ids().to_vec(),
            source.view.worktrees().to_vec(),
        )?;
        let change = self.client().prepare_metadata_change(
            &source,
            next,
            vec![EntityRecord::Marker(marker.clone())],
            OperationKind::CreateMarker,
            "Create Marker",
            stamp,
        )?;
        self.client().accept(&change)?;
        Ok(marker)
    }

    pub fn hide_marker(&self, id: &MarkerId) -> Result<()> {
        let source = self.state()?;
        if !source.view.marker_ids().contains(id) {
            return Err(NativeClientError::Missing("visible Marker"));
        }
        let next = RepositoryView::new(
            source.space.id().clone(),
            source.view.layer_heads().to_vec(),
            source
                .view
                .marker_ids()
                .iter()
                .filter(|m| *m != id)
                .cloned()
                .collect(),
            source.view.release_ids().to_vec(),
            source.view.worktrees().to_vec(),
        )?;
        let change = self.client().prepare_metadata_change(
            &source,
            next,
            vec![],
            OperationKind::HideMarker,
            "Hide Marker",
            MutationStamp::fresh()?,
        )?;
        self.client().accept(&change)?;
        Ok(())
    }

    /// Restore visibility of this exact immutable Marker without moving any
    /// Layer, restoring an old View, or capturing the current worktree.
    pub fn show_marker(&self, id: &MarkerId) -> Result<Marker> {
        let source = self.state()?;
        let store = self.open_store()?;
        let reader = NativeScmReader::new(&store)?;
        let marker = reader
            .get_marker(id)?
            .ok_or(NativeClientError::Missing("Marker"))?;
        if marker.space_id() != source.space.id()
            || !reader.object_admitted_to_space(
                &StoreObjectId::Step(marker.target_step_id().clone()),
                source.space.id(),
            )?
        {
            return Err(NativeClientError::Missing("Marker admitted to this Space"));
        }
        if source.view.marker_ids().contains(id) {
            return Ok(marker);
        }
        let mut ids = source.view.marker_ids().to_vec();
        ids.push(id.clone());
        let next = RepositoryView::new(
            source.space.id().clone(),
            source.view.layer_heads().to_vec(),
            ids,
            source.view.release_ids().to_vec(),
            source.view.worktrees().to_vec(),
        )?;
        drop(reader);
        drop(store);
        let change = self.client().prepare_metadata_change(
            &source,
            next,
            vec![],
            OperationKind::Restore,
            "Restore Marker visibility",
            MutationStamp::fresh()?,
        )?;
        self.client().accept(&change)?;
        Ok(marker)
    }

    pub fn create_release(&self, version: &str, notes: &str) -> Result<Release> {
        if self.releases()?.iter().any(|r| r.version() == version) {
            return Err(NativeClientError::InvalidState(
                "Release version already visible",
            ));
        }
        let stamp = MutationStamp::fresh()?;
        let id = ReleaseId::parse(format!("release:{}", uuid::Uuid::new_v4()))?;
        let make = |source: &WorkspaceState| -> Result<Release> {
            Ok(Release::new(
                id.clone(),
                source.space.id().clone(),
                base_step(source)?,
                version,
                version,
                notes,
                None,
                stamp.timestamp.clone(),
                vec![],
                Metadata::empty(),
            )?)
        };
        make(&self.state()?)?;
        self.capture()?;
        let source = self.state()?;
        if self.releases()?.iter().any(|r| r.version() == version) {
            return Err(NativeClientError::InvalidState(
                "Release version already visible",
            ));
        }
        let release = make(&source)?;
        let mut ids = source.view.release_ids().to_vec();
        ids.push(release.id().clone());
        let next = RepositoryView::new(
            source.space.id().clone(),
            source.view.layer_heads().to_vec(),
            source.view.marker_ids().to_vec(),
            ids,
            source.view.worktrees().to_vec(),
        )?;
        let change = self.client().prepare_metadata_change(
            &source,
            next,
            vec![EntityRecord::Release(release.clone())],
            OperationKind::CreateRelease,
            "Create Release",
            stamp,
        )?;
        self.client().accept(&change)?;
        Ok(release)
    }
}

pub(super) fn base_step(source: &WorkspaceState) -> Result<StepId> {
    source
        .view
        .worktrees()
        .iter()
        .find(|w| w.worktree_id() == source.worktree.id())
        .and_then(|w| w.base_step_id())
        .cloned()
        .ok_or(NativeClientError::Missing("Worktree base Step"))
}

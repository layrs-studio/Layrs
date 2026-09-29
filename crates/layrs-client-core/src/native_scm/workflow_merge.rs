use super::{NativeClientError, NativeMergePreview, NativeWorkspace, Result};
use layrs_core::scm::*;
use layrs_store::scm::{ScmReader, StoreObjectId};
use layrs_store_local::scm_v1::{self, NativeScmReader};
use layrs_weave::{
    graph_merge::GraphMergeLimits,
    reconciliation::{FailureAction, FileMergeLimits, FileMergePolicy},
};
use std::collections::BTreeSet;

impl NativeWorkspace {
    /// Reads only a term reachable from the selected Layer's canonical conflicts.
    pub fn conflict_variant(
        &self,
        layer: &LayerId,
        conflict: &ConflictId,
        term: &ObjectId,
    ) -> Result<serde_json::Value> {
        let documents = self.layer_conflicts(layer)?;
        let document = documents
            .iter()
            .find(|c| c["conflictId"].as_str() == Some(conflict.as_str()))
            .ok_or(NativeClientError::Missing(
                "conflict reachable from selected Layer",
            ))?;
        let variant = ["bases", "sides"]
            .iter()
            .flat_map(|k| document["document"][k].as_array().into_iter().flatten())
            .find(|v| v["termId"].as_str() == Some(term.as_str()))
            .ok_or(NativeClientError::Missing("term in conflict"))?;
        let value = &variant["values"][0]["value"];
        if variant["values"]
            .as_array()
            .is_none_or(|values| values.len() != 1)
            || document["document"]["placements"]
                .as_array()
                .is_none_or(|placements| placements.len() != 1)
        {
            return Err(NativeClientError::InvalidState(
                "variant preview requires one canonical placement",
            ));
        }
        let path = &document["document"]["placements"][0]["path"];
        if value["kind"] != "file" {
            return Ok(
                serde_json::json!({"conflictId":conflict,"termId":term,"path":path,"value":value,"text":null}),
            );
        }
        let state = self.state()?;
        let heads = state
            .view
            .layer_heads()
            .iter()
            .find(|h| h.layer_id() == layer)
            .ok_or(NativeClientError::Missing("Layer"))?;
        let store = scm_v1::open(self.store_root())?;
        let reader = NativeScmReader::new(&store)?;
        let objects = super::merge::load_objects(
            &reader,
            state.space.id(),
            heads.step_ids().to_vec(),
            GraphMergeLimits::default(),
        )?;
        let file = FileId::parse(
            value["objectId"]
                .as_str()
                .ok_or(NativeClientError::Missing("variant File ID"))?,
        )?;
        let bytes = objects
            .files
            .get(&file)
            .ok_or(NativeClientError::Missing("variant bytes"))?;
        let text = if bytes.len() <= 1024 * 1024 && !bytes.contains(&0) {
            std::str::from_utf8(bytes).ok()
        } else {
            None
        };
        Ok(
            serde_json::json!({"conflictId":conflict,"termId":term,"path":path,"value":value,"byteLength":bytes.len(),"text":text}),
        )
    }
    /// Bounded text comparison; omitted binary/large/structural content is never
    /// represented as an empty file or a successful identical comparison.
    pub fn compare_conflict_variants(
        &self,
        layer: &LayerId,
        conflict: &ConflictId,
        before: &ObjectId,
        after: &ObjectId,
    ) -> Result<serde_json::Value> {
        let operation = self.state()?.operation.id().clone();
        let left = self.conflict_variant(layer, conflict, before)?;
        let right = self.conflict_variant(layer, conflict, after)?;
        if self.state()?.operation.id() != &operation {
            return Err(NativeClientError::StaleCapture);
        }
        fn entries(value: &serde_json::Value) -> Option<Vec<layrs_core::scm::DiffEntry<'_>>> {
            match value["value"]["kind"].as_str()? {
                "absent" => Some(vec![]),
                "file" => Some(vec![layrs_core::scm::DiffEntry {
                    path: value["path"].as_str()?,
                    content: layrs_core::scm::DiffContent::File {
                        bytes: value["text"].as_str()?.as_bytes(),
                        executable: value["value"]["executable"].as_bool().unwrap_or(false),
                    },
                }]),
                _ => None,
            }
        }
        let changes = entries(&left)
            .zip(entries(&right))
            .map(|(left, right)| layrs_core::scm::diff_entries(&left, &right));
        Ok(
            serde_json::json!({"conflictId":conflict,"beforeTerm":before,"afterTerm":after,"available":changes.is_some(),"changes":changes,"before":left,"after":right}),
        )
    }
    pub fn layer_named(&self, name: &str) -> Result<LayerId> {
        let state = self.state()?;
        let name = name.strip_prefix("@layer:").unwrap_or(name);
        let candidates: Vec<_> = state
            .layers
            .iter()
            .filter(|layer| layer.id().as_str() == name || layer.name() == name)
            .collect();
        match candidates.as_slice() {
            [layer] => Ok(layer.id().clone()),
            _ => Err(NativeClientError::InvalidState(
                "Layer is missing or ambiguous; use an exact ID",
            )),
        }
    }
    pub fn prepare_named_weave(&self, source: &str, target: &str) -> Result<NativeMergePreview> {
        self.named_weave(source, target, true)
    }
    /// Read-only preview; pending edits must be captured explicitly first.
    pub fn preview_named_weave(&self, source: &str, target: &str) -> Result<NativeMergePreview> {
        self.named_weave(source, target, false)
    }
    fn named_weave(&self, source: &str, target: &str, capture: bool) -> Result<NativeMergePreview> {
        // Only the launching user's deployment environment configures executable
        // trust. Repository files and remote metadata never supply a worker pin.
        let worker = std::env::var_os("LAYRS_LENS_WORKER");
        let digest = std::env::var("LAYRS_LENS_WORKER_SHA256");
        let host = match (worker, digest) {
            (None, Err(std::env::VarError::NotPresent)) => None,
            (Some(path), Ok(digest)) if std::path::Path::new(&path).is_absolute() => Some(
                layrs_weave::wasm_host::WasmHost::open(
                    std::path::Path::new(&path),
                    &digest,
                    Default::default(),
                )
                .map_err(|error| {
                    NativeClientError::InvalidSnapshot(format!("configured Lens host: {error}"))
                })?,
            ),
            _ => {
                return Err(NativeClientError::InvalidState(
                    "configure both an absolute LAYRS_LENS_WORKER and LAYRS_LENS_WORKER_SHA256",
                ));
            }
        };
        if capture {
            self.capture_with_reason(CaptureReason::BeforeOperation)?;
        } else if self.is_dirty()? {
            return Err(NativeClientError::InvalidState(
                "capture pending edits before requesting a read-only preview",
            ));
        }
        let target = self.layer_named(target)?;
        let source_heads = if source.starts_with("blake3:")
            || source.starts_with("@marker:")
            || source.starts_with("@release:")
        {
            vec![self.resolve_step_name(source)?.step_id]
        } else {
            let source = self.layer_named(source)?;
            self.state()?
                .view
                .layer_heads()
                .iter()
                .find(|h| h.layer_id() == &source)
                .ok_or(NativeClientError::Missing("source Layer heads"))?
                .step_ids()
                .to_vec()
        };
        let policy = FileMergePolicy {
            unsupported: FailureAction::StructuralFallback,
            quota: FailureAction::Reject,
            limits: FileMergeLimits::default(),
        };
        match host {
            Some(host) => self.preview_sandboxed_merge(
                &source_heads,
                &target,
                policy,
                GraphMergeLimits::default(),
                &host,
            ),
            None => self.preview_structural_merge(
                &source_heads,
                &target,
                policy,
                GraphMergeLimits::default(),
            ),
        }
    }
    /// Bounded immutable conflict documents rooted in the selected Layer.
    pub fn layer_conflicts(&self, layer: &LayerId) -> Result<Vec<serde_json::Value>> {
        let state = self.state()?;
        let heads = state
            .view
            .layer_heads()
            .iter()
            .find(|h| h.layer_id() == layer)
            .ok_or(NativeClientError::Missing("Layer heads"))?;
        let store = scm_v1::open(self.store_root())?;
        let reader = NativeScmReader::new(&store)?;
        let admitted = |id: &StoreObjectId| -> Result<()> {
            if !reader.object_admitted_to_space(id, state.space.id())? {
                return Err(NativeClientError::Missing("object admitted to this Space"));
            }
            Ok(())
        };
        let mut pending = vec![];
        let mut visited = BTreeSet::new();
        let mut conflicts = BTreeSet::new();
        let mut result = vec![];
        let mut bytes = 0usize;
        let mut entries = 0usize;
        for head in heads.step_ids() {
            admitted(&StoreObjectId::Step(head.clone()))?;
            pending.push((
                reader
                    .get_step(head)?
                    .ok_or(NativeClientError::Missing("Layer head"))?
                    .root_tree_id()
                    .clone(),
                0,
            ));
        }
        while let Some((id, depth)) = pending.pop() {
            if depth > 128 || visited.len() > 100_000 || entries > 100_000 {
                return Err(NativeClientError::InvalidState("conflict traversal limit"));
            }
            if !visited.insert(id.clone()) {
                continue;
            }
            admitted(&StoreObjectId::Tree(id.clone()))?;
            let tree = reader
                .get_tree(&id)?
                .ok_or(NativeClientError::Missing("conflict Tree"))?;
            entries = entries.saturating_add(tree.entries().len());
            if entries > 100_000 {
                return Err(NativeClientError::InvalidState("conflict traversal limit"));
            }
            for entry in tree.entries() {
                match entry.target() {
                    TreeEntryTarget::Tree { tree_id } => pending.push((tree_id.clone(), depth + 1)),
                    TreeEntryTarget::Conflict { conflict_id }
                        if conflicts.insert(conflict_id.clone()) =>
                    {
                        admitted(&StoreObjectId::Conflict(conflict_id.clone()))?;
                        let conflict = reader
                            .get_conflict(conflict_id)?
                            .ok_or(NativeClientError::Missing("Conflict"))?;
                        bytes = bytes.saturating_add(conflict.canonical_json().len());
                        if bytes > 4 * 1024 * 1024 || conflicts.len() > 1000 {
                            return Err(NativeClientError::InvalidState("conflict display limit"));
                        }
                        result.push(serde_json::json!({"conflictId":conflict_id,"document":serde_json::from_slice::<serde_json::Value>(conflict.canonical_json()).map_err(|_|NativeClientError::InvalidState("invalid Conflict JSON"))?}));
                    }
                    _ => {}
                }
            }
        }
        Ok(result)
    }
}

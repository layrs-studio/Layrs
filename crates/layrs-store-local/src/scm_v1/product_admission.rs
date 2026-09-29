//! Contextual receipt checks within the single native write transaction.
use super::*;
use layrs_core::scm::TreeEntryTarget;
use layrs_store::scm::CanonicalProduct;
use layrs_sync::v3::{MergeBase, ProductEnvelope};
use layrs_weave::graph_merge::PlannedConflict;

fn rejected(reason: impl std::fmt::Display) -> StoreError {
    StoreError::Corruption {
        message: format!("Product admission: {reason}"),
    }
}

impl CompositeAdmissionResolver<'_> {
    pub(super) fn validate_product_context(
        &self,
        space: &SpaceId,
        product: &CanonicalProduct,
    ) -> Result<()> {
        product.validate_native_subset()?;
        let declared = match product.envelope() {
            ProductEnvelope::Layer { record, .. } => record.space_id(),
            ProductEnvelope::Marker { record, .. } => record.space_id(),
            ProductEnvelope::Release { record, .. } => record.space_id(),
            ProductEnvelope::Weave { record, .. } => &record.space_id,
            _ => return Err(rejected("unsupported native Product")),
        };
        if declared != space {
            return Err(rejected("Product belongs to another Space"));
        }
        let ProductEnvelope::Weave { record, .. } = product.envelope() else {
            return Ok(());
        };
        let profile = &record.determinism_profile;
        let wasm_host = profile.host_policy.id == layrs_weave::wasm_host::HOST_ID;
        if wasm_host {
            layrs_weave::wasm_host::validate_receipt_host(record).map_err(rejected)?;
        } else if profile.host_policy.id != "layrs.no-lens-execution"
            || !record.lens_executions.is_empty()
        {
            return Err(rejected("Lens executions require a supported sandbox host"));
        }
        let ambient = &profile.host_policy.ambient_capabilities;
        if profile.host_policy.version != "1"
            || profile.merge_algorithm.id != "layrs.native-tree-merge"
            || profile.merge_algorithm.version != "1"
            || profile.base_algorithm.id != "layrs.step-dag-bases"
            || profile.base_algorithm.version != "1"
            || ambient.network
            || ambient.wall_clock
            || ambient.randomness
            || ambient.locale
            || ambient.host_filesystem
            || ambient.environment
        {
            return Err(rejected("unsupported native execution profile"));
        }
        let heads = record
            .source_heads
            .iter()
            .chain(&record.target_heads_before)
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let Some(CanonicalObject::Step(result)) =
            self.object(&StoreObjectId::Step(record.result_step_id.clone()))?
        else {
            return Err(rejected("result Step missing"));
        };
        if result.parent_step_ids() != heads {
            return Err(rejected("result parents do not equal reconciled heads"));
        }
        let bases = record
            .merge_bases
            .iter()
            .map(|base| match base {
                MergeBase::Step { step_id, .. } => Ok(step_id.clone()),
                _ => Err(rejected("virtual base unsupported by native admission")),
            })
            .collect::<Result<Vec<_>>>()?;
        if self.step_dag.merge_bases(&heads).map_err(rejected)? != bases {
            return Err(rejected("receipt bases differ from graph"));
        }
        for set in [&record.source_heads, &record.target_heads_before] {
            for (i, a) in set.iter().enumerate() {
                for b in set.iter().skip(i + 1) {
                    if self.step_dag.is_ancestor(a, b).map_err(rejected)?
                        || self.step_dag.is_ancestor(b, a).map_err(rejected)?
                    {
                        return Err(rejected("receipt head set is not antichain"));
                    }
                }
            }
        }
        let mut conflicts = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut pending = vec![StoreObjectId::Tree(result.root_tree_id().clone())];
        while let Some(id) = pending.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            require_budget("Weave closure objects", seen.len(), MAX_ADMISSION_VISITS)?;
            if !self.object_admitted_result(&id, space)? {
                return Err(rejected("Weave closure outside Space"));
            }
            let object = self
                .object(&id)?
                .ok_or_else(|| rejected("Weave closure object missing"))?;
            match object {
                CanonicalObject::Tree(tree) => {
                    for entry in tree.entries() {
                        match entry.target() {
                            TreeEntryTarget::Tree { tree_id } => {
                                pending.push(StoreObjectId::Tree(tree_id.clone()))
                            }
                            TreeEntryTarget::Conflict { conflict_id } => {
                                pending.push(StoreObjectId::Conflict(conflict_id.clone()))
                            }
                            TreeEntryTarget::File { file_id } => {
                                self.admissible_manifest_for_file(file_id, space)?
                                    .ok_or_else(|| rejected("Weave File closure missing"))?;
                            }
                            TreeEntryTarget::Symlink { symlink_id } => {
                                pending.push(StoreObjectId::Symlink(symlink_id.clone()))
                            }
                            TreeEntryTarget::Tombstone { tombstone_id } => {
                                pending.push(StoreObjectId::Tombstone(tombstone_id.clone()))
                            }
                        }
                    }
                }
                CanonicalObject::Conflict(conflict) => {
                    let parsed = PlannedConflict::from_canonical_bytes(conflict.canonical_json())
                        .map_err(rejected)?;
                    if !parsed.document()["lensPin"].is_null() {
                        let matching = record
                            .lens_executions
                            .iter()
                            .filter(|execution| {
                                execution.conflict_id.as_ref() == Some(conflict.id())
                                    && serde_json::to_value(&execution.pin).ok().as_ref()
                                        == Some(&parsed.document()["lensPin"])
                            })
                            .count();
                        if !wasm_host || matching != 1 {
                            return Err(rejected(
                                "Lens-derived Conflict has no matching sandbox execution",
                            ));
                        }
                    }
                    conflicts.insert(conflict.id().clone());
                    for role in ["bases", "sides"] {
                        for term in parsed.document()[role]
                            .as_array()
                            .ok_or_else(|| rejected("Conflict terms"))?
                        {
                            let provenance = StepId::parse(
                                term["provenance"]["stepId"]
                                    .as_str()
                                    .ok_or_else(|| rejected("Conflict provenance"))?,
                            )
                            .map_err(rejected)?;
                            self.validate_admission(space, &StoreObjectId::Step(provenance))?;
                            let value = &term["values"][0]["value"];
                            let id = value["objectId"].as_str();
                            match value["kind"].as_str() {
                                Some("tree") => pending.push(StoreObjectId::Tree(
                                    TreeId::parse(id.ok_or_else(|| rejected("variant Tree"))?)
                                        .map_err(rejected)?,
                                )),
                                Some("symlink") => pending.push(StoreObjectId::Symlink(
                                    SymlinkId::parse(
                                        id.ok_or_else(|| rejected("variant Symlink"))?,
                                    )
                                    .map_err(rejected)?,
                                )),
                                Some("file") => {
                                    let id =
                                        FileId::parse(id.ok_or_else(|| rejected("variant File"))?)
                                            .map_err(rejected)?;
                                    self.admissible_manifest_for_file(&id, space)?
                                        .ok_or_else(|| rejected("variant File closure"))?;
                                }
                                Some("absent") => {}
                                _ => return Err(rejected("unsupported Conflict variant")),
                            }
                        }
                    }
                }
                CanonicalObject::Symlink(_) | CanonicalObject::Tombstone(_) => {}
                _ => return Err(rejected("unexpected Weave closure kind")),
            }
            require_budget(
                "Weave closure pending",
                pending.len(),
                MAX_ADMISSION_EDGE_VISITS,
            )?;
        }
        if conflicts.into_iter().collect::<Vec<_>>() != record.conflict_ids {
            return Err(rejected(
                "receipt Conflict IDs differ from transitive result closure",
            ));
        }
        Ok(())
    }

    pub(super) fn validate_weave_operations(&self) -> Result<()> {
        for operation in self
            .new_operations
            .values()
            .filter(|op| op.kind() == OperationKind::Weave)
        {
            let receipts = self
                .batch
                .canonical_objects()
                .iter()
                .filter_map(|object| match object {
                    CanonicalObject::Product(product) => match product.envelope() {
                        ProductEnvelope::Weave { record, .. }
                            if record.operation_id == *operation.id() =>
                        {
                            Some((product, record))
                        }
                        _ => None,
                    },
                    _ => None,
                })
                .collect::<Vec<_>>();
            let [(product, receipt)] = receipts.as_slice() else {
                return Err(rejected(
                    "Weave Operation requires exactly one receipt in the same batch",
                ));
            };
            self.validate_product_context(operation.space_id(), product)?;
            if !self.admissions.contains(&(
                operation.space_id().clone(),
                StoreObjectId::Product(product.id().clone()),
            )) {
                return Err(rejected(
                    "Weave receipt requires same-batch Space admission",
                ));
            }
            let [parent] = operation.parent_operation_ids() else {
                return Err(rejected("native Weave requires one local parent Operation"));
            };
            let previous = self
                .operation(parent)?
                .ok_or_else(|| rejected("Weave parent Operation missing"))?;
            let Some(CanonicalObject::RepositoryView(before)) = self.object(
                &StoreObjectId::RepositoryView(previous.resulting_view_id().clone()),
            )?
            else {
                return Err(rejected("Weave previous View missing"));
            };
            let Some(CanonicalObject::RepositoryView(after)) = self.object(
                &StoreObjectId::RepositoryView(operation.resulting_view_id().clone()),
            )?
            else {
                return Err(rejected("Weave resulting View missing"));
            };
            if before.marker_ids() != after.marker_ids()
                || before.release_ids() != after.release_ids()
                || before.layer_heads().len() != after.layer_heads().len()
            {
                return Err(rejected("Weave changed unrelated references"));
            }
            let changed = before
                .layer_heads()
                .iter()
                .zip(after.layer_heads())
                .filter(|(a, b)| a != b)
                .collect::<Vec<_>>();
            let [(old, new)] = changed.as_slice() else {
                return Err(rejected("Weave must move exactly one Layer"));
            };
            if old.layer_id() != new.layer_id()
                || old.step_ids() != receipt.target_heads_before
                || new.step_ids() != [receipt.result_step_id.clone()]
            {
                return Err(rejected("Weave Layer transition differs from receipt"));
            }
            // The canonical envelope remains independently transportable: a
            // remotely received receipt does not require importing Operations.
        }
        Ok(())
    }
}

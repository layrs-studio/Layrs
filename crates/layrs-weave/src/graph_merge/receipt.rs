//! Receipt construction is separate from pure preview. The accepting host must
//! attest to sandbox memory/deadline enforcement; the planner only enforces its
//! deterministic byte/count/depth/work budgets.

use super::*;
use crate::reconciliation::FailureAction;
use layrs_sync::v3::*;

pub struct ReceiptContext {
    pub space_id: SpaceId,
    pub operation_id: OperationId,
    pub created_at: Timestamp,
    pub gate_snapshot_id: Option<ObjectId>,
}

/// Supplying this value is an explicit claim by the accepting host that memory,
/// deadline and optional fuel were enforced around this exact plan. Pure preview
/// alone cannot make that claim. This API does not perform authorization/admission.
pub struct ReceiptHostAttestation {
    pub enforced_profile: DeterminismProfile,
}

impl GraphMergePlan {
    /// Digest the exact implementation and all locally enforced policy knobs.
    /// The host must place this digest in its recorded HostPolicy.
    pub fn execution_policy_digest(&self) -> MergeResult<Sha256Id> {
        let pin = |lens| {
            let mut pin = reconciliation::native_manifest(lens).pin().unwrap();
            if let Some(digest) = self
                .enforced_host
                .as_ref()
                .and_then(|host| host["module_digest"].as_str())
            {
                pin.digest = digest.to_owned();
            }
            pin
        };
        let implementation = [
            include_str!("../graph_merge.rs"),
            include_str!("canonical.rs"),
            include_str!("conflict_layout.rs"),
            include_str!("structural.rs"),
            include_str!("semantic_blocks.rs"),
            include_str!("semantic_blocks/build.rs"),
            include_str!("renames.rs"),
            include_str!("renames/model.rs"),
            include_str!("renames/shape.rs"),
        ]
        .map(|source| layrs_lens_sdk::sha256_digest(source.replace("\r\n", "\n").as_bytes()));
        let policy = json!({
            "algorithm": "layrs.native-tree-merge.v1",
            "implementation": implementation,
            "textPin": pin(NativeLens::Text),
            "codePin": pin(NativeLens::Code),
            "enforcedHost":self.enforced_host,
            "renameAnalysis":renames::POLICY_ID,
            "maxSteps": self.limits.max_steps,
            "fileLimits": {
                "input":self.file_policy.limits.max_input_bytes,
                "output":self.file_policy.limits.max_output_bytes,
                "diffCells":self.file_policy.limits.max_diff_cells,
                "blocks":self.file_policy.limits.max_blocks,
                "segments":self.file_policy.limits.max_segments
            },
            "unsupported":format!("{:?}",self.file_policy.unsupported),
            "quota":format!("{:?}",self.file_policy.quota)
            ,"semanticExecution":self.semantic_execution
        });
        Sha256Id::parse(layrs_lens_sdk::sha256_digest(
            &canonical_json(&policy).map_err(invalid)?,
        ))
        .map_err(invalid)
    }

    pub fn receipt(
        &self,
        context: ReceiptContext,
        attestation: ReceiptHostAttestation,
    ) -> MergeResult<WeaveReceiptRecord> {
        self.receipt_with_profile(context, attestation.enforced_profile)
    }

    /// Zero-execution host. It never attests memory/deadline for the graph
    /// planner: those profile limits are exclusively Lens-invocation budgets,
    /// unused because any Lens execution or inherited Lens conflict rejects.
    pub fn structural_receipt(&self, context: ReceiptContext) -> MergeResult<WeaveReceiptRecord> {
        if !self.executions.is_empty()
            || self
                .conflicts
                .values()
                .any(|c| !c.document()["lensPin"].is_null())
        {
            return Err(GraphMergeError::Unsupported("Lens execution needs an actually enforced host; request explicit structural preview".into()));
        }
        self.receipt_with_native_host(context, "layrs.no-lens-execution", 1, 1, None)
    }

    /// Only a completed sandboxed planner can populate the private enforcement
    /// profile. Unsupervised native previews still reject semantic acceptance.
    pub fn accepted_receipt(&self, context: ReceiptContext) -> MergeResult<WeaveReceiptRecord> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(value) = &self.enforced_host {
            let host: crate::wasm_host::EnforcedProfile =
                serde_json::from_value(value.clone()).map_err(invalid)?;
            if host.module_digest != crate::wasm_host::module_digest()
                || self
                    .executions
                    .iter()
                    .any(|e| e.pin.digest != host.module_digest)
            {
                return Err(invalid("executions do not match the enforced WASM module"));
            }
            return self.receipt_with_native_host(
                context,
                crate::wasm_host::HOST_ID,
                host.limits.process.memory_bytes as u64,
                host.limits.process.deadline_ms as u64,
                Some(host.limits.fuel),
            );
        }
        self.structural_receipt(context)
    }

    fn receipt_with_native_host(
        &self,
        context: ReceiptContext,
        host_id: &str,
        memory_bytes: u64,
        deadline_ms: u64,
        fuel: Option<u64>,
    ) -> MergeResult<WeaveReceiptRecord> {
        let sha = self.execution_policy_digest()?;
        let unsupported = match self.file_policy.unsupported {
            FailureAction::Reject => "reject",
            FailureAction::StructuralFallback => "structural_fallback",
        };
        let mut value = json!({
            "canonicalization":{"id":"rfc8785","version":"RFC8785"},"contentHash":{"id":"blake3","version":"1"},
            "mergeAlgorithm":{"id":"layrs.native-tree-merge","version":"1"},"baseAlgorithm":{"id":"layrs.step-dag-bases","version":"1"},
            "hostPolicy":{"id":host_id,"version":"1","digest":sha,"ambientCapabilities":{"network":false,"wallClock":false,"randomness":false,"locale":false,"hostFilesystem":false,"environment":false}},
            "resolutionPolicy":{"id":"layrs.structural","version":"1","digest":sha,"allowedMethods":["select_term"],"automaticSideSelection":false,
                "absentSemantics":"distinct_from_empty_file","combineOrder":"term_id_ascii","manualInput":"forbidden","renameResolution":"forbidden",
                "onUnsupportedLens":unsupported,"onUnavailableLens":"reject","onTimeout":"reject","onQuotaExceeded":"reject","onTrap":"reject","onInvalidOutput":"reject","onInvalidSignature":"reject","onDigestMismatch":"reject"},
            "limits":{"maxMemoryBytes":memory_bytes,"maxExecutionMs":deadline_ms,"maxFuel":fuel,"maxInputBytes":self.limits.max_input_bytes,"maxOutputBytes":self.limits.max_output_bytes,
                "maxBlocks":self.file_policy.limits.max_blocks,"maxSegments":self.file_policy.limits.max_segments,"maxPaths":self.limits.max_paths,"maxRecursionDepth":self.limits.max_depth}
        });
        value["profileId"] = json!(digest("layrs-determinism-profile-v1", &value)?);
        self.receipt_with_profile(context, serde_json::from_value(value).map_err(invalid)?)
    }

    fn receipt_with_profile(
        &self,
        context: ReceiptContext,
        profile: DeterminismProfile,
    ) -> MergeResult<WeaveReceiptRecord> {
        for (id, conflict) in &self.conflicts {
            if !conflict.document()["lensPin"].is_null()
                && !self
                    .executions
                    .iter()
                    .any(|e| e.conflict.as_ref() == Some(id))
            {
                return Err(GraphMergeError::Unsupported("inherited Lens conflicts need a matching execution profile before receipt admission".into()));
            }
        }
        let mut profile_value = serde_json::to_value(&profile).map_err(invalid)?;
        profile_value.as_object_mut().unwrap().remove("profileId");
        if digest("layrs-determinism-profile-v1", &profile_value)? != profile.profile_id {
            return Err(invalid("profileId does not match its identity projection"));
        }
        if profile.merge_algorithm.id != "layrs.native-tree-merge"
            || profile.merge_algorithm.version != "1"
            || profile.base_algorithm.id != "layrs.step-dag-bases"
            || profile.base_algorithm.version != "1"
            || profile.host_policy.digest != self.execution_policy_digest()?
        {
            return Err(invalid(
                "profile does not describe the executed native algorithms",
            ));
        }
        let ambient = &profile.host_policy.ambient_capabilities;
        if ambient.network
            || ambient.wall_clock
            || ambient.randomness
            || ambient.locale
            || ambient.host_filesystem
            || ambient.environment
        {
            return Err(invalid("native merge has no ambient capabilities"));
        }
        let limits = &profile.limits;
        if limits.max_input_bytes != self.limits.max_input_bytes as u64
            || limits.max_output_bytes != self.limits.max_output_bytes as u64
            || limits.max_paths != self.limits.max_paths as u64
            || limits.max_recursion_depth != self.limits.max_depth as u64
            || limits.max_blocks != self.file_policy.limits.max_blocks as u64
            || limits.max_segments != self.file_policy.limits.max_segments as u64
            || limits.max_memory_bytes == 0
            || limits.max_execution_ms == 0
            || limits.max_fuel.as_ref().is_some_and(|value| *value == 0)
        {
            return Err(invalid("attested limits differ from the plan"));
        }
        // The canonical profile has one quota failure action. This planner has
        // mandatory graph quota rejection, so only a uniformly strict quota
        // policy can presently be represented without misreporting execution.
        let resolution = &profile.resolution_policy;
        let unsupported = match self.file_policy.unsupported {
            FailureAction::Reject => ResolutionFailureAction::Reject,
            FailureAction::StructuralFallback => ResolutionFailureAction::StructuralFallback,
        };
        if self.file_policy.quota != FailureAction::Reject
            || resolution.on_quota_exceeded != ResolutionFailureAction::Reject
            || resolution.on_unsupported_lens != unsupported
            || resolution.on_invalid_output != ResolutionFailureAction::Reject
            || resolution.on_invalid_signature != ResolutionFailureAction::Reject
            || resolution.on_digest_mismatch != ResolutionFailureAction::Reject
            || resolution.automatic_side_selection
            || resolution.allowed_methods != [ResolutionMethod::SelectTerm]
            || resolution.manual_input != PermissionToggle::Forbidden
            || resolution.rename_resolution != PermissionToggle::Forbidden
        {
            return Err(invalid(
                "resolution profile differs from implemented structural choices",
            ));
        }
        let mut executions = vec![];
        for pending in &self.executions {
            let mut execution = LensExecution {
                execution_id: profile.profile_id.clone(),
                path: pending.path.clone(),
                pin: LensPin {
                    lens_id: pending.pin.lens_id.clone(),
                    version: pending.pin.version.clone(),
                    digest: Sha256Id::parse(&pending.pin.digest).map_err(invalid)?,
                },
                implementation_digest: Sha256Id::parse(&pending.pin.digest).map_err(invalid)?,
                determinism_profile_id: profile.profile_id.clone(),
                term_roles: pending.term_roles.clone(),
                result: if pending.conflict.is_some() {
                    LensExecutionResult::StructuralFallback
                } else {
                    LensExecutionResult::AutoResolved
                },
                conflict_id: Nullable(pending.conflict.clone()),
            };
            let mut value = serde_json::to_value(&execution).map_err(invalid)?;
            value.as_object_mut().unwrap().remove("executionId");
            execution.execution_id = digest("layrs-lens-execution-v1", &value)?;
            executions.push(execution);
        }
        executions.sort_by(|a, b| a.execution_id.cmp(&b.execution_id));
        let mut receipt = WeaveReceiptRecord {
            schema_version: WeaveReceiptSchemaVersion::V1,
            weave_id: profile.profile_id.clone(),
            space_id: context.space_id,
            source_heads: self.source_heads.clone(),
            target_heads_before: self.target_heads.clone(),
            merge_bases: self
                .merge_bases
                .iter()
                .map(|step| {
                    Ok(MergeBase::Step {
                        base_id: ObjectId::parse(step.as_str()).map_err(invalid)?,
                        step_id: step.clone(),
                    })
                })
                .collect::<MergeResult<Vec<_>>>()?,
            result_step_id: self.step.id().clone(),
            outcome: if self.conflicts.is_empty() {
                WeaveOutcome::Clean
            } else {
                WeaveOutcome::Conflicted
            },
            conflict_ids: self.conflicts.keys().cloned().collect(),
            lens_executions: executions,
            determinism_profile: profile,
            operation_id: context.operation_id,
            gate_snapshot_id: Nullable(context.gate_snapshot_id),
            created_at: context.created_at,
        };
        let mut value = serde_json::to_value(&receipt).map_err(invalid)?;
        value.as_object_mut().unwrap().remove("weaveId");
        receipt.weave_id = digest("layrs-weave-receipt-v1", &value)?;
        Ok(receipt)
    }
}

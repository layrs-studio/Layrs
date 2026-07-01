use layrs_gates::{GateBlockerKind, GateEvaluation};

pub type WeaveId = String;
pub type LayerId = String;
pub type StepId = String;
pub type ActorId = String;
pub type ArtifactId = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeaveRequest {
    pub id: WeaveId,
    pub source_layer_id: LayerId,
    pub target_layer_id: LayerId,
    pub requested_by: ActorId,
    pub source_steps: Vec<SourceLayerStep>,
    pub gate_evaluation: Option<GateEvaluation>,
}

impl WeaveRequest {
    pub fn new(
        id: impl Into<WeaveId>,
        source_layer_id: impl Into<LayerId>,
        target_layer_id: impl Into<LayerId>,
        requested_by: impl Into<ActorId>,
        source_steps: Vec<SourceLayerStep>,
    ) -> Self {
        Self {
            id: id.into(),
            source_layer_id: source_layer_id.into(),
            target_layer_id: target_layer_id.into(),
            requested_by: requested_by.into(),
            source_steps,
            gate_evaluation: None,
        }
    }

    pub fn with_gate_evaluation(mut self, gate_evaluation: GateEvaluation) -> Self {
        self.gate_evaluation = Some(gate_evaluation);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLayerStep {
    pub id: StepId,
    pub ordinal: u64,
    pub summary: String,
    pub artifact_ids: Vec<ArtifactId>,
}

impl SourceLayerStep {
    pub fn new(id: impl Into<StepId>, ordinal: u64, summary: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ordinal,
            summary: summary.into(),
            artifact_ids: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetLayerStep {
    pub id: StepId,
    pub ordinal: u64,
    pub summary: String,
    pub artifact_ids: Vec<ArtifactId>,
    pub provenance: StepProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepProvenance {
    pub weave_request_id: WeaveId,
    pub source_layer_id: LayerId,
    pub target_layer_id: LayerId,
    pub source_step_id: StepId,
    pub source_step_ordinal: u64,
    pub woven_at_epoch_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeavePlan {
    pub request_id: WeaveId,
    pub source_layer_id: LayerId,
    pub target_layer_id: LayerId,
    pub append_steps: Vec<TargetLayerStep>,
    pub conflicts: Vec<WeaveConflict>,
}

impl WeavePlan {
    pub fn is_blocked(&self) -> bool {
        !self.conflicts.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WeaveConflict {
    SameLayer {
        layer_id: LayerId,
    },
    MissingSourceSteps,
    StepAlreadyWoven {
        source_step_id: StepId,
    },
    GateBlocked {
        gate_id: String,
        blockers: Vec<WeaveGateBlocker>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeaveGateBlocker {
    pub requirement_id: String,
    pub kind: GateBlockerKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeaveOutcome {
    pub request_id: WeaveId,
    pub status: WeaveStatus,
    pub appended_steps: Vec<TargetLayerStep>,
    pub conflicts: Vec<WeaveConflict>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeaveStatus {
    Applied,
    Blocked,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WeavePlanner;

impl WeavePlanner {
    pub fn plan(
        request: &WeaveRequest,
        existing_target_steps: &[TargetLayerStep],
        now_epoch_secs: u64,
    ) -> WeavePlan {
        let mut conflicts = Vec::new();

        if request.source_layer_id == request.target_layer_id {
            conflicts.push(WeaveConflict::SameLayer {
                layer_id: request.source_layer_id.clone(),
            });
        }

        if request.source_steps.is_empty() {
            conflicts.push(WeaveConflict::MissingSourceSteps);
        }

        if let Some(gate_evaluation) = &request.gate_evaluation {
            if !gate_evaluation.allows_weave() {
                conflicts.push(WeaveConflict::GateBlocked {
                    gate_id: gate_evaluation.gate_id.clone(),
                    blockers: gate_evaluation
                        .blockers
                        .iter()
                        .map(|blocker| WeaveGateBlocker {
                            requirement_id: blocker.requirement_id.clone(),
                            kind: blocker.kind,
                            message: blocker.message.clone(),
                        })
                        .collect(),
                });
            }
        }

        for source_step in &request.source_steps {
            if existing_target_steps.iter().any(|target_step| {
                target_step.provenance.source_layer_id == request.source_layer_id
                    && target_step.provenance.source_step_id == source_step.id
            }) {
                conflicts.push(WeaveConflict::StepAlreadyWoven {
                    source_step_id: source_step.id.clone(),
                });
            }
        }

        let next_ordinal = existing_target_steps
            .iter()
            .map(|step| step.ordinal)
            .max()
            .map_or(0, |ordinal| ordinal.saturating_add(1));

        let append_steps = if conflicts.is_empty() {
            request
                .source_steps
                .iter()
                .enumerate()
                .map(|(offset, source_step)| TargetLayerStep {
                    id: format!("{}:{}", request.id, source_step.id),
                    ordinal: next_ordinal.saturating_add(offset as u64),
                    summary: source_step.summary.clone(),
                    artifact_ids: source_step.artifact_ids.clone(),
                    provenance: StepProvenance {
                        weave_request_id: request.id.clone(),
                        source_layer_id: request.source_layer_id.clone(),
                        target_layer_id: request.target_layer_id.clone(),
                        source_step_id: source_step.id.clone(),
                        source_step_ordinal: source_step.ordinal,
                        woven_at_epoch_secs: now_epoch_secs,
                    },
                })
                .collect()
        } else {
            Vec::new()
        };

        WeavePlan {
            request_id: request.id.clone(),
            source_layer_id: request.source_layer_id.clone(),
            target_layer_id: request.target_layer_id.clone(),
            append_steps,
            conflicts,
        }
    }

    pub fn apply(plan: WeavePlan) -> WeaveOutcome {
        let status = if plan.is_blocked() {
            WeaveStatus::Blocked
        } else {
            WeaveStatus::Applied
        };

        WeaveOutcome {
            request_id: plan.request_id,
            status,
            appended_steps: if status == WeaveStatus::Applied {
                plan.append_steps
            } else {
                Vec::new()
            },
            conflicts: plan.conflicts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target_step_from_source(source_layer_id: &str, source_step_id: &str) -> TargetLayerStep {
        TargetLayerStep {
            id: "target-existing-1".to_string(),
            ordinal: 0,
            summary: "existing woven step".to_string(),
            artifact_ids: Vec::new(),
            provenance: StepProvenance {
                weave_request_id: "weave-old".to_string(),
                source_layer_id: source_layer_id.to_string(),
                target_layer_id: "target".to_string(),
                source_step_id: source_step_id.to_string(),
                source_step_ordinal: 0,
                woven_at_epoch_secs: 100,
            },
        }
    }

    #[test]
    fn weave_appends_source_steps_with_provenance() {
        let request = WeaveRequest::new(
            "weave-1",
            "source",
            "target",
            "user-1",
            vec![SourceLayerStep::new("step-1", 7, "copy source step")],
        );

        let plan = WeavePlanner::plan(&request, &[], 200);

        assert!(!plan.is_blocked());
        assert_eq!(plan.append_steps.len(), 1);
        assert_eq!(plan.append_steps[0].ordinal, 0);
        assert_eq!(plan.append_steps[0].provenance.source_layer_id, "source");
        assert_eq!(plan.append_steps[0].provenance.source_step_id, "step-1");
        assert_eq!(plan.append_steps[0].provenance.source_step_ordinal, 7);
    }

    #[test]
    fn weave_blocked_by_conflict() {
        let request = WeaveRequest::new(
            "weave-1",
            "source",
            "target",
            "user-1",
            vec![SourceLayerStep::new("step-1", 0, "copy source step")],
        );
        let existing = vec![target_step_from_source("source", "step-1")];

        let plan = WeavePlanner::plan(&request, &existing, 200);
        let outcome = WeavePlanner::apply(plan.clone());

        assert!(plan.is_blocked());
        assert_eq!(
            plan.conflicts,
            vec![WeaveConflict::StepAlreadyWoven {
                source_step_id: "step-1".to_string()
            }]
        );
        assert_eq!(outcome.status, WeaveStatus::Blocked);
        assert!(outcome.appended_steps.is_empty());
    }
}

use layrs_proof::{Proof, ProofRecipeId, ProofSubjectId, ProofTrustLevel, ProofVerdict};

pub type GateId = String;
pub type GateRequirementId = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gate {
    pub id: GateId,
    pub name: String,
    pub target: GateTarget,
    pub requirements: Vec<GateRequirement>,
}

impl Gate {
    pub fn new(
        id: impl Into<GateId>,
        name: impl Into<String>,
        target: GateTarget,
        requirements: Vec<GateRequirement>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            target,
            requirements,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateTarget {
    Workspace(String),
    Space(String),
    Layer(String),
    Artifact(String),
    Weave(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateRequirement {
    pub id: GateRequirementId,
    pub recipe_id: ProofRecipeId,
    pub subject_id: Option<ProofSubjectId>,
    pub required_verdict: ProofVerdict,
    pub minimum_trust_level: ProofTrustLevel,
    pub max_age_secs: Option<u64>,
}

impl GateRequirement {
    pub fn proof(recipe_id: impl Into<ProofRecipeId>) -> Self {
        let recipe_id = recipe_id.into();

        Self {
            id: recipe_id.clone(),
            recipe_id,
            subject_id: None,
            required_verdict: ProofVerdict::Passed,
            minimum_trust_level: ProofTrustLevel::LocalAutomation,
            max_age_secs: None,
        }
    }

    pub fn for_subject(mut self, subject_id: impl Into<ProofSubjectId>) -> Self {
        self.subject_id = Some(subject_id.into());
        self
    }

    pub fn with_max_age_secs(mut self, max_age_secs: u64) -> Self {
        self.max_age_secs = Some(max_age_secs);
        self
    }

    pub fn with_minimum_trust_level(mut self, trust_level: ProofTrustLevel) -> Self {
        self.minimum_trust_level = trust_level;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateStatus {
    Passed,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateEvaluation {
    pub gate_id: GateId,
    pub status: GateStatus,
    pub requirement_results: Vec<GateRequirementEvaluation>,
    pub blockers: Vec<GateBlocker>,
}

impl GateEvaluation {
    pub fn allows_weave(&self) -> bool {
        self.status == GateStatus::Passed
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateRequirementEvaluation {
    pub requirement_id: GateRequirementId,
    pub status: GateRequirementStatus,
    pub proof_id: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateRequirementStatus {
    Satisfied,
    MissingProof,
    StaleProof,
    FailedProof,
    InsufficientTrust,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateBlocker {
    pub requirement_id: GateRequirementId,
    pub kind: GateBlockerKind,
    pub proof_id: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateBlockerKind {
    MissingProof,
    StaleProof,
    FailedProof,
    InsufficientTrust,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct GateEvaluator;

impl GateEvaluator {
    pub fn evaluate(gate: &Gate, proofs: &[Proof], now_epoch_secs: u64) -> GateEvaluation {
        let mut requirement_results = Vec::with_capacity(gate.requirements.len());
        let mut blockers = Vec::new();

        for requirement in &gate.requirements {
            let result = evaluate_requirement(requirement, proofs, now_epoch_secs);

            if result.status != GateRequirementStatus::Satisfied {
                blockers.push(GateBlocker {
                    requirement_id: requirement.id.clone(),
                    kind: blocker_kind_for_status(result.status),
                    proof_id: result.proof_id.clone(),
                    message: result.reason.clone(),
                });
            }

            requirement_results.push(result);
        }

        let status = if blockers.is_empty() {
            GateStatus::Passed
        } else {
            GateStatus::Blocked
        };

        GateEvaluation {
            gate_id: gate.id.clone(),
            status,
            requirement_results,
            blockers,
        }
    }
}

fn evaluate_requirement(
    requirement: &GateRequirement,
    proofs: &[Proof],
    now_epoch_secs: u64,
) -> GateRequirementEvaluation {
    let selected_proof = proofs
        .iter()
        .filter(|proof| proof.recipe_id == requirement.recipe_id)
        .filter(|proof| {
            requirement
                .subject_id
                .as_ref()
                .is_none_or(|subject_id| proof.subject_id == *subject_id)
        })
        .max_by_key(|proof| proof.produced_at_epoch_secs);

    let Some(proof) = selected_proof else {
        return GateRequirementEvaluation {
            requirement_id: requirement.id.clone(),
            status: GateRequirementStatus::MissingProof,
            proof_id: None,
            reason: "missing proof".to_string(),
        };
    };

    if proof.verdict != requirement.required_verdict {
        return GateRequirementEvaluation {
            requirement_id: requirement.id.clone(),
            status: GateRequirementStatus::FailedProof,
            proof_id: Some(proof.id.clone()),
            reason: "proof verdict does not satisfy requirement".to_string(),
        };
    }

    if proof.is_stale(now_epoch_secs, requirement.max_age_secs) {
        return GateRequirementEvaluation {
            requirement_id: requirement.id.clone(),
            status: GateRequirementStatus::StaleProof,
            proof_id: Some(proof.id.clone()),
            reason: "proof is stale".to_string(),
        };
    }

    if !proof.satisfies_trust(requirement.minimum_trust_level) {
        return GateRequirementEvaluation {
            requirement_id: requirement.id.clone(),
            status: GateRequirementStatus::InsufficientTrust,
            proof_id: Some(proof.id.clone()),
            reason: "proof trust level is too low".to_string(),
        };
    }

    GateRequirementEvaluation {
        requirement_id: requirement.id.clone(),
        status: GateRequirementStatus::Satisfied,
        proof_id: Some(proof.id.clone()),
        reason: "proof satisfies requirement".to_string(),
    }
}

fn blocker_kind_for_status(status: GateRequirementStatus) -> GateBlockerKind {
    match status {
        GateRequirementStatus::Satisfied => unreachable!("satisfied requirements are not blockers"),
        GateRequirementStatus::MissingProof => GateBlockerKind::MissingProof,
        GateRequirementStatus::StaleProof => GateBlockerKind::StaleProof,
        GateRequirementStatus::FailedProof => GateBlockerKind::FailedProof,
        GateRequirementStatus::InsufficientTrust => GateBlockerKind::InsufficientTrust,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate(requirement: GateRequirement) -> Gate {
        Gate::new(
            "gate-test",
            "test gate",
            GateTarget::Layer("layer-1".to_string()),
            vec![requirement],
        )
    }

    #[test]
    fn gate_blocks_missing_proof() {
        let evaluation =
            GateEvaluator::evaluate(&gate(GateRequirement::proof("recipe-test")), &[], 100);

        assert_eq!(evaluation.status, GateStatus::Blocked);
        assert_eq!(evaluation.blockers[0].kind, GateBlockerKind::MissingProof);
    }

    #[test]
    fn gate_blocks_stale_proof() {
        let proof = Proof::new(
            "proof-1",
            "recipe-test",
            "layer-1",
            ProofVerdict::Passed,
            ProofTrustLevel::VerifiedAutomation,
            100,
        );
        let requirement = GateRequirement::proof("recipe-test").with_max_age_secs(50);

        let evaluation = GateEvaluator::evaluate(&gate(requirement), &[proof], 151);

        assert_eq!(evaluation.status, GateStatus::Blocked);
        assert_eq!(evaluation.blockers[0].kind, GateBlockerKind::StaleProof);
    }

    #[test]
    fn gate_blocks_failed_proof() {
        let proof = Proof::new(
            "proof-1",
            "recipe-test",
            "layer-1",
            ProofVerdict::Failed,
            ProofTrustLevel::VerifiedAutomation,
            100,
        );

        let evaluation =
            GateEvaluator::evaluate(&gate(GateRequirement::proof("recipe-test")), &[proof], 100);

        assert_eq!(evaluation.status, GateStatus::Blocked);
        assert_eq!(evaluation.blockers[0].kind, GateBlockerKind::FailedProof);
    }
}

pub type ProofId = String;
pub type ProofRecipeId = String;
pub type ProofSubjectId = String;
pub type TraceId = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofRecipe {
    pub id: ProofRecipeId,
    pub name: String,
    pub description: Option<String>,
    pub trigger: ProofTrigger,
    pub minimum_trust_level: ProofTrustLevel,
}

impl ProofRecipe {
    pub fn new(
        id: impl Into<ProofRecipeId>,
        name: impl Into<String>,
        trigger: ProofTrigger,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
            trigger,
            minimum_trust_level: ProofTrustLevel::LocalAutomation,
        }
    }

    pub fn with_minimum_trust_level(mut self, trust_level: ProofTrustLevel) -> Self {
        self.minimum_trust_level = trust_level;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proof {
    pub id: ProofId,
    pub recipe_id: ProofRecipeId,
    pub subject_id: ProofSubjectId,
    pub trigger: ProofTrigger,
    pub verdict: ProofVerdict,
    pub trust_level: ProofTrustLevel,
    pub produced_at_epoch_secs: u64,
    pub expires_at_epoch_secs: Option<u64>,
    pub trace: Option<ProofTraceRef>,
}

impl Proof {
    pub fn new(
        id: impl Into<ProofId>,
        recipe_id: impl Into<ProofRecipeId>,
        subject_id: impl Into<ProofSubjectId>,
        verdict: ProofVerdict,
        trust_level: ProofTrustLevel,
        produced_at_epoch_secs: u64,
    ) -> Self {
        Self {
            id: id.into(),
            recipe_id: recipe_id.into(),
            subject_id: subject_id.into(),
            trigger: ProofTrigger::Manual {
                actor_id: "unknown".to_string(),
            },
            verdict,
            trust_level,
            produced_at_epoch_secs,
            expires_at_epoch_secs: None,
            trace: None,
        }
    }

    pub fn with_trigger(mut self, trigger: ProofTrigger) -> Self {
        self.trigger = trigger;
        self
    }

    pub fn with_expiry(mut self, expires_at_epoch_secs: u64) -> Self {
        self.expires_at_epoch_secs = Some(expires_at_epoch_secs);
        self
    }

    pub fn with_trace(mut self, trace: ProofTraceRef) -> Self {
        self.trace = Some(trace);
        self
    }

    pub fn is_stale(&self, now_epoch_secs: u64, max_age_secs: Option<u64>) -> bool {
        if self
            .expires_at_epoch_secs
            .is_some_and(|expires_at| expires_at <= now_epoch_secs)
        {
            return true;
        }

        max_age_secs.is_some_and(|max_age| {
            self.produced_at_epoch_secs.saturating_add(max_age) <= now_epoch_secs
        })
    }

    pub fn satisfies_trust(&self, minimum_trust_level: ProofTrustLevel) -> bool {
        self.trust_level >= minimum_trust_level
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofTraceRef {
    pub trace_id: TraceId,
    pub uri: Option<String>,
    pub digest: Option<String>,
}

impl ProofTraceRef {
    pub fn new(trace_id: impl Into<TraceId>) -> Self {
        Self {
            trace_id: trace_id.into(),
            uri: None,
            digest: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProofTrigger {
    Manual {
        actor_id: String,
    },
    Step {
        step_id: String,
    },
    Gate {
        gate_id: String,
    },
    Policy {
        policy_id: String,
    },
    External {
        system: String,
        reference: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProofTrustLevel {
    Untrusted,
    HumanAttested,
    LocalAutomation,
    VerifiedAutomation,
    TrustedService,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofVerdict {
    Passed,
    Failed,
    Inconclusive,
    Skipped,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proof_expires_when_expiry_is_reached() {
        let proof = Proof::new(
            "proof-1",
            "recipe-test",
            "layer-target",
            ProofVerdict::Passed,
            ProofTrustLevel::VerifiedAutomation,
            100,
        )
        .with_expiry(200);

        assert!(proof.is_stale(200, None));
    }

    #[test]
    fn trust_levels_are_ordered_from_light_to_strong() {
        let proof = Proof::new(
            "proof-1",
            "recipe-test",
            "layer-target",
            ProofVerdict::Passed,
            ProofTrustLevel::VerifiedAutomation,
            100,
        );

        assert!(proof.satisfies_trust(ProofTrustLevel::LocalAutomation));
        assert!(!proof.satisfies_trust(ProofTrustLevel::TrustedService));
    }
}

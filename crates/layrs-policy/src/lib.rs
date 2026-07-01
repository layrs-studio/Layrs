pub type PolicyId = String;
pub type WorkspaceId = String;
pub type TeamId = String;
pub type SpaceId = String;
pub type LayerId = String;
pub type ArtifactId = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub id: PolicyId,
    pub subject: PolicySubject,
    pub action: PolicyAction,
    pub scope: PolicyScope,
    pub effect: PolicyEffect,
}

impl Policy {
    pub fn allow(
        id: impl Into<PolicyId>,
        subject: PolicySubject,
        action: PolicyAction,
        scope: PolicyScope,
    ) -> Self {
        Self {
            id: id.into(),
            subject,
            action,
            scope,
            effect: PolicyEffect::Allow,
        }
    }

    pub fn deny(
        id: impl Into<PolicyId>,
        subject: PolicySubject,
        action: PolicyAction,
        scope: PolicyScope,
    ) -> Self {
        Self {
            id: id.into(),
            subject,
            action,
            scope,
            effect: PolicyEffect::Deny,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicySubject {
    Any,
    User(String),
    Team(TeamId),
    ServiceAccount(String),
    Step(String),
}

impl PolicySubject {
    fn matches(&self, request: &PolicyEvaluationRequest) -> bool {
        match self {
            Self::Any => true,
            Self::Team(team_id) => {
                request.subject == Self::Team(team_id.clone())
                    || request.team_ids.iter().any(|id| id == team_id)
            }
            subject => subject == &request.subject,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyAction {
    Any,
    Read,
    Write,
    Administer,
    EvaluateGate,
    ProduceProof,
    WeaveLayer,
    ManagePolicy,
    Custom(String),
}

impl PolicyAction {
    fn matches(&self, requested: &Self) -> bool {
        self == &Self::Any || self == requested
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyEffect {
    Allow,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyScope {
    Workspace {
        workspace_id: WorkspaceId,
    },
    Team {
        workspace_id: WorkspaceId,
        team_id: TeamId,
    },
    Space {
        workspace_id: WorkspaceId,
        space_id: SpaceId,
    },
    Layer {
        workspace_id: WorkspaceId,
        space_id: SpaceId,
        layer_id: LayerId,
    },
    Artifact {
        workspace_id: WorkspaceId,
        space_id: SpaceId,
        layer_id: Option<LayerId>,
        artifact_id: ArtifactId,
    },
}

impl PolicyScope {
    fn precedence_rank(&self) -> u8 {
        match self {
            Self::Workspace { .. } => 0,
            Self::Team { .. } | Self::Space { .. } => 1,
            Self::Layer { .. } => 2,
            Self::Artifact { .. } => 3,
        }
    }

    fn applies_to(&self, request: &PolicyEvaluationRequest) -> bool {
        match self {
            Self::Workspace { workspace_id } => request.target.workspace_id() == workspace_id,
            Self::Team {
                workspace_id,
                team_id,
            } => {
                request.target.workspace_id() == workspace_id
                    && (request.subject == PolicySubject::Team(team_id.clone())
                        || request.team_ids.iter().any(|id| id == team_id))
            }
            Self::Space {
                workspace_id,
                space_id,
            } => {
                request.target.workspace_id() == workspace_id
                    && request.target.space_id().is_some_and(|id| id == space_id)
            }
            Self::Layer {
                workspace_id,
                space_id,
                layer_id,
            } => {
                request.target.workspace_id() == workspace_id
                    && request.target.space_id().is_some_and(|id| id == space_id)
                    && request.target.layer_id().is_some_and(|id| id == layer_id)
            }
            Self::Artifact {
                workspace_id,
                space_id,
                layer_id,
                artifact_id,
            } => {
                request.target.workspace_id() == workspace_id
                    && request.target.space_id().is_some_and(|id| id == space_id)
                    && layer_id.as_ref().is_none_or(|required| {
                        request.target.layer_id().is_some_and(|id| id == required)
                    })
                    && request
                        .target
                        .artifact_id()
                        .is_some_and(|id| id == artifact_id)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyTarget {
    Workspace {
        workspace_id: WorkspaceId,
    },
    Space {
        workspace_id: WorkspaceId,
        space_id: SpaceId,
    },
    Layer {
        workspace_id: WorkspaceId,
        space_id: SpaceId,
        layer_id: LayerId,
    },
    Artifact {
        workspace_id: WorkspaceId,
        space_id: SpaceId,
        layer_id: Option<LayerId>,
        artifact_id: ArtifactId,
    },
}

impl PolicyTarget {
    fn workspace_id(&self) -> &WorkspaceId {
        match self {
            Self::Workspace { workspace_id }
            | Self::Space { workspace_id, .. }
            | Self::Layer { workspace_id, .. }
            | Self::Artifact { workspace_id, .. } => workspace_id,
        }
    }

    fn space_id(&self) -> Option<&SpaceId> {
        match self {
            Self::Workspace { .. } => None,
            Self::Space { space_id, .. }
            | Self::Layer { space_id, .. }
            | Self::Artifact { space_id, .. } => Some(space_id),
        }
    }

    fn layer_id(&self) -> Option<&LayerId> {
        match self {
            Self::Workspace { .. } | Self::Space { .. } => None,
            Self::Layer { layer_id, .. } => Some(layer_id),
            Self::Artifact { layer_id, .. } => layer_id.as_ref(),
        }
    }

    fn artifact_id(&self) -> Option<&ArtifactId> {
        match self {
            Self::Artifact { artifact_id, .. } => Some(artifact_id),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyEvaluationRequest {
    pub subject: PolicySubject,
    pub team_ids: Vec<TeamId>,
    pub action: PolicyAction,
    pub target: PolicyTarget,
}

impl PolicyEvaluationRequest {
    pub fn new(subject: PolicySubject, action: PolicyAction, target: PolicyTarget) -> Self {
        Self {
            subject,
            team_ids: Vec::new(),
            action,
            target,
        }
    }

    pub fn with_team(mut self, team_id: impl Into<TeamId>) -> Self {
        self.team_ids.push(team_id.into());
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyVerdict {
    Allow,
    Deny,
    Abstain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyDecision {
    pub verdict: PolicyVerdict,
    pub decisive_policy_id: Option<PolicyId>,
    pub matched_policy_ids: Vec<PolicyId>,
    pub denied_by_policy_ids: Vec<PolicyId>,
    pub reason: String,
}

impl PolicyDecision {
    pub fn is_allowed(&self) -> bool {
        self.verdict == PolicyVerdict::Allow
    }

    pub fn is_denied(&self) -> bool {
        self.verdict == PolicyVerdict::Deny
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicyEvaluator {
    policies: Vec<Policy>,
}

impl PolicyEvaluator {
    pub fn new(policies: Vec<Policy>) -> Self {
        Self { policies }
    }

    pub fn policies(&self) -> &[Policy] {
        &self.policies
    }

    pub fn evaluate(&self, request: &PolicyEvaluationRequest) -> PolicyDecision {
        let matching: Vec<(usize, &Policy)> = self
            .policies
            .iter()
            .enumerate()
            .filter(|(_, policy)| {
                policy.subject.matches(request)
                    && policy.action.matches(&request.action)
                    && policy.scope.applies_to(request)
            })
            .collect();

        let matched_policy_ids = matching
            .iter()
            .map(|(_, policy)| policy.id.clone())
            .collect::<Vec<_>>();

        let denied_by_policy_ids = matching
            .iter()
            .filter(|(_, policy)| policy.effect == PolicyEffect::Deny)
            .map(|(_, policy)| policy.id.clone())
            .collect::<Vec<_>>();

        if !denied_by_policy_ids.is_empty() {
            let decisive_policy_id = matching
                .iter()
                .filter(|(_, policy)| policy.effect == PolicyEffect::Deny)
                .max_by_key(|(index, policy)| (policy.scope.precedence_rank(), *index))
                .map(|(_, policy)| policy.id.clone());

            return PolicyDecision {
                verdict: PolicyVerdict::Deny,
                decisive_policy_id,
                matched_policy_ids,
                denied_by_policy_ids,
                reason: "explicit deny matched".to_string(),
            };
        }

        if let Some((_, policy)) = matching
            .iter()
            .filter(|(_, policy)| policy.effect == PolicyEffect::Allow)
            .max_by_key(|(index, policy)| (policy.scope.precedence_rank(), *index))
        {
            return PolicyDecision {
                verdict: PolicyVerdict::Allow,
                decisive_policy_id: Some(policy.id.clone()),
                matched_policy_ids,
                denied_by_policy_ids,
                reason: "most specific allow matched".to_string(),
            };
        }

        PolicyDecision {
            verdict: PolicyVerdict::Abstain,
            decisive_policy_id: None,
            matched_policy_ids,
            denied_by_policy_ids,
            reason: "no policy matched".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact_target() -> PolicyTarget {
        PolicyTarget::Artifact {
            workspace_id: "workspace-1".to_string(),
            space_id: "space-1".to_string(),
            layer_id: Some("layer-1".to_string()),
            artifact_id: "artifact-1".to_string(),
        }
    }

    #[test]
    fn policy_precedence_prefers_artifact_specific_allow() {
        let evaluator = PolicyEvaluator::new(vec![
            Policy::allow(
                "workspace-allow",
                PolicySubject::Any,
                PolicyAction::Read,
                PolicyScope::Workspace {
                    workspace_id: "workspace-1".to_string(),
                },
            ),
            Policy::allow(
                "layer-allow",
                PolicySubject::Any,
                PolicyAction::Read,
                PolicyScope::Layer {
                    workspace_id: "workspace-1".to_string(),
                    space_id: "space-1".to_string(),
                    layer_id: "layer-1".to_string(),
                },
            ),
            Policy::allow(
                "artifact-allow",
                PolicySubject::Any,
                PolicyAction::Read,
                PolicyScope::Artifact {
                    workspace_id: "workspace-1".to_string(),
                    space_id: "space-1".to_string(),
                    layer_id: Some("layer-1".to_string()),
                    artifact_id: "artifact-1".to_string(),
                },
            ),
        ]);

        let decision = evaluator.evaluate(&PolicyEvaluationRequest::new(
            PolicySubject::User("user-1".to_string()),
            PolicyAction::Read,
            artifact_target(),
        ));

        assert!(decision.is_allowed());
        assert_eq!(
            decision.decisive_policy_id,
            Some("artifact-allow".to_string())
        );
    }

    #[test]
    fn explicit_deny_wins_over_more_specific_allow() {
        let evaluator = PolicyEvaluator::new(vec![
            Policy::deny(
                "workspace-deny",
                PolicySubject::Any,
                PolicyAction::Write,
                PolicyScope::Workspace {
                    workspace_id: "workspace-1".to_string(),
                },
            ),
            Policy::allow(
                "artifact-allow",
                PolicySubject::Any,
                PolicyAction::Write,
                PolicyScope::Artifact {
                    workspace_id: "workspace-1".to_string(),
                    space_id: "space-1".to_string(),
                    layer_id: Some("layer-1".to_string()),
                    artifact_id: "artifact-1".to_string(),
                },
            ),
        ]);

        let decision = evaluator.evaluate(&PolicyEvaluationRequest::new(
            PolicySubject::User("user-1".to_string()),
            PolicyAction::Write,
            artifact_target(),
        ));

        assert!(decision.is_denied());
        assert_eq!(
            decision.denied_by_policy_ids,
            vec!["workspace-deny".to_string()]
        );
    }
}

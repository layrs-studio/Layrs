use crate::ids::{PolicyId, PrincipalId, TeamId, WorkspaceId};
use crate::validation::{ApiResult, Validate, bounded_len, max_items};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateTeamRequest {
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub member_principal_ids: Vec<PrincipalId>,
    pub policy_ids: Vec<PolicyId>,
}

impl Validate for CreateTeamRequest {
    fn validate(&self) -> ApiResult<()> {
        self.workspace_id.validate_field("workspace_id")?;
        bounded_len("name", &self.name, 2, 128)?;
        max_items("member_principal_ids", &self.member_principal_ids, 512)?;
        max_items("policy_ids", &self.policy_ids, 128)?;
        for principal_id in &self.member_principal_ids {
            principal_id.validate_field("member_principal_id")?;
        }
        for policy_id in &self.policy_ids {
            policy_id.validate_field("policy_id")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TeamMembershipRequest {
    pub workspace_id: WorkspaceId,
    pub team_id: TeamId,
    pub principal_id: PrincipalId,
}

impl Validate for TeamMembershipRequest {
    fn validate(&self) -> ApiResult<()> {
        self.workspace_id.validate_field("workspace_id")?;
        self.team_id.validate_field("team_id")?;
        self.principal_id.validate_field("principal_id")?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TeamResponse {
    pub workspace_id: WorkspaceId,
    pub team_id: TeamId,
    pub name: String,
    pub member_principal_ids: Vec<PrincipalId>,
    pub policy_ids: Vec<PolicyId>,
    pub created_at: String,
    pub updated_at: String,
}

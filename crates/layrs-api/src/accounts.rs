use crate::ids::{AccountId, MembershipId, PrincipalId, TeamId, WorkspaceId};
use crate::validation::{ApiResult, Validate, bounded_len, max_items, required};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum AccountStatus {
    Active,
    Suspended,
    Deleted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateAccountRequest {
    pub display_name: String,
    pub email: String,
    pub external_subject: Option<String>,
}

impl Validate for CreateAccountRequest {
    fn validate(&self) -> ApiResult<()> {
        bounded_len("display_name", &self.display_name, 2, 128)?;
        validate_email("email", &self.email)?;
        if let Some(external_subject) = &self.external_subject {
            bounded_len("external_subject", external_subject, 2, 256)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountResponse {
    pub account_id: AccountId,
    pub principal_id: PrincipalId,
    pub display_name: String,
    pub email: String,
    pub status: AccountStatus,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum WorkspaceMembershipRole {
    Owner,
    Admin,
    Member,
    Viewer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum MembershipState {
    Invited,
    Active,
    Suspended,
    Removed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InviteWorkspaceMemberRequest {
    pub workspace_id: WorkspaceId,
    pub email: String,
    pub role: WorkspaceMembershipRole,
    pub team_ids: Vec<TeamId>,
}

impl Validate for InviteWorkspaceMemberRequest {
    fn validate(&self) -> ApiResult<()> {
        self.workspace_id.validate_field("workspace_id")?;
        validate_email("email", &self.email)?;
        max_items("team_ids", &self.team_ids, 128)?;
        for team_id in &self.team_ids {
            team_id.validate_field("team_id")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceMembershipResponse {
    pub membership_id: MembershipId,
    pub workspace_id: WorkspaceId,
    pub account_id: AccountId,
    pub principal_id: PrincipalId,
    pub role: WorkspaceMembershipRole,
    pub state: MembershipState,
    pub team_ids: Vec<TeamId>,
    pub created_at: String,
    pub updated_at: String,
}

pub fn validate_email(field: &'static str, value: &str) -> ApiResult<()> {
    required(field, value)?;
    if value.len() > 254 || !value.contains('@') || value.starts_with('@') || value.ends_with('@') {
        return Err(crate::validation::ValidationError::new(
            field,
            "must be a valid email address",
        ));
    }
    Ok(())
}

use crate::ids::{PolicyId, PrincipalId, WorkspaceId};
use crate::validation::{ApiResult, Validate, bounded_len, slug};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateWorkspaceRequest {
    pub name: String,
    pub slug: String,
    pub owner_principal_id: PrincipalId,
    pub default_policy_id: Option<PolicyId>,
}

impl Validate for CreateWorkspaceRequest {
    fn validate(&self) -> ApiResult<()> {
        bounded_len("name", &self.name, 2, 128)?;
        slug("slug", &self.slug)?;
        self.owner_principal_id
            .validate_field("owner_principal_id")?;
        if let Some(policy_id) = &self.default_policy_id {
            policy_id.validate_field("default_policy_id")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GetWorkspaceRequest {
    pub workspace_id: WorkspaceId,
}

impl Validate for GetWorkspaceRequest {
    fn validate(&self) -> ApiResult<()> {
        self.workspace_id.validate_field("workspace_id")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceResponse {
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub slug: String,
    pub default_policy_id: Option<PolicyId>,
    pub created_at: String,
    pub updated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_workspace_requires_a_non_empty_name() {
        let request = CreateWorkspaceRequest {
            name: " ".into(),
            slug: "valid-workspace".into(),
            owner_principal_id: PrincipalId::unchecked("principal-1"),
            default_policy_id: None,
        };

        assert!(request.validate().is_err());
    }

    #[test]
    fn create_workspace_accepts_minimal_valid_request() {
        let request = CreateWorkspaceRequest {
            name: "Platform".into(),
            slug: "platform".into(),
            owner_principal_id: PrincipalId::unchecked("principal-1"),
            default_policy_id: None,
        };

        assert_eq!(request.validate(), Ok(()));
    }
}

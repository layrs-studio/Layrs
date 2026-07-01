use crate::accounts::validate_email;
use crate::ids::{AccountId, DeviceFlowId, PrincipalId, WorkspaceId};
use crate::validation::{ApiResult, Validate, bounded_len, optional_required, required};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum AuthProviderKind {
    Password,
    OAuth,
    MagicLink,
    DesktopDeviceFlow,
    PersonalAccessToken,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebSignInRequest {
    pub email: String,
    pub password: Option<String>,
    pub provider: AuthProviderKind,
    pub remember_device: bool,
}

impl Validate for WebSignInRequest {
    fn validate(&self) -> ApiResult<()> {
        validate_email("email", &self.email)?;
        optional_required("password", &self.password)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebSessionResponse {
    pub account_id: AccountId,
    pub principal_id: PrincipalId,
    pub default_workspace_id: Option<WorkspaceId>,
    pub session_expires_at: String,
    pub csrf_token: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateDesktopDeviceFlowRequest {
    pub client_name: String,
    pub device_public_key_thumbprint: String,
    pub requested_workspace_id: Option<WorkspaceId>,
}

impl Validate for CreateDesktopDeviceFlowRequest {
    fn validate(&self) -> ApiResult<()> {
        bounded_len("client_name", &self.client_name, 2, 128)?;
        bounded_len(
            "device_public_key_thumbprint",
            &self.device_public_key_thumbprint,
            16,
            256,
        )?;
        if let Some(workspace_id) = &self.requested_workspace_id {
            workspace_id.validate_field("requested_workspace_id")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DesktopDeviceFlowResponse {
    pub device_flow_id: DeviceFlowId,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_at: String,
    pub interval_seconds: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PollDesktopDeviceFlowRequest {
    pub device_flow_id: DeviceFlowId,
    pub device_public_key_thumbprint: String,
}

impl Validate for PollDesktopDeviceFlowRequest {
    fn validate(&self) -> ApiResult<()> {
        self.device_flow_id.validate_field("device_flow_id")?;
        bounded_len(
            "device_public_key_thumbprint",
            &self.device_public_key_thumbprint,
            16,
            256,
        )?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApproveDesktopDeviceFlowRequest {
    pub device_flow_id: DeviceFlowId,
    pub approving_account_id: AccountId,
    pub workspace_id: Option<WorkspaceId>,
}

impl Validate for ApproveDesktopDeviceFlowRequest {
    fn validate(&self) -> ApiResult<()> {
        self.device_flow_id.validate_field("device_flow_id")?;
        self.approving_account_id
            .validate_field("approving_account_id")?;
        if let Some(workspace_id) = &self.workspace_id {
            workspace_id.validate_field("workspace_id")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DesktopTokenResponse {
    pub account_id: AccountId,
    pub principal_id: PrincipalId,
    pub workspace_id: Option<WorkspaceId>,
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_at: String,
    pub key_binding_thumbprint: String,
}

impl DesktopTokenResponse {
    pub fn bearer(
        account_id: AccountId,
        principal_id: PrincipalId,
        workspace_id: Option<WorkspaceId>,
        access_token: impl Into<String>,
        refresh_token: impl Into<String>,
        expires_at: impl Into<String>,
        key_binding_thumbprint: impl Into<String>,
    ) -> Self {
        Self {
            account_id,
            principal_id,
            workspace_id,
            access_token: access_token.into(),
            refresh_token: refresh_token.into(),
            token_type: "Bearer".into(),
            expires_at: expires_at.into(),
            key_binding_thumbprint: key_binding_thumbprint.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevokeDesktopTokenRequest {
    pub account_id: AccountId,
    pub token_id: String,
}

impl Validate for RevokeDesktopTokenRequest {
    fn validate(&self) -> ApiResult<()> {
        self.account_id.validate_field("account_id")?;
        required("token_id", &self.token_id)?;
        Ok(())
    }
}

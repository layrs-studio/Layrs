use crate::validation::{ApiResult, ValidationError};
use std::fmt;

macro_rules! define_id {
    ($name:ident, $field:literal) => {
        #[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> ApiResult<Self> {
                let value = value.into();
                validate_id_value($field, &value)?;
                Ok(Self(value))
            }

            pub fn unchecked(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_string(self) -> String {
                self.0
            }

            pub fn validate_field(&self, field: &'static str) -> ApiResult<()> {
                validate_id_value(field, &self.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

define_id!(WorkspaceId, "workspace_id");
define_id!(AccountId, "account_id");
define_id!(TeamId, "team_id");
define_id!(MembershipId, "membership_id");
define_id!(SpaceId, "space_id");
define_id!(LayerId, "layer_id");
define_id!(ArtifactId, "artifact_id");
define_id!(ApiChunkId, "chunk_id");
define_id!(WeaveId, "weave_id");
define_id!(ProofId, "proof_id");
define_id!(PolicyId, "policy_id");
define_id!(TimelineEventId, "timeline_event_id");
define_id!(PrincipalId, "principal_id");
define_id!(DeviceFlowId, "device_flow_id");

fn validate_id_value(field: &'static str, value: &str) -> ApiResult<()> {
    if value.trim().is_empty() {
        return Err(ValidationError::new(field, "must not be empty"));
    }
    if value.len() > 128 {
        return Err(ValidationError::new(field, "is longer than expected"));
    }
    let valid = value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'));
    if !valid {
        return Err(ValidationError::new(
            field,
            "contains unsupported characters",
        ));
    }
    Ok(())
}

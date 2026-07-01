use crate::ids::{AccountId, ArtifactId, LayerId, SpaceId, TeamId, WorkspaceId};
use crate::validation::{ApiResult, Validate, ValidationError, max_items, required};
use std::collections::BTreeSet;

pub const LAYER_ACCESS_POLICY_CONFLICT: &str = "LAYER_ACCESS_POLICY_CONFLICT";
pub const LAYER_ACCESS_SCHEMA_V1: &str = "layrs.layer_access.v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum LayerAccessMode {
    InheritLayer,
    Restricted,
    ReservedRedacted,
}

impl LayerAccessMode {
    pub const fn as_wire_value(self) -> &'static str {
        match self {
            Self::InheritLayer => "inherit_layer",
            Self::Restricted => "restricted",
            Self::ReservedRedacted => "reserved_redacted",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum LayerAccessVisibility {
    Full,
    Stub,
}

impl LayerAccessVisibility {
    pub const fn as_wire_value(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Stub => "stub",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum LayerAccessPermission {
    Read,
    Write,
    Admin,
}

impl LayerAccessPermission {
    pub const fn as_wire_value(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Admin => "admin",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayerAccessPrincipalSet {
    pub accounts: Vec<AccountId>,
    pub teams: Vec<TeamId>,
}

impl LayerAccessPrincipalSet {
    pub fn empty() -> Self {
        Self {
            accounts: Vec::new(),
            teams: Vec::new(),
        }
    }

    fn validate(&self, field: &'static str, allow_empty: bool) -> ApiResult<()> {
        max_items(field, &self.accounts, 2048)?;
        max_items(field, &self.teams, 2048)?;

        if !allow_empty && self.accounts.is_empty() && self.teams.is_empty() {
            return Err(ValidationError::new(field, "must list an account or team"));
        }

        let mut accounts = BTreeSet::new();
        for account_id in &self.accounts {
            account_id.validate_field("account_id")?;
            if !accounts.insert(account_id.clone()) {
                return Err(ValidationError::new(field, "contains duplicate accounts"));
            }
        }

        let mut teams = BTreeSet::new();
        for team_id in &self.teams {
            team_id.validate_field("team_id")?;
            if !teams.insert(team_id.clone()) {
                return Err(ValidationError::new(field, "contains duplicate teams"));
            }
        }

        Ok(())
    }

    fn is_empty(&self) -> bool {
        self.accounts.is_empty() && self.teams.is_empty()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayerAccessRulePermissions {
    pub read: LayerAccessPrincipalSet,
    pub write: LayerAccessPrincipalSet,
    pub admin: LayerAccessPrincipalSet,
}

impl LayerAccessRulePermissions {
    pub fn empty() -> Self {
        Self {
            read: LayerAccessPrincipalSet::empty(),
            write: LayerAccessPrincipalSet::empty(),
            admin: LayerAccessPrincipalSet::empty(),
        }
    }

    fn validate(&self) -> ApiResult<()> {
        self.read.validate("permissions.read", true)?;
        self.write.validate("permissions.write", true)?;
        self.admin.validate("permissions.admin", true)?;

        if self.read.is_empty() && self.write.is_empty() && self.admin.is_empty() {
            return Err(ValidationError::new(
                "permissions",
                "must define read, write or admin principals",
            ));
        }

        Ok(())
    }

    fn validate_redacted_stub(&self) -> ApiResult<()> {
        self.read.validate("permissions.read", true)?;
        self.write.validate("permissions.write", true)?;
        self.admin.validate("permissions.admin", true)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayerAccessRule {
    pub id: String,
    pub path: String,
    pub artifact_id: Option<ArtifactId>,
    pub mode: LayerAccessMode,
    pub visibility: LayerAccessVisibility,
    pub permissions: LayerAccessRulePermissions,
}

impl LayerAccessRule {
    fn validate(&self) -> ApiResult<()> {
        required("rule.id", &self.id)?;
        validate_registry_relative_path("rule.path", &self.path)?;
        if let Some(artifact_id) = &self.artifact_id {
            artifact_id.validate_field("artifact_id")?;
        }

        match self.mode {
            LayerAccessMode::InheritLayer => {
                return Err(ValidationError::new(
                    "rule.mode",
                    "inherit_layer is represented by absence of a special rule",
                ));
            }
            LayerAccessMode::Restricted => {
                self.permissions.validate()?;
            }
            LayerAccessMode::ReservedRedacted => {
                if self.visibility != LayerAccessVisibility::Stub {
                    return Err(ValidationError::new(
                        "rule.visibility",
                        "reserved_redacted must expose only a stub",
                    ));
                }
                self.permissions.validate_redacted_stub()?;
            }
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayerAccessSignature {
    pub key_id: String,
    pub value: String,
}

impl LayerAccessSignature {
    fn validate(&self) -> ApiResult<()> {
        required("signature.key_id", &self.key_id)?;
        required("signature.value", &self.value)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayerAccessRegistry {
    pub schema: String,
    pub workspace_id: WorkspaceId,
    pub space_id: SpaceId,
    pub layer_id: LayerId,
    pub registry_path: String,
    pub policy_epoch: u64,
    pub generated_at: String,
    pub rules: Vec<LayerAccessRule>,
    pub signature: Option<LayerAccessSignature>,
}

impl LayerAccessRegistry {
    pub fn expected_registry_path(layer_id: &LayerId) -> String {
        format!(".layrs/layers/{}/access.json", layer_id.as_str())
    }

    pub fn validate_access_policy(&self) -> Result<(), LayerAccessPolicyConflictResponse> {
        self.validate_shape().map_err(|error| {
            self.conflict(
                LayerAccessPolicyConflictKind::InvalidContract,
                error.field,
                error.message,
                None,
                None,
            )
        })?;

        if self.schema != LAYER_ACCESS_SCHEMA_V1 {
            return Err(self.conflict(
                LayerAccessPolicyConflictKind::InvalidContract,
                "schema",
                "must be layrs.layer_access.v1",
                None,
                None,
            ));
        }

        if self.registry_path != Self::expected_registry_path(&self.layer_id) {
            return Err(self.conflict(
                LayerAccessPolicyConflictKind::InvalidRegistryPath,
                "registry_path",
                "must point at .layrs/layers/<layer_id>/access.json",
                Some(self.registry_path.clone()),
                Some(Self::expected_registry_path(&self.layer_id)),
            ));
        }

        let mut rule_ids = BTreeSet::new();
        for rule in &self.rules {
            if !rule_ids.insert(rule.id.clone()) {
                return Err(self.conflict(
                    LayerAccessPolicyConflictKind::DuplicateRule,
                    "rules.id",
                    "rule id appears more than once",
                    Some(rule.id.clone()),
                    None,
                ));
            }
        }

        for left_index in 0..self.rules.len() {
            for right_index in (left_index + 1)..self.rules.len() {
                let left = &self.rules[left_index].path;
                let right = &self.rules[right_index].path;
                if paths_collide(left, right) {
                    return Err(self.conflict(
                        LayerAccessPolicyConflictKind::PathCollision,
                        "rules.path",
                        "layer access rules must not overlap paths",
                        Some(left.clone()),
                        Some(right.clone()),
                    ));
                }
            }
        }

        Ok(())
    }

    fn validate_shape(&self) -> ApiResult<()> {
        required("schema", &self.schema)?;
        self.workspace_id.validate_field("workspace_id")?;
        self.space_id.validate_field("space_id")?;
        self.layer_id.validate_field("layer_id")?;
        validate_registry_relative_path("registry_path", &self.registry_path)?;
        if self.policy_epoch == 0 {
            return Err(ValidationError::new(
                "policy_epoch",
                "must be greater than zero",
            ));
        }
        required("generated_at", &self.generated_at)?;
        max_items("rules", &self.rules, 4096)?;
        for rule in &self.rules {
            rule.validate()?;
        }
        if let Some(signature) = &self.signature {
            signature.validate()?;
        }
        Ok(())
    }

    fn conflict(
        &self,
        kind: LayerAccessPolicyConflictKind,
        field: &'static str,
        message: &'static str,
        path: Option<String>,
        conflicting_path: Option<String>,
    ) -> LayerAccessPolicyConflictResponse {
        LayerAccessPolicyConflictResponse {
            error: LayerAccessPolicyConflictError {
                code: LAYER_ACCESS_POLICY_CONFLICT,
                message,
                workspace_id: self.workspace_id.clone(),
                space_id: self.space_id.clone(),
                layer_id: self.layer_id.clone(),
                policy_epoch: self.policy_epoch,
                conflicts: vec![LayerAccessPolicyConflict {
                    kind,
                    field,
                    path,
                    conflicting_path,
                    message,
                }],
            },
        }
    }
}

impl Validate for LayerAccessRegistry {
    fn validate(&self) -> ApiResult<()> {
        self.validate_access_policy().map_err(|_| {
            ValidationError::new("access_registry", "has conflicting layer access policy")
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PutLayerAccessRegistryRequest {
    pub registry: LayerAccessRegistry,
    pub expected_policy_epoch: Option<u64>,
}

impl Validate for PutLayerAccessRegistryRequest {
    fn validate(&self) -> ApiResult<()> {
        self.registry.validate()?;
        if matches!(self.expected_policy_epoch, Some(0)) {
            return Err(ValidationError::new(
                "expected_policy_epoch",
                "must be greater than zero",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GetLayerAccessRegistryRequest {
    pub workspace_id: WorkspaceId,
    pub space_id: SpaceId,
    pub layer_id: LayerId,
}

impl Validate for GetLayerAccessRegistryRequest {
    fn validate(&self) -> ApiResult<()> {
        self.workspace_id.validate_field("workspace_id")?;
        self.space_id.validate_field("space_id")?;
        self.layer_id.validate_field("layer_id")?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayerAccessRegistryResponse {
    pub registry: LayerAccessRegistry,
    pub updated_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayerAccessPolicyConflictResponse {
    pub error: LayerAccessPolicyConflictError,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayerAccessPolicyConflictError {
    pub code: &'static str,
    pub message: &'static str,
    pub workspace_id: WorkspaceId,
    pub space_id: SpaceId,
    pub layer_id: LayerId,
    pub policy_epoch: u64,
    pub conflicts: Vec<LayerAccessPolicyConflict>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayerAccessPolicyConflict {
    pub kind: LayerAccessPolicyConflictKind,
    pub field: &'static str,
    pub path: Option<String>,
    pub conflicting_path: Option<String>,
    pub message: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum LayerAccessPolicyConflictKind {
    InvalidContract,
    InvalidRegistryPath,
    DuplicateRule,
    PathCollision,
}

pub fn validate_registry_relative_path(field: &'static str, value: &str) -> ApiResult<()> {
    required(field, value)?;
    if value.len() > 1024
        || value.starts_with('/')
        || value.starts_with('\\')
        || value.contains('\\')
        || value.contains("//")
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(ValidationError::new(
            field,
            "must be a normalized relative path",
        ));
    }
    Ok(())
}

fn paths_collide(left: &str, right: &str) -> bool {
    left == right
        || left
            .strip_prefix(right)
            .is_some_and(|suffix| suffix.starts_with('/'))
        || right
            .strip_prefix(left)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry_path() -> String {
        ".layrs/layers/layer-1/access.json".into()
    }

    fn principal_set(account: &str, team: &str) -> LayerAccessPrincipalSet {
        LayerAccessPrincipalSet {
            accounts: vec![AccountId::unchecked(account)],
            teams: vec![TeamId::unchecked(team)],
        }
    }

    fn permissions() -> LayerAccessRulePermissions {
        LayerAccessRulePermissions {
            read: principal_set("account-1", "team-1"),
            write: LayerAccessPrincipalSet {
                accounts: vec![AccountId::unchecked("account-1")],
                teams: Vec::new(),
            },
            admin: LayerAccessPrincipalSet {
                accounts: Vec::new(),
                teams: vec![TeamId::unchecked("team-admin")],
            },
        }
    }

    fn base_registry() -> LayerAccessRegistry {
        LayerAccessRegistry {
            schema: LAYER_ACCESS_SCHEMA_V1.into(),
            workspace_id: WorkspaceId::unchecked("workspace-1"),
            space_id: SpaceId::unchecked("space-1"),
            layer_id: LayerId::unchecked("layer-1"),
            registry_path: registry_path(),
            policy_epoch: 1,
            generated_at: "2026-06-29T20:30:00Z".into(),
            rules: Vec::new(),
            signature: Some(LayerAccessSignature {
                key_id: "server-key-1".into(),
                value: "signature-placeholder".into(),
            }),
        }
    }

    #[test]
    fn restricted_registry_accepts_rule_permissions() {
        let mut registry = base_registry();
        registry.rules = vec![LayerAccessRule {
            id: "access-rule-1".into(),
            path: "Assets/Private/hero.texture.png".into(),
            artifact_id: Some(ArtifactId::unchecked("artifact-1")),
            mode: LayerAccessMode::Restricted,
            visibility: LayerAccessVisibility::Stub,
            permissions: permissions(),
        }];

        assert_eq!(registry.validate_access_policy(), Ok(()));
    }

    #[test]
    fn registry_rejects_duplicate_rule_ids_as_policy_conflict() {
        let mut registry = base_registry();
        registry.rules = vec![
            LayerAccessRule {
                id: "access-rule-1".into(),
                path: "Assets/Private/hero.texture.png".into(),
                artifact_id: Some(ArtifactId::unchecked("artifact-1")),
                mode: LayerAccessMode::Restricted,
                visibility: LayerAccessVisibility::Stub,
                permissions: permissions(),
            },
            LayerAccessRule {
                id: "access-rule-1".into(),
                path: "Assets/Private/villain.texture.png".into(),
                artifact_id: Some(ArtifactId::unchecked("artifact-2")),
                mode: LayerAccessMode::Restricted,
                visibility: LayerAccessVisibility::Stub,
                permissions: permissions(),
            },
        ];

        let response = registry.validate_access_policy().unwrap_err();

        assert_eq!(response.error.code, LAYER_ACCESS_POLICY_CONFLICT);
        assert_eq!(
            response.error.conflicts[0].kind,
            LayerAccessPolicyConflictKind::DuplicateRule
        );
    }

    #[test]
    fn registry_rejects_overlapping_paths() {
        let mut registry = base_registry();
        registry.rules = vec![
            LayerAccessRule {
                id: "access-rule-1".into(),
                path: "Assets/Private".into(),
                artifact_id: None,
                mode: LayerAccessMode::ReservedRedacted,
                visibility: LayerAccessVisibility::Stub,
                permissions: permissions(),
            },
            LayerAccessRule {
                id: "access-rule-2".into(),
                path: "Assets/Private/hero.texture.png".into(),
                artifact_id: Some(ArtifactId::unchecked("artifact-1")),
                mode: LayerAccessMode::Restricted,
                visibility: LayerAccessVisibility::Stub,
                permissions: permissions(),
            },
        ];

        let response = registry.validate_access_policy().unwrap_err();

        assert_eq!(response.error.code, LAYER_ACCESS_POLICY_CONFLICT);
        assert_eq!(
            response.error.conflicts[0].kind,
            LayerAccessPolicyConflictKind::PathCollision
        );
    }

    #[test]
    fn registry_path_must_match_layer_contract_location() {
        let mut registry = base_registry();
        registry.registry_path = ".layrs/layers/other/access.json".into();

        let response = registry.validate_access_policy().unwrap_err();

        assert_eq!(
            response.error.conflicts[0].kind,
            LayerAccessPolicyConflictKind::InvalidRegistryPath
        );
    }
}

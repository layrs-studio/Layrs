use super::*;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamRole {
    Member,
    Auditor,
    ReleaseManager,
}
impl TeamRole {
    pub(super) fn member_role(self) -> MemberRole {
        match self {
            Self::Member => MemberRole::Member,
            Self::Auditor => MemberRole::Auditor,
            Self::ReleaseManager => MemberRole::ReleaseManager,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeamRequest {
    pub idempotency_key: String,
    pub expected_version: u32,
    pub name: String,
    pub role: TeamRole,
    pub member_principal_ids: Vec<PrincipalId>,
}
impl TeamRequest {
    pub(super) fn normalize(&mut self) -> V3StoreResult<()> {
        super::super::types::validate_key(&self.idempotency_key)?;
        self.name = self.name.trim().to_owned();
        if self.name.is_empty()
            || self.name.len() > 128
            || self.name.chars().any(char::is_control)
            || self.member_principal_ids.len() > 64
            || self.expected_version >= 1000
        {
            return Err(invalid("invalid_team_request"));
        }
        self.member_principal_ids
            .sort_by(|a, b| a.as_str().cmp(b.as_str()));
        if self
            .member_principal_ids
            .windows(2)
            .any(|pair| pair[0] == pair[1])
        {
            return Err(invalid("duplicate_team_member"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArchiveTeamRequest {
    pub idempotency_key: String,
    pub expected_version: u32,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeamRecord {
    pub schema_version: String,
    pub team_id: String,
    pub space_id: SpaceId,
    pub version: u32,
    pub name: String,
    pub role: TeamRole,
    pub member_principal_ids: Vec<PrincipalId>,
    pub state: String,
    pub previous_event_object_id: Option<ObjectId>,
    pub policy_product_object_id: Option<ObjectId>,
    pub previous_policy_set_id: ObjectId,
    pub policy_set_id: ObjectId,
    pub authorization_epoch: u64,
    pub changed_by: PrincipalId,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub reason: Option<String>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeamHistoryQuery {
    pub after: Option<u32>,
    pub limit: Option<u16>,
    pub anchor: Option<u32>,
}
pub(super) fn team_id(id: &str) -> V3StoreResult<()> {
    if id.len() != 69
        || !id.starts_with("team:")
        || !id[5..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("invalid_team_id"));
    }
    Ok(())
}
pub(super) fn response(record: &TeamRecord, id: &ObjectId) -> serde_json::Value {
    let mut value = serde_json::to_value(record).expect("typed team record");
    value["eventObjectId"] = serde_json::json!(id);
    serde_json::json!({"team":value,"policySetId":record.policy_set_id,"authorizationEpoch":record.authorization_epoch})
}

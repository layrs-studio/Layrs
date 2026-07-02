use crate::digest::ObjectDigest;
use crate::manifest::SyncManifestV2;
use crate::objects::StoreObjectsV2;
use crate::validation::{
    SyncResult, SyncValidationError, validate_non_empty, validate_object_digest, validate_path,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::fmt;

pub const SYNC_PROTOCOL_V2: &str = "layrs.sync.v2";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishRequestV2 {
    pub protocol: String,
    pub layer_id: String,
    pub policy_epoch: u64,
    pub idempotency_key: IdempotencyKey,
    pub source_client_id: String,
    pub base_tree_id: Option<ObjectDigest>,
    pub root_tree_id: ObjectDigest,
    pub changed_paths: Vec<String>,
    pub store_objects: StoreObjectsV2,
}

impl PublishRequestV2 {
    pub fn validate(&self) -> SyncResult<()> {
        if self.protocol != SYNC_PROTOCOL_V2 {
            return Err(SyncValidationError::new(
                "protocol",
                "must be layrs.sync.v2",
            ));
        }
        validate_non_empty("layer_id", &self.layer_id)?;
        validate_non_empty("source_client_id", &self.source_client_id)?;
        IdempotencyKey::validate_raw(self.idempotency_key.as_str())?;
        if let Some(base_tree_id) = &self.base_tree_id {
            validate_object_digest("base_tree_id", base_tree_id.as_str())?;
        }
        validate_object_digest("root_tree_id", self.root_tree_id.as_str())?;
        for path in &self.changed_paths {
            validate_path("changed_paths", path)?;
        }
        self.store_objects.validate()?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiveRequestV2 {
    pub protocol: String,
    pub workspace_id: String,
    pub space_id: String,
    pub layer_id: String,
    pub since_cursor: Option<String>,
    pub client_id: String,
    pub known_tree_id: Option<ObjectDigest>,
    pub max_objects: u32,
}

impl ReceiveRequestV2 {
    pub fn validate(&self) -> SyncResult<()> {
        if self.protocol != SYNC_PROTOCOL_V2 {
            return Err(SyncValidationError::new(
                "protocol",
                "must be layrs.sync.v2",
            ));
        }
        validate_non_empty("workspace_id", &self.workspace_id)?;
        validate_non_empty("space_id", &self.space_id)?;
        validate_non_empty("layer_id", &self.layer_id)?;
        validate_non_empty("client_id", &self.client_id)?;
        if let Some(cursor) = &self.since_cursor {
            validate_non_empty("since_cursor", cursor)?;
        }
        if let Some(tree_id) = &self.known_tree_id {
            validate_object_digest("known_tree_id", tree_id.as_str())?;
        }
        if self.max_objects == 0 {
            return Err(SyncValidationError::new(
                "max_objects",
                "must be greater than zero",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiveResponseV2 {
    pub protocol: String,
    pub manifest: SyncManifestV2,
    pub store_objects: StoreObjectsV2,
    pub server_cursor: String,
    pub has_more: bool,
}

impl ReceiveResponseV2 {
    pub fn validate(&self) -> SyncResult<()> {
        if self.protocol != SYNC_PROTOCOL_V2 {
            return Err(SyncValidationError::new(
                "protocol",
                "must be layrs.sync.v2",
            ));
        }
        self.manifest.validate()?;
        self.store_objects.validate()?;
        validate_non_empty("server_cursor", &self.server_cursor)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentRequestV2 {
    pub protocol: String,
    pub object_ids: Vec<ObjectDigest>,
    pub max_objects: u32,
}

impl ContentRequestV2 {
    pub fn validate(&self) -> SyncResult<()> {
        if self.protocol != SYNC_PROTOCOL_V2 {
            return Err(SyncValidationError::new(
                "protocol",
                "must be layrs.sync.v2",
            ));
        }
        if self.object_ids.is_empty() {
            return Err(SyncValidationError::new(
                "object_ids",
                "must contain at least one object id",
            ));
        }
        for object_id in &self.object_ids {
            validate_object_digest("object_ids", object_id.as_str())?;
        }
        if self.max_objects == 0 {
            return Err(SyncValidationError::new(
                "max_objects",
                "must be greater than zero",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentResponseV2 {
    pub protocol: String,
    pub store_objects: StoreObjectsV2,
    pub missing_objects: Vec<ObjectDigest>,
}

impl ContentResponseV2 {
    pub fn validate(&self) -> SyncResult<()> {
        if self.protocol != SYNC_PROTOCOL_V2 {
            return Err(SyncValidationError::new(
                "protocol",
                "must be layrs.sync.v2",
            ));
        }
        self.store_objects.validate()?;
        for object_id in &self.missing_objects {
            validate_object_digest("missing_objects", object_id.as_str())?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    pub const MIN_LEN: usize = 16;
    pub const MAX_LEN: usize = 128;

    pub fn new(value: impl Into<String>) -> SyncResult<Self> {
        let value = value.into();
        Self::validate_raw(&value)?;
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

    pub fn validate_raw(value: &str) -> SyncResult<()> {
        if value.len() < Self::MIN_LEN {
            return Err(SyncValidationError::new(
                "idempotency_key",
                "must be at least 16 bytes",
            ));
        }

        if value.len() > Self::MAX_LEN {
            return Err(SyncValidationError::new(
                "idempotency_key",
                "must be at most 128 bytes",
            ));
        }

        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
        {
            return Err(SyncValidationError::new(
                "idempotency_key",
                "may only contain ASCII letters, numbers, '-', '_', ':' or '.'",
            ));
        }

        Ok(())
    }
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for IdempotencyKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for IdempotencyKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

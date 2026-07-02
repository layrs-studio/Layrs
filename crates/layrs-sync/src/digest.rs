use crate::validation::{SyncResult, validate_object_digest};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::fmt;

pub const OBJECT_DIGEST_ALGORITHM: &str = "blake3";
pub const OBJECT_DIGEST_PREFIX: &str = "blake3:";
pub const BLAKE3_HEX_LEN: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ObjectDigest(String);

impl ObjectDigest {
    pub fn new(value: impl Into<String>) -> SyncResult<Self> {
        let value = value.into();
        validate_object_digest("object_digest", &value)?;
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn blake3_for(bytes: &[u8]) -> Self {
        Self(format!(
            "{OBJECT_DIGEST_PREFIX}{}",
            blake3::hash(bytes).to_hex()
        ))
    }

    pub fn unchecked(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn algorithm(&self) -> &str {
        self.0
            .split_once(':')
            .map(|(algorithm, _)| algorithm)
            .unwrap_or_default()
    }

    pub fn hex(&self) -> &str {
        self.0
            .split_once(':')
            .map(|(_, hex)| hex)
            .unwrap_or_default()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for ObjectDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for ObjectDigest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ObjectDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

use super::error::{Result, ScmError};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use std::str::FromStr;

fn validate_object_id(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("blake3:")
        && value.as_bytes()[7..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

macro_rules! content_id {
    ($name:ident, $label:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: impl Into<String>) -> Result<Self> {
                let value = value.into();
                if !validate_object_id(&value) {
                    return Err(ScmError::InvalidId {
                        kind: $label,
                        value,
                    });
                }
                Ok(Self(value))
            }

            #[allow(dead_code)]
            pub(crate) fn from_digest(digest: blake3::Hash) -> Self {
                Self(format!("blake3:{digest}"))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = ScmError;

            fn from_str(value: &str) -> Result<Self> {
                Self::parse(value)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                let value = String::deserialize(deserializer)?;
                Self::parse(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

macro_rules! opaque_id {
    ($name:ident, $prefix:literal, $label:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: impl Into<String>) -> Result<Self> {
                let value = value.into();
                let Some(suffix) = value.strip_prefix($prefix) else {
                    return Err(ScmError::InvalidId {
                        kind: $label,
                        value,
                    });
                };
                let mut chars = suffix.chars();
                let first_valid = chars.next().is_some_and(|ch| ch.is_ascii_alphanumeric());
                let rest_valid = chars
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | ':' | '-'));
                if !first_valid || !rest_valid || suffix.len() > 128 {
                    return Err(ScmError::InvalidId {
                        kind: $label,
                        value,
                    });
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = ScmError;

            fn from_str(value: &str) -> Result<Self> {
                Self::parse(value)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                let value = String::deserialize(deserializer)?;
                Self::parse(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

content_id!(ObjectId, "ObjectId");
content_id!(ChunkId, "ChunkId");
content_id!(FileId, "FileId");
content_id!(FileManifestId, "FileManifestId");
content_id!(TreeId, "TreeId");
content_id!(ConflictId, "ConflictId");
content_id!(SymlinkId, "SymlinkId");
content_id!(TombstoneId, "TombstoneId");
content_id!(StepId, "StepId");
content_id!(RepositoryViewId, "RepositoryViewId");

opaque_id!(SpaceId, "space:", "SpaceId");
opaque_id!(StoreId, "store:", "StoreId");
opaque_id!(LayerId, "layer:", "LayerId");
opaque_id!(WorktreeId, "worktree:", "WorktreeId");
opaque_id!(MarkerId, "marker:", "MarkerId");
opaque_id!(ReleaseId, "release:", "ReleaseId");
opaque_id!(
    ReleasePublicationId,
    "release-publication:",
    "ReleasePublicationId"
);
opaque_id!(OperationId, "operation:", "OperationId");
opaque_id!(ObservationId, "observation:", "ObservationId");
opaque_id!(ActorId, "actor:", "ActorId");
opaque_id!(DeviceId, "device:", "DeviceId");
opaque_id!(ProofId, "proof:", "ProofId");

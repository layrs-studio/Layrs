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

/// Length of every content id: `blake3:` and 64 lowercase hex digits.
const CONTENT_ID_LEN: usize = 71;

/// Writes `blake3:<hex digest>` into a content id's bytes.
fn content_id_bytes(digest: &[u8; 32]) -> [u8; CONTENT_ID_LEN] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0_u8; CONTENT_ID_LEN];
    bytes[..7].copy_from_slice(b"blake3:");
    for (index, byte) in digest.iter().enumerate() {
        bytes[7 + 2 * index] = HEX[usize::from(byte >> 4)];
        bytes[8 + 2 * index] = HEX[usize::from(byte & 0x0f)];
    }
    bytes
}

macro_rules! content_id {
    ($name:ident, $label:literal) => {
        /// Stored inline (no allocation): cloning is a copy of its 71 bytes.
        /// Ordering, equality and text are those of its `blake3:<hex>` form.
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name([u8; CONTENT_ID_LEN]);

        impl $name {
            pub fn parse(value: impl Into<String> + AsRef<str>) -> Result<Self> {
                let text = value.as_ref();
                if !validate_object_id(text) {
                    return Err(ScmError::InvalidId {
                        kind: $label,
                        value: value.into(),
                    });
                }
                let mut bytes = [0_u8; CONTENT_ID_LEN];
                bytes.copy_from_slice(text.as_bytes());
                Ok(Self(bytes))
            }

            #[allow(dead_code)]
            pub(crate) fn from_digest(digest: blake3::Hash) -> Self {
                Self(content_id_bytes(digest.as_bytes()))
            }

            /// The id of a 32-byte blake3 digest (`blake3:<hex>`), without
            /// the text round trip of `parse`.
            pub fn from_digest_bytes(digest: &[u8; 32]) -> Self {
                Self(content_id_bytes(digest))
            }

            /// The 32-byte digest this id names.
            pub fn digest_bytes(&self) -> [u8; 32] {
                let nibble = |byte: u8| match byte {
                    b'0'..=b'9' => byte - b'0',
                    _ => byte - b'a' + 10,
                };
                let mut digest = [0_u8; 32];
                for (index, slot) in digest.iter_mut().enumerate() {
                    *slot = (nibble(self.0[7 + 2 * index]) << 4) | nibble(self.0[8 + 2 * index]);
                }
                digest
            }

            pub fn as_str(&self) -> &str {
                // Only validated ASCII (`parse`) or hex text written by
                // `content_id_bytes` is ever stored.
                std::str::from_utf8(&self.0).unwrap_or_default()
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_tuple(stringify!($name))
                    .field(&self.as_str())
                    .finish()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = ScmError;

            fn from_str(value: &str) -> Result<Self> {
                Self::parse(value)
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                let value = std::borrow::Cow::<'de, str>::deserialize(deserializer)?;
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

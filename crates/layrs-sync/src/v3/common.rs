use layrs_core::scm::{
    LayerId, MarkerId, ObjectId, ProofId, ReleaseId, RepoPath, RepositoryViewId, StepId,
};
use serde::{Deserialize, Deserializer, Serialize};
use std::{fmt, str::FromStr};

/// A JSON field that is mandatory but whose value may be `null`. Unlike
/// `Option<T>`, this wrapper makes Serde reject an absent field.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Nullable<T>(pub Option<T>);
impl<T> Nullable<T> {
    pub fn null() -> Self {
        Self(None)
    }
    pub fn some(value: T) -> Self {
        Self(Some(value))
    }
    pub fn as_ref(&self) -> Option<&T> {
        self.0.as_ref()
    }
    pub fn is_some(&self) -> bool {
        self.0.is_some()
    }
    pub fn is_none(&self) -> bool {
        self.0.is_none()
    }
}
impl<T: Serialize> Serialize for Nullable<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Nullable<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire<T> {
            Value(T),
            Null(()),
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Value(value) => Self::some(value),
            Wire::Null(()) => Self::null(),
        })
    }
}

macro_rules! opaque_id {
    ($name:ident, $prefix:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: impl Into<String>) -> Result<Self, String> {
                let value = value.into();
                let suffix = value.strip_prefix($prefix).ok_or_else(|| {
                    format!("{} must start with `{}`", stringify!($name), $prefix)
                })?;
                let mut chars = suffix.chars();
                let first = chars.next().is_some_and(|c| c.is_ascii_alphanumeric());
                let rest =
                    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'));
                if !first || !rest || suffix.len() > 128 {
                    return Err(format!("invalid {}", stringify!($name)));
                }
                Ok(Self(value))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
        impl FromStr for $name {
            type Err = String;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::parse(value)
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                Self::parse(String::deserialize(d)?).map_err(serde::de::Error::custom)
            }
        }
    };
}

opaque_id!(ClientId, "client:");
opaque_id!(PrincipalId, "principal:");
opaque_id!(RequestId, "request:");
opaque_id!(PlanId, "plan:");
opaque_id!(TransactionId, "transaction:");
opaque_id!(EventId, "event:");
opaque_id!(MutationId, "mutation:");
opaque_id!(PolicyId, "policy:");
opaque_id!(PolicyVersionId, "policy-version:");
opaque_id!(GateDefinitionId, "gate-definition:");
opaque_id!(GateEvaluationId, "gate-evaluation:");
opaque_id!(WeaveLogicalId, "weave:");
opaque_id!(DecisionId, "decision:");

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        let valid = (16..=128).contains(&value.len())
            && value
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric())
            && value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'));
        valid
            .then_some(Self(value))
            .ok_or_else(|| "invalid IdempotencyKey".into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl<'de> Deserialize<'de> for IdempotencyKey {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Cursor(String);
impl Cursor {
    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        (1..=2048)
            .contains(&value.len())
            .then_some(Self(value))
            .ok_or_else(|| "invalid Cursor".into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl<'de> Deserialize<'de> for Cursor {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ProductLogicalId(String);
impl ProductLogicalId {
    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        const PREFIXES: [&str; 9] = [
            "layer:",
            "marker:",
            "release:",
            "proof:",
            "gate-definition:",
            "gate-evaluation:",
            "policy:",
            "weave:",
            "decision:",
        ];
        let suffix = PREFIXES
            .iter()
            .find_map(|prefix| value.strip_prefix(prefix))
            .ok_or_else(|| "invalid ProductLogicalId prefix".to_owned())?;
        let mut chars = suffix.chars();
        let first = chars.next().is_some_and(|c| c.is_ascii_alphanumeric());
        let rest = chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'));
        if !first || !rest || suffix.len() > 128 {
            return Err("invalid ProductLogicalId".into());
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl<'de> Deserialize<'de> for ProductLogicalId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    Step,
    RepositoryView,
    Chunk,
    FileManifest,
    Tree,
    Conflict,
    Symlink,
    Tombstone,
    Evidence,
    Product,
    PolicySet,
    AuthorizationSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectReference {
    pub kind: ObjectKind,
    pub object_id: ObjectId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Compression {
    Identity,
    Zstd,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectDescriptor {
    pub kind: ObjectKind,
    pub object_id: ObjectId,
    pub canonical_byte_len: u64,
    pub encoded_byte_len: u64,
    pub compression: Compression,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProductKind {
    Layer,
    Marker,
    Release,
    Proof,
    GateDefinition,
    GateEvaluation,
    Policy,
    Weave,
    Decision,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum ProductRoot {
    #[serde(rename = "layer")]
    Layer {
        #[serde(rename = "logicalId")]
        logical_id: LayerId,
        #[serde(rename = "objectId")]
        object_id: ObjectId,
    },
    #[serde(rename = "marker")]
    Marker {
        #[serde(rename = "logicalId")]
        logical_id: MarkerId,
        #[serde(rename = "objectId")]
        object_id: ObjectId,
    },
    #[serde(rename = "release")]
    Release {
        #[serde(rename = "logicalId")]
        logical_id: ReleaseId,
        #[serde(rename = "objectId")]
        object_id: ObjectId,
    },
    #[serde(rename = "proof")]
    Proof {
        #[serde(rename = "logicalId")]
        logical_id: ProofId,
        #[serde(rename = "objectId")]
        object_id: ObjectId,
    },
    #[serde(rename = "gateDefinition")]
    GateDefinition {
        #[serde(rename = "logicalId")]
        logical_id: GateDefinitionId,
        #[serde(rename = "objectId")]
        object_id: ObjectId,
    },
    #[serde(rename = "gateEvaluation")]
    GateEvaluation {
        #[serde(rename = "logicalId")]
        logical_id: GateEvaluationId,
        #[serde(rename = "objectId")]
        object_id: ObjectId,
    },
    #[serde(rename = "policy")]
    Policy {
        #[serde(rename = "logicalId")]
        logical_id: PolicyId,
        #[serde(rename = "objectId")]
        object_id: ObjectId,
    },
    #[serde(rename = "weave")]
    Weave {
        #[serde(rename = "logicalId")]
        logical_id: WeaveLogicalId,
        #[serde(rename = "objectId")]
        object_id: ObjectId,
    },
    #[serde(rename = "decision")]
    Decision {
        #[serde(rename = "logicalId")]
        logical_id: DecisionId,
        #[serde(rename = "objectId")]
        object_id: ObjectId,
    },
}

impl ProductRoot {
    pub fn object_id(&self) -> &ObjectId {
        match self {
            Self::Layer { object_id, .. }
            | Self::Marker { object_id, .. }
            | Self::Release { object_id, .. }
            | Self::Proof { object_id, .. }
            | Self::GateDefinition { object_id, .. }
            | Self::GateEvaluation { object_id, .. }
            | Self::Policy { object_id, .. }
            | Self::Weave { object_id, .. }
            | Self::Decision { object_id, .. } => object_id,
        }
    }

    pub fn logical_id(&self) -> ProductLogicalId {
        let value = match self {
            Self::Layer { logical_id, .. } => logical_id.as_str(),
            Self::Marker { logical_id, .. } => logical_id.as_str(),
            Self::Release { logical_id, .. } => logical_id.as_str(),
            Self::Proof { logical_id, .. } => logical_id.as_str(),
            Self::GateDefinition { logical_id, .. } => logical_id.as_str(),
            Self::GateEvaluation { logical_id, .. } => logical_id.as_str(),
            Self::Policy { logical_id, .. } => logical_id.as_str(),
            Self::Weave { logical_id, .. } => logical_id.as_str(),
            Self::Decision { logical_id, .. } => logical_id.as_str(),
        };
        ProductLogicalId::parse(value).expect("typed ProductRoot IDs are valid ProductLogicalIds")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum AuthorizationRoot {
    #[serde(rename = "step")]
    Step {
        #[serde(rename = "stepId")]
        step_id: StepId,
    },
    #[serde(rename = "layerHead")]
    LayerHead {
        #[serde(rename = "layerId")]
        layer_id: LayerId,
        generation: u64,
        #[serde(rename = "headStepId")]
        head_step_id: StepId,
    },
    #[serde(rename = "product")]
    Product { product: ProductRoot },
    #[serde(rename = "repositoryView")]
    RepositoryView {
        #[serde(rename = "repositoryViewId")]
        repository_view_id: RepositoryViewId,
    },
    #[serde(rename = "policySet")]
    PolicySet {
        #[serde(rename = "policySetId")]
        policy_set_id: ObjectId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RootPath {
    pub root: AuthorizationRoot,
    pub repo_path: RepoPath,
}

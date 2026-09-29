use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{
    DiffKind, InspectorValueType, LensCapability, PreviewKind, ReconcileStatus, ResolutionMethods,
    ResolutionScope, ScopedResolutionMethod,
};

macro_rules! impl_extensible_string_serde {
    ($type:ty) => {
        impl Serialize for $type {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $type {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                String::deserialize(deserializer).map(|value| Self::from_wire(&value))
            }
        }
    };
}

macro_rules! impl_closed_string_serde {
    ($type:ty) => {
        impl Serialize for $type {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $type {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::from_wire(&value).ok_or_else(|| {
                    D::Error::custom(format!("unsupported {} value `{value}`", stringify!($type)))
                })
            }
        }
    };
}

impl LensCapability {
    pub fn as_str(&self) -> &str {
        match self {
            Self::View => "view",
            Self::Diff => "diff",
            Self::Reconcile => "reconcile",
            Self::Metadata => "metadata",
            Self::Preview => "preview",
            Self::References => "references",
            Self::ProofRecipes => "proofRecipes",
            Self::Custom(value) => value,
        }
    }

    fn from_wire(value: &str) -> Self {
        match value {
            "view" => Self::View,
            "diff" => Self::Diff,
            "reconcile" => Self::Reconcile,
            "metadata" => Self::Metadata,
            "preview" => Self::Preview,
            "references" => Self::References,
            "proofRecipes" => Self::ProofRecipes,
            custom => Self::Custom(custom.to_string()),
        }
    }
}

impl_extensible_string_serde!(LensCapability);

impl ResolutionMethods {
    pub fn scoped(&self) -> Vec<ScopedResolutionMethod> {
        self.file
            .iter()
            .copied()
            .map(|method| ScopedResolutionMethod::new(method, ResolutionScope::File))
            .chain(
                self.block
                    .iter()
                    .copied()
                    .map(|method| ScopedResolutionMethod::new(method, ResolutionScope::Block)),
            )
            .collect()
    }

    pub fn from_scoped(methods: Vec<ScopedResolutionMethod>) -> Self {
        let mut resolution_methods = Self::default();
        for method in methods {
            match method.scope {
                ResolutionScope::File => resolution_methods.file.push(method.method),
                ResolutionScope::Block => resolution_methods.block.push(method.method),
            }
        }
        resolution_methods
    }
}

impl Serialize for ResolutionMethods {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.scoped().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ResolutionMethods {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Vec::<ScopedResolutionMethod>::deserialize(deserializer).map(Self::from_scoped)
    }
}

impl PreviewKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Raw => "raw",
            Self::Text => "text",
            Self::Code => "code",
            Self::Image => "image",
            Self::Custom(value) => value,
        }
    }

    fn from_wire(value: &str) -> Self {
        match value {
            "raw" => Self::Raw,
            "text" => Self::Text,
            "code" => Self::Code,
            "image" => Self::Image,
            custom => Self::Custom(custom.to_string()),
        }
    }
}

impl_extensible_string_serde!(PreviewKind);

impl DiffKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::TextLines => "textLines",
            Self::Binary => "binary",
            Self::ImageMetadata => "imageMetadata",
            Self::Custom(value) => value,
        }
    }

    fn from_wire(value: &str) -> Self {
        match value {
            "textLines" => Self::TextLines,
            "binary" => Self::Binary,
            "imageMetadata" => Self::ImageMetadata,
            custom => Self::Custom(custom.to_string()),
        }
    }
}

impl_extensible_string_serde!(DiffKind);

impl ReconcileStatus {
    fn from_wire(value: &str) -> Option<Self> {
        match value {
            "unsupported" => Some(Self::Unsupported),
            "needs_manual_resolution" => Some(Self::NeedsManualResolution),
            "auto_resolvable" => Some(Self::AutoResolvable),
            _ => None,
        }
    }
}

impl_closed_string_serde!(ReconcileStatus);

impl InspectorValueType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::StringList => "stringList",
        }
    }

    fn from_wire(value: &str) -> Option<Self> {
        match value {
            "string" => Some(Self::String),
            "number" => Some(Self::Number),
            "boolean" => Some(Self::Boolean),
            "stringList" => Some(Self::StringList),
            _ => None,
        }
    }
}

impl_closed_string_serde!(InspectorValueType);

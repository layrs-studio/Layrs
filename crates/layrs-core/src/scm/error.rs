use std::fmt;

pub type Result<T> = std::result::Result<T, ScmError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScmError {
    InvalidId { kind: &'static str, value: String },
    InvalidPath(String),
    NonNfc(String),
    Duplicate { kind: &'static str, value: String },
    DuplicateStepParent(String),
    LayerHeadsNotAntichain,
    FileManifestChunksNotContiguous(String),
    FastCdcParameterOrder,
    InvalidField { field: &'static str, reason: String },
    IdentityMismatch { claimed: String, computed: String },
    MissingChunk(String),
    InvalidChunk(String),
    Serialization(String),
}

impl fmt::Display for ScmError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidId { kind, value } => write!(formatter, "invalid {kind}: {value}"),
            Self::InvalidPath(reason) => write!(formatter, "invalid repository path: {reason}"),
            Self::NonNfc(value) => write!(formatter, "string is not Unicode NFC: {value:?}"),
            Self::Duplicate { kind, value } => write!(formatter, "duplicate {kind}: {value}"),
            Self::DuplicateStepParent(value) => write!(formatter, "duplicate Step parent: {value}"),
            Self::LayerHeadsNotAntichain => write!(formatter, "Layer heads are not an antichain"),
            Self::FileManifestChunksNotContiguous(reason) => {
                write!(
                    formatter,
                    "FileManifest chunks are not contiguous: {reason}"
                )
            }
            Self::FastCdcParameterOrder => {
                write!(
                    formatter,
                    "FastCDC requires minBytes <= targetBytes <= maxBytes"
                )
            }
            Self::InvalidField { field, reason } => write!(formatter, "invalid {field}: {reason}"),
            Self::IdentityMismatch { claimed, computed } => {
                write!(
                    formatter,
                    "identity mismatch: claimed {claimed}, computed {computed}"
                )
            }
            Self::MissingChunk(id) => write!(formatter, "missing chunk {id}"),
            Self::InvalidChunk(reason) => write!(formatter, "invalid chunk: {reason}"),
            Self::Serialization(reason) => write!(formatter, "serialization failed: {reason}"),
        }
    }
}

impl ScmError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidId { .. } => "invalid_id",
            Self::InvalidPath(_) => "invalid_repo_path",
            Self::NonNfc(_) => "non_nfc_string",
            Self::Duplicate { .. } => "duplicate_value",
            Self::DuplicateStepParent(_) => "duplicate_step_parent",
            Self::LayerHeadsNotAntichain => "layer_heads_not_antichain",
            Self::FileManifestChunksNotContiguous(_) => "file_manifest_chunks_not_contiguous",
            Self::FastCdcParameterOrder => "fastcdc_parameter_order",
            Self::InvalidField { .. } => "invalid_field",
            Self::IdentityMismatch { .. } => "identity_mismatch",
            Self::MissingChunk(_) => "missing_chunk",
            Self::InvalidChunk(_) => "invalid_chunk",
            Self::Serialization(_) => "serialization_error",
        }
    }
}

impl std::error::Error for ScmError {}

impl serde::de::Error for ScmError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self::Serialization(message.to_string())
    }
}

impl From<serde_json::Error> for ScmError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error.to_string())
    }
}

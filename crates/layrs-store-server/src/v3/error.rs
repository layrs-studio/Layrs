use std::{fmt, io};

pub type V3StoreResult<T> = Result<T, V3StoreError>;

#[derive(Debug)]
pub enum V3StoreError {
    InvalidDescriptor(String),
    InvalidRange,
    LimitExceeded {
        resource: &'static str,
        limit: u64,
    },
    LengthMismatch {
        field: &'static str,
        expected: u64,
        actual: u64,
    },
    IdentityMismatch,
    CorruptObject(String),
    NotFound,
    InvalidCapability,
    ExpiredCapability,
    AuthorizationEpochChanged,
    CustomPolicyRootActive,
    ReleaseStateConflict,
    ReleaseGateRejected,
    CapabilityBindingMismatch,
    InvalidCursor,
    DuplicateRequestObject,
    UnsupportedUploadKind,
    Backend(String),
    Io(io::Error),
}

impl V3StoreError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidDescriptor(_) => "invalid_descriptor",
            Self::InvalidRange => "invalid_range",
            Self::LimitExceeded { .. } => "limit_exceeded",
            Self::LengthMismatch { .. } => "length_mismatch",
            Self::IdentityMismatch => "identity_mismatch",
            Self::CorruptObject(_) => "corrupt_object",
            Self::NotFound => "not_found",
            Self::InvalidCapability => "invalid_capability",
            Self::ExpiredCapability => "expired_capability",
            Self::AuthorizationEpochChanged => "authorization_epoch_changed",
            Self::CustomPolicyRootActive => "custom_policy_root_active",
            Self::ReleaseStateConflict => "stale_release_state",
            Self::ReleaseGateRejected => "release_gate_rejected",
            Self::CapabilityBindingMismatch => "capability_binding_mismatch",
            Self::InvalidCursor => "invalid_cursor",
            Self::DuplicateRequestObject => "duplicate_request_object",
            Self::UnsupportedUploadKind => "unsupported_upload_kind",
            Self::Backend(_) => "backend_error",
            Self::Io(_) => "io_error",
        }
    }
}

impl fmt::Display for V3StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDescriptor(reason) => write!(f, "invalid object descriptor: {reason}"),
            Self::InvalidRange => f.write_str("invalid byte range"),
            Self::LimitExceeded { resource, limit } => write!(f, "{resource} exceeds {limit}"),
            Self::LengthMismatch {
                field,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "{field} length mismatch: expected {expected}, got {actual}"
                )
            }
            Self::IdentityMismatch => f.write_str("canonical object identity mismatch"),
            Self::CorruptObject(reason) => write!(f, "corrupt CAS object: {reason}"),
            Self::NotFound => f.write_str("CAS object not found"),
            Self::InvalidCapability => f.write_str("invalid capability"),
            Self::ExpiredCapability => f.write_str("capability expired"),
            Self::AuthorizationEpochChanged => f.write_str("authorization epoch changed"),
            Self::CustomPolicyRootActive => f.write_str(
                "restore the exact managed policy root before using managed access APIs",
            ),
            Self::ReleaseStateConflict => f.write_str("release lifecycle state changed"),
            Self::ReleaseGateRejected => f.write_str("release protection rejected publication"),
            Self::CapabilityBindingMismatch => f.write_str("capability binding mismatch"),
            Self::InvalidCursor => f.write_str("invalid page cursor"),
            Self::DuplicateRequestObject => f.write_str("duplicate descriptor or claim"),
            Self::UnsupportedUploadKind => f.write_str("object kind is server-generated"),
            Self::Backend(reason) => write!(f, "backend failed: {reason}"),
            Self::Io(error) => write!(f, "I/O failed: {error}"),
        }
    }
}

impl std::error::Error for V3StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for V3StoreError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

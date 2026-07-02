use crate::digest::{BLAKE3_HEX_LEN, OBJECT_DIGEST_PREFIX};
use std::fmt;

pub type SyncResult<T> = Result<T, SyncValidationError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncValidationError {
    pub field: &'static str,
    pub message: &'static str,
}

impl SyncValidationError {
    pub const fn new(field: &'static str, message: &'static str) -> Self {
        Self { field, message }
    }
}

impl fmt::Display for SyncValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

impl std::error::Error for SyncValidationError {}

pub(crate) fn validate_non_empty(field: &'static str, value: &str) -> SyncResult<()> {
    if value.trim().is_empty() {
        return Err(SyncValidationError::new(field, "must not be empty"));
    }
    Ok(())
}

pub(crate) fn validate_digest(value: &str) -> SyncResult<()> {
    validate_non_empty("digest", value)?;
    if !value.contains(':') {
        return Err(SyncValidationError::new(
            "digest",
            "must include an algorithm prefix",
        ));
    }
    Ok(())
}

pub(crate) fn validate_object_digest(field: &'static str, value: &str) -> SyncResult<()> {
    validate_non_empty(field, value)?;
    let Some(hex) = value.strip_prefix(OBJECT_DIGEST_PREFIX) else {
        return Err(SyncValidationError::new(
            field,
            "must use the blake3:<hex> digest format",
        ));
    };
    if hex.len() != BLAKE3_HEX_LEN {
        return Err(SyncValidationError::new(
            field,
            "must contain a 64 character BLAKE3 hex digest",
        ));
    }
    if !hex
        .bytes()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(SyncValidationError::new(
            field,
            "must contain only lowercase hex",
        ));
    }
    Ok(())
}

pub(crate) fn validate_path(field: &'static str, value: &str) -> SyncResult<()> {
    validate_non_empty(field, value)?;
    if value.starts_with('/') || value.starts_with('\\') || value.contains("..") {
        return Err(SyncValidationError::new(
            field,
            "must be a relative normalized path",
        ));
    }
    if value
        .bytes()
        .any(|byte| byte == b'\\' || byte == 0 || byte < 0x20)
    {
        return Err(SyncValidationError::new(
            field,
            "contains unsupported characters",
        ));
    }
    Ok(())
}

pub(crate) fn push_line(bytes: &mut Vec<u8>, value: impl AsRef<str>) {
    bytes.extend_from_slice(value.as_ref().as_bytes());
    bytes.push(b'\n');
}

pub(crate) fn push_kv(bytes: &mut Vec<u8>, key: &str, value: impl fmt::Display) {
    push_line(bytes, format!("{key}={value}"));
}

pub(crate) fn push_optional_kv(bytes: &mut Vec<u8>, key: &str, value: Option<&str>) {
    push_kv(bytes, key, value.unwrap_or("-"));
}

use super::error::{Result, ScmError};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use unicode_normalization::UnicodeNormalization;

fn is_nfc(value: &str) -> bool {
    value.nfc().eq(value.chars())
}

fn validate_component(value: &str) -> Result<()> {
    if value.is_empty() || value == "." || value == ".." {
        return Err(ScmError::InvalidPath(
            "empty, dot and dot-dot components are forbidden".into(),
        ));
    }
    if value.chars().count() > 255 {
        return Err(ScmError::InvalidPath(
            "component exceeds 255 Unicode scalars".into(),
        ));
    }
    if value.contains(['/', '\\', '\0']) {
        return Err(ScmError::InvalidPath(
            "component contains a forbidden separator or NUL".into(),
        ));
    }
    if !is_nfc(value) {
        return Err(ScmError::NonNfc(value.to_owned()));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct RepoPathComponent(String);

impl RepoPathComponent {
    pub fn parse(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        validate_component(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RepoPathComponent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RepoPathComponent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct RepoPath(String);

impl RepoPath {
    pub fn parse(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() || value.chars().count() > 32_768 || value.starts_with('/') {
            return Err(ScmError::InvalidPath(
                "path must be a non-empty relative path".into(),
            ));
        }
        if value.len() >= 2
            && value.as_bytes()[0].is_ascii_alphabetic()
            && value.as_bytes()[1] == b':'
        {
            return Err(ScmError::InvalidPath("drive prefixes are forbidden".into()));
        }
        for component in value.split('/') {
            validate_component(component)?;
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RepoPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RepoPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

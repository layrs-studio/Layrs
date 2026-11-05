//! Closed checkout progress DATA in the existing projection_prejournal table.
//! No row grants filesystem authority. In particular an inode is usable only
//! after caller COMMIT and positive close, matching pinned native ownership.
//! This module performs no filesystem operation or transaction completion.
use super::{
    cancellation,
    import_validation::{self, ImportDatabaseKind},
    records::{
        Record,
        import::{ImportSource, ProviderImportKind},
        transition::{ProviderTransitionIntentRecord, TransitionBackendKind, TransitionState},
    },
    recovery::error::{Result, invalid},
};
use crate::projection::format;
use layrs_core::{
    projection::{PathSemantics, ReservedPaths},
    scm::{FileId, FileManifestId, RepoPath, canonical_json},
};
use layrs_vfs::Cancellation;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

mod archive;
mod codec;
mod inventory;
mod sql;
pub(in crate::projection::provider) use archive::read_archived;
pub(in crate::projection::provider) use sql::{
    AdvanceEffect, DeclareEffect, advance, declare, read,
};

const MAX_FILES: usize = 100_000;
const MAX_DIRECTORIES: usize = 100_000;
const MAX_ROWS: usize = MAX_FILES + MAX_DIRECTORIES + 2;
const MAX_PATH_BYTES: usize = 8 * 1024 * 1024;
const MAX_JOURNAL_BYTES: usize = 64 * 1024 * 1024;
const MAX_ROW_BYTES: usize = 8192;
const ROW_SCHEMA: &str = "layrs.provider-checkout-row.v1";
const HEADER_SCHEMA: &str = "layrs.provider-checkout-header.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection::provider) struct CheckoutFile {
    pub path: RepoPath,
    pub manifest_id: FileManifestId,
    pub file_id: FileId,
    pub bytes: u64,
    pub executable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection::provider) struct CheckoutHeader {
    pub schema_version: String,
    pub transaction_id: String,
    pub plan_digest: String,
    pub prefix_id: String,
    pub destination_proof_digest: String,
    pub image_digest: String,
    pub checkout_identity: String,
    pub control_identity: String,
    pub metadata_identity: String,
    pub parent_identity: String,
    pub family_root_identity: String,
    pub database_identity: String,
    pub format_file_identity: String,
    pub inventory_digest: String,
    pub files: u64,
    pub directories: u64,
    pub deletions: u64,
    pub file_bytes: u64,
    pub path_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::projection::provider) enum RowKey {
    Header,
    Stage,
    Directory(u32),
    File(u32),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection::provider) enum DirectoryState {
    Declared,
    Owned { identity: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "phase",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(in crate::projection::provider) enum FileState {
    Declared,
    Owned {
        inode: String,
    },
    Verified {
        inode: String,
    },
    Publishing {
        inode: String,
        parent_identity: String,
    },
    Published {
        inode: String,
        parent_identity: String,
    },
}
impl FileState {
    /// Only this phase permits restarting an unpublished TEMP after proving
    /// the durable inode through the native owner. This is DATA, not a handle.
    pub fn owned_temp_inode(&self) -> Option<&str> {
        match self {
            Self::Owned { inode } => Some(inode),
            _ => None,
        }
    }
    pub fn inode(&self) -> Option<&str> {
        match self {
            Self::Declared => None,
            Self::Owned { inode }
            | Self::Verified { inode }
            | Self::Publishing { inode, .. }
            | Self::Published { inode, .. } => Some(inode),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection::provider) enum CheckoutEntry {
    Header {
        header: CheckoutHeader,
    },
    Stage {
        state: DirectoryState,
    },
    Directory {
        ordinal: u32,
        path: RepoPath,
        state: DirectoryState,
    },
    File {
        ordinal: u32,
        file: CheckoutFile,
        state: FileState,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection::provider) struct CheckoutRow {
    schema_version: String,
    header_digest: String,
    pub entry: CheckoutEntry,
}
impl CheckoutRow {
    pub fn key(&self) -> RowKey {
        match &self.entry {
            CheckoutEntry::Header { .. } => RowKey::Header,
            CheckoutEntry::Stage { .. } => RowKey::Stage,
            CheckoutEntry::Directory { ordinal, .. } => RowKey::Directory(*ordinal),
            CheckoutEntry::File { ordinal, .. } => RowKey::File(*ordinal),
        }
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        codec::encode(self)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        codec::decode(bytes)
    }
    pub fn digest(&self) -> Result<String> {
        Ok(codec::hash(
            "layrs-provider-checkout-row-v1",
            &self.encode()?,
        ))
    }
    pub fn owned_directory(&self, identity: &str) -> Result<Self> {
        codec::directory_identity(identity)?;
        let mut next = self.clone();
        match &mut next.entry {
            CheckoutEntry::Stage { state } | CheckoutEntry::Directory { state, .. }
                if *state == DirectoryState::Declared =>
            {
                *state = DirectoryState::Owned {
                    identity: identity.into(),
                }
            }
            _ => return Err(invalid("checkout directory ownership requires Declared")),
        }
        Ok(next)
    }
    pub fn owned_file(&self, inode: &str) -> Result<Self> {
        codec::file_identity(inode)?;
        let mut next = self.clone();
        match &mut next.entry {
            CheckoutEntry::File { state, .. } if *state == FileState::Declared => {
                *state = FileState::Owned {
                    inode: inode.into(),
                }
            }
            _ => return Err(invalid("checkout file ownership requires Declared")),
        }
        Ok(next)
    }
    pub fn verified_file(&self) -> Result<Self> {
        let mut next = self.clone();
        match &mut next.entry {
            CheckoutEntry::File { state, .. } => match state {
                FileState::Owned { inode } => {
                    *state = FileState::Verified {
                        inode: inode.clone(),
                    }
                }
                _ => return Err(invalid("checkout verification requires Owned")),
            },
            _ => return Err(invalid("checkout verification requires file")),
        }
        Ok(next)
    }
    pub fn publishing_file(&self, parent_identity: &str) -> Result<Self> {
        codec::directory_identity(parent_identity)?;
        let mut next = self.clone();
        match &mut next.entry {
            CheckoutEntry::File { state, .. } => match state {
                FileState::Verified { inode } => {
                    *state = FileState::Publishing {
                        inode: inode.clone(),
                        parent_identity: parent_identity.into(),
                    }
                }
                _ => return Err(invalid("checkout publication requires Verified")),
            },
            _ => return Err(invalid("checkout publication requires file")),
        }
        Ok(next)
    }
    pub fn published_file(&self) -> Result<Self> {
        let mut next = self.clone();
        match &mut next.entry {
            CheckoutEntry::File { state, .. } => match state {
                FileState::Publishing {
                    inode,
                    parent_identity,
                } => {
                    *state = FileState::Published {
                        inode: inode.clone(),
                        parent_identity: parent_identity.clone(),
                    }
                }
                _ => return Err(invalid("checkout acknowledgement requires Publishing")),
            },
            _ => return Err(invalid("checkout acknowledgement requires file")),
        }
        Ok(next)
    }
}

/// An independently derived expected inventory. It authenticates neither the
/// input image nor any native identity; the physical driver retains those.
pub(in crate::projection::provider) struct ExpectedCheckout {
    marker: format::Marker,
    header: CheckoutHeader,
    header_digest: String,
    operation: String,
    files: Vec<CheckoutFile>,
    directories: Vec<RepoPath>,
    deletions: Vec<RepoPath>,
    declared_encoded_bytes: u64,
    maximum_encoded_bytes: u64,
}
impl ExpectedCheckout {
    /// Explicit admitted-ordinary Staging DATA. The caller authenticates the
    /// actual source/destination rows and retains the image and physical pins.
    /// Provider constructors and terminal Ready readers stay separate.
    pub fn ordinary(
        staging: &ProviderTransitionIntentRecord,
        files: &[CheckoutFile],
        deletions: &[RepoPath],
        image_digest: &str,
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        inventory::expected_ordinary(staging, files, deletions, image_digest, cancel)
    }

    pub fn new(
        staging: &ProviderTransitionIntentRecord,
        files: &[CheckoutFile],
        deletions: &[RepoPath],
        image_digest: &str,
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        inventory::expected(staging, files, deletions, image_digest, cancel)
    }
    pub fn header(&self) -> &CheckoutHeader {
        &self.header
    }
    /// Terminal DATA entry. This does not manufacture an earlier Staging row.
    pub fn from_ready(
        ready: &ProviderTransitionIntentRecord,
        files: &[CheckoutFile],
        deletions: &[RepoPath],
        image_digest: &str,
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        inventory::expected_ready(ready, files, deletions, image_digest, cancel)
    }
    /// Bootstrap only bounded current SQL inventory, then run the unchanged
    /// exact journal interpreter against its reconstructed terminal header.
    pub fn read_ready(
        c: &Connection,
        ready: &ProviderTransitionIntentRecord,
        deletions: &[RepoPath],
        cancel: Option<&Cancellation>,
    ) -> Result<(Self, CheckoutJournal)> {
        inventory::read_ready(c, ready, deletions, cancel)
    }
    pub fn marker(&self) -> &format::Marker {
        &self.marker
    }
    pub fn files(&self) -> &[CheckoutFile] {
        &self.files
    }
    pub fn directories(&self) -> &[RepoPath] {
        &self.directories
    }
    pub fn deletions(&self) -> &[RepoPath] {
        &self.deletions
    }
    pub fn operation(&self) -> &str {
        &self.operation
    }
    pub fn stage_relative_path(&self) -> String {
        format!(".layrs/checkout-{}", self.header.transaction_id)
    }
    pub fn temporary_name(&self, ordinal: usize) -> Result<String> {
        if ordinal >= self.files.len() {
            return Err(invalid("checkout temporary ordinal absent"));
        }
        Ok(format!("file-{ordinal:06}"))
    }
    pub fn declared_encoded_bytes(&self) -> u64 {
        self.declared_encoded_bytes
    }
    /// Maximum logical column bytes through Publishing/Published, excluding
    /// SQLite page geometry. The physical driver budgets this before effects.
    pub fn maximum_encoded_bytes(&self) -> u64 {
        self.maximum_encoded_bytes
    }
    pub fn declared_row(&self, key: RowKey) -> Result<CheckoutRow> {
        inventory::declared_row(self, key)
    }
    pub fn sql_key(&self, key: RowKey) -> Result<String> {
        inventory::sql_key(self, key)
    }
}

pub(in crate::projection::provider) struct CheckoutJournal {
    /// SQL key order: directories, files, header, stage.
    rows: Vec<CheckoutRow>,
    directories: usize,
    files: usize,
}
impl CheckoutJournal {
    pub fn rows(&self) -> &[CheckoutRow] {
        &self.rows
    }
    pub fn row(&self, key: RowKey) -> Result<&CheckoutRow> {
        let n = match key {
            RowKey::Directory(n) if (n as usize) < self.directories => n as usize,
            RowKey::File(n) if (n as usize) < self.files => self.directories + n as usize,
            RowKey::Header => self.directories + self.files,
            RowKey::Stage => self.directories + self.files + 1,
            _ => return Err(invalid("checkout journal key absent")),
        };
        self.rows
            .get(n)
            .ok_or_else(|| invalid("checkout journal row absent"))
    }
    pub fn is_complete(&self) -> bool {
        self.rows.iter().all(|r| match &r.entry {
            CheckoutEntry::Header { .. } => true,
            CheckoutEntry::Stage { state } | CheckoutEntry::Directory { state, .. } => {
                matches!(state, DirectoryState::Owned { .. })
            }
            CheckoutEntry::File { state, .. } => matches!(state, FileState::Published { .. }),
        })
    }
}

#[cfg(test)]
mod tests;

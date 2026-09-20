use super::canonical::{canonical_json, domain_hash, domain_separated_bytes};
use super::error::{Result, ScmError};
use super::ids::{ConflictId, FileId, ObjectId, SymlinkId, TombstoneId, TreeId};
use super::path::RepoPathComponent;
use super::serde_support::deserialize_present_nullable;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashSet;

const TREE_DOMAIN: &str = "layrs-tree-v1";
const SYMLINK_DOMAIN: &str = "layrs-symlink-v1";
const TOMBSTONE_DOMAIN: &str = "layrs-tombstone-v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TreeEntryTarget {
    File {
        #[serde(rename = "fileId")]
        file_id: FileId,
    },
    Tree {
        #[serde(rename = "treeId")]
        tree_id: TreeId,
    },
    Conflict {
        #[serde(rename = "conflictId")]
        conflict_id: ConflictId,
    },
    Symlink {
        #[serde(rename = "symlinkId")]
        symlink_id: SymlinkId,
    },
    Tombstone {
        #[serde(rename = "tombstoneId")]
        tombstone_id: TombstoneId,
    },
}

impl TreeEntryTarget {
    pub fn file(file_id: FileId) -> Self {
        Self::File { file_id }
    }

    pub fn tree(tree_id: TreeId) -> Self {
        Self::Tree { tree_id }
    }

    pub fn conflict(conflict_id: ConflictId) -> Self {
        Self::Conflict { conflict_id }
    }

    pub fn symlink(symlink_id: SymlinkId) -> Self {
        Self::Symlink { symlink_id }
    }

    pub fn tombstone(tombstone_id: TombstoneId) -> Self {
        Self::Tombstone { tombstone_id }
    }

    pub fn file_id(&self) -> Option<&FileId> {
        match self {
            Self::File { file_id } => Some(file_id),
            _ => None,
        }
    }

    pub fn tree_id(&self) -> Option<&TreeId> {
        match self {
            Self::Tree { tree_id } => Some(tree_id),
            _ => None,
        }
    }

    pub fn conflict_id(&self) -> Option<&ConflictId> {
        match self {
            Self::Conflict { conflict_id } => Some(conflict_id),
            _ => None,
        }
    }

    pub fn symlink_id(&self) -> Option<&SymlinkId> {
        match self {
            Self::Symlink { symlink_id } => Some(symlink_id),
            _ => None,
        }
    }

    pub fn tombstone_id(&self) -> Option<&TombstoneId> {
        match self {
            Self::Tombstone { tombstone_id } => Some(tombstone_id),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TreeEntry {
    name: RepoPathComponent,
    target: TreeEntryTarget,
    executable: bool,
}

impl TreeEntry {
    pub fn new(name: RepoPathComponent, target: TreeEntryTarget, executable: bool) -> Self {
        Self {
            name,
            target,
            executable,
        }
    }

    pub fn name(&self) -> &RepoPathComponent {
        &self.name
    }

    pub fn target(&self) -> &TreeEntryTarget {
        &self.target
    }

    pub fn executable(&self) -> bool {
        self.executable
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TreeEntryWire {
    name: RepoPathComponent,
    target: TreeEntryTarget,
    executable: bool,
}

impl<'de> Deserialize<'de> for TreeEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let wire = TreeEntryWire::deserialize(deserializer)?;
        Ok(Self::new(wire.name, wire.target, wire.executable))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    id: TreeId,
    entries: Vec<TreeEntry>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TreePayload<'a> {
    schema_version: &'static str,
    entries: &'a [TreeEntry],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TreeRef<'a> {
    schema_version: &'static str,
    tree_id: &'a TreeId,
    entries: &'a [TreeEntry],
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TreeWire {
    schema_version: String,
    tree_id: TreeId,
    entries: Vec<TreeEntry>,
}

impl Tree {
    pub fn new(mut entries: Vec<TreeEntry>) -> Result<Self> {
        let mut names = HashSet::with_capacity(entries.len());
        for entry in &entries {
            if !names.insert(entry.name.as_str().to_owned()) {
                return Err(ScmError::Duplicate {
                    kind: "TreeEntry name",
                    value: entry.name.to_string(),
                });
            }
        }
        entries.sort_by(|left, right| {
            left.name
                .as_str()
                .as_bytes()
                .cmp(right.name.as_str().as_bytes())
        });
        let payload = canonical_json(&TreePayload {
            schema_version: "layrs.tree.v1",
            entries: &entries,
        })?;
        let id = TreeId::from_digest(domain_hash(TREE_DOMAIN, &payload));
        Ok(Self { id, entries })
    }

    pub fn id(&self) -> &TreeId {
        &self.id
    }

    pub fn entries(&self) -> &[TreeEntry] {
        &self.entries
    }

    pub fn canonical_payload(&self) -> Result<Vec<u8>> {
        canonical_json(&TreePayload {
            schema_version: "layrs.tree.v1",
            entries: &self.entries,
        })
    }

    pub fn identity_bytes(&self) -> Result<Vec<u8>> {
        Ok(domain_separated_bytes(
            TREE_DOMAIN,
            &self.canonical_payload()?,
        ))
    }
}

impl Serialize for Tree {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        TreeRef {
            schema_version: "layrs.tree.v1",
            tree_id: &self.id,
            entries: &self.entries,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Tree {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let wire = TreeWire::deserialize(deserializer)?;
        if wire.schema_version != "layrs.tree.v1" {
            return Err(serde::de::Error::custom("unsupported Tree schemaVersion"));
        }
        let value = Self::new(wire.entries).map_err(serde::de::Error::custom)?;
        if value.id != wire.tree_id {
            return Err(serde::de::Error::custom(ScmError::IdentityMismatch {
                claimed: wire.tree_id.to_string(),
                computed: value.id.to_string(),
            }));
        }
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symlink {
    id: SymlinkId,
    target: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SymlinkPayload<'a> {
    schema_version: &'static str,
    target: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SymlinkRef<'a> {
    schema_version: &'static str,
    symlink_id: &'a SymlinkId,
    target: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SymlinkWire {
    schema_version: String,
    symlink_id: SymlinkId,
    target: String,
}

impl Symlink {
    pub fn new(target: impl Into<String>) -> Result<Self> {
        let target = target.into();
        if target.is_empty() || target.chars().count() > 4096 {
            return Err(ScmError::InvalidField {
                field: "target",
                reason: "symlink target must contain 1..=4096 bytes".into(),
            });
        }
        let payload = canonical_json(&SymlinkPayload {
            schema_version: "layrs.symlink.v1",
            target: &target,
        })?;
        let id = SymlinkId::from_digest(domain_hash(SYMLINK_DOMAIN, &payload));
        Ok(Self { id, target })
    }

    pub fn id(&self) -> &SymlinkId {
        &self.id
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn identity_bytes(&self) -> Result<Vec<u8>> {
        let payload = canonical_json(&SymlinkPayload {
            schema_version: "layrs.symlink.v1",
            target: &self.target,
        })?;
        Ok(domain_separated_bytes(SYMLINK_DOMAIN, &payload))
    }
}

impl Serialize for Symlink {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        SymlinkRef {
            schema_version: "layrs.symlink.v1",
            symlink_id: &self.id,
            target: &self.target,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Symlink {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let wire = SymlinkWire::deserialize(deserializer)?;
        if wire.schema_version != "layrs.symlink.v1" {
            return Err(serde::de::Error::custom(
                "unsupported Symlink schemaVersion",
            ));
        }
        let value = Self::new(wire.target).map_err(serde::de::Error::custom)?;
        if value.id != wire.symlink_id {
            return Err(serde::de::Error::custom(ScmError::IdentityMismatch {
                claimed: wire.symlink_id.to_string(),
                computed: value.id.to_string(),
            }));
        }
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tombstone {
    id: TombstoneId,
    previous_object_id: Option<ObjectId>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TombstonePayload<'a> {
    schema_version: &'static str,
    previous_object_id: &'a Option<ObjectId>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TombstoneRef<'a> {
    schema_version: &'static str,
    tombstone_id: &'a TombstoneId,
    previous_object_id: &'a Option<ObjectId>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TombstoneWire {
    schema_version: String,
    tombstone_id: TombstoneId,
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    previous_object_id: Option<Option<ObjectId>>,
}

impl Tombstone {
    pub fn new(previous_object_id: Option<ObjectId>) -> Result<Self> {
        let payload = canonical_json(&TombstonePayload {
            schema_version: "layrs.tombstone.v1",
            previous_object_id: &previous_object_id,
        })?;
        let id = TombstoneId::from_digest(domain_hash(TOMBSTONE_DOMAIN, &payload));
        Ok(Self {
            id,
            previous_object_id,
        })
    }

    pub fn id(&self) -> &TombstoneId {
        &self.id
    }

    pub fn previous_object_id(&self) -> Option<&ObjectId> {
        self.previous_object_id.as_ref()
    }

    pub fn identity_bytes(&self) -> Result<Vec<u8>> {
        let payload = canonical_json(&TombstonePayload {
            schema_version: "layrs.tombstone.v1",
            previous_object_id: &self.previous_object_id,
        })?;
        Ok(domain_separated_bytes(TOMBSTONE_DOMAIN, &payload))
    }
}

impl Serialize for Tombstone {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        TombstoneRef {
            schema_version: "layrs.tombstone.v1",
            tombstone_id: &self.id,
            previous_object_id: &self.previous_object_id,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Tombstone {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let wire = TombstoneWire::deserialize(deserializer)?;
        if wire.schema_version != "layrs.tombstone.v1" {
            return Err(serde::de::Error::custom(
                "unsupported Tombstone schemaVersion",
            ));
        }
        let previous = wire
            .previous_object_id
            .ok_or_else(|| serde::de::Error::missing_field("previousObjectId"))?;
        let value = Self::new(previous).map_err(serde::de::Error::custom)?;
        if value.id != wire.tombstone_id {
            return Err(serde::de::Error::custom(ScmError::IdentityMismatch {
                claimed: wire.tombstone_id.to_string(),
                computed: value.id.to_string(),
            }));
        }
        Ok(value)
    }
}

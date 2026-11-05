//! Rooms (ADR 0025): lightweight working folders, one per task or agent.
//!
//! A Room is a Worktree with its own Layer, recorded in the Store's room
//! registry (`.layrs/rooms/<name>.json`). Its folder lives outside the project
//! tree by default so tools scanning the project never see Room copies.
use super::{NativeClientError, NativeWorkspace, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// How a Room presents its folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomKind {
    /// VFS: files appear when read and are copied on first write.
    Virtual,
    /// A complete physical folder.
    Physical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoomRecord {
    pub name: String,
    pub kind: RoomKind,
    pub path: String,
    pub layer_id: String,
    pub layer_name: String,
    pub source_layer_id: Option<String>,
    pub base_step_id: String,
    pub worktree_id: Option<String>,
    /// VFS host ticket (virtual Rooms), used by Room commands to reach the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<serde_json::Value>,
    pub created_at: String,
    /// Room Step last woven into its target Layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_step_id: Option<String>,
}

/// 1-64 characters of `[A-Za-z0-9._-]`, not starting with `.` or `-`: safe as
/// a file name on every platform and inside a Layer name.
pub fn validate_room_name(name: &str) -> Result<()> {
    let valid = (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        && !name.starts_with(['.', '-'])
        && !name.eq_ignore_ascii_case("con")
        && !name.eq_ignore_ascii_case("nul")
        && !name.eq_ignore_ascii_case("aux")
        && !name.eq_ignore_ascii_case("prn");
    if valid {
        Ok(())
    } else {
        Err(NativeClientError::InvalidState(
            "Room names use 1-64 letters, digits, '.', '_' or '-' and do not start with '.' or '-'",
        ))
    }
}

/// The Layer name a Room works on.
pub fn room_layer_name(name: &str) -> String {
    format!("room/{name}")
}

/// Built-in parent of all Room folders for a Space, outside any project tree
/// (`NativeWorkspace::rooms_location` applies the project's or the user's
/// choice first):
/// Windows `%LOCALAPPDATA%\Layrs\rooms\<space>`, Linux
/// `$XDG_DATA_HOME/layrs/rooms/<space>`, macOS
/// `~/Library/Application Support/Layrs/rooms/<space>`. A Room must share
/// its Store's volume (journaled materialization renames), so when that user
/// directory is on another volume the Rooms go next to the project instead:
/// `<project parent>/<project>.rooms/`.
pub fn default_rooms_parent(
    project_root: &Path,
    space_name: &str,
    space_id: &str,
) -> Result<PathBuf> {
    let base = default_rooms_base()?;
    if !same_volume_prefix(&base, project_root) {
        let name = project_root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "project".into());
        let parent = project_root
            .parent()
            .ok_or(NativeClientError::InvalidState(
                "project folder has no parent",
            ))?;
        return Ok(parent.join(format!("{name}.rooms")));
    }
    Ok(base.join(space_folder_name(space_name, space_id)))
}

/// A Space's folder name under a shared Rooms parent: a readable slug of its
/// name and a short hash of its id, stable and distinct per Space.
pub(super) fn space_folder_name(space_name: &str, space_id: &str) -> String {
    let slug: String = space_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug: String = slug.chars().take(32).collect();
    let short = blake3::hash(space_id.as_bytes()).to_hex()[..8].to_owned();
    if slug.is_empty() {
        short
    } else {
        format!("{slug}-{short}")
    }
}

/// Same drive (Windows) or same mount guess (Unix: always true; the Store
/// creation check is authoritative and reports any mismatch).
fn same_volume_prefix(a: &Path, b: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::path::Component;
        let prefix = |p: &Path| match p.components().next() {
            Some(Component::Prefix(prefix)) => {
                Some(prefix.as_os_str().to_string_lossy().to_ascii_lowercase())
            }
            _ => None,
        };
        prefix(a).is_some() && prefix(a) == prefix(b)
    }
    #[cfg(not(windows))]
    {
        let _ = (a, b);
        true
    }
}

fn default_rooms_base() -> Result<PathBuf> {
    let missing = || NativeClientError::Missing("user data directory for Rooms");
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(|base| PathBuf::from(base).join("Layrs").join("rooms"))
            .ok_or_else(missing)
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(|home| {
                PathBuf::from(home)
                    .join("Library")
                    .join("Application Support")
                    .join("Layrs")
                    .join("rooms")
            })
            .ok_or_else(missing)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join(".local").join("share"))
            })
            .map(|base| base.join("layrs").join("rooms"))
            .ok_or_else(missing)
    }
}

impl NativeWorkspace {
    /// `.layrs/rooms` of the Store this Worktree belongs to.
    pub fn rooms_registry(&self) -> PathBuf {
        self.store_root().join("rooms")
    }

    pub fn rooms(&self) -> Result<Vec<RoomRecord>> {
        let directory = self.rooms_registry();
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut rooms = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            rooms.push(read_record(&path)?);
        }
        rooms.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(rooms)
    }

    pub fn room(&self, name: &str) -> Result<RoomRecord> {
        validate_room_name(name)?;
        let path = self.rooms_registry().join(format!("{name}.json"));
        match std::fs::symlink_metadata(&path) {
            Ok(_) => read_record(&path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(NativeClientError::Missing("Room"))
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Records a Room; refuses to replace an existing record of that name.
    pub fn record_room(&self, record: &RoomRecord) -> Result<()> {
        validate_room_name(&record.name)?;
        let directory = self.rooms_registry();
        std::fs::create_dir_all(&directory)?;
        let target = directory.join(format!("{}.json", record.name));
        let staged = directory.join(format!(".{}.{}.tmp", record.name, uuid::Uuid::new_v4()));
        let bytes = serde_json::to_vec_pretty(record)
            .map_err(|_| NativeClientError::InvalidState("Room record encoding failed"))?;
        {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staged)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        // Hard link then unlink: never replaces an existing record.
        let linked = std::fs::hard_link(&staged, &target);
        let _ = std::fs::remove_file(&staged);
        match linked {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Err(
                NativeClientError::InvalidState("a Room with this name already exists"),
            ),
            Err(error) => Err(error.into()),
        }
    }

    /// Replaces the record of an existing Room (same name).
    pub fn update_room(&self, record: &RoomRecord) -> Result<()> {
        self.room(&record.name)?;
        let directory = self.rooms_registry();
        let target = directory.join(format!("{}.json", record.name));
        let staged = directory.join(format!(".{}.{}.tmp", record.name, uuid::Uuid::new_v4()));
        let bytes = serde_json::to_vec_pretty(record)
            .map_err(|_| NativeClientError::InvalidState("Room record encoding failed"))?;
        {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staged)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        std::fs::rename(&staged, &target).inspect_err(|_| {
            let _ = std::fs::remove_file(&staged);
        })?;
        Ok(())
    }

    /// Retires a Room's Worktree: its history stays, but it is no longer
    /// listed, discovered or protected as a registered folder.
    pub fn retire_worktree(&self, id: &str) -> Result<()> {
        let id = layrs_core::scm::WorktreeId::parse(id.to_owned())?;
        if &id == self.worktree_id() {
            return Err(NativeClientError::InvalidState(
                "cannot retire the current folder",
            ));
        }
        layrs_store_local::scm_v1::worktrees::retire_worktree(
            self.store_root(),
            self.store_identity(),
            self.metadata_identity(),
            &id,
        )?;
        Ok(())
    }

    /// Deletes a retired Room Worktree's change index (a cache).
    pub fn forget_change_index(&self, id: &str) -> Result<()> {
        let id = layrs_core::scm::WorktreeId::parse(id.to_owned())?;
        layrs_store_local::scm_v1::change_index::ChangeIndex::remove(self.store_root(), &id)?;
        Ok(())
    }

    pub fn forget_room(&self, name: &str) -> Result<()> {
        validate_room_name(name)?;
        match std::fs::remove_file(self.rooms_registry().join(format!("{name}.json"))) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(NativeClientError::Missing("Room"))
            }
            Err(error) => Err(error.into()),
        }
    }
}

/// Unregisters a virtual Room's disconnected VFS root so its folder can be
/// deleted (its work is already captured in the Store).
/// Whether `root` still holds a virtual Room's control file.
pub fn is_virtual_room_root(root: &Path) -> bool {
    #[cfg(windows)]
    {
        layrs_vfs::read_control_file(root).is_ok()
    }
    #[cfg(not(windows))]
    {
        let _ = root;
        false
    }
}

pub fn unregister_virtual_room(root: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        layrs_vfs::unregister_root(root)
            .map_err(|_| NativeClientError::InvalidState("VFS root could not be unregistered"))
    }
    #[cfg(not(windows))]
    {
        let _ = root;
        Err(NativeClientError::InvalidState(
            "virtual Rooms need Windows",
        ))
    }
}

/// When `start` is inside a virtual Room: that Room's folder and the folder of
/// the project whose Store it uses (where the Room registry lives).
pub fn virtual_room_origin(start: &Path) -> Result<Option<(PathBuf, PathBuf)>> {
    #[cfg(windows)]
    {
        let start = std::path::absolute(start)?;
        for root in start.ancestors() {
            match std::fs::symlink_metadata(root.join(".layrs")) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                    return Ok(None);
                }
                Ok(_) => {
                    let Ok((bytes, _)) = layrs_vfs::read_control_file(root) else {
                        return Ok(None);
                    };
                    let Ok(locator) = super::VfsWorktreeLocator::decode(&bytes) else {
                        return Ok(None);
                    };
                    let project = locator
                        .source
                        .store_root
                        .parent()
                        .ok_or(NativeClientError::InvalidState(
                            "Store has no project folder",
                        ))?
                        .to_path_buf();
                    return Ok(Some((root.to_path_buf(), project)));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(None)
    }
    #[cfg(not(windows))]
    {
        let _ = start;
        Ok(None)
    }
}

fn read_record(path: &Path) -> Result<RoomRecord> {
    let bytes = std::fs::read(path)?;
    if bytes.len() > 256 * 1024 {
        return Err(NativeClientError::InvalidState("Room record is too large"));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| NativeClientError::InvalidState("Room record is invalid"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_names_are_portable_file_and_layer_names() {
        for good in ["agent-a", "fix_42", "Feature.1", "a"] {
            validate_room_name(good).unwrap();
        }
        for bad in ["", ".hidden", "-x", "a/b", "a b", "con", &"x".repeat(65)] {
            assert!(validate_room_name(bad).is_err(), "{bad:?}");
        }
        assert_eq!(room_layer_name("agent-a"), "room/agent-a");
    }

    #[test]
    fn default_parent_is_outside_projects_and_stable_per_space() {
        let project = default_rooms_base().unwrap().join("..").join("project");
        let a = default_rooms_parent(&project, "My Game!", "space:1").unwrap();
        let b = default_rooms_parent(&project, "My Game!", "space:1").unwrap();
        let c = default_rooms_parent(&project, "My Game!", "space:2").unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
        let leaf = a.file_name().unwrap().to_str().unwrap().to_owned();
        assert!(leaf.starts_with("my-game-"), "{leaf}");
        assert!(a.parent().unwrap().ends_with("rooms"));
        #[cfg(windows)]
        {
            let base = default_rooms_base().unwrap();
            let other = if base.starts_with("C:") {
                r"Q:\work\game"
            } else {
                r"C:\work\game"
            };
            assert_eq!(
                default_rooms_parent(Path::new(other), "x", "space:1").unwrap(),
                Path::new(other).parent().unwrap().join("game.rooms")
            );
        }
    }
}

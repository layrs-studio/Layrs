//! Where new Room folders go (ADR 0025). An explicit folder wins; otherwise
//! the project's choice, then the user's choice, then the built-in default
//! (`default_rooms_parent`).
//!
//! - The project's choice is `.layrs/rooms/default-location` in its Store,
//!   shared by every Worktree of that Store. It is not a `*.json` Room record.
//! - The user's choice lives in the user configuration folder and applies to
//!   every project. Each project gets its own subfolder there, so Rooms of
//!   different projects never collide.
use super::{rooms, NativeClientError, NativeWorkspace, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Which setting chose a Rooms location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomsLocationSource {
    Project,
    User,
    Default,
}

/// The folder that holds new Rooms of a project, and the setting that chose it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomsLocation {
    pub parent: PathBuf,
    pub source: RoomsLocationSource,
}

const SCHEMA: &str = "layrs.rooms-location.v1";
const PROJECT_FILE: &str = "default-location";
const MAX_SETTING_BYTES: u64 = 64 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LocationSetting {
    schema: String,
    parent: String,
}

/// The user's Rooms location setting file.
pub fn user_rooms_location_file() -> Result<PathBuf> {
    let missing = || NativeClientError::Missing("user configuration directory for Rooms");
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA")
            .map(|base| {
                PathBuf::from(base)
                    .join("Layrs")
                    .join("rooms-location.json")
            })
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
                    .join("rooms-location.json")
            })
            .ok_or_else(missing)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .map(|base| base.join("layrs").join("rooms-location.json"))
            .ok_or_else(missing)
    }
}

/// The user's chosen parent of all projects' Rooms, if any.
pub fn user_rooms_location() -> Result<Option<PathBuf>> {
    read_setting(&user_rooms_location_file()?)
}

/// Sets (or with `None` clears) the user's Rooms location; returns the
/// absolute folder now set.
pub fn set_user_rooms_location(parent: Option<&Path>) -> Result<Option<PathBuf>> {
    write_setting(&user_rooms_location_file()?, parent)
}

impl NativeWorkspace {
    fn project_rooms_location_file(&self) -> PathBuf {
        self.rooms_registry().join(PROJECT_FILE)
    }

    /// This project's chosen parent of its Rooms, if any.
    pub fn project_rooms_location(&self) -> Result<Option<PathBuf>> {
        read_setting(&self.project_rooms_location_file())
    }

    /// Sets (or with `None` clears) this project's Rooms location; returns the
    /// absolute folder now set.
    pub fn set_project_rooms_location(&self, parent: Option<&Path>) -> Result<Option<PathBuf>> {
        write_setting(&self.project_rooms_location_file(), parent)
    }

    /// Where a new Room of this project goes when no folder is given.
    pub fn rooms_location(&self, space_name: &str, space_id: &str) -> Result<RoomsLocation> {
        if let Some(parent) = self.project_rooms_location()? {
            return Ok(RoomsLocation {
                parent,
                source: RoomsLocationSource::Project,
            });
        }
        if let Some(base) = user_rooms_location()? {
            return Ok(RoomsLocation {
                parent: base.join(rooms::space_folder_name(space_name, space_id)),
                source: RoomsLocationSource::User,
            });
        }
        Ok(RoomsLocation {
            parent: rooms::default_rooms_parent(&self.checkout_root()?, space_name, space_id)?,
            source: RoomsLocationSource::Default,
        })
    }
}

fn read_setting(path: &Path) -> Result<Option<PathBuf>> {
    let bytes = match std::fs::metadata(path) {
        Ok(metadata) if metadata.len() > MAX_SETTING_BYTES => {
            return Err(NativeClientError::InvalidState(
                "Rooms location setting is too large",
            ));
        }
        Ok(_) => std::fs::read(path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let invalid = || NativeClientError::InvalidState("Rooms location setting is invalid");
    let setting: LocationSetting = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    let parent = PathBuf::from(setting.parent);
    if setting.schema != SCHEMA || !parent.is_absolute() {
        return Err(invalid());
    }
    Ok(Some(parent))
}

fn write_setting(path: &Path, parent: Option<&Path>) -> Result<Option<PathBuf>> {
    let Some(parent) = parent else {
        return match std::fs::remove_file(path) {
            Ok(()) => Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        };
    };
    let parent = std::path::absolute(parent)?;
    let text = parent
        .to_str()
        .ok_or(NativeClientError::InvalidState(
            "Rooms location must be UTF-8",
        ))?
        .to_owned();
    if std::fs::symlink_metadata(&parent).is_ok_and(|metadata| !metadata.is_dir()) {
        return Err(NativeClientError::InvalidState(
            "Rooms location exists and is not a folder",
        ));
    }
    let directory = path.parent().ok_or(NativeClientError::InvalidState(
        "setting file has no folder",
    ))?;
    std::fs::create_dir_all(directory)?;
    let bytes = serde_json::to_vec_pretty(&LocationSetting {
        schema: SCHEMA.into(),
        parent: text,
    })
    .map_err(|_| NativeClientError::InvalidState("Rooms location encoding failed"))?;
    let staged = directory.join(format!(".rooms-location.{}.tmp", uuid::Uuid::new_v4()));
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    // A setting, not history: replacing the previous choice is intended.
    if let Err(error) = std::fs::rename(&staged, path) {
        let _ = std::fs::remove_file(&staged);
        return Err(error.into());
    }
    Ok(Some(parent))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("layrs-rooms-location-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn setting_round_trips_and_reset_removes_it() {
        let dir = scratch();
        let file = dir.join("nested").join(PROJECT_FILE);
        assert_eq!(read_setting(&file).unwrap(), None);
        let chosen = dir.join("rooms-here");
        assert_eq!(
            write_setting(&file, Some(&chosen)).unwrap(),
            Some(chosen.clone())
        );
        assert_eq!(read_setting(&file).unwrap(), Some(chosen));
        let replaced = dir.join("elsewhere");
        write_setting(&file, Some(&replaced)).unwrap();
        assert_eq!(read_setting(&file).unwrap(), Some(replaced));
        assert_eq!(write_setting(&file, None).unwrap(), None);
        assert_eq!(read_setting(&file).unwrap(), None);
        assert_eq!(write_setting(&file, None).unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn invalid_or_relative_settings_and_file_targets_are_refused() {
        let dir = scratch();
        let file = dir.join(PROJECT_FILE);
        std::fs::write(
            &file,
            br#"{"schema":"layrs.rooms-location.v1","parent":"relative/rooms"}"#,
        )
        .unwrap();
        assert!(read_setting(&file).is_err());
        std::fs::write(&file, br#"{"schema":"other","parent":"C:/x"}"#).unwrap();
        assert!(read_setting(&file).is_err());
        std::fs::write(&file, b"not json").unwrap();
        assert!(read_setting(&file).is_err());
        let occupied = dir.join("a-file");
        std::fs::write(&occupied, b"x").unwrap();
        assert!(write_setting(&dir.join("other-setting"), Some(&occupied)).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

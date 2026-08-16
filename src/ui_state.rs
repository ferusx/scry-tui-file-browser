// SPDX-License-Identifier: BSD-3-Clause

use serde::{Deserialize, Serialize};

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

const STATE_DIRECTORY_NAME: &str = "scry";

const UI_STATE_FILENAME: &str = "ui-state.toml";

const UI_STATE_PART_FILENAME: &str = "ui-state.toml.part";

/*
 * Small persistent interface preferences that do not belong to session
 * restoration.
 *
 * These values remain meaningful even when [session].restore_session is false.
 */
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    /*
     * Permanently suppress the local confirmation-range Expand All warning.
     *
     * This never suppresses:
     *
     * - the configured maximum;
     * - the refusal dialog above that maximum;
     * - SSH warnings.
     */
    pub disable_local_expand_all_warning: bool,
}

/*
 * A missing state file is normal and returns the built-in defaults.
 *
 * Malformed or unreadable files are reported so startup can warn and continue.
 */
pub fn load() -> io::Result<UiState> {
    let path = ui_state_file_path()?;

    let content = match fs::read_to_string(&path) {
        Ok(content) => content,

        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(UiState::default());
        }

        Err(error) => {
            return Err(io::Error::new(
                error.kind(),
                format!("unable to read {}: {}", path.display(), error),
            ));
        }
    };

    toml::from_str::<UiState>(&content).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unable to parse {}: {}", path.display(), error),
        )
    })
}

/*
 * Publish through a temporary file so an interrupted write cannot leave a
 * truncated live preference file.
 */
pub fn save(state: &UiState) -> io::Result<PathBuf> {
    let path = ui_state_file_path()?;

    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("UI state path has no parent directory"))?;

    fs::create_dir_all(directory)?;

    let part_path = directory.join(UI_STATE_PART_FILENAME);

    let serialized = toml::to_string_pretty(state).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unable to serialize UI state: {}", error),
        )
    })?;

    fs::write(&part_path, serialized).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("unable to write {}: {}", part_path.display(), error),
        )
    })?;

    replace_atomically(&part_path, &path)?;

    Ok(path)
}

pub fn ui_state_file_path() -> io::Result<PathBuf> {
    Ok(ui_state_directory()?.join(UI_STATE_FILENAME))
}

fn ui_state_directory() -> io::Result<PathBuf> {
    if let Some(xdg_state_home) = env::var_os("XDG_STATE_HOME")
        && !xdg_state_home.is_empty()
    {
        return Ok(PathBuf::from(xdg_state_home).join(STATE_DIRECTORY_NAME));
    }

    let home = env::var_os("HOME").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "neither XDG_STATE_HOME nor HOME is available",
        )
    })?;

    Ok(PathBuf::from(home)
        .join(".local")
        .join("state")
        .join(STATE_DIRECTORY_NAME))
}

fn replace_atomically(part_path: &Path, destination_path: &Path) -> io::Result<()> {
    match fs::rename(part_path, destination_path) {
        Ok(()) => Ok(()),

        Err(first_error) if destination_path.exists() => {
            fs::remove_file(destination_path)?;

            fs::rename(part_path, destination_path).map_err(|second_error| {
                io::Error::new(
                    second_error.kind(),
                    format!(
                        "unable to publish {} after rename failed ({}): {}",
                        destination_path.display(),
                        first_error,
                        second_error,
                    ),
                )
            })
        }

        Err(error) => Err(io::Error::new(
            error.kind(),
            format!(
                "unable to publish {}: {}",
                destination_path.display(),
                error,
            ),
        )),
    }
}

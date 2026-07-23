// SPDX-License-Identifier: BSD-3-Clause

use serde::{Deserialize, Serialize};

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

const SESSION_DIRECTORY_NAME: &str = "scry";

const SESSION_FILENAME: &str = "session.json";

const SESSION_PART_FILENAME: &str = "session.json.part";

pub const SESSION_FORMAT_VERSION: u32 = 1;

/*
 * Persistent browser state written when session restoration is enabled.
 *
 * Temporary UI state is deliberately excluded:
 *
 * - open overlays and dialogs;
 * - notifications and errors;
 * - active transfers;
 * - scanner and worker state;
 * - deletion confirmations.
 */
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub version: u32,

    pub source: SessionSource,

    pub selected_path: Option<PathBuf>,

    pub list_offset: usize,

    pub query: String,

    pub view_mode: String,

    pub search_mode: String,

    pub recursive: bool,

    pub entry_filter: String,

    pub sort_mode: String,

    pub reverse: bool,

    pub show_hidden: bool,

    pub show_icons: bool,

    pub show_details: bool,

    pub show_selection: bool,

    pub show_columns: bool,

    pub show_permissions: bool,

    pub show_size: bool,

    pub show_date: bool,

    pub show_user: bool,

    /*
     * Persistent SSH batch marks.
     *
     * Full paths, filenames, and byte sizes are retained because App's ordinary
     * directory entries may not contain these files after navigation or filtering.
     */
    pub marked_files: Vec<SessionMarkedFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SessionSource {
    Local {
        directory: PathBuf,

        home_directory: PathBuf,
    },

    Ssh {
        host: String,

        user: Option<String>,

        port: u16,

        identity_file: Option<PathBuf>,

        directory: PathBuf,

        home_directory: PathBuf,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMarkedFile {
    pub path: PathBuf,

    pub filename: String,

    pub size_bytes: u64,
}

impl SessionState {
    pub fn is_supported(&self) -> bool {
        self.version == SESSION_FORMAT_VERSION
    }
}

/*
 * Load a previously saved session.
 *
 * A missing file is normal and returns Ok(None). Malformed or unreadable state
 * is reported to the caller so startup can warn and continue normally.
 */
pub fn load() -> io::Result<Option<SessionState>> {
    let path = session_file_path()?;

    let content = match fs::read_to_string(&path) {
        Ok(content) => content,

        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(None);
        }

        Err(error) => {
            return Err(io::Error::new(
                error.kind(),
                format!("unable to read {}: {}", path.display(), error),
            ));
        }
    };

    let state = serde_json::from_str::<SessionState>(&content).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unable to parse {}: {}", path.display(), error),
        )
    })?;

    Ok(Some(state))
}

/*
 * Save through a temporary file and publish with rename.
 *
 * The live session file is never truncated in place. An interrupted write
 * therefore leaves either the previous valid state or an unpublished .part file.
 */
pub fn save(state: &SessionState) -> io::Result<PathBuf> {
    let path = session_file_path()?;

    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("session path has no parent directory"))?;

    fs::create_dir_all(directory)?;

    let part_path = directory.join(SESSION_PART_FILENAME);

    let serialized = serde_json::to_vec_pretty(state).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unable to serialize session state: {}", error),
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

pub fn session_file_path() -> io::Result<PathBuf> {
    Ok(session_state_directory()?.join(SESSION_FILENAME))
}

fn session_state_directory() -> io::Result<PathBuf> {
    if let Some(xdg_state_home) = env::var_os("XDG_STATE_HOME") {
        if !xdg_state_home.is_empty() {
            return Ok(PathBuf::from(xdg_state_home).join(SESSION_DIRECTORY_NAME));
        }
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
        .join(SESSION_DIRECTORY_NAME))
}

fn replace_atomically(part_path: &Path, destination_path: &Path) -> io::Result<()> {
    /*
     * Unix rename replaces an existing destination atomically.
     *
     * The explicit removal fallback is retained for platforms or filesystems
     * that reject replacement through rename.
     */
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

#[cfg(test)]
mod tests {
    use super::{SESSION_FORMAT_VERSION, SessionMarkedFile, SessionSource, SessionState};

    use std::path::PathBuf;

    #[test]
    fn session_state_round_trips_through_json() {
        let state = SessionState {
            version: SESSION_FORMAT_VERSION,

            source: SessionSource::Ssh {
                host: "example-host".to_string(),

                user: Some("testuser".to_string()),

                port: 2222,

                identity_file: Some(PathBuf::from("/home/testuser/.ssh/id_example")),

                directory: PathBuf::from("/srv/projects"),

                home_directory: PathBuf::from("/home/testuser"),
            },

            selected_path: Some(PathBuf::from("/srv/projects/example/src/main.rs")),

            list_offset: 14,

            query: "type:source index".to_string(),

            view_mode: "tree".to_string(),

            search_mode: "fuzzy".to_string(),

            recursive: true,

            entry_filter: "files".to_string(),

            sort_mode: "type".to_string(),

            reverse: true,

            show_hidden: true,

            show_icons: true,

            show_details: true,

            show_selection: true,

            show_columns: true,

            show_permissions: true,

            show_size: true,

            show_date: false,

            show_user: false,

            marked_files: vec![SessionMarkedFile {
                path: PathBuf::from("/srv/projects/example/archive.tar"),

                filename: "archive.tar".to_string(),

                size_bytes: 4096,
            }],
        };

        let serialized = serde_json::to_string_pretty(&state).unwrap();

        let restored: SessionState = serde_json::from_str(&serialized).unwrap();

        assert!(restored.is_supported());

        assert_eq!(restored.query, "type:source index");

        assert_eq!(restored.marked_files.len(), 1);

        assert_eq!(restored.marked_files[0].size_bytes, 4096);
    }
}

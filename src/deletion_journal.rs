// SPDX-License-Identifier: BSD-3-Clause

use serde::{Deserialize, Serialize};

use std::{
    env, fs,
    fs::OpenOptions,
    io::{self, Write},
    path::{Path, PathBuf},
};

const STATE_DIRECTORY_NAME: &str = "scry";

const JOURNAL_FILENAME: &str = "deletions.json";

const JOURNAL_PART_FILENAME: &str = "deletions.json.part";

pub const JOURNAL_FORMAT_VERSION: u32 = 1;

/*
 * One staged local deletion retained across process interruption.
 *
 * Exact paths are stored deliberately. Recovery must never attempt to infer an
 * original pathname by parsing Scry's generated hidden filename.
 */
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletionJournalEntry {
    pub original_path: PathBuf,

    pub staged_path: PathBuf,

    pub is_directory: bool,

    pub is_symlink: bool,
}

/*
 * Versioned journal envelope.
 *
 * Keeping the version outside individual entries allows the complete on-disk
 * format to evolve later without guessing how an older record should be read.
 */
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletionJournal {
    pub version: u32,

    pub entries: Vec<DeletionJournalEntry>,
}

impl DeletionJournal {
    pub fn new(entries: Vec<DeletionJournalEntry>) -> Self {
        Self {
            version: JOURNAL_FORMAT_VERSION,

            entries,
        }
    }

    pub fn is_supported(&self) -> bool {
        self.version == JOURNAL_FORMAT_VERSION
    }
}

/*
 * Load Scry's deletion journal.
 *
 * A missing journal is the ordinary no-recovery-needed state.
 */
pub fn load() -> io::Result<Option<DeletionJournal>> {
    load_from_path(&journal_file_path()?)
}

/*
 * Publish the complete current deletion stack.
 *
 * An empty stack removes the journal because there is no unresolved filesystem
 * transaction left to recover.
 */
pub fn save_entries(entries: &[DeletionJournalEntry]) -> io::Result<()> {
    let path = journal_file_path()?;

    if entries.is_empty() {
        return remove_path_if_present(&path);
    }

    save_to_path(&path, &DeletionJournal::new(entries.to_vec()))
}

pub fn journal_file_path() -> io::Result<PathBuf> {
    Ok(state_directory()?.join(JOURNAL_FILENAME))
}

fn state_directory() -> io::Result<PathBuf> {
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

fn load_from_path(path: &Path) -> io::Result<Option<DeletionJournal>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,

        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(None);
        }

        Err(error) => {
            return Err(io::Error::new(
                error.kind(),
                format!(
                    "unable to read deletion journal {}: {}",
                    path.display(),
                    error,
                ),
            ));
        }
    };

    let journal = serde_json::from_str::<DeletionJournal>(&content).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "unable to parse deletion journal {}: {}",
                path.display(),
                error,
            ),
        )
    })?;

    Ok(Some(journal))
}

/*
 * Write, flush, sync, and then atomically publish the replacement journal.
 *
 * The live journal is never truncated in place. A crash during serialization
 * or writing therefore leaves either the preceding valid journal or an
 * unpublished .part file.
 */
fn save_to_path(path: &Path, journal: &DeletionJournal) -> io::Result<()> {
    let directory = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "deletion journal path has no parent directory: {}",
                path.display(),
            ),
        )
    })?;

    fs::create_dir_all(directory)?;

    let part_path = journal_part_path(path)?;

    let serialized = serde_json::to_vec_pretty(journal).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unable to serialize deletion journal: {}", error,),
        )
    })?;

    {
        let mut part_file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&part_path)
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!(
                        "unable to open deletion journal part file {}: {}",
                        part_path.display(),
                        error,
                    ),
                )
            })?;

        part_file.write_all(&serialized)?;

        part_file.flush()?;

        /*
         * sync_all() asks the operating system to persist the journal contents
         * before its pathname becomes authoritative.
         */
        part_file.sync_all()?;
    }

    fs::rename(&part_path, path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "unable to publish deletion journal {} from {}: {}",
                path.display(),
                part_path.display(),
                error,
            ),
        )
    })?;

    /*
     * Persist the directory entry update as well as the file contents.
     *
     * Linux and FreeBSD both permit opening a directory for synchronization.
     */
    let directory_file = fs::File::open(directory)?;

    directory_file.sync_all()?;

    Ok(())
}

fn journal_part_path(path: &Path) -> io::Result<PathBuf> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "deletion journal path has no parent directory: {}",
                path.display(),
            ),
        )
    })?;

    Ok(parent.join(JOURNAL_PART_FILENAME))
}

fn remove_path_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),

        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),

        Err(error) => Err(io::Error::new(
            error.kind(),
            format!("unable to remove {}: {}", path.display(), error,),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DeletionJournal, DeletionJournalEntry, JOURNAL_FORMAT_VERSION, load_from_path, save_to_path,
    };

    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn journal_round_trips_through_json() {
        let journal = DeletionJournal::new(vec![
            DeletionJournalEntry {
                original_path: PathBuf::from("/tmp/example/file.txt"),

                staged_path: PathBuf::from("/tmp/example/.scry-deleted-1-2-3-file.txt"),

                is_directory: false,

                is_symlink: false,
            },
            DeletionJournalEntry {
                original_path: PathBuf::from("/tmp/example/folder"),

                staged_path: PathBuf::from("/tmp/example/.scry-deleted-4-5-6-folder"),

                is_directory: true,

                is_symlink: false,
            },
        ]);

        let serialized = serde_json::to_string(&journal).expect("serialize deletion journal");

        let restored: DeletionJournal =
            serde_json::from_str(&serialized).expect("deserialize deletion journal");

        assert_eq!(restored, journal);

        assert!(restored.is_supported());

        assert_eq!(restored.version, JOURNAL_FORMAT_VERSION);
    }

    #[test]
    fn save_to_path_publishes_loadable_journal() {
        let directory = temporary_test_directory("publish");

        fs::create_dir_all(&directory).expect("create journal test directory");

        let path = directory.join("deletions.json");

        let journal = DeletionJournal::new(vec![DeletionJournalEntry {
            original_path: directory.join("file.txt"),

            staged_path: directory.join(".scry-deleted-1-2-3-file.txt"),

            is_directory: false,

            is_symlink: false,
        }]);

        save_to_path(&path, &journal).expect("save deletion journal");

        let restored = load_from_path(&path)
            .expect("load deletion journal")
            .expect("saved journal should exist");

        assert_eq!(restored, journal);

        assert!(
            !directory.join("deletions.json.part").exists(),
            "successful publication must not leave a part file",
        );

        fs::remove_dir_all(&directory).expect("remove journal test directory");
    }

    #[test]
    fn missing_journal_loads_as_none() {
        let directory = temporary_test_directory("missing");

        fs::create_dir_all(&directory).expect("create journal test directory");

        let result =
            load_from_path(&directory.join("deletions.json")).expect("load missing journal");

        assert!(result.is_none());

        fs::remove_dir_all(&directory).expect("remove journal test directory");
    }

    fn temporary_test_directory(label: &str) -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();

        std::env::temp_dir().join(format!(
            "scry-deletion-journal-test-{}-{}-{}",
            label,
            std::process::id(),
            timestamp,
        ))
    }
}

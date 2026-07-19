// SPDX-License-Identifier: BSD-3-Clause

use chrono::{DateTime, Local};
use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    mpsc::{self, Receiver, Sender},
};
use std::thread;
use std::time::SystemTime;

use crate::classify::{FileClass, classify};
use crate::entry::{EntryKind, EntryMetadata};

pub(crate) const SCAN_BATCH_SIZE: usize = 256;

pub(crate) const FAST_SCAN_ENTRY_LIMIT: usize = 250_000;

const ROOT_SKIPPED_DIRECTORIES: &[&str] = &["proc", "sys", "dev", "run"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecursiveScanMode {
    Fast,

    #[allow(dead_code)]
    Total,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    Name,
    Size,
    Modified,
    Type,
}

impl SortMode {
    pub fn next(self) -> Self {
        match self {
            Self::Name => Self::Size,
            Self::Size => Self::Modified,
            Self::Modified => Self::Type,
            Self::Type => Self::Name,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Size => "Size",
            Self::Modified => "Date",
            Self::Type => "Type",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: PathBuf,

    pub relative_path: PathBuf,

    /*
     * Lowercase strings shared cheaply with background search workers.
     *
     * Cloning an Arc does not duplicate the underlying filename or path.
     */
    pub searchable_path: Arc<str>,

    pub searchable_name: Arc<str>,

    pub name: String,

    pub is_directory: bool,

    pub is_symlink: bool,

    pub permissions: String,

    pub modified: String,

    pub modified_time: Option<SystemTime>,

    pub owner_id: u32,

    pub size_bytes: u64,

    pub class: FileClass,
}

#[derive(Debug)]
pub enum ScanMessage {
    Batch {
        generation: u64,
        entries: Vec<FileEntry>,
    },

    Finished {
        generation: u64,

        partial: bool,
    },

    Failed {
        generation: u64,
        message: String,
    },
}

pub fn read_directory(
    directory: &Path,
    sort_mode: SortMode,
    sort_descending: bool,
) -> io::Result<Vec<FileEntry>> {
    let mut entries = Vec::new();

    for result in fs::read_dir(directory)? {
        let dir_entry = match result {
            Ok(dir_entry) => dir_entry,

            Err(_) => {
                continue;
            }
        };

        let Some(entry) = make_file_entry(
            dir_entry.path(),
            directory,
            dir_entry.file_name().to_string_lossy().into_owned(),
        ) else {
            continue;
        };

        entries.push(entry);
    }

    sort_entries(&mut entries, sort_mode, sort_descending);

    Ok(entries)
}

pub fn start_recursive_scan(
    root: PathBuf,
    show_hidden: bool,
    generation: u64,
    mode: RecursiveScanMode,
) -> Receiver<ScanMessage> {
    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        scan_directory_tree(root, show_hidden, generation, mode, sender);
    });

    receiver
}

pub fn sort_entries(entries: &mut [FileEntry], mode: SortMode, descending: bool) {
    entries.sort_by(|left, right| {
        /*
         * Directories always remain above ordinary files.
         */
        match (left.is_directory, right.is_directory) {
            (true, false) => {
                return std::cmp::Ordering::Less;
            }

            (false, true) => {
                return std::cmp::Ordering::Greater;
            }

            _ => {}
        }

        /*
         * Directories remain alphabetically ordered because their displayed
         * size and type are not useful sorting criteria.
         */
        let ordering = if left.is_directory && right.is_directory {
            left.searchable_path.cmp(&right.searchable_path)
        } else {
            let primary_ordering = match mode {
                SortMode::Name => left.searchable_path.cmp(&right.searchable_path),

                SortMode::Size => left.size_bytes.cmp(&right.size_bytes),

                SortMode::Modified => left.modified_time.cmp(&right.modified_time),

                SortMode::Type => left.class.label().cmp(right.class.label()),
            };

            /*
             * Name provides deterministic ordering when two files have the
             * same size, date, or classification.
             */
            primary_ordering.then_with(|| left.searchable_path.cmp(&right.searchable_path))
        };

        if descending {
            ordering.reverse()
        } else {
            ordering
        }
    });
}

fn scan_directory_tree(
    root: PathBuf,
    show_hidden: bool,
    generation: u64,
    mode: RecursiveScanMode,
    sender: Sender<ScanMessage>,
) {
    let mut pending_directories = VecDeque::from([root.clone()]);

    let mut scanned_entries = 0_usize;

    let mut batch = Vec::with_capacity(SCAN_BATCH_SIZE);

    while let Some(directory) = pending_directories.pop_front() {
        let directory_entries = match fs::read_dir(&directory) {
            Ok(directory_entries) => directory_entries,

            Err(error) if directory == root => {
                let _ = sender.send(ScanMessage::Failed {
                    generation,

                    message: format!("Unable to search {}: {}", root.display(), error,),
                });

                return;
            }

            Err(_) => {
                continue;
            }
        };

        for result in directory_entries {
            let dir_entry = match result {
                Ok(dir_entry) => dir_entry,

                Err(_) => {
                    continue;
                }
            };

            let name = dir_entry.file_name().to_string_lossy().into_owned();

            if !show_hidden && name.starts_with('.') {
                continue;
            }

            let path = dir_entry.path();

            if should_skip_directory(&root, &path, &name) {
                continue;
            }

            let Some(entry) = make_file_entry(path.clone(), &root, name) else {
                continue;
            };

            /*
             * symlink_metadata() does not follow symlinks.
             *
             * Directory symlinks therefore appear in the results but are not
             * traversed, preventing recursive symlink loops.
             */
            if entry.is_directory && !entry.is_symlink {
                pending_directories.push_back(path);
            }

            batch.push(entry);

            scanned_entries = scanned_entries.saturating_add(1);

            if mode == RecursiveScanMode::Fast && scanned_entries >= FAST_SCAN_ENTRY_LIMIT {
                if !batch.is_empty() && send_batch(&sender, generation, &mut batch).is_err() {
                    return;
                }

                let _ = sender.send(ScanMessage::Finished {
                    generation,

                    partial: true,
                });

                return;
            }

            if batch.len() >= SCAN_BATCH_SIZE {
                if send_batch(&sender, generation, &mut batch).is_err() {
                    /*
                     * The App discarded the receiver, usually because the user
                     * changed directory or started a newer scan.
                     */
                    return;
                }
            }
        }
    }

    if !batch.is_empty() && send_batch(&sender, generation, &mut batch).is_err() {
        return;
    }

    let _ = sender.send(ScanMessage::Finished {
        generation,

        partial: false,
    });
}

pub(crate) fn should_skip_directory(root: &Path, path: &Path, name: &str) -> bool {
    if root != Path::new("/") {
        return false;
    }

    let Some(parent) = path.parent() else {
        return false;
    };

    parent == root && ROOT_SKIPPED_DIRECTORIES.contains(&name)
}

fn send_batch(
    sender: &Sender<ScanMessage>,
    generation: u64,
    batch: &mut Vec<FileEntry>,
) -> Result<(), mpsc::SendError<ScanMessage>> {
    let entries = std::mem::replace(batch, Vec::with_capacity(SCAN_BATCH_SIZE));

    sender.send(ScanMessage::Batch {
        generation,

        entries,
    })
}

fn local_entry_metadata(metadata: &fs::Metadata, is_symlink: bool) -> EntryMetadata {
    EntryMetadata {
        kind: local_entry_kind(metadata, is_symlink),

        permissions_mode: local_permissions_mode(metadata),

        size_bytes: metadata.len(),

        modified_time: metadata.modified().ok(),

        owner_id: local_owner_id(metadata),
    }
}

#[cfg(unix)]
fn local_entry_kind(metadata: &fs::Metadata, is_symlink: bool) -> EntryKind {
    use std::os::unix::fs::FileTypeExt;

    let file_type = metadata.file_type();

    if is_symlink {
        EntryKind::Symlink
    } else if file_type.is_dir() {
        EntryKind::Directory
    } else if file_type.is_block_device() {
        EntryKind::BlockDevice
    } else if file_type.is_char_device() {
        EntryKind::CharDevice
    } else if file_type.is_fifo() {
        EntryKind::Fifo
    } else if file_type.is_socket() {
        EntryKind::Socket
    } else if file_type.is_file() {
        EntryKind::File
    } else {
        EntryKind::Unknown
    }
}

#[cfg(not(unix))]
fn local_entry_kind(metadata: &fs::Metadata, is_symlink: bool) -> EntryKind {
    if is_symlink {
        EntryKind::Symlink
    } else if metadata.is_dir() {
        EntryKind::Directory
    } else if metadata.is_file() {
        EntryKind::File
    } else {
        EntryKind::Unknown
    }
}

#[cfg(unix)]
fn local_permissions_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode()
}

#[cfg(not(unix))]
fn local_permissions_mode(_metadata: &fs::Metadata) -> u32 {
    0
}

#[cfg(unix)]
fn local_owner_id(metadata: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;

    Some(metadata.uid())
}

#[cfg(not(unix))]
fn local_owner_id(_metadata: &fs::Metadata) -> Option<u32> {
    None
}

fn format_modified_date(modified_time: Option<SystemTime>) -> String {
    let Some(modified) = modified_time else {
        return "—".to_string();
    };

    let modified: DateTime<Local> = DateTime::from(modified);

    modified.format("%Y-%m-%d %H:%M").to_string()
}

fn make_file_entry(path: PathBuf, root: &Path, name: String) -> Option<FileEntry> {
    let metadata = fs::symlink_metadata(&path).ok()?;

    let is_symlink = metadata.file_type().is_symlink();

    let entry_metadata = local_entry_metadata(&metadata, is_symlink);

    let permissions = format_permissions(&entry_metadata);

    let modified = format_modified_date(entry_metadata.modified_time);

    let modified_time = entry_metadata.modified_time;

    let owner_id = entry_metadata.owner_id.unwrap_or(0);

    let size_bytes = entry_metadata.size_bytes;

    let class = classify(&path, &entry_metadata);

    let relative_path = path.strip_prefix(root).unwrap_or(&path).to_path_buf();

    let searchable_path: Arc<str> = Arc::from(relative_path.to_string_lossy().to_lowercase());

    let searchable_name: Arc<str> = Arc::from(name.to_lowercase());

    Some(FileEntry {
        path,

        relative_path,

        searchable_path,

        searchable_name,

        name,

        is_directory: entry_metadata.kind.is_directory(),

        is_symlink: entry_metadata.kind.is_symlink(),

        permissions,

        modified,

        modified_time,

        owner_id,

        size_bytes,

        class,
    })
}

#[cfg(unix)]
fn format_permissions(metadata: &EntryMetadata) -> String {
    let type_character = metadata.kind.permission_type_character();

    let mode = metadata.permissions_mode;

    let mut permissions = String::with_capacity(10);

    permissions.push(type_character);

    permissions.push(if mode & 0o400 != 0 { 'r' } else { '-' });

    permissions.push(if mode & 0o200 != 0 { 'w' } else { '-' });

    permissions.push(match (mode & 0o100 != 0, mode & 0o4000 != 0) {
        (true, true) => 's',
        (false, true) => 'S',
        (true, false) => 'x',
        (false, false) => '-',
    });

    permissions.push(if mode & 0o040 != 0 { 'r' } else { '-' });

    permissions.push(if mode & 0o020 != 0 { 'w' } else { '-' });

    permissions.push(match (mode & 0o010 != 0, mode & 0o2000 != 0) {
        (true, true) => 's',
        (false, true) => 'S',
        (true, false) => 'x',
        (false, false) => '-',
    });

    permissions.push(if mode & 0o004 != 0 { 'r' } else { '-' });

    permissions.push(if mode & 0o002 != 0 { 'w' } else { '-' });

    permissions.push(match (mode & 0o001 != 0, mode & 0o1000 != 0) {
        (true, true) => 't',
        (false, true) => 'T',
        (true, false) => 'x',
        (false, false) => '-',
    });

    permissions
}

#[cfg(not(unix))]
fn format_permissions(metadata: &EntryMetadata) -> String {
    format!("{}---------", metadata.kind.permission_type_character(),)
}

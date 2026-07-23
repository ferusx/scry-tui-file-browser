// SPDX-License-Identifier: BSD-3-Clause

use chrono::{DateTime, Local};

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, SystemTime},
};

use users::{get_group_by_gid, get_user_by_uid};

use crate::{classify::FileClass, entry::EntryKind, scan::FileEntry};

/*
 * Information that is already available when the popup opens.
 *
 * This lets Scry display the File Information window immediately while a
 * background worker gathers the extended filesystem metadata.
 */
#[derive(Debug, Clone)]
pub struct FileInfo {
    pub path: PathBuf,

    pub name: String,

    pub kind: EntryKind,

    pub classification: FileClass,

    pub size_bytes: u64,

    pub permissions_mode: Option<u32>,

    pub owner_id: Option<u32>,

    pub owner_name: Option<String>,

    pub group_id: Option<u32>,

    pub group_name: Option<String>,

    pub modified_time: Option<SystemTime>,

    pub accessed_time: Option<SystemTime>,

    pub created_time: Option<SystemTime>,

    pub symlink_target: Option<PathBuf>,

    pub symlink_target_exists: Option<bool>,

    pub directory_summary: Option<DirectorySummary>,

    pub source_label: String,

    pub is_remote: bool,

    pub cache_info: Option<RemoteCacheInfo>,

    /*
     * Nonfatal limitations encountered while collecting metadata.
     *
     * Examples:
     *
     *     birth time was not reported
     *     directory contents could not be counted
     *     symlink target could not be resolved
     */
    pub notes: Vec<String>,
}

impl FileInfo {
    /*
     * Construct the immediately displayable part from the selected FileEntry.
     *
     * Fields requiring a fresh stat call remain None until the background
     * worker returns.
     */
    pub fn from_entry(
        entry: &FileEntry,
        kind: EntryKind,
        source_label: String,
        is_remote: bool,
    ) -> Self {
        Self {
            path: entry.path.clone(),

            name: entry.name.clone(),

            kind,

            classification: entry.class,

            size_bytes: entry.size_bytes,

            permissions_mode: None,

            owner_id: Some(entry.owner_id),

            owner_name: None,

            group_id: None,

            group_name: None,

            modified_time: entry.modified_time,

            accessed_time: None,

            created_time: None,

            symlink_target: None,

            symlink_target_exists: None,

            directory_summary: None,

            source_label,

            is_remote,

            cache_info: None,

            notes: Vec::new(),
        }
    }

    pub fn extension(&self) -> String {
        self.path
            .extension()
            .and_then(|extension| extension.to_str())
            .filter(|extension| !extension.is_empty())
            .unwrap_or("—")
            .to_string()
    }

    pub fn human_size(&self) -> String {
        format_file_size(self.size_bytes)
    }

    pub fn exact_size(&self) -> String {
        format!("{} bytes", format_integer(self.size_bytes),)
    }

    pub fn symbolic_permissions(&self) -> String {
        match self.permissions_mode {
            Some(mode) => format_permissions(self.kind, mode),

            None => "Loading…".to_string(),
        }
    }

    pub fn octal_permissions(&self) -> String {
        match self.permissions_mode {
            Some(mode) => format!("{:04o}", mode & 0o7777,),

            None => "Loading…".to_string(),
        }
    }

    pub fn owner(&self) -> String {
        format_identity(self.owner_name.as_deref(), self.owner_id)
    }

    pub fn group(&self) -> String {
        format_identity(self.group_name.as_deref(), self.group_id)
    }

    pub fn modified(&self) -> String {
        format_timestamp(self.modified_time)
    }

    pub fn accessed(&self) -> String {
        format_optional_timestamp(self.accessed_time, "Not reported")
    }

    pub fn created(&self) -> String {
        format_optional_timestamp(self.created_time, "Not reported")
    }

    pub fn age(&self) -> String {
        format_age(self.modified_time)
    }

    pub fn kind_label(&self) -> &'static str {
        entry_kind_label(self.kind)
    }

    pub fn executable(&self) -> &'static str {
        match self.permissions_mode {
            Some(mode) if mode & 0o111 != 0 => "Yes",

            Some(_) => "No",

            None => "Loading…",
        }
    }

    pub fn hidden(&self) -> &'static str {
        if self.name.starts_with('.') {
            "Yes"
        } else {
            "No"
        }
    }

    pub fn symlink_target_display(&self) -> String {
        match &self.symlink_target {
            Some(target) => target.display().to_string(),

            None if self.kind == EntryKind::Symlink => "Not reported".to_string(),

            None => "Not applicable".to_string(),
        }
    }

    pub fn symlink_target_exists_display(&self) -> &'static str {
        match self.symlink_target_exists {
            Some(true) => "Yes",

            Some(false) => "No",

            None if self.kind == EntryKind::Symlink => "Unknown",

            None => "Not applicable",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DirectorySummary {
    pub total: u64,

    pub directories: u64,

    pub files: u64,

    pub symlinks: u64,

    pub other: u64,
}

impl DirectorySummary {
    pub fn display_line(self) -> String {
        format!(
            "{} entries — {} directories, {} files, {} symlinks, {} other",
            format_integer(self.total),
            format_integer(self.directories),
            format_integer(self.files),
            format_integer(self.symlinks),
            format_integer(self.other),
        )
    }
}

/*
 * Remote cache details are populated by SftpSource later.
 *
 * Keeping the structure here lets the popup renderer remain independent from
 * SSH implementation details.
 */
#[derive(Debug, Clone)]
pub struct RemoteCacheInfo {
    pub cache_path: PathBuf,

    pub cached_copy_exists: bool,

    pub cached_copy_current: Option<bool>,

    pub cached_size_bytes: Option<u64>,
}

impl RemoteCacheInfo {
    pub fn cached_status(&self) -> &'static str {
        if !self.cached_copy_exists {
            return "No";
        }

        match self.cached_copy_current {
            Some(true) => "Yes — current",

            Some(false) => "Yes — outdated",

            None => "Yes — status unknown",
        }
    }

    pub fn cached_size(&self) -> String {
        self.cached_size_bytes
            .map(format_file_size)
            .unwrap_or_else(|| "—".to_string())
    }
}

/*
 * Complete state owned by App while the File Information window is visible.
 *
 * The initial FileInfo is rendered immediately. The worker later replaces it
 * with freshly collected metadata without closing or rearranging the popup.
 */
#[derive(Debug)]
pub struct FileInfoState {
    pub info: FileInfo,

    pub loading: bool,

    pub error: Option<String>,

    pub scroll: u16,

    pub maximum_scroll: u16,
}

impl FileInfoState {
    pub fn loading(info: FileInfo) -> Self {
        Self {
            info,

            loading: true,

            error: None,

            scroll: 0,

            maximum_scroll: 0,
        }
    }

    pub fn finish(&mut self, info: FileInfo) {
        self.info = info;

        self.loading = false;

        self.error = None;
    }

    pub fn fail(&mut self, message: String) {
        self.loading = false;

        self.error = Some(message);
    }

    pub fn status_line(&self) -> String {
        if self.loading {
            if self.info.is_remote {
                return format!("Loading extended metadata from {}…", self.info.source_label,);
            }

            return "Loading extended filesystem metadata…".to_string();
        }

        if let Some(error) = &self.error {
            return error.clone();
        }

        if self.info.notes.is_empty() {
            "Metadata loaded successfully.".to_string()
        } else {
            format!(
                "Metadata loaded with {} note{}.",
                self.info.notes.len(),
                if self.info.notes.len() == 1 { "" } else { "s" },
            )
        }
    }

    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    pub fn scroll_down(&mut self) {
        self.scroll = self.scroll.saturating_add(1).min(self.maximum_scroll);
    }

    pub fn page_up(&mut self, amount: u16) {
        self.scroll = self.scroll.saturating_sub(amount.max(1));
    }

    pub fn page_down(&mut self, amount: u16) {
        self.scroll = self
            .scroll
            .saturating_add(amount.max(1))
            .min(self.maximum_scroll);
    }

    pub fn scroll_to_start(&mut self) {
        self.scroll = 0;
    }

    pub fn scroll_to_end(&mut self) {
        self.scroll = self.maximum_scroll;
    }
}

#[derive(Debug)]
pub enum FileInfoMessage {
    Finished { generation: u64, info: FileInfo },

    Failed { generation: u64, message: String },
}

/*
 * Gather complete metadata for a local filesystem path without blocking the
 * terminal event loop.
 */
pub fn start_local_file_info(initial_info: FileInfo, generation: u64) -> Receiver<FileInfoMessage> {
    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        let path = initial_info.path.clone();

        match collect_local_file_info(initial_info) {
            Ok(info) => {
                let _ = sender.send(FileInfoMessage::Finished { generation, info });
            }

            Err(error) => {
                let _ = sender.send(FileInfoMessage::Failed {
                    generation,

                    message: format!("Unable to inspect {}: {}", path.display(), error,),
                });
            }
        }
    });

    receiver
}

fn collect_local_file_info(mut info: FileInfo) -> io::Result<FileInfo> {
    /*
     * symlink_metadata() describes the link itself rather than silently
     * following it to its target.
     */
    let metadata = fs::symlink_metadata(&info.path)?;

    info.kind = entry_kind_from_metadata(&metadata);

    info.size_bytes = metadata.len();

    info.permissions_mode = permissions_mode(&metadata);

    info.modified_time = metadata.modified().ok();

    info.accessed_time = metadata.accessed().ok();

    info.created_time = metadata.created().ok();

    info.owner_id = owner_id(&metadata);

    info.group_id = group_id(&metadata);

    info.owner_name = info.owner_id.and_then(resolve_owner_name);

    info.group_name = info.group_id.and_then(resolve_group_name);

    if info.kind == EntryKind::Symlink {
        match fs::read_link(&info.path) {
            Ok(target) => {
                let resolved_target = resolve_symlink_target(&info.path, &target);

                info.symlink_target_exists = Some(resolved_target.exists());

                info.symlink_target = Some(target);
            }

            Err(error) => {
                info.notes
                    .push(format!("Unable to read the symlink target: {}", error,));
            }
        }
    }

    if info.kind == EntryKind::Directory {
        match summarize_directory(&info.path) {
            Ok(summary) => {
                info.directory_summary = Some(summary);
            }

            Err(error) => {
                info.notes.push(format!(
                    "Unable to count immediate directory contents: {}",
                    error,
                ));
            }
        }
    }

    Ok(info)
}

fn summarize_directory(path: &Path) -> io::Result<DirectorySummary> {
    let mut summary = DirectorySummary::default();

    for result in fs::read_dir(path)? {
        let entry = match result {
            Ok(entry) => entry,

            /*
             * One unreadable or disappearing child should not invalidate the
             * count of every other visible child.
             */
            Err(_) => {
                summary.total = summary.total.saturating_add(1);

                summary.other = summary.other.saturating_add(1);

                continue;
            }
        };

        summary.total = summary.total.saturating_add(1);

        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,

            Err(_) => {
                summary.other = summary.other.saturating_add(1);

                continue;
            }
        };

        if file_type.is_symlink() {
            summary.symlinks = summary.symlinks.saturating_add(1);
        } else if file_type.is_dir() {
            summary.directories = summary.directories.saturating_add(1);
        } else if file_type.is_file() {
            summary.files = summary.files.saturating_add(1);
        } else {
            summary.other = summary.other.saturating_add(1);
        }
    }

    Ok(summary)
}

fn resolve_symlink_target(link_path: &Path, target: &Path) -> PathBuf {
    if target.is_absolute() {
        return target.to_path_buf();
    }

    link_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(target)
}

fn format_identity(name: Option<&str>, numeric_id: Option<u32>) -> String {
    match (name, numeric_id) {
        (Some(name), Some(id)) => {
            format!("{} ({})", name, id,)
        }

        (Some(name), None) => name.to_string(),

        (None, Some(id)) => id.to_string(),

        (None, None) => "Not reported".to_string(),
    }
}

fn format_optional_timestamp(timestamp: Option<SystemTime>, unavailable: &str) -> String {
    timestamp
        .map(format_system_time)
        .unwrap_or_else(|| unavailable.to_string())
}

pub fn format_timestamp(timestamp: Option<SystemTime>) -> String {
    timestamp
        .map(format_system_time)
        .unwrap_or_else(|| "Not reported".to_string())
}

fn format_system_time(timestamp: SystemTime) -> String {
    let local: DateTime<Local> = DateTime::from(timestamp);

    /*
     * Example:
     *
     *     Mon Jul 20 16:49:41 2026
     */
    local.format("%a %b %d %H:%M:%S %Y").to_string()
}

pub fn format_age(timestamp: Option<SystemTime>) -> String {
    let Some(timestamp) = timestamp else {
        return "Not reported".to_string();
    };

    match SystemTime::now().duration_since(timestamp) {
        Ok(duration) => format_age_duration(duration),

        Err(error) => {
            format!("in {}", format_age_duration(error.duration(),),)
        }
    }
}

fn format_age_duration(duration: Duration) -> String {
    const MINUTE: u64 = 60;

    const HOUR: u64 = 60 * MINUTE;

    const DAY: u64 = 24 * HOUR;

    const MONTH: u64 = 30 * DAY;

    const YEAR: u64 = 365 * DAY;

    let seconds = duration.as_secs();

    if seconds < 5 {
        return "just now".to_string();
    }

    if seconds < MINUTE {
        return format!("{}s", seconds,);
    }

    if seconds < HOUR {
        return format!("{}m", seconds / MINUTE,);
    }

    if seconds < DAY {
        let hours = seconds / HOUR;

        let minutes = (seconds % HOUR) / MINUTE;

        return if minutes == 0 {
            format!("{}h", hours,)
        } else {
            format!("{}h {}m", hours, minutes,)
        };
    }

    if seconds < MONTH {
        let days = seconds / DAY;

        let hours = (seconds % DAY) / HOUR;

        return if hours == 0 {
            format!("{}d", days,)
        } else {
            format!("{}d {}h", days, hours,)
        };
    }

    if seconds < YEAR {
        let months = seconds / MONTH;

        let days = (seconds % MONTH) / DAY;

        return if days == 0 {
            format!("{}mo", months,)
        } else {
            format!("{}mo {}d", months, days,)
        };
    }

    let years = seconds / YEAR;

    let months = (seconds % YEAR) / MONTH;

    if months == 0 {
        format!("{}y", years,)
    } else {
        format!("{}y {}mo", years, months,)
    }
}

pub fn format_file_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;

    const MIB: f64 = KIB * 1024.0;

    const GIB: f64 = MIB * 1024.0;

    const TIB: f64 = GIB * 1024.0;

    let bytes_as_float = bytes as f64;

    if bytes < 1024 {
        format!("{} B", bytes,)
    } else if bytes_as_float < MIB {
        format!("{:.1} KiB", bytes_as_float / KIB,)
    } else if bytes_as_float < GIB {
        format!("{:.1} MiB", bytes_as_float / MIB,)
    } else if bytes_as_float < TIB {
        format!("{:.1} GiB", bytes_as_float / GIB,)
    } else {
        format!("{:.1} TiB", bytes_as_float / TIB,)
    }
}

pub fn format_integer(value: u64) -> String {
    let digits = value.to_string();

    let mut result = String::with_capacity(
        digits
            .len()
            .saturating_add(digits.len().saturating_sub(1) / 3),
    );

    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            result.push(',');
        }

        result.push(character);
    }

    result
}

pub fn format_permissions(kind: EntryKind, mode: u32) -> String {
    let mut result = String::with_capacity(10);

    result.push(permission_type_character(kind));

    result.push(if mode & 0o400 != 0 { 'r' } else { '-' });

    result.push(if mode & 0o200 != 0 { 'w' } else { '-' });

    result.push(match (mode & 0o100 != 0, mode & 0o4000 != 0) {
        (true, true) => 's',

        (false, true) => 'S',

        (true, false) => 'x',

        (false, false) => '-',
    });

    result.push(if mode & 0o040 != 0 { 'r' } else { '-' });

    result.push(if mode & 0o020 != 0 { 'w' } else { '-' });

    result.push(match (mode & 0o010 != 0, mode & 0o2000 != 0) {
        (true, true) => 's',

        (false, true) => 'S',

        (true, false) => 'x',

        (false, false) => '-',
    });

    result.push(if mode & 0o004 != 0 { 'r' } else { '-' });

    result.push(if mode & 0o002 != 0 { 'w' } else { '-' });

    result.push(match (mode & 0o001 != 0, mode & 0o1000 != 0) {
        (true, true) => 't',

        (false, true) => 'T',

        (true, false) => 'x',

        (false, false) => '-',
    });

    result
}

fn entry_kind_label(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::File => "Regular file",

        EntryKind::Directory => "Directory",

        EntryKind::Symlink => "Symbolic link",

        EntryKind::Socket => "Socket",

        EntryKind::Fifo => "Named pipe",

        EntryKind::BlockDevice => "Block device",

        EntryKind::CharDevice => "Character device",

        EntryKind::Unknown => "Unknown",
    }
}

fn permission_type_character(kind: EntryKind) -> char {
    match kind {
        EntryKind::File => '.',

        EntryKind::Directory => 'd',

        EntryKind::Symlink => 'l',

        EntryKind::Socket => 's',

        EntryKind::Fifo => 'p',

        EntryKind::BlockDevice => 'b',

        EntryKind::CharDevice => 'c',

        EntryKind::Unknown => '?',
    }
}

#[cfg(unix)]
fn entry_kind_from_metadata(metadata: &fs::Metadata) -> EntryKind {
    use std::os::unix::fs::FileTypeExt;

    let file_type = metadata.file_type();

    if file_type.is_symlink() {
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
fn entry_kind_from_metadata(metadata: &fs::Metadata) -> EntryKind {
    let file_type = metadata.file_type();

    if file_type.is_symlink() {
        EntryKind::Symlink
    } else if file_type.is_dir() {
        EntryKind::Directory
    } else if file_type.is_file() {
        EntryKind::File
    } else {
        EntryKind::Unknown
    }
}

#[cfg(unix)]
fn permissions_mode(metadata: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;

    Some(metadata.permissions().mode())
}

#[cfg(not(unix))]
fn permissions_mode(_metadata: &fs::Metadata) -> Option<u32> {
    None
}

#[cfg(unix)]
fn owner_id(metadata: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;

    Some(metadata.uid())
}

#[cfg(not(unix))]
fn owner_id(_metadata: &fs::Metadata) -> Option<u32> {
    None
}

#[cfg(unix)]
fn group_id(metadata: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;

    Some(metadata.gid())
}

#[cfg(not(unix))]
fn group_id(_metadata: &fs::Metadata) -> Option<u32> {
    None
}

fn resolve_owner_name(owner_id: u32) -> Option<String> {
    get_user_by_uid(owner_id).map(|user| user.name().to_string_lossy().into_owned())
}

fn resolve_group_name(group_id: u32) -> Option<String> {
    get_group_by_gid(group_id).map(|group| group.name().to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::{format_file_size, format_integer, format_permissions};

    use crate::entry::EntryKind;

    #[test]
    fn integer_format_uses_grouping() {
        assert_eq!(format_integer(0), "0");

        assert_eq!(format_integer(999), "999");

        assert_eq!(format_integer(1_000), "1,000");

        assert_eq!(format_integer(184_958_221), "184,958,221");
    }

    #[test]
    fn file_size_formats_binary_units() {
        assert_eq!(format_file_size(12), "12 B");

        assert_eq!(format_file_size(1024), "1.0 KiB");

        assert_eq!(format_file_size(1024 * 1024), "1.0 MiB");
    }

    #[test]
    fn permissions_include_special_bits() {
        assert_eq!(format_permissions(EntryKind::File, 0o100644), ".rw-r--r--",);

        assert_eq!(
            format_permissions(EntryKind::Directory, 0o41755),
            "drwxr-xr-t",
        );

        assert_eq!(format_permissions(EntryKind::File, 0o104755), ".rwsr-xr-x",);

        assert_eq!(format_permissions(EntryKind::File, 0o1001777), ".rwxrwxrwt",);
    }
}

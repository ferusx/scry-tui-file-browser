// SPDX-License-Identifier: BSD-3-Clause

use std::{
    env, fmt, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, Local};

use futures_util::TryStreamExt;

use openssh::{KnownHosts, SessionBuilder};

use openssh_sftp_client::{
    Sftp, SftpOptions,
    file::TokioCompatFile,
    fs::DirEntry,
    metadata::{FileType, Permissions},
};

use tokio::{io::AsyncReadExt, runtime::Runtime};

use crate::{
    classify::classify,
    entry::{EntryKind, EntryMetadata},
    scan::{FileEntry, SortMode, sort_entries},
    source::{FileSource, TransferControl, TransferProgress},
};

const CACHE_METADATA_SUFFIX: &str = ".scry-meta";

const PART_FILE_SUFFIX: &str = ".scry-part";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RemoteCacheMetadata {
    size_bytes: u64,

    modified_seconds: Option<u64>,

    modified_nanoseconds: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct SshTarget {
    pub host: String,

    pub user: Option<String>,

    pub port: u16,

    pub identity_file: Option<PathBuf>,
}

#[derive(Debug)]
pub struct SftpSource {
    /*
     * Sftp is declared before Runtime so it is dropped first.
     *
     * The asynchronous SFTP background tasks must disappear before their
     * Tokio runtime is destroyed.
     */
    sftp: Sftp,

    runtime: Runtime,

    label: String,

    cache_namespace: String,
}

impl SshTarget {
    pub fn parse(value: &str) -> Result<Self, SshTargetError> {
        let value = value.trim();

        if value.is_empty() {
            return Err(SshTargetError::Empty);
        }

        let (user, host_and_port) = match value.split_once('@') {
            Some((user, remainder)) => {
                if user.is_empty() {
                    return Err(SshTargetError::MissingUser);
                }

                (Some(user.to_string()), remainder)
            }

            None => (None, value),
        };

        let (host, port) = parse_host_and_port(host_and_port)?;

        Ok(Self {
            host,

            user,

            port,

            identity_file: None,
        })
    }

    pub fn destination_label(&self) -> String {
        match &self.user {
            Some(user) => {
                format!("{}@{}", user, self.host,)
            }

            None => self.host.clone(),
        }
    }

    pub fn openssh_destination(&self) -> String {
        /*
         * Preserve ordinary OpenSSH alias handling whenever the default port
         * is used:
         *
         *     nosferatu
         *     ferusx@nosferatu
         *
         * For a custom port, use OpenSSH's URI form so the port remains part
         * of the destination without inventing separate parsing rules.
         */
        if self.port == 22 {
            return self.destination_label();
        }

        let host = if self.host.contains(':') {
            format!("[{}]", self.host,)
        } else {
            self.host.clone()
        };

        match &self.user {
            Some(user) => {
                format!("ssh://{}@{}:{}", user, host, self.port,)
            }

            None => {
                format!("ssh://{}:{}", host, self.port,)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshTargetError {
    Empty,

    MissingUser,

    MissingHost,

    InvalidPort(String),
}

#[derive(Debug)]
pub enum SftpSourceError {
    RuntimeCreation(String),

    Connection {
        destination: String,
        message: String,
    },

    ConnectionCheck(String),

    SftpInitialization(String),

    RemotePath(String),
}

impl fmt::Display for SftpSourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeCreation(message) => {
                write!(formatter, "unable to create the SSH runtime: {}", message,)
            }

            Self::Connection {
                destination,
                message,
            } => {
                write!(
                    formatter,
                    "unable to connect to {} through OpenSSH: {}",
                    destination, message,
                )
            }

            Self::ConnectionCheck(message) => {
                write!(
                    formatter,
                    "the OpenSSH connection was created but failed its health check: {}",
                    message,
                )
            }

            Self::SftpInitialization(message) => {
                write!(
                    formatter,
                    "the SSH connection succeeded but the SFTP subsystem failed to start: {}",
                    message,
                )
            }

            Self::RemotePath(message) => {
                write!(
                    formatter,
                    "unable to determine the remote starting directory: {}",
                    message,
                )
            }
        }
    }
}

impl std::error::Error for SftpSourceError {}

impl SftpSource {
    pub fn connect(target: &SshTarget) -> Result<(PathBuf, Self), SftpSourceError> {
        let runtime =
            Runtime::new().map_err(|error| SftpSourceError::RuntimeCreation(error.to_string()))?;

        let destination = target.openssh_destination();

        let (home_directory, sftp) = runtime.block_on(async {
            let mut builder = SessionBuilder::default();

            builder
                .known_hosts_check(KnownHosts::Strict)
                .connect_timeout(Duration::from_secs(10))
                .server_alive_interval(Duration::from_secs(15));

            if let Some(identity_file) = &target.identity_file {
                builder.keyfile(identity_file);
            }

            let session = builder.connect_mux(&destination).await.map_err(|error| {
                SftpSourceError::Connection {
                    destination: destination.clone(),

                    message: format!("{error:#?}"),
                }
            })?;

            session
                .check()
                .await
                .map_err(|error| SftpSourceError::ConnectionCheck(format!("{error:#?}")))?;

            let sftp = Sftp::from_session(session, SftpOptions::default())
                .await
                .map_err(|error| SftpSourceError::SftpInitialization(format!("{error:#?}")))?;

            let home_directory = {
                let mut filesystem = sftp.fs();

                filesystem
                    .canonicalize(".")
                    .await
                    .map_err(|error| SftpSourceError::RemotePath(format!("{error:#?}")))?
            };

            Ok::<_, SftpSourceError>((home_directory, sftp))
        })?;

        Ok((
            home_directory,
            Self {
                sftp,

                runtime,

                label: format!("SSH: {}", target.destination_label(),),

                cache_namespace: format!(
                    "{}__{}__{}",
                    sanitize_cache_component(&target.host,),
                    sanitize_cache_component(target.user.as_deref().unwrap_or("unknown-user",),),
                    target.port,
                ),
            },
        ))
    }

    fn materialize_remote_file(
        &mut self,
        remote_path: &Path,
        progress: &mut dyn FnMut(TransferProgress) -> io::Result<TransferControl>,
    ) -> io::Result<PathBuf> {
        let cache_path = self.cache_path_for(remote_path)?;

        let metadata_path = cache_metadata_path(&cache_path);

        let part_path = cache_part_path(&cache_path);

        let metadata_part_path = cache_part_path(&metadata_path);

        if let Some(parent) = cache_path.parent() {
            fs::create_dir_all(parent)?;
        }

        /*
         * A previous Scry process may have been interrupted during download.
         *
         * A .scry-part file is never considered a valid cached copy, so remove
         * it before beginning a new transfer.
         */
        remove_file_if_present(&part_path)?;

        remove_file_if_present(&metadata_part_path)?;

        let remote_metadata = self.remote_cache_metadata(remote_path)?;

        if cached_file_is_current(&cache_path, &metadata_path, remote_metadata)? {
            /*
             * A cache hit performs no network transfer, but the caller still
             * receives one truthful completed-progress message.
             */
            match progress(TransferProgress {
                transferred_bytes: remote_metadata.size_bytes,

                total_bytes: remote_metadata.size_bytes,
            })? {
                TransferControl::Continue => {}

                TransferControl::Cancel => {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "remote transfer cancelled",
                    ));
                }
            }

            return Ok(cache_path);
        }

        let download_result = self.stream_remote_file_to_part(
            remote_path,
            &part_path,
            remote_metadata.size_bytes,
            progress,
        );

        if let Err(error) = download_result {
            let _ = fs::remove_file(&part_path);

            return Err(error);
        }

        /*
         * Replace the old cache copy only after the complete new file has been
         * downloaded, flushed, synced, and byte-count validated.
         */
        replace_file_atomically(&part_path, &cache_path)?;

        make_cached_file_non_executable(&cache_path)?;

        write_cache_metadata(&metadata_path, remote_metadata)?;

        Ok(cache_path)
    }

    fn remote_cache_metadata(&self, remote_path: &Path) -> io::Result<RemoteCacheMetadata> {
        self.runtime.block_on(async {
            let mut filesystem = self.sftp.fs();

            let metadata = filesystem
                .metadata(remote_path)
                .await
                .map_err(sftp_io_error)?;

            let modified = metadata.modified().map(|time| time.as_system_time());

            let (modified_seconds, modified_nanoseconds) = system_time_parts(modified);

            Ok(RemoteCacheMetadata {
                size_bytes: metadata.len().unwrap_or(0),

                modified_seconds,

                modified_nanoseconds,
            })
        })
    }

    fn stream_remote_file_to_part(
        &self,
        remote_path: &Path,
        part_path: &Path,
        expected_size: u64,
        progress: &mut dyn FnMut(TransferProgress) -> io::Result<TransferControl>,
    ) -> io::Result<()> {
        const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

        const LOCAL_BUFFER_SIZE: usize = 1024 * 1024;

        let mut local_file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(part_path)?;

        match progress(TransferProgress {
            transferred_bytes: 0,

            total_bytes: expected_size,
        })? {
            TransferControl::Continue => {}

            TransferControl::Cancel => {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "remote transfer cancelled",
                ));
            }
        }

        let transferred_bytes = self.runtime.block_on(async {
            let mut options = self.sftp.options();

            options.read(true);

            let remote_file = options.open(remote_path).await.map_err(sftp_io_error)?;

            /*
             * TokioCompatFile is the crate-supported AsyncRead adapter.
             *
             * It owns SFTP offset handling, buffering, and partial reads.
             * Scry must not manually modify the remote offset.
             */
            let mut remote_file = std::pin::pin!(TokioCompatFile::from(remote_file,));

            let mut buffer = vec![0_u8; LOCAL_BUFFER_SIZE];

            let mut transferred_bytes = 0_u64;

            let mut last_progress_update = Instant::now();

            loop {
                let bytes_read = remote_file.as_mut().read(&mut buffer).await?;

                if bytes_read == 0 {
                    break;
                }

                local_file.write_all(&buffer[..bytes_read])?;

                transferred_bytes = transferred_bytes
                    .checked_add(bytes_read as u64)
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "remote transfer byte count overflowed",
                        )
                    })?;

                /*
                 * Updating after every packet destroys throughput. Report
                 * progress at most ten times per second.
                 */
                if last_progress_update.elapsed() >= PROGRESS_INTERVAL {
                    match progress(TransferProgress {
                        transferred_bytes,

                        total_bytes: expected_size,
                    })? {
                        TransferControl::Continue => {}

                        TransferControl::Cancel => {
                            return Err(io::Error::new(
                                io::ErrorKind::Interrupted,
                                "remote transfer cancelled",
                            ));
                        }
                    }

                    last_progress_update = Instant::now();
                }
            }

            Ok::<u64, io::Error>(transferred_bytes)
        })?;

        /*
         * Always report the exact final amount even if completion occurred
         * between scheduled progress updates.
         */
        match progress(TransferProgress {
            transferred_bytes,

            total_bytes: expected_size,
        })? {
            TransferControl::Continue => {}

            TransferControl::Cancel => {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "remote transfer cancelled",
                ));
            }
        }

        local_file.flush()?;

        local_file.sync_all()?;

        if transferred_bytes != expected_size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "downloaded {} bytes but expected {} bytes from {}",
                    transferred_bytes,
                    expected_size,
                    remote_path.display(),
                ),
            ));
        }

        let local_size = local_file.metadata()?.len();

        if local_size != expected_size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "temporary cache file contains {} bytes but expected {}",
                    local_size, expected_size,
                ),
            ));
        }

        Ok(())
    }

    fn cache_path_for(&self, remote_path: &Path) -> io::Result<PathBuf> {
        let filename = remote_path
            .file_name()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("remote path has no filename: {}", remote_path.display(),),
                )
            })?
            .to_string_lossy();

        let filename = sanitize_cache_filename(&filename);

        let path_hash = stable_path_hash(remote_path);

        Ok(scry_remote_cache_root()?
            .join(&self.cache_namespace)
            .join(format!("{path_hash:016x}__{filename}",)))
    }

    fn read_remote_directory(&self, directory: &Path) -> io::Result<Vec<FileEntry>> {
        self.runtime.block_on(async {
            let mut filesystem = self.sftp.fs();

            let remote_directory = filesystem
                .open_dir(directory)
                .await
                .map_err(sftp_io_error)?;

            let mut entries = Vec::new();

            {
                let mut read_directory = std::pin::pin!(remote_directory.read_dir());

                while let Some(entry) = read_directory
                    .as_mut()
                    .try_next()
                    .await
                    .map_err(sftp_io_error)?
                {
                    let Some(file_entry) = remote_file_entry(directory, entry) else {
                        continue;
                    };

                    entries.push(file_entry);
                }
            }

            Ok(entries)
        })
    }

    fn remote_directory_has_content(&self, directory: &Path) -> io::Result<bool> {
        self.runtime.block_on(async {
            let mut filesystem = self.sftp.fs();

            let remote_directory = filesystem
                .open_dir(directory)
                .await
                .map_err(sftp_io_error)?;

            let mut read_directory = std::pin::pin!(remote_directory.read_dir());

            while let Some(entry) = read_directory
                .as_mut()
                .try_next()
                .await
                .map_err(sftp_io_error)?
            {
                let name = entry.filename().to_string_lossy();

                if name != "." && name != ".." {
                    return Ok(true);
                }
            }

            Ok(false)
        })
    }

    fn remote_path_is_directory(&self, path: &Path) -> io::Result<bool> {
        self.runtime.block_on(async {
            let mut filesystem = self.sftp.fs();

            let metadata = filesystem.metadata(path).await.map_err(sftp_io_error)?;

            Ok(metadata
                .file_type()
                .is_some_and(|file_type| file_type.is_dir()))
        })
    }
}

impl FileSource for SftpSource {
    fn read_directory(
        &mut self,
        directory: &Path,
        sort_mode: SortMode,
        sort_descending: bool,
    ) -> io::Result<Vec<FileEntry>> {
        let mut entries = self.read_remote_directory(directory)?;

        sort_entries(&mut entries, sort_mode, sort_descending);

        Ok(entries)
    }

    fn directory_has_content(&mut self, directory: &Path) -> io::Result<bool> {
        self.remote_directory_has_content(directory)
    }

    fn path_is_directory(&mut self, path: &Path) -> io::Result<bool> {
        self.remote_path_is_directory(path)
    }

    fn supports_recursive_scan(&self) -> bool {
        false
    }

    fn source_label(&self) -> String {
        self.label.clone()
    }

    fn materialize_file(
        &mut self,
        path: &Path,
        progress: &mut dyn FnMut(TransferProgress) -> io::Result<TransferControl>,
    ) -> io::Result<PathBuf> {
        self.materialize_remote_file(path, progress)
    }

    fn is_remote(&self) -> bool {
        true
    }
}

fn remote_file_entry(directory: &Path, entry: DirEntry) -> Option<FileEntry> {
    let name = entry.filename().to_string_lossy().into_owned();

    if name == "." || name == ".." {
        return None;
    }

    let path = directory.join(&name);

    let metadata = entry.metadata();

    let kind = remote_entry_kind(entry.file_type());

    let entry_metadata = EntryMetadata {
        kind,

        permissions_mode: remote_permissions_mode(metadata.permissions()),

        size_bytes: metadata.len().unwrap_or(0),

        modified_time: metadata.modified().map(|time| time.as_system_time()),

        owner_id: metadata.uid(),
    };

    let permissions = format_remote_permissions(&entry_metadata);

    let modified = format_remote_modified_date(entry_metadata.modified_time);

    let class = classify(&path, &entry_metadata);

    let relative_path = PathBuf::from(&name);

    let searchable_path = name.to_lowercase();

    Some(FileEntry {
        path,

        relative_path,

        searchable_path,

        name,

        is_directory: kind.is_directory(),

        is_symlink: kind.is_symlink(),

        permissions,

        modified,

        modified_time: entry_metadata.modified_time,

        owner_id: entry_metadata.owner_id.unwrap_or(0),

        size_bytes: entry_metadata.size_bytes,

        class,
    })
}

fn remote_entry_kind(file_type: Option<FileType>) -> EntryKind {
    let Some(file_type) = file_type else {
        return EntryKind::Unknown;
    };

    if file_type.is_dir() {
        EntryKind::Directory
    } else if file_type.is_symlink() {
        EntryKind::Symlink
    } else if file_type.is_socket() {
        EntryKind::Socket
    } else if file_type.is_fifo() {
        EntryKind::Fifo
    } else if file_type.is_block_device() {
        EntryKind::BlockDevice
    } else if file_type.is_char_device() {
        EntryKind::CharDevice
    } else if file_type.is_file() {
        EntryKind::File
    } else {
        EntryKind::Unknown
    }
}

fn remote_permissions_mode(permissions: Option<Permissions>) -> u32 {
    let Some(permissions) = permissions else {
        return 0;
    };

    let mut mode = 0_u32;

    if permissions.suid() {
        mode |= 0o4000;
    }

    if permissions.sgid() {
        mode |= 0o2000;
    }

    if permissions.svtx() {
        mode |= 0o1000;
    }

    if permissions.read_by_owner() {
        mode |= 0o400;
    }

    if permissions.write_by_owner() {
        mode |= 0o200;
    }

    if permissions.execute_by_owner() {
        mode |= 0o100;
    }

    if permissions.read_by_group() {
        mode |= 0o040;
    }

    if permissions.write_by_group() {
        mode |= 0o020;
    }

    if permissions.execute_by_group() {
        mode |= 0o010;
    }

    if permissions.read_by_other() {
        mode |= 0o004;
    }

    if permissions.write_by_other() {
        mode |= 0o002;
    }

    if permissions.execute_by_other() {
        mode |= 0o001;
    }

    mode
}

fn format_remote_modified_date(modified_time: Option<SystemTime>) -> String {
    let Some(modified_time) = modified_time else {
        return "—".to_string();
    };

    let modified: DateTime<Local> = DateTime::from(modified_time);

    modified.format("%Y-%m-%d %H:%M").to_string()
}

fn format_remote_permissions(metadata: &EntryMetadata) -> String {
    let mut permissions = String::with_capacity(10);

    permissions.push(metadata.kind.permission_type_character());

    let mode = metadata.permissions_mode;

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

fn cache_metadata_path(cache_path: &Path) -> PathBuf {
    let mut value = cache_path.as_os_str().to_os_string();

    value.push(CACHE_METADATA_SUFFIX);

    PathBuf::from(value)
}

fn cache_part_path(cache_path: &Path) -> PathBuf {
    let mut value = cache_path.as_os_str().to_os_string();

    value.push(PART_FILE_SUFFIX);

    PathBuf::from(value)
}

fn system_time_parts(time: Option<SystemTime>) -> (Option<u64>, Option<u32>) {
    let Some(time) = time else {
        return (None, None);
    };

    let Ok(duration) = time.duration_since(UNIX_EPOCH) else {
        return (None, None);
    };

    (Some(duration.as_secs()), Some(duration.subsec_nanos()))
}

fn cached_file_is_current(
    cache_path: &Path,
    metadata_path: &Path,
    remote_metadata: RemoteCacheMetadata,
) -> io::Result<bool> {
    let cache_metadata = match fs::metadata(cache_path) {
        Ok(metadata) => metadata,

        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(false);
        }

        Err(error) => {
            return Err(error);
        }
    };

    if !cache_metadata.is_file() {
        return Ok(false);
    }

    if cache_metadata.len() != remote_metadata.size_bytes {
        return Ok(false);
    }

    let stored_metadata = match read_cache_metadata(metadata_path)? {
        Some(metadata) => metadata,

        None => {
            return Ok(false);
        }
    };

    Ok(stored_metadata == remote_metadata)
}

fn read_cache_metadata(metadata_path: &Path) -> io::Result<Option<RemoteCacheMetadata>> {
    let content = match fs::read_to_string(metadata_path) {
        Ok(content) => content,

        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(None);
        }

        Err(error) => {
            return Err(error);
        }
    };

    let mut size_bytes = None;

    let mut modified_seconds = None;

    let mut modified_nanoseconds = None;

    let mut modified_present = false;

    for line in content.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        match key {
            "size" => {
                size_bytes = value.parse::<u64>().ok();
            }

            "modified_seconds" => {
                modified_present = true;

                modified_seconds = if value == "none" {
                    None
                } else {
                    value.parse::<u64>().ok()
                };
            }

            "modified_nanoseconds" => {
                modified_nanoseconds = if value == "none" {
                    None
                } else {
                    value.parse::<u32>().ok()
                };
            }

            _ => {}
        }
    }

    let Some(size_bytes) = size_bytes else {
        return Ok(None);
    };

    if !modified_present {
        return Ok(None);
    }

    Ok(Some(RemoteCacheMetadata {
        size_bytes,

        modified_seconds,

        modified_nanoseconds,
    }))
}

fn write_cache_metadata(metadata_path: &Path, metadata: RemoteCacheMetadata) -> io::Result<()> {
    let part_path = cache_part_path(metadata_path);

    let modified_seconds = metadata
        .modified_seconds
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string());

    let modified_nanoseconds = metadata
        .modified_nanoseconds
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string());

    let content = format!(
        "size={}\nmodified_seconds={}\nmodified_nanoseconds={}\n",
        metadata.size_bytes, modified_seconds, modified_nanoseconds,
    );

    fs::write(&part_path, content)?;

    replace_file_atomically(&part_path, metadata_path)
}

fn replace_file_atomically(source: &Path, destination: &Path) -> io::Result<()> {
    match fs::rename(source, destination) {
        Ok(()) => {
            return Ok(());
        }

        Err(error)
            if error.kind() != io::ErrorKind::AlreadyExists
                && error.kind() != io::ErrorKind::PermissionDenied =>
        {
            return Err(error);
        }

        Err(_) => {}
    }

    /*
     * Windows does not replace an existing destination through rename().
     * Unix normally does, but this fallback also handles unusual filesystems.
     */
    remove_file_if_present(destination)?;

    fs::rename(source, destination)
}

fn remove_file_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),

        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),

        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn make_cached_file_non_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = fs::Permissions::from_mode(0o600);

    fs::set_permissions(path, permissions)
}

#[cfg(not(unix))]
fn make_cached_file_non_executable(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn sftp_io_error(error: openssh_sftp_client::Error) -> io::Error {
    io::Error::other(format!("{error:#?}"))
}

impl fmt::Display for SshTargetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => {
                write!(formatter, "SSH target cannot be empty",)
            }

            Self::MissingUser => {
                write!(formatter, "SSH target contains an empty username",)
            }

            Self::MissingHost => {
                write!(formatter, "SSH target contains an empty hostname",)
            }

            Self::InvalidPort(port) => {
                write!(formatter, "invalid SSH port: {}", port,)
            }
        }
    }
}

impl std::error::Error for SshTargetError {}

fn parse_host_and_port(value: &str) -> Result<(String, u16), SshTargetError> {
    /*
     * Bracketed addresses permit an IPv6 host and optional port:
     *
     *     [2001:db8::10]
     *     [2001:db8::10]:2222
     */
    if let Some(remainder) = value.strip_prefix('[') {
        let Some(closing_bracket) = remainder.find(']') else {
            return Err(SshTargetError::MissingHost);
        };

        let host = &remainder[..closing_bracket];

        if host.is_empty() {
            return Err(SshTargetError::MissingHost);
        }

        let suffix = &remainder[closing_bracket + 1..];

        let port = if suffix.is_empty() {
            22
        } else {
            let Some(port) = suffix.strip_prefix(':') else {
                return Err(SshTargetError::InvalidPort(suffix.to_string()));
            };

            parse_port(port)?
        };

        return Ok((host.to_string(), port));
    }

    /*
     * Treat a single colon as host:port.
     *
     * Unbracketed values containing several colons are considered IPv6
     * addresses without an explicitly supplied port.
     */
    if value.matches(':').count() == 1 {
        let (host, port) = value.split_once(':').expect("one colon was counted");

        if host.is_empty() {
            return Err(SshTargetError::MissingHost);
        }

        return Ok((host.to_string(), parse_port(port)?));
    }

    if value.is_empty() {
        return Err(SshTargetError::MissingHost);
    }

    Ok((value.to_string(), 22))
}

fn parse_port(value: &str) -> Result<u16, SshTargetError> {
    if value.is_empty() {
        return Err(SshTargetError::InvalidPort(value.to_string()));
    }

    let port = value
        .parse::<u16>()
        .map_err(|_| SshTargetError::InvalidPort(value.to_string()))?;

    if port == 0 {
        return Err(SshTargetError::InvalidPort(value.to_string()));
    }

    Ok(port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hostname() {
        let target = SshTarget::parse("nosferatu").unwrap();

        assert_eq!(target.host, "nosferatu",);

        assert_eq!(target.user, None,);

        assert_eq!(target.port, 22,);
    }

    #[test]
    fn parses_user_and_hostname() {
        let target = SshTarget::parse("ferusx@nosferatu").unwrap();

        assert_eq!(target.host, "nosferatu",);

        assert_eq!(target.user.as_deref(), Some("ferusx"),);

        assert_eq!(target.port, 22,);
    }

    #[test]
    fn parses_custom_port() {
        let target = SshTarget::parse("ferusx@nosferatu:2222").unwrap();

        assert_eq!(target.host, "nosferatu",);

        assert_eq!(target.port, 2222,);
    }

    #[test]
    fn parses_bracketed_ipv6() {
        let target = SshTarget::parse("ferusx@[2001:db8::10]:2222").unwrap();

        assert_eq!(target.host, "2001:db8::10",);

        assert_eq!(target.port, 2222,);
    }

    #[test]
    fn rejects_zero_port() {
        assert!(SshTarget::parse("nosferatu:0",).is_err(),);
    }
}

fn scry_remote_cache_root() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(path).join("scry").join("remote-files"));
    }

    let home = env::var_os("HOME").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "Neither XDG_CACHE_HOME nor HOME is set",
        )
    })?;

    Ok(PathBuf::from(home)
        .join(".cache")
        .join("scry")
        .join("remote-files"))
}

fn stable_path_hash(path: &Path) -> u64 {
    /*
     * Deterministic FNV-1a.
     *
     * Unlike DefaultHasher, this remains stable between Rust versions and
     * therefore keeps cache filenames reusable between Scry builds.
     */
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;

    const FNV_PRIME: u64 = 0x00000100000001B3;

    let mut hash = FNV_OFFSET_BASIS;

    for byte in path.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);

        hash = hash.wrapping_mul(FNV_PRIME);
    }

    hash
}

fn sanitize_cache_filename(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|character| {
            if character == '/' || character == '\\' || character.is_control() {
                '_'
            } else {
                character
            }
        })
        .collect();

    if sanitized.is_empty() {
        "unnamed".to_string()
    } else {
        sanitized
    }
}

fn sanitize_cache_component(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric()
                || character == '-'
                || character == '_'
                || character == '.'
            {
                character
            } else {
                '_'
            }
        })
        .collect();

    if sanitized.is_empty() {
        "_".to_string()
    } else {
        sanitized
    }
}

// SPDX-License-Identifier: BSD-3-Clause

/*
 * The persistent-index subsystem is connected incrementally.
 *
 * Remove this allowance once the popup, loader, writer, and rebuild controls
 * consume every public operation below.
 */
#![allow(dead_code)]

use std::{
    env, fs,
    io::{self, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, Local};

use serde::{Deserialize, Serialize};

use crate::{classify::FileClass, scan::FileEntry};

const REMOTE_INDEX_DIRECTORY_NAME: &str = "remote-indexes";

const REMOTE_INDEX_FILE_EXTENSION: &str = "scry-index";

pub const REMOTE_INDEX_FORMAT_VERSION: u32 = 1;

/*
 * Opening signature at byte zero.
 *
 * This prevents Scry from attempting to decode an unrelated or damaged file
 * as a remote index.
 */
const REMOTE_INDEX_MAGIC: &[u8; 8] = b"SCRYIDX\0";

/*
 * Fixed completion marker stored at the very end of a successful index.
 *
 * An interrupted .scry-part file never receives this footer and can therefore
 * never be mistaken for a complete index.
 */
const REMOTE_INDEX_FINISHED_MAGIC: &[u8; 8] = b"SCRYDNE\0";

const MAXIMUM_HEADER_SIZE: usize = 1024 * 1024;

/*
 * Fixed footer layout:
 *
 *   8 bytes   completion magic
 *   8 bytes   entry count
 *   8 bytes   completion Unix timestamp
 *   8 bytes   scan duration in milliseconds
 *   4 bytes   flags
 *   4 bytes   reserved
 */
const REMOTE_INDEX_FOOTER_SIZE: u64 = 40;

const FOOTER_FLAG_PARTIAL: u32 = 1 << 0;

/*
 * Stable identity for one remote filesystem index.
 *
 * The index always represents the remote filesystem from "/".
 * The user's current browsing directory is search scope, not index identity.
 */
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RemoteIndexIdentity {
    pub host: String,

    pub user: Option<String>,

    pub port: u16,
}

impl RemoteIndexIdentity {
    pub fn new(host: String, user: Option<String>, port: u16) -> Self {
        Self { host, user, port }
    }

    pub fn display_label(&self) -> String {
        let destination = match self.user.as_deref() {
            Some(user) => format!("{}@{}", user, self.host),

            None => self.host.clone(),
        };

        if self.port == 22 {
            destination
        } else {
            format!("{}:{}", destination, self.port)
        }
    }

    pub fn cache_path(&self) -> io::Result<PathBuf> {
        let identity_hash = stable_identity_hash(self);

        let namespace = format!(
            "{}__{}__{}",
            sanitize_component(&self.host),
            sanitize_component(self.user.as_deref().unwrap_or("default-user")),
            self.port,
        );

        Ok(remote_index_cache_root()?.join(namespace).join(format!(
            "v{}__{:016x}.{}",
            REMOTE_INDEX_FORMAT_VERSION, identity_hash, REMOTE_INDEX_FILE_EXTENSION,
        )))
    }

    pub fn part_path(&self) -> io::Result<PathBuf> {
        Ok(append_suffix(&self.cache_path()?, ".scry-part"))
    }

    pub fn inspect(&self) -> io::Result<RemoteIndexStatus> {
        inspect_remote_index_path(&self.cache_path()?, self)
    }
}

/*
 * The scan policy is written into the header.
 *
 * Future changes to limits or exclusions can therefore invalidate an older
 * index explicitly rather than silently interpreting it under new rules.
 */
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemoteIndexScanMode {
    Fast,

    Total,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteIndexHeader {
    pub format_version: u32,

    pub identity: RemoteIndexIdentity,

    pub root: PathBuf,

    pub includes_hidden: bool,

    pub scan_mode: RemoteIndexScanMode,

    pub fast_entry_limit: u64,

    pub skipped_root_directories: Vec<String>,
}

/*
 * Portable cached representation of one remote filesystem entry.
 *
 * FileEntry itself remains an in-memory application structure. Keeping a
 * separate cache DTO prevents ordinary UI changes from silently changing the
 * persistent file format.
 */
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedRemoteEntry {
    pub path: PathBuf,

    pub relative_path: PathBuf,

    pub name: String,

    pub is_directory: bool,

    pub is_symlink: bool,

    pub permissions: String,

    pub modified_seconds: Option<u64>,

    pub modified_nanoseconds: Option<u32>,

    pub owner_id: u32,

    pub size_bytes: u64,

    /*
     * Stable numeric classification code.
     *
     * This is deliberately not FileClass itself. The persistent format must
     * not depend on Rust enum variant ordering.
     */
    pub class_code: u16,
}

impl CachedRemoteEntry {
    pub fn from_file_entry(entry: &FileEntry) -> Self {
        let (modified_seconds, modified_nanoseconds) = system_time_parts(entry.modified_time);

        Self {
            path: entry.path.clone(),

            relative_path: entry.relative_path.clone(),

            name: entry.name.clone(),

            is_directory: entry.is_directory,

            is_symlink: entry.is_symlink,

            permissions: entry.permissions.clone(),

            modified_seconds,

            modified_nanoseconds,

            owner_id: entry.owner_id,

            size_bytes: entry.size_bytes,

            class_code: file_class_code(entry.class),
        }
    }

    pub fn into_file_entry(self) -> io::Result<FileEntry> {
        let modified_time = cached_system_time(self.modified_seconds, self.modified_nanoseconds)?;

        let searchable_path: std::sync::Arc<str> =
            std::sync::Arc::from(self.relative_path.to_string_lossy().to_lowercase());

        let searchable_name: std::sync::Arc<str> = std::sync::Arc::from(self.name.to_lowercase());

        Ok(FileEntry {
            path: self.path,

            relative_path: self.relative_path,

            searchable_path,

            searchable_name,

            name: self.name,

            is_directory: self.is_directory,

            is_symlink: self.is_symlink,

            permissions: self.permissions,

            modified: format_cached_modified_date(modified_time),

            modified_time,

            owner_id: self.owner_id,

            size_bytes: self.size_bytes,

            class: file_class_from_code(self.class_code)?,
        })
    }
}

#[derive(Debug)]
pub enum RemoteIndexBuildMessage {
    Progress { entries_written: u64 },

    Finished(RemoteIndexInfo),

    Failed { message: String },
}

/*
 * Payload batches are independently length-framed.
 *
 * A loader can therefore decode the index progressively without allocating a
 * second vector containing the complete filesystem.
 */
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CachedRemoteBatch {
    entries: Vec<CachedRemoteEntry>,
}

/*
 * Metadata exposed to App and the future Remote Index popup.
 *
 * This contains no file entries. It describes a structurally valid completed
 * cache.
 */
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteIndexInfo {
    pub identity: RemoteIndexIdentity,

    pub includes_hidden: bool,

    pub entry_count: u64,

    pub partial: bool,

    pub completed_at_seconds: u64,

    pub scan_duration_milliseconds: u64,

    pub cache_path: PathBuf,

    pub scan_mode: RemoteIndexScanMode,

    pub fast_entry_limit: u64,

    pub skipped_root_directories: Vec<String>,
}

#[derive(Debug)]
pub struct LoadedRemoteIndex {
    pub info: RemoteIndexInfo,

    pub entries: Vec<FileEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteIndexStatus {
    Missing,

    Valid(RemoteIndexInfo),

    Invalid { path: PathBuf, reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RemoteIndexFooter {
    entry_count: u64,

    completed_at_seconds: u64,

    scan_duration_milliseconds: u64,

    partial: bool,
}

/*
 * Streaming atomic writer for one remote index rebuild.
 *
 * Data is written only to the part file. An older completed index remains
 * untouched until finish() has flushed, synced, and validated the replacement.
 */
#[derive(Debug)]
pub struct RemoteIndexWriter {
    identity: RemoteIndexIdentity,

    final_path: PathBuf,

    part_path: PathBuf,

    writer: Option<BufWriter<fs::File>>,

    entry_count: u64,

    started_at: Instant,

    committed: bool,
}

impl RemoteIndexWriter {
    pub fn create(
        identity: RemoteIndexIdentity,
        includes_hidden: bool,
        scan_mode: RemoteIndexScanMode,
        fast_entry_limit: u64,
        skipped_root_directories: Vec<String>,
    ) -> io::Result<Self> {
        let final_path = identity.cache_path()?;

        let part_path = identity.part_path()?;

        let parent = final_path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("remote index path has no parent: {}", final_path.display(),),
            )
        })?;

        fs::create_dir_all(parent)?;

        make_directory_private(parent)?;

        remove_file_if_present(&part_path)?;

        let file = create_private_part_file(&part_path)?;

        let mut writer = BufWriter::new(file);

        write_header(
            &mut writer,
            &RemoteIndexHeader {
                format_version: REMOTE_INDEX_FORMAT_VERSION,

                identity: identity.clone(),

                root: PathBuf::from("/"),

                includes_hidden,

                scan_mode,

                fast_entry_limit,

                skipped_root_directories,
            },
        )?;

        Ok(Self {
            identity,

            final_path,

            part_path,

            writer: Some(writer),

            entry_count: 0,

            started_at: Instant::now(),

            committed: false,
        })
    }

    pub fn write_batch(&mut self, entries: &[CachedRemoteEntry]) -> io::Result<()> {
        if entries.is_empty() {
            return Ok(());
        }

        let batch = CachedRemoteBatch {
            entries: entries.to_vec(),
        };

        let encoded = bincode::serde::encode_to_vec(&batch, bincode::config::standard()).map_err(
            |error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unable to encode remote-index batch: {}", error,),
                )
            },
        )?;

        let encoded_length = u32::try_from(encoded.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "remote-index batch exceeds the supported size",
            )
        })?;

        let writer = self.writer.as_mut().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "remote-index writer is already finished",
            )
        })?;

        writer.write_all(&encoded_length.to_le_bytes())?;

        writer.write_all(&encoded)?;

        self.entry_count = self
            .entry_count
            .checked_add(entries.len() as u64)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "remote-index entry count overflowed",
                )
            })?;

        Ok(())
    }

    pub fn finish(mut self, partial: bool) -> io::Result<RemoteIndexInfo> {
        let completed_at_seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let scan_duration_milliseconds =
            u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX);

        let mut writer = self.writer.take().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "remote-index writer is already finished",
            )
        })?;

        write_footer(
            &mut writer,
            RemoteIndexFooter {
                entry_count: self.entry_count,

                completed_at_seconds,

                scan_duration_milliseconds,

                partial,
            },
        )?;

        writer.flush()?;

        writer.get_ref().sync_all()?;

        drop(writer);

        /*
         * Validate the completed part file before allowing it to replace an
         * older working index.
         */
        match inspect_remote_index_path(&self.part_path, &self.identity)? {
            RemoteIndexStatus::Valid(_) => {}

            RemoteIndexStatus::Missing => {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "completed remote-index part file disappeared: {}",
                        self.part_path.display(),
                    ),
                ));
            }

            RemoteIndexStatus::Invalid { reason, .. } => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "completed remote-index part file failed validation: {}",
                        reason,
                    ),
                ));
            }
        }

        replace_file_atomically(&self.part_path, &self.final_path)?;

        make_file_private(&self.final_path)?;

        sync_parent_directory(&self.final_path)?;

        self.committed = true;

        match inspect_remote_index_path(&self.final_path, &self.identity)? {
            RemoteIndexStatus::Valid(info) => Ok(info),

            RemoteIndexStatus::Missing => Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "remote index disappeared after commit: {}",
                    self.final_path.display(),
                ),
            )),

            RemoteIndexStatus::Invalid { reason, .. } => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("committed remote index failed validation: {}", reason,),
            )),
        }
    }

    pub fn entry_count(&self) -> u64 {
        self.entry_count
    }

    pub fn part_path(&self) -> &Path {
        &self.part_path
    }
}

impl Drop for RemoteIndexWriter {
    fn drop(&mut self) {
        if self.committed {
            return;
        }

        /*
         * Close the file before deleting it. This is important on platforms
         * that do not permit unlinking an open file.
         */
        if let Some(mut writer) = self.writer.take() {
            let _ = writer.flush();

            drop(writer);
        }

        let _ = fs::remove_file(&self.part_path);
    }
}

pub fn remote_index_cache_root() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(path)
            .join("scry")
            .join(REMOTE_INDEX_DIRECTORY_NAME));
    }

    let home = env::var_os("HOME").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "neither XDG_CACHE_HOME nor HOME is set",
        )
    })?;

    Ok(PathBuf::from(home)
        .join(".cache")
        .join("scry")
        .join(REMOTE_INDEX_DIRECTORY_NAME))
}

pub fn load_remote_index(identity: &RemoteIndexIdentity) -> io::Result<LoadedRemoteIndex> {
    let info = match identity.inspect()? {
        RemoteIndexStatus::Valid(info) => info,

        RemoteIndexStatus::Missing => {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "remote index for {} does not exist",
                    identity.display_label(),
                ),
            ));
        }

        RemoteIndexStatus::Invalid { reason, .. } => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "remote index for {} is invalid: {}",
                    identity.display_label(),
                    reason,
                ),
            ));
        }
    };

    let mut file = fs::File::open(&info.cache_path)?;

    let file_length = file.metadata()?.len();

    /*
     * Skip the opening magic and read the framed header length.
     */
    file.seek(SeekFrom::Start(REMOTE_INDEX_MAGIC.len() as u64))?;

    let header_length = read_u32(&mut file)? as u64;

    let payload_start = REMOTE_INDEX_MAGIC.len() as u64 + size_of::<u32>() as u64 + header_length;

    let payload_end = file_length
        .checked_sub(REMOTE_INDEX_FOOTER_SIZE)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "remote index is smaller than its completion footer",
            )
        })?;

    if payload_start > payload_end {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "remote-index header overlaps its payload footer",
        ));
    }

    file.seek(SeekFrom::Start(payload_start))?;

    let initial_capacity = usize::try_from(info.entry_count).unwrap_or(0);

    let mut entries = Vec::with_capacity(initial_capacity);

    while file.stream_position()? < payload_end {
        let frame_start = file.stream_position()?;

        let remaining = payload_end.saturating_sub(frame_start);

        if remaining < size_of::<u32>() as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "remote-index payload ends with {} incomplete framing bytes",
                    remaining,
                ),
            ));
        }

        let encoded_length = read_u32(&mut file)? as u64;

        if encoded_length == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "remote-index payload contains an empty batch frame",
            ));
        }

        let encoded_start = file.stream_position()?;

        let encoded_end = encoded_start.checked_add(encoded_length).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "remote-index batch length overflowed",
            )
        })?;

        if encoded_end > payload_end {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "remote-index batch extends {} bytes into the completion footer",
                    encoded_end.saturating_sub(payload_end),
                ),
            ));
        }

        let encoded_length_usize = usize::try_from(encoded_length).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "remote-index batch is too large for this platform",
            )
        })?;

        let mut encoded = vec![0_u8; encoded_length_usize];

        file.read_exact(&mut encoded)?;

        let (batch, consumed_bytes): (CachedRemoteBatch, usize) =
            bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).map_err(
                |error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("unable to decode remote-index batch: {}", error,),
                    )
                },
            )?;

        if consumed_bytes != encoded.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "remote-index batch contains {} trailing bytes",
                    encoded.len().saturating_sub(consumed_bytes),
                ),
            ));
        }

        for cached_entry in batch.entries {
            entries.push(cached_entry.into_file_entry()?);
        }
    }

    if file.stream_position()? != payload_end {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "remote-index loader did not stop at the completion footer",
        ));
    }

    let decoded_count = entries.len() as u64;

    if decoded_count != info.entry_count {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "remote-index footer declares {} entries but {} were decoded",
                info.entry_count, decoded_count,
            ),
        ));
    }

    Ok(LoadedRemoteIndex { info, entries })
}

pub fn inspect_remote_index_path(
    path: &Path,
    expected_identity: &RemoteIndexIdentity,
) -> io::Result<RemoteIndexStatus> {
    let mut file = match fs::File::open(path) {
        Ok(file) => file,

        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(RemoteIndexStatus::Missing);
        }

        Err(error) => {
            return Err(error);
        }
    };

    let file_length = file.metadata()?.len();

    let minimum_length =
        REMOTE_INDEX_MAGIC.len() as u64 + size_of::<u32>() as u64 + REMOTE_INDEX_FOOTER_SIZE;

    if file_length < minimum_length {
        return Ok(invalid_status(
            path,
            format!(
                "index is truncated: {} bytes is smaller than the minimum {} bytes",
                file_length, minimum_length,
            ),
        ));
    }

    let mut opening_magic = [0_u8; 8];

    if let Err(error) = file.read_exact(&mut opening_magic) {
        return Ok(invalid_status(
            path,
            format!("unable to read index signature: {}", error),
        ));
    }

    if &opening_magic != REMOTE_INDEX_MAGIC {
        return Ok(invalid_status(
            path,
            "index signature does not match Scry's remote-index format",
        ));
    }

    let header_length = match read_u32(&mut file) {
        Ok(length) => length as usize,

        Err(error) => {
            return Ok(invalid_status(
                path,
                format!("unable to read index-header length: {}", error),
            ));
        }
    };

    if header_length == 0 {
        return Ok(invalid_status(path, "index header is empty"));
    }

    if header_length > MAXIMUM_HEADER_SIZE {
        return Ok(invalid_status(
            path,
            format!(
                "index header claims an unreasonable size of {} bytes",
                header_length,
            ),
        ));
    }

    let payload_start =
        REMOTE_INDEX_MAGIC.len() as u64 + size_of::<u32>() as u64 + header_length as u64;

    if payload_start > file_length.saturating_sub(REMOTE_INDEX_FOOTER_SIZE) {
        return Ok(invalid_status(
            path,
            "index header overlaps the completion footer",
        ));
    }

    let mut header_bytes = vec![0_u8; header_length];

    if let Err(error) = file.read_exact(&mut header_bytes) {
        return Ok(invalid_status(
            path,
            format!("unable to read complete index header: {}", error),
        ));
    }

    let header = match decode_header(&header_bytes) {
        Ok(header) => header,

        Err(error) => {
            return Ok(invalid_status(
                path,
                format!("unable to decode index header: {}", error),
            ));
        }
    };

    if header.format_version != REMOTE_INDEX_FORMAT_VERSION {
        return Ok(invalid_status(
            path,
            format!(
                "index format version {} is not supported by this Scry build (expected {})",
                header.format_version, REMOTE_INDEX_FORMAT_VERSION,
            ),
        ));
    }

    if &header.identity != expected_identity {
        return Ok(invalid_status(
            path,
            format!(
                "index belongs to {}, not {}",
                header.identity.display_label(),
                expected_identity.display_label(),
            ),
        ));
    }

    if header.root != Path::new("/") {
        return Ok(invalid_status(
            path,
            format!(
                "index covers {} instead of the complete remote filesystem /",
                header.root.display(),
            ),
        ));
    }

    let footer = match read_footer(&mut file, file_length) {
        Ok(footer) => footer,

        Err(error) => {
            return Ok(invalid_status(
                path,
                format!("unable to validate completion footer: {}", error),
            ));
        }
    };

    Ok(RemoteIndexStatus::Valid(RemoteIndexInfo {
        identity: header.identity,

        includes_hidden: header.includes_hidden,

        entry_count: footer.entry_count,

        partial: footer.partial,

        completed_at_seconds: footer.completed_at_seconds,

        scan_duration_milliseconds: footer.scan_duration_milliseconds,

        cache_path: path.to_path_buf(),

        scan_mode: header.scan_mode,

        fast_entry_limit: header.fast_entry_limit,

        skipped_root_directories: header.skipped_root_directories,
    }))
}

fn decode_header(bytes: &[u8]) -> io::Result<RemoteIndexHeader> {
    let (header, consumed_bytes) =
        bincode::serde::decode_from_slice(bytes, bincode::config::standard()).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid Bincode header: {}", error),
            )
        })?;

    if consumed_bytes != bytes.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "header contains {} trailing bytes",
                bytes.len().saturating_sub(consumed_bytes),
            ),
        ));
    }

    Ok(header)
}

fn read_footer(file: &mut fs::File, file_length: u64) -> io::Result<RemoteIndexFooter> {
    file.seek(SeekFrom::Start(
        file_length.saturating_sub(REMOTE_INDEX_FOOTER_SIZE),
    ))?;

    let mut finished_magic = [0_u8; 8];

    file.read_exact(&mut finished_magic)?;

    if &finished_magic != REMOTE_INDEX_FINISHED_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "completion marker is missing",
        ));
    }

    let entry_count = read_u64(file)?;

    let completed_at_seconds = read_u64(file)?;

    let scan_duration_milliseconds = read_u64(file)?;

    let flags = read_u32(file)?;

    let reserved = read_u32(file)?;

    if reserved != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "completion footer contains unsupported reserved data",
        ));
    }

    let known_flags = FOOTER_FLAG_PARTIAL;

    if flags & !known_flags != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("completion footer contains unknown flags: 0x{flags:08x}"),
        ));
    }

    Ok(RemoteIndexFooter {
        entry_count,

        completed_at_seconds,

        scan_duration_milliseconds,

        partial: flags & FOOTER_FLAG_PARTIAL != 0,
    })
}

fn invalid_status(path: &Path, reason: impl Into<String>) -> RemoteIndexStatus {
    RemoteIndexStatus::Invalid {
        path: path.to_path_buf(),

        reason: reason.into(),
    }
}

fn read_u32(reader: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0_u8; size_of::<u32>()];

    reader.read_exact(&mut bytes)?;

    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(reader: &mut impl Read) -> io::Result<u64> {
    let mut bytes = [0_u8; size_of::<u64>()];

    reader.read_exact(&mut bytes)?;

    Ok(u64::from_le_bytes(bytes))
}

/*
 * These writers define the exact framing that the next implementation stage
 * will use. They remain private for now except through unit tests.
 */
fn write_header(writer: &mut impl Write, header: &RemoteIndexHeader) -> io::Result<()> {
    let header_bytes =
        bincode::serde::encode_to_vec(header, bincode::config::standard()).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unable to encode index header: {}", error),
            )
        })?;

    let header_length = u32::try_from(header_bytes.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "remote-index header is too large",
        )
    })?;

    writer.write_all(REMOTE_INDEX_MAGIC)?;

    writer.write_all(&header_length.to_le_bytes())?;

    writer.write_all(&header_bytes)
}

fn write_footer(writer: &mut impl Write, footer: RemoteIndexFooter) -> io::Result<()> {
    let flags = if footer.partial {
        FOOTER_FLAG_PARTIAL
    } else {
        0
    };

    writer.write_all(REMOTE_INDEX_FINISHED_MAGIC)?;

    writer.write_all(&footer.entry_count.to_le_bytes())?;

    writer.write_all(&footer.completed_at_seconds.to_le_bytes())?;

    writer.write_all(&footer.scan_duration_milliseconds.to_le_bytes())?;

    writer.write_all(&flags.to_le_bytes())?;

    writer.write_all(&0_u32.to_le_bytes())
}

fn remove_file_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),

        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),

        Err(error) => Err(error),
    }
}

fn create_private_part_file(path: &Path) -> io::Result<fs::File> {
    let file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;

    make_file_private(path)?;

    Ok(file)
}

fn replace_file_atomically(source: &Path, destination: &Path) -> io::Result<()> {
    /*
     * On Scry's Unix targets, rename replaces an existing destination
     * atomically when both files belong to the same filesystem.
     *
     * The part file is deliberately created beside the final file.
     */
    fs::rename(source, destination)
}

#[cfg(unix)]
fn make_directory_private(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn make_directory_private(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn make_file_private(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn make_file_private(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path has no parent: {}", path.display()),
        )
    })?;

    fs::File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn system_time_parts(time: Option<SystemTime>) -> (Option<u64>, Option<u32>) {
    let Some(time) = time else {
        return (None, None);
    };

    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => (Some(duration.as_secs()), Some(duration.subsec_nanos())),

        Err(_) => (None, None),
    }
}

fn cached_system_time(
    seconds: Option<u64>,
    nanoseconds: Option<u32>,
) -> io::Result<Option<SystemTime>> {
    let Some(seconds) = seconds else {
        if nanoseconds.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "cached modification nanoseconds exist without seconds",
            ));
        }

        return Ok(None);
    };

    let nanoseconds = nanoseconds.unwrap_or(0);

    if nanoseconds >= 1_000_000_000 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "cached modification nanoseconds are invalid: {}",
                nanoseconds,
            ),
        ));
    }

    let duration = Duration::new(seconds, nanoseconds);

    let time = UNIX_EPOCH.checked_add(duration).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "cached modification time overflowed SystemTime",
        )
    })?;

    Ok(Some(time))
}

fn format_cached_modified_date(modified_time: Option<SystemTime>) -> String {
    let Some(modified_time) = modified_time else {
        return "—".to_string();
    };

    let date: DateTime<Local> = modified_time.into();

    date.format("%Y-%m-%d %H:%M").to_string()
}

fn file_class_from_code(code: u16) -> io::Result<FileClass> {
    let class = match code {
        0 => FileClass::Unknown,
        1 => FileClass::Directory,
        2 => FileClass::Symlink,
        3 => FileClass::Executable,
        4 => FileClass::ShellScript,
        5 => FileClass::Rust,
        6 => FileClass::Python,
        7 => FileClass::C,
        8 => FileClass::Cpp,
        9 => FileClass::Java,
        10 => FileClass::Kotlin,
        11 => FileClass::JavaScript,
        12 => FileClass::TypeScript,
        13 => FileClass::Web,
        14 => FileClass::SourceCode,
        15 => FileClass::Build,
        16 => FileClass::Config,
        17 => FileClass::StructuredData,
        18 => FileClass::Log,
        19 => FileClass::Archive,
        20 => FileClass::Package,
        21 => FileClass::Document,
        22 => FileClass::Spreadsheet,
        23 => FileClass::Presentation,
        24 => FileClass::Image,
        25 => FileClass::VectorImage,
        26 => FileClass::Audio,
        27 => FileClass::Video,
        28 => FileClass::Font,
        29 => FileClass::Database,
        30 => FileClass::Torrent,
        31 => FileClass::DesktopEntry,
        32 => FileClass::Backup,
        33 => FileClass::Certificate,
        34 => FileClass::DiskImage,
        35 => FileClass::Plugin,
        36 => FileClass::Text,
        37 => FileClass::Binary,

        38 => FileClass::Assembly,
        39 => FileClass::Lua,
        40 => FileClass::Ruby,
        41 => FileClass::Perl,
        42 => FileClass::Php,
        43 => FileClass::Go,
        44 => FileClass::Swift,
        45 => FileClass::Dart,
        46 => FileClass::CSharp,
        47 => FileClass::Scala,
        48 => FileClass::Groovy,
        49 => FileClass::R,
        50 => FileClass::Awk,
        51 => FileClass::Elixir,
        52 => FileClass::Erlang,
        53 => FileClass::FSharp,
        54 => FileClass::VisualBasic,
        55 => FileClass::Clojure,
        56 => FileClass::Zig,
        57 => FileClass::Nim,
        58 => FileClass::Crystal,
        59 => FileClass::Haskell,
        60 => FileClass::Ocaml,
        61 => FileClass::Pascal,
        62 => FileClass::Solidity,
        63 => FileClass::Vala,

        unknown => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("remote index contains unknown file-class code {}", unknown,),
            ));
        }
    };

    Ok(class)
}

fn file_class_code(class: FileClass) -> u16 {
    match class {
        FileClass::Unknown => 0,
        FileClass::Directory => 1,
        FileClass::Symlink => 2,
        FileClass::Executable => 3,
        FileClass::ShellScript => 4,
        FileClass::Rust => 5,
        FileClass::Python => 6,
        FileClass::C => 7,
        FileClass::Cpp => 8,
        FileClass::Java => 9,
        FileClass::Kotlin => 10,
        FileClass::JavaScript => 11,
        FileClass::TypeScript => 12,
        FileClass::Web => 13,
        FileClass::SourceCode => 14,
        FileClass::Build => 15,
        FileClass::Config => 16,
        FileClass::StructuredData => 17,
        FileClass::Log => 18,
        FileClass::Archive => 19,
        FileClass::Package => 20,
        FileClass::Document => 21,
        FileClass::Spreadsheet => 22,
        FileClass::Presentation => 23,
        FileClass::Image => 24,
        FileClass::VectorImage => 25,
        FileClass::Audio => 26,
        FileClass::Video => 27,
        FileClass::Font => 28,
        FileClass::Database => 29,
        FileClass::Torrent => 30,
        FileClass::DesktopEntry => 31,
        FileClass::Backup => 32,
        FileClass::Certificate => 33,
        FileClass::DiskImage => 34,
        FileClass::Plugin => 35,
        FileClass::Text => 36,
        FileClass::Binary => 37,

        FileClass::Assembly => 38,
        FileClass::Lua => 39,
        FileClass::Ruby => 40,
        FileClass::Perl => 41,
        FileClass::Php => 42,
        FileClass::Go => 43,
        FileClass::Swift => 44,
        FileClass::Dart => 45,
        FileClass::CSharp => 46,
        FileClass::Scala => 47,
        FileClass::Groovy => 48,
        FileClass::R => 49,
        FileClass::Awk => 50,
        FileClass::Elixir => 51,
        FileClass::Erlang => 52,
        FileClass::FSharp => 53,
        FileClass::VisualBasic => 54,
        FileClass::Clojure => 55,
        FileClass::Zig => 56,
        FileClass::Nim => 57,
        FileClass::Crystal => 58,
        FileClass::Haskell => 59,
        FileClass::Ocaml => 60,
        FileClass::Pascal => 61,
        FileClass::Solidity => 62,
        FileClass::Vala => 63,
    }
}

fn stable_identity_hash(identity: &RemoteIndexIdentity) -> u64 {
    /*
     * FNV-1a is deterministic across processes and Rust versions.
     *
     * The complete identity is also stored inside the header and compared
     * during inspection. The filename hash is never treated as proof.
     */
    let mut hash = 0xcbf29ce484222325_u64;

    hash_bytes(&mut hash, identity.host.as_bytes());

    hash_bytes(&mut hash, b"\0");

    if let Some(user) = identity.user.as_deref() {
        hash_bytes(&mut hash, user.as_bytes());
    }

    hash_bytes(&mut hash, b"\0");

    hash_bytes(&mut hash, &identity.port.to_le_bytes());

    hash
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);

        *hash = hash.wrapping_mul(0x100000001b3);
    }
}

fn sanitize_component(value: &str) -> String {
    let mut sanitized = String::with_capacity(value.len());

    for character in value.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
            sanitized.push(character);
        } else {
            sanitized.push('_');
        }
    }

    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();

    value.push(suffix);

    PathBuf::from(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_identity() -> RemoteIndexIdentity {
        RemoteIndexIdentity::new("nosferatu".to_string(), Some("ferusx".to_string()), 22)
    }

    fn test_header(identity: RemoteIndexIdentity) -> RemoteIndexHeader {
        RemoteIndexHeader {
            format_version: REMOTE_INDEX_FORMAT_VERSION,

            identity,

            root: PathBuf::from("/"),

            includes_hidden: true,

            scan_mode: RemoteIndexScanMode::Fast,

            fast_entry_limit: 250_000,

            skipped_root_directories: vec![
                "proc".to_string(),
                "sys".to_string(),
                "dev".to_string(),
                "run".to_string(),
            ],
        }
    }

    fn temporary_path(test_name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        env::temp_dir().join(format!(
            "scry-remote-index-test-{}-{}-{}",
            std::process::id(),
            nonce,
            test_name,
        ))
    }

    fn write_complete_test_index(
        path: &Path,
        header: &RemoteIndexHeader,
        entry_count: u64,
        partial: bool,
    ) {
        let mut file = fs::File::create(path).unwrap();

        write_header(&mut file, header).unwrap();

        /*
         * Placeholder payload. Inspection deliberately validates framing and
         * metadata only; the future loader validates entry records.
         */
        file.write_all(b"test-payload").unwrap();

        write_footer(
            &mut file,
            RemoteIndexFooter {
                entry_count,

                completed_at_seconds: 1_700_000_000,

                scan_duration_milliseconds: 12_345,

                partial,
            },
        )
        .unwrap();

        file.flush().unwrap();
    }

    #[test]
    fn missing_index_is_reported_as_missing() {
        let path = temporary_path("missing");

        let status = inspect_remote_index_path(&path, &test_identity()).unwrap();

        assert_eq!(status, RemoteIndexStatus::Missing);
    }

    #[test]
    fn complete_index_is_reported_as_valid() {
        let path = temporary_path("valid");

        let identity = test_identity();

        write_complete_test_index(&path, &test_header(identity.clone()), 42_000, false);

        let status = inspect_remote_index_path(&path, &identity).unwrap();

        match status {
            RemoteIndexStatus::Valid(info) => {
                assert_eq!(info.identity, identity);

                assert!(info.includes_hidden);

                assert_eq!(info.entry_count, 42_000);

                assert!(!info.partial);
            }

            other => {
                panic!("expected valid index, received {other:?}");
            }
        }

        let _ = fs::remove_file(path);
    }

    #[test]
    fn partial_index_metadata_is_preserved() {
        let path = temporary_path("partial");

        let identity = test_identity();

        write_complete_test_index(&path, &test_header(identity.clone()), 250_000, true);

        let status = inspect_remote_index_path(&path, &identity).unwrap();

        match status {
            RemoteIndexStatus::Valid(info) => {
                assert_eq!(info.entry_count, 250_000);

                assert!(info.partial);
            }

            other => {
                panic!("expected valid partial index, received {other:?}");
            }
        }

        let _ = fs::remove_file(path);
    }

    #[test]
    fn missing_completion_footer_is_invalid() {
        let path = temporary_path("unfinished");

        let identity = test_identity();

        let mut file = fs::File::create(&path).unwrap();

        write_header(&mut file, &test_header(identity.clone())).unwrap();

        file.write_all(b"incomplete payload").unwrap();

        file.flush().unwrap();

        let status = inspect_remote_index_path(&path, &identity).unwrap();

        assert!(matches!(status, RemoteIndexStatus::Invalid { .. }));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn wrong_identity_is_invalid() {
        let path = temporary_path("identity");

        let expected_identity = test_identity();

        let other_identity =
            RemoteIndexIdentity::new("vlad".to_string(), Some("ferusx".to_string()), 22);

        write_complete_test_index(&path, &test_header(other_identity), 10, false);

        let status = inspect_remote_index_path(&path, &expected_identity).unwrap();

        assert!(matches!(status, RemoteIndexStatus::Invalid { .. }));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn non_root_index_is_invalid() {
        let path = temporary_path("root");

        let identity = test_identity();

        let mut header = test_header(identity.clone());

        header.root = PathBuf::from("/home/ferusx");

        write_complete_test_index(&path, &header, 10, false);

        let status = inspect_remote_index_path(&path, &identity).unwrap();

        assert!(matches!(status, RemoteIndexStatus::Invalid { .. }));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn default_port_is_omitted_from_display_label() {
        assert_eq!(test_identity().display_label(), "ferusx@nosferatu");
    }

    #[test]
    fn custom_port_is_included_in_display_label() {
        let identity =
            RemoteIndexIdentity::new("nosferatu".to_string(), Some("ferusx".to_string()), 2222);

        assert_eq!(identity.display_label(), "ferusx@nosferatu:2222");
    }

    fn test_cached_entry(name: &str) -> CachedRemoteEntry {
        CachedRemoteEntry {
            path: PathBuf::from(format!("/home/ferusx/{name}")),

            relative_path: PathBuf::from(format!("home/ferusx/{name}")),

            name: name.to_string(),

            is_directory: false,

            is_symlink: false,

            permissions: ".rw-r--r--".to_string(),

            modified_seconds: Some(1_700_000_000),

            modified_nanoseconds: Some(0),

            owner_id: 1000,

            size_bytes: 1234,

            class_code: 0,
        }
    }

    fn temporary_identity(test_name: &str) -> RemoteIndexIdentity {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        RemoteIndexIdentity::new(
            format!("test-host-{}-{}-{}", std::process::id(), nonce, test_name,),
            Some("test-user".to_string()),
            22,
        )
    }

    fn clean_identity_files(identity: &RemoteIndexIdentity) {
        if let Ok(path) = identity.cache_path() {
            let _ = fs::remove_file(&path);

            if let Some(parent) = path.parent() {
                let _ = fs::remove_dir(parent);
            }
        }

        if let Ok(path) = identity.part_path() {
            let _ = fs::remove_file(path);
        }
    }

    #[test]
    fn writer_commits_a_valid_index() {
        let identity = temporary_identity("commit");

        let mut writer = RemoteIndexWriter::create(
            identity.clone(),
            true,
            RemoteIndexScanMode::Total,
            0,
            vec![
                "proc".to_string(),
                "sys".to_string(),
                "dev".to_string(),
                "run".to_string(),
            ],
        )
        .unwrap();

        writer
            .write_batch(&[test_cached_entry("alpha"), test_cached_entry("beta")])
            .unwrap();

        let info = writer.finish(false).unwrap();

        assert_eq!(info.identity, identity);

        assert_eq!(info.entry_count, 2);

        assert!(info.includes_hidden);

        assert!(!info.partial);

        assert!(info.cache_path.is_file());

        clean_identity_files(&identity);
    }

    #[test]
    fn unfinished_writer_removes_only_the_part_file() {
        let identity = temporary_identity("drop");

        let final_path = identity.cache_path().unwrap();

        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }

        fs::write(&final_path, b"existing-index").unwrap();

        let part_path;

        {
            let mut writer = RemoteIndexWriter::create(
                identity.clone(),
                false,
                RemoteIndexScanMode::Total,
                0,
                Vec::new(),
            )
            .unwrap();

            writer
                .write_batch(&[test_cached_entry("unfinished")])
                .unwrap();

            part_path = writer.part_path().to_path_buf();

            assert!(part_path.is_file());
        }

        assert!(!part_path.exists());

        assert_eq!(fs::read(&final_path).unwrap(), b"existing-index",);

        clean_identity_files(&identity);
    }

    #[test]
    fn rebuilding_replaces_the_previous_index_only_on_finish() {
        let identity = temporary_identity("replace");

        let first_writer = RemoteIndexWriter::create(
            identity.clone(),
            false,
            RemoteIndexScanMode::Total,
            0,
            Vec::new(),
        )
        .unwrap();

        let first_info = first_writer.finish(false).unwrap();

        assert_eq!(first_info.entry_count, 0);

        let original_bytes = fs::read(identity.cache_path().unwrap()).unwrap();

        {
            let mut replacement = RemoteIndexWriter::create(
                identity.clone(),
                true,
                RemoteIndexScanMode::Total,
                0,
                Vec::new(),
            )
            .unwrap();

            replacement
                .write_batch(&[test_cached_entry("replacement")])
                .unwrap();

            assert_eq!(
                fs::read(identity.cache_path().unwrap()).unwrap(),
                original_bytes,
            );

            /*
             * Dropping simulates cancellation or scanner failure.
             */
        }

        assert_eq!(
            fs::read(identity.cache_path().unwrap()).unwrap(),
            original_bytes,
        );

        clean_identity_files(&identity);
    }
}

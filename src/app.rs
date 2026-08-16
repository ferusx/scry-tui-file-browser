// SPDX-License-Identifier: BSD-3-Clause

use chrono::Local;
#[cfg(target_os = "linux")]
use cli_clipboard::{ClipboardContext, ClipboardProvider};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{self, Receiver, TryRecvError},
};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use users::get_user_by_uid;

use crate::classify::{FileClass, inspect_file};
use crate::config::{AdvancedTreeConfig, ScryConfig, SshConfig};
use crate::connection::{ConnectionDialogState, ConnectionStore};
use crate::deletion_journal::{self, DeletionJournalEntry};
use crate::file_info::{FileInfo, FileInfoMessage, FileInfoState};
use crate::fuzzy::{FuzzyWorkerResult, WorkerEntryFilter, start_exact_worker, start_fuzzy_worker};
use crate::query::{entry_matches_query, parse_query};
use crate::remote_index::{
    LoadedRemoteIndex, RemoteIndexBuildMessage, RemoteIndexIdentity, load_remote_index,
};
use crate::scan::{FileEntry, RecursiveScanMode, ScanMessage, SortMode, sort_entries};
use crate::search_index::SearchIndex;
use crate::session::{SESSION_FORMAT_VERSION, SessionMarkedFile, SessionSource, SessionState};
use crate::source::{FileSource, LocalSource, TransferControl, TransferProgress};
use crate::ssh::{SftpSource, SshTarget};
use crate::themes::Theme;

const INFO_NOTIFICATION_DURATION: Duration = Duration::from_secs(5);

const ERROR_NOTIFICATION_DURATION: Duration = Duration::from_secs(7);

/*
 * The remote-index loading message completes one dim → bright → dim pulse every
 * 700 ms.
 *
 * Redrawing every 50 ms provides fourteen brightness stages per cycle without
 * placing unnecessary pressure on the terminal renderer.
 */
const REMOTE_INDEX_NOTIFICATION_PULSE_CYCLE: Duration = Duration::from_millis(700);

const REMOTE_INDEX_NOTIFICATION_PULSE_FRAME: Duration = Duration::from_millis(50);

/*
 * Every staged deletion remains beside its original path under a private,
 * hidden sibling name.
 *
 * The prefix is intentionally distinctive so Scry can identify its own
 * temporary entries without confusing them with ordinary user dotfiles.
 */
const STAGED_DELETION_PREFIX: &str = ".scry-deleted-";

/*
 * Monotonic process-local discriminator.
 *
 * Timestamp and process ID identify the session, while this counter guarantees
 * that two staged names generated during the same clock tick remain distinct.
 */
static STAGED_DELETION_COUNTER: AtomicU64 = AtomicU64::new(0);

/*
 * Recursive indexes may contain millions of records.
 *
 * Query text is drawn immediately, but background searching waits briefly for
 * a natural typing pause so one rapid word does not launch one complete worker
 * generation per character.
 */
const RECURSIVE_SEARCH_DEBOUNCE: Duration = Duration::from_millis(75);

/*
 * Local Exact Recursive Tree searches may publish results while their recursive
 * corpus is still being scanned.
 *
 * Rebuilding the bounded contextual Tree after every 256-entry scanner batch
 * would create needless UI churn. Publish after this much additional corpus
 * growth instead, while still publishing the first useful result immediately.
 */
const EXACT_TREE_PROGRESS_ENTRY_INTERVAL: usize = 16_384;

#[derive(Debug, Clone)]
struct LocalSessionState {
    directory: PathBuf,

    home_directory: PathBuf,

    selected_path: Option<PathBuf>,

    list_offset: usize,

    query: String,

    view_mode: ViewMode,

    search_mode: SearchMode,

    recursive_mode: bool,

    /*
     * Hidden visibility belongs to the local browsing session.
     *
     * A newly connected SSH source always starts with both Hidden and
     * Hidden Only disabled. Disconnect restores the exact local visibility
     * state that existed before entering SSH.
     */
    show_hidden: bool,

    hidden_only: bool,
}

/*
 * Complete local recursive corpus retained when an Exact Recursive Tree is
 * rerooted from a child directory into its immediate parent.
 *
 * excluded_subtree is the old root. Its descendants are already represented
 * by entries and therefore must not be visited again by the new parent scan.
 */
#[derive(Debug)]
struct RecursiveScanSeed {
    excluded_subtree: PathBuf,

    entries: Vec<FileEntry>,
}

#[derive(Debug, Clone)]
struct NavigationState {
    selected_path: Option<PathBuf>,
    list_offset: usize,
}

#[derive(Debug, Clone)]
struct SearchModeSelectionState {
    selected_path: PathBuf,

    viewport_row: usize,
}

#[derive(Debug, Clone)]
struct BackHistoryEntry {
    directory: PathBuf,

    view_mode: ViewMode,
}

#[derive(Debug, Clone)]
struct SearchReturnState {
    root_directory: PathBuf,

    landed_directory: PathBuf,

    query: String,

    search_mode: SearchMode,

    selected_path: Option<PathBuf>,

    list_offset: usize,

    view_mode: ViewMode,

    recursive_mode: bool,
}

/*
 * Identity of one progressive local Exact Recursive Tree search.
 *
 * scan_generation identifies the corpus/root/visibility policy. The query and
 * entry filter may change while that corpus is still being built, so they must
 * participate independently.
 */
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProgressiveExactTreeIdentity {
    query: String,

    scan_generation: u64,

    entry_filter: EntryFilter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FuzzyRequestIdentity {
    query: String,

    scope_directory: PathBuf,

    recursive_mode: bool,

    show_hidden: bool,

    hidden_only: bool,

    /*
     * A recursive SearchIndex remains Arc-backed and resident.
     *
     * Its address distinguishes the currently installed corpus from an older
     * index that happened to contain the same number of records.
     */
    recursive_index_identity: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    List,
    Tree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    Exact,

    Fuzzy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryFilter {
    All,

    FilesOnly,

    DirectoriesOnly,
}

impl EntryFilter {
    fn matches(self, entry: &FileEntry) -> bool {
        match self {
            Self::All => true,

            /*
             * Symlinks remain file-like results unless they were classified as
             * directories by the source itself.
             */
            Self::FilesOnly => !entry.is_directory,

            Self::DirectoriesOnly => entry.is_directory,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecursiveTreeIdentity {
    root_directory: PathBuf,

    show_hidden: bool,

    hidden_only: bool,

    entry_filter: EntryFilter,

    sort_mode: SortMode,

    sort_descending: bool,

    scan_generation: u64,

    recursive_entry_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TreeExpandAllTarget {
    /*
     * Recursive Tree with an active Exact or Fuzzy query.
     *
     * Search-result branches are open by default, so Expand All clears the set
     * of explicitly collapsed directories.
     */
    RecursiveSearch,

    /*
     * Queryless recursive Tree.
     *
     * Branches are closed by default, so Expand All records every represented
     * expandable directory.
     */
    RecursiveQueryless,

    /*
     * Ordinary Tree.
     *
     * The complete hierarchy may already be resident, or expansion may first
     * require an asynchronous recursive scan.
     */
    Ordinary,
}

#[derive(Debug, Clone)]
struct PendingTreeExpandAll {
    target: TreeExpandAllTarget,

    selected_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
enum RefusedTreeExpandState {
    Ordinary {
        root_directory: PathBuf,
        show_hidden: bool,
        hidden_only: bool,
        entry_filter: EntryFilter,
        expanded_directories: HashSet<PathBuf>,
    },

    RecursiveQueryless {
        root_directory: PathBuf,
        expanded_directories: HashSet<PathBuf>,
    },

    RecursiveSearch {
        root_directory: PathBuf,
        collapsed_directories: HashSet<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeExpandAllDialogKind {
    /*
     * Confirmation for a local Tree between the configured warning and maximum
     * thresholds.
     */
    LocalConfirmation,

    /*
     * Confirmation for an SSH Tree between the configured warning and maximum
     * thresholds.
     */
    SshConfirmation,

    /*
     * Informational refusal when complete Alt+E expansion would exceed the
     * configured visible-Tree ceiling.
     */
    Refusal,

    /*
     * Persistent explanation shown whenever one manual branch expansion would
     * exceed the configured visible-Tree ceiling.
     */
    DisplayLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeDisplayLimitAction {
    BranchExpansion,

    ShowHidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeExpandAllDialogFocus {
    ExpandAll,

    Cancel,
}

#[derive(Debug, Clone)]
pub struct TreeExpandAllDialogState {
    pub kind: TreeExpandAllDialogKind,

    pub projected_rows: usize,

    pub configured_max_rows: usize,

    pub disable_warning: bool,

    pub display_limit_action: TreeDisplayLimitAction,

    pub focus: TreeExpandAllDialogFocus,

    target: TreeExpandAllTarget,

    selected_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteIndexDialogPurpose {
    InitialSetup,

    Rebuild,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteIndexDialogFocus {
    Policy,

    Ok,

    Cancel,
}

impl RemoteIndexDialogFocus {
    pub fn next(self) -> Self {
        match self {
            Self::Policy => Self::Ok,

            Self::Ok => Self::Cancel,

            Self::Cancel => Self::Policy,
        }
    }

    pub fn previous(self) -> Self {
        match self {
            Self::Policy => Self::Cancel,

            Self::Ok => Self::Policy,

            Self::Cancel => Self::Ok,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RemoteIndexSetupState {
    pub identity: crate::remote_index::RemoteIndexIdentity,

    #[allow(dead_code)]
    pub purpose: RemoteIndexDialogPurpose,

    pub includes_hidden: bool,

    pub focus: RemoteIndexDialogFocus,

    /*
     * Present when an existing cache was found but failed validation.
     */
    pub invalid_reason: Option<String>,
}

/*
 * Complete remote corpus prepared outside the terminal event thread.
 *
 * Loading millions of entries is only the first part of making an index usable.
 * The path lookup, child lookup, and SearchIndex are also expensive to construct,
 * so the loader worker prepares them before reporting completion.
 */
#[derive(Debug)]
struct PreparedRemoteIndex {
    loaded: LoadedRemoteIndex,

    child_indices: HashMap<PathBuf, Vec<usize>>,

    search_index: SearchIndex,
}

#[derive(Debug)]
struct RemoteIndexLoadResult {
    result: Result<PreparedRemoteIndex, String>,
}

/*
 * Why the persistent remote index is currently being loaded.
 *
 * Background loading attaches the host-wide corpus to an SSH connection without
 * changing the user's List/Tree or Exact/Recursive mode.
 *
 * EnableRecursive means Alt+R, startup configuration, or session restoration is
 * waiting for the load and Recursive mode must be enabled after installation.
 */
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteIndexLoadPurpose {
    Background,

    EnableRecursive,
}

/*
 * Read, decode, and fully prepare one persistent remote index.
 *
 * This function runs on the loader thread. The terminal event thread receives a
 * corpus whose expensive derived structures are already complete.
 */
fn prepare_remote_index(identity: &RemoteIndexIdentity) -> Result<PreparedRemoteIndex, String> {
    let loaded = load_remote_index(identity).map_err(|error| error.to_string())?;

    let mut child_indices: HashMap<PathBuf, Vec<usize>> = HashMap::new();

    for (index, entry) in loaded.entries.iter().enumerate() {
        let Some(parent) = entry.path.parent() else {
            continue;
        };

        child_indices
            .entry(parent.to_path_buf())
            .or_default()
            .push(index);
    }

    let search_index = SearchIndex::from_entries(&loaded.entries);

    Ok(PreparedRemoteIndex {
        loaded,

        child_indices,

        search_index,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    None,

    Help,

    Legend,

    About,

    Connection,

    RemoteIndexSetup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionChoice {
    Delete,

    Cancel,
}

#[derive(Debug, Clone)]
pub struct DeletionState {
    pub path: PathBuf,

    pub name: String,

    pub is_directory: bool,

    pub is_symlink: bool,

    pub directory_has_content: bool,

    pub choice: DeletionChoice,
}

/*
 * One filesystem entry renamed out of its original location for the lifetime
 * of the current Scry session.
 *
 * Stage A records enough information for later restoration and clean-exit
 * finalization without following symbolic links or rediscovering the original
 * object type.
 */
#[derive(Debug, Clone)]
struct StagedDeletion {
    original_path: PathBuf,

    staged_path: PathBuf,

    is_directory: bool,

    is_symlink: bool,
}

impl StagedDeletion {
    fn journal_entry(&self) -> DeletionJournalEntry {
        DeletionJournalEntry {
            original_path: self.original_path.clone(),

            staged_path: self.staged_path.clone(),

            is_directory: self.is_directory,

            is_symlink: self.is_symlink,
        }
    }

    fn from_journal_entry(entry: &DeletionJournalEntry) -> Self {
        Self {
            original_path: entry.original_path.clone(),

            staged_path: entry.staged_path.clone(),

            is_directory: entry.is_directory,

            is_symlink: entry.is_symlink,
        }
    }
}

fn staged_deletion_journal_entries(
    staged_deletions: &[StagedDeletion],
) -> Vec<DeletionJournalEntry> {
    staged_deletions
        .iter()
        .map(StagedDeletion::journal_entry)
        .collect()
}

/*
 * Validate the structural relationship recorded for one staged deletion.
 *
 * Every staged path is created as a hidden sibling of its original path. Exact
 * paths come from the journal; recovery never reconstructs names by parsing the
 * generated filename.
 */
fn validate_journal_entry_paths(entry: &DeletionJournalEntry) -> Result<(), String> {
    if entry.original_path == entry.staged_path {
        return Err(format!(
            "original and staged paths are identical: {}",
            entry.original_path.display(),
        ));
    }

    let original_parent = entry.original_path.parent().ok_or_else(|| {
        format!(
            "original path has no parent: {}",
            entry.original_path.display(),
        )
    })?;

    let staged_parent = entry
        .staged_path
        .parent()
        .ok_or_else(|| format!("staged path has no parent: {}", entry.staged_path.display(),))?;

    if original_parent != staged_parent {
        return Err(format!(
            "staged path {} is not beside original path {}",
            entry.staged_path.display(),
            entry.original_path.display(),
        ));
    }

    let staged_name = entry
        .staged_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            format!(
                "staged path has no valid filename: {}",
                entry.staged_path.display(),
            )
        })?;

    if !staged_name.starts_with(STAGED_DELETION_PREFIX) {
        return Err(format!(
            "staged path does not use Scry's deletion prefix: {}",
            entry.staged_path.display(),
        ));
    }

    if entry.is_directory && entry.is_symlink {
        return Err(format!(
            "journal entry cannot be both a directory and a symbolic link: {}",
            entry.original_path.display(),
        ));
    }

    Ok(())
}

/*
 * Find a later staged real directory that originally contained this entry's
 * hidden sibling.
 *
 * When that parent directory was staged, the child's hidden path moved inside
 * the parent's staged pathname. The recorded child path becomes reachable again
 * after the parent is restored first.
 */
fn containing_later_staged_directory(
    entries: &[DeletionJournalEntry],
    entry_index: usize,
) -> Option<usize> {
    let entry = entries.get(entry_index)?;

    entries
        .iter()
        .enumerate()
        .skip(entry_index.saturating_add(1))
        .find_map(|(container_index, container)| {
            (container.is_directory
                && !container.is_symlink
                && entry.staged_path.starts_with(&container.original_path))
            .then_some(container_index)
        })
}

/*
 * Validate the live filesystem object at one currently reachable staged path.
 */
fn validate_reachable_staged_object(entry: &DeletionJournalEntry) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(&entry.staged_path).map_err(|error| {
        format!(
            "unable to inspect staged path {}: {}",
            entry.staged_path.display(),
            error,
        )
    })?;

    let file_type = metadata.file_type();

    let staged_is_symlink = file_type.is_symlink();

    let staged_is_directory = file_type.is_dir() && !staged_is_symlink;

    if staged_is_symlink != entry.is_symlink || staged_is_directory != entry.is_directory {
        return Err(format!(
            "staged object type no longer matches the journal for {}",
            entry.original_path.display(),
        ));
    }

    Ok(())
}

#[derive(Debug, Clone)]
pub struct TreeRow {
    pub entry: FileEntry,

    /*
     * One value for every ancestor level.
     *
     * true means that ancestor has later siblings, so a vertical │ line
     * should continue through this row.
     */
    pub ancestor_has_more: Vec<bool>,

    pub is_last: bool,

    pub expanded: bool,
}

#[derive(Debug)]
struct ConnectionWorkerSuccess {
    source: Box<dyn FileSource>,

    target: SshTarget,

    directory: PathBuf,

    home_directory: PathBuf,

    entries: Vec<FileEntry>,
}
#[derive(Debug)]
struct ConnectionWorkerResult {
    result: Result<ConnectionWorkerSuccess, String>,
}

#[derive(Debug)]
struct TransferWorkerResult {
    source: Box<dyn FileSource>,

    result: io::Result<PathBuf>,
}

#[derive(Debug)]
struct BatchTransferFailure {
    remote_path: PathBuf,

    message: String,
}

#[derive(Debug)]
struct BatchTransferWorkerResult {
    source: Box<dyn FileSource>,

    completed_paths: Vec<PathBuf>,

    failures: Vec<BatchTransferFailure>,

    cancelled: bool,
}

#[derive(Debug)]
enum TransferWorkerMessage {
    Progress(TransferProgress),

    BatchProgress {
        item_index: usize,

        item_count: usize,

        filename: String,

        item_transferred_bytes: u64,

        item_total_bytes: u64,

        completed_bytes: u64,
    },

    Finished(TransferWorkerResult),

    BatchFinished(BatchTransferWorkerResult),
}

#[derive(Debug)]
pub struct TransferState {
    pub filename: String,

    pub total_bytes: u64,

    pub transferred_bytes: u64,

    pub started_at: Instant,

    pub finished_elapsed: Option<Duration>,

    pub error: Option<String>,

    pub cancel_requested: bool,

    remote_path: PathBuf,

    local_path: Option<PathBuf>,

    /*
     * Batch-transfer information.
     *
     * Single-file Enter leaves destination_root as None and item_count as one.
     */
    pub destination_root: Option<PathBuf>,

    pub item_index: usize,

    pub item_count: usize,

    pub item_transferred_bytes: u64,

    pub item_total_bytes: u64,

    pub completed_count: usize,

    pub failed_count: usize,

    pub failures: Vec<String>,

    pub is_batch: bool,

    receiver: Receiver<TransferWorkerMessage>,

    cancel_signal: Arc<AtomicBool>,
}

/*
 * The real source temporarily lives inside the transfer worker.
 *struct AppClipboard(ClipboardContext);

impl fmt::Debug for AppClipboard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AppClipboard")
    }
}
 * This placeholder keeps App structurally valid while the worker owns the
 * SSH/SFTP source. The transfer popup is modal, so filesystem operations are
 * not permitted while this placeholder is installed.
 */
struct TransferPlaceholderSource {
    label: String,
}

impl fmt::Debug for TransferPlaceholderSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TransferPlaceholderSource")
            .field("label", &self.label)
            .finish()
    }
}

impl TransferPlaceholderSource {
    fn new(label: String) -> Self {
        Self { label }
    }

    fn unavailable() -> io::Error {
        io::Error::other("filesystem source is busy transferring a remote file")
    }
}

impl FileSource for TransferPlaceholderSource {
    fn read_directory(
        &mut self,
        _directory: &Path,
        _sort_mode: SortMode,
        _sort_descending: bool,
    ) -> io::Result<Vec<FileEntry>> {
        Err(Self::unavailable())
    }

    fn directory_has_content(&mut self, _directory: &Path) -> io::Result<bool> {
        Err(Self::unavailable())
    }

    fn path_is_directory(&mut self, _path: &Path) -> io::Result<bool> {
        Err(Self::unavailable())
    }

    fn supports_recursive_scan(&self) -> bool {
        false
    }

    fn source_label(&self) -> String {
        self.label.clone()
    }

    fn materialize_file(
        &mut self,
        _path: &Path,
        _progress: &mut dyn FnMut(TransferProgress) -> io::Result<TransferControl>,
    ) -> io::Result<PathBuf> {
        Err(Self::unavailable())
    }

    fn download_file_to(
        &mut self,
        _source_path: &Path,
        _destination_path: &Path,
        _progress: &mut dyn FnMut(TransferProgress) -> io::Result<TransferControl>,
    ) -> io::Result<PathBuf> {
        Err(Self::unavailable())
    }

    fn is_remote(&self) -> bool {
        true
    }
}

#[cfg(target_os = "linux")]
struct AppClipboard(ClipboardContext);

#[cfg(target_os = "linux")]
impl fmt::Debug for AppClipboard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AppClipboard")
    }
}

/*
 * Persistent information retained for every marked file.
 *
 * Directory listings are replaced as the user navigates, so retaining only
 * the path would lose the original filename and byte size needed to construct
 * a truthful multi-file transfer queue later.
 */
#[derive(Debug, Clone)]
struct MarkedFile {
    path: PathBuf,

    filename: String,

    size_bytes: u64,
}

/*
 * One immutable file operation inside a marked SSH batch.
 *
 * The destination path is calculated before the worker starts. Depending on
 * configuration, it either preserves the remote hierarchy or places the file
 * directly beneath the batch root with safe collision disambiguation.
 */
#[derive(Debug, Clone)]
struct BatchTransferItem {
    remote_path: PathBuf,

    destination_path: PathBuf,

    filename: String,

    expected_size: u64,
}

#[derive(Debug)]
pub struct App {
    source: Box<dyn FileSource>,

    /*
     * Local process directory captured once when Scry starts.
     *
     * Direct SSH sessions have no saved local browser session. Batch downloads
     * from those sessions return here rather than querying the process working
     * directory again at transfer time.
     */
    launch_directory: PathBuf,

    /*
     * Complete identity of the active SSH source.
     *
     * This remains available even while the real source temporarily lives in a
     * transfer worker and App contains TransferPlaceholderSource.
     */
    active_ssh_target: Option<SshTarget>,

    pub current_directory: PathBuf,

    pub home_directory: PathBuf,

    pub entries: Vec<FileEntry>,

    /*
     * Files explicitly marked for a future batch operation.
     *
     * Full paths allow marks to survive filtering, directory navigation,
     * and switching between List and Tree modes.
     *
     * Directories are not markable during the first implementation stage.
     */
    marked_files: HashMap<PathBuf, MarkedFile>,

    pub recursive_entries: Vec<FileEntry>,

    /*
     * Root directory covered by the resident recursive corpus.
     *
     * For local browsing this may be broader than current_directory.
     * Moving inside an already covered subtree therefore changes only the
     * visible/search scope instead of discarding and rebuilding the corpus.
     *
     * Remote persistent indexes already behave this way at host scope.
     */
    recursive_corpus_root: Option<PathBuf>,

    /*
     * Direct-child lookup for the resident recursive corpus.
     *
     * Each value contains indices into recursive_entries for entries whose
     * immediate parent is the corresponding directory.
     *
     * Recursive Tree branch expansion can therefore inspect only the selected
     * directory's children instead of scanning the complete corpus.
     */
    recursive_child_indices: HashMap<PathBuf, Vec<usize>>,

    search_index: Arc<SearchIndex>,

    pub filtered_indices: Vec<usize>,

    pub query: String,

    /*
     * UTF-8 byte position of the insertion caret inside `query`.
     *
     * The value is always kept on a valid character boundary.
     */
    pub query_cursor: usize,

    pub search_mode: SearchMode,

    pub fuzzy_result_limit: usize,

    pub exact_tree_match_limit: usize,

    pub entry_filter: EntryFilter,

    pub allow_file_opening: bool,

    /*
     * Close Scry only after an external opener has been launched successfully.
     *
     * Failed opens and directory navigation leave the application running.
     */
    pub exit_on_open: bool,

    pub theme: Theme,

    /*
     * Ordinary hidden-entry inclusion controlled by Alt+H.
     *
     * This value remains intact while Hidden Only is active so F6 can return to
     * the exact ordinary visibility state that preceded it.
     */
    pub show_hidden: bool,

    /*
     * Restrict the active view to paths that contain at least one hidden component.
     *
     * Non-recursive browsing therefore shows direct dot-entries. Recursive
     * browsing also includes every descendant beneath a hidden directory.
     */
    pub hidden_only: bool,

    pub show_icons: bool,

    /*
     * Apply the bright FileClass palette to ordinary filenames.
     *
     * Structural directory and symlink colors remain unchanged.
     */
    pub show_file_colors: bool,

    pub show_permissions: bool,

    pub show_date: bool,

    pub show_size: bool,

    pub show_user: bool,

    pub show_details: bool,

    pub show_selection: bool,

    pub show_columns: bool,

    pub sort_mode: SortMode,

    pub sort_descending: bool,

    pub selected: usize,

    /*
     * True only while the entries scrollbar is actively being dragged.
     *
     * The renderer may use this to hide the ordinary selection highlight while
     * the viewport itself is moving.
     */
    pub scrollbar_drag_active: bool,

    /*
     * True while repeated keyboard or mouse-wheel navigation is arriving.
     *
     * During this brief state, redraw-time layout measurement examines only the
     * visible viewport instead of traversing the complete result set for every
     * movement event.
     */
    pub rapid_navigation_active: bool,

    rapid_navigation_expires_at: Option<Instant>,

    #[cfg(target_os = "linux")]
    clipboard: Option<AppClipboard>,

    last_copied_path: Option<String>,

    pub file_info: Option<FileInfoState>,

    file_info_generation: u64,

    file_info_receiver: Option<Receiver<FileInfoMessage>>,

    pub transfer: Option<TransferState>,

    pub enable_deletion: bool,

    pub deletion: Option<DeletionState>,

    /*
     * Entries renamed into hidden sibling paths during this process.
     *
     * Restoration and permanent clean-exit removal are added in later stages.
     */
    staged_deletions: Vec<StagedDeletion>,

    pub list_offset: usize,

    pub viewport_rows: usize,

    /*
     * Shared horizontal viewport for Metadata and filesystem entry rows.
     *
     * Stage one only establishes and clamps this state. Row clipping and mouse
     * interaction are added separately after the scrollbar placement is verified.
     */
    pub horizontal_offset: usize,

    pub horizontal_max_offset: usize,

    pending_selection_path: Option<PathBuf>,

    /*
     * Screen row to preserve while an asynchronous Recursive List worker rebuilds
     * the result set.
     *
     * The selected path may move to a different numeric result position between
     * progressive snapshots, so the viewport must be reconstructed after every
     * worker update until the final result arrives.
     */
    pending_list_viewport_row: Option<usize>,

    /*
     * Screen row to preserve while an asynchronous Recursive Tree worker rebuilds
     * the destination view during a List/Tree transition.
     */
    pending_tree_viewport_row: Option<usize>,

    /*
     * Viewport offset waiting for an asynchronous recursive scan or remote-index
     * load to make the restored selection visible.
     */
    pending_session_list_offset: Option<usize>,

    /*
     * Screen row occupied by the restored session selection.
     *
     * Unlike an absolute list offset, this remains valid when rebuilding a Tree
     * changes the selected path's numeric row position.
     */
    pending_session_viewport_row: Option<usize>,

    /*
     * Screen row to preserve while a visibility-changing rebuild replaces the
     * current result set.
     *
     * This is used by Alt+H in both List and Tree mode. The selected path may move
     * to a different absolute index while hidden entries are added or removed, but
     * its visual position inside the viewport should remain stable.
     */
    pending_visibility_viewport_row: Option<usize>,

    pub error_message: Option<String>,

    error_message_expires_at: Option<Instant>,

    /*
     * Non-error operational information shown in amber.
     */
    pub status_message: Option<String>,

    status_message_expires_at: Option<Instant>,

    /*
     * Dedicated animation state for the persistent remote-index loading message.
     *
     * Ordinary informational messages leave both values as None and therefore
     * retain their normal steady status color.
     */
    status_message_pulse_started_at: Option<Instant>,

    status_message_pulse_next_frame_at: Option<Instant>,

    pub should_quit: bool,

    pub scan_in_progress: bool,

    pub recursive_scan_partial: bool,

    /*
     * Controls whether local recursive scanning uses the bounded Fast corpus or
     * traverses the complete eligible directory tree.
     *
     * Keeping this choice in App removes the scan policy from
     * ensure_recursive_scan() and gives future configuration and UI controls one
     * authoritative value to change.
     */
    pub recursive_scan_mode: RecursiveScanMode,

    pub recursive_mode: bool,

    pub view_mode: ViewMode,

    pub overlay: Overlay,

    pub remote_index_setup: Option<RemoteIndexSetupState>,

    /*
     * Set after the setup window is confirmed.
     *
     * The next worker-integration stage consumes this value and starts the
     * independent full-filesystem index build from "/".
     */
    pending_remote_index_hidden_policy: Option<bool>,

    remote_index_build_receiver: Option<Receiver<RemoteIndexBuildMessage>>,

    pub remote_index_build_in_progress: bool,

    pub remote_index_entries_written: u64,

    remote_index_load_receiver: Option<Receiver<RemoteIndexLoadResult>>,

    pub remote_index_load_in_progress: bool,

    /*
     * Records whether the current loader merely attaches the index in the
     * background or must enable Recursive mode after installation.
     */
    remote_index_load_purpose: Option<RemoteIndexLoadPurpose>,

    remote_index_loaded: bool,

    remote_index_includes_hidden: bool,

    pub connection_store: ConnectionStore,

    pub connection_dialog: ConnectionDialogState,

    pub connection_in_progress: bool,

    pub ssh_config: SshConfig,

    /*
     * User-configurable Tree-size policy.
     *
     * The warning threshold governs bulk Alt+E expansion, while
     * max_visible_tree_rows is the absolute ceiling for simultaneously visible
     * Tree rows. Neither setting limits the recursive corpus or searching.
     */
    pub advanced_tree_config: AdvancedTreeConfig,

    /*
     * Modal decision awaiting confirmation or acknowledgement before an extreme
     * Expand All operation may continue.
     */
    pub tree_expand_all_dialog: Option<TreeExpandAllDialogState>,

    /*
     * Session-local suppression selected through:
     *
     *     [o] Disable this warning message
     *
     * This suppresses only local confirmation-range dialogs. It never bypasses the
     * configured maximum and never suppresses SSH warnings.
     *
     * Persistence will be connected in a later update.
     */
    local_expand_all_warning_disabled: bool,

    /*
     * Local confirmation is shown at most once during one Scry process.
     *
     * This becomes true as soon as the dialog is presented, whether the user
     * approves or cancels.
     */
    local_expand_all_warning_shown_this_session: bool,

    ssh_expand_all_warning_shown_this_session: bool,

    /*
     * The configured Expand All maximum applies equally to local and SSH Trees.
     *
     * The first over-limit attempt explains the policy. Later attempts during the
     * same Scry process continue obeying the maximum without repeating the dialog.
     */
    tree_expand_all_refusal_shown_this_session: bool,

    connection_receiver: Option<Receiver<ConnectionWorkerResult>>,

    saved_local_session: Option<LocalSessionState>,

    pub help_scroll: u16,

    pub help_max_scroll: u16,

    pub help_tips_scroll: u16,

    pub help_tips_hovered: bool,

    pub help_top_hovered: bool,

    pub legend_scroll: u16,

    pub legend_max_scroll: u16,

    pub tree_rows: Vec<TreeRow>,

    pub filtered_tree_indices: Vec<usize>,

    tree_search_saved_selection: Option<PathBuf>,

    tree_search_saved_offset: usize,

    owner_name_cache: HashMap<u32, String>,

    search_collapsed_directories: HashSet<PathBuf>,

    recursive_expanded_directories: HashSet<PathBuf>,

    search_tree_children: HashMap<PathBuf, Vec<FileEntry>>,

    /*
     * Identity of the queryless recursive Tree currently retained in
     * search_tree_children and tree_rows.
     *
     * Ctrl+T may reuse that Tree only while every structural input still matches.
     */
    recursive_tree_identity: Option<RecursiveTreeIdentity>,

    tree_children: HashMap<PathBuf, Vec<FileEntry>>,

    directory_has_content_cache: HashMap<PathBuf, bool>,

    classification_inspection_cache: HashMap<PathBuf, FileClass>,

    expanded_directories: HashSet<PathBuf>,

    /*
     * True only when ordinary queryless Tree mode was explicitly placed into
     * complete Expand All state through Alt+E.
     *
     * This must never be inferred from the currently materialized lazy Tree.
     * Manual branch collapse invalidates the bulk state.
     */
    ordinary_expand_all_active: bool,

    recursive_cache_complete: bool,

    /*
     * Expand All request waiting for the complete recursive corpus.
     *
     * Ordinary lazy Tree mode cannot calculate or display its complete hierarchy
     * until the asynchronous recursive scan has finished.
     */
    pending_tree_expand_all: Option<PendingTreeExpandAll>,

    /*
     * A local Hidden visibility change may replace the recursive corpus
     * asynchronously.
     *
     * When the previous queryless Recursive Tree was explicitly fully expanded,
     * restore that existing non-hidden expansion after the replacement scan
     * completes while leaving newly introduced hidden branches collapsed.
     */
    pending_recursive_visibility_expand_restore: bool,

    /*
     * Hidden ON may require a replacement local recursive corpus before Scry can
     * know the truthful final visible-row count.
     *
     * When set, scan completion must validate the reconstructed Tree against
     * max_visible_tree_rows before the visibility transition is accepted.
     */
    pending_hidden_tree_limit_check: bool,

    /*
     * Expansion state retained when complete Alt+E expansion exceeds the configured
     * maximum.
     *
     * This lets Alt+E remain a real bulk toggle:
     *
     *     safe partial expansion -> Collapse All -> safe partial expansion
     *
     * without repeatedly attempting the forbidden full expansion.
     */
    refused_tree_expand_state: Option<RefusedTreeExpandState>,

    /*
     * Compact session bulk states are applied only after their hierarchy maps
     * have been reconstructed from the recursive corpus or search results.
     */
    pending_session_recursive_expand_all: bool,

    pending_session_search_collapse_all: bool,

    scan_generation: u64,

    scan_receiver: Option<Receiver<ScanMessage>>,

    /*
     * Optional complete child corpus waiting to seed the next local recursive scan.
     *
     * This is populated only for the tightly constrained ancestor-reroot optimization.
     */
    pending_recursive_scan_seed: Option<RecursiveScanSeed>,

    fuzzy_generation: u64,

    fuzzy_receiver: Option<Receiver<FuzzyWorkerResult>>,

    fuzzy_cancel_signal: Option<Arc<AtomicBool>>,

    active_fuzzy_request: Option<FuzzyRequestIdentity>,

    /*
     * Deadline for the newest recursive query edit.
     *
     * None means no debounced search is waiting to launch.
     */
    pending_recursive_search_at: Option<Instant>,

    pub fuzzy_filter_in_progress: bool,

    pub fuzzy_examined: usize,

    pub fuzzy_total: usize,

    /*
     * Last genuine selection used in Fuzzy mode.
     *
     * Exact queries may produce no results. In that state there is no current
     * selection to carry back into Fuzzy, so retain the most recent Fuzzy path
     * and visual viewport row independently.
     */
    fuzzy_selection_state: Option<SearchModeSelectionState>,

    /*
     * True when the completed Exact Recursive Tree result exceeded the configured
     * direct-match cap.
     */
    pub exact_tree_limit_reached: bool,

    /*
     * Progressive Exact Tree state used only while a local recursive corpus is
     * actively being constructed.
     *
     * Matches contain direct result indices only. Contextual ancestors are added
     * by the existing bounded Tree builder when a snapshot is published.
     */
    progressive_exact_tree_identity: Option<ProgressiveExactTreeIdentity>,

    progressive_exact_tree_matches: Vec<usize>,

    progressive_exact_tree_last_published_entry_count: usize,

    progressive_exact_tree_last_published_match_count: usize,

    navigation_states: HashMap<PathBuf, NavigationState>,

    back_history: Vec<BackHistoryEntry>,

    /*
     * List and Tree display different row collections.
     *
     * Retain their viewport state independently. The selected path itself is shared:
     * Ctrl+T changes only the representation and carries the current selection into
     * the destination view.
     */
    list_view_state: Option<NavigationState>,

    tree_view_state: Option<NavigationState>,

    /*
     * Screen row occupied by the List selection when List mode was last left.
     *
     * This preserves the selector's visual position even when refreshing the
     * Recursive List changes the selected path's absolute result index.
     */
    list_selection_viewport_row: usize,

    /*
     * Screen row occupied by the Tree selection when Tree mode was last left.
     *
     * Unlike an absolute list_offset, this remains meaningful when an asynchronous
     * Recursive Tree rebuild changes the selected path's numeric row position.
     */
    tree_selection_viewport_row: usize,

    search_return_state: Option<SearchReturnState>,

    pub search_navigation_active: bool,
}

/*
 * Decide whether an entry is hidden relative to the current search root.
 *
 * Recursive results must hide the complete subtree beneath a dot-directory,
 * not merely entries whose own filename starts with a dot.
 *
 * Example beneath /home/ferusx:
 *
 *     .cache                         hidden
 *     .cache/chromium                hidden
 *     .cache/chromium/Default        hidden
 */
fn path_contains_hidden_component(path: &Path) -> bool {
    path.components().any(|component| {
        let component = component.as_os_str().to_string_lossy();

        component != "." && component != ".." && component.starts_with('.')
    })
}

fn entry_is_hidden_below(entry: &FileEntry, root: &Path) -> bool {
    /*
     * The browsing root may itself already lie inside hidden content.
     *
     * Example:
     *
     *     root:  /home/ferusx/.config
     *     entry: /home/ferusx/.config/scry/scry.toml
     *
     * Stripping the root would hide the .config component and incorrectly
     * classify scry.toml as ordinary content.
     */
    if path_contains_hidden_component(root) {
        return true;
    }

    let relative_path = entry.path.strip_prefix(root).unwrap_or(&entry.path);

    path_contains_hidden_component(relative_path)
}

/*
 * Apply Scry's complete hidden-entry visibility policy.
 *
 * Hidden Only is intentionally independent from Recursive mode:
 *
 * - in an ordinary directory, only direct dot-entries match;
 * - in a recursive corpus, every descendant beneath a hidden component matches.
 *
 * entry_is_hidden_below() already implements both cases because it examines the
 * entry path relative to the current root.
 */
fn entry_matches_visibility(
    entry: &FileEntry,
    root: &Path,
    show_hidden: bool,
    hidden_only: bool,
) -> bool {
    let hidden = entry_is_hidden_below(entry, root);

    if hidden_only {
        hidden
    } else {
        show_hidden || !hidden
    }
}

/*
 * Generate an unused hidden sibling path for one staged deletion.
 *
 * Example:
 *
 *     report.txt
 *
 * becomes something resembling:
 *
 *     .scry-deleted-1785061485123456789-4281-0-report.txt
 *
 * The candidate remains in the same parent directory as the original object,
 * allowing std::fs::rename() to remain a same-filesystem metadata operation.
 */
fn staged_deletion_path(original_path: &Path) -> io::Result<PathBuf> {
    let parent = original_path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "deletion target has no parent directory: {}",
                original_path.display(),
            ),
        )
    })?;

    let original_name = original_path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "deletion target has no filename: {}",
                original_path.display(),
            ),
        )
    })?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    let process_id = std::process::id();

    /*
     * A pre-existing candidate causes another counter value to be tried.
     *
     * symlink_metadata() is essential here: a dangling symlink still occupies
     * its pathname and must therefore count as a collision.
     */
    loop {
        let counter = STAGED_DELETION_COUNTER.fetch_add(1, Ordering::Relaxed);

        let staged_name = format!(
            "{}{}-{}-{}-{}",
            STAGED_DELETION_PREFIX,
            timestamp,
            process_id,
            counter,
            original_name.to_string_lossy(),
        );

        let candidate = parent.join(staged_name);

        match std::fs::symlink_metadata(&candidate) {
            Ok(_) => {
                continue;
            }

            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(candidate);
            }

            Err(error) => {
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "unable to validate staged deletion path {}: {}",
                        candidate.display(),
                        error,
                    ),
                ));
            }
        }
    }
}

/*
 * A staged directory hides its complete renamed subtree, not merely the
 * directory row itself.
 *
 * For a staged regular file or symbolic link, starts_with() is equivalent to
 * exact equality because no descendant paths can exist beneath it.
 */
fn path_belongs_to_staged_deletion(path: &Path, staged_deletions: &[StagedDeletion]) -> bool {
    staged_deletions
        .iter()
        .any(|deletion| path.starts_with(&deletion.staged_path))
}

fn rebase_recursive_entry(entry: &mut FileEntry, root: &Path) {
    entry.relative_path = entry
        .path
        .strip_prefix(root)
        .unwrap_or(&entry.path)
        .to_path_buf();

    entry.searchable_path = Arc::from(entry.relative_path.to_string_lossy().to_lowercase());
}

impl App {
    pub fn new(start_path: PathBuf) -> io::Result<Self> {
        let current_directory = normalize_start_path(start_path)?;

        let home_directory = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| current_directory.clone());

        Self::with_source_and_home(
            current_directory,
            home_directory,
            Box::new(LocalSource::new()),
        )
    }

    pub fn with_source_and_home(
        current_directory: PathBuf,
        home_directory: PathBuf,
        mut source: Box<dyn FileSource>,
    ) -> io::Result<Self> {
        /*
         * Capture the process launch directory once.
         *
         * Re-reading current_dir() during a later batch transfer made the
         * destination depend on mutable process state and could unexpectedly
         * resolve to filesystem root.
         */
        let launch_directory = std::env::current_dir().map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("unable to determine Scry's launch directory: {}", error),
            )
        })?;

        let sort_mode = SortMode::Name;

        let sort_descending = false;

        let connection_store = ConnectionStore::load()?;

        let connection_dialog = ConnectionDialogState::new(&connection_store);

        let entries = source.read_directory(&current_directory, sort_mode, sort_descending)?;

        let mut app = Self {
            source,

            launch_directory,

            active_ssh_target: None,

            current_directory,

            home_directory,

            entries,

            marked_files: HashMap::new(),

            recursive_entries: Vec::new(),

            recursive_corpus_root: None,

            recursive_child_indices: HashMap::new(),

            search_index: Arc::new(SearchIndex::new()),

            filtered_indices: Vec::new(),

            query: String::new(),

            query_cursor: 0,

            search_mode: SearchMode::Exact,

            fuzzy_result_limit: crate::config::DEFAULT_FUZZY_RESULT_LIMIT,

            exact_tree_match_limit: crate::config::DEFAULT_EXACT_TREE_MATCH_LIMIT,

            entry_filter: EntryFilter::All,

            allow_file_opening: true,

            exit_on_open: false,

            theme: Theme::default(),

            show_hidden: false,

            hidden_only: false,

            show_icons: false,

            show_file_colors: false,

            show_permissions: false,

            show_date: false,

            show_size: false,

            show_user: false,

            show_details: true,

            show_selection: true,

            show_columns: true,

            sort_mode,

            sort_descending,

            #[cfg(target_os = "linux")]
            clipboard: None,

            last_copied_path: None,

            file_info: None,

            file_info_generation: 0,

            file_info_receiver: None,

            transfer: None,

            enable_deletion: false,

            deletion: None,

            staged_deletions: Vec::new(),

            selected: 0,

            scrollbar_drag_active: false,

            rapid_navigation_active: false,

            rapid_navigation_expires_at: None,

            list_offset: 0,

            viewport_rows: 1,

            horizontal_offset: 0,

            horizontal_max_offset: 0,

            pending_selection_path: None,

            pending_visibility_viewport_row: None,

            pending_list_viewport_row: None,

            pending_session_list_offset: None,

            pending_session_viewport_row: None,

            error_message: None,

            error_message_expires_at: None,

            status_message: None,

            status_message_expires_at: None,

            status_message_pulse_started_at: None,

            status_message_pulse_next_frame_at: None,

            should_quit: false,

            scan_in_progress: false,

            recursive_scan_partial: false,

            recursive_scan_mode: RecursiveScanMode::Total,

            recursive_mode: false,

            view_mode: ViewMode::List,

            overlay: Overlay::None,

            remote_index_setup: None,

            pending_remote_index_hidden_policy: None,

            remote_index_build_receiver: None,

            remote_index_build_in_progress: false,

            remote_index_entries_written: 0,

            remote_index_load_receiver: None,

            remote_index_load_in_progress: false,

            remote_index_load_purpose: None,

            remote_index_loaded: false,

            remote_index_includes_hidden: false,

            connection_store,

            connection_dialog,

            ssh_config: SshConfig::default(),

            advanced_tree_config: AdvancedTreeConfig::default(),

            tree_expand_all_dialog: None,

            local_expand_all_warning_disabled: false,

            local_expand_all_warning_shown_this_session: false,

            ssh_expand_all_warning_shown_this_session: false,

            tree_expand_all_refusal_shown_this_session: false,

            connection_in_progress: false,

            connection_receiver: None,

            saved_local_session: None,

            help_scroll: 0,

            help_max_scroll: 0,

            help_tips_scroll: 0,

            help_tips_hovered: false,

            help_top_hovered: false,

            legend_scroll: 0,

            legend_max_scroll: 0,

            tree_rows: Vec::new(),

            filtered_tree_indices: Vec::new(),

            tree_search_saved_selection: None,

            tree_search_saved_offset: 0,

            owner_name_cache: HashMap::new(),

            search_collapsed_directories: HashSet::new(),

            recursive_expanded_directories: HashSet::new(),

            search_tree_children: HashMap::new(),

            recursive_tree_identity: None,

            tree_children: HashMap::new(),

            directory_has_content_cache: HashMap::new(),

            classification_inspection_cache: HashMap::new(),

            expanded_directories: HashSet::new(),

            ordinary_expand_all_active: false,

            recursive_cache_complete: false,

            pending_tree_expand_all: None,

            pending_recursive_visibility_expand_restore: false,

            pending_hidden_tree_limit_check: false,

            refused_tree_expand_state: None,

            pending_session_recursive_expand_all: false,

            pending_session_search_collapse_all: false,

            pending_tree_viewport_row: None,

            scan_generation: 0,

            scan_receiver: None,

            pending_recursive_scan_seed: None,

            fuzzy_generation: 0,

            fuzzy_receiver: None,

            fuzzy_cancel_signal: None,

            active_fuzzy_request: None,

            pending_recursive_search_at: None,

            fuzzy_filter_in_progress: false,

            fuzzy_examined: 0,

            fuzzy_total: 0,

            fuzzy_selection_state: None,

            exact_tree_limit_reached: false,

            progressive_exact_tree_identity: None,

            progressive_exact_tree_matches: Vec::new(),

            progressive_exact_tree_last_published_entry_count: 0,

            progressive_exact_tree_last_published_match_count: 0,

            navigation_states: HashMap::new(),

            back_history: Vec::new(),

            list_view_state: None,

            tree_view_state: None,

            list_selection_viewport_row: 0,

            tree_selection_viewport_row: 0,

            search_return_state: None,

            search_navigation_active: false,
        };

        app.refresh_filter();

        Ok(app)
    }

    /*
     * Record the SSH identity associated with a source installed outside App.
     *
     * Direct --ssh startup constructs the source in main.rs, whereas connections
     * opened through F4 receive their target through ConnectionWorkerSuccess.
     */
    pub fn set_active_ssh_target(&mut self, target: SshTarget) {
        self.active_ssh_target = Some(target);
    }

    pub fn apply_startup_config(&mut self, config: &ScryConfig) {
        /*
         * Resolve the selected theme once during startup.
         *
         * Missing files, malformed TOML, and invalid individual colors all fall
         * back safely through Theme::load().
         */
        self.theme = Theme::load(&config.theme);

        /*
         * Display panels can be assigned directly because the application has
         * only just been constructed and has not yet entered its event loop.
         */
        self.ssh_config = config.ssh;

        /*
         * Retain the complete validated Expand All policy.
         *
         * ScryConfig::load() has already normalized invalid threshold pairs, so
         * the event loop may use these values without repeating configuration
         * validation.
         */
        self.advanced_tree_config = config.advanced.tree;

        self.enable_deletion = config.features.enable_deletion;

        self.allow_file_opening = config.features.allow_file_opening;

        self.exit_on_open = config.features.exit_on_open;

        self.show_icons = config.display.show_icons;

        self.show_file_colors = config.display.show_file_colors;

        self.show_details = config.display.show_details;

        self.show_selection = config.display.show_selection;

        self.show_columns = config.display.show_columns;

        self.show_permissions = config.display.show_permissions;

        self.show_size = config.display.show_size;

        self.show_date = config.display.show_date;

        self.show_user = config.display.show_user;

        /*
         * Hidden entries require the normal application operation rather than a
         * raw field assignment because toggling hidden files also refreshes the
         * current view and invalidates recursive scan state.
         */
        if config.display.show_hidden && !self.show_hidden {
            self.toggle_hidden();
        }

        /*
         * Apply the configured sort before starting recursive mode or building a
         * Tree view. This ensures that every initial listing begins in the correct
         * order.
         */
        self.sort_mode = match config.browser.sort.as_str() {
            "size" => SortMode::Size,

            "date" => SortMode::Modified,

            "type" => SortMode::Type,

            _ => SortMode::Name,
        };

        self.sort_descending = config.browser.reverse;

        self.apply_sort();

        /*
         * Establish startup search policy before recursive or Tree mode is enabled.
         *
         * These can be assigned directly because startup begins with an empty query
         * and no active background fuzzy worker.
         */
        self.search_mode = if config.browser.fuzzy {
            SearchMode::Fuzzy
        } else {
            SearchMode::Exact
        };

        self.fuzzy_result_limit = config.browser.fuzzy_result_limit;

        self.exact_tree_match_limit = config.browser.exact_tree_match_limit;

        self.entry_filter = match config.browser.entry_filter.as_str() {
            "files" => EntryFilter::FilesOnly,

            "directories" => EntryFilter::DirectoriesOnly,

            _ => EntryFilter::All,
        };

        /*
         * Recursive mode must be established before Tree mode. That allows
         * toggle_tree_mode() to choose the recursive-tree startup route when both
         * settings are enabled.
         */
        if config.browser.recursive {
            self.request_recursive_mode();
        }

        /*
         * Hidden Only must be established before Tree mode so initial Tree
         * construction uses the intended hidden-content corpus.
         *
         * Use the normal toggle operation because it also refreshes filters,
         * recursive state, and any installed remote index correctly.
         */
        if config.browser.hidden_only && !self.hidden_only {
            self.toggle_hidden_only();
        }

        if config.browser.view == "tree" && self.view_mode != ViewMode::Tree {
            self.toggle_tree_mode();
        }

        /*
         * Configuration and restored sessions may contain an older Fuzzy List state
         * with Reverse still enabled.
         *
         * Normalize only after the final startup view has been established so a valid
         * Fuzzy Tree Reverse setting remains untouched.
         */
        self.disable_reverse_for_fuzzy_list();
    }

    pub fn apply_ui_state(&mut self, state: crate::ui_state::UiState) {
        self.local_expand_all_warning_disabled = state.disable_local_expand_all_warning;
    }

    pub fn set_entry_filter(&mut self, entry_filter: EntryFilter) {
        if self.entry_filter == entry_filter {
            return;
        }

        let selected_path = self.selected_entry().map(|entry| entry.path.clone());

        self.entry_filter = entry_filter;

        match self.view_mode {
            ViewMode::List => {
                self.refresh_filter();
            }

            ViewMode::Tree if self.recursive_search_active() => {
                self.refresh_active_recursive_tree(selected_path.clone());
            }

            ViewMode::Tree => {
                self.refresh_tree_filter();
            }
        }

        if let Some(path) = selected_path {
            self.select_visible_path(&path);
        }

        self.ensure_selection_visible(self.viewport_rows);
    }

    pub fn disable_file_opening(&mut self) {
        self.allow_file_opening = false;
    }

    pub fn enable_exit_on_open(&mut self) {
        self.exit_on_open = true;
    }

    pub fn enable_preserved_download_hierarchy(&mut self) {
        self.ssh_config.preserve_hierarchy = true;
    }

    pub fn set_startup_query(&mut self, query: String) {
        self.search_navigation_active = false;

        self.search_return_state = None;

        self.query = query;

        self.query_cursor = self.query.len();
        self.selected = 0;

        self.list_offset = 0;

        if self.recursive_search_active() {
            self.ensure_recursive_scan();
        }

        match self.view_mode {
            ViewMode::List => {
                self.refresh_filter();
            }

            ViewMode::Tree if self.recursive_search_active() => {
                if !self.scan_in_progress {
                    match self.search_mode {
                        SearchMode::Exact => {
                            self.start_current_exact_filter();
                        }

                        SearchMode::Fuzzy => {
                            self.start_current_fuzzy_filter();
                        }
                    }
                }
            }

            ViewMode::Tree => {
                self.refresh_tree_filter();
            }
        }
    }

    pub fn enable_fuzzy_mode(&mut self) {
        if self.search_mode == SearchMode::Fuzzy {
            return;
        }

        self.toggle_search_mode();
    }

    fn effective_query_is_active(&self) -> bool {
        if self.query == "." {
            return false;
        }

        !parse_query(&self.query).is_effectively_empty()
    }

    pub fn recursive_search_active(&self) -> bool {
        self.recursive_mode
    }

    pub fn hidden_only_active(&self) -> bool {
        self.hidden_only
    }

    pub fn active_entry_count(&self) -> usize {
        self.active_entries().len()
    }

    pub fn source_label(&self) -> String {
        self.source.source_label()
    }

    /*
     * Timed informational and success notification.
     *
     * These messages are displayed steadily in the normal amber status color and
     * disappear automatically after five seconds.
     */
    pub fn show_info_message(&mut self, message: impl Into<String>) {
        self.error_message = None;

        self.error_message_expires_at = None;

        self.status_message = Some(message.into());

        self.status_message_expires_at = Some(Instant::now() + INFO_NOTIFICATION_DURATION);

        self.status_message_pulse_started_at = None;

        self.status_message_pulse_next_frame_at = None;
    }

    /*
     * Persistent informational notification whose amber brightness rises and falls
     * while an asynchronous operation remains active.
     *
     * The caller replaces or clears the message when the operation finishes.
     */
    fn show_pulsating_persistent_info_message(&mut self, message: impl Into<String>) {
        let now = Instant::now();

        self.error_message = None;

        self.error_message_expires_at = None;

        self.status_message = Some(message.into());

        /*
         * The remote-index loader owns this message's lifetime. Success or failure
         * replaces it, so no ordinary notification timeout is installed.
         */
        self.status_message_expires_at = None;

        self.status_message_pulse_started_at = Some(now);

        self.status_message_pulse_next_frame_at = Some(now + REMOTE_INDEX_NOTIFICATION_PULSE_FRAME);
    }

    /*
     * Timed error notification.
     *
     * Errors remain visible slightly longer than ordinary information because they
     * generally require more attention from the user.
     */
    pub fn show_error_message(&mut self, message: impl Into<String>) {
        self.status_message = None;

        self.status_message_expires_at = None;

        self.status_message_pulse_started_at = None;

        self.status_message_pulse_next_frame_at = None;

        self.error_message = Some(message.into());

        self.error_message_expires_at = Some(Instant::now() + ERROR_NOTIFICATION_DURATION);
    }

    /*
     * Persistent informational state.
     *
     * Examples:
     *
     *     Building remote index…
     *     Loading persistent remote index…
     *
     * These remain until the operation replaces or explicitly clears them.
     */
    pub fn show_persistent_info_message(&mut self, message: impl Into<String>) {
        self.error_message = None;

        self.error_message_expires_at = None;

        self.status_message = Some(message.into());

        self.status_message_expires_at = None;

        self.status_message_pulse_started_at = None;

        self.status_message_pulse_next_frame_at = None;
    }

    pub fn clear_messages(&mut self) {
        self.error_message = None;

        self.error_message_expires_at = None;

        self.status_message = None;

        self.status_message_expires_at = None;

        self.status_message_pulse_started_at = None;

        self.status_message_pulse_next_frame_at = None;
    }

    /*
     * Called by the event loop even when no keyboard or mouse input occurs.
     *
     * Returning true requests a redraw for notification expiration or one remote
     * index loading-pulse animation frame.
     */
    pub fn process_notification_timeouts(&mut self) -> bool {
        let now = Instant::now();

        let mut changed = false;

        if self
            .error_message_expires_at
            .is_some_and(|deadline| now >= deadline)
        {
            self.error_message = None;

            self.error_message_expires_at = None;

            changed = true;
        }

        if self
            .status_message_expires_at
            .is_some_and(|deadline| now >= deadline)
        {
            self.status_message = None;

            self.status_message_expires_at = None;

            self.status_message_pulse_started_at = None;

            self.status_message_pulse_next_frame_at = None;

            changed = true;
        }

        /*
         * Request regular redraws while the loading pulse is active.
         *
         * Advance repeatedly after a delayed event-loop iteration so the next frame
         * remains synchronized with real elapsed time rather than slowing the pulse.
         */
        if self.status_message.is_some()
            && self.status_message_pulse_started_at.is_some()
            && let Some(mut next_frame) = self.status_message_pulse_next_frame_at
            && now >= next_frame
        {
            while now >= next_frame {
                next_frame += REMOTE_INDEX_NOTIFICATION_PULSE_FRAME;
            }

            self.status_message_pulse_next_frame_at = Some(next_frame);

            changed = true;
        }

        changed
    }

    /*
     * Return the current remote-index loading-pulse brightness.
     *
     * 0 represents the dim endpoint and 255 the bright endpoint. Ordinary steady
     * notifications return None.
     */
    pub fn status_message_pulse_level(&self) -> Option<u8> {
        let started_at = self.status_message_pulse_started_at?;

        let cycle_milliseconds = REMOTE_INDEX_NOTIFICATION_PULSE_CYCLE.as_millis().max(1);

        let elapsed_milliseconds = started_at.elapsed().as_millis();

        let position = elapsed_milliseconds % cycle_milliseconds;

        let half_cycle = cycle_milliseconds / 2;

        let rising_value = if position <= half_cycle {
            position
        } else {
            cycle_milliseconds.saturating_sub(position)
        };

        let level = rising_value
            .saturating_mul(u8::MAX as u128)
            .checked_div(half_cycle.max(1))
            .unwrap_or(0);

        Some(level.min(u8::MAX as u128) as u8)
    }

    /*
     * Enter the lightweight redraw path used during rapid result navigation.
     *
     * Every repeated movement event extends the deadline. Once navigation has been
     * idle briefly, the normal complete layout measurement runs exactly once again.
     */
    pub fn begin_rapid_navigation(&mut self) {
        const RAPID_NAVIGATION_IDLE_TIME: Duration = Duration::from_millis(100);

        self.rapid_navigation_active = true;

        self.rapid_navigation_expires_at = Some(Instant::now() + RAPID_NAVIGATION_IDLE_TIME);
    }

    /*
     * Leave rapid-navigation mode after its short inactivity period.
     *
     * Returning true requests one final full redraw so complete horizontal and
     * metadata width measurements are restored after movement stops.
     */
    pub fn process_rapid_navigation_timeout(&mut self) -> bool {
        if !self.rapid_navigation_active {
            return false;
        }

        let Some(deadline) = self.rapid_navigation_expires_at else {
            self.rapid_navigation_active = false;

            return true;
        };

        if Instant::now() < deadline {
            return false;
        }

        self.rapid_navigation_active = false;

        self.rapid_navigation_expires_at = None;

        true
    }

    pub fn source_is_remote(&self) -> bool {
        self.source.is_remote()
    }

    /*
     * Build a serializable snapshot of the stable browser state.
     *
     * Transient overlays, workers, transfers, notifications, and confirmation
     * dialogs are deliberately excluded by the SessionState schema.
     */
    pub fn session_state(&self) -> io::Result<SessionState> {
        let source = if self.source.is_remote() {
            let target = self.active_ssh_target.as_ref().ok_or_else(|| {
                io::Error::other(
                    "the active filesystem source is remote but has no SSH target identity",
                )
            })?;

            SessionSource::Ssh {
                host: target.host.clone(),

                user: target.user.clone(),

                port: target.port,

                identity_file: target.identity_file.clone(),

                directory: self.current_directory.clone(),

                home_directory: self.home_directory.clone(),
            }
        } else {
            SessionSource::Local {
                directory: self.current_directory.clone(),

                home_directory: self.home_directory.clone(),
            }
        };

        let mut marked_files: Vec<SessionMarkedFile> = self
            .marked_files
            .values()
            .map(|marked| SessionMarkedFile {
                path: marked.path.clone(),

                filename: marked.filename.clone(),

                size_bytes: marked.size_bytes,
            })
            .collect();

        /*
         * HashMap order is undefined. Stable ordering makes session files easier to
         * inspect, compare, and test.
         */
        marked_files.sort_by(|left, right| left.path.cmp(&right.path));

        let recursive_expandable_directories: HashSet<PathBuf> = self
            .search_tree_children
            .iter()
            .filter_map(|(path, children)| {
                (path != &self.current_directory && !children.is_empty()).then_some(path.clone())
            })
            .collect();

        let ordinary_expand_all = self.view_mode == ViewMode::Tree
            && !self.recursive_search_active()
            && self.ordinary_expand_all_active;

        let recursive_expand_all = self.view_mode == ViewMode::Tree
            && self.recursive_mode
            && !self.effective_query_is_active()
            && !recursive_expandable_directories.is_empty()
            && recursive_expandable_directories
                .iter()
                .all(|path| self.recursive_expanded_directories.contains(path));

        let search_collapse_all = self.view_mode == ViewMode::Tree
            && self.recursive_search_active()
            && self.effective_query_is_active()
            && !recursive_expandable_directories.is_empty()
            && recursive_expandable_directories
                .iter()
                .all(|path| self.search_collapsed_directories.contains(path));

        /*
         * HashSet order is undefined. Sort manually arranged branch paths so the
         * resulting JSON remains stable and readable.
         *
         * Bulk states deliberately serialize an empty path array.
         */
        let mut expanded_directories: Vec<PathBuf> = if ordinary_expand_all {
            Vec::new()
        } else {
            self.expanded_directories.iter().cloned().collect()
        };

        expanded_directories.sort();

        let mut recursive_expanded_directories: Vec<PathBuf> = if recursive_expand_all {
            Vec::new()
        } else {
            self.recursive_expanded_directories
                .iter()
                .cloned()
                .collect()
        };

        recursive_expanded_directories.sort();

        let mut search_collapsed_directories: Vec<PathBuf> = if search_collapse_all {
            Vec::new()
        } else {
            self.search_collapsed_directories.iter().cloned().collect()
        };

        search_collapsed_directories.sort();

        Ok(SessionState {
            version: SESSION_FORMAT_VERSION,

            source,

            selected_path: self.selected_entry().map(|entry| entry.path.clone()),

            list_offset: self.list_offset,

            selected_viewport_row: Some(self.selected.saturating_sub(self.list_offset)),

            query: self.query.clone(),

            view_mode: match self.view_mode {
                ViewMode::List => "list",

                ViewMode::Tree => "tree",
            }
            .to_string(),

            search_mode: match self.search_mode {
                SearchMode::Exact => "exact",

                SearchMode::Fuzzy => "fuzzy",
            }
            .to_string(),

            recursive: self.recursive_mode,

            entry_filter: match self.entry_filter {
                EntryFilter::All => "all",

                EntryFilter::FilesOnly => "files",

                EntryFilter::DirectoriesOnly => "directories",
            }
            .to_string(),

            sort_mode: match self.sort_mode {
                SortMode::Name => "name",

                SortMode::Size => "size",

                SortMode::Modified => "date",

                SortMode::Type => "type",
            }
            .to_string(),

            reverse: self.sort_descending,

            show_hidden: self.show_hidden,

            hidden_only: self.hidden_only,

            show_icons: self.show_icons,

            show_file_colors: self.show_file_colors,

            show_details: self.show_details,

            show_selection: self.show_selection,

            show_columns: self.show_columns,

            show_permissions: self.show_permissions,

            show_size: self.show_size,

            show_date: self.show_date,

            show_user: self.show_user,

            ordinary_expand_all,

            recursive_expand_all,

            search_collapse_all,

            expanded_directories,

            recursive_expanded_directories,

            search_collapsed_directories,

            marked_files,
        })
    }

    /*
     * Restore state that depends on App's already constructed filesystem source.
     *
     * Source construction itself belongs in main.rs because an SSH session may fail
     * before App exists. This method restores only stable browser and selection state.
     */
    pub fn restore_session_state(&mut self, state: &SessionState) {
        /*
         * Marked files are meaningful only while the restored source is remote.
         *
         * A failed SSH restoration must never expose remote marks inside a local
         * fallback session.
         */
        self.marked_files.clear();

        if self.source.is_remote() {
            for marked in &state.marked_files {
                self.marked_files.insert(
                    marked.path.clone(),
                    MarkedFile {
                        path: marked.path.clone(),

                        filename: marked.filename.clone(),

                        size_bytes: marked.size_bytes,
                    },
                );
            }
        }

        self.pending_selection_path = state.selected_path.clone();

        self.pending_session_list_offset = Some(state.list_offset);

        self.pending_session_viewport_row = state.selected_viewport_row;

        /*
         * Session restoration never replays saved Tree expansion.
         *
         * A previous session may have ended with an extremely large hierarchy
         * expanded. Restoring that state automatically would bypass the user's
         * opportunity to apply the current Tree-display policy deliberately.
         *
         * Collapse state is safe to restore because it can only reduce visible
         * Tree rows.
         */
        self.pending_session_recursive_expand_all = false;

        self.pending_session_search_collapse_all = state.search_collapse_all;

        /*
         * Restore only Tree state that cannot increase the visible row count.
         *
         * Saved branch expansion is deliberately discarded during session restoration.
         * Search-collapse state remains safe to restore because it can only hide rows.
         *
         * Do not replay saved branch expansion during startup.
         *
         * Ordinary and queryless Recursive Trees will reopen only the ancestor
         * corridor required to reveal the restored selection.
         */
        self.expanded_directories.clear();

        self.recursive_expanded_directories.clear();

        /*
         * Saved collapse state is safe because it never increases the visible
         * Tree-row count.
         */
        self.search_collapsed_directories =
            state.search_collapsed_directories.iter().cloned().collect();

        /*
         * Expand All is never restored automatically.
         *
         * A restored session begins from a safely collapsed Tree and may reopen
         * only the ancestor corridor required for its saved selection.
         */
        self.ordinary_expand_all_active = false;
        /*
         * The startup configuration has already established view, search, recursive,
         * sorting, filtering, hidden-entry, and panel state.
         *
         * Install the query afterward so it is evaluated using those final modes.
         */
        self.set_startup_query(state.query.clone());

        /*
         * Ordinary Tree mode has no complete descendant corpus.
         *
         * Reopen the directory chain leading to the saved entry before attempting to
         * locate it in tree_rows. Recursive Tree restoration remains asynchronous and
         * is handled later when its scan or persistent index becomes available.
         */
        if self.view_mode == ViewMode::Tree
            && !self.recursive_mode
            && let Err(error) = self.restore_pending_non_recursive_tree_branch()
        {
            self.show_error_message(format!(
                "Unable to restore the saved Tree branch: {}",
                error,
            ));
        }

        /*
         * Non-recursive and already-loaded states can restore immediately.
         *
         * Recursive scans and remote-index loads retain the pending values and call
         * restore_pending_selection_if_available() when their results arrive.
         */
        self.restore_pending_selection_if_available();

        /*
         * Older session files do not contain selected_viewport_row.
         *
         * For those files only, retain the former absolute-offset restoration.
         * New sessions restore the selected entry's visual viewport row instead.
         */
        if state.selected_viewport_row.is_none()
            && self.pending_selection_path.is_none()
            && let Some(saved_offset) = self.pending_session_list_offset.take()
        {
            self.list_offset = saved_offset;
        }
    }

    fn persistent_remote_index_available(&self) -> bool {
        self.source.is_remote()
            && self.remote_index_loaded
            && self.recursive_cache_complete
            && !self.recursive_entries.is_empty()
    }

    #[allow(dead_code)]
    pub fn remote_index_identity(&self) -> Option<crate::remote_index::RemoteIndexIdentity> {
        self.source.remote_index_identity()
    }

    #[allow(dead_code)]
    pub fn remote_index_status(
        &self,
    ) -> io::Result<Option<crate::remote_index::RemoteIndexStatus>> {
        let Some(identity) = self.source.remote_index_identity() else {
            return Ok(None);
        };

        Ok(Some(identity.inspect()?))
    }

    pub fn selected_entry(&self) -> Option<&FileEntry> {
        match self.view_mode {
            ViewMode::List => self.entry_at_filtered_position(self.selected),

            ViewMode::Tree => self
                .tree_row_at_filtered_position(self.selected)
                .map(|row| &row.entry),
        }
    }

    fn prepare_marked_transfer_batch(&self) -> io::Result<(PathBuf, Vec<BatchTransferItem>, u64)> {
        if !self.source.is_remote() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "marked downloads are available only while browsing through SSH",
            ));
        }

        if self.marked_files.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "no files are marked",
            ));
        }

        /*
         * An SSH session entered through Scry's F4 dialog retains the complete local
         * browser session.
         *
         * A session started directly with `scry --ssh ...` has no earlier App-local
         * session, so its natural download destination is the process directory from
         * which Scry was launched.
         */
        let local_directory = self
            .saved_local_session
            .as_ref()
            .map(|session| session.directory.clone())
            .unwrap_or_else(|| self.launch_directory.clone());

        let destination_root = create_batch_download_directory(&local_directory)?;

        let mut marked_files: Vec<&MarkedFile> = self.marked_files.values().collect();

        /*
         * HashMap order is deliberately undefined. Sort by full remote path so the
         * queue order and popup progression remain stable and predictable.
         */
        marked_files.sort_by(|left, right| left.path.cmp(&right.path));

        let mut items = Vec::with_capacity(marked_files.len());

        let mut total_bytes = 0_u64;

        /*
         * Used only by flattened downloads to prevent two marked files with the same
         * basename from receiving the same destination.
         */
        let mut reserved_flat_paths = HashSet::new();

        for marked_file in marked_files {
            let destination_path = if self.ssh_config.preserve_hierarchy {
                let relative_path = match safe_batch_relative_path(&marked_file.path) {
                    Ok(relative_path) => relative_path,

                    Err(error) => {
                        /*
                         * Nothing has been downloaded yet. Remove the newly created empty
                         * batch root before returning the validation error.
                         */
                        let _ = std::fs::remove_dir(&destination_root);

                        return Err(error);
                    }
                };

                destination_root.join(relative_path)
            } else {
                match unique_flat_batch_destination(
                    &destination_root,
                    &marked_file.filename,
                    &mut reserved_flat_paths,
                ) {
                    Ok(destination_path) => destination_path,

                    Err(error) => {
                        let _ = std::fs::remove_dir(&destination_root);

                        return Err(error);
                    }
                }
            };

            total_bytes = match total_bytes.checked_add(marked_file.size_bytes) {
                Some(total_bytes) => total_bytes,

                None => {
                    let _ = std::fs::remove_dir(&destination_root);

                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "the marked transfer size exceeded the supported byte range",
                    ));
                }
            };

            items.push(BatchTransferItem {
                remote_path: marked_file.path.clone(),

                destination_path,

                filename: marked_file.filename.clone(),

                expected_size: marked_file.size_bytes,
            });
        }

        Ok((destination_root, items, total_bytes))
    }

    /*
     * Mark or unmark the file beneath the cursor.
     *
     * The ordinary cursor selection remains independent from this persistent
     * batch-selection state.
     */
    /*
     * Mark or unmark one remote file for a future SSH batch download.
     *
     * Local marking is deliberately unavailable because marks exist only to build
     * a remote download queue. The ordinary cursor selection remains independent
     * from this persistent batch-selection state.
     */
    pub fn toggle_mark_selected(&mut self) {
        if !self.source.is_remote() {
            self.show_info_message(
                "Selecting files can only be done via SSH when downloading files",
            );

            return;
        }

        let Some(entry) = self.selected_entry().cloned() else {
            self.show_error_message("No filesystem entry is selected");

            return;
        };

        if entry.is_directory {
            self.show_info_message("Directory marking is not supported yet");

            return;
        }

        if self.marked_files.remove(&entry.path).is_none() {
            let path = entry.path.clone();

            self.marked_files.insert(
                path.clone(),
                MarkedFile {
                    path,

                    filename: entry.name,

                    size_bytes: entry.size_bytes,
                },
            );
        }
    }

    pub fn is_path_marked(&self, path: &Path) -> bool {
        self.marked_files.contains_key(path)
    }

    pub fn marked_count(&self) -> usize {
        self.marked_files.len()
    }

    pub fn clear_marks(&mut self) {
        if !self.source.is_remote() {
            self.show_info_message("Deselection is available only while browsing through SSH");

            return;
        }

        let marked_count = self.marked_files.len();

        if marked_count == 0 {
            self.show_info_message("No files are marked");

            return;
        }

        self.marked_files.clear();

        self.show_info_message(format!(
            "Cleared {} marked file{}",
            marked_count,
            if marked_count == 1 { "" } else { "s" },
        ));
    }

    pub fn copy_selected_path(&mut self) {
        let Some(path) = self.selected_entry().map(|entry| entry.path.clone()) else {
            self.show_error_message("No filesystem entry is selected");

            return;
        };

        let path_text = path.to_string_lossy().into_owned();

        match copy_path_to_clipboard(&path_text, self) {
            Ok(()) => {
                self.last_copied_path = Some(path_text.clone());

                self.show_info_message(format!("Copied path: {}", path_text));
            }

            Err(error) => {
                self.show_error_message(format!("Unable to copy path to clipboard: {}", error,));
            }
        }
    }

    #[cfg(target_os = "linux")]
    pub fn clipboard_handoff_text(&mut self) -> Option<String> {
        let expected_text = self.last_copied_path.as_ref()?;

        /*
         * Do not restore an old Scry path if the user copied something else before
         * closing Scry.
         */
        let current_text = self.clipboard.as_mut()?.0.get_contents().ok()?;

        if current_text == *expected_text {
            Some(expected_text.clone())
        } else {
            None
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn clipboard_handoff_text(&mut self) -> Option<String> {
        /*
         * OSC 52 transfers the clipboard contents to the terminal emulator.
         *
         * No post-exit owner handoff is needed.
         */
        None
    }

    pub fn file_info_visible(&self) -> bool {
        self.file_info.is_some()
    }

    pub fn open_file_info(&mut self) {
        /*
         * Opening File Information again while it is already visible behaves
         * like a toggle.
         */
        if self.file_info_visible() {
            self.close_file_info();

            return;
        }

        /*
         * Copy everything needed from the selected entry before mutably
         * borrowing the active source.
         */
        let Some(entry) = self.selected_entry().cloned() else {
            self.show_error_message("No filesystem entry is selected");

            return;
        };

        let kind = if entry.is_symlink {
            crate::entry::EntryKind::Symlink
        } else if entry.is_directory {
            crate::entry::EntryKind::Directory
        } else {
            crate::entry::EntryKind::File
        };

        let source_label = self.source.source_label();

        let is_remote = self.source.is_remote();

        let initial_info = FileInfo::from_entry(&entry, kind, source_label, is_remote);

        self.file_info_generation = self.file_info_generation.wrapping_add(1);

        let generation = self.file_info_generation;

        /*
         * Install the initial popup state before starting the worker.
         *
         * Even when the source cannot provide extended information, the window
         * can still display everything already stored in FileEntry.
         */
        self.file_info = Some(FileInfoState::loading(initial_info.clone()));

        match self.source.start_file_info(initial_info, generation) {
            Ok(receiver) => {
                self.file_info_receiver = Some(receiver);
            }

            Err(error) => {
                self.file_info_receiver = None;

                if let Some(state) = self.file_info.as_mut() {
                    state.fail(format!(
                        "Unable to load extended file information: {}",
                        error,
                    ));
                }
            }
        }
    }

    pub fn close_file_info(&mut self) {
        /*
         * Advancing the generation makes any late worker result obsolete.
         */
        self.file_info_generation = self.file_info_generation.wrapping_add(1);

        self.file_info_receiver = None;

        self.file_info = None;
    }

    pub fn process_file_info_messages(&mut self) -> bool {
        let message = match self.file_info_receiver.as_ref() {
            Some(receiver) => match receiver.try_recv() {
                Ok(message) => message,

                Err(TryRecvError::Empty) => {
                    return false;
                }

                Err(TryRecvError::Disconnected) => {
                    self.file_info_receiver = None;

                    if let Some(state) = self.file_info.as_mut()
                        && state.loading
                    {
                        state.fail("The file-information worker stopped unexpectedly".to_string());
                    }

                    return true;
                }
            },

            None => {
                return false;
            }
        };

        match message {
            FileInfoMessage::Finished { generation, info } => {
                if generation != self.file_info_generation {
                    return false;
                }

                self.file_info_receiver = None;

                if let Some(state) = self.file_info.as_mut() {
                    state.finish(*info);
                }
            }

            FileInfoMessage::Failed {
                generation,
                message,
            } => {
                if generation != self.file_info_generation {
                    return false;
                }

                self.file_info_receiver = None;

                if let Some(state) = self.file_info.as_mut() {
                    state.fail(message);
                }
            }
        }

        true
    }

    pub fn scroll_file_info_up(&mut self) {
        if let Some(state) = self.file_info.as_mut() {
            state.scroll_up();
        }
    }

    pub fn scroll_file_info_down(&mut self) {
        if let Some(state) = self.file_info.as_mut() {
            state.scroll_down();
        }
    }

    pub fn page_file_info_up(&mut self) {
        let amount = self.viewport_rows.saturating_sub(1).max(1) as u16;

        if let Some(state) = self.file_info.as_mut() {
            state.page_up(amount);
        }
    }

    pub fn page_file_info_down(&mut self) {
        let amount = self.viewport_rows.saturating_sub(1).max(1) as u16;

        if let Some(state) = self.file_info.as_mut() {
            state.page_down(amount);
        }
    }

    pub fn file_info_scroll_to_start(&mut self) {
        if let Some(state) = self.file_info.as_mut() {
            state.scroll_to_start();
        }
    }

    pub fn file_info_scroll_to_end(&mut self) {
        if let Some(state) = self.file_info.as_mut() {
            state.scroll_to_end();
        }
    }

    pub fn selected_classification(&mut self) -> Option<FileClass> {
        /*
         * Copy what we need out of the selected entry first so that the immutable
         * entry borrow ends before the cache is modified.
         */
        let (path, initial_class) = {
            let entry = self.selected_entry()?;

            (entry.path.clone(), entry.class)
        };

        if !matches!(initial_class, FileClass::Unknown | FileClass::Executable) {
            return Some(initial_class);
        }

        if let Some(class) = self.classification_inspection_cache.get(&path) {
            return Some(*class);
        }

        let refined_class = inspect_file(&path, initial_class);

        self.classification_inspection_cache
            .insert(path, refined_class);

        Some(refined_class)
    }

    pub fn tree_row_at_filtered_position(&self, position: usize) -> Option<&TreeRow> {
        let tree_index = *self.filtered_tree_indices.get(position)?;

        self.tree_rows.get(tree_index)
    }

    pub fn entry_at_filtered_position(&self, position: usize) -> Option<&FileEntry> {
        let entry_index = *self.filtered_indices.get(position)?;

        self.active_entries().get(entry_index)
    }

    pub fn ensure_selection_visible(&mut self, visible_rows: usize) {
        let entry_count = self.current_visible_entry_count();

        if visible_rows == 0 || entry_count == 0 {
            self.selected = 0;

            self.list_offset = 0;

            return;
        }

        self.selected = self.selected.min(entry_count.saturating_sub(1));

        if self.selected < self.list_offset {
            self.list_offset = self.selected;
        } else {
            let viewport_end = self.list_offset.saturating_add(visible_rows);

            if self.selected >= viewport_end {
                self.list_offset = self.selected.saturating_add(1).saturating_sub(visible_rows);
            }
        }

        let maximum_offset = entry_count.saturating_sub(visible_rows);

        self.list_offset = self.list_offset.min(maximum_offset);
    }

    pub fn process_remote_index_load_messages(&mut self) -> bool {
        let message = match self.remote_index_load_receiver.as_ref() {
            Some(receiver) => match receiver.try_recv() {
                Ok(message) => message,

                Err(TryRecvError::Empty) => {
                    return false;
                }

                Err(TryRecvError::Disconnected) => {
                    self.remote_index_load_receiver = None;

                    self.remote_index_load_in_progress = false;

                    self.remote_index_load_purpose = None;

                    self.show_error_message("Remote index loader stopped unexpectedly");

                    return true;
                }
            },

            None => {
                return false;
            }
        };

        self.remote_index_load_receiver = None;

        self.remote_index_load_in_progress = false;

        let load_purpose = self
            .remote_index_load_purpose
            .take()
            .unwrap_or(RemoteIndexLoadPurpose::Background);

        match message.result {
            Ok(prepared) => {
                self.cancel_fuzzy_filter();

                /*
                 * Move the already prepared worker result into the application.
                 *
                 * Destructure first so every large allocation can be moved without
                 * cloning.
                 */
                let PreparedRemoteIndex {
                    loaded,

                    child_indices,

                    search_index,
                } = prepared;

                /*
                 * A directory-rooted recursive scan may still be running while the
                 * persistent host-wide index loads in parallel.
                 *
                 * The installed index becomes the sole authoritative corpus.
                 */
                self.scan_receiver = None;

                self.scan_generation = self.scan_generation.wrapping_add(1);

                self.scan_in_progress = false;

                self.recursive_entries = loaded.entries;

                self.recursive_child_indices = child_indices;

                self.search_index = Arc::new(search_index);

                self.recursive_cache_complete = true;

                self.recursive_scan_partial = loaded.info.partial;

                self.remote_index_loaded = true;

                self.remote_index_includes_hidden = loaded.info.includes_hidden;

                self.error_message = None;

                self.show_info_message(format!(
                    "Remote index successfully loaded — {} entries",
                    loaded.info.entry_count,
                ));

                /*
                 * Automatic connection loading must not alter the visible browser.
                 *
                 * The complete corpus is now resident for future Alt+R, Alt+E, and
                 * remote Tree operations, while ordinary List/Tree mode stays exactly
                 * where the user left it.
                 */
                if load_purpose == RemoteIndexLoadPurpose::Background {
                    return true;
                }

                /*
                 * An explicit Recursive request was waiting for this load.
                 */
                self.recursive_mode = true;

                self.selected = 0;

                self.list_offset = 0;

                match self.view_mode {
                    ViewMode::List => {
                        self.refresh_filter();

                        self.restore_pending_selection_if_available();
                    }

                    ViewMode::Tree => {
                        if self.effective_query_is_active() {
                            match self.search_mode {
                                SearchMode::Exact => {
                                    self.start_current_exact_filter();
                                }

                                SearchMode::Fuzzy => {
                                    self.start_current_fuzzy_filter();
                                }
                            }
                        } else {
                            let selected_path = self.pending_selection_path.clone();

                            self.rebuild_recursive_search_tree(selected_path);

                            self.restore_pending_selection_if_available();
                        }
                    }
                }
            }

            Err(message) => {
                self.remote_index_loaded = false;

                self.show_error_message(format!("Unable to load remote index: {}", message,));
            }
        }

        true
    }

    /*
     * Drain every currently queued worker message.
     *
     * The explicit loop keeps receiver removal, completion handling, and
     * disconnection cleanup visible in one place.
     */
    #[allow(clippy::while_let_loop)]
    pub fn process_remote_index_messages(&mut self) -> bool {
        let mut changed = false;

        loop {
            let message = match self.remote_index_build_receiver.as_ref() {
                Some(receiver) => match receiver.try_recv() {
                    Ok(message) => message,

                    Err(TryRecvError::Empty) => {
                        break;
                    }

                    Err(TryRecvError::Disconnected) => {
                        self.remote_index_build_receiver = None;

                        if self.remote_index_build_in_progress {
                            self.remote_index_build_in_progress = false;

                            self.show_error_message("Remote index worker stopped unexpectedly");

                            changed = true;
                        }

                        break;
                    }
                },

                None => {
                    break;
                }
            };

            match message {
                RemoteIndexBuildMessage::Progress { entries_written } => {
                    self.remote_index_entries_written = entries_written;

                    self.show_persistent_info_message(format!(
                        "Building remote index from / — {} entries written…",
                        entries_written,
                    ));

                    changed = true;
                }

                RemoteIndexBuildMessage::Finished(info) => {
                    self.remote_index_entries_written = info.entry_count;

                    self.remote_index_build_in_progress = false;

                    self.remote_index_build_receiver = None;

                    self.pending_remote_index_hidden_policy = None;

                    self.show_info_message(format!(
                        "Remote index ready — {} entries saved",
                        info.entry_count,
                    ));

                    changed = true;

                    break;
                }

                RemoteIndexBuildMessage::Failed { message } => {
                    self.remote_index_build_in_progress = false;

                    self.remote_index_build_receiver = None;

                    self.pending_remote_index_hidden_policy = None;

                    self.show_error_message(message);

                    changed = true;

                    break;
                }
            }
        }

        changed
    }

    fn progressive_exact_tree_is_active(&self) -> bool {
        !self.source.is_remote()
            && self.view_mode == ViewMode::Tree
            && self.recursive_search_active()
            && self.search_mode == SearchMode::Exact
            && self.effective_query_is_active()
    }

    fn reset_progressive_exact_tree(&mut self) {
        self.progressive_exact_tree_identity = None;

        self.progressive_exact_tree_matches.clear();

        self.progressive_exact_tree_last_published_entry_count = 0;

        self.progressive_exact_tree_last_published_match_count = 0;
    }

    /*
     * Ensure that the retained progressive matches describe the query currently
     * visible in the Search field.
     *
     * Normally a new recursive scan begins with an empty corpus, so this reset is
     * practically free. If the user edits the query while that same scan is already
     * underway, the existing portion of the corpus is evaluated once for the new
     * query so entries discovered before the edit cannot disappear from consideration.
     */
    fn prepare_progressive_exact_tree(&mut self) {
        if !self.progressive_exact_tree_is_active() {
            self.reset_progressive_exact_tree();

            return;
        }

        let identity = ProgressiveExactTreeIdentity {
            query: self.query.clone(),

            scan_generation: self.scan_generation,

            entry_filter: self.entry_filter,
        };

        if self.progressive_exact_tree_identity.as_ref() == Some(&identity) {
            return;
        }

        let parsed_query = parse_query(&self.query);

        let entry_filter = self.entry_filter;

        let mut matches = Vec::with_capacity(self.exact_tree_match_limit);

        let mut limit_reached = false;

        for (index, entry) in self.recursive_entries.iter().enumerate() {
            if !entry.path.starts_with(&self.current_directory) {
                continue;
            }

            if !entry_filter.matches(entry) {
                continue;
            }

            if !entry_matches_query(entry, &parsed_query) {
                continue;
            }

            if matches.len() >= self.exact_tree_match_limit {
                limit_reached = true;

                break;
            }

            matches.push(index);
        }

        self.progressive_exact_tree_identity = Some(identity);

        self.progressive_exact_tree_matches = matches;

        self.progressive_exact_tree_last_published_entry_count = 0;

        self.progressive_exact_tree_last_published_match_count = 0;

        self.exact_tree_limit_reached = limit_reached;
    }

    /*
     * Evaluate only the newly arrived scanner batch.
     *
     * base_entry_index is the future position of entries[0] inside
     * recursive_entries. This is the same index relationship used by SearchIndex,
     * recursive_path_indices, and recursive_child_indices.
     */
    fn extend_progressive_exact_tree(&mut self, entries: &[FileEntry], base_entry_index: usize) {
        self.prepare_progressive_exact_tree();

        if self.progressive_exact_tree_identity.is_none() {
            return;
        }

        /*
         * Once an additional match beyond the configured retained-entry limit has
         * proved that the cap was reached, later entries cannot alter the retained
         * result set. The scanner itself continues providing truthful corpus progress.
         */
        if self.exact_tree_limit_reached {
            return;
        }

        let parsed_query = parse_query(&self.query);

        let entry_filter = self.entry_filter;

        for (offset, entry) in entries.iter().enumerate() {
            if !entry.path.starts_with(&self.current_directory) {
                continue;
            }

            if !entry_filter.matches(entry) {
                continue;
            }

            if !entry_matches_query(entry, &parsed_query) {
                continue;
            }

            if self.progressive_exact_tree_matches.len() >= self.exact_tree_match_limit {
                self.exact_tree_limit_reached = true;

                break;
            }

            self.progressive_exact_tree_matches
                .push(base_entry_index + offset);
        }
    }

    fn publish_progressive_exact_tree(&mut self, force: bool) -> bool {
        self.prepare_progressive_exact_tree();

        if self.progressive_exact_tree_identity.is_none() {
            return false;
        }

        let entry_count = self.recursive_entries.len();

        let match_count = self.progressive_exact_tree_matches.len();

        let first_useful_result =
            self.progressive_exact_tree_last_published_match_count == 0 && match_count > 0;

        let enough_corpus_growth = entry_count
            .saturating_sub(self.progressive_exact_tree_last_published_entry_count)
            >= EXACT_TREE_PROGRESS_ENTRY_INTERVAL;

        let result_changed = match_count != self.progressive_exact_tree_last_published_match_count;

        if !force && !first_useful_result && !(enough_corpus_growth && result_changed) {
            return false;
        }

        let preferred_selection = self
            .pending_selection_path
            .clone()
            .or_else(|| self.selected_entry().map(|entry| entry.path.clone()));

        /*
         * At most the configured Exact Tree match limit of indices are cloned here.
         *
         * The complete recursive corpus remains untouched.
         */
        let matched_indices = self.progressive_exact_tree_matches.clone();

        self.rebuild_fuzzy_search_tree_from_indices(&matched_indices, preferred_selection);

        self.restore_pending_selection_if_available();

        self.progressive_exact_tree_last_published_entry_count = entry_count;

        self.progressive_exact_tree_last_published_match_count = match_count;

        true
    }

    /*
     * Drain all currently available scan batches before rebuilding or
     * refreshing the visible result state.
     *
     * The receiver may be cleared after completion or disconnection, so the
     * explicit loop makes the ownership transition easier to follow.
     */
    #[allow(clippy::while_let_loop)]
    pub fn process_scan_messages(&mut self) -> bool {
        /*
         * A loaded persistent remote index is the authoritative recursive corpus.
         *
         * No directory-rooted scanner message may append to it. This guard protects
         * against stale receivers retained by an unexpected future transition as well
         * as the normal index-load race.
         */
        if self.persistent_remote_index_available() {
            self.scan_receiver = None;

            self.scan_in_progress = false;

            return false;
        }

        let mut changed = false;

        let mut scan_finished = false;

        loop {
            let message = match self.scan_receiver.as_ref() {
                Some(receiver) => match receiver.try_recv() {
                    Ok(message) => message,

                    Err(TryRecvError::Empty) => {
                        break;
                    }

                    Err(TryRecvError::Disconnected) => {
                        scan_finished = true;

                        break;
                    }
                },

                None => {
                    break;
                }
            };

            match message {
                ScanMessage::Batch {
                    generation,
                    mut entries,
                } => {
                    if generation != self.scan_generation {
                        continue;
                    }

                    let base_entry_index = self.recursive_entries.len();

                    /*
                     * Exact Recursive Tree can consume this local batch immediately.
                     *
                     * The entries have not yet moved into recursive_entries, but their
                     * future indices are already known from base_entry_index.
                     */
                    self.extend_progressive_exact_tree(&entries, base_entry_index);

                    /*
                     * Index the batch before moving its entries into recursive_entries.
                     */
                    Arc::make_mut(&mut self.search_index)
                        .extend_from_entries(&entries, base_entry_index);

                    /*
                     * Record each path's future position in recursive_entries.
                     *
                     * The batch is appended unchanged immediately below, so
                     * base_entry_index + offset is the exact resulting vector index.
                     */
                    for (offset, entry) in entries.iter().enumerate() {
                        let future_index = base_entry_index + offset;

                        if let Some(parent) = entry.path.parent() {
                            self.recursive_child_indices
                                .entry(parent.to_path_buf())
                                .or_default()
                                .push(future_index);
                        }
                    }

                    self.recursive_entries.append(&mut entries);

                    changed = true;
                }

                ScanMessage::Finished {
                    generation,
                    partial,
                } => {
                    if generation != self.scan_generation {
                        continue;
                    }

                    self.scan_in_progress = false;

                    self.recursive_scan_partial = partial;

                    self.recursive_cache_complete = true;

                    scan_finished = true;

                    changed = true;
                }

                ScanMessage::Failed {
                    generation,
                    message,
                } => {
                    if generation != self.scan_generation {
                        continue;
                    }

                    self.show_error_message(message);

                    self.scan_in_progress = false;

                    self.recursive_cache_complete = true;

                    scan_finished = true;

                    changed = true;
                }
            }
        }

        if scan_finished {
            self.scan_receiver = None;
        }

        if changed {
            let text_filter_active = self.effective_query_is_active();

            if scan_finished
                && self.pending_tree_expand_all.is_some()
                && self.view_mode == ViewMode::Tree
            {
                let request = self
                    .pending_tree_expand_all
                    .take()
                    .expect("pending Tree expansion request disappeared");

                /*
                 * Keep the shared pending selection alive through the completed Tree rebuild.
                 *
                 * Session restoration may still need its saved viewport row after Expand All
                 * has materialized the final hierarchy. restore_pending_selection_if_available()
                 * is responsible for consuming that state once the Tree is stable.
                 */
                let pending_selection_path = self.pending_selection_path.clone();

                let selected_path = request
                    .selected_path
                    .or(pending_selection_path)
                    .or_else(|| self.selected_entry().map(|entry| entry.path.clone()));
                /*
                 * Resume through the centralized expansion path.
                 *
                 * The recursive corpus is now complete, so both ordinary and queryless
                 * Recursive targets can be counted and materialized without starting
                 * another scan.
                 */
                self.apply_tree_expand_all(request.target, selected_path);

                return true;
            }

            if scan_finished
                && self.pending_hidden_tree_limit_check
                && self.view_mode == ViewMode::Tree
                && !text_filter_active
            {
                self.pending_hidden_tree_limit_check = false;

                let selected_path = self
                    .pending_selection_path
                    .clone()
                    .or_else(|| self.selected_entry().map(|entry| entry.path.clone()));

                if self.recursive_search_active() {
                    /*
                     * Queryless Recursive Tree.
                     *
                     * Reconstruct the real Hidden-inclusive representation from the completed
                     * corpus before checking the hard visible-row ceiling.
                     */
                    if self.pending_recursive_visibility_expand_restore {
                        self.pending_recursive_visibility_expand_restore = false;

                        self.prepare_complete_queryless_recursive_tree();

                        let currently_expandable = self.indexed_ordinary_expandable_directories();

                        self.recursive_expanded_directories
                            .retain(|path| currently_expandable.contains(path));

                        self.rebuild_recursive_search_rows(selected_path.clone());
                    } else {
                        self.rebuild_recursive_search_tree(selected_path.clone());
                    }
                } else {
                    /*
                     * Ordinary Tree.
                     *
                     * Build the complete hierarchy from the now-stable Hidden-inclusive corpus,
                     * while preserving only the branches that were already open.
                     */
                    self.prepare_ordinary_tree_from_recursive_corpus();

                    self.rebuild_tree_rows(selected_path.clone());
                }

                let would_be_rows = self.filtered_tree_indices.len();

                if would_be_rows > self.advanced_tree_config.max_visible_tree_rows {
                    self.show_hidden = false;

                    if self.recursive_search_active() {
                        self.refresh_active_recursive_tree(selected_path.clone());
                    } else {
                        self.rebuild_tree_rows(selected_path.clone());
                    }

                    self.refuse_tree_visibility_transition(
                        would_be_rows,
                        TreeDisplayLimitAction::ShowHidden,
                    );
                }

                self.restore_pending_selection_if_available();
                self.ensure_selection_visible(self.viewport_rows);

                return true;
            }

            if self.view_mode == ViewMode::Tree && self.recursive_search_active() {
                if text_filter_active {
                    match self.search_mode {
                        SearchMode::Exact if self.progressive_exact_tree_is_active() => {
                            /*
                             * Local Exact Tree results are accumulated directly from
                             * scanner batches.
                             *
                             * No completed-corpus Exact worker is required afterward:
                             * every scanned entry has already been evaluated exactly once.
                             */
                            let published = self.publish_progressive_exact_tree(scan_finished);

                            if scan_finished {
                                self.pending_recursive_search_at = None;

                                self.fuzzy_filter_in_progress = false;

                                self.fuzzy_examined = self.recursive_entries.len();

                                self.fuzzy_total = self.recursive_entries.len();

                                /*
                                 * A force-published empty result is still a valid final
                                 * Tree. publish_progressive_exact_tree() handles it.
                                 */
                                if !published {
                                    self.publish_progressive_exact_tree(true);
                                }
                            }
                        }

                        SearchMode::Exact => {
                            /*
                             * Remote persistent indexes and any future non-progressive
                             * Exact corpus retain the normal completed-index worker.
                             */
                            if scan_finished {
                                self.start_current_exact_filter();
                            }
                        }

                        SearchMode::Fuzzy => {
                            /*
                             * Fuzzy ranking depends on comparing candidates across the
                             * complete stable corpus.
                             */
                            if scan_finished {
                                self.start_current_fuzzy_filter();
                            }
                        }
                    }
                } else if scan_finished {
                    let selected_path = self
                        .pending_selection_path
                        .clone()
                        .or_else(|| self.selected_entry().map(|entry| entry.path.clone()));

                    if !self.pending_hidden_tree_limit_check {
                        self.rebuild_recursive_search_tree(selected_path);
                        self.restore_pending_selection_if_available();
                    }
                }
            } else if !text_filter_active {
                /*
                 * An empty query is ordinary directory browsing.
                 *
                 * The recursive scanner is only building a background corpus. Incoming
                 * batches must not refresh the visible directory list, alter its
                 * selection, or move its viewport.
                 *
                 * A redraw is still requested through the returned `changed` value so
                 * scan status may update without touching navigation state.
                 */
            } else if self.search_mode == SearchMode::Fuzzy && self.recursive_search_active() {
                /*
                 * Recursive Fuzzy mode consumes only a complete stable index.
                 *
                 * Publishing fuzzy results for every scanner batch causes constant
                 * reranking and severe UI churn.
                 */
                if scan_finished {
                    self.refresh_filter();
                }
            } else {
                /*
                 * Exact recursive text search remains incremental.
                 *
                 * Preserve the selected path across each result-set update rather than
                 * allowing a newly inserted batch to displace the current selection.
                 */
                let selected_path = self.selected_entry().map(|entry| entry.path.clone());

                self.refresh_filter();

                if let Some(path) = selected_path {
                    self.select_visible_path(&path);
                } else {
                    self.restore_pending_selection_if_available();
                }
            }
        }

        changed
    }

    pub fn move_query_cursor_left(&mut self) {
        /*
         * Query-clearing and restored-navigation paths may replace the query while
         * an older caret position still exists. Normalize it before slicing.
         */
        self.query_cursor = self.query_cursor.min(self.query.len());

        while !self.query.is_char_boundary(self.query_cursor) {
            self.query_cursor = self.query_cursor.saturating_sub(1);
        }

        if self.query_cursor == 0 {
            return;
        }

        self.query_cursor = self.query[..self.query_cursor]
            .char_indices()
            .next_back()
            .map(|(index, _)| index)
            .unwrap_or(0);
    }

    pub fn move_query_cursor_right(&mut self) {
        if self.query_cursor >= self.query.len() {
            self.query_cursor = self.query.len();

            return;
        }

        let next_character_length = self.query[self.query_cursor..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(0);

        self.query_cursor = self
            .query_cursor
            .saturating_add(next_character_length)
            .min(self.query.len());
    }

    pub fn move_query_cursor_to_start(&mut self) {
        self.query_cursor = 0;
    }

    pub fn move_query_cursor_to_end(&mut self) {
        self.query_cursor = self.query.len();
    }

    fn insert_query_character_at_cursor(&mut self, character: char) {
        /*
         * Recover safely if an older restored state left the caret beyond
         * the current query.
         */
        self.query_cursor = self.query_cursor.min(self.query.len());

        while !self.query.is_char_boundary(self.query_cursor) {
            self.query_cursor = self.query_cursor.saturating_sub(1);
        }

        self.query.insert(self.query_cursor, character);

        self.query_cursor += character.len_utf8();
    }

    fn remove_query_character_before_cursor(&mut self) -> bool {
        if self.query_cursor == 0 || self.query.is_empty() {
            return false;
        }

        self.query_cursor = self.query_cursor.min(self.query.len());

        while !self.query.is_char_boundary(self.query_cursor) {
            self.query_cursor = self.query_cursor.saturating_sub(1);
        }

        let previous_character_start = self.query[..self.query_cursor]
            .char_indices()
            .next_back()
            .map(|(index, _)| index)
            .unwrap_or(0);

        self.query
            .drain(previous_character_start..self.query_cursor);

        self.query_cursor = previous_character_start;

        true
    }

    pub fn push_query_character(&mut self, character: char) {
        self.search_navigation_active = false;

        /*
         * A newly edited query is a new search session.
         *
         * Do not allow a return state from an older root or older query to survive and
         * later redirect navigation unexpectedly.
         */
        self.search_return_state = None;

        if self.view_mode == ViewMode::Tree {
            let search_was_active = self.effective_query_is_active();

            let selected_path = self.selected_entry().map(|entry| entry.path.clone());

            /*
             * Save the ordinary manually expanded Tree at the moment a genuine search
             * begins. Directive-only and incomplete queries do not count as searches.
             */
            if !search_was_active {
                self.tree_search_saved_selection = selected_path.clone();

                self.tree_search_saved_offset = self.list_offset;

                self.search_collapsed_directories.clear();
            }

            self.insert_query_character_at_cursor(character);

            let search_is_active = self.effective_query_is_active();

            /*
             * An effective query has just become inactive.
             *
             * Examples:
             *
             *     type:dir  -> type:
             *     README    -> type:sensitive
             *
             * Discard the automatically expanded search hierarchy and restore the
             * ordinary manual Tree immediately.
             */
            if search_was_active && !search_is_active {
                self.pending_recursive_search_at = None;

                self.cancel_fuzzy_filter();

                self.restore_manual_tree();

                return;
            }

            if search_is_active {
                if !search_was_active {
                    self.ensure_recursive_scan();
                }

                if !self.scan_in_progress {
                    self.pending_selection_path = selected_path;

                    self.schedule_current_recursive_search();
                }
            } else {
                /*
                 * The query remains ineffective, such as while typing `type:`.
                 *
                 * Keep displaying the ordinary Tree.
                 */
                self.refresh_tree_filter();
            }

            return;
        }
        let search_was_active = self.recursive_search_active();

        self.insert_query_character_at_cursor(character);

        if !search_was_active && self.recursive_search_active() {
            self.ensure_recursive_scan();
        }

        self.selected = 0;

        self.list_offset = 0;

        if self.recursive_search_active() && !self.query.is_empty() && self.query != "." {
            self.schedule_current_recursive_search();
        } else {
            self.pending_recursive_search_at = None;

            self.refresh_filter();
        }
    }

    pub fn pop_query_character(&mut self) {
        self.search_navigation_active = false;

        /*
         * A newly edited query is a new search session.
         *
         * Do not allow a return state from an older root or older query to survive and
         * later redirect navigation unexpectedly.
         */
        self.search_return_state = None;

        if self.view_mode == ViewMode::Tree {
            let search_was_active = self.effective_query_is_active();

            let selected_path = self.selected_entry().map(|entry| entry.path.clone());

            self.remove_query_character_before_cursor();

            let search_is_active = self.effective_query_is_active();

            /*
             * A real search has just become directive-only, incomplete, or empty.
             *
             * Restore the manually browsed Tree instead of preserving the expanded
             * recursive search hierarchy.
             */
            if search_was_active && !search_is_active {
                self.pending_recursive_search_at = None;

                self.cancel_fuzzy_filter();

                self.restore_manual_tree();

                return;
            }

            /*
             * Deleting a character can also make an ineffective query effective:
             *
             *     type:  -> type
             *
             * Capture the current manual Tree before starting that new search.
             */
            if !search_was_active && search_is_active {
                self.tree_search_saved_selection = selected_path.clone();

                self.tree_search_saved_offset = self.list_offset;

                self.search_collapsed_directories.clear();

                self.ensure_recursive_scan();
            }

            if search_is_active {
                if !self.scan_in_progress {
                    self.pending_selection_path = selected_path;

                    self.schedule_current_recursive_search();
                }
            } else {
                self.refresh_tree_filter();
            }

            return;
        }
        self.remove_query_character_before_cursor();

        if self.recursive_search_active() {
            self.ensure_recursive_scan();
        }

        self.selected = 0;

        self.list_offset = 0;

        if self.recursive_search_active() && !self.query.is_empty() && self.query != "." {
            self.schedule_current_recursive_search();
        } else {
            self.pending_recursive_search_at = None;

            self.refresh_filter();
        }
    }

    pub fn clear_query(&mut self) {
        self.search_navigation_active = false;

        self.search_return_state = None;

        self.pending_recursive_search_at = None;

        self.cancel_fuzzy_filter();

        if self.view_mode == ViewMode::Tree {
            self.query.clear();

            self.query_cursor = 0;

            self.selected = 0;

            self.list_offset = 0;

            if self.recursive_search_active() {
                self.ensure_recursive_scan();

                if !self.scan_in_progress {
                    self.rebuild_recursive_search_tree(None);
                } else {
                    self.tree_rows.clear();

                    self.filtered_tree_indices.clear();

                    self.search_tree_children.clear();
                }
            } else {
                self.restore_manual_tree();
            }

            return;
        }

        self.query.clear();

        self.query_cursor = 0;

        self.selected = 0;

        self.list_offset = 0;

        self.refresh_filter();
    }

    pub fn toggle_details(&mut self) {
        self.show_details = !self.show_details;
    }

    pub fn toggle_icons(&mut self) {
        self.show_icons = !self.show_icons;
    }

    /*
     * F12 changes presentation only.
     *
     * FileEntry already stores its established FileClass, so this toggle requires
     * no filesystem scan, index rebuild, filtering, or Tree reconstruction.
     */
    pub fn toggle_file_colors(&mut self) {
        self.show_file_colors = !self.show_file_colors;
    }

    pub fn toggle_selection_panel(&mut self) {
        self.show_selection = !self.show_selection;
    }

    #[allow(dead_code)]
    pub fn toggle_columns_panel(&mut self) {
        let has_enabled_column =
            self.show_permissions || self.show_size || self.show_date || self.show_user;

        if !has_enabled_column {
            /*
             * An enabled-but-empty Metadata panel is visually indistinguishable
             * from a failed shortcut. Keep it closed and explain how to populate it.
             */
            self.show_columns = false;

            self.show_info_message(
                "The Metadata panel has no enabled columns. Use F7–F10 to enable one first.",
            );

            return;
        }

        self.show_columns = !self.show_columns;
    }

    pub fn toggle_permissions_column(&mut self) {
        self.show_permissions = !self.show_permissions;
    }

    pub fn toggle_size_column(&mut self) {
        self.show_size = !self.show_size;
    }

    pub fn toggle_date_column(&mut self) {
        self.show_date = !self.show_date;
    }

    pub fn toggle_user_column(&mut self) {
        self.show_user = !self.show_user;
    }

    pub fn toggle_hidden(&mut self) {
        if self.hidden_only {
            self.show_info_message(
                "Hidden-entry toggle is unavailable while Hidden Only is active",
            );

            return;
        }

        /*
         * Preserve the visible Tree state before changing the hidden-entry policy.
         *
         * Alt+H changes which entries are visible; it must not be treated as a new
         * Tree session or collapse branches that the user explicitly opened.
         */
        let selected_path = self.selected_entry().map(|entry| entry.path.clone());

        let saved_list_offset = self.list_offset;

        /*
         * Preserve the selector's visual row rather than relying only on the absolute
         * list offset.
         *
         * Enabling hidden entries can start a replacement recursive scan, causing the
         * same selected path to move to a different absolute result position.
         */
        let saved_viewport_row = self.selected.saturating_sub(self.list_offset);

        self.pending_visibility_viewport_row = Some(saved_viewport_row);

        let saved_expanded_directories = self.expanded_directories.clone();

        let saved_recursive_expanded_directories = self.recursive_expanded_directories.clone();

        /*
         * Queryless Recursive Tree normally uses a lazy hierarchy map.
         *
         * If Alt+E has explicitly placed it into complete expansion, visibility changes
         * must rebuild the complete map rather than silently falling back to the normal
         * one-level look-ahead representation.
         */
        let preserve_recursive_expand_all = self.view_mode == ViewMode::Tree
            && self.recursive_mode
            && !self.effective_query_is_active()
            && self.recursive_cache_complete
            && !self.recursive_entries.is_empty()
            && {
                let expandable = self.indexed_ordinary_expandable_directories();

                !expandable.is_empty()
                    && expandable
                        .iter()
                        .all(|path| saved_recursive_expanded_directories.contains(path))
            };

        let saved_search_collapsed_directories = self.search_collapsed_directories.clone();

        /*
         * If the ordinary Tree currently represents Expand All, remember exactly which
         * directories belong to that fully expanded visible hierarchy before changing
         * the hidden-entry policy.
         *
         * Newly revealed hidden branches must not inherit Expand All automatically.
         */
        let previous_ordinary_expandable_directories = if self.view_mode == ViewMode::Tree
            && !self.recursive_search_active()
            && self.recursive_cache_complete
            && !self.recursive_entries.is_empty()
        {
            self.indexed_ordinary_expandable_directories()
        } else {
            HashSet::new()
        };

        let preserve_ordinary_expand_all = !previous_ordinary_expandable_directories.is_empty()
            && previous_ordinary_expandable_directories
                .iter()
                .all(|path| saved_expanded_directories.contains(path));

        self.show_hidden = !self.show_hidden;

        self.directory_has_content_cache.clear();

        if self.source.is_remote() && self.remote_index_loaded {
            /*
             * A loaded remote index already owns the recursive corpus.
             *
             * Hidden filtering is performed by the search worker. Do not discard the
             * index or its Tree branch state merely because visibility changed.
             */
            self.cancel_fuzzy_filter();

            if self.show_hidden && !self.remote_index_includes_hidden {
                self.show_error_message(
                    "This remote index contains standard entries only; \
                 rebuild it to include dot-entries",
                );
            }
        } else if self.show_hidden {
            /*
             * A local recursive scan created while hidden entries were disabled does
             * not contain hidden descendants. Rebuild that corpus only when hidden
             * entries are being enabled, while retaining the user's Tree expansion
             * state across the invalidation.
             *
             * Disabling hidden entries needs no new scan because an inclusive corpus
             * can simply be filtered.
             */
            self.invalidate_recursive_cache();

            self.expanded_directories = saved_expanded_directories.clone();

            self.recursive_expanded_directories = saved_recursive_expanded_directories.clone();

            self.search_collapsed_directories = saved_search_collapsed_directories.clone();

            /*
             * invalidate_recursive_cache() deliberately clears transient rebuild state.
             *
             * Re-arm this one request only after the old corpus has been discarded so
             * scan completion knows to reconstruct the previous full non-hidden
             * expansion rather than the normal lazy queryless Tree.
             */
            self.pending_recursive_visibility_expand_restore = preserve_recursive_expand_all;

            /*
             * Ordinary and queryless Recursive Tree transitions cannot know their truthful
             * post-Hidden visible-row count until this replacement corpus is complete.
             */
            self.pending_hidden_tree_limit_check =
                self.view_mode == ViewMode::Tree && !self.effective_query_is_active();
        }

        /*
         * Keep the path alive through any asynchronous recursive scan or search worker
         * started by the hidden-entry toggle.
         */
        self.pending_selection_path = selected_path.clone();

        match self.view_mode {
            ViewMode::Tree if self.recursive_search_active() => {
                /*
                 * Restore the branch-state sets after any local cache invalidation.
                 *
                 * The asynchronous scan or worker completion will then rebuild the
                 * hierarchy using the same expanded and collapsed directories.
                 */
                self.expanded_directories = saved_expanded_directories;

                self.recursive_expanded_directories = saved_recursive_expanded_directories;

                self.search_collapsed_directories = saved_search_collapsed_directories;

                /*
                 * Preserve an explicitly Alt+E-expanded queryless Recursive Tree across a
                 * Hidden visibility change without promoting newly introduced hidden branches
                 * into the expanded set.
                 *
                 * Ordinary lazy Tree rebuilding remains in use for every other state.
                 */
                if preserve_recursive_expand_all
                    && self.recursive_cache_complete
                    && !self.scan_in_progress
                {
                    self.prepare_complete_queryless_recursive_tree();

                    let currently_expandable = self.indexed_ordinary_expandable_directories();

                    self.recursive_expanded_directories
                        .retain(|path| currently_expandable.contains(path));

                    self.rebuild_recursive_search_rows(self.pending_selection_path.clone());
                } else {
                    /*
                     * The replacement Tree may be asynchronous. Its selected path and viewport
                     * row are restored through the pending visibility state rather than by applying
                     * an absolute offset to an incomplete hierarchy.
                     */
                    self.refresh_active_recursive_tree(self.pending_selection_path.clone());
                }
            }

            ViewMode::Tree => {
                /*
                 * Preserve semantic Expand All across a visibility change without expanding
                 * branches that have only just become visible.
                 *
                 * Existing visible branches remain open. Newly revealed hidden directories
                 * begin collapsed and can later be expanded explicitly with Alt+E.
                 */
                if preserve_ordinary_expand_all {
                    self.expanded_directories = previous_ordinary_expandable_directories;
                }

                self.rebuild_tree_rows(selected_path);

                /*
                 * Reconstruct the provisional viewport from the selected path's current
                 * position. The same operation will run again when replacement scan/search
                 * results arrive.
                 */
                if self.pending_selection_path.is_some() {
                    self.restore_pending_selection_if_available();
                } else {
                    self.list_offset = saved_list_offset;
                }

                self.ensure_selection_visible(self.viewport_rows);
            }

            ViewMode::List => {
                if self.recursive_search_active() && self.effective_query_is_active() {
                    /*
                     * Recursive search results may be replaced asynchronously by a new scan
                     * followed by an Exact or Fuzzy worker.
                     *
                     * Do not restore the old numeric selection or list offset against that
                     * temporary result set. pending_selection_path and
                     * pending_visibility_viewport_row will restore the selected path at its
                     * original visual row whenever the replacement result contains it.
                     */
                    self.ensure_recursive_scan();

                    if !self.scan_in_progress {
                        match self.search_mode {
                            SearchMode::Exact => {
                                self.start_current_exact_filter();
                            }

                            SearchMode::Fuzzy => {
                                self.start_current_fuzzy_filter();
                            }
                        }
                    }
                } else {
                    /*
                     * Ordinary current-directory filtering is synchronous, so its selection
                     * and absolute offset can be restored immediately.
                     */
                    self.refresh_filter();

                    if let Some(path) = selected_path {
                        self.select_visible_path(&path);
                    }

                    self.list_offset = saved_list_offset;

                    self.ensure_selection_visible(self.viewport_rows);
                }
            }
        }
    }

    pub fn toggle_hidden_only(&mut self) {
        let selected_path = self.selected_entry().map(|entry| entry.path.clone());

        let saved_viewport_row = self.selected.saturating_sub(self.list_offset);

        self.hidden_only = !self.hidden_only;

        self.directory_has_content_cache.clear();

        self.pending_selection_path = selected_path;

        self.pending_visibility_viewport_row = Some(saved_viewport_row);

        /*
         * Local recursive corpora depend on the active visibility policy.
         *
         * Hidden Only requires its own scan because ordinary-only and
         * hidden-inclusive corpora do not represent the same result domain.
         *
         * Ordinary Tree Alt+E also consumes this recursive corpus even while
         * Recursive mode itself is disabled, so a visibility transition must
         * invalidate any resident local corpus regardless of the current
         * Recursive-mode state.
         */
        if !self.source.is_remote() {
            self.invalidate_recursive_cache();
        }

        match self.view_mode {
            ViewMode::List => {
                if self.recursive_search_active() && self.effective_query_is_active() {
                    self.ensure_recursive_scan();

                    if !self.scan_in_progress {
                        match self.search_mode {
                            SearchMode::Exact => {
                                self.start_current_exact_filter();
                            }

                            SearchMode::Fuzzy => {
                                self.start_current_fuzzy_filter();
                            }
                        }
                    }
                } else {
                    self.refresh_filter();
                }
            }

            ViewMode::Tree if self.recursive_search_active() => {
                self.refresh_active_recursive_tree(self.pending_selection_path.clone());
            }

            ViewMode::Tree => {
                self.rebuild_tree_rows(self.pending_selection_path.clone());
            }
        }

        self.restore_pending_selection_if_available();

        self.ensure_selection_visible(self.viewport_rows);

        self.show_info_message(if self.hidden_only {
            "Hidden Only enabled"
        } else {
            "Hidden Only disabled"
        });
    }

    pub fn toggle_search_mode(&mut self) {
        /*
         * Changing the search interpretation creates a new active search state.
         * An older suspended-search bookmark must not later overwrite it.
         */
        self.search_return_state = None;

        self.search_navigation_active = false;

        /*
         * Capture the genuine selection belonging to the mode being left.
         */
        let current_selected_path = self.selected_entry().map(|entry| entry.path.clone());

        let current_viewport_row = self.selected.saturating_sub(self.list_offset);

        let previous_search_mode = self.search_mode;

        /*
         * Remember the last real Fuzzy selection before leaving Fuzzy mode.
         *
         * Do not replace an existing bookmark when Fuzzy currently has no result.
         */
        if previous_search_mode == SearchMode::Fuzzy
            && let Some(path) = current_selected_path.clone()
        {
            self.fuzzy_selection_state = Some(SearchModeSelectionState {
                selected_path: path,

                viewport_row: current_viewport_row,
            });
        }

        self.search_mode = match previous_search_mode {
            SearchMode::Exact => SearchMode::Fuzzy,

            SearchMode::Fuzzy => SearchMode::Exact,
        };

        self.disable_reverse_for_fuzzy_list();

        /*
         * Prefer the selection visible in the mode being left.
         *
         * If Exact has no result, fall back to the previous genuine Fuzzy bookmark
         * rather than allowing the new Fuzzy result to begin at absolute position zero.
         */
        let restoration_state = if let Some(path) = current_selected_path {
            Some(SearchModeSelectionState {
                selected_path: path,

                viewport_row: current_viewport_row,
            })
        } else if self.search_mode == SearchMode::Fuzzy {
            self.fuzzy_selection_state.clone()
        } else {
            None
        };

        let selected_path = restoration_state
            .as_ref()
            .map(|state| state.selected_path.clone());

        self.pending_selection_path = selected_path.clone();

        self.pending_list_viewport_row = None;

        self.pending_tree_viewport_row = None;

        if let Some(state) = restoration_state.as_ref() {
            match self.view_mode {
                ViewMode::List => {
                    self.pending_list_viewport_row = Some(state.viewport_row);
                }

                ViewMode::Tree => {
                    self.pending_tree_viewport_row = Some(state.viewport_row);
                }
            }
        }

        match self.view_mode {
            ViewMode::List => {
                self.refresh_filter();

                self.restore_pending_selection_if_available();
            }

            ViewMode::Tree => {
                if self.recursive_search_active() {
                    /*
                     * A query-driven recursive Tree must always be rebuilt from worker
                     * results.
                     *
                     * rebuild_recursive_search_tree() owns only the ordinary queryless
                     * hierarchy. Calling it with an active query clears the derived Tree map
                     * and leaves the interface with zero visible rows.
                     */
                    if self.effective_query_is_active() {
                        match self.search_mode {
                            SearchMode::Exact => {
                                self.start_current_exact_filter();
                            }

                            SearchMode::Fuzzy => {
                                self.start_current_fuzzy_filter();
                            }
                        }
                    } else {
                        self.cancel_fuzzy_filter();

                        self.rebuild_recursive_search_tree(selected_path.clone());

                        self.restore_pending_selection_if_available();
                    }
                } else {
                    self.refresh_tree_filter();

                    if let Some(path) = selected_path {
                        self.select_visible_path(&path);
                    }
                }
            }
        }
    }

    /*
     * Enable recursive mode through the correct source-specific startup route.
     *
     * Local sources can enter recursive mode immediately.
     *
     * Remote sources must first inspect and load their persistent index. A valid
     * index begins loading asynchronously; a missing or invalid index opens the
     * normal setup dialog.
     *
     * This method is safe for configuration startup, command-line startup, and
     * interactive activation. It never disables an already-enabled mode.
     */
    pub fn request_recursive_mode(&mut self) {
        if self.recursive_mode {
            return;
        }

        if !self.source.supports_recursive_scan() {
            self.show_error_message("Recursive mode is not available for the current source");

            return;
        }

        if self.source.is_remote() && !self.prepare_remote_recursive_mode() {
            /*
             * false means that preparation has started an asynchronous index load,
             * opened the setup dialog, or reported a preparation error.
             *
             * process_remote_index_load_messages() enables recursive mode after a
             * successful load.
             */
            return;
        }

        self.enable_recursive_mode();
    }

    pub fn enable_recursive_mode(&mut self) {
        if self.recursive_mode {
            return;
        }

        /*
         * Changing recursive scope must not throw away the user's position.
         */
        let selected_path = self.selected_entry().map(|entry| entry.path.clone());

        /*
         * Preserve the selected entry's current screen row while the first Recursive
         * Tree representation is built.
         *
         * The selected path itself is already carried across the transition; retaining
         * its viewport row prevents the initial rebuild from pushing it to the top of
         * the listing.
         */
        let selected_viewport_row = self.selected.saturating_sub(self.list_offset);

        /*
         * Queryless Tree expansion describes the same directory branches before and
         * after Recursive mode is enabled.
         *
         * Carry the currently open ordinary branches into the recursive Tree
         * representation instead of implicitly collapsing the hierarchy.
         */
        let ordinary_expanded_directories =
            if self.view_mode == ViewMode::Tree && !self.effective_query_is_active() {
                Some(self.expanded_directories.clone())
            } else {
                None
            };

        self.pending_selection_path = selected_path.clone();

        if self.view_mode == ViewMode::Tree {
            self.pending_tree_viewport_row = Some(selected_viewport_row);
        }

        self.search_return_state = None;

        self.search_navigation_active = false;

        self.recursive_mode = true;

        if let Some(expanded_directories) = ordinary_expanded_directories {
            self.recursive_expanded_directories = expanded_directories;
        }

        self.error_message = None;

        self.ensure_recursive_scan();

        match self.view_mode {
            ViewMode::List => {
                /*
                 * With an empty query, active_entries() still refers to the current
                 * directory. With search text, the recursive result set becomes
                 * authoritative once scanning and filtering complete.
                 */
                self.refresh_filter();

                self.restore_pending_selection_if_available();
            }

            ViewMode::Tree => {
                if !self.query.is_empty() && self.query != "." {
                    if !self.scan_in_progress {
                        match self.search_mode {
                            SearchMode::Exact => {
                                self.start_current_exact_filter();
                            }

                            SearchMode::Fuzzy => {
                                self.start_current_fuzzy_filter();
                            }
                        }
                    }
                } else {
                    self.refresh_active_recursive_tree(selected_path.clone());
                }
            }
        }
    }

    fn prepare_remote_recursive_mode(&mut self) -> bool {
        /*
         * Recursive mode may be requested while the automatic background loader
         * is already running.
         *
         * Upgrade the existing operation rather than starting a duplicate load.
         * Completion will then enable Recursive mode normally.
         */
        if self.remote_index_load_in_progress {
            /*
             * Upgrade what happens after completion, but retain the existing
             * pulsating loader notification.
             *
             * No second operation has begun, so replacing the active message with a
             * steady "still loading" message would misrepresent the same load.
             */
            self.remote_index_load_purpose = Some(RemoteIndexLoadPurpose::EnableRecursive);

            return false;
        }

        /*
         * Once installed in memory, the host-wide corpus is immediately reusable.
         */
        if self.remote_index_loaded {
            return true;
        }

        self.prepare_remote_index_load(RemoteIndexLoadPurpose::EnableRecursive, true)
    }

    /*
     * Shared remote-index preparation for background connection loading and
     * explicit Recursive-mode requests.
     *
     * show_setup_dialog:
     *
     * - true:  missing or invalid indexes open the normal builder dialog;
     * - false: connection setup remains non-intrusive and ordinary browsing
     *          continues without a resident index.
     */
    fn prepare_remote_index_load(
        &mut self,
        purpose: RemoteIndexLoadPurpose,
        show_setup_dialog: bool,
    ) -> bool {
        if self.remote_index_build_in_progress {
            if purpose == RemoteIndexLoadPurpose::EnableRecursive {
                self.show_info_message(format!(
                    "Remote index is still building — {} entries written",
                    self.remote_index_entries_written,
                ));
            }

            return false;
        }

        if self.remote_index_load_in_progress {
            if purpose == RemoteIndexLoadPurpose::EnableRecursive {
                /*
                 * Promote the active background load without replacing its pulsating
                 * status message.
                 */
                self.remote_index_load_purpose = Some(RemoteIndexLoadPurpose::EnableRecursive);
            }

            return false;
        }

        if self.remote_index_loaded {
            return true;
        }

        let Some(identity) = self.source.remote_index_identity() else {
            return true;
        };

        let mut status = match identity.inspect() {
            Ok(status) => status,

            Err(error) => {
                if purpose == RemoteIndexLoadPurpose::EnableRecursive {
                    self.show_error_message(format!(
                        "Unable to inspect the remote index for {}: {}",
                        identity.display_label(),
                        error,
                    ));
                }

                return false;
            }
        };

        /*
         * Compatibility with indexes created through an OpenSSH alias where the
         * username was omitted from Scry's command.
         *
         * Prefer the exact identity, but reuse a valid legacy default-user index
         * when it represents the same host and port.
         */
        if matches!(status, crate::remote_index::RemoteIndexStatus::Missing)
            && identity.user.is_some()
        {
            let legacy_identity =
                RemoteIndexIdentity::new(identity.host.clone(), None, identity.port);

            match legacy_identity.inspect() {
                Ok(crate::remote_index::RemoteIndexStatus::Valid(info)) => {
                    status = crate::remote_index::RemoteIndexStatus::Valid(info);
                }

                Ok(_) => {}

                Err(error) => {
                    if purpose == RemoteIndexLoadPurpose::EnableRecursive {
                        self.show_error_message(format!(
                            "Unable to inspect the compatible remote index for {}: {}",
                            legacy_identity.display_label(),
                            error,
                        ));
                    }

                    return false;
                }
            }
        }

        match status {
            crate::remote_index::RemoteIndexStatus::Missing => {
                if show_setup_dialog {
                    self.remote_index_setup = Some(RemoteIndexSetupState {
                        identity,

                        purpose: RemoteIndexDialogPurpose::InitialSetup,

                        includes_hidden: false,

                        focus: RemoteIndexDialogFocus::Policy,

                        invalid_reason: None,
                    });

                    self.overlay = Overlay::RemoteIndexSetup;
                }

                false
            }

            crate::remote_index::RemoteIndexStatus::Invalid { reason, .. } => {
                if show_setup_dialog {
                    self.remote_index_setup = Some(RemoteIndexSetupState {
                        identity,

                        purpose: RemoteIndexDialogPurpose::InitialSetup,

                        includes_hidden: false,

                        focus: RemoteIndexDialogFocus::Policy,

                        invalid_reason: Some(reason),
                    });

                    self.overlay = Overlay::RemoteIndexSetup;
                }

                false
            }

            crate::remote_index::RemoteIndexStatus::Valid(info) => {
                self.begin_remote_index_load(info.identity, purpose);

                false
            }
        }
    }

    fn begin_remote_index_load(
        &mut self,
        identity: RemoteIndexIdentity,
        purpose: RemoteIndexLoadPurpose,
    ) {
        if self.remote_index_load_in_progress {
            /*
             * An explicit Recursive request takes priority over a background load.
             */
            if purpose == RemoteIndexLoadPurpose::EnableRecursive {
                self.remote_index_load_purpose = Some(RemoteIndexLoadPurpose::EnableRecursive);
            }

            return;
        }

        let (sender, receiver) = mpsc::channel();

        thread::spawn(move || {
            /*
             * Do not report completion after decoding alone.
             *
             * The notification must keep pulsating until every structure required by
             * Scry's resident corpus has been prepared.
             */
            let result = prepare_remote_index(&identity);

            let _ = sender.send(RemoteIndexLoadResult { result });
        });

        self.remote_index_load_receiver = Some(receiver);

        self.remote_index_load_in_progress = true;

        self.remote_index_load_purpose = Some(purpose);

        self.show_pulsating_persistent_info_message(
            "Loading persistent remote index, please wait...",
        );
    }

    pub fn toggle_recursive_mode(&mut self) {
        /*
         * Some future filesystem sources may support ordinary browsing without
         * supporting recursive traversal.
         */
        if !self.source.supports_recursive_scan() {
            /*
             * A source that cannot scan recursively must never remain in recursive
             * mode. Normally connection installation already enforces this, but this
             * recovery keeps startup flags and future source transitions safe.
             */
            if self.recursive_mode {

                self.recursive_mode = false;

                self.invalidate_recursive_cache();

                self.selected = 0;

                self.list_offset = 0;

                match self.view_mode {
                    ViewMode::List => {
                        self.refresh_filter();
                    }

                    ViewMode::Tree => {
                        self.reset_tree();
                    }
                }
            }

            self.error_message =
                Some("Recursive mode is not available for the current source".to_string());

            return;
        }

        if !self.recursive_mode {
            self.request_recursive_mode();

            return;
        }

        /*
         * Disable recursive scope while preserving the query and search style.
         *
         * Exact and Fuzzy searches immediately return to the entries loaded from the
         * current directory. The completed recursive cache is retained so Alt+R can
         * restore recursive results without rescanning the filesystem.
         */
        let selected_path = self.selected_entry().map(|entry| entry.path.clone());

        self.pending_selection_path = selected_path.clone();

        self.search_return_state = None;

        self.search_navigation_active = false;

        self.recursive_mode = false;

        self.error_message = None;

        self.pending_recursive_search_at = None;

        self.cancel_fuzzy_filter();

        match self.view_mode {
            ViewMode::List => {
                self.selected = 0;

                self.list_offset = 0;

                self.refresh_filter();

                self.restore_pending_selection_if_available();
            }

            ViewMode::Tree => {
                if self.effective_query_is_active() {
                    /*
                     * Queried Trees use their own open-by-default branch model.
                     *
                     * search_tree_children contains the represented result hierarchy, while
                     * search_collapsed_directories records only branches the user explicitly
                     * closed. Disabling Recursive scope must not throw that state away and
                     * fall immediately into the ordinary closed-by-default Tree model.
                     *
                     * Keep the currently represented queried Tree intact here. The next query
                     * evaluation will naturally rebuild it for non-recursive scope through the
                     * normal Exact/Fuzzy path.
                     */
                    self.rebuild_recursive_search_rows(selected_path.clone());

                    if let Some(path) = selected_path {
                        self.select_visible_path(&path);
                    }
                } else {
                    /*
                     * Queryless Recursive Tree and ordinary Tree use different expansion
                     * models. The ordinary backing hierarchy remained resident while
                     * Recursive mode was active, so return to it directly without reset_tree().
                     */
                    self.rebuild_tree_rows(selected_path.clone());

                    self.refresh_tree_filter();

                    if let Some(path) = selected_path {
                        self.select_visible_path(&path);
                    }
                }
            }
        }

        self.ensure_selection_visible(self.viewport_rows);
    }

    pub fn toggle_tree_mode(&mut self) {
        match self.view_mode {
            ViewMode::List => {
                /*
                 * Save the current List selection before changing which backing collection
                 * selected_entry() reads from.
                 */
                let list_selected_path = self.selected_entry().map(|entry| entry.path.clone());

                /*
                 * Record the selector's present screen row in List mode.
                 *
                 * selected is the absolute result position, while list_offset is the first
                 * visible result. Their difference is therefore the selector's row inside
                 * the viewport.
                 */
                let source_viewport_row = self.selected.saturating_sub(self.list_offset);

                self.list_selection_viewport_row = source_viewport_row;

                self.list_view_state = Some(NavigationState {
                    selected_path: list_selected_path.clone(),

                    list_offset: self.list_offset,
                });

                /*
                 * A queried Tree may contain contextual directories that are not themselves
                 * direct List results.
                 *
                 * Tree -> List therefore sometimes maps the selected Tree path to the first
                 * actual matching List result beneath it. Compare against that mapped List
                 * path rather than requiring the original Tree and List paths to be identical.
                 *
                 * If the mapped selection is still active, this is a genuine return to the
                 * same Tree position and its remembered viewport row should be restored.
                 * If the user moved to another List result, carry that result's current List
                 * viewport row into Tree instead.
                 */
                let returning_to_same_tree_selection = self
                    .tree_view_state
                    .as_ref()
                    .and_then(|state| state.selected_path.as_deref())
                    .and_then(|path| self.list_path_for_tree_selection(path))
                    .as_ref()
                    == list_selected_path.as_ref();

                let desired_tree_viewport_row = if returning_to_same_tree_selection {
                    self.tree_selection_viewport_row
                } else {
                    source_viewport_row
                };

                let desired_tree_state = NavigationState {
                    selected_path: list_selected_path,

                    list_offset: 0,
                };

                /*
                 * Keep both the selected path and its intended Tree screen row alive through
                 * any asynchronous Exact or Fuzzy Tree rebuild.
                 */
                self.pending_selection_path = desired_tree_state.selected_path.clone();

                self.pending_tree_viewport_row = Some(desired_tree_viewport_row);

                self.view_mode = ViewMode::Tree;

                self.selected = 0;

                self.list_offset = 0;

                /*
                 * When returning to a previously visited Tree, its old rows are still resident.
                 *
                 * Restore the saved Tree selection immediately before an asynchronous recursive
                 * worker is started. Otherwise the temporary numeric selection of zero causes
                 * ensure_selection_visible() to discard the saved viewport offset while the new
                 * result is still being calculated.
                 *
                 * The worker continues to receive the saved path below and will reapply it to
                 * the rebuilt Tree when its result arrives.
                 */
                if let Some(path) = desired_tree_state.selected_path.as_ref() {
                    self.select_visible_path(path);
                }

                if self.recursive_mode {
                    /*
                     * A queried recursive Tree belongs to the Exact/Fuzzy worker and must
                     * follow its normal rebuild route.
                     *
                     * A queryless recursive Tree, however, can be retained across Ctrl+T.
                     * Rebuild it only when its root, visibility, filter, sort, or recursive
                     * corpus has changed while List mode was active.
                     */
                    if self.effective_query_is_active()
                        || !self.retained_queryless_recursive_tree_is_current()
                    {
                        self.refresh_active_recursive_tree(
                            desired_tree_state.selected_path.clone(),
                        );
                    } else {
                        /*
                         * search_tree_children, branch expansion state, and tree_rows still
                         * describe the current queryless recursive Tree.
                         *
                         * Reuse them directly rather than cloning and sorting the complete
                         * recursive corpus again merely because the representation changed.
                         */
                        self.restore_pending_selection_if_available();
                    }
                } else {
                    /*
                     * Do not call reset_tree() when returning to an existing ordinary
                     * Tree. That would erase expanded_directories and collapse every
                     * branch.
                     */
                    if self.tree_children.is_empty() {
                        self.reset_tree();
                    } else {
                        self.rebuild_tree_rows(desired_tree_state.selected_path.clone());

                        /*
                         * A retained ordinary Tree may have been built while another entry filter
                         * was active. Re-publish its visible rows through the current policy before
                         * restoring selection.
                         */
                        self.refresh_tree_filter();
                    }

                    if let Some(path) = desired_tree_state.selected_path {
                        self.select_visible_path(&path);
                    }
                }

                /*
                 * The previous Tree snapshot may already contain the carried path.
                 *
                 * Use it for immediate visual placement, but do not consume the pending
                 * selection here. A new Exact or Fuzzy worker may still replace the complete
                 * Tree, and the path must remain pinned through every progressive snapshot
                 * and the final result.
                 *
                 * restore_pending_selection_if_available() is the sole owner of clearing the
                 * pending path after the destination result is stable.
                 */
                if self.pending_selection_path.as_ref().is_some_and(|path| {
                    self.selected_entry()
                        .is_some_and(|entry| &entry.path == path)
                }) {
                    self.list_offset = self.selected.saturating_sub(desired_tree_viewport_row);
                }

                self.ensure_selection_visible(self.viewport_rows);
            }

            ViewMode::Tree => {
                let tree_selected_path = self.selected_entry().map(|entry| entry.path.clone());

                let source_viewport_row = self.selected.saturating_sub(self.list_offset);

                self.tree_selection_viewport_row = source_viewport_row;

                self.tree_view_state = Some(NavigationState {
                    selected_path: tree_selected_path.clone(),

                    list_offset: self.list_offset,
                });

                self.view_mode = ViewMode::List;

                self.disable_reverse_for_fuzzy_list();

                self.selected = 0;

                self.list_offset = 0;

                /*
                  * A queried List may be rebuilt asynchronously.
                  *
                  * Its filtered_indices still belong to the previous List result until the
                  * destination worker publishes, so do not map the Tree selection through those
                  * indices here. Carry the actual Tree path into the destination instead.
                  *
                  * Queryless List mode is synchronous and may perform its ordinary ancestor
                  * mapping immediately.
                  */
                let desired_list_path = if self.effective_query_is_active() {
                    tree_selected_path.clone()
                } else {
                    tree_selected_path
                        .as_deref()
                        .and_then(|path| self.list_path_for_tree_selection(path))
                };

                let desired_list_state = NavigationState {
                    selected_path: desired_list_path,

                    list_offset: 0,
                };

                self.pending_selection_path = desired_list_state.selected_path.clone();

                self.pending_list_viewport_row = Some(source_viewport_row);

                self.refresh_filter();

                if let Some(path) = desired_list_state.selected_path.as_ref() {
                    self.select_visible_path(path);
                }

                if desired_list_state
                    .selected_path
                    .as_ref()
                    .is_some_and(|path| {
                        self.selected_entry()
                            .is_some_and(|entry| &entry.path == path)
                    })
                {
                    self.list_offset = self.selected.saturating_sub(source_viewport_row);
                }

                self.ensure_selection_visible(self.viewport_rows);
            }
        }
    }

    fn prepare_ordinary_tree_from_recursive_corpus(&mut self) {
        /*
         * Build one authoritative path map first.
         *
         * `entries` guarantees that the current root's immediate children exist
         * even if a scanner or persistent index omitted or reordered them.
         * `recursive_entries` then supplies every known descendant.
         */
        let mut entries_by_path: HashMap<PathBuf, FileEntry> = HashMap::new();

        for entry in self.entries.iter().chain(self.recursive_entries.iter()) {
            /*
             * A persistent remote index may contain entries outside the current
             * ordinary Tree root. Those paths must not enter this hierarchy.
             */
            if entry.path.parent() != Some(self.current_directory.as_path())
                && !entry.path.starts_with(&self.current_directory)
            {
                continue;
            }

            entries_by_path.insert(entry.path.clone(), entry.clone());
        }

        self.tree_children.clear();

        for entry in entries_by_path.into_values() {
            let Some(parent) = entry.path.parent() else {
                continue;
            };

            self.tree_children
                .entry(parent.to_path_buf())
                .or_default()
                .push(entry);
        }

        for children in self.tree_children.values_mut() {
            sort_entries(children, self.sort_mode, self.sort_descending);
        }
    }

    fn populate_ordinary_tree_from_recursive_corpus(
        &mut self,
        preferred_selection: Option<PathBuf>,
    ) {
        self.prepare_ordinary_tree_from_recursive_corpus();

        /*
         * Every directory represented as a parent with real children is
         * expandable. The current root itself is not a visible Tree row.
         */
        self.expanded_directories = self.ordinary_expandable_directories();

        self.rebuild_tree_rows(preferred_selection);

        /*
         * A session-restored Expand All Tree may have carried both a selected path and
         * its original visual viewport row through the asynchronous recursive scan.
         *
         * Now that the complete hierarchy is stable, restore that viewport placement
         * before applying the ordinary visibility clamp.
         */
        self.restore_pending_selection_if_available();

        self.ensure_selection_visible(self.viewport_rows);
    }

    fn toggle_refused_tree_expand_state(
        &mut self,
        target: TreeExpandAllTarget,
        selected_path: Option<PathBuf>,
    ) -> bool {
        let Some(state) = self.refused_tree_expand_state.clone() else {
            return false;
        };

        match (target, state) {
            (
                TreeExpandAllTarget::Ordinary,
                RefusedTreeExpandState::Ordinary {
                    root_directory,
                    show_hidden,
                    hidden_only,
                    entry_filter,
                    expanded_directories,
                },
            ) if root_directory == self.current_directory
                && show_hidden == self.show_hidden
                && hidden_only == self.hidden_only
                && entry_filter == self.entry_filter =>
            {
                if self.expanded_directories == expanded_directories
                    && !expanded_directories.is_empty()
                {
                    /*
                     * Safe partial expansion -> Collapse All.
                     */
                    self.expanded_directories.clear();
                } else if self.expanded_directories.is_empty() && !expanded_directories.is_empty() {
                    /*
                     * Collapse All -> remembered safe partial expansion.
                     */
                    self.expanded_directories = expanded_directories;
                } else {
                    /*
                     * Manual branch edits changed the remembered state.
                     *
                     * Let ordinary Alt+E policy evaluate the Tree again rather than
                     * forcing an obsolete snapshot onto it.
                     */
                    self.refused_tree_expand_state = None;

                    return false;
                }

                self.rebuild_tree_rows(selected_path);

                self.ensure_selection_visible(self.viewport_rows);

                true
            }

            (
                TreeExpandAllTarget::RecursiveQueryless,
                RefusedTreeExpandState::RecursiveQueryless {
                    root_directory,
                    expanded_directories,
                },
            ) if root_directory == self.current_directory => {
                if self.recursive_expanded_directories == expanded_directories
                    && !expanded_directories.is_empty()
                {
                    self.recursive_expanded_directories.clear();
                } else if self.recursive_expanded_directories.is_empty()
                    && !expanded_directories.is_empty()
                {
                    self.recursive_expanded_directories = expanded_directories;
                } else {
                    self.refused_tree_expand_state = None;

                    return false;
                }

                self.rebuild_recursive_search_rows(selected_path);

                self.ensure_selection_visible(self.viewport_rows);

                true
            }

            (
                TreeExpandAllTarget::RecursiveSearch,
                RefusedTreeExpandState::RecursiveSearch {
                    root_directory,
                    collapsed_directories,
                },
            ) if root_directory == self.current_directory => {
                let all_collapsed = self.recursive_expandable_directories();

                if self.search_collapsed_directories == collapsed_directories {
                    /*
                     * Safe partial search Tree -> Collapse All.
                     */
                    self.search_collapsed_directories = all_collapsed;
                } else if self.search_collapsed_directories == all_collapsed {
                    /*
                     * Collapse All -> remembered safe partial search Tree.
                     */
                    self.search_collapsed_directories = collapsed_directories;
                } else {
                    self.refused_tree_expand_state = None;

                    return false;
                }

                self.rebuild_recursive_search_rows(selected_path);

                self.ensure_selection_visible(self.viewport_rows);

                true
            }

            _ => {
                /*
                 * Root, mode, visibility policy, or Tree kind changed.
                 *
                 * The old safe snapshot no longer describes this Tree.
                 */
                self.refused_tree_expand_state = None;

                false
            }
        }
    }

    pub fn request_toggle_all_tree_branches(&mut self) {
        if self.view_mode != ViewMode::Tree {
            return;
        }

        if self.tree_expand_all_dialog_visible() {
            return;
        }

        let selected_path = self.selected_entry().map(|entry| entry.path.clone());



        eprintln!(
            "ALT+E STATE: remote={} recursive_active={} query_active={} hidden={} ordinary_bulk={} expanded={} recursive_expanded={} refused_state={} rows={} max={}",
            self.source.is_remote(),
            self.recursive_search_active(),
            self.effective_query_is_active(),
            self.show_hidden,
            self.ordinary_expand_all_active,
            self.expanded_directories.len(),
            self.recursive_expanded_directories.len(),
            self.refused_tree_expand_state.is_some(),
            self.filtered_tree_indices.len(),
            self.advanced_tree_config.max_visible_tree_rows,
        );




        if self.recursive_search_active() {
            if self.effective_query_is_active() {
                if self.toggle_refused_tree_expand_state(
                    TreeExpandAllTarget::RecursiveSearch,
                    selected_path.clone(),
                ) {
                    return;
                }
                /*
                 * Recursive search Trees are expanded by default.
                 *
                 * An empty collapsed set means every represented search branch is
                 * open. The first Alt+E therefore collapses everything immediately.
                 *
                 * A nonempty set means at least one branch is closed, so the action
                 * is Expand All and must pass through the centralized expansion
                 * request path.
                 */
                if self.search_collapsed_directories.is_empty() {
                    self.collapse_all_recursive_search_branches(selected_path);
                } else {
                    self.apply_tree_expand_all(TreeExpandAllTarget::RecursiveSearch, selected_path);
                }

                return;
            }

            if self.toggle_refused_tree_expand_state(
                TreeExpandAllTarget::RecursiveQueryless,
                selected_path.clone(),
            ) {
                return;
            }

            /*
             * A queryless recursive Tree is collapsed by default.
             *
             * Determine whether every represented expandable directory is already
             * open. Collapse All remains immediate; only Expand All enters the
             * centralized expansion path.
             */
            let expandable_directories = self.indexed_ordinary_expandable_directories();

            let all_expanded = !expandable_directories.is_empty()
                && expandable_directories
                    .iter()
                    .all(|path| self.recursive_expanded_directories.contains(path));

            if all_expanded {
                self.recursive_expanded_directories.clear();

                self.rebuild_recursive_search_rows(selected_path);

                self.ensure_selection_visible(self.viewport_rows);
            } else {
                self.apply_tree_expand_all(TreeExpandAllTarget::RecursiveQueryless, selected_path);
            }

            return;
        }

        if self
            .toggle_refused_tree_expand_state(TreeExpandAllTarget::Ordinary, selected_path.clone())
        {
            return;
        }

        /*
         * A complete recursive corpus lets ordinary Alt+E determine its bulk state
         * directly from the resident child index.
         *
         * Do not materialize tree_children merely to decide whether this request is
         * Expand All or Collapse All. Full Tree construction belongs only to the
         * confirmed expansion path.
         */
        let complete_hierarchy_available =
            self.recursive_cache_complete && !self.recursive_entries.is_empty();

        let expandable_directories = if complete_hierarchy_available {
            self.indexed_ordinary_expandable_directories()
        } else {
            self.ordinary_expandable_directories()
        };

        let all_expanded = complete_hierarchy_available
            && !expandable_directories.is_empty()
            && expandable_directories
                .iter()
                .all(|path| self.expanded_directories.contains(path));

        if all_expanded {
            self.expanded_directories.clear();

            self.ordinary_expand_all_active = false;

            self.rebuild_tree_rows(selected_path);

            self.ensure_selection_visible(self.viewport_rows);

            return;
        }

        self.apply_tree_expand_all(TreeExpandAllTarget::Ordinary, selected_path);
    }

    fn projected_tree_expand_all_rows(&mut self, target: TreeExpandAllTarget) -> usize {
        match target {
            TreeExpandAllTarget::RecursiveSearch => Self::count_complete_tree_rows(
                &self.current_directory,
                &self.search_tree_children,
                false,
            ),

            TreeExpandAllTarget::RecursiveQueryless => self.indexed_ordinary_expand_all_row_count(),

            TreeExpandAllTarget::Ordinary => {
                /*
                 * Large-Tree policy needs only the projected visible row count.
                 *
                 * The resident recursive child index already represents the complete
                 * hierarchy, so count directly from it without cloning FileEntry values
                 * into the ordinary Tree map.
                 */
                self.indexed_ordinary_expand_all_row_count()
            }
        }
    }

    pub fn tree_expand_all_dialog_visible(&self) -> bool {
        self.tree_expand_all_dialog.is_some()
    }

    pub fn toggle_tree_expand_all_warning_suppression(&mut self) {
        let Some(dialog) = self.tree_expand_all_dialog.as_mut() else {
            return;
        };

        /*
         * Only the local confirmation offers an interactive suppression checkbox.
         *
         * SSH suppression remains an explicit scry.toml choice, and a refusal is
         * never suppressible.
         */
        if dialog.kind != TreeExpandAllDialogKind::LocalConfirmation {
            return;
        }

        dialog.disable_warning = !dialog.disable_warning;
    }

    pub fn cancel_tree_expand_all_dialog(&mut self) {
        self.tree_expand_all_dialog = None;
    }

    pub fn confirm_tree_expand_all_dialog(&mut self) {
        let Some(dialog) = self.tree_expand_all_dialog.take() else {
            return;
        };

        if matches!(
            dialog.kind,
            TreeExpandAllDialogKind::Refusal | TreeExpandAllDialogKind::DisplayLimit
        ) {
            /*
             * Enter merely acknowledges an informational Tree-policy refusal.
             */
            return;
        }

        if dialog.focus == TreeExpandAllDialogFocus::Cancel {
            return;
        }

        if dialog.kind == TreeExpandAllDialogKind::LocalConfirmation && dialog.disable_warning {
            self.local_expand_all_warning_disabled = true;

            let state = crate::ui_state::UiState {
                disable_local_expand_all_warning: true,
            };

            if let Err(error) = crate::ui_state::save(&state) {
                /*
                 * Expansion may still continue. Failure to save a convenience
                 * preference must not block the requested Tree operation.
                 */
                self.show_error_message(format!(
                    "Unable to save the warning preference: {}",
                    error,
                ));
            }
        }

        self.perform_tree_expand_all(dialog.target, dialog.selected_path);
    }

    pub fn select_tree_expand_all_dialog_focus(&mut self, focus: TreeExpandAllDialogFocus) {
        let Some(dialog) = self.tree_expand_all_dialog.as_mut() else {
            return;
        };

        if dialog.kind == TreeExpandAllDialogKind::Refusal {
            return;
        }

        dialog.focus = focus;
    }

    fn count_complete_tree_rows(
        root: &Path,
        children_by_parent: &HashMap<PathBuf, Vec<FileEntry>>,
        hide_dot_entries: bool,
    ) -> usize {
        /*
         * Count the same hierarchy that a fully expanded Tree would display, but do
         * not allocate TreeRow values or modify expansion state.
         *
         * An explicit stack avoids recursive call depth while traversing unusually
         * deep directory hierarchies.
         */
        let mut row_count = 0_usize;

        let mut pending_directories = vec![root.to_path_buf()];

        while let Some(directory) = pending_directories.pop() {
            let Some(children) = children_by_parent.get(&directory) else {
                continue;
            };

            for entry in children {
                if hide_dot_entries && entry.name.starts_with('.') {
                    continue;
                }

                row_count = row_count.saturating_add(1);

                if entry.is_directory
                    && !entry.is_symlink
                    && children_by_parent
                        .get(&entry.path)
                        .is_some_and(|children| !children.is_empty())
                {
                    pending_directories.push(entry.path.clone());
                }
            }
        }

        row_count
    }

    fn remember_refused_tree_expand_state(&mut self, target: TreeExpandAllTarget) {
        self.refused_tree_expand_state = Some(match target {
            TreeExpandAllTarget::Ordinary => RefusedTreeExpandState::Ordinary {
                root_directory: self.current_directory.clone(),

                show_hidden: self.show_hidden,

                hidden_only: self.hidden_only,

                entry_filter: self.entry_filter,

                expanded_directories: self.expanded_directories.clone(),
            },

            TreeExpandAllTarget::RecursiveQueryless => RefusedTreeExpandState::RecursiveQueryless {
                root_directory: self.current_directory.clone(),

                expanded_directories: self.recursive_expanded_directories.clone(),
            },

            TreeExpandAllTarget::RecursiveSearch => RefusedTreeExpandState::RecursiveSearch {
                root_directory: self.current_directory.clone(),

                collapsed_directories: self.search_collapsed_directories.clone(),
            },
        });
    }

    fn apply_tree_expand_all(
        &mut self,
        target: TreeExpandAllTarget,
        selected_path: Option<PathBuf>,
    ) {
        /*
         * Ordinary and queryless Recursive Expand All both require the complete
         * recursive corpus before their projected hierarchy can be counted or
         * materialized truthfully.
         *
         * Hidden Only may invalidate that corpus immediately before Alt+E is pressed.
         * Queue the request and resume it after the replacement scan finishes instead
         * of treating an empty child index as an empty Tree.
         */
        if matches!(
            target,
            TreeExpandAllTarget::Ordinary | TreeExpandAllTarget::RecursiveQueryless
        ) {
            let complete_hierarchy_available =
                self.recursive_cache_complete && !self.recursive_entries.is_empty();

            if !complete_hierarchy_available {
                /*
                 * Remote Trees obtain their complete corpus from the persistent index.
                 * Alt+E never owns remote-index startup.
                 */
                if self.source.is_remote() {
                    if self.remote_index_load_in_progress {
                        self.show_info_message("Remote index is still loading…");
                    } else {
                        self.show_info_message(
                            "No remote index is loaded; press F5 to build or rebuild it",
                        );
                    }

                    return;
                }

                self.pending_tree_expand_all = Some(PendingTreeExpandAll {
                    target,

                    selected_path: selected_path.clone(),
                });

                self.pending_selection_path = selected_path;

                self.ensure_recursive_scan();

                return;
            }
        }

        let projected_rows = self.projected_tree_expand_all_rows(target);

        let warning_rows = self.advanced_tree_config.expand_all_warning_rows;

        let maximum_rows = self.advanced_tree_config.max_visible_tree_rows;

        /*
         * The maximum is authoritative under the current configuration.
         *
         * It is checked before warning suppression because suppression affects only
         * confirmation dialogs—it never permits expansion above the configured
         * maximum.
         */
        if projected_rows > maximum_rows {
            /*
             * Full expansion is forbidden under the current configured maximum, but
             * Alt+E must remain a useful bulk toggle.
             *
             * Remember the Tree exactly as it safely exists now. Later Alt+E presses can
             * alternate between this partial expansion and Collapse All without trying
             * to construct the prohibited complete hierarchy.
             */
            self.remember_refused_tree_expand_state(target);

            /*
             * Explain the maximum through the full refusal dialog only once per Scry process.
             *
             * Later over-limit attempts receive a lightweight status notification so Alt+E
             * never appears unresponsive while the configured maximum remains authoritative.
             */
            if self.advanced_tree_config.show_max_visible_tree_rows
                && !self.tree_expand_all_refusal_shown_this_session
            {
                self.tree_expand_all_refusal_shown_this_session = true;

                self.tree_expand_all_dialog = Some(TreeExpandAllDialogState {
                    kind: TreeExpandAllDialogKind::Refusal,

                    projected_rows,

                    configured_max_rows: maximum_rows,

                    disable_warning: false,

                    focus: TreeExpandAllDialogFocus::Cancel,

                    display_limit_action: TreeDisplayLimitAction::BranchExpansion,

                    target,

                    selected_path,
                });
            } else {
                self.show_info_message(format!(
                    "Expand All unavailable: {} projected rows exceed the configured maximum of {}",
                    projected_rows, maximum_rows,
                ));
            }

            return;
        }

        /*
         * Values at or below the warning threshold expand immediately.
         */
        if projected_rows <= warning_rows {
            self.perform_tree_expand_all(target, selected_path);

            return;
        }

        let source_is_remote = self.source.is_remote();

        /*
         * The global warning switch suppresses the confirmation dialog only.
         *
         * The first warning-range expansion during each source type's session is still
         * reported through the notification area so a suppressed dialog never makes
         * the policy change appear silent.
         */
        if !self.advanced_tree_config.show_expand_all_warning {
            let warning_already_reported = if source_is_remote {
                self.ssh_expand_all_warning_shown_this_session
            } else {
                self.local_expand_all_warning_shown_this_session
            };

            if !warning_already_reported {
                if source_is_remote {
                    self.ssh_expand_all_warning_shown_this_session = true;
                } else {
                    self.local_expand_all_warning_shown_this_session = true;
                }

                self.show_info_message(format!(
                    "Large Tree warning suppressed: expanding {} projected rows (configured maximum {})",
                    projected_rows, maximum_rows,
                ));
            }

            self.perform_tree_expand_all(target, selected_path);

            return;
        }

        /*
         * Warning-range explanations are provided only once per source type during one
         * Scry session.
         *
         * Normally this is the confirmation dialog. When the global warning dialog is
         * disabled, the first affected expansion is reported through a notification
         * instead. Permanent local suppression and SSH-specific suppression remain
         * independent.
         */
        if !source_is_remote
            && (self.local_expand_all_warning_disabled
                || self.local_expand_all_warning_shown_this_session)
        {
            self.perform_tree_expand_all(target, selected_path);

            return;
        }

        if source_is_remote
            && (!self.advanced_tree_config.show_ssh_expand_all_warning
                || self.ssh_expand_all_warning_shown_this_session)
        {
            self.perform_tree_expand_all(target, selected_path);

            return;
        }

        /*
         * Merely presenting the warning satisfies the once-per-session policy.
         *
         * Cancelling does not cause the same interruption to return repeatedly during
         * the current run.
         */
        if source_is_remote {
            self.ssh_expand_all_warning_shown_this_session = true;
        } else {
            self.local_expand_all_warning_shown_this_session = true;
        }

        self.tree_expand_all_dialog = Some(TreeExpandAllDialogState {
            kind: if source_is_remote {
                TreeExpandAllDialogKind::SshConfirmation
            } else {
                TreeExpandAllDialogKind::LocalConfirmation
            },

            projected_rows,

            configured_max_rows: maximum_rows,

            disable_warning: false,

            display_limit_action: TreeDisplayLimitAction::BranchExpansion,

            /*
             * Confirmation dialogs now use one acknowledgement action.
             *
             * Escape remains available when the user does not want to continue.
             */
            focus: TreeExpandAllDialogFocus::ExpandAll,

            target,

            selected_path,
        });
    }

    fn perform_tree_expand_all(
        &mut self,
        target: TreeExpandAllTarget,
        selected_path: Option<PathBuf>,
    ) {
        match target {
            TreeExpandAllTarget::RecursiveSearch => {
                /*
                 * Search-result Trees are open by default.
                 *
                 * Removing every explicit collapsed-directory exception therefore
                 * expands the complete represented result hierarchy.
                 */
                self.search_collapsed_directories.clear();

                self.rebuild_recursive_search_rows(selected_path);

                self.ensure_selection_visible(self.viewport_rows);
            }

            TreeExpandAllTarget::RecursiveQueryless => {
                /*
                 * Queryless Recursive Tree browsing normally keeps only a lazy root and
                 * one-level look-ahead in search_tree_children.
                 *
                 * Expand All is the explicit exception. The policy gate has already
                 * approved the projected complete row count, so materialize the full
                 * hierarchy from the resident child index before marking every represented
                 * branch expanded.
                 */
                self.prepare_complete_queryless_recursive_tree();

                self.recursive_expanded_directories =
                    self.indexed_ordinary_expandable_directories();

                self.rebuild_recursive_search_rows(selected_path);

                self.ensure_selection_visible(self.viewport_rows);
            }

            TreeExpandAllTarget::Ordinary => {
                /*
                 * The policy gate guarantees that the complete corpus is resident
                 * before this method is entered.
                 */
                self.pending_tree_expand_all = None;

                self.populate_ordinary_tree_from_recursive_corpus(selected_path);

                /*
                 * This is the only route that establishes authoritative ordinary
                 * Expand All state.
                 */
                self.ordinary_expand_all_active = true;
            }
        }
    }

    fn collapse_all_recursive_search_branches(&mut self, selected_path: Option<PathBuf>) {
        self.search_collapsed_directories = self
            .search_tree_children
            .iter()
            .filter_map(|(path, children)| {
                (path != &self.current_directory && !children.is_empty()).then_some(path.clone())
            })
            .collect();

        self.rebuild_recursive_search_rows(selected_path);

        self.ensure_selection_visible(self.viewport_rows);
    }

    fn recursive_expandable_directories(&self) -> HashSet<PathBuf> {
        self.search_tree_children
            .iter()
            .filter_map(|(path, children)| {
                (path != &self.current_directory && !children.is_empty()).then_some(path.clone())
            })
            .collect()
    }

    fn indexed_ordinary_expandable_directories(&self) -> HashSet<PathBuf> {
        let mut directories = HashSet::new();

        for (path, child_indices) in &self.recursive_child_indices {
            if path == &self.current_directory || !path.starts_with(&self.current_directory) {
                continue;
            }

            let has_visible_child = child_indices.iter().any(|index| {
                self.recursive_entries.get(*index).is_some_and(|entry| {
                    entry_matches_visibility(
                        entry,
                        &self.current_directory,
                        self.show_hidden,
                        self.hidden_only,
                    ) && match self.entry_filter {
                        EntryFilter::DirectoriesOnly => entry.is_directory,

                        /*
                         * FilesOnly still needs directories as structural corridors
                         * in Tree mode.
                         */
                        EntryFilter::All | EntryFilter::FilesOnly => true,
                    }
                })
            });

            if has_visible_child {
                directories.insert(path.clone());
            }
        }

        directories
    }

    fn indexed_ordinary_expand_all_row_count(&self) -> usize {
        let mut row_count = 0_usize;

        let mut pending_directories = vec![self.current_directory.clone()];

        while let Some(directory) = pending_directories.pop() {
            let Some(child_indices) = self.recursive_child_indices.get(&directory) else {
                continue;
            };

            for index in child_indices {
                let Some(entry) = self.recursive_entries.get(*index) else {
                    continue;
                };

                if !entry_matches_visibility(
                    entry,
                    &self.current_directory,
                    self.show_hidden,
                    self.hidden_only,
                ) {
                    continue;
                }

                if self.entry_filter == EntryFilter::DirectoriesOnly && !entry.is_directory {
                    continue;
                }

                row_count = row_count.saturating_add(1);

                if entry.is_directory
                    && !entry.is_symlink
                    && self
                        .recursive_child_indices
                        .get(&entry.path)
                        .is_some_and(|children| {
                            children.iter().any(|child_index| {
                                self.recursive_entries
                                    .get(*child_index)
                                    .is_some_and(|child| {
                                        entry_matches_visibility(
                                            child,
                                            &self.current_directory,
                                            self.show_hidden,
                                            self.hidden_only,
                                        ) && match self.entry_filter {
                                            EntryFilter::DirectoriesOnly => child.is_directory,
                                            EntryFilter::All | EntryFilter::FilesOnly => true,
                                        }
                                    })
                            })
                        })
                {
                    pending_directories.push(entry.path.clone());
                }
            }
        }

        row_count
    }

    fn ordinary_expandable_directories(&self) -> HashSet<PathBuf> {
        self.tree_children
            .iter()
            .filter_map(|(path, children)| {
                (path != &self.current_directory && !children.is_empty()).then_some(path.clone())
            })
            .collect()
    }

    pub fn cycle_sort_mode(&mut self) {
        if !self.sort_controls_available() {
            self.show_info_message("Fuzzy List results are ordered by relevance");

            return;
        }

        self.sort_mode = self.sort_mode.next();

        self.apply_sort();
    }

    pub fn sort_controls_available(&self) -> bool {
        /*
         * A flat Fuzzy result is ordered globally by worker relevance.
         *
         * Tree still uses the configured sort mode to arrange siblings inside
         * its hierarchy, so sorting remains meaningful there.
         */
        self.search_mode != SearchMode::Fuzzy || self.view_mode == ViewMode::Tree
    }

    fn disable_reverse_for_fuzzy_list(&mut self) {
        /*
         * Flat Fuzzy results are ordered globally by relevance.
         *
         * Reverse remains meaningful in Fuzzy Tree mode because it controls the
         * ordering of siblings inside each branch. Disable it only when Scry enters
         * the flat Fuzzy List combination.
         */
        if self.search_mode == SearchMode::Fuzzy && self.view_mode == ViewMode::List {
            self.sort_descending = false;
        }
    }

    pub fn toggle_sort_direction(&mut self) {
        if !self.sort_controls_available() {
            self.show_info_message("Fuzzy List results are ordered by relevance");

            return;
        }

        self.sort_descending = !self.sort_descending;

        self.apply_sort();
    }

    fn apply_sort(&mut self) {
        let selected_path = self.selected_entry().map(|entry| entry.path.clone());

        /*
         * The immediate-directory list is always kept sorted because it is used
         * both by normal List mode and as the root of ordinary Tree mode.
         */
        sort_entries(&mut self.entries, self.sort_mode, self.sort_descending);

        match self.view_mode {
            ViewMode::List => {
                self.refresh_filter();

                if let Some(path) = selected_path {
                    self.select_visible_path(&path);
                }
            }

            ViewMode::Tree if self.recursive_search_active() => {
                /*
                 * A genuine recursive query must be rerun through its Exact/Fuzzy worker.
                 *
                 * Calling rebuild_recursive_search_tree() directly here would interpret
                 * the complete query as literal path text and would discard valid modifier,
                 * Boolean, or fuzzy results.
                 */
                self.refresh_active_recursive_tree(selected_path);
            }

            ViewMode::Tree => {
                /*
                 * Ordinary Tree mode retains its already-loaded child maps, so
                 * those sibling vectors must be reordered in place.
                 */
                for children in self.tree_children.values_mut() {
                    sort_entries(children, self.sort_mode, self.sort_descending);
                }

                self.rebuild_tree_rows(selected_path);
            }
        }

        self.ensure_selection_visible(self.viewport_rows);
    }

    pub fn select_visible_position(&mut self, position: usize) {
        let entry_count = self.current_visible_entry_count();

        if position >= entry_count {
            return;
        }

        self.selected = position;

        self.clear_messages();
    }

    pub fn scroll_selection(&mut self, amount: isize) {
        let entry_count = self.current_visible_entry_count();

        if entry_count == 0 {
            self.selected = 0;
            self.list_offset = 0;

            return;
        }

        if amount < 0 {
            self.selected = self.selected.saturating_sub(amount.unsigned_abs());
        } else {
            self.selected = self
                .selected
                .saturating_add(amount as usize)
                .min(entry_count.saturating_sub(1));
        }
    }

    pub fn scroll_horizontal_left(&mut self) {
        const HORIZONTAL_SCROLL_STEP: usize = 6;

        self.horizontal_offset = self
            .horizontal_offset
            .saturating_sub(HORIZONTAL_SCROLL_STEP);
    }

    pub fn scroll_horizontal_right(&mut self) {
        const HORIZONTAL_SCROLL_STEP: usize = 6;

        self.horizontal_offset = self
            .horizontal_offset
            .saturating_add(HORIZONTAL_SCROLL_STEP)
            .min(self.horizontal_max_offset);
    }

    pub fn move_up(&mut self) {
        let entry_count = self.current_visible_entry_count();

        if entry_count == 0 {
            self.selected = 0;

            self.list_offset = 0;

            return;
        }

        if self.selected == 0 {
            self.selected = entry_count.saturating_sub(1);
        } else {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        let entry_count = self.current_visible_entry_count();

        if entry_count == 0 {
            self.selected = 0;

            self.list_offset = 0;

            return;
        }

        self.selected = (self.selected + 1) % entry_count;
    }

    pub fn page_down(&mut self) {
        let entry_count = self.current_visible_entry_count();

        if entry_count == 0 {
            self.selected = 0;

            self.list_offset = 0;

            return;
        }

        let amount = self.page_amount();

        self.selected = self
            .selected
            .saturating_add(amount)
            .min(entry_count.saturating_sub(1));
    }

    pub fn fast_page_down(&mut self) {
        let entry_count = self.current_visible_entry_count();

        if entry_count == 0 {
            self.selected = 0;

            self.list_offset = 0;

            return;
        }

        let amount = self.page_amount().saturating_mul(10);

        self.selected = self
            .selected
            .saturating_add(amount)
            .min(entry_count.saturating_sub(1));
    }

    pub fn page_up(&mut self) {
        let amount = self.page_amount();

        self.selected = self.selected.saturating_sub(amount);
    }

    pub fn fast_page_up(&mut self) {
        let amount = self.page_amount().saturating_mul(10);

        self.selected = self.selected.saturating_sub(amount);
    }

    fn page_amount(&self) -> usize {
        /*
         * Preserve one visible row between pages so that the user retains
         * visual context when moving through a long listing.
         */
        self.viewport_rows.saturating_sub(1).max(1)
    }

    pub fn select_first(&mut self) {
        self.selected = 0;

        self.list_offset = 0;
    }

    pub fn select_last(&mut self) {
        self.selected = self.current_visible_entry_count().saturating_sub(1);
    }

    pub fn owner_name(&mut self, owner_id: u32) -> String {
        if let Some(name) = self.owner_name_cache.get(&owner_id) {
            return name.clone();
        }

        let name = if self.source.is_remote() {
            /*
             * Remote UIDs belong to the remote host. Never resolve them through
             * the local machine because identical numbers may name different users.
             */
            self.source
                .owner_name(owner_id)
                .ok()
                .flatten()
                .unwrap_or_else(|| owner_id.to_string())
        } else {
            get_user_by_uid(owner_id)
                .map(|user| user.name().to_string_lossy().into_owned())
                .unwrap_or_else(|| owner_id.to_string())
        };

        self.owner_name_cache.insert(owner_id, name.clone());

        name
    }

    fn path_is_directory(&mut self, path: &Path, fallback: bool) -> bool {
        self.source.path_is_directory(path).unwrap_or(fallback)
    }

    /*
     * Return the best immediately available directory-content hint for rendering.
     *
     * Scrollbar dragging and rapid navigation must never begin fresh filesystem
     * reads on the terminal thread. A drag across a large recursive corpus may jump
     * between directories thousands of entries apart, and synchronous read_dir()
     * calls would freeze mouse-event processing.
     *
     * Cached answers remain authoritative. During rapid movement, an uncached
     * directory receives a conservative expansion hint and may be inspected after
     * navigation settles.
     */
    pub fn directory_has_content_for_render(&mut self, path: &PathBuf) -> bool {
        if let Some(has_content) = self.directory_has_content_cache.get(path) {
            return *has_content;
        }

        if self.scrollbar_drag_active || self.rapid_navigation_active {
            /*
             * Showing an expansion marker temporarily is safer than blocking the UI
             * or falsely presenting a potentially populated directory as final.
             */
            return true;
        }

        self.directory_has_content(path)
    }

    pub fn directory_has_content(&mut self, path: &PathBuf) -> bool {
        if let Some(has_content) = self.directory_has_content_cache.get(path) {
            return *has_content;
        }

        let has_content = self.source.directory_has_content(path).unwrap_or(false);

        self.directory_has_content_cache
            .insert(path.clone(), has_content);

        has_content
    }

    pub fn tree_directory_has_visible_children(&mut self, path: &PathBuf) -> bool {
        /*
         * Outside directories-only mode, the ordinary physical-content check is
         * still correct.
         */
        if self.entry_filter != EntryFilter::DirectoriesOnly {
            return self.directory_has_content_for_render(path);
        }

        /*
         * A closed directory retains the ordinary Tree hint.
         *
         * It may physically contain files even though those files are excluded by
         * directories-only mode. The arrow tells the user that the directory can
         * still be inspected.
         */
        let branch_is_open = if self.recursive_search_active() {
            if self.effective_query_is_active() {
                !self.search_collapsed_directories.contains(path)
            } else {
                self.recursive_expanded_directories.contains(path)
            }
        } else {
            self.expanded_directories.contains(path)
        };

        if !branch_is_open {
            return self.directory_has_content_for_render(path);
        }

        /*
         * Once opened, describe only children visible in directories-only mode.
         *
         * A directory containing files but no subdirectories therefore changes
         * from "→" to "/" while it is open.
         */
        let children = if self.recursive_search_active() {
            self.search_tree_children.get(path)
        } else {
            self.tree_children.get(path)
        };

        children.is_some_and(|children| {
            children.iter().any(|entry| {
                entry.is_directory && (self.show_hidden || !entry.name.starts_with('.'))
            })
        })
    }

    pub fn enter_selected_directory(&mut self) {
        if self.view_mode == ViewMode::Tree {
            self.expand_selected_tree_directory();

            return;
        }

        let Some(entry) = self.selected_entry() else {
            return;
        };

        if !entry.is_directory {
            return;
        }

        let target = entry.path.clone();

        /*
         * Right Arrow enters a List search result without passing through
         * activate_selected(). Save the same return state that Enter saves.
         *
         * Replacing any older state prevents a previous search rooted at "/" from
         * unexpectedly being restored later.
         */
        if self.recursive_search_active() && !self.query.is_empty() && self.query != "." {
            self.save_search_return_state(target.clone());
        }

        self.navigate_to_directory(target, None);
    }

    fn enter_selected_tree_directory_as_root(&mut self) {
        let Some(entry) = self.selected_entry() else {
            return;
        };

        let path = entry.path.clone();

        let entry_is_directory = entry.is_directory;

        let is_directory = self.path_is_directory(&path, entry_is_directory);

        if !is_directory {
            return;
        }

        /*
         * Directive-only and incomplete queries remain visible while ordinary Tree
         * navigation continues.
         *
         * Preserve their caret position explicitly across the Tree reroot.
         */
        let preserve_inactive_query = !self.query.is_empty() && !self.effective_query_is_active();

        let preserved_query = preserve_inactive_query.then(|| self.query.clone());

        let preserved_query_cursor = self.query_cursor;

        if !self.navigate_to_directory(path, None) {
            return;
        }

        if let Some(query) = preserved_query {
            self.query = query;

            self.query_cursor = preserved_query_cursor.min(self.query.len());
        }

        /*
         * Enter originated in Tree mode, so the selected directory becomes the new
         * Tree root.
         */
        self.view_mode = ViewMode::Tree;

        self.selected = 0;

        self.list_offset = 0;

        if self.recursive_mode {
            self.refresh_active_recursive_tree(None);
        } else {
            /*
             * Empty, directive-only, and incomplete queries use the ordinary Tree.
             *
             * Build it immediately from the new root's already loaded entries.
             */
            self.reset_tree();
        }
    }

    pub fn enter_home_directory(&mut self) {
        let home_directory = self.home_directory.clone();

        if self.current_directory == home_directory {
            self.show_info_message("Already home :)");

            return;
        }
        /*
         * During an active search, Home changes only the search root.
         *
         * The query, Exact/Fuzzy mode, recursive mode, and result view remain
         * active—matching the behavior of Left-arrow search-root navigation.
         */
        if !self.query.is_empty() && self.query != "." {
            let previous_root = self.current_directory.clone();

            self.change_search_root(home_directory, Some(previous_root));

            return;
        }

        /*
         * Without an active query, Home behaves as ordinary directory navigation.
         */
        self.search_return_state = None;

        self.search_navigation_active = false;

        self.navigate_to_directory(home_directory, None);
    }

    pub fn enter_previous_directory(&mut self) {
        while let Some(previous) = self.back_history.pop() {
            if previous.directory == self.current_directory {
                continue;
            }

            /*
             * Back restoration deliberately calls the low-level operation.
             *
             * It must not record the location being left, or repeated Back clicks
             * would bounce forever between two directories.
             */
            if !self.change_directory(previous.directory, None) {
                continue;
            }

            if previous.view_mode == ViewMode::Tree {
                self.view_mode = ViewMode::Tree;

                self.selected = 0;

                self.list_offset = 0;

                if self.recursive_mode {
                    self.refresh_active_recursive_tree(None);
                } else {
                    self.reset_tree();
                }
            }

            return;
        }

        self.show_info_message("No previous directory");
    }

    pub fn enter_parent_directory(&mut self) {
        /*
         * Tree mode owns Left/Escape navigation even while a search query is active.
         *
         * Otherwise, the query-root navigation route would intercept navigation before
         * the selected branch can be collapsed.
         */
        if self.view_mode == ViewMode::Tree {
            self.collapse_selected_tree_directory_or_select_parent();

            return;
        }

        /*
         * An active List-mode query belongs to the current search session and takes
         * priority over an older suspended-search return bookmark.
         *
         * Carry the same query upward through as many parent directories as the user
         * requests.
         */
        if !self.query.is_empty() && self.query != "." {
            let previous_root = self.current_directory.clone();

            let Some(parent) = previous_root.parent() else {
                return;
            };

            let parent = parent.to_path_buf();

            if parent == previous_root {
                return;
            }

            self.search_return_state = None;

            self.change_search_root(parent, Some(previous_root));

            return;
        }

        /*
         * With no active query, Left may restore a search that was suspended when one
         * of its results was entered.
         */
        if self.restore_search_return_state() {
            return;
        }

        let child_directory = self.current_directory.clone();

        let Some(parent) = self.current_directory.parent() else {
            return;
        };

        let parent = parent.to_path_buf();

        if parent == self.current_directory {
            return;
        }

        self.navigate_to_directory(parent, Some(child_directory));
    }

    fn restore_search_return_state(&mut self) -> bool {
        let Some(state) = self.search_return_state.clone() else {
            return false;
        };

        /*
         * The saved search is restored only when backing directly out of the
         * directory into which that search result originally landed.
         *
         * If the user navigates deeper, ordinary parent navigation remains intact.
         */
        if self.current_directory != state.landed_directory {
            return false;
        }

        let entries = match self.source.read_directory(
            &state.root_directory,
            self.sort_mode,
            self.sort_descending,
        ) {
            Ok(entries) => entries,

            Err(error) => {
                self.error_message = Some(format!(
                    "Unable to restore search root {}: {}",
                    state.root_directory.display(),
                    error,
                ));

                return true;
            }
        };

        if self.persistent_remote_index_available() {
            /*
             * Restoring a suspended remote search changes only the visible scope.
             *
             * Keep the complete host-wide corpus, path lookup, and SearchIndex
             * resident. Only the active fuzzy worker and derived view state are
             * disposable.
             */
            self.cancel_fuzzy_filter();

            self.filtered_indices.clear();

            self.tree_rows.clear();

            self.filtered_tree_indices.clear();

            self.tree_children.clear();

            self.search_tree_children.clear();

            self.expanded_directories.clear();

            self.search_collapsed_directories.clear();

            self.recursive_expanded_directories.clear();
        } else {
            self.invalidate_recursive_cache();
        }

        self.current_directory = state.root_directory;

        self.entries = entries;

        self.query = state.query;

        self.query_cursor = self.query.len();

        self.search_mode = state.search_mode;

        self.recursive_mode = state.recursive_mode;

        self.view_mode = ViewMode::List;

        self.selected = 0;

        self.list_offset = 0;

        self.pending_selection_path = state.selected_path.clone();

        self.error_message = None;

        self.tree_rows.clear();

        self.filtered_tree_indices.clear();

        self.tree_children.clear();

        self.search_tree_children.clear();

        self.expanded_directories.clear();

        self.search_collapsed_directories.clear();

        self.recursive_expanded_directories.clear();

        self.directory_has_content_cache.clear();

        self.ensure_recursive_scan();

        match state.view_mode {
            ViewMode::List => {
                self.refresh_filter();

                self.restore_pending_selection_if_available();
            }

            ViewMode::Tree => {
                self.view_mode = ViewMode::Tree;

                /*
                 * Restore the search through the same parsed worker route used by a
                 * live recursive Tree query.
                 *
                 * Structured queries such as:
                 *
                 *     type:dir
                 *     ext:rs
                 *     +rust
                 *     -java
                 *
                 * must not be rebuilt as literal path substrings.
                 */
                if !self.scan_in_progress {
                    match self.search_mode {
                        SearchMode::Exact => {
                            self.start_current_exact_filter();
                        }

                        SearchMode::Fuzzy => {
                            self.start_current_fuzzy_filter();
                        }
                    }
                }
            }
        }

        self.list_offset = state.list_offset;

        self.ensure_selection_visible(self.viewport_rows);

        self.search_return_state = None;

        self.search_navigation_active = true;

        true
    }

    fn reset_tree(&mut self) {
        self.tree_rows.clear();

        self.filtered_tree_indices.clear();

        self.tree_children.clear();

        self.expanded_directories.clear();

        self.search_collapsed_directories.clear();

        self.search_tree_children.clear();

        /*
         * Keep the complete root listing in the Tree backing map.
         *
         * Visibility belongs exclusively to refresh_tree_filter(), which applies:
         *
         * - hidden-entry policy;
         * - staged-deletion filtering;
         * - FilesOnly / DirectoriesOnly;
         * - any effective ordinary Tree query.
         *
         * This keeps the backing hierarchy reusable without allowing unfiltered
         * rows from a previous Tree state to remain visible.
         */
        self.tree_children
            .insert(self.current_directory.clone(), self.entries.clone());

        self.rebuild_tree_rows(None);

        /*
         * rebuild_tree_rows() already refreshes the filter. Reassert it here at the
         * ordinary Tree reset boundary so every caller receives a fully filtered
         * published Tree, even if the rebuild implementation changes later.
         */
        self.refresh_tree_filter();
    }

    /*
     * Reconstruct the ordinary non-recursive Tree branch leading to a pending
     * session selection.
     *
     * Recursive Tree mode has a resident descendant index from which branches can
     * be rebuilt. Ordinary Tree mode contains only directories that have already
     * been opened during the current run, so every ancestor of the saved entry must
     * be loaded again explicitly.
     */
    fn restore_pending_non_recursive_tree_branch(&mut self) -> io::Result<()> {
        if self.view_mode != ViewMode::Tree || self.recursive_mode {
            return Ok(());
        }

        /*
         * Begin with every branch explicitly saved in the session.
         *
         * Also include the ancestor chain of the selected entry. This preserves the
         * old useful behavior where a selected deep entry can still be revealed even
         * when loading an older session that has no serialized expansion fields.
         */
        let mut directories_to_restore: HashSet<PathBuf> = self
            .expanded_directories
            .iter()
            .filter(|path| {
                path.starts_with(&self.current_directory) && *path != &self.current_directory
            })
            .cloned()
            .collect();

        if let Some(target) = self.pending_selection_path.as_ref()
            && target.starts_with(&self.current_directory)
        {
            let mut ancestor = target.parent();

            while let Some(path) = ancestor {
                if path == self.current_directory {
                    break;
                }

                if !path.starts_with(&self.current_directory) {
                    break;
                }

                directories_to_restore.insert(path.to_path_buf());

                ancestor = path.parent();
            }
        }

        /*
         * reset_tree() establishes the root's immediate entries but clears the
         * in-memory expansion set. The desired paths have already been copied above.
         */
        self.reset_tree();

        /*
         * Parents must be loaded before descendants.
         *
         * Path order breaks ties so restoration remains deterministic.
         */
        let mut ordered_directories: Vec<PathBuf> = directories_to_restore.into_iter().collect();

        ordered_directories.sort_by(|left, right| {
            left.components()
                .count()
                .cmp(&right.components().count())
                .then_with(|| left.cmp(right))
        });

        for directory in ordered_directories {
            let children =
                self.source
                    .read_directory(&directory, self.sort_mode, self.sort_descending)?;

            self.tree_children.insert(directory.clone(), children);

            self.expanded_directories.insert(directory);
        }

        /*
         * All saved ordinary branches are now populated and marked open. Build one
         * coherent visible Tree before selection and viewport restoration occur.
         */
        self.rebuild_tree_rows(None);

        Ok(())
    }

    fn allow_manual_tree_expansion(
        &mut self,
        would_be_rows: usize,
        target: TreeExpandAllTarget,
        selected_path: Option<PathBuf>,
    ) -> bool {
        let maximum_rows = self.advanced_tree_config.max_visible_tree_rows;

        if would_be_rows <= maximum_rows {
            return true;
        }

        /*
         * Manual branch expansion is governed by the absolute visible-Tree
         * ceiling, but unlike the once-per-session Alt+E refusal this explanation
         * is deliberately shown every time an operation is refused.
         *
         * A refused expansion reaches this gate before any branch-expansion state
         * is committed, so selection and the existing Tree remain untouched.
         */
        self.tree_expand_all_dialog = Some(TreeExpandAllDialogState {
            kind: TreeExpandAllDialogKind::DisplayLimit,

            projected_rows: would_be_rows,

            configured_max_rows: maximum_rows,

            disable_warning: false,

            display_limit_action: TreeDisplayLimitAction::BranchExpansion,

            focus: TreeExpandAllDialogFocus::Cancel,

            target,

            selected_path,
        });

        false
    }

    fn refuse_tree_visibility_transition(
        &mut self,
        would_be_rows: usize,
        action: TreeDisplayLimitAction,
    ) -> bool {
        let maximum_rows = self.advanced_tree_config.max_visible_tree_rows;

        if would_be_rows <= maximum_rows {
            return false;
        }

        /*
         * Visibility transitions use the same persistent hard-limit explanation as
         * manual branch expansion.
         *
         * The caller reaches this gate before committing the new visibility mode, so
         * a refusal leaves the current valid Tree and its branch state untouched.
         */
        self.tree_expand_all_dialog = Some(TreeExpandAllDialogState {
            kind: TreeExpandAllDialogKind::DisplayLimit,

            projected_rows: would_be_rows,

            configured_max_rows: maximum_rows,

            disable_warning: false,

            focus: TreeExpandAllDialogFocus::Cancel,

            target: TreeExpandAllTarget::Ordinary,

            selected_path: self.selected_entry().map(|entry| entry.path.clone()),

            display_limit_action: action,
        });

        true
    }

    fn ordinary_tree_rows_revealed_by_expansion(&self, directory: &Path) -> usize {
        let Some(children) = self.tree_children.get(directory) else {
            return 0;
        };

        let mut count = 0_usize;

        for entry in children {
            if !entry_matches_visibility(
                entry,
                &self.current_directory,
                self.show_hidden,
                self.hidden_only,
            ) {
                continue;
            }

            /*
             * tree_rows retains structural directories even when the active
             * entry-kind filter hides their own row. Count only rows that would
             * actually be displayed, while still descending through remembered
             * expanded directories below them.
             */
            if !path_belongs_to_staged_deletion(&entry.path, &self.staged_deletions)
                && self.entry_filter.matches(entry)
            {
                count = count.saturating_add(1);
            }

            if entry.is_directory
                && !entry.is_symlink
                && self.expanded_directories.contains(&entry.path)
            {
                count = count
                    .saturating_add(self.ordinary_tree_rows_revealed_by_expansion(&entry.path));
            }
        }

        count
    }

    fn expand_selected_tree_directory(&mut self) {
        if self.recursive_search_active() {
            self.expand_selected_recursive_branch();

            return;
        }

        let Some(row) = self.tree_row_at_filtered_position(self.selected).cloned() else {
            return;
        };

        if !row.entry.is_directory || row.entry.is_symlink {
            return;
        }

        let path = row.entry.path.clone();

        /*
         * Right on an already open ordinary branch has nothing further to expose.
         */
        if self.expanded_directories.contains(&path) {
            return;
        }

        if !self.tree_children.contains_key(&path) {
            match self
                .source
                .read_directory(&path, self.sort_mode, self.sort_descending)
            {
                Ok(entries) => {
                    self.tree_children.insert(path.clone(), entries);

                    self.error_message = None;
                }

                Err(error) => {
                    self.error_message =
                        Some(format!("Unable to open {}: {}", path.display(), error,));

                    return;
                }
            }
        }

        /*
         * Count only rows that this closed branch would reveal.
         *
         * Descendant expansion state may still be remembered beneath the closed
         * parent, so reopening one branch can legitimately expose much more than
         * one immediate directory level.
         */
        let added_rows = self.ordinary_tree_rows_revealed_by_expansion(&path);

        let would_be_rows = self
            .current_visible_entry_count()
            .saturating_add(added_rows);

        if !self.allow_manual_tree_expansion(
            would_be_rows,
            TreeExpandAllTarget::Ordinary,
            Some(path.clone()),
        ) {
            return;
        }

        self.expanded_directories.insert(path.clone());

        self.rebuild_tree_rows(Some(path));
    }

    fn collapse_selected_tree_directory_or_select_parent(&mut self) {
        /*
         * An empty Tree has no selected row. Move the invisible Tree root upward.
         */
        if self.current_visible_entry_count() == 0 {
            if self.recursive_search_active() {
                self.move_recursive_tree_root_to_parent();
            } else {
                self.move_tree_root_to_parent();
            }

            return;
        }

        let Some(tree_index) = self.filtered_tree_indices.get(self.selected).copied() else {
            return;
        };

        let Some(row) = self.tree_rows.get(tree_index).cloned() else {
            return;
        };

        let path = row.entry.path.clone();

        if self.recursive_search_active() {
            /*
             * Do not trust row.expanded here.
             *
             * A branch is visibly open when the next displayed row is one of its
             * descendants. Path comparison remains correct after session restoration
             * and after rebuilding either kind of recursive Tree.
             */
            let has_visible_descendants = self
                .tree_rows
                .get(tree_index.saturating_add(1))
                .is_some_and(|next_row| {
                    next_row.entry.path != path && next_row.entry.path.starts_with(&path)
                });

            let query_is_active = self.effective_query_is_active();

            /*
             * A queryless recursive branch can be explicitly open even when the active
             * entry filter leaves it with no visible descendants.
             *
             * Example in directories-only mode:
             *
             *     directory →
             *
             * Right inspects it. If it contains files but no subdirectories:
             *
             *     directory/
             *
             * It is still an explicitly opened branch, so Left must be able to close
             * it and restore the ordinary arrow hint.
             */
            let branch_is_open = if query_is_active {
                has_visible_descendants
            } else {
                self.recursive_expanded_directories.contains(&path)
            };

            /*
             * Left always closes the selected open branch first.
             *
             * Alt+E is only a convenient bulk-expansion action. It must not prevent the
             * user from reducing that expanded Tree one branch at a time afterward.
             */
            if row.entry.is_directory && branch_is_open {
                if query_is_active {
                    /*
                     * Search-result Trees are open by default.
                     */
                    self.search_collapsed_directories.insert(path.clone());
                } else {
                    /*
                     * Queryless recursive Trees are closed by default.
                     *
                     * Close only the selected branch. Expanded descendants remain remembered while
                     * hidden beneath it, so reopening the parent restores the subtree exactly as
                     * the user left it.
                     *
                     * Collapse All remains responsible for forgetting the complete expanded state.
                     */
                    self.recursive_expanded_directories.remove(&path);
                }

                self.rebuild_recursive_search_rows(Some(path));

                self.ensure_selection_visible(self.viewport_rows);

                return;
            }

            let Some(parent) = path.parent() else {
                return;
            };

            /*
             * The recursive root itself is invisible. Reaching a direct child of
             * that root means the next Left must move the root upward.
             */
            if parent == self.current_directory {
                self.move_recursive_tree_root_to_parent();

                return;
            }

            self.select_parent_in_search_tree();

            return;
        }

        /*
         * Left always closes the selected open branch first, including branches
         * opened through Alt+E Expand All.
         */
        if row.entry.is_directory && self.expanded_directories.contains(&path) {
            /*
             * Close only the selected branch.
             *
             * Descendant expansion choices remain recorded while the subtree is hidden.
             * Reopening this directory therefore restores the nested branches exactly as
             * the user arranged them.
             *
             * Manually closing even one branch means the Tree is no longer in the
             * authoritative Alt+E Expand All state.
             */
            self.expanded_directories.remove(&path);

            self.ordinary_expand_all_active = false;

            self.rebuild_tree_rows(Some(path));

            return;
        }

        let Some(parent) = path.parent() else {
            return;
        };

        if parent == self.current_directory {
            self.move_tree_root_to_parent();

            return;
        }

        if let Some(parent_position) = self.filtered_tree_indices.iter().position(|tree_index| {
            self.tree_rows
                .get(*tree_index)
                .is_some_and(|candidate| candidate.entry.path == parent)
        }) {
            self.selected = parent_position;
        }
    }

    fn expand_selected_recursive_branch(&mut self) {
        let Some(tree_index) = self.filtered_tree_indices.get(self.selected).copied() else {
            return;
        };

        let Some(row) = self.tree_rows.get(tree_index).cloned() else {
            return;
        };

        if !row.entry.is_directory || row.entry.is_symlink {
            return;
        }

        let path = row.entry.path.clone();

        /*
         * The search hierarchy may contain only a partial contextual child list:
         *
         * - direct matches;
         * - ancestors required to connect those matches.
         *
         * Right Arrow is an explicit request to expose this directory's complete
         * immediate matching branch. Recover those children from the resident
         * recursive corpus even when search_tree_children already contains one or
         * more contextual children.
         */
        let mut recovered_children: Vec<FileEntry> = self
            .recursive_child_indices
            .get(&path)
            .into_iter()
            .flatten()
            .filter_map(|index| self.recursive_entries.get(*index))
            .filter(|entry| self.show_hidden || !entry.name.starts_with('.'))
            .filter(|entry| self.entry_filter.matches(entry))
            .cloned()
            .collect();

        /*
         * A partial local recursive scan may not contain this directory's immediate
         * children yet. In that case, fall back to one ordinary source read.
         *
         * A loaded remote persistent index should normally satisfy the resident
         * lookup above without an SFTP request.
         */
        if recovered_children.is_empty() {
            recovered_children =
                match self
                    .source
                    .read_directory(&path, self.sort_mode, self.sort_descending)
                {
                    Ok(entries) => entries
                        .into_iter()
                        .filter(|entry| self.show_hidden || !entry.name.starts_with('.'))
                        .filter(|entry| self.entry_filter.matches(entry))
                        .collect(),

                    Err(error) => {
                        self.show_error_message(format!(
                            "Unable to expand {}: {}",
                            path.display(),
                            error,
                        ));

                        return;
                    }
                };
        }

        if recovered_children.is_empty() {
            return;
        }

        sort_entries(
            &mut recovered_children,
            self.sort_mode,
            self.sort_descending,
        );

        /*
         * In queryless Recursive Tree mode, materialize one level beyond the newly
         * opened branch so each child directory immediately knows whether it is
         * expandable.
         *
         * Query-driven Trees keep their established contextual worker hierarchy.
         */
        if !self.effective_query_is_active() {
            let child_directories: Vec<PathBuf> = recovered_children
                .iter()
                .filter(|entry| entry.is_directory && !entry.is_symlink)
                .map(|entry| entry.path.clone())
                .collect();

            for directory in child_directories {
                let children = self.queryless_recursive_children_from_index(&directory);

                if !children.is_empty() {
                    self.search_tree_children.insert(directory, children);
                }
            }
        }

        /*
         * Replace any bounded contextual child list with the complete immediate
         * matching branch recovered above.
         */
        self.search_tree_children
            .insert(path.clone(), recovered_children);

        let fallback_position = self.selected;

        let mut rows = Vec::new();

        if self.effective_query_is_active() {
            /*
             * Search-result Trees are open by default.
             *
             * Calculate the complete prospective visible representation using a
             * temporary collapsed-directory set. The real branch state is not
             * changed unless the result satisfies the visible-row ceiling.
             */
            let mut prospective_collapsed_directories = self.search_collapsed_directories.clone();

            prospective_collapsed_directories.remove(&path);

            Self::append_recursive_search_children(
                self.current_directory.clone(),
                Vec::new(),
                &self.search_tree_children,
                &prospective_collapsed_directories,
                &mut rows,
            );

            let would_be_rows = rows.len();

            if !self.allow_manual_tree_expansion(
                would_be_rows,
                TreeExpandAllTarget::RecursiveSearch,
                Some(path.clone()),
            ) {
                return;
            }

            self.search_collapsed_directories = prospective_collapsed_directories;
        } else {
            /*
             * Queryless Recursive Tree branches are closed by default.
             *
             * Calculate the prospective representation with a temporary expanded
             * set so a refused operation cannot modify remembered branch state.
             */
            let mut prospective_expanded_directories = self.recursive_expanded_directories.clone();

            prospective_expanded_directories.insert(path.clone());

            Self::append_recursive_direct_children(
                self.current_directory.clone(),
                Vec::new(),
                &self.search_tree_children,
                &prospective_expanded_directories,
                &mut rows,
            );

            let would_be_rows = rows.len();

            if !self.allow_manual_tree_expansion(
                would_be_rows,
                TreeExpandAllTarget::RecursiveQueryless,
                Some(path.clone()),
            ) {
                return;
            }

            self.recursive_expanded_directories = prospective_expanded_directories;
        }

        /*
         * The prospective Tree was already constructed for the policy check.
         *
         * Reuse it directly instead of rebuilding the same hierarchy a second
         * time after approval.
         */
        self.tree_rows = rows;

        self.filtered_tree_indices = (0..self.tree_rows.len()).collect();

        self.restore_search_tree_selection(Some(path), fallback_position);

        self.ensure_selection_visible(self.viewport_rows);
    }

    fn move_recursive_tree_root_to_parent(&mut self) {
        let previous_root = self.current_directory.clone();

        let Some(parent) = previous_root.parent() else {
            return;
        };

        let parent = parent.to_path_buf();

        if parent == previous_root {
            return;
        }

        /*
         * If the resident local recursive corpus already covers the parent, moving the
         * Tree root upward is only a scope change. Leave that corpus completely intact.
         *
         * A reusable seed is needed only when the destination parent lies outside the
         * current corpus root. In that case, retain the completed former-root subtree
         * while scanning the newly exposed parent scope around it.
         */
        let parent_already_covered = self.local_recursive_corpus_covers(&parent);

        let reusable_seed = !parent_already_covered
            && !self.source.is_remote()
            && self.view_mode == ViewMode::Tree
            && self.recursive_mode
            && !self.hidden_only
            && self.recursive_cache_complete
            && !self.recursive_scan_partial
            && !self.scan_in_progress;

        if reusable_seed {
            self.pending_recursive_scan_seed = Some(RecursiveScanSeed {
                excluded_subtree: previous_root.clone(),

                entries: std::mem::take(&mut self.recursive_entries),
            });
        } else {
            self.pending_recursive_scan_seed = None;
        }

        if !self.change_directory(parent, Some(previous_root.clone())) {
            return;
        }

        /*
         * change_directory() returns to List mode.
         *
         * Restore Tree mode and rebuild the recursive hierarchy using the
         * parent directory as the new recursive root.
         */
        self.view_mode = ViewMode::Tree;

        self.selected = 0;

        self.list_offset = 0;

        if self.progressive_exact_tree_is_active() {
            self.publish_progressive_exact_tree(true);
        } else {
            self.refresh_active_recursive_tree(Some(previous_root.clone()));
        }
    }

    fn move_tree_root_to_parent(&mut self) {
        let previous_root = self.current_directory.clone();

        let Some(parent) = previous_root.parent() else {
            return;
        };

        let parent = parent.to_path_buf();

        /*
         * At the filesystem root, parent and current path are identical.
         */
        if parent == previous_root {
            return;
        }

        /*
         * change_directory() normally clears the query because ordinary directory
         * navigation begins a fresh browsing session.
         *
         * This operation originated from Tree rerooting, so preserve the visible
         * directive-only or incomplete query across that directory change.
         */
        let preserved_query = self.query.clone();

        let preserved_query_cursor = self.query_cursor;

        if !self.change_directory(parent, Some(previous_root.clone())) {
            return;
        }

        self.query = preserved_query;

        self.query_cursor = preserved_query_cursor.min(self.query.len());

        /*
         * change_directory() returns to List mode. Restore Tree mode and construct
         * a new ordinary Tree rooted one directory higher.
         */
        self.view_mode = ViewMode::Tree;

        self.selected = 0;

        self.list_offset = 0;

        self.reset_tree();

        /*
         * Select the former root in the newly created parent Tree.
         */
        if let Some(position) = self.filtered_tree_indices.iter().position(|tree_index| {
            self.tree_rows
                .get(*tree_index)
                .is_some_and(|row| row.entry.path == previous_root)
        }) {
            self.selected = position;
        }

        self.ensure_selection_visible(self.viewport_rows);
    }

    fn rebuild_tree_rows(&mut self, preserve_selection: Option<PathBuf>) {
        /*
         * `selected` is a position inside filtered_tree_indices, not a raw
         * tree_rows index. Preserve the selected path before rebuilding so it can
         * be mapped back only after the new filtered Tree collection exists.
         */
        let selected_path = preserve_selection.or_else(|| {
            self.tree_row_at_filtered_position(self.selected)
                .map(|row| row.entry.path.clone())
        });

        let fallback_position = self.selected;

        self.tree_rows.clear();

        self.append_tree_children(self.current_directory.clone(), Vec::new());

        /*
         * Build filtered_tree_indices before restoring selection. Restoring against
         * raw tree_rows here would mix two different coordinate systems and make
         * selection jump whenever filtering excludes or rearranges rows.
         */
        self.refresh_tree_filter();

        if let Some(selected_path) = selected_path {
            if let Some(position) = self.filtered_tree_indices.iter().position(|tree_index| {
                self.tree_rows
                    .get(*tree_index)
                    .is_some_and(|row| row.entry.path == selected_path)
            }) {
                self.selected = position;
            } else {
                self.selected =
                    fallback_position.min(self.filtered_tree_indices.len().saturating_sub(1));
            }
        } else {
            self.selected =
                fallback_position.min(self.filtered_tree_indices.len().saturating_sub(1));
        }

        if self.filtered_tree_indices.is_empty() {
            self.selected = 0;
            self.list_offset = 0;
        } else {
            self.list_offset = self
                .list_offset
                .min(self.filtered_tree_indices.len().saturating_sub(1));
        }
    }

    fn append_tree_children(&mut self, directory: PathBuf, ancestor_has_more: Vec<bool>) {
        let Some(children) = self.tree_children.get(&directory).cloned() else {
            return;
        };

        let visible_children: Vec<FileEntry> = children
            .into_iter()
            .filter(|entry| {
                entry_matches_visibility(
                    entry,
                    &self.current_directory,
                    self.show_hidden,
                    self.hidden_only,
                )
            })
            .collect();

        let child_count = visible_children.len();

        for (index, entry) in visible_children.into_iter().enumerate() {
            let is_last = index + 1 == child_count;

            let expanded = entry.is_directory
                && !entry.is_symlink
                && self.expanded_directories.contains(&entry.path);

            let child_path = entry.path.clone();

            self.tree_rows.push(TreeRow {
                entry,

                ancestor_has_more: ancestor_has_more.clone(),

                is_last,

                expanded,
            });

            if expanded {
                let mut child_ancestor_has_more = ancestor_has_more.clone();

                child_ancestor_has_more.push(!is_last);

                self.append_tree_children(child_path, child_ancestor_has_more);
            }
        }
    }

    pub fn transfer_visible(&self) -> bool {
        self.transfer.is_some()
    }

    pub fn transfer_finished(&self) -> bool {
        self.transfer
            .as_ref()
            .is_some_and(|transfer| transfer.finished_elapsed.is_some())
    }

    pub fn transfer_elapsed(&self) -> Duration {
        let Some(transfer) = self.transfer.as_ref() else {
            return Duration::ZERO;
        };

        transfer
            .finished_elapsed
            .unwrap_or_else(|| transfer.started_at.elapsed())
    }

    pub fn request_transfer_cancel(&mut self) {
        let Some(transfer) = self.transfer.as_mut() else {
            return;
        };

        if transfer.finished_elapsed.is_some() || transfer.cancel_requested {
            return;
        }

        transfer.cancel_requested = true;

        transfer.cancel_signal.store(true, Ordering::Relaxed);
    }

    fn begin_remote_transfer(&mut self, remote_path: PathBuf, total_bytes: u64) {
        if self.transfer.is_some() {
            return;
        }

        let filename = remote_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| remote_path.display().to_string());

        let label = self.source.source_label();

        let placeholder: Box<dyn FileSource> = Box::new(TransferPlaceholderSource::new(label));

        let mut source = std::mem::replace(&mut self.source, placeholder);

        let worker_path = remote_path.clone();

        let (sender, receiver) = mpsc::channel();

        let cancel_signal = Arc::new(AtomicBool::new(false));

        let worker_cancel_signal = Arc::clone(&cancel_signal);

        thread::spawn(move || {
            let progress_sender = sender.clone();

            let mut report_progress =
                move |progress: TransferProgress| -> io::Result<TransferControl> {
                    if worker_cancel_signal.load(Ordering::Relaxed) {
                        return Ok(TransferControl::Cancel);
                    }

                    match progress_sender.send(TransferWorkerMessage::Progress(progress)) {
                        Ok(()) => {
                            if worker_cancel_signal.load(Ordering::Relaxed) {
                                Ok(TransferControl::Cancel)
                            } else {
                                Ok(TransferControl::Continue)
                            }
                        }

                        Err(_) => Ok(TransferControl::Cancel),
                    }
                };

            let result = source.materialize_file(&worker_path, &mut report_progress);

            let _ = sender.send(TransferWorkerMessage::Finished(TransferWorkerResult {
                source,

                result,
            }));
        });

        self.error_message = None;

        self.transfer = Some(TransferState {
            filename,

            total_bytes,

            transferred_bytes: 0,

            started_at: Instant::now(),

            finished_elapsed: None,

            error: None,

            cancel_requested: false,

            remote_path,

            local_path: None,

            receiver,

            cancel_signal,

            destination_root: None,

            item_index: 0,

            item_count: 1,

            item_transferred_bytes: 0,

            item_total_bytes: total_bytes,

            completed_count: 0,

            failed_count: 0,

            failures: Vec::new(),

            is_batch: false,
        });
    }

    pub fn begin_marked_transfer_batch(&mut self) {
        if self.transfer.is_some() {
            return;
        }

        let (destination_root, items, total_bytes) = match self.prepare_marked_transfer_batch() {
            Ok(batch) => batch,

            Err(error) => {
                self.show_error_message(error.to_string());

                return;
            }
        };

        let item_count = items.len();

        let first_filename = items
            .first()
            .map(|item| item.filename.clone())
            .unwrap_or_else(|| "Marked files".to_string());

        let label = self.source.source_label();

        let placeholder: Box<dyn FileSource> = Box::new(TransferPlaceholderSource::new(label));

        let mut source = std::mem::replace(&mut self.source, placeholder);

        let (sender, receiver) = mpsc::channel();

        let cancel_signal = Arc::new(AtomicBool::new(false));

        let worker_cancel_signal = Arc::clone(&cancel_signal);

        thread::spawn(move || {
            let mut completed_paths = Vec::new();

            let mut completed_bytes = 0_u64;

            let mut failures = Vec::new();

            let mut cancelled = false;

            for (item_index, item) in items.into_iter().enumerate() {
                if worker_cancel_signal.load(Ordering::Relaxed) {
                    cancelled = true;

                    break;
                }

                let progress_sender = sender.clone();

                let progress_filename = item.filename.clone();

                let item_total_bytes = item.expected_size;

                let worker_cancel_signal_for_item = Arc::clone(&worker_cancel_signal);

                let mut report_progress =
                    move |progress: TransferProgress| -> io::Result<TransferControl> {
                        if worker_cancel_signal_for_item.load(Ordering::Relaxed) {
                            return Ok(TransferControl::Cancel);
                        }

                        let message = TransferWorkerMessage::BatchProgress {
                            item_index,

                            item_count,

                            filename: progress_filename.clone(),

                            item_transferred_bytes: progress.transferred_bytes,

                            item_total_bytes: if progress.total_bytes > 0 {
                                progress.total_bytes
                            } else {
                                item_total_bytes
                            },

                            completed_bytes,
                        };

                        match progress_sender.send(message) {
                            Ok(()) => {
                                if worker_cancel_signal_for_item.load(Ordering::Relaxed) {
                                    Ok(TransferControl::Cancel)
                                } else {
                                    Ok(TransferControl::Continue)
                                }
                            }

                            Err(_) => Ok(TransferControl::Cancel),
                        }
                    };

                let result = source.download_file_to(
                    &item.remote_path,
                    &item.destination_path,
                    &mut report_progress,
                );

                match result {
                    Ok(_) => {
                        completed_bytes = completed_bytes.saturating_add(item.expected_size);

                        completed_paths.push(item.remote_path);
                    }

                    Err(error)
                        if error.kind() == io::ErrorKind::Interrupted
                            && worker_cancel_signal.load(Ordering::Relaxed) =>
                    {
                        cancelled = true;

                        break;
                    }

                    Err(error) => {
                        failures.push(BatchTransferFailure {
                            remote_path: item.remote_path,

                            message: error.to_string(),
                        });
                    }
                }
            }

            let _ = sender.send(TransferWorkerMessage::BatchFinished(
                BatchTransferWorkerResult {
                    source,

                    completed_paths,

                    failures,

                    cancelled,
                },
            ));
        });

        self.clear_messages();

        self.transfer = Some(TransferState {
            filename: first_filename,

            total_bytes,

            transferred_bytes: 0,

            started_at: Instant::now(),

            finished_elapsed: None,

            error: None,

            cancel_requested: false,

            /*
             * Batch acknowledgement does not open one remote path.
             *
             * The destination root is the meaningful final result.
             */
            remote_path: PathBuf::new(),

            local_path: None,

            destination_root: Some(destination_root),

            item_index: 0,

            item_count,

            item_transferred_bytes: 0,

            item_total_bytes: 0,

            completed_count: 0,

            failed_count: 0,

            failures: Vec::new(),

            is_batch: true,

            receiver,

            cancel_signal,
        });
    }

    pub fn process_transfer_messages(&mut self) -> bool {
        let message = match self.transfer.as_ref() {
            Some(transfer) if transfer.finished_elapsed.is_none() => {
                match transfer.receiver.try_recv() {
                    Ok(message) => Some(message),

                    Err(TryRecvError::Empty) => None,

                    Err(TryRecvError::Disconnected) => {
                        if let Some(transfer) = self.transfer.as_mut() {
                            transfer.finished_elapsed = Some(transfer.started_at.elapsed());

                            transfer.error =
                                Some("remote transfer worker stopped unexpectedly".to_string());
                        }

                        return true;
                    }
                }
            }

            _ => None,
        };

        let Some(message) = message else {
            return false;
        };

        match message {
            TransferWorkerMessage::Progress(progress) => {
                let Some(transfer) = self.transfer.as_mut() else {
                    return false;
                };

                /*
                 * Prefer the total reported by the actual transfer implementation.
                 *
                 * The directory listing normally supplied the same value when the
                 * transfer began, but the remote metadata queried during transfer
                 * is the authoritative source.
                 */
                if progress.total_bytes > 0 {
                    transfer.total_bytes = progress.total_bytes;
                }

                transfer.transferred_bytes = progress.transferred_bytes.min(transfer.total_bytes);

                true
            }

            TransferWorkerMessage::BatchProgress {
                item_index,
                item_count,
                filename,
                item_transferred_bytes,
                item_total_bytes,
                completed_bytes,
            } => {
                let Some(transfer) = self.transfer.as_mut() else {
                    return false;
                };

                transfer.item_index = item_index;

                transfer.item_count = item_count;

                transfer.filename = filename;

                transfer.item_transferred_bytes = item_transferred_bytes.min(item_total_bytes);

                transfer.item_total_bytes = item_total_bytes;

                transfer.transferred_bytes = completed_bytes
                    .saturating_add(transfer.item_transferred_bytes)
                    .min(transfer.total_bytes);

                true
            }

            TransferWorkerMessage::Finished(message) => {
                /*
                 * The worker always returns ownership of the real source, regardless of
                 * success, failure, or cancellation.
                 */
                self.source = message.source;

                let cancellation_requested = self
                    .transfer
                    .as_ref()
                    .is_some_and(|transfer| transfer.cancel_requested);

                match message.result {
                    Err(error)
                        if cancellation_requested && error.kind() == io::ErrorKind::Interrupted =>
                    {
                        /*
                         * The SFTP implementation has already removed the unfinished
                         * .scry-part file. Close the modal and resume browsing normally.
                         */
                        self.transfer = None;

                        self.clear_messages();

                        true
                    }

                    result => {
                        let Some(transfer) = self.transfer.as_mut() else {
                            return false;
                        };

                        transfer.finished_elapsed = Some(transfer.started_at.elapsed());

                        match result {
                            Ok(local_path) => {
                                transfer.transferred_bytes = transfer.total_bytes;

                                /*
                                 * Keep the cache path in the completed transfer state so the popup may
                                 * remain visible until the user acknowledges it.
                                 */
                                transfer.local_path = Some(local_path.clone());

                                /*
                                 * Open the file immediately after a successful transfer.
                                 *
                                 * The completed transfer window deliberately remains open. Its OK button
                                 * now dismisses the result rather than delaying file activation.
                                 */
                                match crate::open::open_file(&local_path) {
                                    Ok(()) => {
                                        if self.exit_on_open {
                                            self.should_quit = true;
                                        }
                                    }

                                    Err(error) => {
                                        transfer.error = Some(error);
                                    }
                                }
                            }

                            Err(error) => {
                                transfer.error = Some(error.to_string());
                            }
                        }

                        true
                    }
                }
            }

            TransferWorkerMessage::BatchFinished(message) => {
                /*
                 * As with a single transfer, the worker always returns the real source.
                 */
                self.source = message.source;

                /*
                 * Keep the newly created local batch root available if cancellation requires
                 * pruning empty hierarchy directories before the transfer state is discarded.
                 */
                let destination_root = self
                    .transfer
                    .as_ref()
                    .and_then(|transfer| transfer.destination_root.clone());

                /*
                 * Every successfully downloaded file leaves the persistent marked set.
                 *
                 * Failed or unattempted files remain marked so the user can retry them.
                 */
                for path in &message.completed_paths {
                    self.marked_files.remove(path);
                }

                if message.cancelled {
                    /*
                     * The active .scry-part file has already been removed by the SFTP layer.
                     *
                     * Now remove only empty directories that Scry created while preparing
                     * preserved hierarchy paths. Directories containing completed files remain.
                     */
                    let cleanup_error = destination_root.as_deref().and_then(|root| {
                        remove_empty_batch_directories(root)
                            .err()
                            .map(|error| (root.to_path_buf(), error))
                    });

                    self.transfer = None;

                    self.clear_messages();

                    if let Some((root, error)) = cleanup_error {
                        self.show_error_message(format!(
                            "Transfer cancelled, but empty directories under {} could not be fully cleaned: {}",
                            root.display(),
                            error,
                        ));
                    }

                    return true;
                }

                let Some(transfer) = self.transfer.as_mut() else {
                    return false;
                };

                transfer.finished_elapsed = Some(transfer.started_at.elapsed());

                transfer.completed_count = message.completed_paths.len();

                transfer.failed_count = message.failures.len();

                transfer.failures = message
                    .failures
                    .into_iter()
                    .map(|failure| {
                        format!("{}: {}", failure.remote_path.display(), failure.message,)
                    })
                    .collect();

                /*
                 * Do not force transferred_bytes to total_bytes when files failed.
                 *
                 * The aggregate byte display should remain truthful.
                 */
                true
            }
        }
    }
    pub fn acknowledge_transfer(&mut self) {
        if !self.transfer_finished() {
            return;
        }

        let Some(transfer) = self.transfer.take() else {
            return;
        };

        if transfer.is_batch {
            let destination = transfer
                .destination_root
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "the batch download directory".to_string());

            if transfer.failed_count == 0 {
                self.show_info_message(format!(
                    "Downloaded {} file{} to {}",
                    transfer.completed_count,
                    if transfer.completed_count == 1 {
                        ""
                    } else {
                        "s"
                    },
                    destination,
                ));
            } else {
                self.show_error_message(format!(
                    "Downloaded {} file{} to {}; {} file{} failed",
                    transfer.completed_count,
                    if transfer.completed_count == 1 {
                        ""
                    } else {
                        "s"
                    },
                    destination,
                    transfer.failed_count,
                    if transfer.failed_count == 1 { "" } else { "s" },
                ));
            }

            return;
        }

        if let Some(error) = transfer.error {
            /*
             * A local path means transfer preparation succeeded and the later opener
             * failed. Without one, the transfer/materialization itself failed.
             */
            if transfer.local_path.is_some() {
                self.show_error_message(format!(
                    "Unable to open {}: {}",
                    transfer.remote_path.display(),
                    error,
                ));
            } else {
                self.show_error_message(format!(
                    "Unable to prepare {} for opening: {}",
                    transfer.remote_path.display(),
                    error,
                ));
            }

            return;
        }

        if transfer.local_path.is_none() {
            self.show_error_message("Remote transfer completed without producing a local file");

            return;
        }

        /*
         * The file was already opened when the transfer finished.
         *
         * Acknowledgment now closes the completed transfer window and returns control
         * to Scry.
         */
        self.show_info_message(format!("Opened {}", transfer.remote_path.display(),));
    }

    pub fn deletion_visible(&self) -> bool {
        self.deletion.is_some()
    }

    pub fn begin_deletion_confirmation(&mut self) {
        /*
         * Deletion is a deliberately opt-in local feature.
         *
         * When disabled, the command must behave as though it does not exist.
         */
        if !self.enable_deletion {
            return;
        }

        /*
         * The first implementation is local-only.
         *
         * Remote deletion requires a separate FileSource operation and must not
         * accidentally act on Scry's downloaded cache copy.
         */
        if self.source.is_remote() {
            self.show_info_message("Deletion is not available while browsing through SSH");

            return;
        }

        /*
         * Never begin another modal operation while a transfer or connection is
         * active.
         */
        if self.transfer.is_some() || self.connection_in_progress {
            return;
        }

        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };

        let path = entry.path.clone();

        /*
         * Every deletable target must be an absolute entry beneath the current
         * browsing root.
         *
         * FileEntry paths originate from the filesystem scanner, but validating
         * them again here keeps the destructive boundary self-contained.
         */
        if !path.is_absolute() {
            self.show_error_message(format!(
                "Refusing to delete a non-absolute path: {}",
                path.display(),
            ));

            return;
        }

        if path == Path::new("/") {
            self.show_error_message("Refusing to delete the filesystem root");

            return;
        }

        if path == self.current_directory {
            self.show_error_message("Refusing to delete Scry's current browsing root");

            return;
        }

        if !path.starts_with(&self.current_directory) {
            self.show_error_message(format!(
                "Refusing to delete a path outside the current browsing root: {}",
                path.display(),
            ));

            return;
        }

        if path.file_name().is_none() {
            self.show_error_message(format!(
                "Refusing to delete a path without a filename: {}",
                path.display(),
            ));

            return;
        }

        /*
         * symlink_metadata() inspects the selected link itself rather than
         * following it to some other filesystem object.
         */
        if let Err(error) = std::fs::symlink_metadata(&path) {
            self.show_error_message(format!(
                "Unable to validate {} for deletion: {}",
                path.display(),
                error,
            ));

            return;
        }

        let directory_has_content =
            entry.is_directory && !entry.is_symlink && self.directory_has_content(&path);

        self.clear_messages();

        self.deletion = Some(DeletionState {
            path,

            name: entry.name,

            is_directory: entry.is_directory,

            is_symlink: entry.is_symlink,

            directory_has_content,

            /*
             * Cancel receives the initial focus.
             *
             * Merely pressing Delete followed by Enter must never destroy the
             * selected entry.
             */
            choice: DeletionChoice::Cancel,
        });
    }

    pub fn cancel_deletion(&mut self) {
        self.deletion = None;
    }

    pub fn select_deletion_choice(&mut self, choice: DeletionChoice) {
        let Some(deletion) = self.deletion.as_mut() else {
            return;
        };

        deletion.choice = choice;
    }

    pub fn toggle_deletion_choice(&mut self) {
        let Some(deletion) = self.deletion.as_mut() else {
            return;
        };

        deletion.choice = match deletion.choice {
            DeletionChoice::Delete => DeletionChoice::Cancel,

            DeletionChoice::Cancel => DeletionChoice::Delete,
        };
    }

    /*
     * Load staged deletions left by an interrupted earlier Scry process.
     *
     * Recovery is local-only. A malformed, unsupported, or inconsistent journal is
     * left untouched and rejected as a whole so Scry never acts on a partially
     * trusted transaction set.
     */
    pub fn recover_staged_deletions(&mut self) -> Result<usize, String> {
        if self.source.is_remote() {
            return Ok(0);
        }

        let Some(journal) = deletion_journal::load().map_err(|error| error.to_string())? else {
            return Ok(0);
        };

        if !journal.is_supported() {
            return Err(format!(
                "deletion journal format {} is unsupported; expected version {}",
                journal.version,
                deletion_journal::JOURNAL_FORMAT_VERSION,
            ));
        }

        if journal.entries.is_empty() {
            /*
             * save_entries() does not ordinarily leave an empty journal, but an
             * externally edited or older file may contain one.
             */
            deletion_journal::save_entries(&[]).map_err(|error| error.to_string())?;

            return Ok(0);
        }

        let mut original_paths = HashSet::new();

        let mut staged_paths = HashSet::new();

        for entry in &journal.entries {
            validate_journal_entry_paths(entry)?;

            if !original_paths.insert(entry.original_path.clone()) {
                return Err(format!(
                    "deletion journal contains duplicate original path {}",
                    entry.original_path.display(),
                ));
            }

            if !staged_paths.insert(entry.staged_path.clone()) {
                return Err(format!(
                    "deletion journal contains duplicate staged path {}",
                    entry.staged_path.display(),
                ));
            }
        }

        /*
         * Validate every currently reachable staged root.
         *
         * A missing entry is accepted only when a later staged real directory moved
         * that hidden pathname with its complete subtree.
         */
        for (index, entry) in journal.entries.iter().enumerate() {
            match std::fs::symlink_metadata(&entry.staged_path) {
                Ok(_) => {
                    validate_reachable_staged_object(entry)?;
                }

                Err(error)
                    if error.kind() == io::ErrorKind::NotFound
                        && containing_later_staged_directory(&journal.entries, index).is_some() => {
                }

                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Err(format!(
                        "staged path is missing and is not contained by a later staged directory: {}",
                        entry.staged_path.display(),
                    ));
                }

                Err(error) => {
                    return Err(format!(
                        "unable to inspect staged path {}: {}",
                        entry.staged_path.display(),
                        error,
                    ));
                }
            }
        }

        /*
         * Do not merge with a non-empty live stack. Recovery is called during
         * startup before the user can stage any new deletion.
         */
        if !self.staged_deletions.is_empty() {
            return Err("cannot load deletion recovery into a non-empty undo stack".to_string());
        }

        self.staged_deletions = journal
            .entries
            .iter()
            .map(StagedDeletion::from_journal_entry)
            .collect();

        let recovered_count = self.staged_deletions.len();

        /*
         * Hide recovered staged paths from the already loaded immediate directory.
         */
        self.refresh_filter();

        Ok(recovered_count)
    }

    /*
     * Restore the most recently staged deletion.
     *
     * Staged deletions form a last-in, first-out undo stack:
     *
     *     delete A
     *     delete B
     *     Ctrl+Z restores B
     *     Ctrl+Z restores A
     *
     * The transaction remains recorded unless the filesystem rename succeeds.
     */
    pub fn restore_last_deletion(&mut self) {
        if self.source.is_remote() {
            self.show_info_message("Deletion undo is available only for local files");

            return;
        }

        let Some(deletion) = self.staged_deletions.last().cloned() else {
            self.show_info_message("Nothing to restore");

            return;
        };

        /*
         * Never overwrite an entry recreated at the original pathname.
         *
         * symlink_metadata() checks the pathname itself, so a dangling symbolic link
         * also counts as an occupied restoration destination.
         */
        match std::fs::symlink_metadata(&deletion.original_path) {
            Ok(_) => {
                self.show_error_message(format!(
                    "Unable to restore {} because that path already exists",
                    deletion.original_path.display(),
                ));

                return;
            }

            Err(error) if error.kind() == io::ErrorKind::NotFound => {}

            Err(error) => {
                self.show_error_message(format!(
                    "Unable to validate restoration path {}: {}",
                    deletion.original_path.display(),
                    error,
                ));

                return;
            }
        }

        /*
         * Confirm that the staged pathname still exists and still represents the
         * object kind recorded when deletion was staged.
         *
         * This guards against an external process replacing the hidden staged entry
         * while Scry remains open.
         */
        let staged_metadata = match std::fs::symlink_metadata(&deletion.staged_path) {
            Ok(metadata) => metadata,

            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.show_error_message(format!(
                    "Unable to restore {} because its staged copy is missing",
                    deletion.original_path.display(),
                ));

                return;
            }

            Err(error) => {
                self.show_error_message(format!(
                    "Unable to inspect staged deletion {}: {}",
                    deletion.staged_path.display(),
                    error,
                ));

                return;
            }
        };

        let staged_file_type = staged_metadata.file_type();

        let staged_is_symlink = staged_file_type.is_symlink();

        let staged_is_directory = staged_file_type.is_dir() && !staged_is_symlink;

        if staged_is_symlink != deletion.is_symlink || staged_is_directory != deletion.is_directory
        {
            self.show_error_message(format!(
                "Unable to restore {} because its staged object type has changed",
                deletion.original_path.display(),
            ));

            return;
        }

        /*
         * Both paths are siblings, so restoration ordinarily remains an atomic
         * same-filesystem metadata operation.
         *
         * Files, directories, and symbolic links all use rename(); symbolic links
         * are moved as links and are never followed.
         */
        if let Err(error) = std::fs::rename(&deletion.staged_path, &deletion.original_path) {
            self.show_error_message(format!(
                "Unable to restore {}: {}",
                deletion.original_path.display(),
                error,
            ));

            return;
        }

        /*
         * Remove the restored transaction and publish the reduced journal.
         *
         * If publication fails, move the object back into its staged pathname and put
         * the transaction back on the undo stack. This preserves recoverability rather
         * than leaving the journal and filesystem knowingly inconsistent.
         */
        self.staged_deletions.pop();

        let journal_entries = staged_deletion_journal_entries(&self.staged_deletions);

        if let Err(journal_error) = deletion_journal::save_entries(&journal_entries) {
            match std::fs::rename(&deletion.original_path, &deletion.staged_path) {
                Ok(()) => {
                    self.staged_deletions.push(deletion.clone());

                    self.show_error_message(format!(
                        "Unable to update the deletion journal while restoring {}: {}. The restoration was rolled back.",
                        deletion.original_path.display(),
                        journal_error,
                    ));
                }

                Err(rollback_error) => {
                    /*
                     * The object is restored at its original path, but the journal still
                     * describes the old staged pathname. Preserve the truthful in-memory
                     * state and report both failures prominently.
                     */
                    self.show_error_message(format!(
                        "CRITICAL: {} was restored, but the deletion journal could not be updated: {}; rollback to {} also failed: {}",
                        deletion.original_path.display(),
                        journal_error,
                        deletion.staged_path.display(),
                        rollback_error,
                    ));
                }
            }

            return;
        }

        /*
         * Preserve the current visual position where practical. The restored path
         * becomes the preferred selection only when it belongs to the active browser
         * root.
         */
        let previous_view_mode = self.view_mode;

        let previous_selected = self.selected;

        let previous_offset = self.list_offset;

        let restored_path = deletion.original_path;

        let restored_is_inside_current_root = restored_path.starts_with(&self.current_directory);

        let entries = match self.source.read_directory(
            &self.current_directory,
            self.sort_mode,
            self.sort_descending,
        ) {
            Ok(entries) => entries,

            Err(error) => {
                /*
                 * The filesystem restoration has already succeeded. Keep the success
                 * truthful while explaining that only Scry's current view is stale.
                 */
                self.show_error_message(format!(
                    "{} was restored, but Scry could not refresh {}: {}",
                    restored_path.display(),
                    self.current_directory.display(),
                    error,
                ));

                return;
            }
        };

        self.entries = entries;

        /*
         * Recursive indexes and Tree caches may describe the filesystem state from
         * before restoration.
         */
        self.invalidate_recursive_cache();

        self.directory_has_content_cache.clear();

        self.classification_inspection_cache
            .retain(|cached_path, _| {
                cached_path != &restored_path && !cached_path.starts_with(&restored_path)
            });

        self.search_return_state = None;

        self.pending_selection_path =
            restored_is_inside_current_root.then_some(restored_path.clone());

        self.selected = previous_selected;

        self.list_offset = previous_offset;

        match previous_view_mode {
            ViewMode::List => {
                if self.recursive_search_active() {
                    self.ensure_recursive_scan();
                }

                self.refresh_filter();

                self.restore_pending_selection_if_available();
            }

            ViewMode::Tree if self.recursive_search_active() => {
                self.tree_rows.clear();

                self.filtered_tree_indices.clear();

                self.search_tree_children.clear();

                self.ensure_recursive_scan();

                if !self.scan_in_progress {
                    self.rebuild_recursive_search_tree(self.pending_selection_path.clone());

                    self.restore_pending_selection_if_available();
                }
            }

            ViewMode::Tree => {
                /*
                 * Ordinary Tree data may contain a cached copy of the restored
                 * entry's parent directory. Rebuild from the live filesystem rather
                 * than attempting to patch that cache in place.
                 */
                self.view_mode = ViewMode::Tree;

                self.reset_tree();

                self.restore_pending_selection_if_available();
            }
        }

        if !self.recursive_search_active() {
            if restored_is_inside_current_root {
                self.select_visible_path(&restored_path);
            }

            self.pending_selection_path = None;
        }

        self.ensure_selection_visible(self.viewport_rows);

        self.show_info_message(format!("Restored {}", restored_path.display(),));
    }

    /*
     * Permanently remove every deletion still staged when Scry exits cleanly.
     *
     * Only outermost staged roots are removed directly. A staged directory may
     * contain entries that were staged earlier while the directory still had its
     * original name. Removing that outer directory recursively also finalizes those
     * nested transactions.
     *
     * Returns the number of recorded transactions finalized and a list of failures
     * that must be reported after terminal mode has been restored.
     */
    pub fn finalize_staged_deletions(&mut self) -> (usize, Vec<String>) {
        if self.staged_deletions.is_empty() {
            return (0, Vec::new());
        }

        /*
         * A transaction is nested when its staged pathname originally lived beneath
         * another transaction's original directory.
         *
         * Example:
         *
         *     /test/file.txt
         *         -> /test/.scry-deleted-...-file.txt
         *
         * followed by:
         *
         *     /test
         *         -> /.scry-deleted-...-test
         *
         * The file's hidden path moved together with /test. Deleting the outer
         * staged directory therefore finalizes both records.
         */
        let root_indices: Vec<usize> = self
            .staged_deletions
            .iter()
            .enumerate()
            .filter_map(|(index, deletion)| {
                let nested_inside_another_deletion =
                    self.staged_deletions
                        .iter()
                        .enumerate()
                        .any(|(other_index, other)| {
                            index != other_index
                                && deletion.staged_path.starts_with(&other.original_path)
                        });

                (!nested_inside_another_deletion).then_some(index)
            })
            .collect();

        let mut finalized_records = vec![false; self.staged_deletions.len()];

        let mut failures = Vec::new();

        for root_index in root_indices {
            let root = &self.staged_deletions[root_index];

            /*
             * Verify that the staged root still exists and retains the object kind
             * recorded when it was renamed.
             *
             * symlink_metadata() inspects symbolic links themselves rather than
             * following their targets.
             */
            let metadata = match std::fs::symlink_metadata(&root.staged_path) {
                Ok(metadata) => metadata,

                Err(error) => {
                    failures.push(format!(
                        "unable to permanently delete {} from staged path {}: {}",
                        root.original_path.display(),
                        root.staged_path.display(),
                        error,
                    ));

                    continue;
                }
            };

            let file_type = metadata.file_type();

            let staged_is_symlink = file_type.is_symlink();

            let staged_is_directory = file_type.is_dir() && !staged_is_symlink;

            if staged_is_symlink != root.is_symlink || staged_is_directory != root.is_directory {
                failures.push(format!(
                    "refused to permanently delete {} because its staged object type changed",
                    root.original_path.display(),
                ));

                continue;
            }

            /*
             * Symbolic links are always removed as links. Real directories are
             * removed recursively; every other filesystem object uses remove_file().
             */
            let deletion_result = if root.is_directory {
                std::fs::remove_dir_all(&root.staged_path)
            } else {
                std::fs::remove_file(&root.staged_path)
            };

            if let Err(error) = deletion_result {
                failures.push(format!(
                    "unable to permanently delete {} from staged path {}: {}",
                    root.original_path.display(),
                    root.staged_path.display(),
                    error,
                ));

                continue;
            }

            /*
             * The successfully removed root also consumed every transaction whose
             * hidden staged pathname originally lived below that root's original
             * directory.
             */
            for (index, deletion) in self.staged_deletions.iter().enumerate() {
                if index == root_index || deletion.staged_path.starts_with(&root.original_path) {
                    finalized_records[index] = true;
                }
            }
        }

        let finalized_count = finalized_records
            .iter()
            .filter(|finalized| **finalized)
            .count();

        /*
         * Retain only failed transactions. This is mainly useful for truthful state
         * during orderly shutdown and future callers; the process is about to exit.
         */
        let mut index = 0_usize;

        self.staged_deletions.retain(|_| {
            let retain = !finalized_records[index];

            index = index.saturating_add(1);

            retain
        });

        /*
         * Persist only transactions that could not be finalized.
         *
         * An empty stack removes the journal completely.
         */
        let journal_entries = staged_deletion_journal_entries(&self.staged_deletions);

        if let Err(error) = deletion_journal::save_entries(&journal_entries) {
            failures.push(format!(
                "unable to update the deletion journal after finalization: {}",
                error,
            ));
        }

        (finalized_count, failures)
    }

    pub fn confirm_deletion(&mut self) {
        let Some(deletion) = self.deletion.take() else {
            return;
        };

        /*
         * Enter on the default Cancel choice is always harmless.
         */
        if deletion.choice != DeletionChoice::Delete {
            return;
        }

        /*
         * Repeat the destructive-boundary checks immediately before removal.
         *
         * The confirmation state may have remained open while the filesystem
         * changed outside Scry.
         */
        if !self.enable_deletion {
            return;
        }

        if self.source.is_remote() {
            self.error_message =
                Some("Deletion is not available while browsing through SSH".to_string());

            return;
        }

        let path = deletion.path;

        if !path.is_absolute() {
            self.error_message = Some(format!(
                "Refusing to delete a non-absolute path: {}",
                path.display(),
            ));

            return;
        }

        if path == Path::new("/") {
            self.error_message = Some("Refusing to delete the filesystem root".to_string());

            return;
        }

        if path == self.current_directory {
            self.error_message =
                Some("Refusing to delete Scry's current browsing root".to_string());

            return;
        }

        if !path.starts_with(&self.current_directory) {
            self.error_message = Some(format!(
                "Refusing to delete a path outside the current browsing root: {}",
                path.display(),
            ));

            return;
        }

        if path.file_name().is_none() {
            self.error_message = Some(format!(
                "Refusing to delete a path without a filename: {}",
                path.display(),
            ));

            return;
        }

        /*
         * symlink_metadata() examines the link itself.
         *
         * A symlink pointing to a directory must be removed with remove_file(),
         * never followed into its target with remove_dir_all().
         */
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,

            Err(error) => {
                self.show_error_message(format!(
                    "Unable to validate {} for deletion: {}",
                    path.display(),
                    error,
                ));

                return;
            }
        };

        let file_type = metadata.file_type();

        let is_symlink = file_type.is_symlink();

        let is_directory = file_type.is_dir() && !is_symlink;

        /*
         * Generate the private hidden sibling path before changing the filesystem.
         *
         * Failure here leaves the original entry completely untouched.
         */
        let staged_path = match staged_deletion_path(&path) {
            Ok(staged_path) => staged_path,

            Err(error) => {
                self.show_error_message(format!(
                    "Unable to prepare {} for staged deletion: {}",
                    path.display(),
                    error,
                ));

                return;
            }
        };

        /*
         * Rename rather than remove.
         *
         * Because the staged path resides beside the original entry, this ordinarily
         * remains an atomic same-filesystem metadata operation. Files, directories,
         * and symbolic links all use the same rename operation; symbolic links are
         * moved as links and are never followed.
         */
        if let Err(error) = std::fs::rename(&path, &staged_path) {
            self.show_error_message(format!(
                "Unable to stage {} for deletion: {}",
                path.display(),
                error,
            ));

            return;
        }

        /*
         * Record the transaction immediately after the successful rename.
         *
         * The in-memory stack is then published atomically to the persistent journal
         * before Scry treats the deletion as accepted.
         */
        self.staged_deletions.push(StagedDeletion {
            original_path: path.clone(),

            staged_path: staged_path.clone(),

            is_directory,

            is_symlink,
        });

        let journal_entries = staged_deletion_journal_entries(&self.staged_deletions);

        if let Err(journal_error) = deletion_journal::save_entries(&journal_entries) {
            /*
             * Journal publication failed after the filesystem rename.
             *
             * Roll the rename back immediately so Scry never knowingly leaves an
             * unjournaled hidden deletion behind.
             */
            self.staged_deletions.pop();

            match std::fs::rename(&staged_path, &path) {
                Ok(()) => {
                    self.show_error_message(format!(
                        "Unable to record staged deletion for {}: {}. The deletion was cancelled.",
                        path.display(),
                        journal_error,
                    ));
                }

                Err(rollback_error) => {
                    self.show_error_message(format!(
                        "CRITICAL: unable to record staged deletion for {}: {}; rollback from {} also failed: {}",
                        path.display(),
                        journal_error,
                        staged_path.display(),
                        rollback_error,
                    ));
                }
            }

            return;
        }

        /*
         * A successfully deleted file must not remain in the persistent batch
         * selection.
         *
         * This occurs before refreshing the directory because the filesystem removal
         * has already succeeded even if the subsequent refresh happens to fail.
         */
        self.marked_files.remove(&path);

        /*
         * Prefer the closest surviving row above the deleted entry.
         *
         * When no row exists above it, use the closest surviving row below it. A
         * descendant of a deleted directory is not a valid fallback because it will
         * disappear together with that directory.
         */
        let preferred_selection =
            match self.view_mode {
                ViewMode::List => {
                    let above = (0..self.selected).rev().find_map(|position| {
                        let entry = self.entry_at_filtered_position(position)?;

                        if entry.path != path && !entry.path.starts_with(&path) {
                            Some(entry.path.clone())
                        } else {
                            None
                        }
                    });

                    above.or_else(|| {
                        (self.selected.saturating_add(1)..self.current_visible_entry_count())
                            .find_map(|position| {
                                let entry = self.entry_at_filtered_position(position)?;

                                if entry.path != path && !entry.path.starts_with(&path) {
                                    Some(entry.path.clone())
                                } else {
                                    None
                                }
                            })
                    })
                }

                ViewMode::Tree => {
                    let above = (0..self.selected).rev().find_map(|position| {
                        let row = self.tree_row_at_filtered_position(position)?;

                        if row.entry.path != path && !row.entry.path.starts_with(&path) {
                            Some(row.entry.path.clone())
                        } else {
                            None
                        }
                    });

                    above.or_else(|| {
                        (self.selected.saturating_add(1)..self.current_visible_entry_count())
                            .find_map(|position| {
                                let row = self.tree_row_at_filtered_position(position)?;

                                if row.entry.path != path && !row.entry.path.starts_with(&path) {
                                    Some(row.entry.path.clone())
                                } else {
                                    None
                                }
                            })
                    })
                }
            };

        let previous_offset = self.list_offset;

        let previous_viewport_row = self.selected.saturating_sub(self.list_offset);

        let previous_view_mode = self.view_mode;

        /*
         * Preserve the visual Tree state before recursive invalidation or cache
         * replacement removes it.
         */
        let saved_expanded_directories = self.expanded_directories.clone();

        let saved_recursive_expanded_directories = self.recursive_expanded_directories.clone();

        let saved_search_collapsed_directories = self.search_collapsed_directories.clone();

        let mut saved_tree_children = self.tree_children.clone();

        let entries = match self.source.read_directory(
            &self.current_directory,
            self.sort_mode,
            self.sort_descending,
        ) {
            Ok(entries) => entries,

            Err(error) => {
                self.show_error_message(format!(
                    "{} was staged for deletion, but Scry could not refresh {}: {}",
                    path.display(),
                    self.current_directory.display(),
                    error,
                ));

                return;
            }
        };

        self.entries = entries;

        /*
         * Ordinary Tree mode stores loaded children separately from the current root
         * listing. Refresh only the deleted entry's parent and retain every other
         * already-loaded branch.
         */
        if previous_view_mode == ViewMode::Tree && !self.recursive_search_active() {
            if let Some(parent) = path.parent() {
                let refreshed_children = if parent == self.current_directory {
                    Some(self.entries.clone())
                } else {
                    match self
                        .source
                        .read_directory(parent, self.sort_mode, self.sort_descending)
                    {
                        Ok(children) => Some(children),

                        Err(error) => {
                            self.show_error_message(format!(
                                "{} was deleted, but Scry could not refresh its Tree branch: {}",
                                path.display(),
                                error,
                            ));

                            None
                        }
                    }
                };

                if let Some(children) = refreshed_children {
                    saved_tree_children.insert(parent.to_path_buf(), children);
                }
            }

            /*
             * No cached directory beneath the removed path can still be valid.
             */
            saved_tree_children
                .retain(|directory, _| directory != &path && !directory.starts_with(&path));
        }

        /*
         * Every recursive representation may still contain the removed path or one
         * of its descendants.
         */
        self.invalidate_recursive_cache();

        self.directory_has_content_cache.clear();

        self.classification_inspection_cache
            .retain(|cached_path, _| cached_path != &path && !cached_path.starts_with(&path));

        self.search_return_state = None;

        self.pending_selection_path = preferred_selection.clone();

        self.selected = 0;

        self.list_offset = previous_offset;

        match previous_view_mode {
            ViewMode::List => {
                if self.recursive_search_active() {
                    /*
                     * Restore queryless/search Tree state before the replacement scan.
                     * The rebuilt corpus will discard paths that no longer exist.
                     */
                    self.recursive_expanded_directories = saved_recursive_expanded_directories;

                    self.search_collapsed_directories = saved_search_collapsed_directories;

                    self.ensure_recursive_scan();
                }

                self.refresh_filter();

                self.restore_pending_selection_if_available();
            }

            ViewMode::Tree if self.recursive_search_active() => {
                /*
                 * Recursive Tree state survives the rescan. Remove only the deleted
                 * branch and its descendants from the preserved sets.
                 */
                self.recursive_expanded_directories = saved_recursive_expanded_directories;

                self.recursive_expanded_directories.retain(|expanded_path| {
                    expanded_path != &path && !expanded_path.starts_with(&path)
                });

                self.search_collapsed_directories = saved_search_collapsed_directories;

                self.search_collapsed_directories.retain(|collapsed_path| {
                    collapsed_path != &path && !collapsed_path.starts_with(&path)
                });

                self.tree_rows.clear();

                self.filtered_tree_indices.clear();

                self.search_tree_children.clear();

                self.ensure_recursive_scan();

                /*
                 * A complete resident corpus can be rebuilt immediately. Otherwise the
                 * asynchronous scan restores the pending selection when it completes.
                 */
                if !self.scan_in_progress {
                    self.rebuild_recursive_search_tree(preferred_selection.clone());

                    self.restore_pending_selection_if_available();
                }
            }

            ViewMode::Tree => {
                /*
                 * Ordinary Tree mode does not require a full reset. Reuse every loaded
                 * branch except the deleted subtree, then rebuild the same open shape.
                 */
                self.view_mode = ViewMode::Tree;

                self.tree_children = saved_tree_children;

                self.expanded_directories = saved_expanded_directories;

                self.expanded_directories.retain(|expanded_path| {
                    expanded_path != &path && !expanded_path.starts_with(&path)
                });

                self.rebuild_tree_rows(preferred_selection.clone());

                self.restore_pending_selection_if_available();
            }
        }

        if !self.recursive_search_active() {
            /*
             * Ordinary List and Tree rebuilding is synchronous.
             *
             * Apply the surviving neighbor only after the final filtered indices exist,
             * then restore the previous viewport. This avoids the temporary selected = 0
             * state becoming the visible result.
             */
            if let Some(path) = preferred_selection.as_ref() {
                self.select_visible_path(path);
            } else {
                self.selected = self
                    .selected
                    .min(self.current_visible_entry_count().saturating_sub(1));
            }

            self.pending_selection_path = None;

            self.list_offset = self.selected.saturating_sub(previous_viewport_row);
        } else if self.pending_selection_path.is_none() {
            /*
             * A completed recursive rebuild may already have restored the path.
             */
            self.list_offset = self.selected.saturating_sub(previous_viewport_row);
        }

        self.ensure_selection_visible(self.viewport_rows);

        self.show_info_message(format!("Deleted {}", path.display(),));
    }

    pub fn activate_selected(&mut self) {
        let Some(entry) = self.selected_entry() else {
            return;
        };

        let path = entry.path.clone();

        let entry_is_directory = entry.is_directory;

        let is_directory = self.path_is_directory(&path, entry_is_directory);

        /*
         * Remember the complete search before entering a directory result.
         */
        if self.recursive_search_active()
            && !self.query.is_empty()
            && self.query != "."
            && is_directory
        {
            self.save_search_return_state(path.clone());
        }

        /*
         * Enter on a directory:
         *
         * Works as → (right) and will enter the directory inside Scry.
         */
        if is_directory {
            if self.view_mode == ViewMode::Tree {
                self.enter_selected_tree_directory_as_root();
            } else {
                self.enter_selected_directory();
            }

            return;
        }

        /*
         * --no-open blocks only external file activation.
         *
         * Directory navigation remains fully functional.
         */
        if !self.allow_file_opening {
            self.show_info_message(format!("File opening is disabled — {}", path.display(),));

            return;
        }

        /*
         * Enter on a file opens that file immediately, including recursive
         * List and Tree results.
         */
        if self.source.is_remote() {
            let total_bytes = self
                .selected_entry()
                .map(|entry| entry.size_bytes)
                .unwrap_or(0);

            self.begin_remote_transfer(path, total_bytes);

            return;
        }

        /*
         * Local files need no transfer popup.
         */
        let mut ignore_progress = |_progress: TransferProgress| -> io::Result<TransferControl> {
            Ok(TransferControl::Continue)
        };

        let local_open_path = match self.source.materialize_file(&path, &mut ignore_progress) {
            Ok(local_path) => local_path,

            Err(error) => {
                self.show_error_message(format!(
                    "Unable to prepare {} for opening: {}",
                    path.display(),
                    error,
                ));

                return;
            }
        };

        match crate::open::open_file(&local_open_path) {
            Ok(()) => {
                if self.exit_on_open {
                    self.should_quit = true;
                } else {
                    self.show_info_message(format!("Opened {}", path.display()));
                }
            }

            Err(error) => {
                self.show_error_message(error);
            }
        }
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    pub fn remote_index_setup_visible(&self) -> bool {
        self.overlay == Overlay::RemoteIndexSetup && self.remote_index_setup.is_some()
    }

    pub fn remote_index_dialog_next_focus(&mut self) {
        let Some(setup) = self.remote_index_setup.as_mut() else {
            return;
        };

        setup.focus = setup.focus.next();
    }

    pub fn remote_index_dialog_previous_focus(&mut self) {
        let Some(setup) = self.remote_index_setup.as_mut() else {
            return;
        };

        setup.focus = setup.focus.previous();
    }

    pub fn select_remote_index_dialog_focus(&mut self, focus: RemoteIndexDialogFocus) {
        let Some(setup) = self.remote_index_setup.as_mut() else {
            return;
        };

        setup.focus = focus;
    }

    pub fn select_remote_index_policy(&mut self, includes_hidden: bool) {
        let Some(setup) = self.remote_index_setup.as_mut() else {
            return;
        };

        setup.includes_hidden = includes_hidden;
    }

    pub fn toggle_remote_index_policy(&mut self) {
        let Some(setup) = self.remote_index_setup.as_mut() else {
            return;
        };

        setup.includes_hidden = !setup.includes_hidden;

        setup.focus = RemoteIndexDialogFocus::Policy;
    }

    fn begin_remote_index_build(&mut self, includes_hidden: bool) {
        if self.remote_index_build_in_progress {
            return;
        }

        let receiver = match self.source.start_remote_index_build(includes_hidden) {
            Ok(receiver) => receiver,

            Err(error) => {
                self.pending_remote_index_hidden_policy = None;

                self.show_error_message(format!("Unable to start remote indexing: {}", error,));

                return;
            }
        };

        self.remote_index_entries_written = 0;

        self.remote_index_build_in_progress = true;

        self.remote_index_build_receiver = Some(receiver);

        self.show_persistent_info_message("Building remote index from /…");
    }

    pub fn open_remote_index_builder(&mut self) {
        if !self.source.is_remote() {
            self.show_info_message("Remote indexes are available only for SSH connections");

            return;
        }

        if self.remote_index_build_in_progress {
            self.error_message = None;

            self.show_info_message(format!(
                "Remote index is already building — {} entries written",
                self.remote_index_entries_written,
            ));

            return;
        }

        if self.remote_index_load_in_progress {
            self.error_message = None;

            self.show_info_message("Wait for the current remote index to finish loading");

            return;
        }

        let Some(identity) = self.source.remote_index_identity() else {
            self.error_message = None;

            self.show_error_message("The current SSH source has no remote-index identity");

            return;
        };

        /*
         * Begin with the policy of the currently loaded index where possible.
         *
         * Otherwise default to standard entries.
         */
        let includes_hidden = if self.remote_index_loaded {
            self.remote_index_includes_hidden
        } else {
            match identity.inspect() {
                Ok(crate::remote_index::RemoteIndexStatus::Valid(info)) => info.includes_hidden,

                _ => false,
            }
        };

        self.remote_index_setup = Some(RemoteIndexSetupState {
            identity,

            purpose: RemoteIndexDialogPurpose::Rebuild,

            includes_hidden,

            focus: RemoteIndexDialogFocus::Policy,

            invalid_reason: None,
        });

        self.overlay = Overlay::RemoteIndexSetup;

        self.clear_messages();
    }

    pub fn close_remote_index_setup(&mut self) {
        self.remote_index_setup = None;

        self.overlay = Overlay::None;
    }

    pub fn confirm_remote_index_setup(&mut self) {
        let Some((focus, includes_hidden)) = self
            .remote_index_setup
            .as_ref()
            .map(|setup| (setup.focus, setup.includes_hidden))
        else {
            self.overlay = Overlay::None;

            return;
        };

        match focus {
            RemoteIndexDialogFocus::Policy => {
                /*
                 * Enter while the policy group has focus changes the selected
                 * radio option but never begins the index build.
                 */
                self.toggle_remote_index_policy();
            }

            RemoteIndexDialogFocus::Ok => {
                self.pending_remote_index_hidden_policy = Some(includes_hidden);

                self.remote_index_setup = None;

                self.overlay = Overlay::None;

                self.begin_remote_index_build(includes_hidden);
            }

            RemoteIndexDialogFocus::Cancel => {
                self.close_remote_index_setup();
            }
        }
    }

    pub fn connection_visible(&self) -> bool {
        self.overlay == Overlay::Connection
    }

    pub fn toggle_connection_dialog(&mut self) {
        self.remote_index_setup = None;

        if self.connection_visible() {
            self.close_connection_dialog();

            return;
        }

        self.connection_dialog
            .load_selected_profile(&self.connection_store);

        self.overlay = Overlay::Connection;
    }

    pub fn set_connection_focus(&mut self, field: crate::connection::ConnectionField) {
        self.connection_dialog.set_focus(field);
    }

    pub fn connection_focus_next(&mut self) {
        /*
         * Twelve distinct controls exist. The bound prevents an accidental
         * infinite loop if every optional control were ever disabled.
         */
        for _ in 0..12 {
            self.connection_dialog.focus_next();

            if self.connection_focus_is_enabled() {
                break;
            }
        }
    }

    pub fn connection_focus_previous(&mut self) {
        for _ in 0..12 {
            self.connection_dialog.focus_previous();

            if self.connection_focus_is_enabled() {
                break;
            }
        }
    }

    pub fn connection_previous_profile(&mut self) {
        let profile_count = self.connection_store.profiles().len();

        if profile_count == 0 {
            return;
        }

        self.connection_dialog.selected_profile = if self.connection_dialog.selected_profile == 0 {
            profile_count - 1
        } else {
            self.connection_dialog.selected_profile - 1
        };

        self.connection_dialog
            .load_selected_profile(&self.connection_store);

        self.connection_dialog.focus = crate::connection::ConnectionField::Profiles;
    }

    pub fn connection_next_profile(&mut self) {
        let profile_count = self.connection_store.profiles().len();

        if profile_count == 0 {
            return;
        }

        self.connection_dialog.selected_profile =
            (self.connection_dialog.selected_profile + 1) % profile_count;

        self.connection_dialog
            .load_selected_profile(&self.connection_store);

        self.connection_dialog.focus = crate::connection::ConnectionField::Profiles;
    }

    fn connection_focus_is_enabled(&self) -> bool {
        use crate::connection::ConnectionField;

        match self.connection_dialog.focus {
            /*
             * The saved-profile selector has nothing to select on first use.
             */
            ConnectionField::Profiles => !self.connection_store.profiles().is_empty(),

            /*
             * Delete needs an existing saved profile.
             */
            ConnectionField::Delete => !self.connection_store.profiles().is_empty(),

            /*
             * Disconnect is meaningful only while browsing through SSH.
             */
            ConnectionField::Disconnect => self.source.is_remote(),

            _ => true,
        }
    }

    pub fn connection_push_character(&mut self, character: char) {
        self.connection_dialog.push_character(character);
    }

    #[allow(dead_code)]
    pub fn connection_pop_character(&mut self) {
        self.connection_dialog.pop_character();
    }

    pub fn connection_clear_field(&mut self) {
        self.connection_dialog.clear_focused_field();
    }

    pub fn connection_move_cursor_left(&mut self) {
        self.connection_dialog.move_cursor_left();
    }

    pub fn connection_move_cursor_right(&mut self) {
        self.connection_dialog.move_cursor_right();
    }

    pub fn connection_move_cursor_to_start(&mut self) {
        self.connection_dialog.move_cursor_to_start();
    }

    pub fn connection_move_cursor_to_end(&mut self) {
        self.connection_dialog.move_cursor_to_end();
    }

    pub fn save_connection_profile(&mut self) {
        let profile = match self.connection_dialog.completed_profile() {
            Ok(profile) => profile,

            Err(message) => {
                self.connection_dialog.error_message = Some(message);

                return;
            }
        };

        match self.connection_store.save_profile(profile) {
            Ok(index) => {
                self.connection_dialog.selected_profile = index;

                self.connection_dialog
                    .load_selected_profile(&self.connection_store);

                self.connection_dialog.focus = crate::connection::ConnectionField::Save;

                self.connection_dialog.error_message = Some("Profile saved".to_string());
            }

            Err(message) => {
                self.connection_dialog.error_message =
                    Some(format!("Unable to save profile: {}", message,));
            }
        }
    }

    pub fn delete_connection_profile(&mut self) {
        let profile_count = self.connection_store.profiles().len();

        if profile_count == 0 {
            self.connection_dialog.error_message =
                Some("There is no saved profile to delete".to_string());

            return;
        }

        let selected_profile = self
            .connection_dialog
            .selected_profile
            .min(profile_count.saturating_sub(1));

        let removed_name = self
            .connection_store
            .profile(selected_profile)
            .map(|profile| profile.name.clone())
            .unwrap_or_else(|| "profile".to_string());

        match self.connection_store.remove_profile(selected_profile) {
            Ok(Some(_)) => {
                let remaining_profiles = self.connection_store.profiles().len();

                if remaining_profiles == 0 {
                    /*
                     * load_selected_profile() resets the draft and moves focus to
                     * Profile name when the final saved profile disappears.
                     */
                    self.connection_dialog.selected_profile = 0;

                    self.connection_dialog
                        .load_selected_profile(&self.connection_store);

                    self.connection_dialog.error_message =
                        Some(format!("Profile '{}' deleted", removed_name));
                } else {
                    /*
                     * If the final item was removed, move to the new final index.
                     * Otherwise retain the same position, which now points to the
                     * profile that followed the deleted one.
                     */
                    self.connection_dialog.selected_profile =
                        selected_profile.min(remaining_profiles.saturating_sub(1));

                    self.connection_dialog
                        .load_selected_profile(&self.connection_store);

                    self.connection_dialog.focus = crate::connection::ConnectionField::Delete;

                    self.connection_dialog.error_message =
                        Some(format!("Profile '{}' deleted", removed_name));
                }
            }

            Ok(None) => {
                self.connection_dialog.error_message =
                    Some("The selected profile no longer exists".to_string());

                self.connection_dialog
                    .load_selected_profile(&self.connection_store);
            }

            Err(error) => {
                self.connection_dialog.error_message =
                    Some(format!("Unable to delete profile: {}", error));
            }
        }
    }

    pub fn begin_connection(&mut self) {
        if self.connection_in_progress {
            return;
        }

        let profile = match self.connection_dialog.completed_profile() {
            Ok(profile) => profile,

            Err(message) => {
                self.connection_dialog.error_message = Some(message);

                return;
            }
        };

        let identity_file = match expand_local_identity_path(&profile.identity_file) {
            Ok(path) => path,

            Err(message) => {
                self.connection_dialog.error_message = Some(message);

                return;
            }
        };

        let target = SshTarget {
            host: profile.host.clone(),

            user: if profile.username.is_empty() {
                None
            } else {
                Some(profile.username.clone())
            },

            port: profile.port,

            identity_file,
        };

        let start_directory = profile.start_directory.clone();

        let sort_mode = self.sort_mode;

        let sort_descending = self.sort_descending;

        let ssh_config: SshConfig = self.ssh_config;

        let (sender, receiver) = mpsc::channel();

        thread::spawn(move || {
            let result = connect_profile_worker(
                target,
                start_directory,
                sort_mode,
                sort_descending,
                ssh_config,
            );

            let _ = sender.send(ConnectionWorkerResult { result });
        });

        self.connection_receiver = Some(receiver);

        self.connection_in_progress = true;

        self.connection_dialog.error_message =
            Some(format!("Connecting to {}…", profile.destination_label(),));
    }

    pub fn process_connection_messages(&mut self) -> bool {
        if !self.connection_in_progress {
            return false;
        }

        let message = match self.connection_receiver.as_ref() {
            Some(receiver) => match receiver.try_recv() {
                Ok(message) => Some(message),

                Err(TryRecvError::Empty) => {
                    return false;
                }

                Err(TryRecvError::Disconnected) => {
                    self.connection_in_progress = false;

                    self.connection_receiver = None;

                    self.connection_dialog.error_message =
                        Some("SSH connection worker stopped unexpectedly".to_string());

                    return true;
                }
            },

            None => {
                self.connection_in_progress = false;

                return false;
            }
        };

        self.connection_receiver = None;

        self.connection_in_progress = false;

        let Some(message) = message else {
            return false;
        };

        match message.result {
            Ok(success) => {
                self.install_connected_source(success);

                self.overlay = Overlay::None;

                self.show_info_message(format!("Connected — {}", self.source_label(),));
            }

            Err(message) => {
                self.connection_dialog.error_message = Some(message);
            }
        }

        true
    }

    fn install_connected_source(&mut self, success: ConnectionWorkerSuccess) {
        let ConnectionWorkerSuccess {
            source,
            target,
            directory,
            home_directory,
            entries,
        } = success;

        self.search_return_state = None;

        self.marked_files.clear();

        /*
         * Preserve the local browser position only when leaving a local source.
         *
         * Connecting from one SSH host to another must not overwrite the original
         * local session to which Disconnect should eventually return.
         */
        if !self.source.is_remote() && self.saved_local_session.is_none() {
            self.saved_local_session = Some(LocalSessionState {
                directory: self.current_directory.clone(),

                home_directory: self.home_directory.clone(),

                selected_path: self.selected_entry().map(|entry| entry.path.clone()),

                list_offset: self.list_offset,

                query: self.query.clone(),

                view_mode: self.view_mode,

                recursive_mode: self.recursive_mode,

                search_mode: self.search_mode,

                show_hidden: self.show_hidden,

                hidden_only: self.hidden_only,
            });
        }

        /*
         * The old corpus and persistent-index state belong to the source being left.
         *
         * Clear both before installing the newly connected SSH source so Alt+R must
         * inspect and load the index belonging to the new target.
         */
        self.invalidate_recursive_cache();

        self.invalidate_remote_index_state();

        self.source = source;

        /*
         * Owner IDs belong to the active machine. A cached local or previous-remote
         * username must not survive after installing a new SSH source.
         */
        self.owner_name_cache.clear();

        self.active_ssh_target = Some(target);

        /*
         * A newly connected filesystem source must begin outside recursive mode.
         *
         * Recursive state belongs to the previous source and must never be inherited
         * after its corpus has been invalidated. In particular, an SSH connection must
         * pass through prepare_remote_recursive_mode() when the user next enables
         * Recursive mode so its persistent index can be located and loaded.
         */
        self.recursive_mode = false;

        /*
         * Every newly connected SSH source starts with ordinary visibility.
         *
         * The previous local Hidden and Hidden Only state is retained separately in
         * saved_local_session and restored only when returning to the local source.
         */
        self.show_hidden = false;

        self.hidden_only = false;

        self.current_directory = directory;

        self.home_directory = home_directory;

        self.entries = entries;

        self.query.clear();

        self.query_cursor = 0;

        self.search_mode = SearchMode::Exact;

        self.clear_messages();

        self.selected = 0;

        self.list_offset = 0;

        self.pending_selection_path = None;

        self.view_mode = ViewMode::List;

        self.tree_rows.clear();

        self.tree_children.clear();

        self.search_tree_children.clear();

        self.expanded_directories.clear();

        self.search_collapsed_directories.clear();

        self.recursive_expanded_directories.clear();

        self.directory_has_content_cache.clear();

        self.classification_inspection_cache.clear();

        self.navigation_states.clear();

        self.back_history.clear();

        self.refresh_filter();
    }

    pub fn disconnect_remote(&mut self) {
        self.search_return_state = None;

        if !self.source.is_remote() || self.transfer_visible() || self.connection_in_progress {
            return;
        }

        self.marked_files.clear();

        let saved_session = self.saved_local_session.take();

        let fallback_directory = self.launch_directory.clone();

        /*
         * If no saved local session exists, use the real local HOME as the
         * destination of the new Go Home control.
         *
         * Fall back to the current working directory only when HOME is unavailable.
         */
        let fallback_home_directory = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| fallback_directory.clone());

        let session = saved_session.unwrap_or(LocalSessionState {
            directory: fallback_directory,

            home_directory: fallback_home_directory,

            selected_path: None,

            list_offset: 0,

            query: String::new(),

            view_mode: ViewMode::List,

            search_mode: SearchMode::Exact,

            recursive_mode: false,

            show_hidden: false,

            hidden_only: false,
        });

        let mut local_source = LocalSource::new();

        let entries = match local_source.read_directory(
            &session.directory,
            self.sort_mode,
            self.sort_descending,
        ) {
            Ok(entries) => entries,

            Err(error) => {
                /*
                 * Preserve the saved session so the user may correct the local
                 * filesystem problem and try Disconnect again.
                 */
                self.saved_local_session = Some(session);

                self.connection_dialog.error_message =
                    Some(format!("Unable to restore the local directory: {}", error,));

                return;
            }
        };

        /*
         * Assigning LocalSource drops the previous SftpSource and therefore closes
         * the SSH/SFTP connection.
         */
        self.source = Box::new(local_source);

        /*
         * Return to the local machine's user database. Discard usernames cached from
         * the remote host before rendering local owner metadata again.
         */
        self.owner_name_cache.clear();

        self.active_ssh_target = None;

        self.invalidate_recursive_cache();

        self.invalidate_remote_index_state();

        /*
         * Restore the local meaning of Home before restoring the local
         * working directory.
         */
        self.home_directory = session.home_directory;

        self.current_directory = session.directory;

        self.entries = entries;

        self.query = session.query;

        self.query_cursor = self.query.len();

        self.search_mode = session.search_mode;

        /*
         * Restore the local visibility policy before rebuilding filters, scans,
         * or Tree state.
         */
        self.show_hidden = session.show_hidden;

        self.hidden_only = session.hidden_only;

        self.error_message = None;

        self.selected = 0;

        self.list_offset = 0;

        self.pending_selection_path = session.selected_path.clone();

        self.view_mode = ViewMode::List;

        self.tree_rows.clear();

        self.tree_children.clear();

        self.search_tree_children.clear();

        self.expanded_directories.clear();

        self.search_collapsed_directories.clear();

        self.recursive_expanded_directories.clear();

        self.directory_has_content_cache.clear();

        self.classification_inspection_cache.clear();

        self.navigation_states.clear();

        /*
         * Re-establish the saved recursive mode deliberately. invalidate_recursive_cache()
         * clears scan data but does not change the user's recursive preference.
         */
        self.recursive_mode = session.recursive_mode;

        if self.recursive_mode {
            self.ensure_recursive_scan();
        }

        match session.view_mode {
            ViewMode::List => {
                self.refresh_filter();

                self.restore_pending_selection_if_available();

                self.list_offset = session.list_offset;

                self.ensure_selection_visible(self.viewport_rows);
            }

            ViewMode::Tree => {
                self.view_mode = ViewMode::Tree;

                if self.recursive_mode {
                    self.ensure_recursive_scan();

                    /*
                     * The recursive tree will finish rebuilding when the scanner
                     * delivers its result.
                     */
                    self.rebuild_recursive_search_tree(session.selected_path);
                } else {
                    self.reset_tree();

                    if let Some(path) = session.selected_path {
                        self.select_visible_path(&path);
                    }

                    self.list_offset = session.list_offset;

                    self.ensure_selection_visible(self.viewport_rows);
                }
            }
        }

        self.overlay = Overlay::None;

        self.connection_dialog.error_message = None;

        self.show_info_message("Disconnected — local browsing restored");
    }

    pub fn close_connection_dialog(&mut self) {
        self.overlay = Overlay::None;

        self.connection_dialog.error_message = None;
    }

    pub fn about_visible(&self) -> bool {
        self.overlay == Overlay::About
    }

    pub fn toggle_about(&mut self) {
        self.remote_index_setup = None;

        self.overlay = match self.overlay {
            Overlay::About => Overlay::None,

            Overlay::None
            | Overlay::Help
            | Overlay::Legend
            | Overlay::Connection
            | Overlay::RemoteIndexSetup => Overlay::About,
        };
    }

    pub fn close_about(&mut self) {
        self.overlay = Overlay::None;
    }

    pub fn legend_visible(&self) -> bool {
        self.overlay == Overlay::Legend
    }

    pub fn toggle_legend(&mut self) {
        if self.overlay == Overlay::Legend {
            self.overlay = Overlay::None;

            return;
        }

        /*
         * Preserve the Legend's previous scroll position.
         *
         * The renderer clamps it if the terminal size or content height changed
         * while the window was closed.
         */
        self.overlay = Overlay::Legend;
    }

    pub fn close_legend(&mut self) {
        self.overlay = Overlay::None;
    }

    pub fn help_visible(&self) -> bool {
        self.overlay == Overlay::Help
    }

    pub fn toggle_help(&mut self) {
        self.remote_index_setup = None;

        /*
         * Hover is transient mouse state and must never survive closing or
         * reopening the Help overlay.
         */
        self.help_tips_hovered = false;

        self.help_top_hovered = false;

        self.overlay = match self.overlay {
            Overlay::Help => Overlay::None,

            Overlay::None
            | Overlay::Legend
            | Overlay::About
            | Overlay::Connection
            | Overlay::RemoteIndexSetup => {
                /*
                 * Preserve the Help document's previous scroll position.
                 *
                 * render_help_overlay() clamps it against the current content and
                 * viewport dimensions whenever the window is drawn.
                 */
                Overlay::Help
            }
        };
    }

    pub fn close_help(&mut self) {
        self.help_tips_hovered = false;

        self.help_top_hovered = false;

        self.overlay = Overlay::None;
    }

    pub fn scroll_help_up(&mut self) {
        self.help_scroll = self.help_scroll.saturating_sub(1);
    }

    pub fn scroll_help_down(&mut self) {
        self.help_scroll = self.help_scroll.saturating_add(1).min(self.help_max_scroll);
    }

    pub fn page_help_up(&mut self) {
        let amount = self.viewport_rows.saturating_sub(4).max(1) as u16;

        self.help_scroll = self.help_scroll.saturating_sub(amount);
    }

    pub fn page_help_down(&mut self) {
        let amount = self.viewport_rows.saturating_sub(4).max(1) as u16;

        self.help_scroll = self
            .help_scroll
            .saturating_add(amount)
            .min(self.help_max_scroll);
    }

    pub fn help_scroll_to_end(&mut self) {
        self.help_scroll = self.help_max_scroll;
    }

    pub fn help_scroll_to_tips(&mut self) {
        self.help_scroll = self.help_tips_scroll.min(self.help_max_scroll);
    }

    pub fn help_scroll_to_top(&mut self) {
        self.help_scroll = 0;
    }

    pub fn scroll_legend_up(&mut self) {
        self.legend_scroll = self.legend_scroll.saturating_sub(1);
    }

    pub fn scroll_legend_down(&mut self) {
        self.legend_scroll = self
            .legend_scroll
            .saturating_add(1)
            .min(self.legend_max_scroll);
    }

    pub fn page_legend_up(&mut self) {
        let amount = self.viewport_rows.saturating_sub(4).max(1) as u16;

        self.legend_scroll = self.legend_scroll.saturating_sub(amount);
    }

    pub fn page_legend_down(&mut self) {
        let amount = self.viewport_rows.saturating_sub(4).max(1) as u16;

        self.legend_scroll = self
            .legend_scroll
            .saturating_add(amount)
            .min(self.legend_max_scroll);
    }

    pub fn legend_scroll_to_home(&mut self) {
        self.legend_scroll = 0;
    }

    pub fn legend_scroll_to_end(&mut self) {
        self.legend_scroll = self.legend_max_scroll;
    }

    fn active_entries(&self) -> &[FileEntry] {
        /*
         * An empty query is ordinary filesystem browsing.
         *
         * Recursive mode may scan and cache descendants in the background, but the
         * flat List must continue to display only the current directory until the
         * user enters actual search text.
         */
        let query_active = self.effective_query_is_active();

        if query_active && self.recursive_search_active() {
            &self.recursive_entries
        } else {
            &self.entries
        }
    }

    fn ensure_recursive_scan(&mut self) {
        if self.persistent_remote_index_available() {
            /*
             * A loaded persistent remote index is the authoritative recursive
             * corpus for this host.
             *
             * Navigation and search restoration may call this method, but they
             * must never replace the host-wide corpus with the older directory-
             * rooted Fast scanner.
             */
            self.scan_receiver = None;

            self.scan_in_progress = false;

            return;
        }

        if !self.source.supports_recursive_scan() {
            self.show_error_message("Recursive scanning is not available for the current source");

            self.scan_in_progress = false;

            self.recursive_scan_partial = false;

            return;
        }

        if self.recursive_cache_complete || self.scan_receiver.is_some() {
            return;
        }

        self.scan_generation = self.scan_generation.wrapping_add(1);

        /*
         * A newly started directory-rooted scan establishes the coverage root for
         * this resident recursive corpus.
         *
         * Persistent remote indexes keep their own host-wide authority and never
         * arrive through this path.
         */
        self.recursive_corpus_root = Some(self.current_directory.clone());

        let pending_seed = self.pending_recursive_scan_seed.take();

        self.recursive_entries.clear();

        self.recursive_child_indices.clear();

        Arc::make_mut(&mut self.search_index).clear();

        self.reset_progressive_exact_tree();

        let mut excluded_subtree = None;

        if let Some(mut seed) = pending_seed {
            /*
             * change_directory() has already loaded the new parent's immediate entries.
             *
             * The excluded subtree root itself was not present in its former recursive
             * corpus because a recursive scan emits descendants of its root, not the root
             * entry itself. Add the parent's already-loaded FileEntry so contextual Tree
             * ancestors can connect retained matches to the new root immediately.
             */
            if let Some(root_entry) = self
                .entries
                .iter()
                .find(|entry| entry.path == seed.excluded_subtree)
                .cloned()
            {
                seed.entries.push(root_entry);
            }

            for entry in &mut seed.entries {
                rebase_recursive_entry(entry, &self.current_directory);
            }

            /*
             * Deduplicate defensively by absolute path.
             *
             * The old recursive corpus should already contain unique paths, but the
             * explicitly added subtree-root entry makes correctness more important than
             * relying on that assumption.
             */
            let mut seen_paths = HashSet::with_capacity(seed.entries.len());

            seed.entries
                .retain(|entry| seen_paths.insert(entry.path.clone()));

            self.recursive_entries = seed.entries;

            self.recursive_child_indices.clear();

            for (index, entry) in self.recursive_entries.iter().enumerate() {
                let Some(parent) = entry.path.parent() else {
                    continue;
                };

                self.recursive_child_indices
                    .entry(parent.to_path_buf())
                    .or_default()
                    .push(index);
            }

            self.search_index = Arc::new(SearchIndex::from_entries(&self.recursive_entries));

            excluded_subtree = Some(seed.excluded_subtree);
        }

        let receiver = match self.source.start_recursive_scan(
            self.current_directory.clone(),
            self.show_hidden,
            self.hidden_only,
            excluded_subtree,
            self.scan_generation,
            self.recursive_scan_mode,
        ) {
            Ok(receiver) => receiver,

            Err(error) => {
                self.show_error_message(format!(
                    "Unable to start recursive scan of {}: {}",
                    self.current_directory.display(),
                    error,
                ));

                self.scan_in_progress = false;

                self.recursive_scan_partial = false;

                return;
            }
        };

        self.scan_receiver = Some(receiver);

        self.scan_in_progress = true;

        self.clear_messages();
    }

    fn invalidate_recursive_cache(&mut self) {
        /*
         * Dropping the receiver causes the old scanner to stop the next time
         * it attempts to send a batch.
         */
        self.cancel_fuzzy_filter();

        self.scan_receiver = None;

        self.scan_generation = self.scan_generation.wrapping_add(1);

        self.scan_in_progress = false;

        self.pending_tree_expand_all = None;

        self.pending_recursive_visibility_expand_restore = false;

        self.tree_expand_all_dialog = None;

        self.recursive_cache_complete = false;

        self.recursive_corpus_root = None;

        self.pending_session_recursive_expand_all = false;

        self.pending_session_search_collapse_all = false;

        self.recursive_scan_partial = false;

        /*
         * This corpus is being invalidated completely, not reused in place.
         *
         * Replace the large containers rather than clear()ing them. clear() retains
         * their allocated capacity and can therefore leave a multi-million-entry
         * remote corpus consuming substantial memory after disconnect or rerooting.
         */
        self.recursive_entries = Vec::new();

        self.recursive_child_indices = HashMap::new();

        /*
         * Never use Arc::make_mut(...).clear() when discarding the complete SearchIndex.
         *
         * A background Exact/Fuzzy worker may still own another Arc. In that case
         * make_mut() would clone the complete multi-million-record index merely so the
         * new clone could immediately be cleared.
         *
         * Give App a fresh empty index instead. Any cancelled worker keeps its old Arc
         * until it exits, after which that old allocation is released normally.
         */
        self.search_index = Arc::new(SearchIndex::new());

        self.search_tree_children.clear();

        self.recursive_tree_identity = None;

        self.recursive_expanded_directories.clear();

        self.reset_progressive_exact_tree();
    }

    /*
     * Forget every persistent-index state associated with the previous SSH source.
     *
     * Recursive corpus invalidation alone is not enough when the filesystem source
     * changes. remote_index_loaded describes a specific host/account/port index and
     * must never survive a disconnect or connection to another source.
     */
    fn invalidate_remote_index_state(&mut self) {
        /*
         * Dropping the receiver prevents a late result from the previous source from
         * being installed after a new SSH connection has taken ownership of App.
         */
        self.remote_index_load_receiver = None;

        self.remote_index_load_in_progress = false;

        self.remote_index_loaded = false;

        self.remote_index_includes_hidden = false;
    }

    pub fn current_visible_entry_count(&self) -> usize {
        match self.view_mode {
            ViewMode::List => self.filtered_indices.len(),

            ViewMode::Tree => self.filtered_tree_indices.len(),
        }
    }

    fn save_current_navigation_state(&mut self) {
        let selected_path = self.selected_entry().map(|entry| entry.path.clone());

        self.navigation_states.insert(
            self.current_directory.clone(),
            NavigationState {
                selected_path,

                list_offset: self.list_offset,
            },
        );
    }

    fn save_search_return_state(&mut self, landed_directory: PathBuf) {
        /*
         * Only typed searches need a return state.
         *
         * Persistent recursive browsing with an empty query is ordinary navigation,
         * not a search that should be restored after backing out.
         */
        if self.query.is_empty() || self.query == "." {
            return;
        }

        self.search_return_state = Some(SearchReturnState {
            root_directory: self.current_directory.clone(),

            landed_directory,

            query: self.query.clone(),

            search_mode: self.search_mode,

            selected_path: self.selected_entry().map(|entry| entry.path.clone()),

            list_offset: self.list_offset,

            view_mode: self.view_mode,

            recursive_mode: self.recursive_mode,
        });
    }

    fn change_search_root(&mut self, target: PathBuf, fallback_selection: Option<PathBuf>) -> bool {
        let entries =
            match self
                .source
                .read_directory(&target, self.sort_mode, self.sort_descending)
            {
                Ok(entries) => entries,

                Err(error) => {
                    self.show_error_message(format!(
                        "Unable to open {}: {}",
                        target.display(),
                        error,
                    ));

                    return false;
                }
            };

        /*
         * Explicit search-root navigation supersedes any older suspended-search
         * return bookmark.
         *
         * Without clearing it, a later Left can accidentally restore an obsolete
         * state and replace the query that the user is carrying upward through the
         * directory hierarchy.
         */
        self.search_return_state = None;

        /*
         * Preserve the active search while changing only its filesystem root.
         */
        let query = self.query.clone();

        let view_mode = self.view_mode;

        self.save_current_navigation_state();

        if self.source.is_remote() && self.remote_index_loaded {
            /*
             * The persistent remote corpus covers the complete host.
             *
             * Directory navigation changes only the search scope. It must not discard
             * or reload the host-wide index.
             */
            self.cancel_fuzzy_filter();

            self.search_tree_children.clear();

            self.recursive_expanded_directories.clear();
        } else {
            self.invalidate_recursive_cache();
        }

        self.tree_rows.clear();

        self.filtered_tree_indices.clear();

        self.tree_children.clear();

        self.search_tree_children.clear();

        self.directory_has_content_cache.clear();

        self.expanded_directories.clear();

        self.search_collapsed_directories.clear();

        self.recursive_expanded_directories.clear();

        self.current_directory = target;

        self.entries = entries;

        self.query = query;

        self.query_cursor = self.query.len();

        self.search_navigation_active = true;

        self.clear_messages();

        self.selected = 0;

        self.list_offset = 0;

        self.pending_selection_path = fallback_selection.clone();

        self.ensure_recursive_scan();

        match view_mode {
            ViewMode::List => {
                self.view_mode = ViewMode::List;

                self.refresh_filter();

                self.restore_pending_selection_if_available();
            }

            ViewMode::Tree => {
                self.view_mode = ViewMode::Tree;

                /*
                 * The hierarchy will be completed when the recursive scanner
                 * finishes. If the cache is already complete, build immediately.
                 */
                if !self.scan_in_progress {
                    self.rebuild_recursive_search_tree(fallback_selection);
                }
            }
        }

        true
    }

    fn navigate_to_directory(
        &mut self,
        target: PathBuf,
        fallback_selection: Option<PathBuf>,
    ) -> bool {
        if target == self.current_directory {
            return false;
        }

        let history_entry = BackHistoryEntry {
            directory: self.current_directory.clone(),

            view_mode: self.view_mode,
        };

        if !self.change_directory(target, fallback_selection) {
            return false;
        }

        /*
         * Consecutive copies of the same location provide no useful Back step.
         */
        let duplicate = self.back_history.last().is_some_and(|previous| {
            previous.directory == history_entry.directory
                && previous.view_mode == history_entry.view_mode
        });

        if !duplicate {
            self.back_history.push(history_entry);
        }

        true
    }

    fn local_recursive_corpus_covers(&self, target: &Path) -> bool {
        !self.source.is_remote()
            && self.recursive_mode
            && self
                .recursive_corpus_root
                .as_ref()
                .is_some_and(|root| target.starts_with(root))
    }

    fn change_directory(&mut self, target: PathBuf, fallback_selection: Option<PathBuf>) -> bool {
        let entries =
            match self
                .source
                .read_directory(&target, self.sort_mode, self.sort_descending)
            {
                Ok(entries) => entries,

                Err(error) => {
                    self.show_error_message(format!(
                        "Unable to open {}: {}",
                        target.display(),
                        error,
                    ));

                    return false;
                }
            };

        self.save_current_navigation_state();

        let retain_local_recursive_corpus = self.local_recursive_corpus_covers(&target);

        if (self.source.is_remote() && self.remote_index_loaded) || retain_local_recursive_corpus {
            /*
             * The resident recursive corpus already covers the destination.
             *
             * Remote persistent indexes cover the host, while a local corpus covers
             * recursive_corpus_root and every descendant beneath it.
             *
             * Navigation therefore changes only the active browsing/search scope.
             * Do not cancel a still-running local scan or discard a completed corpus.
             */
            self.cancel_fuzzy_filter();

            self.search_tree_children.clear();

            self.recursive_expanded_directories.clear();

            /*
             * A local progressive Exact Tree is scoped to current_directory.
             *
             * Navigation inside a broader retained corpus changes that scope, so force
             * its bounded match set to be rebuilt for the new directory.
             */
            if retain_local_recursive_corpus {
                self.reset_progressive_exact_tree();
            }
        } else {
            self.invalidate_recursive_cache();
        }

        self.tree_rows.clear();

        self.tree_children.clear();

        self.directory_has_content_cache.clear();

        self.expanded_directories.clear();

        self.view_mode = ViewMode::List;

        self.current_directory = target.clone();

        self.entries = entries;

        /*
         * Directory navigation does not own the query.
         *
         * The active query and cursor survive Enter, Right, Left, Back,
         * Home, and Tree rerooting. Only explicit query editing or Ctrl+U may alter
         * or clear them.
         */
        self.query_cursor = self.query_cursor.min(self.query.len());

        self.clear_messages();

        self.selected = 0;

        self.list_offset = 0;

        let saved_state = self.navigation_states.get(&target).cloned();

        /*
         * A fallback selection represents the directory or file we just came from.
         * It therefore takes priority over an older saved selection for this root.
         */
        let desired_selection = fallback_selection.clone().or_else(|| {
            saved_state
                .as_ref()
                .and_then(|state| state.selected_path.clone())
        });

        let desired_offset = if fallback_selection.is_some() {
            0
        } else {
            saved_state
                .as_ref()
                .map(|state| state.list_offset)
                .unwrap_or(0)
        };

        if self.recursive_mode {
            /*
             * The recursive results are initially empty. Remember the intended
             * selection and restore it when its scan batch arrives.
             */
            self.pending_selection_path = desired_selection;

            self.ensure_recursive_scan();

            self.refresh_filter();

            self.restore_pending_selection_if_available();
        } else {
            self.pending_selection_path = None;

            self.refresh_filter();

            if let Some(path) = desired_selection {
                self.select_path(&path);
            }

            self.list_offset = desired_offset;
        }

        true
    }

    fn select_path(&mut self, target: &PathBuf) {
        if let Some(position) = self.filtered_indices.iter().position(|entry_index| {
            self.entries
                .get(*entry_index)
                .is_some_and(|entry| &entry.path == target)
        }) {
            self.selected = position;
        }
    }

    fn select_visible_path(&mut self, target: &PathBuf) {
        let position = match self.view_mode {
            ViewMode::List => self.filtered_indices.iter().position(|entry_index| {
                self.active_entries()
                    .get(*entry_index)
                    .is_some_and(|entry| &entry.path == target)
            }),

            ViewMode::Tree => self.filtered_tree_indices.iter().position(|tree_index| {
                self.tree_rows
                    .get(*tree_index)
                    .is_some_and(|row| &row.entry.path == target)
            }),
        };

        if let Some(position) = position {
            self.selected = position;
        }
    }

    /*
     * Map the current Tree selection into the flat List result.
     *
     * A queried recursive List represents matching descendants directly, while an
     * empty List deliberately displays only the current directory's immediate
     * children.
     *
     * Mapping policy:
     *
     * 1. Preserve the exact selected path when List represents it.
     * 2. With no active query, map a deep Tree entry to its nearest visible ancestor
     *    in the current-directory List.
     * 3. For a queried Tree-only structural directory, select the first actual List
     *    result beneath that directory.
     * 4. Return None when List represents neither the path nor a useful relation.
     */
    fn list_path_for_tree_selection(&self, tree_path: &Path) -> Option<PathBuf> {
        /*
         * An empty List contains only entries directly beneath current_directory.
         *
         * A deeply selected Tree file therefore cannot itself remain visible. Retain
         * the user's location by selecting the immediate child of the current root
         * that contains it.
         *
         * Apply the same visibility and entry-kind policy used by the destination
         * List so Files Only, Directories Only, and hidden-entry filtering remain
         * truthful.
         */
        if !self.effective_query_is_active() {
            return self
                .entries
                .iter()
                .filter(|entry| {
                    entry_matches_visibility(
                        entry,
                        &self.current_directory,
                        self.show_hidden,
                        self.hidden_only,
                    ) && self.entry_filter.matches(entry)
                })
                .filter(|entry| entry.path == tree_path || tree_path.starts_with(&entry.path))
                .max_by_key(|entry| entry.path.components().count())
                .map(|entry| entry.path.clone());
        }

        let active_entries = self.active_entries();

        /*
         * Preserve the exact path whenever it is a direct queried List result.
         */
        if self.filtered_indices.iter().any(|entry_index| {
            active_entries
                .get(*entry_index)
                .is_some_and(|entry| entry.path == tree_path)
        }) {
            return Some(tree_path.to_path_buf());
        }

        /*
         * A structural Tree directory may not itself satisfy the query.
         *
         * Choose the first actual List result beneath it. filtered_indices already
         * follows the displayed Exact ordering or Fuzzy relevance ordering.
         */
        self.filtered_indices.iter().find_map(|entry_index| {
            active_entries.get(*entry_index).and_then(|entry| {
                entry
                    .path
                    .starts_with(tree_path)
                    .then(|| entry.path.clone())
            })
        })
    }

    /*
     * Reveal the complete ancestor chain leading to a pending session selection.
     *
     * A restored Tree selection cannot be found until every directory between the
     * current Tree root and the saved entry has been expanded. The recursive index
     * may already contain the entry, but collapsed ancestors otherwise keep it out
     * of tree_rows.
     */
    fn expand_ancestors_for_pending_tree_selection(&mut self) {
        if self.view_mode != ViewMode::Tree || !self.recursive_mode || !self.query.is_empty() {
            return;
        }

        let Some(target) = self.pending_selection_path.clone() else {
            return;
        };

        /*
         * Collect the saved ancestor corridor from the selected entry back toward
         * the current Tree root.
         *
         * Queryless Recursive Tree mode is deliberately lazy, so restoring only the
         * expansion HashSet is insufficient: the corresponding child maps must also
         * exist before tree_rows can expose the saved descendant.
         */
        let mut ancestors = Vec::new();

        let mut ancestor = target.parent();

        while let Some(path) = ancestor {
            if path == self.current_directory {
                break;
            }

            /*
             * Ignore paths outside the restored Tree root. This protects against a
             * stale session path after a directory has been moved or removed.
             */
            if !path.starts_with(&self.current_directory) {
                break;
            }

            ancestors.push(path.to_path_buf());

            ancestor = path.parent();
        }

        /*
         * Reopen every saved branch.
         */
        for path in &ancestors {
            self.recursive_expanded_directories.insert(path.clone());
        }

        /*
         * Materialize only the saved corridor.
         *
         * This does not reconstruct the complete recursive hierarchy. At most one
         * child list per ancestor of the restored selection is retained, preserving
         * the normal lazy Tree representation and its memory characteristics.
         */
        for path in ancestors.into_iter().rev() {
            let children = self.queryless_recursive_children_from_index(&path);

            if !children.is_empty() {
                self.search_tree_children.insert(path, children);
            }
        }
    }

    fn restore_pending_selection_if_available(&mut self) {
        let Some(target) = self.pending_selection_path.clone() else {
            return;
        };

        let position = match self.view_mode {
            ViewMode::List => self.filtered_indices.iter().position(|entry_index| {
                self.active_entries()
                    .get(*entry_index)
                    .is_some_and(|entry| entry.path == target)
            }),

            ViewMode::Tree => self.filtered_tree_indices.iter().position(|tree_index| {
                self.tree_rows
                    .get(*tree_index)
                    .is_some_and(|row| row.entry.path == target)
            }),
        };

        if let Some(position) = position {
            self.selected = position;

            /*
             * Session restoration owns an exact saved absolute offset.
             *
             * It takes priority over hidden-toggle and List/Tree viewport restoration
             * and must not be clamped before the renderer knows the real viewport size.
             */
            if let Some(viewport_row) = self.pending_session_viewport_row {
                self.list_offset = self.selected.saturating_sub(viewport_row);
            } else if let Some(saved_offset) = self.pending_session_list_offset.take() {
                /*
                 * Compatibility fallback for older session files that predate
                 * selected_viewport_row.
                 */
                self.list_offset = saved_offset;
            } else if let Some(viewport_row) = self.pending_visibility_viewport_row {
                /*
                 * Alt+H or Hidden Only may replace the Recursive List or Tree
                 * asynchronously.
                 *
                 * Keep the selected path on the same visual row even when visibility
                 * changes its absolute position in the result set.
                 */
                self.list_offset = self.selected.saturating_sub(viewport_row);
            } else if self.view_mode == ViewMode::List
                && let Some(viewport_row) = self.pending_list_viewport_row
            {
                /*
                 * A Recursive List worker may publish progressively broader snapshots.
                 *
                 * Reconstruct the viewport after every snapshot so the selected path
                 * remains on the same screen row.
                 */
                self.list_offset = self.selected.saturating_sub(viewport_row);
            } else if self.view_mode == ViewMode::Tree
                && let Some(viewport_row) = self.pending_tree_viewport_row
            {
                /*
                 * A Recursive Tree worker may publish progressively rebuilt hierarchies.
                 *
                 * Reconstruct the viewport after every snapshot so the selected path
                 * remains on the intended Tree screen row.
                 */
                self.list_offset = self.selected.saturating_sub(viewport_row);
            }

            /*
             * A running recursive scan or Exact/Fuzzy worker may still replace the
             * destination result set after an intermediate snapshot contains the path.
             *
             * Keep every pending restoration value alive until the result is stable.
             * Once stable, consume all transition-specific state so an old Hidden,
             * Hidden Only, List, or Tree viewport row cannot affect a later operation.
             */
            if !self.scan_in_progress
                && !self.fuzzy_filter_in_progress
                && !self.remote_index_load_in_progress
            {
                self.pending_selection_path = None;

                self.pending_list_viewport_row = None;

                self.pending_tree_viewport_row = None;

                self.pending_visibility_viewport_row = None;

                self.pending_session_viewport_row = None;

                self.pending_session_list_offset = None;
            }
        }
    }

    fn cancel_fuzzy_filter(&mut self) {
        if let Some(signal) = self.fuzzy_cancel_signal.take() {
            signal.store(true, Ordering::Relaxed);
        }

        self.fuzzy_receiver = None;

        self.active_fuzzy_request = None;

        self.fuzzy_filter_in_progress = false;

        self.fuzzy_examined = 0;

        self.fuzzy_total = 0;

        self.fuzzy_generation = self.fuzzy_generation.wrapping_add(1);

        self.exact_tree_limit_reached = false;
    }

    fn schedule_current_recursive_search(&mut self) {
        if !self.recursive_search_active() || !self.effective_query_is_active() {
            self.pending_recursive_search_at = None;

            self.cancel_fuzzy_filter();

            /*
             * Ordinary Exact Tree filtering is synchronous and owns its own visible
             * Tree-row index.
             *
             * Non-recursive Fuzzy Tree still uses the temporary background worker
             * started through refresh_filter(), whose returned indices are converted
             * by rebuild_local_search_tree_from_indices().
             */
            if self.view_mode == ViewMode::Tree
                && self.search_mode == SearchMode::Exact
                && self.effective_query_is_active()
            {
                self.refresh_tree_filter();
            } else {
                self.refresh_filter();
            }

            return;
        }

        /*
         * Stop the worker for the previous query immediately.
         *
         * The visible result list remains in place while the newest query waits for
         * its deadline. Only the obsolete computation disappears.
         */
        self.cancel_fuzzy_filter();

        self.pending_recursive_search_at = Some(Instant::now() + RECURSIVE_SEARCH_DEBOUNCE);

        self.fuzzy_examined = 0;

        self.fuzzy_total = self.search_index.len();

        /*
         * This represents a pending live search as well as a running worker.
         *
         * The interface can therefore continue to indicate that the result set is
         * being updated during the short debounce interval.
         */
        self.fuzzy_filter_in_progress = true;
    }

    pub fn process_pending_recursive_search(&mut self) -> bool {
        let Some(deadline) = self.pending_recursive_search_at else {
            return false;
        };

        if Instant::now() < deadline {
            return false;
        }

        self.pending_recursive_search_at = None;

        let parsed_query = parse_query(&self.query);

        if parsed_query.is_effectively_empty() {
            self.cancel_fuzzy_filter();

            self.fuzzy_filter_in_progress = false;

            return true;
        }

        /*
         * A scan may still be preparing the stable recursive index.
         *
         * Scanner completion already re-enters the normal filtering route, so do
         * not start a worker against an incomplete corpus.
         */
        if self.scan_in_progress {
            self.fuzzy_filter_in_progress = false;

            return true;
        }

        match self.search_mode {
            SearchMode::Exact => {
                self.start_current_exact_filter();
            }

            SearchMode::Fuzzy => {
                self.start_current_fuzzy_filter();
            }
        }

        true
    }

    fn start_current_exact_filter(&mut self) {
        /*
         * Exact background searching is required only for a completed recursive
         * corpus.
         *
         * Ordinary single-directory filtering remains synchronous because that
         * entry set is small and avoids unnecessary worker overhead.
         */
        self.pending_recursive_search_at = None;

        self.exact_tree_limit_reached = false;

        if !self.recursive_search_active() {
            return;
        }

        let query_active = self.effective_query_is_active();

        if !query_active {
            self.cancel_fuzzy_filter();

            self.filtered_indices = self
                .active_entries()
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| {
                    if !self.show_hidden && entry_is_hidden_below(entry, &self.current_directory) {
                        return None;
                    }

                    if !self.entry_filter.matches(entry) {
                        return None;
                    }

                    if !entry.path.starts_with(&self.current_directory) {
                        return None;
                    }

                    Some(index)
                })
                .collect();

            self.normalize_filtered_selection();

            return;
        }

        /*
         * Wait for a stable recursive index.
         *
         * The scanner completion path starts the search again automatically.
         */
        if self.scan_in_progress {
            self.cancel_fuzzy_filter();

            self.fuzzy_examined = 0;

            self.fuzzy_total = self.search_index.len();

            return;
        }

        self.cancel_fuzzy_filter();

        let generation = self.fuzzy_generation;

        let index = Arc::clone(&self.search_index);

        let parsed_query = parse_query(&self.query);

        let worker_entry_filter = match self.entry_filter {
            EntryFilter::All => WorkerEntryFilter::All,

            EntryFilter::FilesOnly => WorkerEntryFilter::FilesOnly,

            EntryFilter::DirectoriesOnly => WorkerEntryFilter::DirectoriesOnly,
        };

        let scope_prefix = self.recursive_worker_scope_prefix();

        let result_limit = match self.view_mode {
            ViewMode::List => None,

            ViewMode::Tree => Some(self.exact_tree_match_limit),
        };

        let cancel_signal = Arc::new(AtomicBool::new(false));

        self.fuzzy_examined = 0;

        self.fuzzy_total = index.len();

        self.fuzzy_receiver = Some(start_exact_worker(
            index,
            parsed_query,
            generation,
            self.show_hidden,
            self.hidden_only,
            scope_prefix,
            worker_entry_filter,
            result_limit,
            self.sort_mode,
            self.sort_descending,
            Arc::clone(&cancel_signal),
        ));

        self.fuzzy_cancel_signal = Some(cancel_signal);

        self.active_fuzzy_request = None;

        self.fuzzy_filter_in_progress = true;

        /*
         * Keep the previous result visible until the first preview or final result
         * arrives. The query field can therefore redraw immediately without a
         * distracting empty-list flash.
         */
    }

    fn recursive_worker_scope_prefix(&self) -> Option<String> {
        if !self.recursive_search_active() {
            return None;
        }

        let corpus_root = if self.source.is_remote() {
            Path::new("/")
        } else {
            self.recursive_corpus_root.as_deref()?
        };

        if self.current_directory == corpus_root {
            return None;
        }

        let relative_scope = self.current_directory.strip_prefix(corpus_root).ok()?;

        let scope = relative_scope.to_string_lossy().to_lowercase();

        (!scope.is_empty()).then_some(scope)
    }

    fn start_current_fuzzy_filter(&mut self) {
        self.pending_recursive_search_at = None;

        self.exact_tree_limit_reached = false;

        let query_active = self.effective_query_is_active();

        if !query_active {
            self.cancel_fuzzy_filter();

            self.fuzzy_examined = 0;

            self.fuzzy_total = 0;

            self.filtered_indices = self
                .active_entries()
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| {
                    if !entry_matches_visibility(
                        entry,
                        &self.current_directory,
                        self.show_hidden,
                        self.hidden_only,
                    ) {
                        return None;
                    }

                    if !self.entry_filter.matches(entry) {
                        return None;
                    }

                    Some(index)
                })
                .collect();

            self.normalize_filtered_selection();

            return;
        }

        /*
         * Fuzzy relevance exists only when the query contains filename/path text.
         *
         * A Recursive Tree query made entirely from structural selectors such as
         * type: and ext: has nothing to score approximately. Route that case through
         * the Exact worker so toggling Exact/Fuzzy cannot change the direct-match set,
         * its configured Tree cap, or the resulting contextual hierarchy.
         *
         * SearchMode remains Fuzzy; only the worker semantics are shared while no
         * textual operand exists. Adding textual input immediately returns the query
         * to the normal Fuzzy worker.
         */
        if self.recursive_search_active() && self.view_mode == ViewMode::Tree {
            let parsed_query = parse_query(&self.query);

            if !parsed_query.has_textual_operands() {
                self.start_current_exact_filter();

                return;
            }
        }

        /*
         * The recursive index is still being constructed.
         *
         * We will add live scan-index searching later. For this pass, wait until
         * the index is stable rather than launching workers against incomplete
         * snapshots after every scanner batch.
         */
        if self.recursive_search_active() && self.scan_in_progress {
            self.cancel_fuzzy_filter();

            self.fuzzy_examined = 0;

            self.fuzzy_total = self.search_index.len();

            self.filtered_indices.clear();

            self.selected = 0;

            self.list_offset = 0;

            return;
        }

        let request = FuzzyRequestIdentity {
            query: self.query.clone(),

            scope_directory: self.current_directory.clone(),

            recursive_mode: self.recursive_search_active(),

            show_hidden: self.show_hidden,

            hidden_only: self.hidden_only,

            recursive_index_identity: self
                .recursive_search_active()
                .then_some(Arc::as_ptr(&self.search_index) as usize),
        };

        /*
         * Redraws, navigation restoration, and message processing can converge on
         * this method more than once for the same user-visible search.
         *
         * Do not cancel and restart a worker that is already evaluating precisely
         * the same request.
         */
        if self.fuzzy_filter_in_progress && self.active_fuzzy_request.as_ref() == Some(&request) {
            return;
        }

        if let Some(signal) = self.fuzzy_cancel_signal.take() {
            signal.store(true, Ordering::Relaxed);
        }

        self.fuzzy_receiver = None;

        self.fuzzy_generation = self.fuzzy_generation.wrapping_add(1);

        let generation = self.fuzzy_generation;

        /*
         * Recursive local search reuses the incrementally built index.
         *
         * Non-recursive sources such as SSH normally contain only one directory,
         * so constructing that small temporary index is inexpensive.
         */
        let index = if self.recursive_search_active() {
            Arc::clone(&self.search_index)
        } else {
            Arc::new(SearchIndex::from_entries(self.active_entries()))
        };

        self.fuzzy_examined = 0;

        self.fuzzy_total = index.len();

        let cancel_signal = Arc::new(AtomicBool::new(false));

        let scope_prefix = self.recursive_worker_scope_prefix();

        /*
         * Parse the query once for this worker generation.
         *
         * Structured modifiers decide which entries may participate. Only
         * ordinary unsigned text is sent to the fuzzy scorer.
         */
        let parsed_query = parse_query(&self.query);

        let worker_entry_filter = match self.entry_filter {
            EntryFilter::All => WorkerEntryFilter::All,

            EntryFilter::FilesOnly => WorkerEntryFilter::FilesOnly,

            EntryFilter::DirectoriesOnly => WorkerEntryFilter::DirectoriesOnly,
        };

        self.fuzzy_receiver = Some(start_fuzzy_worker(
            index,
            parsed_query,
            generation,
            self.show_hidden,
            self.hidden_only,
            scope_prefix,
            worker_entry_filter,
            self.fuzzy_result_limit,
            Arc::clone(&cancel_signal),
        ));

        self.fuzzy_cancel_signal = Some(cancel_signal);

        self.active_fuzzy_request = Some(request);

        self.fuzzy_filter_in_progress = true;

        /*
         * Deliberately do not clear filtered_indices here.
         *
         * Results from the previous query remain visible until the first progressive
         * snapshot for this generation arrives.
         */
    }

    /*
     * Drain progressive and final worker snapshots for the active generation.
     *
     * Completion, cancellation, and disconnection can all clear the receiver
     * and its associated worker state inside the loop.
     */
    #[allow(clippy::while_let_loop)]
    pub fn process_fuzzy_messages(&mut self) -> bool {
        let mut changed = false;

        loop {
            let message = match self.fuzzy_receiver.as_ref() {
                Some(receiver) => match receiver.try_recv() {
                    Ok(message) => message,

                    Err(TryRecvError::Empty) => {
                        break;
                    }

                    Err(TryRecvError::Disconnected) => {
                        self.fuzzy_receiver = None;

                        self.fuzzy_cancel_signal = None;

                        self.active_fuzzy_request = None;

                        self.fuzzy_filter_in_progress = false;

                        return true;
                    }
                },

                None => {
                    break;
                }
            };

            if message.generation != self.fuzzy_generation {
                continue;
            }

            if message.cancelled {
                if message.finished {
                    self.fuzzy_receiver = None;

                    self.fuzzy_cancel_signal = None;

                    self.active_fuzzy_request = None;

                    self.fuzzy_filter_in_progress = false;
                }

                continue;
            }

            /*
             * A List/Tree transition or session/visibility restoration may already own
             * an explicit target path.
             *
             * During an asynchronous Tree rebuild, selected_entry() can still refer to
             * the temporary index-zero row. That transient selection must never replace
             * the path deliberately carried into the destination view.
             */
            let selected_path = self
                .pending_selection_path
                .clone()
                .or_else(|| self.selected_entry().map(|entry| entry.path.clone()));

            self.fuzzy_examined = message.examined;

            self.fuzzy_total = message.total;

            /*
             * Exact Tree workers may cap very broad result sets to keep contextual Tree
             * construction responsive. Preserve that signal for the existing UI state.
             */
            self.exact_tree_limit_reached = message.limit_reached;

            /*
             * Worker indices belong to the corpus used when the worker was started.
             *
             * Recursive workers search search_index/recursive_entries. Ordinary Fuzzy
             * workers build a temporary SearchIndex from the current directory entries.
             */

            let recursive_result = self.recursive_search_active();

            match self.view_mode {
                ViewMode::List => {
                    self.filtered_indices = message
                        .indices
                        .into_iter()
                        .filter(|index| {
                            self.active_entries()
                                .get(*index)
                                .is_some_and(|entry| self.entry_filter.matches(entry))
                        })
                        .collect();

                    if let Some(path) = selected_path {
                        self.select_visible_path(&path);
                    } else {
                        self.normalize_filtered_selection();
                    }

                    self.restore_pending_selection_if_available();
                }

                ViewMode::Tree => {
                    if recursive_result {
                        /*
                         * Exact Recursive List is unlimited, while Exact Recursive Tree retains
                         * only a bounded number of direct matches.
                         *
                         * A file selected in List may therefore lie outside Tree's ordinary
                         * capped worker result. Pin that selected direct match into the Tree
                         * result so Ctrl+T can preserve the user's actual selection.
                         */
                        let mut tree_indices = message.indices.clone();

                        /*
                         * Exact Recursive Tree is capped while Exact List is unlimited.
                         *
                         * Preserve a selected Exact List result beyond that cap. Fuzzy results must
                         * not receive this exception: a path omitted by the Fuzzy worker may genuinely
                         * fail the approximate query and must not be injected as a false match.
                         */
                        if self.search_mode == SearchMode::Exact
                            && let Some(path) = self.pending_selection_path.as_ref()
                            && let Some(entry_index) = self.recursive_entry_index_for_path(path)
                            && !tree_indices.contains(&entry_index)
                        {
                            /*
                             * A selected Exact List result may legitimately lie beyond Tree's bounded
                             * worker result and still needs to survive a List -> Tree transition.
                             *
                             * pending_selection_path is also used by ordinary query editing, however.
                             * Never inject that carried path unless it genuinely satisfies the current
                             * query, or a zero-result search would display the previously selected entry
                             * as a false match.
                             */
                            let parsed_query = parse_query(&self.query);

                            if self
                                .recursive_entries
                                .get(entry_index)
                                .is_some_and(|entry| entry_matches_query(entry, &parsed_query))
                            {
                                tree_indices.push(entry_index);
                            }
                        }

                        self.rebuild_fuzzy_search_tree_from_indices(&tree_indices, selected_path);
                    } else {
                        self.rebuild_local_search_tree_from_indices(
                            &message.indices,
                            selected_path,
                        );
                    }

                    self.restore_pending_selection_if_available();
                }
            }

            if self.view_mode == ViewMode::List || message.finished {
                changed = true;
            }

            if message.finished {
                self.fuzzy_receiver = None;

                self.fuzzy_cancel_signal = None;

                self.fuzzy_filter_in_progress = false;

                /*
                 * The final result is now stable.
                 *
                 * Run restoration once more after clearing fuzzy_filter_in_progress so
                 * restore_pending_selection_if_available() can consume the carried path
                 * and viewport row.
                 */
                self.restore_pending_selection_if_available();

                /*
                 * pending_list_viewport_row and pending_tree_viewport_row are cleared by
                 * restore_pending_selection_if_available() only after the target path has
                 * actually been found.
                 */

                break;
            }
        }

        changed
    }

    fn normalize_filtered_selection(&mut self) {
        if self.filtered_indices.is_empty() {
            self.selected = 0;

            self.list_offset = 0;
        } else {
            self.selected = self
                .selected
                .min(self.filtered_indices.len().saturating_sub(1));

            self.list_offset = self
                .list_offset
                .min(self.filtered_indices.len().saturating_sub(1));
        }
    }

    fn current_recursive_tree_identity(&self) -> RecursiveTreeIdentity {
        RecursiveTreeIdentity {
            root_directory: self.current_directory.clone(),

            show_hidden: self.show_hidden,

            hidden_only: self.hidden_only,

            entry_filter: self.entry_filter,

            sort_mode: self.sort_mode,

            sort_descending: self.sort_descending,

            scan_generation: self.scan_generation,

            recursive_entry_count: self.recursive_entries.len(),
        }
    }

    fn retained_queryless_recursive_tree_is_current(&self) -> bool {
        !self.search_tree_children.is_empty()
            && self.recursive_tree_identity.as_ref()
                == Some(&self.current_recursive_tree_identity())
    }

    /*
     * Refresh the active recursive Tree after a view, sort, filter, or mode change.
     *
     * A genuine query must always be evaluated by the normal Exact/Fuzzy worker,
     * because only those workers understand Scry's complete query language.
     *
     * The synchronous Tree builder is reserved for the ordinary queryless
     * recursive hierarchy.
     */
    fn refresh_active_recursive_tree(&mut self, preferred_selection: Option<PathBuf>) {
        if !self.recursive_search_active() || self.view_mode != ViewMode::Tree {
            return;
        }

        self.pending_selection_path = preferred_selection;

        /*
         * Both query-driven and queryless recursive Trees require a valid recursive
         * corpus.
         *
         * Hidden-entry activation may invalidate that corpus. Always restart the
         * scan before deciding how the resulting Tree should be rebuilt.
         */
        self.ensure_recursive_scan();

        if self.scan_in_progress {
            /*
             * Queryless Recursive Tree browsing must not wait for the complete
             * descendant corpus.
             *
             * The current directory's immediate entries are already available, so
             * publish them as a usable recursive Tree root while indexing continues
             * independently in the background.
             *
             * Active searches retain their existing progressive Exact/Fuzzy routes.
             */
            if !self.effective_query_is_active() {
                let preferred_selection = self.pending_selection_path.clone();

                self.rebuild_recursive_tree_root_from_entries(preferred_selection);
            }

            return;
        }

        if self.effective_query_is_active() {
            match self.search_mode {
                SearchMode::Exact => {
                    self.start_current_exact_filter();
                }

                SearchMode::Fuzzy => {
                    self.start_current_fuzzy_filter();
                }
            }
        } else {
            self.cancel_fuzzy_filter();

            let preferred_selection = self.pending_selection_path.clone();

            /*
             * Queryless Recursive Tree browsing needs only the current root and a
             * one-level child look-ahead.
             *
             * Never rebuild the complete retained hierarchy merely because the user
             * changed browsing root.
             */
            self.rebuild_recursive_tree_root_from_entries(preferred_selection);

            self.restore_pending_selection_if_available();
        }
    }

    fn refresh_filter(&mut self) {
        /*
         * Every recursive text search uses a background worker.
         *
         * Exact and Fuzzy therefore have identical input responsiveness even when
         * the resident corpus contains millions of records.
         */
        if self.recursive_search_active() && self.effective_query_is_active() {
            match self.search_mode {
                SearchMode::Exact => {
                    self.start_current_exact_filter();
                }

                SearchMode::Fuzzy => {
                    self.start_current_fuzzy_filter();
                }
            }

            return;
        }

        /*
         * Non-recursive Fuzzy search retains its existing worker route.
         */
        if self.search_mode == SearchMode::Fuzzy {
            self.start_current_fuzzy_filter();

            return;
        }

        self.cancel_fuzzy_filter();

        let parsed_query = parse_query(&self.query);

        let show_hidden = self.show_hidden;

        let hidden_only = self.hidden_only;

        let staged_deletions = &self.staged_deletions;

        self.filtered_indices = self
            .active_entries()
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                if path_belongs_to_staged_deletion(&entry.path, staged_deletions) {
                    return None;
                }

                if !entry_matches_visibility(
                    entry,
                    &self.current_directory,
                    show_hidden,
                    hidden_only,
                ) {
                    return None;
                }

                if !self.entry_filter.matches(entry) {
                    return None;
                }

                if !entry_matches_query(entry, &parsed_query) {
                    return None;
                }

                Some(index)
            })
            .collect();

        self.normalize_filtered_selection();
    }

    fn rebuild_local_search_tree_from_indices(
        &mut self,
        matched_indices: &[usize],
        preferred_selection: Option<PathBuf>,
    ) {
        self.recursive_tree_identity = None;

        self.search_tree_children.clear();

        /*
         * A non-recursive Fuzzy worker was constructed from entries, so its indices
         * must be resolved against that same current-directory collection.
         *
         * All matching entries are immediate children of current_directory. The
         * shared search-tree row builder can therefore render them without consulting
         * recursive_entries or recursive_path_indices.
         */
        for &matched_index in matched_indices {
            let Some(entry) = self.entries.get(matched_index).cloned() else {
                continue;
            };

            let Some(parent) = entry.path.parent() else {
                continue;
            };

            self.search_tree_children
                .entry(parent.to_path_buf())
                .or_default()
                .push(entry);
        }

        /*
         * Preserve normal Scry sibling ordering rather than worker ranking in Tree
         * view. List mode retains the worker's ranked order.
         */
        for children in self.search_tree_children.values_mut() {
            sort_entries(children, self.sort_mode, self.sort_descending);
        }

        self.rebuild_recursive_search_rows(preferred_selection);
    }

    fn recursive_entry_index_for_path(&self, path: &Path) -> Option<usize> {
        let parent = path.parent()?;

        let child_indices = self.recursive_child_indices.get(parent)?;

        child_indices.iter().copied().find(|&entry_index| {
            self.recursive_entries
                .get(entry_index)
                .is_some_and(|entry| entry.path == path)
        })
    }

    fn rebuild_fuzzy_search_tree_from_indices(
        &mut self,
        matched_indices: &[usize],
        preferred_selection: Option<PathBuf>,
    ) {
        self.recursive_tree_identity = None;

        self.search_tree_children.clear();

        /*
         * The worker has already searched the complete compact index.
         *
         * Tree construction therefore touches only the returned matches and the
         * ancestors required to connect them to the current root.
         */
        let mut included_indices: HashSet<usize> = HashSet::new();

        for &matched_index in matched_indices {
            let Some(matched_entry) = self.recursive_entries.get(matched_index) else {
                continue;
            };

            included_indices.insert(matched_index);

            let mut ancestor = matched_entry.path.parent();

            while let Some(path) = ancestor {
                if path == self.current_directory {
                    break;
                }

                if let Some(ancestor_index) = self.recursive_entry_index_for_path(path) {
                    included_indices.insert(ancestor_index);
                }

                ancestor = path.parent();
            }
        }

        /*
         * Convert the bounded included set into the parent → children structure
         * consumed by the existing Tree-row builder.
         */
        for entry_index in included_indices {
            let Some(entry) = self.recursive_entries.get(entry_index).cloned() else {
                continue;
            };

            let Some(parent) = entry.path.parent() else {
                continue;
            };

            self.search_tree_children
                .entry(parent.to_path_buf())
                .or_default()
                .push(entry);
        }

        for children in self.search_tree_children.values_mut() {
            sort_entries(children, self.sort_mode, self.sort_descending);
        }

        self.rebuild_recursive_search_rows(preferred_selection);
    }

    /*
     * Recover one directory's immediate queryless Recursive Tree children directly
     * from the resident recursive child index.
     *
     * This touches only one child group instead of walking recursive_entries.
     */
    fn queryless_recursive_children_from_index(&self, directory: &Path) -> Vec<FileEntry> {
        let mut children: Vec<FileEntry> = self
            .recursive_child_indices
            .get(directory)
            .into_iter()
            .flatten()
            .filter_map(|index| self.recursive_entries.get(*index))
            .filter(|entry| {
                entry_matches_visibility(
                    entry,
                    &self.current_directory,
                    self.show_hidden,
                    self.hidden_only,
                )
            })
            .filter(|entry| {
                /*
                 * FilesOnly retains directories as Tree navigation corridors.
                 */
                self.entry_filter != EntryFilter::DirectoriesOnly || entry.is_directory
            })
            .cloned()
            .collect();

        sort_entries(&mut children, self.sort_mode, self.sort_descending);

        children
    }

    fn prepare_complete_queryless_recursive_tree(&mut self) {
        /*
         * Queryless Recursive Tree browsing is normally lazy.
         *
         * Alt+E is the explicit exception. Once the complete-expansion policy has
         * approved the request, materialize the full visible hierarchy from the
         * already resident recursive child index so every expanded directory can
         * actually be rendered.
         *
         * Ordinary browsing continues using the lightweight root + look-ahead map.
         */
        self.search_tree_children.clear();

        let mut pending_directories = vec![self.current_directory.clone()];

        while let Some(directory) = pending_directories.pop() {
            let children = self.queryless_recursive_children_from_index(&directory);

            if children.is_empty() {
                continue;
            }

            for entry in &children {
                if entry.is_directory && !entry.is_symlink {
                    pending_directories.push(entry.path.clone());
                }
            }

            self.search_tree_children.insert(directory, children);
        }

        self.recursive_tree_identity = Some(self.current_recursive_tree_identity());
    }

    /*
     * Publish an immediately usable queryless Recursive Tree from the entries
     * already loaded for the current directory.
     *
     * Recursive indexing may still be running in the background. Browsing must not
     * wait for the complete descendant corpus merely to display the current root.
     *
     * Keep this inside the recursive Tree representation rather than falling back
     * to ordinary tree_children: recursive branch expansion and session state use
     * their own search_tree_children / recursive_expanded_directories machinery.
     */
    fn rebuild_recursive_tree_root_from_entries(&mut self, preferred_selection: Option<PathBuf>) {
        let mut root_children: Vec<FileEntry> = self
            .entries
            .iter()
            .filter(|entry| {
                entry_matches_visibility(
                    entry,
                    &self.current_directory,
                    self.show_hidden,
                    self.hidden_only,
                )
            })
            .filter(|entry| {
                /*
                 * DirectoriesOnly may discard ordinary files immediately.
                 *
                 * FilesOnly keeps directories internally because they remain useful
                 * navigation structure.
                 */
                self.entry_filter != EntryFilter::DirectoriesOnly || entry.is_directory
            })
            .cloned()
            .collect();

        sort_entries(&mut root_children, self.sort_mode, self.sort_descending);

        /*
         * Remember the root's visible directories before moving root_children into
         * search_tree_children.
         *
         * One indexed look-ahead per visible directory keeps its Tree marker truthful
         * without constructing the complete recursive hierarchy.
         */
        let child_directories: Vec<PathBuf> = root_children
            .iter()
            .filter(|entry| entry.is_directory && !entry.is_symlink)
            .map(|entry| entry.path.clone())
            .collect();

        self.search_tree_children
            .insert(self.current_directory.clone(), root_children);

        for directory in child_directories {
            let children = self.queryless_recursive_children_from_index(&directory);

            if !children.is_empty() {
                self.search_tree_children.insert(directory, children);
            }
        }

        /*
         * An expanded path is useful only when its immediate children are also
         * materialized in the lazy Recursive Tree.
         *
         * Alt+R may carry already-open ordinary Tree branches into
         * recursive_expanded_directories. Recreate those same branch child maps from
         * the resident recursive index so switching Recursive mode does not visually
         * collapse a previously open hierarchy.
         */
        let expanded_directories: Vec<PathBuf> = self
            .recursive_expanded_directories
            .iter()
            .cloned()
            .collect();

        for directory in expanded_directories {
            if directory == self.current_directory
                || !directory.starts_with(&self.current_directory)
            {
                continue;
            }

            let children = self.queryless_recursive_children_from_index(&directory);

            if !children.is_empty() {
                self.search_tree_children.insert(directory, children);
            }
        }

        self.rebuild_recursive_search_rows(preferred_selection);

        self.recursive_tree_identity = Some(self.current_recursive_tree_identity());
    }

    fn rebuild_recursive_search_tree(&mut self, preferred_selection: Option<PathBuf>) {
        if !self.recursive_search_active() {
            return;
        }

        self.search_tree_children.clear();

        /*
         * Session restoration may be waiting for a descendant hidden beneath
         * collapsed Tree branches.
         *
         * Restore both its saved expansion state and the narrow materialized ancestor
         * corridor after clearing the previous lazy Tree representation.
         */
        self.expand_ancestors_for_pending_tree_selection();

        /*
         * Active recursive queries are built by their Exact/Fuzzy result paths.
         */
        if self.effective_query_is_active() {
            return;
        }

        /*
         * Queryless Recursive Tree mode is lazy.
         *
         * The complete recursive corpus remains available through recursive_entries and
         * recursive_child_indices, but only the current root and a small look-ahead are
         * materialized as FileEntry children.
         */
        self.rebuild_recursive_tree_root_from_entries(preferred_selection);
    }

    fn rebuild_recursive_search_rows(&mut self, preferred_selection: Option<PathBuf>) {
        /*
         * Apply compact session bulk states only after search_tree_children contains
         * the hierarchy against which they were recorded.
         */
        if self.pending_session_recursive_expand_all
            && self.recursive_mode
            && !self.effective_query_is_active()
        {
            self.recursive_expanded_directories = self
                .search_tree_children
                .iter()
                .filter_map(|(path, children)| {
                    (path != &self.current_directory && !children.is_empty())
                        .then_some(path.clone())
                })
                .collect();

            self.pending_session_recursive_expand_all = false;
        }

        if self.pending_session_search_collapse_all
            && self.recursive_search_active()
            && self.effective_query_is_active()
        {
            self.search_collapsed_directories = self
                .search_tree_children
                .iter()
                .filter_map(|(path, children)| {
                    (path != &self.current_directory && !children.is_empty())
                        .then_some(path.clone())
                })
                .collect();

            self.pending_session_search_collapse_all = false;
        }

        let fallback_position = self.selected;

        let mut rows = Vec::new();

        if self.recursive_mode && self.query.is_empty() {
            Self::append_recursive_direct_children(
                self.current_directory.clone(),
                Vec::new(),
                &self.search_tree_children,
                &self.recursive_expanded_directories,
                &mut rows,
            );
        } else {
            Self::append_recursive_search_children(
                self.current_directory.clone(),
                Vec::new(),
                &self.search_tree_children,
                &self.search_collapsed_directories,
                &mut rows,
            );
        }

        self.tree_rows = rows;

        self.filtered_tree_indices = (0..self.tree_rows.len()).collect();

        self.restore_search_tree_selection(preferred_selection, fallback_position);
    }

    fn append_recursive_direct_children(
        directory: PathBuf,
        ancestor_has_more: Vec<bool>,
        search_children: &HashMap<PathBuf, Vec<FileEntry>>,
        expanded_directories: &HashSet<PathBuf>,
        rows: &mut Vec<TreeRow>,
    ) {
        let Some(children) = search_children.get(&directory) else {
            return;
        };

        let child_count = children.len();

        for (index, entry) in children.iter().cloned().enumerate() {
            let is_last = index.saturating_add(1) == child_count;

            let child_path = entry.path.clone();

            let has_children = entry.is_directory
                && !entry.is_symlink
                && search_children
                    .get(&child_path)
                    .is_some_and(|children| !children.is_empty());

            let expanded = has_children && expanded_directories.contains(&child_path);

            rows.push(TreeRow {
                entry,

                ancestor_has_more: ancestor_has_more.clone(),

                is_last,

                expanded,
            });

            if expanded {
                let mut child_ancestor_has_more = ancestor_has_more.clone();

                child_ancestor_has_more.push(!is_last);

                Self::append_recursive_direct_children(
                    child_path,
                    child_ancestor_has_more,
                    search_children,
                    expanded_directories,
                    rows,
                );
            }
        }
    }

    fn append_recursive_search_children(
        directory: PathBuf,
        ancestor_has_more: Vec<bool>,
        search_children: &HashMap<PathBuf, Vec<FileEntry>>,
        collapsed_directories: &HashSet<PathBuf>,
        rows: &mut Vec<TreeRow>,
    ) {
        let Some(children) = search_children.get(&directory) else {
            return;
        };

        let child_count = children.len();

        for (index, entry) in children.iter().cloned().enumerate() {
            let is_last = index.saturating_add(1) == child_count;

            let child_path = entry.path.clone();

            let has_visible_children = search_children
                .get(&child_path)
                .is_some_and(|children| !children.is_empty());

            let expanded = entry.is_directory
                && has_visible_children
                && !collapsed_directories.contains(&child_path);

            rows.push(TreeRow {
                entry,

                ancestor_has_more: ancestor_has_more.clone(),

                is_last,

                expanded,
            });

            if expanded {
                let mut child_ancestor_has_more = ancestor_has_more.clone();

                child_ancestor_has_more.push(!is_last);

                Self::append_recursive_search_children(
                    child_path,
                    child_ancestor_has_more,
                    search_children,
                    collapsed_directories,
                    rows,
                );
            }
        }
    }

    fn restore_search_tree_selection(
        &mut self,
        preferred_selection: Option<PathBuf>,
        fallback_position: usize,
    ) {
        let mut candidate = preferred_selection;

        while let Some(path) = candidate {
            if let Some(position) = self.tree_rows.iter().position(|row| row.entry.path == path) {
                self.selected = position;

                self.list_offset = self.list_offset.min(self.tree_rows.len().saturating_sub(1));

                return;
            }

            if path == self.current_directory {
                break;
            }

            candidate = path.parent().map(PathBuf::from);
        }

        self.selected = fallback_position.min(self.tree_rows.len().saturating_sub(1));

        self.list_offset = self.list_offset.min(self.tree_rows.len().saturating_sub(1));
    }

    fn restore_manual_tree(&mut self) {
        let saved_selection = self.tree_search_saved_selection.take();

        let saved_offset = self.tree_search_saved_offset;

        /*
         * Recursive Tree mode and ordinary Tree mode have different backing
         * hierarchies.
         *
         * Clearing a recursive Tree query must restore the queryless recursive
         * hierarchy from search_tree_children. Calling reset_tree() here would
         * replace it with the ordinary lazy tree_children map, leaving Right
         * with no recursive hierarchy to expand.
         */
        if self.recursive_mode {
            self.rebuild_recursive_search_tree(saved_selection);

            self.list_offset = saved_offset.min(self.tree_rows.len().saturating_sub(1));

            self.ensure_selection_visible(self.viewport_rows);

            return;
        }

        /*
         * Ordinary Tree search temporarily replaces only the visible Tree rows.
         *
         * tree_children and expanded_directories still contain the manually
         * browsed hierarchy that existed before the search began. Rebuild from
         * that retained state rather than calling reset_tree(), which would erase
         * every remembered branch expansion.
         */
        self.rebuild_tree_rows(saved_selection.clone());

        self.refresh_tree_filter();

        if let Some(saved_selection) = saved_selection
            && let Some(position) = self.filtered_tree_indices.iter().position(|tree_index| {
                self.tree_rows
                    .get(*tree_index)
                    .is_some_and(|row| row.entry.path == saved_selection)
            })
        {
            self.selected = position;
        }

        self.list_offset = saved_offset.min(self.filtered_tree_indices.len().saturating_sub(1));

        self.ensure_selection_visible(self.viewport_rows);
    }

    fn select_parent_in_search_tree(&mut self) {
        let Some(row) = self.tree_row_at_filtered_position(self.selected).cloned() else {
            return;
        };

        let Some(parent) = row.entry.path.parent() else {
            return;
        };

        if parent == self.current_directory {
            return;
        }

        if let Some(position) = self
            .tree_rows
            .iter()
            .position(|candidate| candidate.entry.path == parent)
        {
            self.selected = position;
        }
    }

    fn refresh_tree_filter(&mut self) {
        let staged_deletions = &self.staged_deletions;

        /*
         * Queryless ordinary Tree browsing merely applies the current entry-type
         * filter to the existing visible hierarchy.
         */
        if !self.effective_query_is_active() {
            self.filtered_tree_indices = self
                .tree_rows
                .iter()
                .enumerate()
                .filter_map(|(index, row)| {
                    if path_belongs_to_staged_deletion(&row.entry.path, staged_deletions) {
                        return None;
                    }

                    if !entry_matches_visibility(
                        &row.entry,
                        &self.current_directory,
                        self.show_hidden,
                        self.hidden_only,
                    ) {
                        return None;
                    }

                    self.entry_filter.matches(&row.entry).then_some(index)
                })
                .collect();

            self.normalize_tree_selection();

            return;
        }

        /*
         * Fuzzy Tree results are already converted into a dedicated matched Tree by
         * process_fuzzy_messages(). Do not run Exact matching over those rows.
         */
        if self.search_mode == SearchMode::Fuzzy {
            self.filtered_tree_indices = (0..self.tree_rows.len()).collect();

            self.normalize_tree_selection();

            return;
        }

        let parsed_query = parse_query(&self.query);

        /*
         * First identify genuine Exact matches. Then retain only their ancestor
         * directories so each result remains connected and understandable in Tree
         * view. Unrelated siblings are deliberately excluded.
         */
        let mut included_paths: HashSet<PathBuf> = HashSet::new();

        for row in &self.tree_rows {
            let entry = &row.entry;

            if path_belongs_to_staged_deletion(&entry.path, staged_deletions) {
                continue;
            }

            if !entry_matches_visibility(
                entry,
                &self.current_directory,
                self.show_hidden,
                self.hidden_only,
            ) {
                continue;
            }

            if !self.entry_filter.matches(entry) {
                continue;
            }

            if !entry_matches_query(entry, &parsed_query) {
                continue;
            }

            included_paths.insert(entry.path.clone());

            let mut ancestor = entry.path.parent();

            while let Some(path) = ancestor {
                if path == self.current_directory {
                    break;
                }

                included_paths.insert(path.to_path_buf());

                ancestor = path.parent();
            }
        }

        self.filtered_tree_indices = self
            .tree_rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| included_paths.contains(&row.entry.path).then_some(index))
            .collect();

        self.normalize_tree_selection();
    }

    fn normalize_tree_selection(&mut self) {
        if self.filtered_tree_indices.is_empty() {
            self.selected = 0;
            self.list_offset = 0;
        } else {
            self.selected = self
                .selected
                .min(self.filtered_tree_indices.len().saturating_sub(1));

            self.list_offset = self
                .list_offset
                .min(self.filtered_tree_indices.len().saturating_sub(1));
        }
    }
}

fn connect_profile_worker(
    target: SshTarget,
    start_directory: String,
    sort_mode: SortMode,
    sort_descending: bool,
    ssh_config: SshConfig,
) -> Result<ConnectionWorkerSuccess, String> {
    let (remote_home, mut source) =
        SftpSource::connect(&target, &ssh_config).map_err(|error| error.to_string())?;

    let directory = resolve_remote_start_directory(&remote_home, &start_directory);

    let entries = source
        .read_directory(&directory, sort_mode, sort_descending)
        .map_err(|error| {
            format!(
                "Connected successfully, but unable to open {}: {}",
                directory.display(),
                error,
            )
        })?;

    Ok(ConnectionWorkerSuccess {
        source: Box::new(source),

        target,

        directory,

        home_directory: remote_home,

        entries,
    })
}

fn resolve_remote_start_directory(remote_home: &Path, value: &str) -> PathBuf {
    let value = value.trim();

    if value.is_empty() || value == "~" || value == "~/" {
        return remote_home.to_path_buf();
    }

    if let Some(relative) = value.strip_prefix("~/") {
        return remote_home.join(relative);
    }

    let path = PathBuf::from(value);

    if path.is_absolute() {
        path
    } else {
        remote_home.join(path)
    }
}

fn expand_local_identity_path(value: &str) -> Result<Option<PathBuf>, String> {
    let value = value.trim();

    if value.is_empty() {
        return Ok(None);
    }

    let path = if value == "~" {
        local_home_directory()?
    } else if let Some(relative) = value.strip_prefix("~/") {
        local_home_directory()?.join(relative)
    } else {
        PathBuf::from(value)
    };

    if !path.is_file() {
        return Err(format!("Identity file does not exist: {}", path.display(),));
    }

    Ok(Some(path))
}

fn local_home_directory() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set; unable to expand the identity-file path".to_string())
}

fn normalize_start_path(start_path: PathBuf) -> io::Result<PathBuf> {
    let canonical = std::fs::canonicalize(start_path)?;

    if canonical.is_dir() {
        return Ok(canonical);
    }

    let Some(parent) = canonical.parent() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "starting path has no parent directory",
        ));
    };

    Ok(parent.to_path_buf())
}

/*
 * Convert an absolute remote path into a safe relative path beneath the batch
 * download directory.
 *
 * Example:
 *
 *     /home/ferusx/docs/report.pdf
 *
 * becomes:
 *
 *     home/ferusx/docs/report.pdf
 *
 * Parent-directory components are rejected so a malformed remote path can
 * never escape the chosen destination root.
 */
fn safe_batch_relative_path(remote_path: &Path) -> io::Result<PathBuf> {
    use std::path::Component;

    let mut relative_path = PathBuf::new();

    for component in remote_path.components() {
        match component {
            Component::RootDir | Component::CurDir => {}

            Component::Normal(component) => {
                relative_path.push(component);
            }

            Component::ParentDir => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "remote path contains a parent-directory component: {}",
                        remote_path.display(),
                    ),
                ));
            }

            Component::Prefix(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "remote path contains an unsupported platform prefix: {}",
                        remote_path.display(),
                    ),
                ));
            }
        }
    }

    if relative_path.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "remote path does not contain a downloadable filename: {}",
                remote_path.display(),
            ),
        ));
    }

    Ok(relative_path)
}

/*
 * Choose a unique filename directly beneath the batch root.
 *
 * Files from different remote directories may share the same basename. Add a
 * numeric suffix before the extension rather than overwriting an earlier item.
 *
 * Examples:
 *
 *     report.pdf
 *     report-2.pdf
 *     report-3.pdf
 *
 *     LICENSE
 *     LICENSE-2
 */
fn unique_flat_batch_destination(
    destination_root: &Path,
    filename: &str,
    reserved_paths: &mut HashSet<PathBuf>,
) -> io::Result<PathBuf> {
    let filename_path = Path::new(filename);

    let stem = filename_path
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("remote file has no usable filename: {}", filename),
            )
        })?;

    let extension = filename_path
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty());

    for suffix in 1_u32..=100_000 {
        let candidate_name = if suffix == 1 {
            filename.to_string()
        } else {
            match extension {
                Some(extension) => {
                    format!("{}-{}.{}", stem, suffix, extension)
                }

                None => {
                    format!("{}-{}", stem, suffix)
                }
            }
        };

        let candidate = destination_root.join(candidate_name);

        /*
         * Check both the precomputed queue and the filesystem. The latter
         * protects retries or externally created files inside the batch root.
         */
        if !reserved_paths.contains(&candidate) && !candidate.exists() {
            reserved_paths.insert(candidate.clone());

            return Ok(candidate);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "unable to choose a unique download name for {} inside {}",
            filename,
            destination_root.display(),
        ),
    ))
}

/*
 * Remove empty directories left behind by a cancelled batch download.
 *
 * The batch destination root is newly created by Scry. Walk its directories
 * recursively and remove them from the deepest level upward.
 *
 * Files, symlinks, and every directory containing anything are preserved.
 */
fn remove_empty_batch_directories(directory: &Path) -> io::Result<()> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,

        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(());
        }

        Err(error) => {
            return Err(error);
        }
    };

    for entry in entries {
        let entry = entry?;

        /*
         * file_type().is_dir() does not follow directory symlinks.
         * Cleanup must never walk outside Scry's destination tree through one.
         */
        if entry.file_type()?.is_dir() {
            remove_empty_batch_directories(&entry.path())?;
        }
    }

    /*
     * Reopen the directory after cleaning its children. The first traversal
     * described its state before descendant directories were removed.
     */
    let mut remaining_entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,

        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(());
        }

        Err(error) => {
            return Err(error);
        }
    };

    match remaining_entries.next() {
        None => match std::fs::remove_dir(directory) {
            Ok(()) => Ok(()),

            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),

            Err(error) => Err(error),
        },

        Some(Ok(_)) => Ok(()),

        Some(Err(error)) => Err(error),
    }
}

/*
 * Create a new visible batch directory inside the local directory from which
 * the SSH session was entered.
 *
 * A numeric suffix protects against two batches starting during the same
 * second or against an older directory already using the timestamp.
 */
fn create_batch_download_directory(local_directory: &Path) -> io::Result<PathBuf> {
    let timestamp = Local::now().format("%Y-%m-%d-%H%M%S");

    let base_name = format!("scry-download-{}", timestamp);

    for suffix in 0_u32..10_000 {
        let directory_name = if suffix == 0 {
            base_name.clone()
        } else {
            format!("{}-{}", base_name, suffix)
        };

        let candidate = local_directory.join(directory_name);

        match std::fs::create_dir(&candidate) {
            Ok(()) => {
                return Ok(candidate);
            }

            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                continue;
            }

            Err(error) => {
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "unable to create batch download directory {}: {}",
                        candidate.display(),
                        error,
                    ),
                ));
            }
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "unable to create a unique batch download directory inside {}",
            local_directory.display(),
        ),
    ))
}

#[cfg(target_os = "linux")]
fn copy_path_to_clipboard(path_text: &str, app: &mut App) -> io::Result<()> {
    if app.clipboard.is_none() {
        let context =
            ClipboardContext::new().map_err(|error| io::Error::other(error.to_string()))?;

        app.clipboard = Some(AppClipboard(context));
    }

    app.clipboard
        .as_mut()
        .expect("clipboard was initialized above")
        .0
        .set_contents(path_text.to_string())
        .map_err(|error| io::Error::other(error.to_string()))
}

#[cfg(target_os = "freebsd")]
fn copy_path_to_clipboard(path_text: &str, _app: &mut App) -> io::Result<()> {
    crate::clipboard::copy_with_osc52(path_text)
}

#[cfg(test)]
mod tests {
    use super::{STAGED_DELETION_PREFIX, staged_deletion_path};

    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn staged_deletion_path_is_hidden_and_preserves_original_name() {
        let directory = temporary_deletion_test_directory("hidden-name");

        fs::create_dir_all(&directory).expect("create temporary staged-deletion test directory");

        let original_path = directory.join("example report.txt");

        fs::write(&original_path, b"test").expect("create original staged-deletion test file");

        let staged_path =
            staged_deletion_path(&original_path).expect("generate staged deletion path");

        let staged_name = staged_path
            .file_name()
            .expect("staged path should have a filename")
            .to_string_lossy();

        assert!(
            staged_name.starts_with(STAGED_DELETION_PREFIX),
            "staged filename should begin with Scry's private prefix: {staged_name}",
        );

        assert!(
            staged_name.ends_with("example report.txt"),
            "staged filename should preserve the original basename: {staged_name}",
        );

        assert_eq!(
            staged_path.parent(),
            original_path.parent(),
            "staged deletion must remain beside the original path",
        );

        assert!(
            fs::symlink_metadata(&staged_path).is_err(),
            "the generated staged path should not already exist",
        );

        fs::remove_dir_all(&directory).expect("remove temporary staged-deletion test directory");
    }

    #[test]
    fn consecutive_staged_deletion_paths_are_distinct() {
        let directory = temporary_deletion_test_directory("distinct-names");

        fs::create_dir_all(&directory).expect("create temporary staged-deletion test directory");

        let original_path = directory.join("same-name.txt");

        let first =
            staged_deletion_path(&original_path).expect("generate first staged deletion path");

        let second =
            staged_deletion_path(&original_path).expect("generate second staged deletion path");

        assert_ne!(
            first, second,
            "consecutive staged paths must always be distinct",
        );

        fs::remove_dir_all(&directory).expect("remove temporary staged-deletion test directory");
    }

    fn temporary_deletion_test_directory(label: &str) -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();

        std::env::temp_dir().join(format!(
            "scry-staged-deletion-test-{}-{}-{}",
            label,
            std::process::id(),
            timestamp,
        ))
    }
}

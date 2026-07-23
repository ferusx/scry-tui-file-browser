// SPDX-License-Identifier: BSD-3-Clause

use chrono::Local;
use cli_clipboard::{ClipboardContext, ClipboardProvider};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, TryRecvError},
};
use std::thread;
use std::time::{Duration, Instant};
use users::get_user_by_uid;

use crate::classify::{FileClass, inspect_file};
use crate::config::{ScryConfig, SshConfig};
use crate::connection::{ConnectionDialogState, ConnectionStore};
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
 * Recursive indexes may contain millions of records.
 *
 * Query text is drawn immediately, but background searching waits briefly for
 * a natural typing pause so one rapid word does not launch one complete worker
 * generation per character.
 */
const RECURSIVE_SEARCH_DEBOUNCE: Duration = Duration::from_millis(75);

/*
 * Exact List mode remains unlimited.
 *
 * Exact Tree mode is a navigational representation and therefore retains only
 * a bounded number of direct matches before connecting their ancestors.
 */
pub(crate) const EXACT_TREE_MATCH_LIMIT: usize = 5_000;

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
}

#[derive(Debug, Clone)]
struct NavigationState {
    selected_path: Option<PathBuf>,
    list_offset: usize,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct FuzzyRequestIdentity {
    query: String,

    scope_directory: PathBuf,

    recursive_mode: bool,

    show_hidden: bool,

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

#[derive(Debug)]
struct RemoteIndexLoadResult {
    result: Result<LoadedRemoteIndex, String>,
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

struct AppClipboard(ClipboardContext);

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
     * Stable lookup from a scanned path to its current position in
     * recursive_entries.
     *
     * Fuzzy Tree results use this to retrieve only their required ancestors
     * without rebuilding a path map across the complete recursive corpus after
     * every worker update.
     */
    recursive_path_indices: HashMap<PathBuf, usize>,

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

    pub entry_filter: EntryFilter,

    pub allow_file_opening: bool,

    /*
     * Close Scry only after an external opener has been launched successfully.
     *
     * Failed opens and directory navigation leave the application running.
     */
    pub exit_on_open: bool,

    pub theme: Theme,

    pub show_hidden: bool,

    pub show_icons: bool,

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

    clipboard: Option<AppClipboard>,

    last_copied_path: Option<String>,

    pub file_info: Option<FileInfoState>,

    file_info_generation: u64,

    file_info_receiver: Option<Receiver<FileInfoMessage>>,

    pub transfer: Option<TransferState>,

    pub enable_deletion: bool,

    pub deletion: Option<DeletionState>,

    pub list_offset: usize,

    pub viewport_rows: usize,

    pending_selection_path: Option<PathBuf>,

    /*
     * Viewport offset waiting for an asynchronous recursive scan or remote-index
     * load to make the restored selection visible.
     */
    pending_session_list_offset: Option<usize>,

    pub error_message: Option<String>,

    error_message_expires_at: Option<Instant>,

    /*
     * Non-error operational information shown in amber.
     */
    pub status_message: Option<String>,

    status_message_expires_at: Option<Instant>,

    pub should_quit: bool,

    pub scan_in_progress: bool,

    pub recursive_scan_partial: bool,

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

    remote_index_loaded: bool,

    remote_index_includes_hidden: bool,

    pub connection_store: ConnectionStore,

    pub connection_dialog: ConnectionDialogState,

    pub connection_in_progress: bool,

    pub ssh_config: SshConfig,

    connection_receiver: Option<Receiver<ConnectionWorkerResult>>,

    saved_local_session: Option<LocalSessionState>,

    pub help_scroll: u16,

    pub help_max_scroll: u16,

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

    tree_children: HashMap<PathBuf, Vec<FileEntry>>,

    directory_has_content_cache: HashMap<PathBuf, bool>,

    classification_inspection_cache: HashMap<PathBuf, FileClass>,

    expanded_directories: HashSet<PathBuf>,

    recursive_cache_complete: bool,

    scan_generation: u64,

    scan_receiver: Option<Receiver<ScanMessage>>,

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
     * True when the completed Exact Recursive Tree result exceeded the configured
     * direct-match cap.
     */
    pub exact_tree_limit_reached: bool,

    navigation_states: HashMap<PathBuf, NavigationState>,

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
fn entry_is_hidden_below(entry: &FileEntry, root: &Path) -> bool {
    let relative_path = entry.path.strip_prefix(root).unwrap_or(&entry.path);

    relative_path.components().any(|component| {
        let component = component.as_os_str().to_string_lossy();

        component != "." && component != ".." && component.starts_with('.')
    })
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
        let sort_mode = SortMode::Name;

        let sort_descending = false;

        let connection_store = ConnectionStore::load()?;

        let connection_dialog = ConnectionDialogState::new(&connection_store);

        let entries = source.read_directory(&current_directory, sort_mode, sort_descending)?;

        let mut app = Self {
            source,

            active_ssh_target: None,

            current_directory,

            home_directory,

            entries,

            marked_files: HashMap::new(),

            recursive_entries: Vec::new(),

            recursive_path_indices: HashMap::new(),

            recursive_child_indices: HashMap::new(),

            search_index: Arc::new(SearchIndex::new()),

            filtered_indices: Vec::new(),

            query: String::new(),

            query_cursor: 0,

            search_mode: SearchMode::Exact,

            entry_filter: EntryFilter::All,

            allow_file_opening: true,

            exit_on_open: false,

            theme: Theme::default(),

            show_hidden: false,

            show_icons: true,

            show_permissions: false,

            show_date: false,

            show_size: false,

            show_user: false,

            show_details: true,

            show_selection: true,

            show_columns: true,

            sort_mode,

            sort_descending,

            clipboard: None,

            last_copied_path: None,

            file_info: None,

            file_info_generation: 0,

            file_info_receiver: None,

            transfer: None,

            enable_deletion: false,

            deletion: None,

            selected: 0,

            scrollbar_drag_active: false,

            list_offset: 0,

            viewport_rows: 1,

            pending_selection_path: None,

            pending_session_list_offset: None,

            error_message: None,

            error_message_expires_at: None,

            status_message: None,

            status_message_expires_at: None,

            should_quit: false,

            scan_in_progress: false,

            recursive_scan_partial: false,

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

            remote_index_loaded: false,

            remote_index_includes_hidden: false,

            connection_store,

            connection_dialog,

            ssh_config: SshConfig::default(),

            connection_in_progress: false,

            connection_receiver: None,

            saved_local_session: None,

            help_scroll: 0,

            help_max_scroll: 0,

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

            tree_children: HashMap::new(),

            directory_has_content_cache: HashMap::new(),

            classification_inspection_cache: HashMap::new(),

            expanded_directories: HashSet::new(),

            recursive_cache_complete: false,

            scan_generation: 0,

            scan_receiver: None,

            fuzzy_generation: 0,

            fuzzy_receiver: None,

            fuzzy_cancel_signal: None,

            active_fuzzy_request: None,

            pending_recursive_search_at: None,

            fuzzy_filter_in_progress: false,

            fuzzy_examined: 0,

            fuzzy_total: 0,

            exact_tree_limit_reached: false,

            navigation_states: HashMap::new(),

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

        self.enable_deletion = config.features.enable_deletion;

        self.allow_file_opening = config.features.allow_file_opening;

        self.exit_on_open = config.features.exit_on_open;

        self.show_icons = config.display.show_icons;

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

        if config.browser.view == "tree" && self.view_mode != ViewMode::Tree {
            self.toggle_tree_mode();
        }
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
                if self.search_mode == SearchMode::Fuzzy
                    && !self.query.is_empty()
                    && self.query != "."
                {
                    self.start_current_fuzzy_filter();
                } else {
                    self.rebuild_recursive_search_tree(selected_path.clone());
                }
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

    pub fn active_entry_count(&self) -> usize {
        self.active_entries().len()
    }

    pub fn source_label(&self) -> String {
        self.source.source_label()
    }

    /*
     * Timed informational and success notification.
     *
     * These messages are displayed in the normal amber status color and disappear
     * automatically after five seconds.
     */
    pub fn show_info_message(&mut self, message: impl Into<String>) {
        self.error_message = None;

        self.error_message_expires_at = None;

        self.status_message = Some(message.into());

        self.status_message_expires_at = Some(Instant::now() + INFO_NOTIFICATION_DURATION);
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
    }

    pub fn clear_messages(&mut self) {
        self.error_message = None;

        self.error_message_expires_at = None;

        self.status_message = None;

        self.status_message_expires_at = None;
    }

    /*
     * Called by the event loop even when no keyboard or mouse input occurs.
     *
     * Returning true requests a redraw so the expired notification disappears
     * immediately rather than waiting for the next user action.
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

            changed = true;
        }

        changed
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
        let source = match self.active_ssh_target.as_ref() {
            Some(target) => SessionSource::Ssh {
                host: target.host.clone(),

                user: target.user.clone(),

                port: target.port,

                identity_file: target.identity_file.clone(),

                directory: self.current_directory.clone(),

                home_directory: self.home_directory.clone(),
            },

            None => SessionSource::Local {
                directory: self.current_directory.clone(),

                home_directory: self.home_directory.clone(),
            },
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

        Ok(SessionState {
            version: SESSION_FORMAT_VERSION,

            source,

            selected_path: self.selected_entry().map(|entry| entry.path.clone()),

            list_offset: self.list_offset,

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

            show_icons: self.show_icons,

            show_details: self.show_details,

            show_selection: self.show_selection,

            show_columns: self.show_columns,

            show_permissions: self.show_permissions,

            show_size: self.show_size,

            show_date: self.show_date,

            show_user: self.show_user,

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

        /*
         * The startup configuration has already established view, search, recursive,
         * sorting, filtering, hidden-entry, and panel state.
         *
         * Install the query afterward so it is evaluated using those final modes.
         */
        self.set_startup_query(state.query.clone());

        /*
         * Non-recursive and already-loaded states can restore immediately.
         *
         * Recursive scans and remote-index loads retain the pending values and call
         * restore_pending_selection_if_available() when their results arrive.
         */
        self.restore_pending_selection_if_available();

        if self.pending_selection_path.is_none() {
            if let Some(saved_offset) = self.pending_session_list_offset.take() {
                self.list_offset = saved_offset;

                self.ensure_selection_visible(self.viewport_rows);
            }
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
        let local_directory = match self.saved_local_session.as_ref() {
            Some(session) => session.directory.clone(),

            None => std::env::current_dir().map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!(
                        "unable to determine the local batch-download directory: {}",
                        error,
                    ),
                )
            })?,
        };

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

        if self.clipboard.is_none() {
            match ClipboardContext::new() {
                Ok(context) => {
                    self.clipboard = Some(AppClipboard(context));
                }

                Err(error) => {
                    self.show_error_message(format!(
                        "Unable to access the system clipboard: {}",
                        error,
                    ));

                    return;
                }
            }
        }

        let result = self
            .clipboard
            .as_mut()
            .expect("clipboard was initialized above")
            .0
            .set_contents(path_text.clone());

        match result {
            Ok(()) => {
                self.last_copied_path = Some(path_text.clone());

                self.show_info_message(format!("Copied path: {}", path_text));
            }

            Err(error) => {
                self.clipboard = None;

                self.show_error_message(format!("Unable to copy path to clipboard: {}", error,));
            }
        }
    }

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

                    if let Some(state) = self.file_info.as_mut() {
                        if state.loading {
                            state.fail(
                                "The file-information worker stopped unexpectedly".to_string(),
                            );
                        }
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
                    state.finish(info);
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

        match message.result {
            Ok(mut loaded) => {
                /*
                 * Exact mode displays the backing corpus directly.
                 *
                 * Preserve normal Scry sorting. Fuzzy mode deliberately keeps
                 * scanner/cache order because relevance owns its result order.
                 */
                if self.search_mode == SearchMode::Exact {
                    sort_entries(&mut loaded.entries, self.sort_mode, self.sort_descending);
                }

                self.cancel_fuzzy_filter();

                self.recursive_entries = loaded.entries;

                self.rebuild_recursive_path_indices();

                self.search_index = Arc::new(SearchIndex::from_entries(&self.recursive_entries));

                self.recursive_cache_complete = true;

                self.recursive_scan_partial = loaded.info.partial;

                self.scan_in_progress = false;

                self.remote_index_loaded = true;

                self.remote_index_includes_hidden = loaded.info.includes_hidden;

                self.recursive_mode = true;

                self.selected = 0;

                self.list_offset = 0;

                self.error_message = None;

                self.show_info_message(format!(
                    "Remote index loaded — {} entries",
                    loaded.info.entry_count,
                ));

                match self.view_mode {
                    ViewMode::List => {
                        self.refresh_filter();

                        self.restore_pending_selection_if_available();
                    }

                    ViewMode::Tree => {
                        let selected_path = self.pending_selection_path.clone();

                        self.rebuild_recursive_search_tree(selected_path);

                        self.restore_pending_selection_if_available();
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

    pub fn process_scan_messages(&mut self) -> bool {
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

                        self.recursive_path_indices
                            .insert(entry.path.clone(), future_index);

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

                    /*
                     * Exact recursive List mode consumes recursive_entries in display order,
                     * so its complete backing vector must remain sorted.
                     *
                     * Fuzzy mode ranks only its bounded result set. Sorting millions of backing
                     * entries here would freeze the UI and provide no benefit.
                     */
                    if self.view_mode == ViewMode::List && self.search_mode == SearchMode::Exact {
                        sort_entries(
                            &mut self.recursive_entries,
                            self.sort_mode,
                            self.sort_descending,
                        );

                        self.rebuild_recursive_path_indices();

                        Arc::make_mut(&mut self.search_index)
                            .rebuild_from_entries(&self.recursive_entries);
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

            if self.view_mode == ViewMode::Tree
                && self.recursive_search_active()
                && text_filter_active
            {
                /*
                 * Only a genuine effective query owns the recursive search Tree.
                 *
                 * Directive-only and incomplete queries such as `type:` remain visible in
                 * the search field but continue displaying the ordinary browsing Tree.
                 */
                if scan_finished {
                    let selected_path = self
                        .pending_selection_path
                        .clone()
                        .or_else(|| self.selected_entry().map(|entry| entry.path.clone()));

                    match self.search_mode {
                        SearchMode::Fuzzy => {
                            self.start_current_fuzzy_filter();
                        }

                        SearchMode::Exact => {
                            self.rebuild_recursive_search_tree(selected_path);

                            self.restore_pending_selection_if_available();
                        }
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

    pub fn toggle_selection_panel(&mut self) {
        self.show_selection = !self.show_selection;
    }

    #[allow(dead_code)]
    pub fn toggle_columns_panel(&mut self) {
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
        let selected_path = self.selected_entry().map(|entry| entry.path.clone());

        self.show_hidden = !self.show_hidden;

        self.directory_has_content_cache.clear();

        if self.source.is_remote() && self.remote_index_loaded {
            self.cancel_fuzzy_filter();

            if self.show_hidden && !self.remote_index_includes_hidden {
                self.error_message = Some(
                    "This remote index contains standard entries only; \
                    rebuild it to include dot-entries"
                        .to_string(),
                );
            }
        } else {
            self.invalidate_recursive_cache();
        }

        if self.view_mode == ViewMode::Tree {
            if self.recursive_search_active() {
                self.ensure_recursive_scan();

                self.rebuild_recursive_search_tree(selected_path);
            } else {
                self.reset_tree();
            }

            return;
        }

        if self.recursive_search_active() {
            self.ensure_recursive_scan();
        }

        self.selected = 0;

        self.list_offset = 0;

        self.refresh_filter();
    }

    pub fn toggle_search_mode(&mut self) {
        /*
         * Changing the search interpretation creates a new active search state.
         * An older suspended-search bookmark must not later overwrite it.
         */
        self.search_return_state = None;

        self.search_navigation_active = false;

        /*
         * Preserve the selected path while the result set is rebuilt.
         */
        let selected_path = self.selected_entry().map(|entry| entry.path.clone());

        self.pending_selection_path = selected_path.clone();

        self.search_mode = match self.search_mode {
            SearchMode::Exact => SearchMode::Fuzzy,

            SearchMode::Fuzzy => SearchMode::Exact,
        };

        /*
         * Fuzzy recursive mode deliberately leaves recursive_entries in scanner
         * arrival order because ranking its bounded result set does not require the
         * enormous backing vector to be sorted.
         *
         * When switching back to Exact mode, however, that backing vector becomes
         * the displayed result order. Sort it once here and rebuild the compact index
         * so every SearchRecord continues to point at the correct FileEntry.
         */
        if self.search_mode == SearchMode::Exact && self.recursive_search_active() {
            sort_entries(
                &mut self.recursive_entries,
                self.sort_mode,
                self.sort_descending,
            );

            self.rebuild_recursive_path_indices();

            Arc::make_mut(&mut self.search_index).rebuild_from_entries(&self.recursive_entries);
        }

        match self.view_mode {
            ViewMode::List => {
                self.refresh_filter();

                self.restore_pending_selection_if_available();
            }

            ViewMode::Tree => {
                if self.recursive_search_active() {
                    match self.search_mode {
                        SearchMode::Fuzzy => {
                            self.start_current_fuzzy_filter();
                        }

                        SearchMode::Exact => {
                            self.rebuild_recursive_search_tree(selected_path.clone());

                            self.restore_pending_selection_if_available();
                        }
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

        self.pending_selection_path = selected_path.clone();

        self.search_return_state = None;

        self.search_navigation_active = false;

        self.recursive_mode = true;

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
                    self.rebuild_recursive_search_tree(selected_path.clone());

                    self.restore_pending_selection_if_available();
                }
            }
        }
    }

    fn prepare_remote_recursive_mode(&mut self) -> bool {
        /*
         * A running host-wide build is independent from recursive-mode display.
         *
         * Alt+R must not begin another build, toggle the current mode, or inspect
         * a part file while the existing build is still active.
         */
        if self.remote_index_build_in_progress {
            self.show_info_message(format!(
                "Remote index is still building — {} entries written",
                self.remote_index_entries_written,
            ));

            return false;
        }

        /*
         * Loading is also a single background operation.
         *
         * Repeated Alt+R presses while loading must not start duplicate loaders.
         */
        if self.remote_index_load_in_progress {
            self.show_info_message("Remote index is still loading…");

            return false;
        }

        /*
         * Once installed in memory, the host-wide corpus is immediately reusable.
         *
         * No disk inspection, SFTP scan, or second load is required.
         */
        if self.remote_index_loaded {
            return true;
        }

        let Some(identity) = self.source.remote_index_identity() else {
            return true;
        };

        let mut status = match identity.inspect() {
            Ok(status) => status,

            Err(error) => {
                self.show_error_message(format!(
                    "Unable to inspect the remote index for {}: {}",
                    identity.display_label(),
                    error,
                ));

                return false;
            }
        };

        /*
         * Compatibility with indexes created through an OpenSSH alias where the
         * username was omitted from Scry's command:
         *
         *     scry --ssh nosferatu
         *
         * OpenSSH may still resolve that alias to the same account as:
         *
         *     ferusx@nosferatu
         *
         * Older indexes therefore use the `default-user` identity. Prefer the exact
         * explicit-user identity, but when it is missing, reuse a valid legacy index
         * rather than prompting for an unnecessary rebuild.
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
                    self.show_error_message(format!(
                        "Unable to inspect the compatible remote index for {}: {}",
                        legacy_identity.display_label(),
                        error,
                    ));

                    return false;
                }
            }
        }

        match status {
            crate::remote_index::RemoteIndexStatus::Missing => {
                self.remote_index_setup = Some(RemoteIndexSetupState {
                    identity,

                    purpose: RemoteIndexDialogPurpose::InitialSetup,

                    includes_hidden: false,

                    focus: RemoteIndexDialogFocus::Policy,

                    invalid_reason: None,
                });

                self.overlay = Overlay::RemoteIndexSetup;

                false
            }

            crate::remote_index::RemoteIndexStatus::Invalid { reason, .. } => {
                self.remote_index_setup = Some(RemoteIndexSetupState {
                    identity,

                    purpose: RemoteIndexDialogPurpose::InitialSetup,

                    includes_hidden: false,

                    focus: RemoteIndexDialogFocus::Policy,

                    invalid_reason: Some(reason),
                });

                self.overlay = Overlay::RemoteIndexSetup;

                false
            }

            crate::remote_index::RemoteIndexStatus::Valid(info) => {
                self.begin_remote_index_load(info.identity);

                false
            }
        }
    }

    fn begin_remote_index_load(&mut self, identity: RemoteIndexIdentity) {
        if self.remote_index_load_in_progress {
            return;
        }

        let (sender, receiver) = mpsc::channel();

        thread::spawn(move || {
            let result = load_remote_index(&identity).map_err(|error| error.to_string());

            let _ = sender.send(RemoteIndexLoadResult { result });
        });

        self.remote_index_load_receiver = Some(receiver);

        self.remote_index_load_in_progress = true;

        self.show_persistent_info_message("Loading persistent remote index, please wait...");
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
                /*
                 * Recursive Tree rows belong to the complete descendant corpus.
                 *
                 * Rebuild the ordinary Tree from the current directory, then apply the
                 * retained Exact or Fuzzy query to that local hierarchy.
                 */
                self.selected = 0;

                self.list_offset = 0;

                self.reset_tree();

                self.refresh_tree_filter();

                if let Some(path) = selected_path {
                    self.select_visible_path(&path);
                }
            }
        }

        self.ensure_selection_visible(self.viewport_rows);
    }

    pub fn toggle_tree_mode(&mut self) {
        match self.view_mode {
            ViewMode::List => {
                self.view_mode = ViewMode::Tree;

                self.selected = 0;

                self.list_offset = 0;

                let selected_path = self.selected_entry().map(|entry| entry.path.clone());

                if self.recursive_mode && self.effective_query_is_active() {
                    /*
                     * A genuine recursive query becomes a recursive result Tree.
                     */
                    self.ensure_recursive_scan();

                    if !self.scan_in_progress {
                        match self.search_mode {
                            SearchMode::Exact => {
                                self.rebuild_recursive_search_tree(selected_path);
                            }

                            SearchMode::Fuzzy => {
                                self.pending_selection_path = selected_path;

                                self.start_current_fuzzy_filter();
                            }
                        }
                    }
                } else {
                    /*
                     * Empty, incomplete, and directive-only queries are ordinary browsing.
                     *
                     * Keep the visible query text, but do not treat it as a Tree search.
                     */
                    self.reset_tree();

                    self.refresh_tree_filter();

                    if let Some(path) = selected_path {
                        self.select_visible_path(&path);
                    }
                }
            }

            ViewMode::Tree => {
                self.view_mode = ViewMode::List;

                self.selected = 0;

                self.list_offset = 0;

                if self.recursive_search_active() && self.search_mode == SearchMode::Exact {
                    sort_entries(
                        &mut self.recursive_entries,
                        self.sort_mode,
                        self.sort_descending,
                    );

                    self.rebuild_recursive_path_indices();

                    Arc::make_mut(&mut self.search_index)
                        .rebuild_from_entries(&self.recursive_entries);
                }

                self.refresh_filter();
            }
        }
    }

    pub fn cycle_sort_mode(&mut self) {
        self.sort_mode = self.sort_mode.next();

        self.apply_sort();
    }

    pub fn toggle_sort_direction(&mut self) {
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
                /*
                 * Exact recursive List mode displays the backing entries in the selected
                 * sort order.
                 *
                 * Fuzzy mode owns its relevance ordering and must never sort millions of
                 * backing records merely because a display sort command was issued.
                 */
                if self.recursive_search_active() && self.search_mode == SearchMode::Exact {
                    sort_entries(
                        &mut self.recursive_entries,
                        self.sort_mode,
                        self.sort_descending,
                    );

                    self.rebuild_recursive_path_indices();

                    Arc::make_mut(&mut self.search_index)
                        .rebuild_from_entries(&self.recursive_entries);
                }

                self.refresh_filter();

                if let Some(path) = selected_path {
                    self.select_visible_path(&path);
                }
            }

            ViewMode::Tree if self.recursive_search_active() => {
                /*
                 * Do not sort recursive_entries here.
                 *
                 * rebuild_recursive_search_tree() groups entries by parent and
                 * sorts each resulting sibling vector. Sorting the enormous flat
                 * recursive vector first would be redundant.
                 *
                 * Also do not sort the existing search_tree_children map because
                 * that map is cleared and recreated by the rebuild below.
                 */
                self.rebuild_recursive_search_tree(selected_path);
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

    pub fn page_up(&mut self) {
        let amount = self.page_amount();

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

        let name = get_user_by_uid(owner_id)
            .map(|user| user.name().to_string_lossy().into_owned())
            .unwrap_or_else(|| owner_id.to_string());

        self.owner_name_cache.insert(owner_id, name.clone());

        name
    }

    fn path_is_directory(&mut self, path: &PathBuf, fallback: bool) -> bool {
        self.source.path_is_directory(path).unwrap_or(fallback)
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

        self.change_directory(target, None);
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
         * change_directory() normally clears the query, so preserve it explicitly.
         */
        let preserve_inactive_query = !self.query.is_empty() && !self.effective_query_is_active();

        let preserved_query = preserve_inactive_query.then(|| self.query.clone());

        let preserved_query_cursor = self.query_cursor;

        if !self.change_directory(path, None) {
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

        if self.recursive_mode && self.effective_query_is_active() {
            /*
             * A genuine query owns the recursive result Tree.
             */
            self.ensure_recursive_scan();

            if !self.scan_in_progress {
                match self.search_mode {
                    SearchMode::Exact => {
                        self.rebuild_recursive_search_tree(None);
                    }

                    SearchMode::Fuzzy => {
                        self.start_current_fuzzy_filter();
                    }
                }
            } else {
                self.tree_rows.clear();

                self.filtered_tree_indices.clear();

                self.search_tree_children.clear();
            }
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

        self.change_directory(home_directory, None);
    }

    pub fn enter_parent_directory(&mut self) {
        if self.restore_search_return_state() {
            return;
        }

        /*
         * Tree mode owns Left/Escape/middle-click navigation even while a search
         * query is active.
         *
         * Otherwise the query-root navigation route intercepts Left before the
         * selected branch can be collapsed.
         */
        if self.view_mode == ViewMode::Tree {
            self.collapse_selected_tree_directory_or_select_parent();

            return;
        }

        /*
         * List-mode search navigation may move the active search root upward while
         * retaining the query.
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

            self.change_search_root(parent, Some(previous_root));

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

        self.change_directory(parent, Some(child_directory));
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

        self.tree_children.clear();

        self.expanded_directories.clear();

        self.search_collapsed_directories.clear();

        self.search_tree_children.clear();

        /*
         * The root's immediate contents are already loaded in self.entries.
         */
        self.tree_children
            .insert(self.current_directory.clone(), self.entries.clone());

        self.rebuild_tree_rows(None);
    }
    fn expand_selected_tree_directory(&mut self) {
        if self.recursive_search_active() && self.effective_query_is_active() {
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

        self.expanded_directories.insert(path.clone());

        self.rebuild_tree_rows(Some(path));
    }

    fn collapse_selected_tree_directory_or_select_parent(&mut self) {
        /*
         * An empty Tree view has no selected TreeRow from which to determine a
         * parent. In that case, move the tree root itself one level upward.
         *
         * This commonly happens when a directory contains only hidden entries
         * while hidden files are disabled.
         */
        if self.current_visible_entry_count() == 0 {
            if self.effective_query_is_active() {
                self.move_recursive_tree_root_to_parent();
            } else {
                self.move_tree_root_to_parent();
            }

            return;
        }

        if self.effective_query_is_active() {
            let Some(row) = self.tree_row_at_filtered_position(self.selected).cloned() else {
                return;
            };

            let path = row.entry.path.clone();

            if row.entry.is_directory && row.expanded {
                self.collapse_selected_recursive_branch();

                return;
            }

            let Some(parent) = path.parent() else {
                return;
            };

            /*
             * The recursive tree does not render its root as a row.
             *
             * If the selected entry belongs directly to the current root,
             * moving left/back reroots Scry one directory higher.
             */
            if parent == self.current_directory {
                self.move_recursive_tree_root_to_parent();

                return;
            }

            self.select_parent_in_search_tree();

            return;
        }

        let Some(row) = self.tree_row_at_filtered_position(self.selected).cloned() else {
            return;
        };

        let path = row.entry.path.clone();

        if row.entry.is_directory && self.expanded_directories.remove(&path) {
            self.rebuild_tree_rows(Some(path));

            return;
        }

        let Some(parent) = path.parent() else {
            return;
        };

        /*
         * The current tree root itself is not rendered as a row.
         *
         * Once Left reaches that boundary, reroot the tree one level higher
         * and select the directory we just came from.
         */
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

    fn collapse_selected_recursive_branch(&mut self) {
        let Some(tree_index) = self.filtered_tree_indices.get(self.selected).copied() else {
            return;
        };

        let Some(row) = self.tree_rows.get(tree_index) else {
            return;
        };

        if !row.entry.is_directory || !row.expanded {
            return;
        }

        let path = row.entry.path.clone();

        let row_depth = row.ancestor_has_more.len();

        /*
         * Every descendant has a greater ancestry depth than the directory row.
         * Stop as soon as we encounter its next sibling or an ancestor's sibling.
         */
        let mut removal_end = tree_index.saturating_add(1);

        while removal_end < self.tree_rows.len()
            && self.tree_rows[removal_end].ancestor_has_more.len() > row_depth
        {
            removal_end += 1;
        }

        self.recursive_expanded_directories
            .retain(|expanded_path| expanded_path != &path && !expanded_path.starts_with(&path));

        if let Some(row) = self.tree_rows.get_mut(tree_index) {
            row.expanded = false;
        }

        if tree_index.saturating_add(1) < removal_end {
            self.tree_rows
                .drain(tree_index.saturating_add(1)..removal_end);
        }

        self.refresh_recursive_tree_indices();

        self.selected = self
            .selected
            .min(self.filtered_tree_indices.len().saturating_sub(1));

        self.ensure_selection_visible(self.viewport_rows);
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
         * Replace any bounded contextual child list with the complete immediate
         * matching branch recovered above.
         */
        self.search_tree_children
            .insert(path.clone(), recovered_children);

        /*
         * Search Tree visibility is governed by search_collapsed_directories.
         *
         * Removing the selected path from that set is the authoritative expansion
         * operation. Do not mix this with recursive_expanded_directories and manual
         * row splicing.
         */
        self.search_collapsed_directories.remove(&path);

        let fallback_position = self.selected;

        let mut rows = Vec::new();

        Self::append_recursive_search_children(
            self.current_directory.clone(),
            Vec::new(),
            &self.search_tree_children,
            &self.search_collapsed_directories,
            &mut rows,
        );

        self.tree_rows = rows;

        self.filtered_tree_indices = (0..self.tree_rows.len()).collect();

        self.restore_search_tree_selection(Some(path), fallback_position);

        self.ensure_selection_visible(self.viewport_rows);

        self.error_message = None;
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

        self.ensure_recursive_scan();

        self.rebuild_recursive_search_tree(Some(previous_root));
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
        let selected_path = preserve_selection.or_else(|| {
            self.tree_row_at_filtered_position(self.selected)
                .map(|row| row.entry.path.clone())
        });

        self.tree_rows.clear();

        self.append_tree_children(self.current_directory.clone(), 0, Vec::new());

        if let Some(selected_path) = selected_path {
            if let Some(position) = self
                .tree_rows
                .iter()
                .position(|row| row.entry.path == selected_path)
            {
                self.selected = position;
            } else {
                self.selected = self.selected.min(self.tree_rows.len().saturating_sub(1));
            }
        } else {
            self.selected = self.selected.min(self.tree_rows.len().saturating_sub(1));
        }

        self.list_offset = self.list_offset.min(self.tree_rows.len().saturating_sub(1));

        self.refresh_tree_filter();
    }

    fn append_tree_children(
        &mut self,
        directory: PathBuf,
        depth: usize,
        ancestor_has_more: Vec<bool>,
    ) {
        let Some(children) = self.tree_children.get(&directory).cloned() else {
            return;
        };

        let visible_children: Vec<FileEntry> = children
            .into_iter()
            .filter(|entry| self.show_hidden || !entry.name.starts_with('.'))
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

                self.append_tree_children(child_path, depth + 1, child_ancestor_has_more);
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

                                transfer.local_path = Some(local_path);
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
                 * Every successfully downloaded file leaves the persistent marked set.
                 *
                 * Failed or unattempted files remain marked so the user can retry them.
                 */
                for path in &message.completed_paths {
                    self.marked_files.remove(path);
                }

                if message.cancelled {
                    self.transfer = None;

                    self.clear_messages();

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
            self.show_error_message(format!(
                "Unable to prepare {} for opening: {}",
                transfer.remote_path.display(),
                error,
            ));

            return;
        }

        let Some(local_path) = transfer.local_path else {
            self.show_error_message("Remote transfer completed without producing a local file");

            return;
        };

        match crate::open::open_file(&local_path) {
            Ok(()) => {
                if self.exit_on_open {
                    self.should_quit = true;
                } else {
                    self.show_info_message(format!("Opened {}", transfer.remote_path.display(),));
                }
            }

            Err(error) => {
                self.show_error_message(error);
            }
        }
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

    pub fn toggle_deletion_choice(&mut self) {
        let Some(deletion) = self.deletion.as_mut() else {
            return;
        };

        deletion.choice = match deletion.choice {
            DeletionChoice::Delete => DeletionChoice::Cancel,

            DeletionChoice::Cancel => DeletionChoice::Delete,
        };
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

        let deletion_result = if file_type.is_symlink() {
            std::fs::remove_file(&path)
        } else if file_type.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };

        if let Err(error) = deletion_result {
            self.show_error_message(format!("Unable to delete {}: {}", path.display(), error,));

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
         * Remember the current visible position rather than a path.
         *
         * The deleted path no longer exists, so selection should naturally land
         * on the next entry occupying that position, or the previous final entry
         * when the deleted item was last.
         */
        let previous_selected = self.selected;

        let previous_offset = self.list_offset;

        let previous_view_mode = self.view_mode;

        let entries = match self.source.read_directory(
            &self.current_directory,
            self.sort_mode,
            self.sort_descending,
        ) {
            Ok(entries) => entries,

            Err(error) => {
                self.show_error_message(format!(
                    "{} was deleted, but Scry could not refresh {}: {}",
                    path.display(),
                    self.current_directory.display(),
                    error,
                ));

                return;
            }
        };

        self.entries = entries;

        /*
         * Every cached representation may still contain the removed path or one
         * of its descendants.
         */
        self.invalidate_recursive_cache();

        self.directory_has_content_cache.clear();

        self.classification_inspection_cache
            .retain(|cached_path, _| cached_path != &path && !cached_path.starts_with(&path));

        self.tree_rows.clear();

        self.filtered_tree_indices.clear();

        self.tree_children.clear();

        self.search_tree_children.clear();

        self.expanded_directories
            .retain(|expanded_path| expanded_path != &path && !expanded_path.starts_with(&path));

        self.search_collapsed_directories
            .retain(|collapsed_path| collapsed_path != &path && !collapsed_path.starts_with(&path));

        self.recursive_expanded_directories
            .retain(|expanded_path| expanded_path != &path && !expanded_path.starts_with(&path));

        self.search_return_state = None;

        self.pending_selection_path = None;

        self.selected = previous_selected;

        self.list_offset = previous_offset;

        match previous_view_mode {
            ViewMode::List => {
                if self.recursive_search_active() {
                    self.ensure_recursive_scan();
                }

                self.refresh_filter();
            }

            ViewMode::Tree => {
                if self.recursive_search_active() {
                    self.ensure_recursive_scan();

                    /*
                     * The recursive tree is rebuilt when the new scan finishes.
                     *
                     * During that brief interval, keep the visible tree empty
                     * rather than displaying stale rows containing the deleted path.
                     */
                    if !self.scan_in_progress {
                        self.rebuild_recursive_search_tree(None);
                    }
                } else {
                    self.reset_tree();
                }
            }
        }

        self.selected = self
            .selected
            .min(self.current_visible_entry_count().saturating_sub(1));

        self.ensure_selection_visible(self.viewport_rows);

        self.show_info_message(format!("Deleted {}", path.display(),));
    }

    pub fn activate_selected(&mut self) {
        let Some(entry) = self.selected_entry() else {
            return;
        };

        let path = entry.path.clone();

        let entry_is_directory = entry.is_directory;

        /*
         * Ask the active filesystem source whether the path resolves to a
         * directory. LocalSource follows symlinks just as std::fs::metadata()
         * did previously.
         */
        let is_directory = self.path_is_directory(&path, entry_is_directory);

        /*
         * Remember the complete search before leaving its root.
         *
         * Directory results land in that directory. File results land in their
         * containing directory.
         */
        if self.recursive_search_active() && !self.query.is_empty() && self.query != "." {
            let landed_directory = if is_directory {
                path.clone()
            } else {
                match path.parent() {
                    Some(parent) => parent.to_path_buf(),

                    None => {
                        self.show_error_message(format!(
                            "Unable to determine the containing directory of {}",
                            path.display(),
                        ));

                        return;
                    }
                }
            };

            self.save_search_return_state(landed_directory);
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
         * First Enter on a recursive file result:
         *
         * Keep Scry open, move internally to the file's containing directory,
         * clear the recursive query, and select that exact file.
         *
         * A second Enter then opens the now-local file normally.
         */
        if self.recursive_search_active() && !self.query.is_empty() && self.query != "." {
            let Some(parent) = path.parent() else {
                self.show_error_message(format!(
                    "Unable to determine the containing directory of {}",
                    path.display(),
                ));

                return;
            };

            let parent = parent.to_path_buf();

            if !self.change_directory(parent, Some(path)) {
                return;
            }

            /*
             * change_directory() already:
             *
             * - clears the query;
             * - returns to List mode;
             * - loads the containing directory;
             * - selects the file through fallback_selection.
             */
            return;
        }

        /*
         * Enter on a normal file, including the second Enter after landing on a
         * recursive result:
         *
         * Open it, exit Scry, and remember the path for the post-exit summary.
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
            self.error_message = None;

            self.show_info_message(format!(
                "Remote index is already building — {} entries written",
                self.remote_index_entries_written,
            ));

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
        self.connection_dialog.focus = field;

        self.connection_dialog.error_message = None;
    }

    pub fn connection_focus_next(&mut self) {
        /*
         * At most eleven distinct controls exist. The bound prevents an
         * accidental infinite loop if every control were ever disabled.
         */
        for _ in 0..11 {
            self.connection_dialog.focus_next();

            if self.connection_focus_is_enabled() {
                break;
            }
        }
    }

    pub fn connection_focus_previous(&mut self) {
        for _ in 0..11 {
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
            });
        }

        self.invalidate_recursive_cache();

        self.source = source;

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

        self.refresh_filter();
    }

    pub fn disconnect_remote(&mut self) {
        self.search_return_state = None;

        if !self.source.is_remote() || self.transfer_visible() || self.connection_in_progress {
            return;
        }

        self.marked_files.clear();

        let saved_session = self.saved_local_session.take();

        let fallback_directory = match std::env::current_dir() {
            Ok(directory) => directory,

            Err(error) => {
                self.connection_dialog.error_message = Some(format!(
                    "Unable to determine the local working directory: {}",
                    error,
                ));

                return;
            }
        };

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

        self.active_ssh_target = None;

        self.invalidate_recursive_cache();

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

        self.recursive_entries.clear();

        self.recursive_path_indices.clear();

        self.recursive_child_indices.clear();

        Arc::make_mut(&mut self.search_index).clear();

        let receiver = match self.source.start_recursive_scan(
            self.current_directory.clone(),
            self.show_hidden,
            self.scan_generation,
            RecursiveScanMode::Fast,
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

        self.recursive_cache_complete = false;

        self.recursive_scan_partial = false;

        self.recursive_entries.clear();

        self.recursive_path_indices.clear();

        self.recursive_child_indices.clear();

        Arc::make_mut(&mut self.search_index).clear();

        self.search_tree_children.clear();

        self.recursive_expanded_directories.clear();
    }

    fn rebuild_recursive_path_indices(&mut self) {
        self.recursive_path_indices.clear();

        self.recursive_child_indices.clear();

        /*
         * Reserve the path lookup at its final approximate size.
         *
         * The child map cannot know its directory count in advance, but every
         * recursive entry receives exactly one numeric child index.
         */
        self.recursive_path_indices
            .reserve(self.recursive_entries.len());

        for (index, entry) in self.recursive_entries.iter().enumerate() {
            self.recursive_path_indices
                .insert(entry.path.clone(), index);

            let Some(parent) = entry.path.parent() else {
                continue;
            };

            self.recursive_child_indices
                .entry(parent.to_path_buf())
                .or_default()
                .push(index);
        }
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

        self.tree_children.clear();

        self.directory_has_content_cache.clear();

        self.expanded_directories.clear();

        self.view_mode = ViewMode::List;

        self.current_directory = target.clone();

        self.entries = entries;

        self.query.clear();

        self.query_cursor = 0;

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

            self.pending_selection_path = None;

            if let Some(saved_offset) = self.pending_session_list_offset.take() {
                self.list_offset = saved_offset;
            }

            /*
             * A changed terminal height may make the exact old offset invalid.
             *
             * Keep it whenever possible, but always leave the restored selection
             * visible inside the current viewport.
             */
            self.ensure_selection_visible(self.viewport_rows);
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
             * Directive-only and incomplete queries are equivalent to an empty
             * search. Immediately discard stale recursive results and restore
             * ordinary browsing.
             */
            self.refresh_filter();

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

                    if self.source.is_remote() && !entry.path.starts_with(&self.current_directory) {
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

        let scope_prefix = if self.source.is_remote() {
            Some(
                self.current_directory
                    .strip_prefix("/")
                    .unwrap_or(&self.current_directory)
                    .to_string_lossy()
                    .to_lowercase(),
            )
        } else {
            None
        };

        let result_limit = match self.view_mode {
            ViewMode::List => None,

            ViewMode::Tree => Some(EXACT_TREE_MATCH_LIMIT),
        };

        let cancel_signal = Arc::new(AtomicBool::new(false));

        self.fuzzy_examined = 0;

        self.fuzzy_total = index.len();

        self.fuzzy_receiver = Some(start_exact_worker(
            index,
            parsed_query,
            generation,
            self.show_hidden,
            scope_prefix,
            worker_entry_filter,
            result_limit,
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
                    if !self.show_hidden && entry_is_hidden_below(entry, &self.current_directory) {
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

            recursive_index_identity: self
                .recursive_search_active()
                .then(|| Arc::as_ptr(&self.search_index) as usize),
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

        let scope_prefix = if self.source.is_remote() && self.recursive_search_active() {
            Some(
                self.current_directory
                    .strip_prefix("/")
                    .unwrap_or(&self.current_directory)
                    .to_string_lossy()
                    .to_lowercase(),
            )
        } else {
            None
        };

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
            scope_prefix,
            worker_entry_filter,
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

            let selected_path = self.selected_entry().map(|entry| entry.path.clone());

            self.fuzzy_examined = message.examined;

            self.fuzzy_total = message.total;

            match self.view_mode {
                ViewMode::List => {
                    self.filtered_indices = message.indices;

                    if let Some(path) = selected_path {
                        self.select_visible_path(&path);
                    } else {
                        self.normalize_filtered_selection();
                    }

                    self.restore_pending_selection_if_available();
                }

                ViewMode::Tree => {
                    /*
                     * Tree hierarchy construction is substantially heavier than replacing a
                     * flat List result.
                     *
                     * Progressive worker snapshots are useful in List mode, but rebuilding
                     * parent-child maps, ancestors, sorted siblings, and TreeRows for every
                     * snapshot can monopolize the event thread. Tree therefore waits for the
                     * final stable result.
                     */
                    if message.finished {
                        /*
                         * Only Exact Recursive Tree uses the configurable 5,000-match safety cap.
                         */
                        self.exact_tree_limit_reached =
                            self.search_mode == SearchMode::Exact && message.limit_reached;

                        self.rebuild_fuzzy_search_tree_from_indices(
                            &message.indices,
                            selected_path,
                        );

                        self.restore_pending_selection_if_available();
                    }
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
                 * The final ranked result is now stable. Release any selection path that
                 * was pinned across the progressive fuzzy snapshots.
                 */
                self.pending_selection_path = None;

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

        self.filtered_indices = self
            .active_entries()
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                if !show_hidden && entry_is_hidden_below(entry, &self.current_directory) {
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

    fn rebuild_fuzzy_search_tree_from_indices(
        &mut self,
        matched_indices: &[usize],
        preferred_selection: Option<PathBuf>,
    ) {
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

                if let Some(&ancestor_index) = self.recursive_path_indices.get(path) {
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

    fn rebuild_recursive_search_tree(&mut self, preferred_selection: Option<PathBuf>) {
        if !self.recursive_search_active() {
            return;
        }

        let query = self.query.to_lowercase();

        self.search_tree_children.clear();

        /*
         * Fast path for plain recursive Tree mode.
         *
         * The hierarchy contains every recursive entry, but visible TreeRows remain
         * lazy and are created only for the root and explicitly expanded branches.
         */
        if query.is_empty() {
            for entry in &self.recursive_entries {
                let Some(parent) = entry.path.parent() else {
                    continue;
                };

                if self.source.is_remote() && !entry.path.starts_with(&self.current_directory) {
                    continue;
                }

                self.search_tree_children
                    .entry(parent.to_path_buf())
                    .or_default()
                    .push(entry.clone());
            }
        } else {
            /*
             * Fallback synchronous reconstruction.
             *
             * Normal live Exact and Fuzzy searches now arrive through their
             * background workers and rebuild_fuzzy_search_tree_from_indices().
             */
            let mut included_indices: HashSet<usize> = HashSet::new();

            for (entry_index, entry) in self.recursive_entries.iter().enumerate() {
                if !entry.searchable_path.contains(&query) {
                    continue;
                }

                if self.source.is_remote() && !entry.path.starts_with(&self.current_directory) {
                    continue;
                }

                included_indices.insert(entry_index);

                let mut ancestor = entry.path.parent();

                while let Some(path) = ancestor {
                    if path == self.current_directory {
                        break;
                    }

                    if let Some(&ancestor_index) = self.recursive_path_indices.get(path) {
                        included_indices.insert(ancestor_index);
                    }

                    ancestor = path.parent();
                }
            }

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
        }

        for children in self.search_tree_children.values_mut() {
            sort_entries(children, self.sort_mode, self.sort_descending);
        }

        self.rebuild_recursive_search_rows(preferred_selection);
    }

    fn rebuild_recursive_search_rows(&mut self, preferred_selection: Option<PathBuf>) {
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
         * Reconstruct the ordinary browsing Tree from the current directory.
         *
         * rebuild_tree_rows() alone is insufficient when the recursive search
         * hierarchy has replaced or cleared the ordinary tree_children map.
         */
        self.reset_tree();

        if let Some(saved_selection) = saved_selection {
            if let Some(position) = self
                .tree_rows
                .iter()
                .position(|row| row.entry.path == saved_selection)
            {
                self.selected = position;
            }
        }

        self.list_offset = saved_offset.min(self.tree_rows.len().saturating_sub(1));

        self.refresh_tree_filter();
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
        self.filtered_tree_indices = self
            .tree_rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| self.entry_filter.matches(&row.entry).then_some(index))
            .collect();

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

    fn refresh_recursive_tree_indices(&mut self) {
        self.filtered_tree_indices = (0..self.tree_rows.len()).collect();

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

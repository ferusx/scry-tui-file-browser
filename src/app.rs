// SPDX-License-Identifier: BSD-3-Clause

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
use crate::fuzzy::{FuzzyWorkerResult, start_fuzzy_worker};

use crate::scan::{
    FileEntry, RecursiveScanMode, ScanMessage, SortMode, sort_entries, start_recursive_scan,
};
use crate::search_index::SearchIndex;
use crate::source::{FileSource, LocalSource, TransferControl, TransferProgress};
use crate::ssh::{SftpSource, SshTarget};
use crate::themes::Theme;

#[derive(Debug, Clone)]
struct LocalSessionState {
    directory: PathBuf,

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
pub enum Overlay {
    None,

    Help,

    About,

    Connection,
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

    directory: PathBuf,

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
enum TransferWorkerMessage {
    Progress(TransferProgress),

    Finished(TransferWorkerResult),
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

    receiver: Receiver<TransferWorkerMessage>,

    cancel_signal: Arc<AtomicBool>,
}

/*
 * The real source temporarily lives inside the transfer worker.
 *
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

    fn is_remote(&self) -> bool {
        true
    }
}

#[derive(Debug)]
pub struct App {
    source: Box<dyn FileSource>,

    pub current_directory: PathBuf,

    pub entries: Vec<FileEntry>,

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

    search_index: Arc<SearchIndex>,

    pub filtered_indices: Vec<usize>,

    pub query: String,

    pub search_mode: SearchMode,

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

    pub opened_file_path: Option<PathBuf>,

    pub transfer: Option<TransferState>,

    pub enable_deletion: bool,

    pub deletion: Option<DeletionState>,

    pub list_offset: usize,

    pub viewport_rows: usize,

    pending_selection_path: Option<PathBuf>,

    pub error_message: Option<String>,

    pub should_quit: bool,

    pub scan_in_progress: bool,

    pub recursive_scan_partial: bool,

    pub recursive_mode: bool,

    pub view_mode: ViewMode,

    pub overlay: Overlay,

    pub connection_store: ConnectionStore,

    pub connection_dialog: ConnectionDialogState,

    pub connection_in_progress: bool,

    pub ssh_config: SshConfig,

    connection_receiver: Option<Receiver<ConnectionWorkerResult>>,

    saved_local_session: Option<LocalSessionState>,

    pub help_scroll: u16,

    pub help_max_scroll: u16,

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

    pub fuzzy_filter_in_progress: bool,

    pub fuzzy_examined: usize,

    pub fuzzy_total: usize,

    navigation_states: HashMap<PathBuf, NavigationState>,

    search_return_state: Option<SearchReturnState>,

    pub search_navigation_active: bool,
}

impl App {
    pub fn new(start_path: PathBuf) -> io::Result<Self> {
        let current_directory = normalize_start_path(start_path)?;

        Self::with_source(current_directory, Box::new(LocalSource::new()))
    }
    pub fn with_source(
        current_directory: PathBuf,
        mut source: Box<dyn FileSource>,
    ) -> io::Result<Self> {
        let sort_mode = SortMode::Name;

        let sort_descending = false;

        let connection_store = ConnectionStore::load()?;

        let connection_dialog = ConnectionDialogState::new(&connection_store);

        let entries = source.read_directory(&current_directory, sort_mode, sort_descending)?;

        let mut app = Self {
            source,

            current_directory,

            entries,

            recursive_entries: Vec::new(),

            recursive_path_indices: HashMap::new(),

            search_index: Arc::new(SearchIndex::new()),

            filtered_indices: Vec::new(),

            query: String::new(),

            search_mode: SearchMode::Exact,

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

            opened_file_path: None,

            transfer: None,

            enable_deletion: false,

            deletion: None,

            selected: 0,

            list_offset: 0,

            viewport_rows: 1,

            pending_selection_path: None,

            error_message: None,

            should_quit: false,

            scan_in_progress: false,

            recursive_scan_partial: false,

            recursive_mode: false,

            view_mode: ViewMode::List,

            overlay: Overlay::None,

            connection_store,

            connection_dialog,

            ssh_config: SshConfig::default(),

            connection_in_progress: false,

            connection_receiver: None,

            saved_local_session: None,

            help_scroll: 0,

            help_max_scroll: 0,

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

            fuzzy_filter_in_progress: false,

            fuzzy_examined: 0,

            fuzzy_total: 0,

            navigation_states: HashMap::new(),

            search_return_state: None,

            search_navigation_active: false,
        };

        app.refresh_filter();

        Ok(app)
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
         * Recursive mode must be established before Tree mode. That allows
         * toggle_tree_mode() to choose the recursive-tree startup route when both
         * settings are enabled.
         */
        if config.browser.recursive && !self.recursive_mode {
            self.enable_recursive_mode();
        }

        if config.browser.view == "tree" && self.view_mode != ViewMode::Tree {
            self.toggle_tree_mode();
        }
    }

    pub fn recursive_search_active(&self) -> bool {
        self.recursive_mode
            || (self.source.supports_recursive_scan()
                && !self.query.is_empty()
                && self.query != ".")
    }

    pub fn active_entry_count(&self) -> usize {
        self.active_entries().len()
    }

    pub fn source_label(&self) -> String {
        self.source.source_label()
    }

    pub fn source_is_remote(&self) -> bool {
        self.source.is_remote()
    }

    pub fn selected_entry(&self) -> Option<&FileEntry> {
        match self.view_mode {
            ViewMode::List => self.entry_at_filtered_position(self.selected),

            ViewMode::Tree => self
                .tree_row_at_filtered_position(self.selected)
                .map(|row| &row.entry),
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
                    self.recursive_path_indices.extend(
                        entries
                            .iter()
                            .enumerate()
                            .map(|(offset, entry)| (entry.path.clone(), base_entry_index + offset)),
                    );

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

                    self.error_message = Some(message);

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
            if self.view_mode == ViewMode::Tree && self.recursive_search_active() {
                /*
                push_query_character                * every incoming scan batch.
                                *
                                * Accumulate entries while scanning and construct the tree once when
                                * the scan has finished.
                                */
                if scan_finished {
                    let selected_path = self
                        .pending_selection_path
                        .clone()
                        .or_else(|| self.selected_entry().map(|entry| entry.path.clone()));

                    let text_filter_active = !self.query.is_empty() && self.query != ".";

                    if self.search_mode == SearchMode::Fuzzy && text_filter_active {
                        /*
                         * A genuine fuzzy text query consumes the completed compact index.
                         */
                        self.start_current_fuzzy_filter();
                    } else {
                        /*
                         * An empty query is ordinary recursive Tree browsing.
                         *
                         * SearchMode::Fuzzy changes how typed text is interpreted; it must not
                         * erase the unfiltered recursive tree when no text search exists.
                         */
                        self.rebuild_recursive_search_tree(selected_path);

                        self.restore_pending_selection_if_available();
                    }
                }
            } else if self.search_mode == SearchMode::Fuzzy && self.recursive_search_active() {
                /*
                 * Recursive Fuzzy mode waits for the complete scanner result.
                 *
                 * Running and sorting fuzzy matches after every 256-entry batch caused the
                 * severe UI stalls observed with large directory trees.
                 */
                if scan_finished {
                    self.refresh_filter();
                }
            } else {
                /*
                 * Exact List mode remains incremental.
                 */
                self.refresh_filter();

                self.restore_pending_selection_if_available();
            }
        }

        changed
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
            let search_was_active = self.recursive_search_active();

            let selected_path = self.selected_entry().map(|entry| entry.path.clone());

            if !search_was_active {
                self.tree_search_saved_selection = selected_path.clone();

                self.tree_search_saved_offset = self.list_offset;

                self.search_collapsed_directories.clear();
            }

            self.query.push(character);

            if !search_was_active && self.recursive_search_active() {
                self.ensure_recursive_scan();
            }

            if self.recursive_search_active() {
                /*
                 * Fuzzy Tree mode shares the same bounded background worker as List.
                 *
                 * Exact Tree mode retains its existing synchronous hierarchy builder for
                 * now. While the recursive scanner is running, both routes wait until its
                 * current index is ready.
                 */
                if !self.scan_in_progress {
                    match self.search_mode {
                        SearchMode::Fuzzy => {
                            self.start_current_fuzzy_filter();
                        }

                        SearchMode::Exact => {
                            self.rebuild_recursive_search_tree(selected_path);
                        }
                    }
                }
            } else {
                self.refresh_tree_filter();
            }

            return;
        }

        let search_was_active = self.recursive_search_active();

        self.query.push(character);

        if !search_was_active && self.recursive_search_active() {
            self.ensure_recursive_scan();
        }

        self.selected = 0;

        self.list_offset = 0;

        self.refresh_filter();
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
            let search_was_active = self.recursive_search_active();

            let selected_path = self.selected_entry().map(|entry| entry.path.clone());

            self.query.pop();

            if search_was_active && !self.recursive_search_active() {
                self.restore_manual_tree();

                return;
            }

            if self.recursive_search_active() {
                self.ensure_recursive_scan();

                if !self.scan_in_progress {
                    match self.search_mode {
                        SearchMode::Fuzzy => {
                            self.start_current_fuzzy_filter();
                        }

                        SearchMode::Exact => {
                            self.rebuild_recursive_search_tree(selected_path);
                        }
                    }
                }
            } else {
                self.refresh_tree_filter();
            }

            return;
        }

        self.query.pop();

        if self.recursive_search_active() {
            self.ensure_recursive_scan();
        }

        self.selected = 0;

        self.list_offset = 0;

        self.refresh_filter();
    }

    pub fn clear_query(&mut self) {
        self.search_navigation_active = false;

        self.search_return_state = None;

        if self.view_mode == ViewMode::Tree {
            let search_was_active = self.recursive_search_active();

            self.query.clear();

            if search_was_active {
                self.restore_manual_tree();
            } else {
                self.refresh_tree_filter();
            }

            return;
        }

        self.query.clear();

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

    pub fn toggle_hidden(&mut self) {
        let selected_path = self.selected_entry().map(|entry| entry.path.clone());

        self.show_hidden = !self.show_hidden;

        self.directory_has_content_cache.clear();

        self.invalidate_recursive_cache();

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
                if self.search_mode == SearchMode::Fuzzy
                    && !self.query.is_empty()
                    && self.query != "."
                {
                    if !self.scan_in_progress {
                        self.start_current_fuzzy_filter();
                    }
                } else {
                    self.rebuild_recursive_search_tree(selected_path.clone());

                    self.restore_pending_selection_if_available();
                }
            }
        }
    }

    pub fn toggle_recursive_mode(&mut self) {
        /*
         * Remote SFTP currently supports directory-by-directory browsing but not
         * Scry's background recursive scanner.
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
            self.enable_recursive_mode();

            return;
        }

        /*
         * Disable the persistent recursive listing.
         *
         * A non-empty query may still keep recursive search active. That is
         * intentional: Alt+R controls recursive browsing, while typing performs
         * an on-demand recursive search.
         *
         * Preserve the selected path while the visible result source is rebuilt.
         */
        let selected_path = self.selected_entry().map(|entry| entry.path.clone());

        self.pending_selection_path = selected_path.clone();

        self.search_return_state = None;

        self.search_navigation_active = false;

        self.recursive_mode = false;

        self.error_message = None;

        if self.query.is_empty() || self.query == "." {
            /*
             * No text search remains active, so stop and discard the recursive
             * scan state and return to the immediate directory listing.
             */
            self.invalidate_recursive_cache();

            match self.view_mode {
                ViewMode::List => {
                    self.refresh_filter();
                }

                ViewMode::Tree => {
                    self.reset_tree();
                }
            }
        } else {
            /*
             * A text query still requests recursive search. Keep or restart the
             * recursive cache and rebuild the current search view accordingly.
             */
            self.ensure_recursive_scan();

            match self.view_mode {
                ViewMode::List => {
                    self.refresh_filter();
                }

                ViewMode::Tree => {
                    self.rebuild_recursive_search_tree(None);
                }
            }
        }

        self.restore_pending_selection_if_available();

        self.ensure_selection_visible(self.viewport_rows);
    }

    pub fn toggle_tree_mode(&mut self) {
        match self.view_mode {
            ViewMode::List => {
                self.view_mode = ViewMode::Tree;

                self.selected = 0;

                self.list_offset = 0;

                if self.recursive_mode {
                    /*
                     * Startup recursive mode already represents the complete descendant
                     * set, so Tree mode should display that same set hierarchically.
                     */
                    self.ensure_recursive_scan();

                    self.rebuild_recursive_search_tree(None);
                } else {
                    /*
                     * Ordinary Tree mode represents the current directory hierarchy
                     * rather than an active text search.
                     */
                    self.query.clear();

                    self.reset_tree();
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

        self.error_message = None;
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

        /*
         * Ask the active filesystem source whether the path resolves to a
         * directory. This preserves directory-symlink behavior for both local
         * and future remote sources.
         */
        let is_directory = self.path_is_directory(&path, entry_is_directory);

        if !is_directory {
            return;
        }

        if !self.change_directory(path, None) {
            return;
        }

        /*
         * change_directory() deliberately returns to List mode.
         *
         * Enter originated in Tree mode, so establish the selected directory as
         * a completely new Tree root. All former ancestors disappear.
         */
        self.view_mode = ViewMode::Tree;

        self.selected = 0;

        self.list_offset = 0;

        if self.recursive_mode {
            /*
             * change_directory() has already started the new recursive scan.
             *
             * Usually the tree remains in its scanning state until the worker
             * finishes. If a complete cache is already available, construct it
             * immediately.
             */
            self.ensure_recursive_scan();

            if !self.scan_in_progress {
                self.rebuild_recursive_search_tree(None);
            }
        } else {
            self.reset_tree();
        }
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

        self.invalidate_recursive_cache();

        self.current_directory = state.root_directory;

        self.entries = entries;

        self.query = state.query;

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
                 * The recursive tree is rebuilt once the restored scan completes.
                 * If a complete result is already available, this builds it now.
                 */
                if !self.scan_in_progress {
                    self.rebuild_recursive_search_tree(state.selected_path.clone());
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
            if self.recursive_search_active() {
                self.move_recursive_tree_root_to_parent();
            } else {
                self.move_tree_root_to_parent();
            }

            return;
        }

        if self.recursive_search_active() {
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

        if !row.entry.is_directory || row.entry.is_symlink || row.expanded {
            return;
        }

        let path = row.entry.path.clone();

        /*
         * A bounded fuzzy-result tree initially contains only:
         *
         * - the best matching entries;
         * - the ancestors required to connect them.
         *
         * A directory may therefore have real filesystem content without yet having
         * contextual children in search_tree_children. When the user explicitly
         * presses Right, lazily load that directory's immediate children instead of
         * pretending that the branch cannot expand.
         */
        if !self
            .search_tree_children
            .get(&path)
            .is_some_and(|children| !children.is_empty())
        {
            let children =
                match self
                    .source
                    .read_directory(&path, self.sort_mode, self.sort_descending)
                {
                    Ok(children) => children
                        .into_iter()
                        .filter(|entry| self.show_hidden || !entry.name.starts_with('.'))
                        .collect::<Vec<_>>(),

                    Err(error) => {
                        self.error_message =
                            Some(format!("Unable to expand {}: {}", path.display(), error,));

                        return;
                    }
                };

            if children.is_empty() {
                return;
            }

            self.search_tree_children.insert(path.clone(), children);

            self.error_message = None;
        }

        self.recursive_expanded_directories.insert(path.clone());

        let mut child_ancestor_has_more = row.ancestor_has_more.clone();

        child_ancestor_has_more.push(!row.is_last);

        let mut inserted_rows = Vec::new();

        /*
         * Insert only this directory's immediate children.
         *
         * Their descendants remain indexed but are not turned into TreeRows
         * until those child directories are explicitly expanded.
         */
        Self::append_recursive_direct_children(
            path,
            child_ancestor_has_more,
            &self.search_tree_children,
            &self.recursive_expanded_directories,
            &mut inserted_rows,
        );

        if let Some(row) = self.tree_rows.get_mut(tree_index) {
            row.expanded = true;
        }

        self.tree_rows.splice(
            tree_index.saturating_add(1)..tree_index.saturating_add(1),
            inserted_rows,
        );

        self.refresh_recursive_tree_indices();

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
         * At the filesystem root, parent and current path are the same.
         */
        if parent == previous_root {
            return;
        }

        if !self.change_directory(parent, Some(previous_root.clone())) {
            return;
        }

        /*
         * change_directory() normally returns Wraith to List mode.
         * This action originated in Tree mode, so construct a new tree rooted
         * one directory higher instead.
         */
        self.view_mode = ViewMode::Tree;

        self.query.clear();

        self.selected = 0;

        self.list_offset = 0;

        self.reset_tree();

        /*
         * Select the former root in the newly created parent tree.
         */
        if let Some(position) = self.filtered_tree_indices.iter().position(|tree_index| {
            self.tree_rows
                .get(*tree_index)
                .is_some_and(|row| row.entry.path == previous_root)
        }) {
            self.selected = position;
        }
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

                        self.error_message = None;

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
        }
    }
    pub fn acknowledge_transfer(&mut self) {
        if !self.transfer_finished() {
            return;
        }

        let Some(transfer) = self.transfer.take() else {
            return;
        };

        if let Some(error) = transfer.error {
            self.error_message = Some(format!(
                "Unable to prepare {} for opening: {}",
                transfer.remote_path.display(),
                error,
            ));

            return;
        }

        let Some(local_path) = transfer.local_path else {
            self.error_message =
                Some("Remote transfer completed without producing a local file".to_string());

            return;
        };

        match crate::open::open_file(&local_path) {
            Ok(()) => {
                self.opened_file_path = Some(transfer.remote_path);

                self.should_quit = true;
            }

            Err(error) => {
                self.error_message = Some(error);
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
            self.error_message =
                Some("Deletion is not available while browsing through SSH".to_string());

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
         * symlink_metadata() inspects the selected link itself rather than
         * following it to some other filesystem object.
         */
        if let Err(error) = std::fs::symlink_metadata(&path) {
            self.error_message = Some(format!(
                "Unable to validate {} for deletion: {}",
                path.display(),
                error,
            ));

            return;
        }

        let directory_has_content =
            entry.is_directory && !entry.is_symlink && self.directory_has_content(&path);

        self.error_message = None;

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
                self.error_message = Some(format!(
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
            self.error_message = Some(format!("Unable to delete {}: {}", path.display(), error,));

            return;
        }

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
                self.error_message = Some(format!(
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

        self.error_message = None;

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
                        self.error_message = Some(format!(
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
         * Works as → (right) and will enter the directory inside Wraith.
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
         * First Enter on a recursive file result:
         *
         * Keep Wraith open, move internally to the file's containing directory,
         * clear the recursive query, and select that exact file.
         *
         * A second Enter then opens the now-local file normally.
         */
        if self.recursive_search_active() && !self.query.is_empty() && self.query != "." {
            let Some(parent) = path.parent() else {
                self.error_message = Some(format!(
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
         * Open it, exit Wraith, and remember the path for the post-exit summary.
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
                self.error_message = Some(format!(
                    "Unable to prepare {} for opening: {}",
                    path.display(),
                    error,
                ));

                return;
            }
        };

        match crate::open::open_file(&local_open_path) {
            Ok(()) => {
                self.opened_file_path = Some(path);

                self.should_quit = true;
            }

            Err(error) => {
                self.error_message = Some(error);
            }
        }
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    pub fn connection_visible(&self) -> bool {
        self.overlay == Overlay::Connection
    }

    pub fn toggle_connection_dialog(&mut self) {
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
            }

            Err(message) => {
                self.connection_dialog.error_message = Some(message);
            }
        }

        true
    }

    fn install_connected_source(&mut self, success: ConnectionWorkerSuccess) {
        self.search_return_state = None;

        /*
         * Preserve the local browser position only when leaving a local source.
         *
         * Connecting from one SSH host to another must not overwrite the original
         * local session to which Disconnect should eventually return.
         */
        if !self.source.is_remote() && self.saved_local_session.is_none() {
            self.saved_local_session = Some(LocalSessionState {
                directory: self.current_directory.clone(),

                selected_path: self.selected_entry().map(|entry| entry.path.clone()),

                list_offset: self.list_offset,

                query: self.query.clone(),

                view_mode: self.view_mode,

                recursive_mode: self.recursive_mode,

                search_mode: self.search_mode,
            });
        }

        self.invalidate_recursive_cache();

        self.source = success.source;

        /*
         * Remote SFTP currently supports directory-by-directory browsing but not
         * recursive scanning.
         *
         * Never carry a local recursive state into the connected source. Doing so
         * makes active_entries() select the empty recursive corpus and leaves the
         * user unable to disable the mode because the source rejects recursive
         * operations.
         *
         * The original local recursive preference is already preserved inside
         * saved_local_session and will return on Disconnect.
         */
        if !self.source.supports_recursive_scan() {
            self.recursive_mode = false;
        }

        self.current_directory = success.directory;

        self.entries = success.entries;

        self.query.clear();

        /*
         * Remote browsing currently supports ordinary current-directory filtering,
         * but not the complete recursive search engine.
         *
         * Begin in Exact mode for a predictable, fully supported remote experience.
         * Once every search mode is implemented remotely, this forced fallback can be
         * removed so the user's active mode carries across unchanged.
         */
        self.search_mode = SearchMode::Exact;

        self.error_message = None;

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

        let session = saved_session.unwrap_or(LocalSessionState {
            directory: fallback_directory,

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

        self.invalidate_recursive_cache();

        self.current_directory = session.directory;

        self.entries = entries;

        self.query = session.query;

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
    }

    pub fn close_connection_dialog(&mut self) {
        self.overlay = Overlay::None;

        self.connection_dialog.error_message = None;
    }

    pub fn about_visible(&self) -> bool {
        self.overlay == Overlay::About
    }

    pub fn toggle_about(&mut self) {
        self.overlay = match self.overlay {
            Overlay::About => Overlay::None,

            Overlay::None | Overlay::Help | Overlay::Connection => Overlay::About,
        };
    }

    pub fn close_about(&mut self) {
        self.overlay = Overlay::None;
    }

    pub fn help_visible(&self) -> bool {
        self.overlay == Overlay::Help
    }

    pub fn toggle_help(&mut self) {
        self.overlay = match self.overlay {
            Overlay::Help => Overlay::None,

            Overlay::None | Overlay::About | Overlay::Connection => {
                self.help_scroll = 0;

                self.help_max_scroll = 0;

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

    fn active_entries(&self) -> &[FileEntry] {
        /*
         * An empty query is ordinary filesystem browsing.
         *
         * Recursive mode may scan and cache descendants in the background, but the
         * flat List must continue to display only the current directory until the
         * user enters actual search text.
         */
        let text_filter_active = !self.query.is_empty() && self.query != ".";

        if text_filter_active && self.recursive_search_active() {
            &self.recursive_entries
        } else {
            &self.entries
        }
    }

    fn ensure_recursive_scan(&mut self) {
        if !self.source.supports_recursive_scan() {
            self.error_message = Some("Recursive remote scanning is not active yet".to_string());

            self.scan_in_progress = false;

            self.recursive_scan_partial = false;

            return;
        }

        if self.recursive_cache_complete || self.scan_receiver.is_some() {
            return;
        }

        self.scan_generation = self.scan_generation.wrapping_add(1);

        self.recursive_entries.clear();

        Arc::make_mut(&mut self.search_index).clear();

        self.scan_in_progress = true;

        self.error_message = None;

        self.scan_receiver = Some(start_recursive_scan(
            self.current_directory.clone(),
            self.show_hidden,
            self.scan_generation,
            RecursiveScanMode::Fast,
        ));
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

        Arc::make_mut(&mut self.search_index).clear();

        self.search_tree_children.clear();

        self.recursive_expanded_directories.clear();
    }

    fn rebuild_recursive_path_indices(&mut self) {
        self.recursive_path_indices.clear();

        self.recursive_path_indices.extend(
            self.recursive_entries
                .iter()
                .enumerate()
                .map(|(index, entry)| (entry.path.clone(), index)),
        );
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
                    self.error_message =
                        Some(format!("Unable to open {}: {}", target.display(), error));

                    return false;
                }
            };

        /*
         * Preserve the active search while changing only its filesystem root.
         */
        let query = self.query.clone();

        let view_mode = self.view_mode;

        self.save_current_navigation_state();

        self.invalidate_recursive_cache();

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

        self.search_navigation_active = true;

        self.error_message = None;

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
                    self.error_message =
                        Some(format!("Unable to open {}: {}", target.display(), error,));

                    return false;
                }
            };

        self.save_current_navigation_state();

        self.invalidate_recursive_cache();

        self.tree_rows.clear();

        self.tree_children.clear();

        self.directory_has_content_cache.clear();

        self.expanded_directories.clear();

        self.view_mode = ViewMode::List;

        self.current_directory = target.clone();

        self.entries = entries;

        self.query.clear();

        self.error_message = None;

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

            /*
             * Exact recursive scans and fuzzy workers both rebuild their result sets
             * asynchronously.
             *
             * Keep the intended path pinned until neither operation is still capable of
             * moving entries around beneath the numeric selection row.
             */
            if !self.scan_in_progress && !self.fuzzy_filter_in_progress {
                self.pending_selection_path = None;
            }

            self.ensure_selection_visible(self.viewport_rows);
        }
    }

    fn cancel_fuzzy_filter(&mut self) {
        if let Some(signal) = self.fuzzy_cancel_signal.take() {
            signal.store(true, Ordering::Relaxed);
        }

        self.fuzzy_receiver = None;

        self.fuzzy_filter_in_progress = false;

        self.fuzzy_examined = 0;

        self.fuzzy_total = 0;

        self.fuzzy_generation = self.fuzzy_generation.wrapping_add(1);
    }

    fn start_current_fuzzy_filter(&mut self) {
        let text_filter_active = !self.query.is_empty() && self.query != ".";

        if !text_filter_active {
            self.cancel_fuzzy_filter();

            self.fuzzy_examined = 0;

            self.fuzzy_total = 0;

            self.filtered_indices = self
                .active_entries()
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| {
                    if !self.show_hidden && entry.name.starts_with('.') {
                        None
                    } else {
                        Some(index)
                    }
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

        self.fuzzy_receiver = Some(start_fuzzy_worker(
            index,
            self.query.clone(),
            generation,
            self.show_hidden,
            Arc::clone(&cancel_signal),
        ));

        self.fuzzy_cancel_signal = Some(cancel_signal);

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
                     * Tree consumes the same bounded, ranked worker result as List mode.
                     *
                     * rebuild_fuzzy_search_tree_from_indices() adds only the best matches
                     * and the ancestors required to connect them to the current root.
                     */
                    self.rebuild_fuzzy_search_tree_from_indices(&message.indices, selected_path);

                    self.restore_pending_selection_if_available();
                }
            }

            changed = true;

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
        if self.search_mode == SearchMode::Fuzzy {
            self.start_current_fuzzy_filter();

            return;
        }

        /*
         * Exact search remains synchronous and otherwise unchanged.
         */
        self.cancel_fuzzy_filter();

        let text_filter_active = !self.query.is_empty() && self.query != ".";

        let show_hidden = self.show_hidden;

        let query = self.query.to_lowercase();

        self.filtered_indices = self
            .active_entries()
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                if !show_hidden && entry.name.starts_with('.') {
                    return None;
                }

                if !text_filter_active || entry.searchable_path.contains(&query) {
                    Some(index)
                } else {
                    None
                }
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
         * The fuzzy worker has already searched and ranked the complete compact
         * index. Its result contains at most the best 500 recursive-entry indices.
         *
         * Tree construction must therefore touch only:
         *
         * - those ranked matches;
         * - the ancestors needed to connect them to the current root.
         *
         * It must never search the complete recursive corpus again.
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
         * Convert the bounded path set into the same parent → children structure
         * already consumed by Scry's proven Tree-row renderer and navigation code.
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

        /*
         * Preserve Scry's established sibling ordering and directory-first rule.
         *
         * Only small sibling groups belonging to the bounded contextual tree are
         * sorted here—not the complete recursive population.
         */
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
         * Fast path for startup recursive mode.
         *
         * With an empty query, every scanned entry belongs in the tree.
         * Each FileEntry already carries its complete path, so it can be
         * inserted directly beneath its parent without:
         *
         * - constructing a second path -> entry lookup;
         * - collecting every included path;
         * - walking every entry's ancestors;
         * - looking every path up again afterward.
         */
        if query.is_empty() {
            for entry in &self.recursive_entries {
                let Some(parent) = entry.path.parent() else {
                    continue;
                };

                self.search_tree_children
                    .entry(parent.to_path_buf())
                    .or_default()
                    .push(entry.clone());
            }
        } else {
            /*
             * A text-filtered recursive tree must also include the ancestors of
             * every matching entry so that each result remains connected to the
             * current root.
             */
            let mut entries_by_path: HashMap<PathBuf, FileEntry> = HashMap::new();

            for entry in self.entries.iter().chain(self.recursive_entries.iter()) {
                entries_by_path.insert(entry.path.clone(), entry.clone());
            }

            let mut included_paths: HashSet<PathBuf> = HashSet::new();

            for entry in &self.recursive_entries {
                if !entry.searchable_path.contains(&query) {
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

            for path in included_paths {
                let Some(entry) = entries_by_path.get(&path).cloned() else {
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
            /*
             * Plain recursive Tree mode is lazy.
             *
             * The complete hierarchy is already indexed in
             * search_tree_children, but initially only the root's immediate
             * children become visible TreeRows.
             */
            Self::append_recursive_direct_children(
                self.current_directory.clone(),
                Vec::new(),
                &self.search_tree_children,
                &self.recursive_expanded_directories,
                &mut rows,
            );
        } else {
            /*
             * A text-search tree remains fully connected so every match and its
             * ancestors are visible immediately.
             */
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

            /*
             * Recreate only branches that the user had already expanded.
             *
             * This preserves the visible tree and selected path when sorting,
             * without materializing every descendant in the recursive index.
             */
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
            let is_last = index + 1 == child_count;

            let is_directory = entry.is_directory;

            let child_path = entry.path.clone();

            let has_visible_children = search_children
                .get(&child_path)
                .is_some_and(|children| !children.is_empty());

            let expanded = is_directory
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
        self.search_collapsed_directories.clear();

        self.search_tree_children.clear();

        let saved_selection = self.tree_search_saved_selection.take();

        let saved_offset = self.tree_search_saved_offset;

        self.rebuild_tree_rows(None);

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

        directory,

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

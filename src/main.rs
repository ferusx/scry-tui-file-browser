// SPDX-License-Identifier: BSD-3-Clause

/*
 * FreeBSD's jemalloc normally retains large unused virtual-memory extents for
 * reuse. Scry can temporarily allocate multi-gigabyte recursive SSH indexes,
 * so retaining those extents leaves an unnecessarily large resident footprint
 * after the remote corpus has been discarded.
 *
 * Disable jemalloc extent retention for Scry on FreeBSD so released index
 * memory is returned promptly to the operating system.
 */
#[cfg(target_os = "freebsd")]
#[unsafe(no_mangle)]
pub static mut malloc_conf: *const std::os::raw::c_char = b"retain:false\0".as_ptr().cast();

mod app;
mod args;
mod classify;
mod clipboard;
mod config;
mod connection;
mod deletion_journal;
mod entry;
mod external_help;
mod file_info;
mod fuzzy;
mod help;
mod open;
mod query;
mod remote_index;
mod scan;
mod search_index;
mod session;
mod source;
mod ssh;
mod themes;
mod ui;
mod ui_state;

use app::{
    App, DeletionChoice, EntryFilter, RemoteIndexDialogFocus, TreeExpandAllDialogFocus,
    TreeExpandAllDialogKind, ViewMode,
};
use args::Cli;
use clap::Parser;
use connection::ConnectionField;
use ratatui::layout::Rect;
use session::{SessionSource, SessionState};
use ssh::{SftpSource, SshTarget};
use std::io::{self, IsTerminal, stdout};
use std::time::{Duration, Instant};

use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
        MouseButton, MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute, terminal,
};

/*
 * Copy persisted display and browser choices into the ordinary startup
 * configuration.
 *
 * App::apply_startup_config() remains the single place that establishes modes
 * in the required order: sorting, search style, recursive scope, then Tree mode.
 */
fn apply_session_to_startup_config(config: &mut config::ScryConfig, state: &SessionState) {
    config.display.show_hidden = state.show_hidden;

    config.display.show_icons = state.show_icons;

    config.display.show_file_colors = state.show_file_colors;

    config.display.show_details = state.show_details;

    config.display.show_selection = state.show_selection;

    config.display.show_columns = state.show_columns;

    config.display.show_permissions = state.show_permissions;

    config.display.show_size = state.show_size;

    config.display.show_date = state.show_date;

    config.display.show_user = state.show_user;

    config.browser.view = match state.view_mode.as_str() {
        "tree" => "tree",

        _ => "list",
    }
    .to_string();

    config.browser.fuzzy = state.search_mode == "fuzzy";

    config.browser.recursive = state.recursive;

    config.browser.hidden_only = state.hidden_only;

    config.browser.entry_filter = match state.entry_filter.as_str() {
        "files" => "files",

        "directories" => "directories",

        _ => "all",
    }
    .to_string();

    config.browser.sort = match state.sort_mode.as_str() {
        "size" => "size",

        "date" => "date",

        "type" => "type",

        _ => "name",
    }
    .to_string();

    config.browser.reverse = state.reverse;
}

/*
 * Construct the filesystem source recorded by a saved session.
 *
 * SSH connection errors are returned so main() can fall back to a normal local
 * listing without entering raw terminal mode.
 */
fn app_from_session(state: &SessionState, ssh_config: &config::SshConfig) -> Result<App, String> {
    match &state.source {
        SessionSource::Local { directory, .. } => App::new(directory.clone()).map_err(|error| {
            format!(
                "unable to restore local directory {}: {}",
                directory.display(),
                error,
            )
        }),

        SessionSource::Ssh {
            host,
            user,
            port,
            identity_file,
            directory,
            ..
        } => {
            let target = SshTarget {
                host: host.clone(),

                user: user.clone(),

                port: *port,

                identity_file: identity_file.clone(),
            };

            eprintln!(
                "scry: restoring SSH session through {}...",
                target.openssh_destination(),
            );

            let (remote_home, source) =
                SftpSource::connect(&target, ssh_config).map_err(|error| {
                    format!(
                        "unable to reconnect to {}: {}",
                        target.openssh_destination(),
                        error,
                    )
                })?;

            let mut app =
                App::with_source_and_home(directory.clone(), remote_home, Box::new(source))
                    .map_err(|error| {
                        format!(
                            "unable to restore remote directory {}: {}",
                            directory.display(),
                            error,
                        )
                    })?;

            app.set_active_ssh_target(target);

            Ok(app)
        }
    }
}

fn main() -> io::Result<()> {
    if let Some(result) = clipboard::run_owner_if_requested() {
        return result;
    }

    let cli = Cli::parse();

    /*
     * Configuration generation must happen before ScryConfig::load().
     *
     * Normal loading may create the live scry.toml when it is missing, whereas
     * --generate-config must create only the inert .generated copy.
     */
    if cli.generate_config {
        let generated_path = match config::generate_config_copy() {
            Ok(path) => path,

            Err(error) => {
                eprintln!("scry: unable to generate configuration: {}", error);

                std::process::exit(1);
            }
        };

        let live_path = match config::config_file_path() {
            Ok(path) => path,

            Err(error) => {
                eprintln!(
                    "scry: generated {}, but unable to determine the live configuration path: {}",
                    generated_path.display(),
                    error,
                );

                std::process::exit(1);
            }
        };

        println!(
            "Generated configuration template: {}",
            generated_path.display(),
        );

        println!(
            "Rename it to {} after reviewing and editing it.",
            live_path.display(),
        );

        return Ok(());
    }

    let config = config::ScryConfig::load();

    let session_enabled = cli.restore_session || config.session.restore_session;

    /*
     * A saved source is restored only for an otherwise destination-less launch.
     *
     * Explicit PATH and --ssh values always identify an intentional startup source
     * and therefore take precedence over yesterday's session.
     */
    let should_restore_source = session_enabled && cli.path.is_none() && cli.ssh.is_none();

    let mut save_session_on_exit = session_enabled;

    let mut restored_session = if should_restore_source {
        match session::load() {
            Ok(Some(state)) if state.is_supported() => Some(state),

            Ok(Some(state)) => {
                eprintln!(
                    "scry: saved session format {} is unsupported; expected version {}",
                    state.version,
                    session::SESSION_FORMAT_VERSION,
                );

                /*
                 * Preserve the newer or otherwise unsupported file rather than
                 * replacing it with an ordinary fallback session on exit.
                 */
                save_session_on_exit = false;

                None
            }

            Ok(None) => None,

            Err(error) => {
                eprintln!("scry: unable to load saved session: {}", error);

                /*
                 * A malformed or temporarily unreadable session should not be
                 * overwritten merely because Scry successfully opened normally.
                 */
                save_session_on_exit = false;

                None
            }
        }
    } else {
        None
    };

    if cli.manual {
        /*
         * Reuse the complete F1 Help document.
         *
         * Interactive terminals use their current width, while redirected output
         * uses a stable readable width so the resulting file does not depend on an
         * unavailable or unusually narrow terminal.
         */
        let text_width = if io::stdout().is_terminal() {
            crossterm::terminal::size()
                .map(|(width, _)| width.saturating_sub(4).clamp(40, 100) as usize)
                .unwrap_or(78)
        } else {
            78
        };

        let theme = crate::themes::Theme::load(&config.theme);

        help::print_manual(&theme, text_width)?;

        return Ok(());
    }

    if cli.help {
        external_help::print_help()?;

        return Ok(());
    }

    let mut startup_warning: Option<String> = None;

    let mut app = if let Some(value) = cli.ssh.as_deref() {
        /*
         * Explicit --ssh always overrides a saved session source.
         */
        let target = match SshTarget::parse(value) {
            Ok(target) => target,

            Err(error) => {
                eprintln!("scry: invalid SSH target '{}': {}", value, error,);

                std::process::exit(2);
            }
        };

        eprintln!(
            "scry: connecting to {} through OpenSSH...",
            target.openssh_destination(),
        );

        let (remote_home, source) = match SftpSource::connect(&target, &config.ssh) {
            Ok(connection) => connection,

            Err(error) => {
                eprintln!("scry: remote connection failed: {}", error,);

                std::process::exit(1);
            }
        };

        let remote_start = match cli.path.as_deref() {
            None => remote_home.clone(),

            Some(path) if path == std::path::Path::new(".") => remote_home.clone(),

            Some(path) => path.to_path_buf(),
        };

        match App::with_source_and_home(remote_start.clone(), remote_home, Box::new(source)) {
            Ok(mut app) => {
                app.set_active_ssh_target(target);

                app
            }

            Err(error) => {
                eprintln!(
                    "scry: unable to open remote starting directory {}: {}",
                    remote_start.display(),
                    error,
                );

                std::process::exit(1);
            }
        }
    } else if let Some(state) = restored_session.as_ref() {
        match app_from_session(state, &config.ssh) {
            Ok(app) => app,

            Err(error) => {
                /*
                 * A failed saved SSH connection or vanished local directory must not
                 * prevent Scry from opening.
                 *
                 * Preserve the old session file so the user may retry later.
                 */
                save_session_on_exit = false;

                startup_warning = Some(format!(
                    "Unable to restore the saved session: {}. Started locally instead.",
                    error,
                ));

                restored_session = None;

                match App::new(std::path::PathBuf::from(".")) {
                    Ok(app) => app,

                    Err(fallback_error) => {
                        eprintln!(
                            "scry: session restoration failed, and the local fallback could not open: {}",
                            fallback_error,
                        );

                        std::process::exit(1);
                    }
                }
            }
        }
    } else {
        let local_start = cli
            .path
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from("."));

        match App::new(local_start) {
            Ok(app) => app,

            Err(error) => {
                eprintln!("scry: unable to open starting path: {}", error,);

                std::process::exit(1);
            }
        }
    };

    let mut startup_config = config.clone();

    if let Some(state) = restored_session.as_ref() {
        /*
         * scry.toml supplies startup defaults when no session is restored.
         *
         * A restored session represents the user's most recent interactive state
         * and therefore overrides those defaults. Explicit command-line options
         * are applied afterward and remain authoritative for this launch.
         */
        apply_session_to_startup_config(&mut startup_config, state);
    }

    let ui_state = ui_state::load().unwrap_or_else(|error| {
        eprintln!(
            "scry: unable to load persistent interface state: {}; using defaults",
            error,
        );

        ui_state::UiState::default()
    });

    app.apply_ui_state(ui_state);

    app.apply_startup_config(&startup_config);

    /*
     * Adopt recoverable local deletions before restoring selection and query state.
     *
     * SSH sessions never load the local deletion journal. Invalid journals remain
     * untouched and produce a visible startup warning rather than being acted upon.
     */
    match app.recover_staged_deletions() {
        Ok(0) => {}

        Ok(count) => {
            startup_warning = Some(format!(
                "Recovered {} staged deletion{} from an interrupted Scry session. Press Ctrl+Z to restore.",
                count,
                if count == 1 { "" } else { "s" },
            ));
        }

        Err(error) => {
            startup_warning = Some(format!("Unable to load deletion recovery: {}", error,));
        }
    }

    if let Some(state) = restored_session.as_ref() {
        app.restore_session_state(state);
    }

    if let Some(message) = startup_warning {
        if message.starts_with("Recovered ") {
            app.show_persistent_info_message(message);
        } else {
            app.show_error_message(message);
        }
    }

    if cli.preserve_hierarchy {
        app.enable_preserved_download_hierarchy();
    }

    /*
     * Command-line switches override configuration values.
     *
     * Startup modes are established before the startup query is installed.
     * This ensures that the query is evaluated only after its final scope,
     * matching mode, entry-kind policy, and view have been selected.
     */
    if cli.all && !app.show_hidden {
        app.toggle_hidden();
    }

    if cli.recursive {
        app.request_recursive_mode();
    }

    if cli.fuzzy {
        app.enable_fuzzy_mode();
    }

    if cli.hidden_only && !app.hidden_only_active() {
        app.toggle_hidden_only();
    }

    if cli.files_only {
        app.set_entry_filter(EntryFilter::FilesOnly);
    } else if cli.dirs_only {
        app.set_entry_filter(EntryFilter::DirectoriesOnly);
    }

    if cli.tree && app.view_mode != ViewMode::Tree {
        app.toggle_tree_mode();
    }

    /*
     * Apply the query after List/Tree, Exact/Fuzzy, and recursive scope have
     * reached their final startup state.
     *
     * This is particularly important for non-recursive Tree mode because entering
     * that mode establishes its hierarchy before filtering begins.
     */
    if let Some(query) = cli.query {
        app.set_startup_query(query);
    }

    if cli.permissions {
        app.show_permissions = true;
    }

    if cli.date {
        app.show_date = true;
    }

    if cli.size {
        app.show_size = true;
    }

    if cli.user {
        app.show_user = true;
    }

    if cli.no_open {
        app.disable_file_opening();
    }

    if cli.exit_on_open {
        app.enable_exit_on_open();
    }

    execute!(
        stdout(),
        EnableMouseCapture,
        EnableBracketedPaste,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
    )?;

    let run_result = ratatui::run(|terminal| run_app(terminal, &mut app));

    let disable_result = execute!(
        stdout(),
        PopKeyboardEnhancementFlags,
        DisableBracketedPaste,
        DisableMouseCapture,
    );

    run_result?;

    disable_result?;

    /*
     * Staged deletions become permanent only after Scry has left raw terminal mode.
     *
     * Any failure can therefore be printed normally and remains visible to the user.
     * This runs for every orderly exit route, including Ctrl+C and exit-on-open.
     */
    let (finalized_deletions, deletion_failures) = app.finalize_staged_deletions();

    if finalized_deletions > 0 {
        eprintln!(
            "scry: permanently deleted {} staged entr{}",
            finalized_deletions,
            if finalized_deletions == 1 { "y" } else { "ies" },
        );
    }

    for failure in deletion_failures {
        eprintln!("scry: {}", failure);
    }

    if save_session_on_exit {
        match app.session_state() {
            Ok(state) => {
                if let Err(error) = session::save(&state) {
                    eprintln!("scry: unable to save session state: {}", error);
                }
            }

            Err(error) => {
                eprintln!("scry: unable to construct session state: {}", error);
            }
        }
    }

    if let Some(text) = app.clipboard_handoff_text()
        && let Err(error) = clipboard::spawn_owner(&text)
    {
        eprintln!(
            "scry: unable to preserve clipboard contents after exit: {}",
            error,
        );
    }

    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct ScrollbarDragState {
    start_mouse_row: u16,

    /*
     * Thumb-top position inside its available travel range when dragging began.
     */
    start_thumb_top: usize,

    /*
     * Preserve the selection's screen row while the viewport moves.
     */
    selected_viewport_row: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScrollbarTrackDirection {
    Up,

    Down,
}

#[derive(Debug, Clone, Copy)]
struct ScrollbarTrackHoldState {
    direction: ScrollbarTrackDirection,

    /*
     * Pointer position inside the scrollbar track, excluding panel borders.
     */
    target_track_position: usize,

    next_repeat: Instant,
}

#[derive(Debug, Clone, Copy)]
struct OverlayScrollbarTrackHoldState {
    direction: ScrollbarTrackDirection,

    /*
     * Pointer position inside the Help/Legend scrollbar track.
     */
    target_track_position: usize,

    next_repeat: Instant,
}

#[derive(Debug, Clone, Copy)]
struct HorizontalScrollbarDragState {
    start_mouse_column: u16,

    start_thumb_left: usize,
}

fn run_app(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> io::Result<()> {
    let mut ui_regions = ui::UiRegions::default();

    let mut last_left_click: Option<(Instant, u16, u16)> = None;

    let mut scrollbar_drag: Option<ScrollbarDragState> = None;

    let mut scrollbar_track_hold: Option<ScrollbarTrackHoldState> = None;

    let mut overlay_scrollbar_track_hold: Option<OverlayScrollbarTrackHoldState> = None;

    let mut horizontal_scrollbar_drag: Option<HorizontalScrollbarDragState> = None;

    let mut help_scrollbar_drag = false;

    terminal.draw(|frame| {
        ui_regions = ui::render(frame, app);
    })?;

    while !app.should_quit {
        let mut needs_redraw = app.process_scan_messages();

        if app.process_pending_recursive_search() {
            needs_redraw = true;
        }

        if app.process_notification_timeouts() {
            needs_redraw = true;
        }

        if app.process_rapid_navigation_timeout() {
            needs_redraw = true;
        }

        if app.process_remote_index_load_messages() {
            needs_redraw = true;
        }

        if app.process_remote_index_messages() {
            needs_redraw = true;
        }

        if app.process_fuzzy_messages() {
            needs_redraw = true;
        }

        if app.process_transfer_messages() {
            needs_redraw = true;
        }

        if app.process_connection_messages() {
            needs_redraw = true;
        }

        if app.process_file_info_messages() {
            needs_redraw = true;
        }
        /*
         * Redraw while a transfer is active so elapsed time and the popup remain
         * current between genuine byte-progress messages.
         */
        if app.transfer_visible() && !app.transfer_finished() {
            needs_redraw = true;
        }

        if process_scrollbar_track_hold(app, ui_regions.entries, &mut scrollbar_track_hold) {
            needs_redraw = true;
        }

        if process_overlay_scrollbar_track_hold(
            app,
            ui_regions.help_scrollbar,
            &mut overlay_scrollbar_track_hold,
        ) {
            needs_redraw = true;
        }

        if event::poll(Duration::from_millis(25))? {
            match event::read()? {
                Event::Key(key_event) => {
                    if key_event.kind != KeyEventKind::Press {
                        continue;
                    }

                    let terminal_is_large_enough = terminal::size()
                        .map(|(width, height)| ui::terminal_size_is_sufficient(width, height))
                        .unwrap_or(true);

                    if !terminal_is_large_enough {
                        /*
                         * While the normal interface is hidden, do not allow invisible browser
                         * operations to modify application state.
                         *
                         * Ctrl+C remains available so Scry can always be exited.
                         */
                        if key_event.code == KeyCode::Char('c')
                            && key_event.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            app.quit();
                        }

                        needs_redraw = true;

                        continue;
                    }

                    handle_key_event(app, key_event);

                    needs_redraw = true;
                }

                Event::Paste(text) => {
                    let terminal_is_large_enough = terminal::size()
                        .map(|(width, height)| ui::terminal_size_is_sufficient(width, height))
                        .unwrap_or(true);

                    if terminal_is_large_enough {
                        handle_paste_event(app, &text);
                    }

                    needs_redraw = true;
                }

                Event::Mouse(mouse_event) => {
                    let terminal_is_large_enough = terminal::size()
                        .map(|(width, height)| ui::terminal_size_is_sufficient(width, height))
                        .unwrap_or(true);

                    if terminal_is_large_enough {
                        handle_mouse_event(
                            app,
                            mouse_event,
                            ui_regions,
                            &mut last_left_click,
                            &mut scrollbar_drag,
                            &mut scrollbar_track_hold,
                            &mut overlay_scrollbar_track_hold,
                            &mut horizontal_scrollbar_drag,
                            &mut help_scrollbar_drag,
                        );
                    }

                    needs_redraw = true;
                }

                Event::Resize(_, _) => {
                    /*
                     * The next draw recalculates every layout rectangle and
                     * updates app.viewport_rows.
                     */
                    needs_redraw = true;
                }

                _ => {}
            }
        }

        if app.process_scan_messages() {
            needs_redraw = true;
        }

        if app.process_pending_recursive_search() {
            needs_redraw = true;
        }

        if app.process_remote_index_load_messages() {
            needs_redraw = true;
        }

        if app.process_remote_index_messages() {
            needs_redraw = true;
        }

        if app.process_fuzzy_messages() {
            needs_redraw = true;
        }

        if app.process_transfer_messages() {
            needs_redraw = true;
        }

        if app.process_connection_messages() {
            needs_redraw = true;
        }

        if app.process_file_info_messages() {
            needs_redraw = true;
        }

        if app.connection_in_progress {
            needs_redraw = true;
        }

        if needs_redraw {
            terminal.draw(|frame| {
                ui_regions = ui::render(frame, app);
            })?;
        }
    }

    Ok(())
}

fn handle_paste_event(app: &mut App, text: &str) {
    /*
     * Bracketed paste must never reinterpret clipboard contents as Scry commands.
     *
     * In particular, newlines from a multiline clipboard must not become Enter
     * presses that activate files or enter directories.
     *
     * Scry's editable fields are single-line, so pasted line breaks and tabs are
     * normalized to ordinary spaces. Other control characters are discarded.
     */
    let paste_into_connection = app.connection_visible();

    /*
     * Paste is ignored while a non-editable overlay or modal operation owns input.
     */
    if !paste_into_connection
        && (app.file_info_visible()
            || app.tree_expand_all_dialog_visible()
            || app.remote_index_setup_visible()
            || app.deletion_visible()
            || app.transfer_visible()
            || app.about_visible()
            || app.legend_visible()
            || app.help_visible())
    {
        return;
    }

    let mut previous_was_separator = false;

    for character in text.chars() {
        let character = match character {
            '\r' | '\n' | '\t' => {
                if previous_was_separator {
                    continue;
                }

                previous_was_separator = true;

                ' '
            }

            character if character.is_control() => {
                continue;
            }

            character => {
                previous_was_separator = false;

                character
            }
        };

        if paste_into_connection {
            app.connection_push_character(character);
        } else {
            app.push_query_character(character);
        }
    }
}

fn handle_key_event(app: &mut App, mut key_event: KeyEvent) {
    /*
     * Alphabetic Ctrl/Alt shortcuts are case-insensitive.
     *
     * Caps Lock and enhanced keyboard protocols may report Alt+R as either:
     *
     *     Char('R') with ALT
     *     Char('R') with ALT | SHIFT
     *
     * Normalize only modified shortcut events. Ordinary text entry retains its
     * original case.
     */
    if key_event
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        && let KeyCode::Char(character) = key_event.code
        && character.is_ascii_alphabetic()
    {
        key_event.code = KeyCode::Char(character.to_ascii_lowercase());

        key_event.modifiers.remove(KeyModifiers::SHIFT);
    }

    if app.file_info_visible() {
        match (key_event.code, key_event.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                app.quit();
            }

            (KeyCode::F(2), _) | (KeyCode::Char('i'), KeyModifiers::ALT) => {
                app.close_file_info();
            }

            (KeyCode::Enter, modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
                app.close_file_info();
            }

            (KeyCode::Esc, _) | (KeyCode::Enter, _) => {
                app.close_file_info();
            }

            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                app.scroll_file_info_up();
            }

            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                app.scroll_file_info_down();
            }

            (KeyCode::PageUp, _) => {
                app.page_file_info_up();
            }

            (KeyCode::PageDown, _) => {
                app.page_file_info_down();
            }

            (KeyCode::Home, _) => {
                app.file_info_scroll_to_start();
            }

            (KeyCode::End, _) => {
                app.file_info_scroll_to_end();
            }

            _ => {}
        }

        return;
    }

    if app.tree_expand_all_dialog_visible() {
        let dialog_kind = app
            .tree_expand_all_dialog
            .as_ref()
            .map(|dialog| dialog.kind);

        match (key_event.code, key_event.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                app.quit();
            }

            (KeyCode::Esc, _) => {
                app.cancel_tree_expand_all_dialog();
            }

            /*
             * The refusal window has no button or selectable action.
             *
             * Enter simply acknowledges and closes it.
             */
            (KeyCode::Enter, _)
                if matches!(
                    dialog_kind,
                    Some(TreeExpandAllDialogKind::Refusal | TreeExpandAllDialogKind::DisplayLimit)
                ) =>
            {
                app.confirm_tree_expand_all_dialog();
            }

            /*
             * Space toggles persistent suppression only in the local warning.
             *
             * The App method rejects the action for every other dialog kind.
             */
            (KeyCode::Char(' '), _) => {
                app.toggle_tree_expand_all_warning_suppression();
            }

            /*
             * Local and SSH confirmations continue through their single OK action.
             */
            (KeyCode::Enter, _) => {
                app.confirm_tree_expand_all_dialog();
            }

            _ => {}
        }

        return;
    }

    if app.remote_index_setup_visible() {
        let focus = app.remote_index_setup.as_ref().map(|setup| setup.focus);

        match (key_event.code, key_event.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                app.quit();
            }

            (KeyCode::Esc, _) => {
                app.close_remote_index_setup();
            }

            /*
             * Tab changes between the policy group, OK, and Cancel.
             */
            (KeyCode::BackTab, _) | (KeyCode::Tab, KeyModifiers::SHIFT) => {
                app.remote_index_dialog_previous_focus();
            }

            (KeyCode::Tab, _) => {
                app.remote_index_dialog_next_focus();
            }

            /*
             * Left and Right change the radio selection only while the policy
             * group owns focus. The focus remains on the group.
             */
            (KeyCode::Left, _) if focus == Some(RemoteIndexDialogFocus::Policy) => {
                app.select_remote_index_policy(false);
            }

            (KeyCode::Right, _) if focus == Some(RemoteIndexDialogFocus::Policy) => {
                app.select_remote_index_policy(true);
            }

            /*
             * Up and Down may also switch the two vertically displayed options.
             */
            (KeyCode::Up, _) if focus == Some(RemoteIndexDialogFocus::Policy) => {
                app.select_remote_index_policy(false);
            }

            (KeyCode::Down, _) if focus == Some(RemoteIndexDialogFocus::Policy) => {
                app.select_remote_index_policy(true);
            }

            /*
             * Space changes the selected radio policy. It never confirms a build.
             */
            (KeyCode::Char(' '), _) if focus == Some(RemoteIndexDialogFocus::Policy) => {
                app.toggle_remote_index_policy();
            }

            (KeyCode::Enter, _) => {
                app.confirm_remote_index_setup();
            }

            _ => {}
        }

        return;
    }

    if app.deletion_visible() {
        match (key_event.code, key_event.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                app.quit();
            }

            (KeyCode::Esc, _) => {
                app.cancel_deletion();
            }

            (KeyCode::Left, _)
            | (KeyCode::Right, _)
            | (KeyCode::Tab, _)
            | (KeyCode::BackTab, _) => {
                app.toggle_deletion_choice();
            }

            (KeyCode::Enter, _) => {
                let choice = app.deletion.as_ref().map(|deletion| deletion.choice);

                match choice {
                    Some(DeletionChoice::Cancel) => {
                        app.cancel_deletion();
                    }

                    /*
                     * Actual filesystem removal is connected in the next stage.
                     *
                     * For now, Enter deliberately leaves the confirmation window
                     * open when Delete is selected.
                     */
                    Some(DeletionChoice::Delete) => {
                        app.confirm_deletion();
                    }

                    None => {}
                }
            }

            _ => {}
        }

        return;
    }

    if app.transfer_visible() {
        match (key_event.code, key_event.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                app.quit();
            }

            (KeyCode::Enter, _) | (KeyCode::Esc, _) if app.transfer_finished() => {
                app.acknowledge_transfer();
            }

            _ => {}
        }

        return;
    }

    if app.connection_visible() {
        match (key_event.code, key_event.modifiers) {
            (KeyCode::F(4), _) | (KeyCode::Esc, _) => {
                app.close_connection_dialog();
            }

            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                app.quit();
            }

            (KeyCode::Tab, KeyModifiers::SHIFT) | (KeyCode::BackTab, _) => {
                app.connection_focus_previous();
            }

            (KeyCode::Tab, _) => {
                app.connection_focus_next();
            }

            (KeyCode::Left, _) if app.connection_dialog.focus == ConnectionField::Profiles => {
                app.connection_previous_profile();
            }

            (KeyCode::Right, _) if app.connection_dialog.focus == ConnectionField::Profiles => {
                app.connection_next_profile();
            }

            (KeyCode::Left, _) => {
                app.connection_move_cursor_left();
            }

            (KeyCode::Right, _) => {
                app.connection_move_cursor_right();
            }

            (KeyCode::Home, _) => {
                app.connection_move_cursor_to_start();
            }

            (KeyCode::End, _) => {
                app.connection_move_cursor_to_end();
            }

            (KeyCode::Enter, _) => {
                use crate::connection::ConnectionField;

                match app.connection_dialog.focus {
                    ConnectionField::Connect => {
                        app.begin_connection();
                    }

                    ConnectionField::Save => {
                        app.save_connection_profile();
                    }

                    ConnectionField::Delete => {
                        app.delete_connection_profile();
                    }

                    ConnectionField::Disconnect => {
                        app.disconnect_remote();
                    }

                    ConnectionField::Close => {
                        app.close_connection_dialog();
                    }

                    /*
                     * Enter inside an editable field or the profile selector advances to
                     * the next enabled control.
                     */
                    _ => {
                        app.connection_focus_next();
                    }
                }
            }

            (KeyCode::Up, _) => {
                app.connection_focus_previous();
            }

            (KeyCode::Down, _) => {
                app.connection_focus_next();
            }

            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                app.connection_clear_field();
            }

            /*
             * Backspace may arrive either as the dedicated key code or as Ctrl+H,
             * depending on the terminal and keyboard-enhancement support.
             */
            (KeyCode::Backspace, _) | (KeyCode::Char('h'), KeyModifiers::CONTROL) => {
                app.connection_pop_character();
            }

            (KeyCode::Char(character), modifiers)
                if !modifiers.contains(KeyModifiers::CONTROL)
                    && !modifiers.contains(KeyModifiers::ALT) =>
            {
                app.connection_push_character(character);
            }

            _ => {}
        }

        return;
    }

    if app.about_visible() {
        match (key_event.code, key_event.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                app.quit();
            }

            (KeyCode::Char('a'), KeyModifiers::ALT) | (KeyCode::Esc, _) | (KeyCode::Enter, _) => {
                app.close_about();
            }

            _ => {}
        }

        return;
    }

    if app.legend_visible() {
        match (key_event.code, key_event.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                app.quit();
            }

            (KeyCode::Char('?'), _) | (KeyCode::Esc, _) | (KeyCode::Enter, _) => {
                app.close_legend();
            }

            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                app.scroll_legend_up();
            }

            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                app.scroll_legend_down();
            }

            (KeyCode::PageUp, _) => {
                app.page_legend_up();
            }

            (KeyCode::PageDown, _) => {
                app.page_legend_down();
            }

            (KeyCode::End, _) => {
                app.legend_scroll_to_end();
            }

            (KeyCode::Home, _) => {
                app.legend_scroll_to_home();
            }

            _ => {}
        }

        return;
    }

    if app.help_visible() {
        match (key_event.code, key_event.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                app.quit();
            }

            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                app.scroll_help_up();
            }

            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                app.scroll_help_down();
            }

            (KeyCode::PageUp, _) => {
                app.page_help_up();
            }

            (KeyCode::PageDown, _) => {
                app.page_help_down();
            }

            (KeyCode::Home, _) => {
                app.help_scroll = 0;
            }

            (KeyCode::End, _) => {
                app.help_scroll_to_end();
            }

            (KeyCode::F(1), _) | (KeyCode::Esc, _) | (KeyCode::Enter, _) => {
                app.close_help();
            }

            _ => {}
        }

        return;
    }

    match (key_event.code, key_event.modifiers) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            app.quit();
        }

        (KeyCode::F(1), _) => {
            app.toggle_help();
        }

        (KeyCode::F(2), _) => {
            app.open_file_info();
        }

        (KeyCode::Char('i'), KeyModifiers::ALT) => {
            app.open_file_info();
        }

        (KeyCode::F(3), _) => {
            app.toggle_icons();
        }

        (KeyCode::F(4), _) => {
            app.toggle_connection_dialog();
        }

        (KeyCode::F(5), _) => {
            app.open_remote_index_builder();
        }

        (KeyCode::F(6), _) => {
            app.toggle_hidden_only();
        }

        (KeyCode::F(7), _) => {
            app.toggle_permissions_column();
        }

        (KeyCode::F(8), _) => {
            app.toggle_size_column();
        }

        (KeyCode::F(9), _) => {
            app.toggle_date_column();
        }

        (KeyCode::F(10), _) => {
            app.toggle_user_column();
        }

        /*
         * F11 remains available for a future feature.
         *
         * F12 toggles classified filename colors without changing icons,
         * classification, sorting, filtering, or filesystem state.
         */
        (KeyCode::F(12), _) => {
            app.toggle_file_colors();
        }

        (KeyCode::Char('u'), KeyModifiers::ALT) => {
            app.clear_marks();
        }

        (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
            app.clear_query();
        }

        (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
            app.toggle_details();
        }

        (KeyCode::Char('s'), KeyModifiers::CONTROL) => {
            app.toggle_selection_panel();
        }

        /*
         * Backspace belongs exclusively to search-field editing.
         *
         * Some terminals report Backspace as Ctrl+H, so both forms retain the same
         * query-editing behavior. At the beginning of an empty query, the key does
         * nothing instead of unexpectedly navigating to the parent directory.
         */
        (KeyCode::Backspace, _) | (KeyCode::Char('h'), KeyModifiers::CONTROL) => {
            app.pop_query_character();
        }

        /*
         * Ctrl+M is the carriage-return control code and may be reported as Enter by
         * the terminal. Never allow it to activate a directory or file.
         */
        (KeyCode::Enter, modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            app.open_file_info();
        }

        /*
         * Ctrl+M must never activate anything.
         */
        (KeyCode::Char('m'), KeyModifiers::CONTROL) => {}

        (KeyCode::Char('h'), KeyModifiers::ALT) => {
            app.toggle_hidden();
        }

        (KeyCode::Char('d'), KeyModifiers::ALT) => {
            app.begin_marked_transfer_batch();
        }

        (KeyCode::Char('m'), KeyModifiers::ALT) => {
            app.toggle_columns_panel();
        }

        (KeyCode::Char('a'), KeyModifiers::ALT) => {
            app.toggle_about();
        }

        (KeyCode::Char('t'), KeyModifiers::CONTROL) => {
            app.toggle_tree_mode();
        }

        (KeyCode::Char('e'), KeyModifiers::ALT) => {
            app.request_toggle_all_tree_branches();
        }

        (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
            app.toggle_search_mode();
        }

        (KeyCode::Char('o'), KeyModifiers::CONTROL) => {
            app.cycle_sort_mode();
        }

        (KeyCode::Char('r'), KeyModifiers::ALT) => {
            app.toggle_recursive_mode();
        }

        (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
            app.toggle_sort_direction();
        }

        /*
         * Horizontal entry scrolling.
         *
         * Shift+Left/Right moves the shared Metadata/filesystem viewport.
         * Plain arrows remain dedicated to browser navigation, while Control-modified
         * arrows edit the search-field caret.
         */
        (KeyCode::Left, modifiers)
            if modifiers.contains(KeyModifiers::SHIFT)
                && !modifiers.contains(KeyModifiers::CONTROL) =>
        {
            app.scroll_horizontal_left();
        }

        (KeyCode::Right, modifiers)
            if modifiers.contains(KeyModifiers::SHIFT)
                && !modifiers.contains(KeyModifiers::CONTROL) =>
        {
            app.scroll_horizontal_right();
        }

        /*
         * Browser navigation follows conventional file-browser bindings.
         *
         * Plain Left/Right operate on the current directory or Tree structure.
         * Control-modified arrows are reserved for editing the always-active query.
         */
        (KeyCode::Left, modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            app.move_query_cursor_left();
        }

        (KeyCode::Right, modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            app.move_query_cursor_right();
        }

        (KeyCode::Left, _) => {
            app.enter_parent_directory();
        }

        (KeyCode::Right, _) => {
            app.enter_selected_directory();
        }

        /*
         * Home and End belong to browser navigation.
         *
         * Control moves the query caret to its corresponding boundary.
         */
        (KeyCode::Home, modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            app.move_query_cursor_to_start();
        }

        (KeyCode::End, modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            app.move_query_cursor_to_end();
        }

        (KeyCode::Home, _) => {
            app.begin_rapid_navigation();

            app.select_first();
        }

        (KeyCode::End, _) => {
            app.begin_rapid_navigation();

            app.select_last();
        }

        (KeyCode::Esc, _) => {
            app.enter_parent_directory();
        }

        (KeyCode::Up, _) => {
            app.begin_rapid_navigation();

            app.move_up();
        }

        (KeyCode::Down, _) => {
            app.begin_rapid_navigation();

            app.move_down();
        }

        (KeyCode::PageUp, modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            app.begin_rapid_navigation();

            app.fast_page_up();
        }

        (KeyCode::PageDown, modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            app.begin_rapid_navigation();

            app.fast_page_down();
        }

        (KeyCode::PageUp, _) => {
            app.begin_rapid_navigation();

            app.page_up();
        }

        (KeyCode::PageDown, _) => {
            app.begin_rapid_navigation();

            app.page_down();
        }

        /*
         * Undo the most recently staged local deletion.
         *
         * Scry receives Ctrl+Z while its raw terminal interface is active, so the key
         * is handled here rather than being passed to the shell as job suspension.
         */
        (KeyCode::Char('z'), KeyModifiers::CONTROL) => {
            app.restore_last_deletion();
        }

        (KeyCode::Delete, _) => {
            app.begin_deletion_confirmation();
        }

        /*
         * Enter always activates the currently highlighted entry.
         *
         * Remote files use the single-file transfer path independently from persistent
         * batch marks. Marked files are downloaded only through Alt+D.
         */
        (KeyCode::Enter, KeyModifiers::NONE) => {
            app.activate_selected();
        }

        (KeyCode::Char('?'), _) => {
            app.toggle_legend();
        }

        (KeyCode::Char('y'), KeyModifiers::CONTROL) => {
            app.copy_selected_path();
        }

        /*
         * Ctrl+Space marks or unmarks the file beneath the cursor.
         *
         * Some terminals report Ctrl+Space as a literal space carrying CONTROL,
         * while others expose the traditional NUL character. Support both forms.
         */
        (KeyCode::Char(' '), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            app.toggle_mark_selected();
        }

        (KeyCode::Char('\0'), _) => {
            app.toggle_mark_selected();
        }

        (KeyCode::Char(character), modifiers)
            if !modifiers.contains(KeyModifiers::CONTROL)
                && !modifiers.contains(KeyModifiers::ALT) =>
        {
            app.push_query_character(character);
        }

        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_mouse_event(
    app: &mut App,
    event: MouseEvent,
    regions: ui::UiRegions,
    last_left_click: &mut Option<(Instant, u16, u16)>,
    scrollbar_drag: &mut Option<ScrollbarDragState>,
    scrollbar_track_hold: &mut Option<ScrollbarTrackHoldState>,
    overlay_scrollbar_track_hold: &mut Option<OverlayScrollbarTrackHoldState>,
    horizontal_scrollbar_drag: &mut Option<HorizontalScrollbarDragState>,
    help_scrollbar_drag: &mut bool,
) {
    /*
     * File Information is modal.
     *
     * A left click on its Close button dismisses the window. Every other mouse
     * event is consumed so it cannot affect the browser hidden beneath it.
     */
    if app.file_info_visible() {
        if let (MouseEventKind::Down(MouseButton::Left), Some(close_area)) =
            (event.kind, regions.file_info_close)
        {
            let inside_close_button = event.column >= close_area.x
                && event.column < close_area.x.saturating_add(close_area.width)
                && event.row >= close_area.y
                && event.row < close_area.y.saturating_add(close_area.height);

            if inside_close_button {
                app.close_file_info();
            }
        }

        return;
    }

    if app.tree_expand_all_dialog_visible() {
        /*
         * The large-Tree dialog owns every mouse event while visible.
         *
         * Nothing may pass through into the Tree behind the popup.
         */
        *scrollbar_drag = None;

        *scrollbar_track_hold = None;

        *horizontal_scrollbar_drag = None;

        *help_scrollbar_drag = false;

        *last_left_click = None;

        if !matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
            return;
        }

        let Some(dialog_regions) = regions.tree_expand_all else {
            return;
        };

        let inside = |area: Rect| {
            event.column >= area.x
                && event.column < area.x.saturating_add(area.width)
                && event.row >= area.y
                && event.row < area.y.saturating_add(area.height)
        };

        if dialog_regions.warning_checkbox.is_some_and(inside) {
            app.toggle_tree_expand_all_warning_suppression();
        } else if dialog_regions.expand_all.is_some_and(inside) {
            app.select_tree_expand_all_dialog_focus(TreeExpandAllDialogFocus::ExpandAll);

            app.confirm_tree_expand_all_dialog();
        } else if dialog_regions.cancel.is_some_and(inside) {
            app.select_tree_expand_all_dialog_focus(TreeExpandAllDialogFocus::Cancel);

            app.cancel_tree_expand_all_dialog();
        } else if dialog_regions.ok.is_some_and(inside) {
            app.confirm_tree_expand_all_dialog();
        }

        return;
    }

    if app.remote_index_setup_visible() {
        /*
         * The setup window owns every mouse event while visible.
         *
         * Events that do not land on one of its choices are deliberately ignored
         * rather than being passed to the browser underneath.
         */
        *scrollbar_drag = None;

        *scrollbar_track_hold = None;

        *horizontal_scrollbar_drag = None;

        *help_scrollbar_drag = false;

        *last_left_click = None;

        if !matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
            return;
        }

        let Some(setup_regions) = regions.remote_index_setup else {
            return;
        };

        let inside = |area: Rect| {
            event.column >= area.x
                && event.column < area.x.saturating_add(area.width)
                && event.row >= area.y
                && event.row < area.y.saturating_add(area.height)
        };

        if inside(setup_regions.standard) {
            app.select_remote_index_dialog_focus(RemoteIndexDialogFocus::Policy);

            app.select_remote_index_policy(false);
        } else if inside(setup_regions.include_hidden) {
            app.select_remote_index_dialog_focus(RemoteIndexDialogFocus::Policy);

            app.select_remote_index_policy(true);
        } else if inside(setup_regions.ok) {
            app.select_remote_index_dialog_focus(RemoteIndexDialogFocus::Ok);

            app.confirm_remote_index_setup();
        } else if inside(setup_regions.cancel) {
            app.select_remote_index_dialog_focus(RemoteIndexDialogFocus::Cancel);

            app.close_remote_index_setup();
        }

        return;
    }

    if app.deletion_visible() {
        *scrollbar_drag = None;

        *scrollbar_track_hold = None;

        *horizontal_scrollbar_drag = None;

        *help_scrollbar_drag = false;

        *last_left_click = None;

        /*
         * Deletion confirmation is modal.
         *
         * Every mouse event is consumed here so the browser beneath the popup
         * cannot be selected or activated.
         */
        if !matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
            return;
        }

        let Some(deletion_regions) = regions.deletion else {
            return;
        };

        let inside = |area: Rect| {
            event.column >= area.x
                && event.column < area.x.saturating_add(area.width)
                && event.row >= area.y
                && event.row < area.y.saturating_add(area.height)
        };

        if inside(deletion_regions.delete) {
            app.select_deletion_choice(DeletionChoice::Delete);

            app.confirm_deletion();
        } else if inside(deletion_regions.cancel) {
            app.select_deletion_choice(DeletionChoice::Cancel);

            app.cancel_deletion();
        }

        return;
    }

    if app.transfer_visible() {
        *scrollbar_drag = None;

        *scrollbar_track_hold = None;

        *horizontal_scrollbar_drag = None;

        *last_left_click = None;

        handle_transfer_mouse_event(app, event, regions.transfer);

        return;
    }

    /*
     * The connection window is modal.
     *
     * Mouse input must never reach the filesystem view behind it. Actual
     * connection-window hit testing will be added with its editable controls.
     */
    if app.connection_visible() {
        *scrollbar_drag = None;

        *scrollbar_track_hold = None;

        *horizontal_scrollbar_drag = None;

        *last_left_click = None;

        handle_connection_mouse_event(app, event, regions.connection);

        return;
    }

    if app.about_visible() {
        *scrollbar_drag = None;

        *scrollbar_track_hold = None;

        *horizontal_scrollbar_drag = None;

        *help_scrollbar_drag = false;

        *last_left_click = None;

        /*
         * About is modal. Mouse events must not activate filesystem controls
         * behind the popup.
         */
        return;
    }

    if app.help_visible() || app.legend_visible() {
        *scrollbar_drag = None;

        *scrollbar_track_hold = None;

        *horizontal_scrollbar_drag = None;

        *last_left_click = None;

        let overlay_scrollbar = regions.help_scrollbar;

        let on_overlay_scrollbar = overlay_scrollbar.is_some_and(|area| {
            event.column >= area.x
                && event.column < area.x.saturating_add(area.width)
                && event.row >= area.y
                && event.row < area.y.saturating_add(area.height)
        });

        let on_help_tips_link = app.help_visible()
            && regions.help_tips_link.is_some_and(|area| {
                event.column >= area.x
                    && event.column < area.x.saturating_add(area.width)
                    && event.row >= area.y
                    && event.row < area.y.saturating_add(area.height)
            });

        let on_help_top_link = app.help_visible()
            && regions.help_top_link.is_some_and(|area| {
                event.column >= area.x
                    && event.column < area.x.saturating_add(area.width)
                    && event.row >= area.y
                    && event.row < area.y.saturating_add(area.height)
            });

        match event.kind {
            MouseEventKind::ScrollUp => {
                if app.legend_visible() {
                    app.scroll_legend_up();
                } else {
                    app.scroll_help_up();
                }
            }

            MouseEventKind::ScrollDown => {
                if app.legend_visible() {
                    app.scroll_legend_down();
                } else {
                    app.scroll_help_down();
                }
            }

            MouseEventKind::Down(MouseButton::Left) if on_help_tips_link => {
                *last_left_click = None;

                *help_scrollbar_drag = false;

                *overlay_scrollbar_track_hold = None;

                app.help_tips_hovered = false;

                app.help_scroll_to_tips();
            }

            MouseEventKind::Down(MouseButton::Left) if on_help_top_link => {
                *last_left_click = None;

                *help_scrollbar_drag = false;

                *overlay_scrollbar_track_hold = None;

                app.help_top_hovered = false;

                app.help_scroll_to_top();
            }

            MouseEventKind::Down(MouseButton::Left) if on_overlay_scrollbar => {
                *last_left_click = None;

                let area = overlay_scrollbar.expect("checked overlay scrollbar region");

                let maximum_scroll = if app.legend_visible() {
                    app.legend_max_scroll
                } else {
                    app.help_max_scroll
                };

                let viewport_length = area.height as usize;

                let content_length = maximum_scroll as usize + viewport_length;

                let track_length = area.height as usize;

                if maximum_scroll == 0 || track_length == 0 {
                    *help_scrollbar_drag = false;

                    *overlay_scrollbar_track_hold = None;

                    return;
                }

                let thumb_length =
                    scrollbar_thumb_length(content_length, viewport_length, track_length);

                let thumb_travel = track_length.saturating_sub(thumb_length);

                if thumb_travel == 0 {
                    *help_scrollbar_drag = false;

                    *overlay_scrollbar_track_hold = None;

                    return;
                }

                let current_scroll = if app.legend_visible() {
                    app.legend_scroll
                } else {
                    app.help_scroll
                } as usize;

                let current_thumb_top = current_scroll
                    .saturating_mul(thumb_travel)
                    .saturating_add(maximum_scroll as usize / 2)
                    / maximum_scroll as usize;

                let clicked_track_position = event
                    .row
                    .saturating_sub(area.y)
                    .min(area.height.saturating_sub(1))
                    as usize;

                let clicked_inside_thumb = clicked_track_position >= current_thumb_top
                    && clicked_track_position < current_thumb_top.saturating_add(thumb_length);

                if clicked_inside_thumb {
                    /*
                     * Clicking the thumb retains ordinary proportional dragging.
                     */
                    *overlay_scrollbar_track_hold = None;

                    *help_scrollbar_drag = true;
                } else {
                    /*
                     * Clicking the track moves exactly one viewport toward the pointer.
                     *
                     * Holding the button continues paging until the thumb reaches the
                     * clicked position, matching the filesystem scrollbar.
                     */
                    *help_scrollbar_drag = false;

                    let direction = if clicked_track_position < current_thumb_top {
                        if app.legend_visible() {
                            app.page_legend_up();
                        } else {
                            app.page_help_up();
                        }

                        ScrollbarTrackDirection::Up
                    } else {
                        if app.legend_visible() {
                            app.page_legend_down();
                        } else {
                            app.page_help_down();
                        }

                        ScrollbarTrackDirection::Down
                    };

                    *overlay_scrollbar_track_hold = Some(OverlayScrollbarTrackHoldState {
                        direction,

                        target_track_position: clicked_track_position,

                        next_repeat: Instant::now() + Duration::from_millis(300),
                    });
                }
            }

            MouseEventKind::Drag(MouseButton::Left) if *help_scrollbar_drag => {
                if let Some(area) = overlay_scrollbar {
                    drag_overlay_scrollbar(app, event.row, area);
                }
            }

            MouseEventKind::Up(MouseButton::Left) => {
                *help_scrollbar_drag = false;

                *overlay_scrollbar_track_hold = None;

                *scrollbar_drag = None;

                *scrollbar_track_hold = None;

                *horizontal_scrollbar_drag = None;

                app.scrollbar_drag_active = false;
            }

            _ => {}
        }

        return;
    }

    *help_scrollbar_drag = false;

    const WHEEL_STEP: isize = 3;

    let area = regions.entries;

    let parent_button = regions.parent_button;

    let on_parent_button = event.column >= parent_button.x
        && event.column < parent_button.x.saturating_add(parent_button.width)
        && event.row >= parent_button.y
        && event.row < parent_button.y.saturating_add(parent_button.height);

    let right_edge = area.x.saturating_add(area.width).saturating_sub(1);

    let inside_entries_panel = event.column >= area.x
        && event.column < area.x.saturating_add(area.width)
        && event.row >= area.y
        && event.row < area.y.saturating_add(area.height);

    let inside_entry_rows = inside_entries_panel
        && event.row > area.y
        && event.row < area.y.saturating_add(area.height).saturating_sub(1);

    /*
     * The home control occupies the lower-left border of the filesystem panel.
     */
    let inside_home_button = event.column >= regions.home_button.x
        && event.column
            < regions
                .home_button
                .x
                .saturating_add(regions.home_button.width)
        && event.row >= regions.home_button.y
        && event.row
            < regions
                .home_button
                .y
                .saturating_add(regions.home_button.height);

    let horizontal_scrollbar_area = regions.horizontal_scrollbar;

    let on_horizontal_scrollbar = horizontal_scrollbar_area.is_some_and(|area| {
        event.column >= area.x
            && event.column < area.x.saturating_add(area.width)
            && event.row >= area.y
            && event.row < area.y.saturating_add(area.height)
    });

    /*
     * The terminal's visible mouse pointer may overlap the scrollbar while its
     * reported cell lies immediately to either side of the rendered column.
     *
     * Test the scrollbar rows independently, from inside_entries_panel so the
     * cell immediately to the right of the panel, remains a valid grab target.
     */
    let inside_scrollbar_rows =
        event.row > area.y && event.row < area.y.saturating_add(area.height).saturating_sub(1);

    let scrollbar_hit_left = right_edge.saturating_sub(1);

    let scrollbar_hit_right = right_edge.saturating_add(1);

    let on_scrollbar = inside_scrollbar_rows
        && event.column >= scrollbar_hit_left
        && event.column <= scrollbar_hit_right;

    match event.kind {
        MouseEventKind::Down(MouseButton::Left) if on_horizontal_scrollbar => {
            *last_left_click = None;

            *scrollbar_drag = None;

            let Some(area) = horizontal_scrollbar_area else {
                return;
            };

            const HORIZONTAL_THUMB_WIDTH: usize = 5;

            let track_length = area.width as usize;

            let thumb_width = HORIZONTAL_THUMB_WIDTH.min(track_length);

            let thumb_travel = track_length.saturating_sub(thumb_width);

            if thumb_travel == 0 || app.horizontal_max_offset == 0 {
                return;
            }

            let current_thumb_left = app
                .horizontal_offset
                .saturating_mul(thumb_travel)
                .saturating_add(app.horizontal_max_offset / 2)
                / app.horizontal_max_offset;

            let clicked_position = event.column.saturating_sub(area.x) as usize;

            let clicked_inside_thumb = clicked_position >= current_thumb_left
                && clicked_position < current_thumb_left.saturating_add(thumb_width);

            /*
             * Clicking outside the handle jumps it so its center lands beneath the
             * pointer. Clicking the handle itself begins an ordinary relative drag.
             */
            let start_thumb_left = if clicked_inside_thumb {
                current_thumb_left
            } else {
                let requested_thumb_left = clicked_position.saturating_sub(thumb_width / 2);

                let requested_thumb_left = requested_thumb_left.min(thumb_travel);

                app.horizontal_offset = requested_thumb_left
                    .saturating_mul(app.horizontal_max_offset)
                    .saturating_add(thumb_travel / 2)
                    / thumb_travel;

                requested_thumb_left
            };

            *horizontal_scrollbar_drag = Some(HorizontalScrollbarDragState {
                start_mouse_column: event.column,

                start_thumb_left,
            });

            app.scrollbar_drag_active = true;
        }

        MouseEventKind::Drag(MouseButton::Left) if horizontal_scrollbar_drag.is_some() => {
            let Some(area) = horizontal_scrollbar_area else {
                *horizontal_scrollbar_drag = None;

                app.scrollbar_drag_active = false;

                return;
            };

            let Some(drag) = *horizontal_scrollbar_drag else {
                return;
            };

            drag_horizontal_scrollbar(app, event.column, area, drag);
        }

        MouseEventKind::Down(MouseButton::Left) if on_parent_button => {
            *scrollbar_drag = None;

            *scrollbar_track_hold = None;

            *horizontal_scrollbar_drag = None;

            app.scrollbar_drag_active = false;

            *last_left_click = None;

            app.enter_previous_directory();
        }

        MouseEventKind::ScrollUp => {
            app.begin_rapid_navigation();

            app.scroll_selection(-WHEEL_STEP);
        }

        MouseEventKind::ScrollDown => {
            app.begin_rapid_navigation();

            app.scroll_selection(WHEEL_STEP);
        }

        /*
         * Clicking the vertical scrollbar behaves like the horizontal scrollbar:
         *
         * - clicking the thumb begins a relative drag;
         * - clicking elsewhere on the track jumps the thumb beneath the pointer,
         *   then allows dragging to continue from that new position.
         */
        MouseEventKind::Down(MouseButton::Left) if on_scrollbar => {
            *last_left_click = None;

            /*
             * A vertical-scrollbar interaction cancels any horizontal drag that may
             * still be active from an unusual mouse-event sequence.
             */
            *horizontal_scrollbar_drag = None;

            let content_length = app.current_visible_entry_count();

            let viewport_length = app.viewport_rows;

            let track_length = area.height.saturating_sub(2) as usize;

            if content_length <= viewport_length || track_length == 0 {
                *scrollbar_drag = None;

                app.scrollbar_drag_active = false;

                return;
            }

            let thumb_length =
                scrollbar_thumb_length(content_length, viewport_length, track_length);

            let thumb_travel = track_length.saturating_sub(thumb_length);

            let maximum_offset = content_length.saturating_sub(viewport_length);

            if thumb_travel == 0 || maximum_offset == 0 {
                *scrollbar_drag = None;

                app.scrollbar_drag_active = false;

                return;
            }

            /*
             * Match ui::render_vertical_scrollbar() and Ratatui's proportional
             * placement scale.
             *
             * The rendered scrollbar first maps list_offset onto
             * content_length - 1. Reusing that intermediate scale here prevents
             * the mouse hitbox from sitting one row above the visible one-cell thumb.
             */
            let scrollbar_position = app
                .list_offset
                .saturating_mul(content_length.saturating_sub(1))
                .checked_div(maximum_offset)
                .unwrap_or(0);

            let current_thumb_top = scrollbar_position
                .saturating_mul(track_length)
                .saturating_add(content_length / 2)
                .checked_div(content_length)
                .unwrap_or(0)
                .min(thumb_travel);

            /*
             * Preserve where the selected row currently appears inside the viewport.
             *
             * After a track jump, the selector therefore moves with the viewport rather
             * than unexpectedly snapping to its first or final visible row.
             */
            let selected_viewport_row = app
                .selected
                .saturating_sub(app.list_offset)
                .min(viewport_length.saturating_sub(1));

            /*
             * The track begins one row below the panel's top border.
             */
            let clicked_track_position =
                event.row.saturating_sub(area.y).saturating_sub(1) as usize;

            let clicked_track_position = clicked_track_position.min(track_length.saturating_sub(1));

            let clicked_inside_thumb = clicked_track_position >= current_thumb_top
                && clicked_track_position < current_thumb_top.saturating_add(thumb_length);

            if clicked_inside_thumb {
                /*
                 * Grabbing the thumb starts ordinary proportional dragging.
                 */
                *scrollbar_track_hold = None;

                *scrollbar_drag = Some(ScrollbarDragState {
                    start_mouse_row: event.row,

                    start_thumb_top: current_thumb_top,

                    selected_viewport_row,
                });

                app.scrollbar_drag_active = true;
            } else {
                /*
                 * Clicking the track moves one viewport immediately. Keeping the button
                 * held continues paging toward the pointer after a short initial delay.
                 */
                *scrollbar_drag = None;

                app.scrollbar_drag_active = true;

                app.begin_rapid_navigation();

                let direction = if clicked_track_position < current_thumb_top {
                    app.page_up();

                    ScrollbarTrackDirection::Up
                } else {
                    app.page_down();

                    ScrollbarTrackDirection::Down
                };

                *scrollbar_track_hold = Some(ScrollbarTrackHoldState {
                    direction,

                    target_track_position: clicked_track_position,

                    /*
                     * Preserve an ordinary click as one page. Repetition begins only when
                     * the button remains held for a noticeable moment.
                     */
                    next_repeat: Instant::now() + Duration::from_millis(300),
                });
            }
        }

        /*
         * Continue moving while the left button remains held.
         */
        MouseEventKind::Drag(MouseButton::Left) => {
            let Some(drag) = *scrollbar_drag else {
                return;
            };

            drag_scrollbar(app, event.row, area, drag);
        }

        /*
         * Releasing the mouse ends scrollbar dragging.
         */
        MouseEventKind::Up(MouseButton::Left) => {
            *scrollbar_drag = None;

            *scrollbar_track_hold = None;

            *horizontal_scrollbar_drag = None;

            app.scrollbar_drag_active = false;
        }

        MouseEventKind::Down(MouseButton::Left) => {
            *scrollbar_drag = None;

            *scrollbar_track_hold = None;

            *horizontal_scrollbar_drag = None;

            app.scrollbar_drag_active = false;

            /*
             * The bottom-border Home control must be handled before the ordinary
             * entry-row check because the border is deliberately outside the rows.
             */
            if inside_home_button {
                *last_left_click = None;

                app.enter_home_directory();

                return;
            }

            if !inside_entry_rows {
                *last_left_click = None;

                return;
            }

            /*
             * The top border occupies one row.
             */
            let visible_row = event.row.saturating_sub(area.y).saturating_sub(1) as usize;

            let selected_position = app.list_offset.saturating_add(visible_row);

            app.select_visible_position(selected_position);

            let now = Instant::now();

            let is_double_click =
                last_left_click.is_some_and(|(previous_time, previous_column, previous_row)| {
                    previous_column == event.column
                        && previous_row == event.row
                        && now.duration_since(previous_time) <= Duration::from_millis(400)
                });

            if is_double_click {
                let selected_tree_directory_state = if app.view_mode == ViewMode::Tree {
                    app.tree_row_at_filtered_position(app.selected)
                        .filter(|row| row.entry.is_directory)
                        .map(|row| row.expanded)
                } else {
                    None
                };

                match selected_tree_directory_state {
                    /*
                     * Double-clicking an expanded Tree directory collapses its branch
                     * without changing the active Tree root.
                     */
                    Some(true) => {
                        app.enter_parent_directory();
                    }

                    /*
                     * Double-clicking a closed Tree directory expands its branch
                     * without changing the active Tree root.
                     */
                    Some(false) => {
                        app.enter_selected_directory();
                    }

                    /*
                     * List directories and ordinary files retain their normal
                     * double-click activation.
                     */
                    None => {
                        app.activate_selected();
                    }
                }

                *last_left_click = None;
            } else {
                *last_left_click = Some((now, event.column, event.row));
            }
        }

        _ => {}
    }
}

fn drag_overlay_scrollbar(app: &mut App, mouse_row: u16, area: Rect) {
    let maximum_scroll = if app.legend_visible() {
        app.legend_max_scroll
    } else {
        app.help_max_scroll
    };

    if maximum_scroll == 0 || area.height <= 1 {
        if app.legend_visible() {
            app.legend_scroll = 0;
        } else {
            app.help_scroll = 0;
        }

        return;
    }

    let track_position = mouse_row
        .saturating_sub(area.y)
        .min(area.height.saturating_sub(1)) as usize;

    let track_maximum = area.height.saturating_sub(1) as usize;

    let scroll = track_position * maximum_scroll as usize / track_maximum;

    let scroll = scroll.min(maximum_scroll as usize) as u16;

    if app.legend_visible() {
        app.legend_scroll = scroll;
    } else {
        app.help_scroll = scroll;
    }
}

fn handle_transfer_mouse_event(
    app: &mut App,
    event: MouseEvent,
    regions: Option<ui::TransferUiRegions>,
) {
    let Some(regions) = regions else {
        return;
    };

    if event.kind != MouseEventKind::Down(MouseButton::Left) {
        return;
    }

    let area = regions.action;

    let inside_action = event.column >= area.x
        && event.column < area.x.saturating_add(area.width)
        && event.row >= area.y
        && event.row < area.y.saturating_add(area.height);

    if !inside_action {
        return;
    }

    if app.transfer_finished() {
        app.acknowledge_transfer();
    } else {
        app.request_transfer_cancel();
    }
}

fn handle_connection_mouse_event(
    app: &mut App,
    event: MouseEvent,
    regions: Option<ui::ConnectionUiRegions>,
) {
    let Some(regions) = regions else {
        return;
    };

    if event.kind != MouseEventKind::Down(MouseButton::Left) {
        return;
    }

    let point_inside = |area: Rect| {
        event.column >= area.x
            && event.column < area.x.saturating_add(area.width)
            && event.row >= area.y
            && event.row < area.y.saturating_add(area.height)
    };

    if point_inside(regions.name) {
        app.set_connection_focus(ConnectionField::Name);
    } else if point_inside(regions.host) {
        app.set_connection_focus(ConnectionField::Host);
    } else if point_inside(regions.username) {
        app.set_connection_focus(ConnectionField::Username);
    } else if point_inside(regions.port) {
        app.set_connection_focus(ConnectionField::Port);
    } else if point_inside(regions.identity_file) {
        app.set_connection_focus(ConnectionField::IdentityFile);
    } else if point_inside(regions.start_directory) {
        app.set_connection_focus(ConnectionField::StartDirectory);
    } else if point_inside(regions.connect) {
        if !app.connection_in_progress {
            app.set_connection_focus(ConnectionField::Connect);

            app.begin_connection();
        }
    } else if point_inside(regions.save) {
        app.set_connection_focus(ConnectionField::Save);

        app.save_connection_profile();
    } else if point_inside(regions.delete) {
        if !app.connection_store.profiles().is_empty() {
            app.set_connection_focus(ConnectionField::Delete);

            app.delete_connection_profile();
        }
    } else if point_inside(regions.disconnect) {
        if app.source_is_remote() {
            app.set_connection_focus(ConnectionField::Disconnect);

            app.disconnect_remote();
        }
    } else if point_inside(regions.close) {
        app.close_connection_dialog();
    } else if point_inside(regions.profiles) && !app.connection_store.profiles().is_empty() {
        app.set_connection_focus(ConnectionField::Profiles);
    }
}

fn scrollbar_thumb_length(
    content_length: usize,
    viewport_length: usize,
    track_length: usize,
) -> usize {
    if content_length == 0 || track_length == 0 {
        return 0;
    }

    let numerator = viewport_length
        .saturating_mul(track_length)
        .saturating_add(content_length.saturating_sub(1));

    let thumb_length = numerator / content_length;

    thumb_length.max(1).min(track_length)
}

fn drag_horizontal_scrollbar(
    app: &mut App,
    mouse_column: u16,
    area: Rect,
    drag: HorizontalScrollbarDragState,
) {
    const HORIZONTAL_THUMB_WIDTH: usize = 5;

    let track_length = area.width as usize;

    let thumb_width = HORIZONTAL_THUMB_WIDTH.min(track_length);

    let thumb_travel = track_length.saturating_sub(thumb_width);

    if thumb_travel == 0 || app.horizontal_max_offset == 0 {
        app.horizontal_offset = 0;

        return;
    }

    let mouse_delta = i32::from(mouse_column) - i32::from(drag.start_mouse_column);

    let requested_thumb_left = if mouse_delta < 0 {
        drag.start_thumb_left
            .saturating_sub(mouse_delta.unsigned_abs() as usize)
    } else {
        drag.start_thumb_left.saturating_add(mouse_delta as usize)
    }
    .min(thumb_travel);

    app.horizontal_offset = requested_thumb_left
        .saturating_mul(app.horizontal_max_offset)
        .saturating_add(thumb_travel / 2)
        / thumb_travel;
}

fn drag_scrollbar(
    app: &mut App,
    mouse_row: u16,
    area: ratatui::layout::Rect,
    drag: ScrollbarDragState,
) {
    let content_length = app.current_visible_entry_count();

    let viewport_length = app.viewport_rows;

    let track_length = area.height.saturating_sub(2) as usize;

    if content_length <= viewport_length || track_length == 0 {
        return;
    }

    let thumb_length = scrollbar_thumb_length(content_length, viewport_length, track_length);

    let thumb_travel = track_length.saturating_sub(thumb_length);

    let maximum_offset = content_length.saturating_sub(viewport_length);

    if thumb_travel == 0 || maximum_offset == 0 {
        return;
    }

    let mouse_delta = mouse_row as isize - drag.start_mouse_row as isize;

    /*
     * Move in scrollbar-track coordinates first.
     *
     * One mouse-cell movement therefore moves the thumb by exactly one
     * available track cell, preserving pointer-to-handle synchronization.
     */
    let new_thumb_top =
        (drag.start_thumb_top as isize + mouse_delta).clamp(0, thumb_travel as isize) as usize;

    /*
     * Convert the exact thumb position back into a valid viewport offset.
     *
     * Rounded division avoids the truncation that could leave the handle one
     * cell away from the top or bottom.
     */
    let new_offset = new_thumb_top
        .saturating_mul(maximum_offset)
        .saturating_add(thumb_travel / 2)
        / thumb_travel;

    let new_selected = new_offset
        .saturating_add(drag.selected_viewport_row)
        .min(content_length.saturating_sub(1));

    app.list_offset = new_offset;

    app.selected = new_selected;
}

fn process_scrollbar_track_hold(
    app: &mut App,
    area: ratatui::layout::Rect,
    track_hold: &mut Option<ScrollbarTrackHoldState>,
) -> bool {
    let Some(mut hold) = *track_hold else {
        return false;
    };

    let now = Instant::now();

    if now < hold.next_repeat {
        return false;
    }

    let content_length = app.current_visible_entry_count();

    let viewport_length = app.viewport_rows;

    let track_length = area.height.saturating_sub(2) as usize;

    if content_length <= viewport_length || track_length == 0 {
        *track_hold = None;

        app.scrollbar_drag_active = false;

        return false;
    }

    let thumb_length = scrollbar_thumb_length(content_length, viewport_length, track_length);

    let thumb_travel = track_length.saturating_sub(thumb_length);

    let maximum_offset = content_length.saturating_sub(viewport_length);

    if thumb_travel == 0 || maximum_offset == 0 {
        *track_hold = None;

        app.scrollbar_drag_active = false;

        return false;
    }

    let current_thumb_top = app
        .list_offset
        .saturating_mul(thumb_travel)
        .saturating_add(maximum_offset / 2)
        / maximum_offset;

    let current_thumb_bottom = current_thumb_top.saturating_add(thumb_length.saturating_sub(1));

    /*
     * Stop once the thumb reaches or crosses the pointer.
     */
    let target_reached = match hold.direction {
        ScrollbarTrackDirection::Up => hold.target_track_position >= current_thumb_top,

        ScrollbarTrackDirection::Down => hold.target_track_position <= current_thumb_bottom,
    };

    if target_reached {
        *track_hold = None;

        app.scrollbar_drag_active = false;

        return false;
    }

    app.begin_rapid_navigation();

    match hold.direction {
        ScrollbarTrackDirection::Up => {
            app.page_up();
        }

        ScrollbarTrackDirection::Down => {
            app.page_down();
        }
    }

    /*
     * Roughly fourteen viewport movements per second: considerably faster than
     * repeated manual clicks, but still controlled and visibly gradual.
     */
    hold.next_repeat = now + Duration::from_millis(70);

    *track_hold = Some(hold);

    true
}

fn process_overlay_scrollbar_track_hold(
    app: &mut App,
    area: Option<Rect>,
    track_hold: &mut Option<OverlayScrollbarTrackHoldState>,
) -> bool {
    let Some(mut hold) = *track_hold else {
        return false;
    };

    /*
     * Closing Help/Legend or losing its scrollbar immediately ends the hold.
     */
    if !app.help_visible() && !app.legend_visible() {
        *track_hold = None;

        return false;
    }

    let Some(area) = area else {
        *track_hold = None;

        return false;
    };

    let now = Instant::now();

    if now < hold.next_repeat {
        return false;
    }

    let maximum_scroll = if app.legend_visible() {
        app.legend_max_scroll
    } else {
        app.help_max_scroll
    };

    let viewport_length = area.height as usize;

    let content_length = maximum_scroll as usize + viewport_length;

    let track_length = area.height as usize;

    if maximum_scroll == 0 || track_length == 0 {
        *track_hold = None;

        return false;
    }

    let thumb_length = scrollbar_thumb_length(content_length, viewport_length, track_length);

    let thumb_travel = track_length.saturating_sub(thumb_length);

    if thumb_travel == 0 {
        *track_hold = None;

        return false;
    }

    let current_scroll = if app.legend_visible() {
        app.legend_scroll
    } else {
        app.help_scroll
    } as usize;

    let current_thumb_top = current_scroll
        .saturating_mul(thumb_travel)
        .saturating_add(maximum_scroll as usize / 2)
        / maximum_scroll as usize;

    let current_thumb_bottom = current_thumb_top.saturating_add(thumb_length.saturating_sub(1));

    /*
     * Stop as soon as the thumb reaches or crosses the original click.
     */
    let target_reached = match hold.direction {
        ScrollbarTrackDirection::Up => hold.target_track_position >= current_thumb_top,

        ScrollbarTrackDirection::Down => hold.target_track_position <= current_thumb_bottom,
    };

    if target_reached {
        *track_hold = None;

        return false;
    }

    match hold.direction {
        ScrollbarTrackDirection::Up => {
            if app.legend_visible() {
                app.page_legend_up();
            } else {
                app.page_help_up();
            }
        }

        ScrollbarTrackDirection::Down => {
            if app.legend_visible() {
                app.page_legend_down();
            } else {
                app.page_help_down();
            }
        }
    }

    /*
     * Match the filesystem scrollbar's established repeat cadence.
     */
    hold.next_repeat = now + Duration::from_millis(70);

    *track_hold = Some(hold);

    true
}

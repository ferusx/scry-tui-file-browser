// SPDX-License-Identifier: BSD-3-Clause

mod app;
mod args;
mod classify;
mod clipboard;
mod config;
mod connection;
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

use app::{App, DeletionChoice, EntryFilter, RemoteIndexDialogFocus, ViewMode};
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
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
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
         * Session state overrides the browser/display defaults from scry.toml.
         *
         * Explicit command-line switches are applied afterward and therefore remain
         * the final authority for this launch.
         */
        apply_session_to_startup_config(&mut startup_config, state);
    }

    app.apply_startup_config(&startup_config);

    if let Some(state) = restored_session.as_ref() {
        app.restore_session_state(state);
    }

    if let Some(message) = startup_warning {
        app.show_error_message(message);
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
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
    )?;

    let run_result = ratatui::run(|terminal| run_app(terminal, &mut app));

    let disable_result = execute!(stdout(), PopKeyboardEnhancementFlags, DisableMouseCapture,);

    run_result?;

    disable_result?;

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

fn run_app(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> io::Result<()> {
    let mut ui_regions = ui::UiRegions::default();

    let mut last_left_click: Option<(Instant, u16, u16)> = None;

    let mut scrollbar_drag: Option<ScrollbarDragState> = None;

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

        if event::poll(Duration::from_millis(25))? {
            match event::read()? {
                Event::Key(key_event) => {
                    if key_event.kind != KeyEventKind::Press {
                        continue;
                    }

                    handle_key_event(app, key_event);

                    needs_redraw = true;
                }

                Event::Mouse(mouse_event) => {
                    handle_mouse_event(
                        app,
                        mouse_event,
                        ui_regions,
                        &mut last_left_click,
                        &mut scrollbar_drag,
                        &mut help_scrollbar_drag,
                    );

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

            (KeyCode::Enter, _) => {
                use crate::connection::ConnectionField;

                match app.connection_dialog.focus {
                    ConnectionField::Connect => {
                        app.begin_connection();
                    }

                    ConnectionField::Save => {
                        app.save_connection_profile();
                    }

                    ConnectionField::Disconnect => {
                        app.disconnect_remote();
                    }

                    /*
                     * Connect, Delete, and Disconnect receive their real actions in the
                     * upcoming connection-management stages.
                     */
                    ConnectionField::Delete => {
                        app.delete_connection_profile();
                    }

                    /*
                     * Enter inside an editable field advances to the next enabled control.
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

            (KeyCode::Char('!'), _) | (KeyCode::Esc, _) | (KeyCode::Enter, _) => {
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

            (KeyCode::Home, _) => {
                app.help_scroll = 0;
            }

            (KeyCode::End, _) => {
                app.legend_scroll_to_end();
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

            (KeyCode::Char('?'), _)
            | (KeyCode::F(1), _)
            | (KeyCode::Esc, _)
            | (KeyCode::Enter, _) => {
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
         * Plain Left/Right edit the active query safely.
         *
         * Hold Control to perform structural navigation. This prevents an accidental
         * arrow press while editing a query from leaving the current directory or
         * changing the active Tree branch.
         */
        (KeyCode::Left, modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            app.enter_parent_directory();
        }

        (KeyCode::Right, modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            app.enter_selected_directory();
        }

        (KeyCode::Left, _) => {
            app.move_query_cursor_left();
        }

        (KeyCode::Right, _) => {
            app.move_query_cursor_right();
        }

        (KeyCode::Home, modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            app.move_query_cursor_to_start();
        }

        (KeyCode::End, modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            app.move_query_cursor_to_end();
        }

        (KeyCode::Esc, _) => {
            app.enter_parent_directory();
        }

        (KeyCode::Up, _) => {
            app.move_up();
        }

        (KeyCode::Down, _) => {
            app.move_down();
        }

        (KeyCode::PageUp, _) => {
            app.page_up();
        }

        (KeyCode::PageDown, _) => {
            app.page_down();
        }

        (KeyCode::Home, _) => {
            app.select_first();
        }

        (KeyCode::End, _) => {
            app.select_last();
        }

        (KeyCode::Delete, _) => {
            app.begin_deletion_confirmation();
        }

        /*
         * While browsing through SSH, marked files take precedence over ordinary
         * activation.
         *
         * This applies specifically to keyboard Enter. Mouse double-click continues
         * activating the entry beneath the pointer rather than unexpectedly launching
         * marks collected elsewhere.
         */
        (KeyCode::Enter, KeyModifiers::NONE) => {
            if app.source_is_remote() && app.marked_count() > 0 {
                app.begin_marked_transfer_batch();
            } else {
                app.activate_selected();
            }
        }

        (KeyCode::Char('?'), _) => {
            app.toggle_help();
        }

        (KeyCode::Char('!'), _) => {
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

fn handle_mouse_event(
    app: &mut App,
    event: MouseEvent,
    regions: ui::UiRegions,
    last_left_click: &mut Option<(Instant, u16, u16)>,
    scrollbar_drag: &mut Option<ScrollbarDragState>,
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

    if app.remote_index_setup_visible() {
        /*
         * The setup window owns every mouse event while visible.
         *
         * Events that do not land on one of its choices are deliberately ignored
         * rather than being passed to the browser underneath.
         */
        *scrollbar_drag = None;

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

        *help_scrollbar_drag = false;

        *last_left_click = None;

        /*
         * Deletion confirmation is modal.
         *
         * Mouse events must not select or activate entries behind the popup.
         * Button hit testing will be added separately after keyboard behavior has
         * been verified.
         */
        return;
    }

    if app.transfer_visible() {
        *scrollbar_drag = None;

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

        *last_left_click = None;

        handle_connection_mouse_event(app, event, regions.connection);

        return;
    }

    if app.about_visible() {
        *scrollbar_drag = None;

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

        *last_left_click = None;

        let overlay_scrollbar = regions.help_scrollbar;

        let on_overlay_scrollbar = overlay_scrollbar.is_some_and(|area| {
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

            MouseEventKind::Down(MouseButton::Left) if on_overlay_scrollbar => {
                *help_scrollbar_drag = true;

                drag_overlay_scrollbar(
                    app,
                    event.row,
                    overlay_scrollbar.expect("checked overlay scrollbar region"),
                );
            }

            MouseEventKind::Drag(MouseButton::Left) if *help_scrollbar_drag => {
                if let Some(area) = overlay_scrollbar {
                    drag_overlay_scrollbar(app, event.row, area);
                }
            }

            MouseEventKind::Up(MouseButton::Left) => {
                *help_scrollbar_drag = false;

                *scrollbar_drag = None;

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

    /*
     * The terminal's visible mouse pointer may overlap the scrollbar while its
     * reported cell lies immediately to either side of the rendered column.
     *
     * Test the scrollbar rows independently from inside_entries_panel so the
     * cell immediately to the right of the panel remains a valid grab target.
     */
    let inside_scrollbar_rows =
        event.row > area.y && event.row < area.y.saturating_add(area.height).saturating_sub(1);

    let scrollbar_hit_left = right_edge.saturating_sub(1);

    let scrollbar_hit_right = right_edge.saturating_add(1);

    let on_scrollbar = inside_scrollbar_rows
        && event.column >= scrollbar_hit_left
        && event.column <= scrollbar_hit_right;

    match event.kind {
        MouseEventKind::Down(MouseButton::Left) if on_parent_button => {
            *scrollbar_drag = None;

            *last_left_click = None;

            app.enter_parent_directory();
        }

        MouseEventKind::ScrollUp => {
            app.scroll_selection(-WHEEL_STEP);
        }

        MouseEventKind::ScrollDown => {
            app.scroll_selection(WHEEL_STEP);
        }

        /*
         * Clicking the scrollbar begins a drag immediately and moves the
         * selection to the clicked proportional position.
         */
        MouseEventKind::Down(MouseButton::Left) if on_scrollbar => {
            *last_left_click = None;

            let content_length = app.current_visible_entry_count();

            let viewport_length = app.viewport_rows;

            let track_length = area.height.saturating_sub(2) as usize;

            let thumb_length =
                scrollbar_thumb_length(content_length, viewport_length, track_length);

            let thumb_travel = track_length.saturating_sub(thumb_length);

            let maximum_offset = content_length.saturating_sub(viewport_length);

            /*
             * Convert the current viewport offset into the thumb's actual track position.
             *
             * Rounded division keeps this coordinate consistent with the rendered handle.
             */
            let start_thumb_top = if maximum_offset == 0 || thumb_travel == 0 {
                0
            } else {
                app.list_offset
                    .saturating_mul(thumb_travel)
                    .saturating_add(maximum_offset / 2)
                    / maximum_offset
            };

            let selected_viewport_row = app
                .selected
                .saturating_sub(app.list_offset)
                .min(app.viewport_rows.saturating_sub(1));

            *scrollbar_drag = Some(ScrollbarDragState {
                start_mouse_row: event.row,

                start_thumb_top,

                selected_viewport_row,
            });

            app.scrollbar_drag_active = true;
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

            app.scrollbar_drag_active = false;
        }

        MouseEventKind::Down(MouseButton::Left) => {
            *scrollbar_drag = None;

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
                app.activate_selected();

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

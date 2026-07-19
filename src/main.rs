// SPDX-License-Identifier: BSD-3-Clause

mod app;
mod args;
mod classify;
mod config;
mod connection;
mod entry;
mod fuzzy;
mod help;
mod legend;
mod open;
mod query;
mod remote_index;
mod scan;
mod search_index;
mod source;
mod ssh;
mod themes;
mod ui;

use app::{App, DeletionChoice, RemoteIndexDialogFocus, ViewMode};
use args::Cli;
use clap::Parser;
use connection::ConnectionField;
use ratatui::layout::Rect;
use ssh::{SftpSource, SshTarget};
use std::io::{self, stdout};
use std::time::{Duration, Instant};

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
};

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    let config = config::ScryConfig::load();

    if cli.help {
        legend::print_help(config.features.enable_deletion)?;

        return Ok(());
    }

    let mut app = if let Some(value) = cli.ssh.as_deref() {
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

        /*
         * Remote startup rules:
         *
         *     no PATH    remote home directory
         *     .          remote home directory
         *     PATH       PATH exactly as supplied
         */
        let remote_start = match cli.path.as_deref() {
            None => remote_home,

            Some(path) if path == std::path::Path::new(".") => remote_home,

            Some(path) => path.to_path_buf(),
        };

        match App::with_source(remote_start.clone(), Box::new(source)) {
            Ok(app) => app,

            Err(error) => {
                eprintln!(
                    "scry: unable to open remote starting directory {}: {}",
                    remote_start.display(),
                    error,
                );

                std::process::exit(1);
            }
        }
    } else {
        /*
         * An omitted local PATH has the ordinary shell meaning: begin in the
         * process's current directory.
         */
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

    app.apply_startup_config(&config);
    /*
     * Command-line switches override configuration values.
     *
     * Existing positive Boolean switches can force a feature on, but must never
     * toggle an already-enabled configuration value back off.
     */
    if cli.all && !app.show_hidden {
        app.toggle_hidden();
    }

    if cli.recursive && !app.recursive_mode {
        app.enable_recursive_mode();
    }

    if cli.tree && app.view_mode != ViewMode::Tree {
        app.toggle_tree_mode();
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

    execute!(
        stdout(),
        EnableMouseCapture,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
    )?;

    let run_result = ratatui::run(|mut terminal| run_app(&mut terminal, &mut app));

    let disable_result = execute!(stdout(), PopKeyboardEnhancementFlags, DisableMouseCapture,);

    run_result?;

    disable_result?;

    if let Some(path) = app.opened_file_path {
        let containing_directory = path
            .parent()
            .map(|parent| parent.display().to_string())
            .unwrap_or_else(|| ".".to_string());

        let filename = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());

        println!("{} - {}", containing_directory, filename,);
    }

    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct ScrollbarDragState {
    start_mouse_row: u16,

    start_selected: usize,
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

fn handle_key_event(app: &mut App, key_event: KeyEvent) {
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

            (KeyCode::Char('!'), _) | (KeyCode::Esc, _) | (KeyCode::Enter, _) => {
                app.close_legend();
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

            (KeyCode::Char('?'), _) | (KeyCode::Esc, _) | (KeyCode::Enter, _) => {
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
         * Backspace may be reported either as KeyCode::Backspace or as Ctrl+H,
         * depending on the terminal and keyboard-enhancement support.
         */
        (KeyCode::Backspace, _) | (KeyCode::Char('h'), KeyModifiers::CONTROL) => {
            if app.query.is_empty() {
                app.enter_parent_directory();
            } else {
                app.pop_query_character();
            }
        }

        /*
         * Ctrl+M is the carriage-return control code and may be reported as Enter by
         * the terminal. Never allow it to activate a directory or file.
         */
        (KeyCode::Enter, modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {}

        /*
         * Ctrl+M must never activate anything.
         */
        (KeyCode::Char('m'), KeyModifiers::CONTROL) => {}

        (KeyCode::Char('h'), KeyModifiers::ALT) => {
            app.toggle_hidden();
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
         * Query-caret movement uses Ctrl so ordinary navigation keys
         * remain available to the filesystem browser and Tree mode.
         */
        (KeyCode::Left, KeyModifiers::CONTROL) => {
            app.move_query_cursor_left();
        }

        (KeyCode::Right, KeyModifiers::CONTROL) => {
            app.move_query_cursor_right();
        }

        (KeyCode::Home, KeyModifiers::CONTROL) => {
            app.move_query_cursor_to_start();
        }

        (KeyCode::End, KeyModifiers::CONTROL) => {
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

        (KeyCode::Left, _) => {
            app.enter_parent_directory();
        }

        (KeyCode::Right, _) => {
            app.enter_selected_directory();
        }

        (KeyCode::Delete, _) => {
            app.begin_deletion_confirmation();
        }

        (KeyCode::Enter, KeyModifiers::NONE) => {
            if !app.commit_pending_query_modifier() {
                app.activate_selected();
            }
        }

        (KeyCode::Char('?'), _) => {
            app.toggle_help();
        }

        (KeyCode::Char('!'), _) => {
            app.toggle_legend();
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

    if app.help_visible() {
        *scrollbar_drag = None;

        *last_left_click = None;

        let help_scrollbar = regions.help_scrollbar;

        let on_help_scrollbar = help_scrollbar.is_some_and(|area| {
            event.column >= area.x
                && event.column < area.x.saturating_add(area.width)
                && event.row >= area.y
                && event.row < area.y.saturating_add(area.height)
        });

        match event.kind {
            MouseEventKind::ScrollUp => {
                app.scroll_help_up();
            }

            MouseEventKind::ScrollDown => {
                app.scroll_help_down();
            }

            MouseEventKind::Down(MouseButton::Left) if on_help_scrollbar => {
                *help_scrollbar_drag = true;

                drag_help_scrollbar(
                    app,
                    event.row,
                    help_scrollbar.expect("checked help scrollbar region"),
                );
            }

            MouseEventKind::Drag(MouseButton::Left) if *help_scrollbar_drag => {
                if let Some(area) = help_scrollbar {
                    drag_help_scrollbar(app, event.row, area);
                }
            }

            MouseEventKind::Up(MouseButton::Left) => {
                *help_scrollbar_drag = false;
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

    let on_scrollbar = inside_entry_rows && event.column == right_edge;

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

            *scrollbar_drag = Some(ScrollbarDragState {
                start_mouse_row: event.row,

                start_selected: app.selected,
            });
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
        }

        MouseEventKind::Down(MouseButton::Left) => {
            *scrollbar_drag = None;

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

fn drag_help_scrollbar(app: &mut App, mouse_row: u16, area: Rect) {
    if app.help_max_scroll == 0 || area.height <= 1 {
        app.help_scroll = 0;

        return;
    }

    let track_position = mouse_row
        .saturating_sub(area.y)
        .min(area.height.saturating_sub(1)) as usize;

    let track_maximum = area.height.saturating_sub(1) as usize;

    let scroll = track_position * app.help_max_scroll as usize / track_maximum;

    app.help_scroll = scroll.min(app.help_max_scroll as usize) as u16;
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

    /*
     * The thumb itself occupies part of the track, so this is the actual
     * distance through which its top edge can travel.
     */
    let thumb_travel = track_length.saturating_sub(thumb_length);

    let selection_travel = content_length.saturating_sub(1);

    if thumb_travel == 0 || selection_travel == 0 {
        return;
    }

    let mouse_delta = mouse_row as isize - drag.start_mouse_row as isize;

    let selection_delta =
        mouse_delta.saturating_mul(selection_travel as isize) / thumb_travel as isize;

    let new_position = drag.start_selected as isize + selection_delta;

    let new_position = new_position.clamp(0, selection_travel as isize) as usize;

    app.select_visible_position(new_position);
}

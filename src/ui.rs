// SPDX-License-Identifier: BSD-3-Clause

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState,
    },
};
use std::time::{Duration, SystemTime};

use crate::app::{
    App, DeletionChoice, EXACT_TREE_MATCH_LIMIT, RemoteIndexDialogFocus, SearchMode, TreeRow,
    ViewMode,
};
use crate::connection::ConnectionField;
use crate::fuzzy::fuzzy_highlight_positions;
use crate::help;
use crate::query::{
    QUERY_SYNTAX_REFERENCE, QUERY_TYPE_REFERENCES, QueryHighlightTerm, parse_query,
};
use crate::scan::FileEntry;
use crate::themes::Theme;

// COLOR CONSTANTS
const COLOR_FRAME: Color = Color::Rgb(160, 110, 220);

const COLOR_DIRECTORY: Color = Color::Rgb(80, 155, 235);

const COLOR_FILE: Color = Color::Rgb(195, 200, 210);

const COLOR_SYMLINK: Color = Color::Rgb(75, 195, 210);

// const COLOR_QUERY: Color = Color::Rgb(110, 220, 225);

// const COLOR_SELECTED_BACKGROUND: Color = Color::Rgb(55, 40, 75);

const COLOR_MUTED: Color = Color::Rgb(95, 105, 120);

const COLOR_ERROR: Color = Color::Rgb(220, 55, 70);

const COLOR_QUERY: Color = Color::Rgb(110, 220, 225);

const COLOR_MATCH: Color = Color::Rgb(166, 119, 199);

// const COLOR_PERMISSIONS: Color = COLOR_FRAME; // Color::Rgb(255, 255, 255);

const COLOR_DATE: Color = COLOR_DIRECTORY; // Color::Rgb(160, 110, 220);

const COLOR_USER: Color = Color::Rgb(91, 93, 99); //rgb(91, 93, 99)

const COLOR_SIZE: Color = COLOR_QUERY;

/*
 * Permissions and modification dates use fixed-width formats:
 *
 *     .rwxr-xr-x
 *     2026-07-16 10:42
 */
const PERMISSIONS_COLUMN_WIDTH: u16 = 10;

const DATE_COLUMN_WIDTH: u16 = 16;

/*
 * Special clickable area in the main listing frame
 * for going up one entry in the hierarchy.
 *
*/
const PARENT_BUTTON_LEFT_BRACKET: &str = "[ ";

const PARENT_BUTTON_TEXT: &str = "← go back";

const PARENT_BUTTON_RIGHT_BRACKET: &str = " ]";

const HOME_BUTTON_LEFT_BRACKET: &str = "[ ";

const HOME_BUTTON_TEXT: &str = "↑ go home";

const HOME_BUTTON_RIGHT_BRACKET: &str = " ]";

/*
 * Size and owner widths adapt to the current result set.
 *
 * The limits prevent unusually long values from consuming the filesystem
 * panel.
 */
const SIZE_COLUMN_MIN_WIDTH: u16 = 4;

const SIZE_COLUMN_MAX_WIDTH: u16 = 10;

const USER_COLUMN_MIN_WIDTH: u16 = 4;

const USER_COLUMN_MAX_WIDTH: u16 = 16;

const METADATA_COLUMN_GAP: u16 = 2;

#[derive(Debug, Clone, Copy)]
struct MetadataWidths {
    permissions: u16,

    size: u16,

    date: u16,

    user: u16,
}

impl Default for MetadataWidths {
    fn default() -> Self {
        Self {
            permissions: PERMISSIONS_COLUMN_WIDTH,

            size: SIZE_COLUMN_MIN_WIDTH,

            date: DATE_COLUMN_WIDTH,

            user: USER_COLUMN_MIN_WIDTH,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TransferUiRegions {
    pub action: Rect,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ConnectionUiRegions {
    pub profiles: Rect,

    pub name: Rect,

    pub host: Rect,

    pub username: Rect,

    pub port: Rect,

    pub identity_file: Rect,

    pub start_directory: Rect,

    pub connect: Rect,

    pub save: Rect,

    pub delete: Rect,

    pub disconnect: Rect,

    pub close: Rect,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct UiRegions {
    pub entries: Rect,

    pub parent_button: Rect,

    pub home_button: Rect,

    pub file_info_close: Option<Rect>,

    pub help_scrollbar: Option<Rect>,

    pub connection: Option<ConnectionUiRegions>,

    pub transfer: Option<TransferUiRegions>,

    pub remote_index_setup: Option<RemoteIndexSetupUiRegions>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RemoteIndexSetupUiRegions {
    pub standard: Rect,

    pub include_hidden: Rect,

    pub ok: Rect,

    pub cancel: Rect,
}

pub fn render(frame: &mut Frame, app: &mut App) -> UiRegions {
    let mut help_scrollbar_region = None;

    let mut connection_regions = None;

    let mut transfer_regions = None;

    let mut file_info_close_region = None;

    let mut remote_index_setup_regions = None;

    let (search_area, details_area, entries_area, selection_area, footer_area) =
        match (app.show_details, app.show_selection) {
            /*
             * Both optional panels visible.
             */
            (true, true) => {
                let areas = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3),
                        Constraint::Length(5),
                        Constraint::Min(6),
                        Constraint::Length(3),
                        Constraint::Length(1),
                    ])
                    .split(frame.area());

                (areas[0], Some(areas[1]), areas[2], Some(areas[3]), areas[4])
            }

            /*
             * Details visible, Selection hidden.
             */
            (true, false) => {
                let areas = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3),
                        Constraint::Length(5),
                        Constraint::Min(6),
                        Constraint::Length(1),
                    ])
                    .split(frame.area());

                (areas[0], Some(areas[1]), areas[2], None, areas[3])
            }

            /*
             * Details hidden, Selection visible.
             */
            (false, true) => {
                let areas = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3),
                        Constraint::Min(6),
                        Constraint::Length(3),
                        Constraint::Length(1),
                    ])
                    .split(frame.area());

                (areas[0], None, areas[1], Some(areas[2]), areas[3])
            }

            /*
             * Both optional panels hidden.
             */
            (false, false) => {
                let areas = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3),
                        Constraint::Min(6),
                        Constraint::Length(1),
                    ])
                    .split(frame.area());

                (areas[0], None, areas[1], None, areas[2])
            }
        };

    render_search(frame, app, search_area);

    if let Some(area) = details_area {
        render_details(frame, app, area);
    }

    let entries_area = render_entries(frame, app, entries_area);

    let parent_button = Rect {
        /*
         * A Block title begins immediately after the top-left border.
         */
        x: entries_area.x.saturating_add(1),

        y: entries_area.y,

        width: (PARENT_BUTTON_LEFT_BRACKET.chars().count()
            + PARENT_BUTTON_TEXT.chars().count()
            + PARENT_BUTTON_RIGHT_BRACKET.chars().count()) as u16,

        height: 1,
    };

    let home_button = Rect {
        /*
         * The home control occupies the lower-left border of the filesystem panel.
         */
        x: entries_area.x.saturating_add(1),

        y: entries_area
            .y
            .saturating_add(entries_area.height.saturating_sub(1)),

        width: (HOME_BUTTON_LEFT_BRACKET.chars().count()
            + HOME_BUTTON_TEXT.chars().count()
            + HOME_BUTTON_RIGHT_BRACKET.chars().count()) as u16,

        height: 1,
    };

    if let Some(area) = selection_area {
        render_selection(frame, app, area);
    }

    render_footer(frame, app, footer_area);

    if app.help_visible() {
        help_scrollbar_region = render_help_overlay(frame, app, frame.area());
    }

    if app.legend_visible() {
        help_scrollbar_region = render_legend_overlay(frame, app, frame.area());
    }

    if app.about_visible() {
        render_about_overlay(frame, app, frame.area());
    }

    if app.remote_index_setup_visible() {
        remote_index_setup_regions =
            Some(render_remote_index_setup_overlay(frame, app, frame.area()));
    }

    if app.connection_visible() {
        connection_regions = Some(render_connection_overlay(frame, app, frame.area()));
    }

    if app.transfer_visible() {
        transfer_regions = Some(render_transfer_overlay(frame, app, frame.area()));
    }

    if app.deletion_visible() {
        render_deletion_overlay(frame, app, frame.area());
    }

    /*
     * File Information is rendered last because it is a fully modal inspection
     * window and must remain above every ordinary browser panel.
     */
    if app.file_info_visible() {
        file_info_close_region = render_file_info_overlay(frame, app, frame.area());
    }

    UiRegions {
        entries: entries_area,

        parent_button,

        home_button,

        help_scrollbar: help_scrollbar_region,

        connection: connection_regions,

        transfer: transfer_regions,

        remote_index_setup: remote_index_setup_regions,

        file_info_close: file_info_close_region,
    }
}

fn render_search(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let theme = &app.theme;

    let title = format!(
        " Scry — {} — {} ",
        app.source_label(),
        app.current_directory.display(),
    );

    let mode_label = match (
        app.search_mode,
        app.recursive_mode,
        app.sort_descending,
    ) {
        (SearchMode::Exact, false, false) => "Exact",

        (SearchMode::Exact, false, true) => "Exact+Reverse",

        (SearchMode::Fuzzy, false, false) => "Fuzzy",

        (SearchMode::Fuzzy, false, true) => "Fuzzy+Reverse",

        (SearchMode::Exact, true, false) => "Recursive",

        (SearchMode::Exact, true, true) => "Recursive+Reverse",

        (SearchMode::Fuzzy, true, false) => "Fuzzy+Recursive",

        (SearchMode::Fuzzy, true, true) => "Fuzzy+Recursive+Reverse",
    };

    let placeholder = match (app.search_mode, app.recursive_mode) {
        (SearchMode::Exact, false) => {
            r#"type to filter — e.g. "hello", "ext:rs", "type:source""#
        }

        (SearchMode::Fuzzy, false) => {
            r#"type to search fuzzily — e.g. "help", "hlep", "-java""#
        }

        (SearchMode::Exact, true) => {
            r#"type to filter recursively — e.g. "config", "type:dir", "+rust""#
        }

        (SearchMode::Fuzzy, true) => {
            r#"type to search recursively — e.g. "index", "ext:rs", "rust AND test""#
        }
    };

    let emphasized_mode =
        app.search_mode == SearchMode::Fuzzy
            || app.recursive_mode
            || app.sort_descending;

    let mode_color = if emphasized_mode {
        theme.ui.query
    } else {
        theme.ui.muted
    };

    let search = Paragraph::new(Line::from(vec![
        Span::styled("Search [", Style::default().fg(theme.ui.muted)),
        Span::styled(
            mode_label,
            Style::default()
                .fg(mode_color)
                .add_modifier(if emphasized_mode {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ),
        Span::styled("]: ", Style::default().fg(theme.ui.muted)),
        Span::styled(
            if app.query.is_empty() {
                placeholder
            } else {
                &app.query
            },
            Style::default().fg(theme.ui.query),
        ),
    ]))
    .block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.frames.search)),
    );

    frame.render_widget(search, area);

    if area.width > 2 && area.height > 2 {
        let mode_prefix = format!("Search [{}]: ", mode_label);

        let mut cursor = app.query_cursor.min(app.query.len());

        while !app.query.is_char_boundary(cursor) {
            cursor = cursor.saturating_sub(1);
        }

        let query_prefix = &app.query[..cursor];

        let cursor_column = mode_prefix
            .chars()
            .count()
            .saturating_add(query_prefix.chars().count());

        let inner_width = area.width.saturating_sub(2) as usize;

        if cursor_column < inner_width {
            frame.set_cursor_position((
                area.x
                    .saturating_add(1)
                    .saturating_add(cursor_column as u16),
                area.y.saturating_add(1),
            ));
        }
    }
}

fn render_details(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme;

    let block = Block::default()
        .title(" Details ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.frames.details));

    let inner = block.inner(area);

    frame.render_widget(block, area);

    /*
     * Resolve the classification before cloning the selected entry because
     * selected_classification() may update Scry's inspection cache.
     */
    let classification = app
        .selected_classification()
        .map(|class| class.label().to_string());

    let Some(entry) = app.selected_entry().cloned() else {
        frame.render_widget(
            Paragraph::new(Line::styled(
                " No entry selected",
                Style::default().fg(theme.ui.muted),
            )),
            inner,
        );

        return;
    };

    let classification = classification.unwrap_or_else(|| entry.class.label().to_string());

    let owner = app.owner_name(entry.owner_id);

    let size = if entry.is_directory {
        "—".to_string()
    } else {
        format_file_size(entry.size_bytes)
    };

    let age = format_file_age(entry.modified_time);

    let name_color = if entry.is_directory {
        theme.ui.directory
    } else if entry.is_symlink {
        theme.ui.symlink
    } else {
        theme.ui.file
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    /*
     * Row one:
     *
     * Name | Type | Size
     */
    let first_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(42),
            Constraint::Percentage(29),
            Constraint::Percentage(29),
        ])
        .split(rows[0]);

    render_detail_name(
        frame,
        first_row[0],
        &entry,
        app.show_icons,
        name_color,
        &theme,
    );

    render_detail_value(
        frame,
        first_row[1],
        "Type",
        classification,
        theme.ui.classification,
        &theme,
    );

    render_detail_value(frame, first_row[2], "Size", size, theme.ui.size, &theme);

    /*
     * Row two:
     *
     * Modified | Age | Owner
     */
    let second_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(42),
            Constraint::Percentage(29),
            Constraint::Percentage(29),
        ])
        .split(rows[1]);

    render_detail_value(
        frame,
        second_row[0],
        "Modified",
        entry.modified.clone(),
        theme.ui.date,
        &theme,
    );

    render_detail_value(frame, second_row[1], "Age", age, theme.ui.query, &theme);

    render_detail_value(frame, second_row[2], "Owner", owner, theme.ui.user, &theme);

    /*
     * Row three:
     *
     * Permissions | full path
     */
    let third_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(rows[2]);

    render_detail_permissions(frame, third_row[0], &entry.permissions, &theme);

    render_detail_value(
        frame,
        third_row[1],
        "Path",
        entry.path.display().to_string(),
        name_color,
        &theme,
    );
}

fn render_detail_name(
    frame: &mut Frame,
    area: Rect,
    entry: &FileEntry,
    show_icons: bool,
    name_color: Color,
    theme: &Theme,
) {
    let mut spans = vec![Span::styled(" Name: ", Style::default().fg(theme.ui.muted))];

    if show_icons {
        spans.push(Span::styled(
            format!("{} ", file_icon(entry)),
            Style::default().fg(file_icon_color(entry, theme)),
        ));
    }

    spans.push(Span::styled(
        entry.name.clone(),
        Style::default().fg(name_color),
    ));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_detail_value(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    value: String,
    value_color: Color,
    theme: &Theme,
) {
    let line = Line::from(vec![
        Span::styled(format!(" {}: ", label), Style::default().fg(theme.ui.muted)),
        Span::styled(value, Style::default().fg(value_color)),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}

fn render_detail_permissions(frame: &mut Frame, area: Rect, permissions: &str, theme: &Theme) {
    let mut spans = vec![Span::styled(
        " Permissions: ",
        Style::default().fg(theme.ui.muted),
    )];

    /*
     * Reuse the same per-character permission palette as the metadata column.
     */
    spans.extend(permission_spans(permissions, theme));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn format_file_age(modified_time: Option<SystemTime>) -> String {
    let Some(modified_time) = modified_time else {
        return "—".to_string();
    };

    match SystemTime::now().duration_since(modified_time) {
        Ok(age) => format_past_age(age),

        Err(error) => {
            format!("in {}", format_age_duration(error.duration(),),)
        }
    }
}

fn format_past_age(age: Duration) -> String {
    if age.as_secs() < 5 {
        "just now".to_string()
    } else {
        format_age_duration(age)
    }
}

fn format_age_duration(duration: Duration) -> String {
    const MINUTE: u64 = 60;

    const HOUR: u64 = 60 * MINUTE;

    const DAY: u64 = 24 * HOUR;

    const MONTH: u64 = 30 * DAY;

    const YEAR: u64 = 365 * DAY;

    let seconds = duration.as_secs();

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

fn render_entries(frame: &mut Frame, app: &mut App, area: Rect) -> Rect {
    if metadata_visible(app) {
        /*
         * Calculate the widths once and pass the same values to the panel,
         * heading, and rows.
         */
        let widths = metadata_widths(app);

        let areas = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(metadata_panel_width(app, widths)),
                Constraint::Min(20),
            ])
            .split(area);

        render_metadata(frame, app, areas[0], widths);

        render_filesystem_entries(frame, app, areas[1]);

        areas[1]
    } else {
        render_filesystem_entries(frame, app, area);

        area
    }
}

fn metadata_visible(app: &App) -> bool {
    app.show_columns && (app.show_permissions || app.show_size || app.show_date || app.show_user)
}

fn metadata_widths(app: &mut App) -> MetadataWidths {
    let mut widths = MetadataWidths::default();

    /*
     * Collect the small pieces needed for width calculation first.
     *
     * owner_name() mutably accesses Scry's UID cache, so we cannot retain an
     * immutable entry borrow while resolving the displayed owner.
     */
    let metadata: Vec<(u64, bool, u32)> = match app.view_mode {
        ViewMode::List => (0..app.filtered_indices.len())
            .filter_map(|position| {
                app.entry_at_filtered_position(position)
                    .map(|entry| (entry.size_bytes, entry.is_directory, entry.owner_id))
            })
            .collect(),

        ViewMode::Tree => (0..app.filtered_tree_indices.len())
            .filter_map(|position| {
                app.tree_row_at_filtered_position(position).map(|row| {
                    (
                        row.entry.size_bytes,
                        row.entry.is_directory,
                        row.entry.owner_id,
                    )
                })
            })
            .collect(),
    };

    if app.show_size {
        for (size_bytes, is_directory, _) in &metadata {
            let displayed_size = if *is_directory {
                "—".to_string()
            } else {
                format_file_size(*size_bytes)
            };

            widths.size = widths.size.max(displayed_size.chars().count() as u16);
        }

        widths.size = widths
            .size
            .clamp(SIZE_COLUMN_MIN_WIDTH, SIZE_COLUMN_MAX_WIDTH);
    }

    if app.show_user {
        /*
         * Avoid repeatedly resolving the same owner when a directory contains
         * many entries owned by one account.
         */
        let mut owner_ids: Vec<u32> = metadata.iter().map(|(_, _, owner_id)| *owner_id).collect();

        owner_ids.sort_unstable();

        owner_ids.dedup();

        for owner_id in owner_ids {
            let owner = app.owner_name(owner_id);

            widths.user = widths.user.max(owner.chars().count() as u16);
        }

        widths.user = widths
            .user
            .clamp(USER_COLUMN_MIN_WIDTH, USER_COLUMN_MAX_WIDTH);
    }

    widths
}

fn metadata_content_width(app: &App, widths: MetadataWidths) -> u16 {
    let mut width = 0;

    let mut column_count = 0;

    if app.show_permissions {
        width += widths.permissions;

        column_count += 1;
    }

    if app.show_size {
        width += widths.size;

        column_count += 1;
    }

    if app.show_date {
        width += widths.date;

        column_count += 1;
    }

    if app.show_user {
        width += widths.user;

        column_count += 1;
    }

    if column_count > 1 {
        width += METADATA_COLUMN_GAP * (column_count - 1);
    }

    width
}

fn metadata_panel_width(app: &App, widths: MetadataWidths) -> u16 {
    /*
     * The List reserves two cells for its selection symbol:
     *
     *     ▶
     *
     * Each metadata row also begins with one padding cell. Together with
     * the two surrounding border cells, the panel therefore needs five
     * cells beyond the calculated metadata content width.
     */
    metadata_content_width(app, widths) + 6
}

fn render_filesystem_entries(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    match app.view_mode {
        ViewMode::List => {
            render_list_entries(frame, app, area);
        }

        ViewMode::Tree => {
            render_tree_entries(frame, app, area);
        }
    }
}

fn render_metadata(
    frame: &mut Frame,
    app: &mut App,
    area: ratatui::layout::Rect,
    widths: MetadataWidths,
) {
    let visible_rows = area.height.saturating_sub(2) as usize;

    app.ensure_selection_visible(visible_rows);

    let entry_count = match app.view_mode {
        ViewMode::List => app.filtered_indices.len(),

        ViewMode::Tree => app.filtered_tree_indices.len(),
    };

    let window_start = app.list_offset;

    let window_end = window_start.saturating_add(visible_rows).min(entry_count);

    let mut items = Vec::new();

    for position in window_start..window_end {
        /*
         * Clone only the small pieces needed for rendering so that the
         * immutable entry borrow ends before owner_name() mutably accesses
         * Scry's UID cache.
         */
        let metadata = match app.view_mode {
            ViewMode::List => app.entry_at_filtered_position(position).map(|entry| {
                (
                    entry.permissions.clone(),
                    entry.size_bytes,
                    entry.is_directory,
                    entry.modified.clone(),
                    entry.owner_id,
                )
            }),

            ViewMode::Tree => app.tree_row_at_filtered_position(position).map(|row| {
                (
                    row.entry.permissions.clone(),
                    row.entry.size_bytes,
                    row.entry.is_directory,
                    row.entry.modified.clone(),
                    row.entry.owner_id,
                )
            }),
        };

        let Some((permissions, size_bytes, is_directory, modified, owner_id)) = metadata else {
            continue;
        };

        let owner = if app.show_user {
            Some(app.owner_name(owner_id))
        } else {
            None
        };

        items.push(metadata_list_item(
            app,
            &permissions,
            size_bytes,
            is_directory,
            &modified,
            owner.as_deref(),
            widths,
        ));
    }

    let title = metadata_title(app, widths);

    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.frames.entries)),
        )
        .highlight_symbol("▶")
        .highlight_style(
            Style::default()
                .fg(app.theme.selection.text)
                .bg(app.theme.selection.background)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = ListState::default();

    if entry_count > 0 {
        state.select(Some(app.selected.saturating_sub(window_start)));
    }

    frame.render_stateful_widget(list, area, &mut state);
}

fn metadata_title(app: &App, widths: MetadataWidths) -> String {
    let mut columns = Vec::new();

    if app.show_permissions {
        columns.push(format!(
            "{:<width$}",
            "Permissions",
            width = widths.permissions as usize,
        ));
    }

    if app.show_size {
        columns.push(format!("{:>width$}", "Size", width = widths.size as usize,));
    }

    if app.show_date {
        columns.push(format!("{:<width$}", "Date", width = widths.date as usize,));
    }

    if app.show_user {
        columns.push(format!("{:<width$}", "User", width = widths.user as usize,));
    }

    format!(" {} ", columns.join("  "),)
}

fn metadata_list_item(
    app: &App,
    permissions: &str,
    size_bytes: u64,
    is_directory: bool,
    modified: &str,
    owner: Option<&str>,
    widths: MetadataWidths,
) -> ListItem<'static> {
    let mut spans = vec![Span::raw(" ")];

    let mut needs_gap = false;

    if app.show_permissions {
        spans.extend(permission_spans(permissions, &app.theme));

        let permission_length = permissions.chars().count();

        let permission_width = widths.permissions as usize;

        if permission_length < permission_width {
            spans.push(Span::raw(" ".repeat(permission_width - permission_length)));
        }

        needs_gap = true;
    }

    if app.show_size {
        if needs_gap {
            spans.push(Span::raw(" ".repeat(METADATA_COLUMN_GAP as usize)));
        }

        let size = if is_directory {
            "—".to_string()
        } else {
            format_file_size(size_bytes)
        };

        spans.push(Span::styled(
            format!("{:>width$}", size, width = widths.size as usize,),
            Style::default().fg(if is_directory {
                COLOR_MUTED
            } else {
                COLOR_SIZE
            }),
        ));

        needs_gap = true;
    }

    if app.show_date {
        if needs_gap {
            spans.push(Span::raw(" ".repeat(METADATA_COLUMN_GAP as usize)));
        }

        spans.push(Span::styled(
            format!("{:<width$}", modified, width = widths.date as usize,),
            Style::default().fg(COLOR_DATE),
        ));

        needs_gap = true;
    }

    if app.show_user {
        if needs_gap {
            spans.push(Span::raw(" ".repeat(METADATA_COLUMN_GAP as usize)));
        }

        let owner = owner.unwrap_or("—");

        spans.push(Span::styled(
            truncate_and_pad(owner, widths.user as usize),
            Style::default().fg(COLOR_USER),
        ));
    }

    ListItem::new(Line::from(spans))
}

fn permission_spans(permissions: &str, theme: &Theme) -> Vec<Span<'static>> {
    permissions
        .chars()
        .enumerate()
        .map(|(index, character)| {
            let style = if index == 0 {
                /*
                 * File-type character:
                 *
                 * d l . b c p s
                 */
                Style::default().fg(theme.permissions.file_type)
            } else {
                match character {
                    'r' => Style::default().fg(theme.permissions.read),

                    'w' => Style::default().fg(theme.permissions.write),

                    'x' => Style::default().fg(theme.permissions.execute),

                    's' | 'S' | 't' | 'T' => Style::default()
                        .fg(theme.permissions.special)
                        .add_modifier(Modifier::BOLD),

                    '-' => Style::default().fg(theme.permissions.missing),

                    _ => Style::default().fg(theme.permissions.file_type),
                }
            };

            Span::styled(character.to_string(), style)
        })
        .collect()
}

fn truncate_and_pad(value: &str, width: usize) -> String {
    let mut result: String = value.chars().take(width).collect();

    let current_width = result.chars().count();

    if current_width < width {
        result.push_str(&" ".repeat(width - current_width));
    }

    result
}

fn format_file_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    const TIB: f64 = GIB * 1024.0;

    let bytes_as_float = bytes as f64;

    if bytes < 1024 {
        format!("{} B", bytes)
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

fn entries_title_with_parent_button(title: String, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            PARENT_BUTTON_LEFT_BRACKET,
            Style::default().fg(theme.frames.parent_brackets),
        ),
        Span::styled(
            PARENT_BUTTON_TEXT,
            Style::default()
                .fg(theme.frames.parent_text)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            PARENT_BUTTON_RIGHT_BRACKET,
            Style::default().fg(theme.frames.parent_brackets),
        ),
        Span::raw(title),
    ])
}

fn entries_title_with_home_button(theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            HOME_BUTTON_LEFT_BRACKET,
            Style::default().fg(theme.frames.parent_brackets),
        ),
        Span::styled(
            HOME_BUTTON_TEXT,
            Style::default()
                .fg(theme.frames.parent_text)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            HOME_BUTTON_RIGHT_BRACKET,
            Style::default().fg(theme.frames.parent_brackets),
        ),
    ])
}

fn render_list_entries(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    /*
     * Two rows belong to the surrounding border.
     */
    let visible_rows = area.height.saturating_sub(2) as usize;

    app.viewport_rows = visible_rows.max(1);

    app.ensure_selection_visible(visible_rows);

    let window_start = app.list_offset;

    let window_end = window_start
        .saturating_add(visible_rows)
        .min(app.filtered_indices.len());

    /*
     * Query modifiers affect eligibility but are not filename/path text.
     *
     * For:
     *
     *     type:source index -java
     *
     * only "index" should be highlighted.
     */
    let parsed_query = parse_query(&app.query);

    let highlight_terms = parsed_query.highlight_terms();

    /*
     * The ordinary fuzzy search text retains scattered-character highlighting.
     *
     * Positive +terms and Boolean operands are exact eligibility conditions and
     * therefore receive ordinary substring highlighting.
     */
    let fuzzy_highlight_query = if app.search_mode == SearchMode::Fuzzy
        && !parsed_query.search_text().is_empty()
        && parsed_query.search_text() != "."
    {
        Some(parsed_query.search_text())
    } else {
        None
    };

    let mut items: Vec<ListItem> = Vec::new();

    for position in window_start..window_end {
        /*
         * Clone the entry before asking App whether the directory has content.
         *
         * directory_has_content() mutably accesses Scry's cache, so the immutable
         * entry borrow must end first.
         */
        let Some(entry) = app.entry_at_filtered_position(position).cloned() else {
            continue;
        };

        let marked = app.is_path_marked(&entry.path);

        let has_content = entry.is_directory && app.directory_has_content(&entry.path);

        items.push(entry_list_item(
            &entry,
            &highlight_terms,
            fuzzy_highlight_query,
            has_content,
            marked,
            app.show_icons,
            &app.theme,
        ));
    }

    let heading = if app.search_mode == SearchMode::Fuzzy && app.fuzzy_filter_in_progress {
        format!(
            "Fuzzy results — updating… — best {}",
            app.filtered_indices.len(),
        )
    } else if app.recursive_search_active() {
        if app.scan_in_progress {
            match app.search_mode {
                SearchMode::Exact => "Recursive results — updating…".to_string(),

                SearchMode::Fuzzy => format!(
                    "Fuzzy results — updating… — best {}",
                    app.filtered_indices.len(),
                ),
            }
        } else if app.search_mode == SearchMode::Fuzzy {
            if app.recursive_scan_partial {
                format!(
                    "Fuzzy results — best {} — partial index",
                    app.filtered_indices.len(),
                )
            } else {
                format!("Fuzzy results — best {}", app.filtered_indices.len(),)
            }
        } else {
            "Recursive results".to_string()
        }
    } else {
        "Entries".to_string()
    };

    let sort_arrow = if app.sort_descending { "↓" } else { "↑" };

    let marked_title = if app.marked_count() == 0 {
        String::new()
    } else {
        format!(" — {} marked", app.marked_count())
    };

    let title = if app.scan_in_progress && app.recursive_search_active() {
        format!(
            " {}{} — {} shown — {} {} ",
            heading,
            marked_title,
            app.filtered_indices.len(),
            app.sort_mode.label(),
            sort_arrow,
        )
    } else {
        format!(
            " {}{} — {} shown / {} scanned — {} {} ",
            heading,
            marked_title,
            app.filtered_indices.len(),
            app.active_entry_count(),
            app.sort_mode.label(),
            sort_arrow,
        )
    };

    let list = List::new(items)
        .block(
            Block::default()
                .title(entries_title_with_parent_button(title, &app.theme))
                .title_bottom(entries_title_with_home_button(&app.theme))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.frames.entries)),
        )
        .highlight_symbol("▶")
        .highlight_style(
            Style::default()
                .fg(app.theme.selection.text)
                .bg(app.theme.selection.background)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = ListState::default();

    if !app.filtered_indices.is_empty() && !app.scrollbar_drag_active {
        state.select(Some(app.selected.saturating_sub(window_start)));
    }

    frame.render_stateful_widget(list, area, &mut state);

    render_entries_scrollbar(
        frame,
        area,
        app.filtered_indices.len(),
        visible_rows,
        app.list_offset,
        &app.theme,
    );
}

fn render_entries_scrollbar(
    frame: &mut Frame,
    area: Rect,
    content_length: usize,
    viewport_length: usize,
    position: usize,
    theme: &Theme,
) {
    /*
     * Do not render a scrollbar when every entry already fits inside
     * the visible viewport.
     */
    if content_length <= viewport_length || viewport_length == 0 {
        return;
    }

    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(Some("│"))
        .thumb_symbol("█")
        .track_style(Style::default().fg(theme.scrollbar.track))
        .thumb_style(Style::default().fg(theme.scrollbar.thumb));

    let maximum_offset = content_length.saturating_sub(viewport_length);

    /*
     * Ratatui's position scale extends to content_length - 1, while list_offset
     * extends only to content_length - viewport_length.
     *
     * Scale the viewport offset onto Ratatui's full position range so the thumb
     * can still reach both ends of the track.
     */
    let scrollbar_position = position
        .saturating_mul(content_length.saturating_sub(1))
        .checked_div(maximum_offset)
        .unwrap_or(0);

    let mut scrollbar_state = ScrollbarState::new(content_length)
        .position(scrollbar_position)
        .viewport_content_length(viewport_length);

    /*
     * Keep the scrollbar inside the filesystem frame rather than drawing
     * over its top and bottom borders.
     */
    let scrollbar_area = area.inner(Margin {
        vertical: 1,
        horizontal: 0,
    });

    frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
}

fn render_tree_entries(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let visible_rows = area.height.saturating_sub(2) as usize;

    app.viewport_rows = visible_rows.max(1);

    app.ensure_selection_visible(visible_rows);

    let window_start = app.list_offset;

    let window_end = window_start
        .saturating_add(visible_rows)
        .min(app.filtered_tree_indices.len());

    /*
     * Tree rows highlight only ordinary search text, never query modifiers.
     */
    let parsed_query = parse_query(&app.query);

    let highlight_terms = parsed_query.highlight_terms();

    let fuzzy_highlight_query = if app.search_mode == SearchMode::Fuzzy
        && !parsed_query.search_text().is_empty()
        && parsed_query.search_text() != "."
    {
        Some(parsed_query.search_text())
    } else {
        None
    };

    let mut items: Vec<ListItem> = Vec::new();

    for position in window_start..window_end {
        let Some(row) = app.tree_row_at_filtered_position(position).cloned() else {
            continue;
        };

        let marked = app.is_path_marked(&row.entry.path);

        let has_content = row.entry.is_directory && app.directory_has_content(&row.entry.path);

        items.push(tree_list_item(
            &row,
            &highlight_terms,
            fuzzy_highlight_query,
            has_content,
            marked,
            app.show_icons,
            &app.theme,
        ));
    }

    let sort_arrow = if app.sort_descending { "↓" } else { "↑" };

    let marked_title = if app.marked_count() == 0 {
        String::new()
    } else {
        format!(" — {} marked", app.marked_count())
    };

    let title = if app.scan_in_progress && app.recursive_search_active() {
        format!(
            " Recursive Tree{} — scanning {} entries… — {} {} ",
            marked_title,
            app.recursive_entries.len(),
            app.sort_mode.label(),
            sort_arrow,
        )
    } else {
        let tree_kind = if app.recursive_search_active() && app.recursive_scan_partial {
            "Tree — partial index"
        } else {
            "Tree"
        };

        if app.exact_tree_limit_reached {
            format!(
                " {}{} — {} matches capped / {} nodes — limit reached — {} {} ",
                tree_kind,
                marked_title,
                EXACT_TREE_MATCH_LIMIT,
                app.tree_rows.len(),
                app.sort_mode.label(),
                sort_arrow,
            )
        } else {
            format!(
                " {}{} — {} shown / {} nodes — {} {} ",
                tree_kind,
                marked_title,
                app.filtered_tree_indices.len(),
                app.tree_rows.len(),
                app.sort_mode.label(),
                sort_arrow,
            )
        }
    };

    let list = List::new(items)
        .block(
            Block::default()
                .title(entries_title_with_parent_button(title, &app.theme))
                .title_bottom(entries_title_with_home_button(&app.theme))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.frames.entries)),
        )
        .highlight_symbol("▶")
        .highlight_style(
            Style::default()
                .fg(app.theme.selection.text)
                .bg(app.theme.selection.background)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = ListState::default();

    if !app.filtered_tree_indices.is_empty() && !app.scrollbar_drag_active {
        state.select(Some(app.selected.saturating_sub(window_start)));
    }

    frame.render_stateful_widget(list, area, &mut state);

    render_entries_scrollbar(
        frame,
        area,
        app.filtered_tree_indices.len(),
        visible_rows,
        app.list_offset,
        &app.theme,
    );
}

fn tree_list_item(
    row: &TreeRow,
    highlight_terms: &[QueryHighlightTerm],
    fuzzy_highlight_query: Option<&str>,
    has_content: bool,
    marked: bool,
    show_icons: bool,
    theme: &Theme,
) -> ListItem<'static> {
    let mut spans = Vec::new();

    for ancestor_has_more in &row.ancestor_has_more {
        spans.push(Span::styled(
            if *ancestor_has_more { "│  " } else { "   " },
            Style::default().fg(COLOR_MUTED),
        ));
    }

    spans.push(Span::styled(
        if row.is_last { "└─ " } else { "├─ " },
        Style::default().fg(COLOR_MUTED),
    ));

    let (marker, color, suffix) = if row.entry.is_directory {
        (
            if row.expanded { "▾ " } else { "▸ " },
            COLOR_DIRECTORY,
            if has_content && !row.expanded {
                " →"
            } else {
                "/"
            },
        )
    } else if row.entry.is_symlink {
        ("↪ ", COLOR_SYMLINK, "@")
    } else {
        ("  ", COLOR_FILE, "")
    };

    spans.push(Span::styled(marker.to_string(), Style::default().fg(color)));

    if show_icons {
        spans.push(Span::styled(
            format!("{} ", file_icon(&row.entry)),
            file_icon_color(&row.entry, theme),
        ));
    }

    if marked {
        spans.push(Span::styled("✓ ", Style::default().fg(theme.ui.query)));
    }

    spans.extend(highlighted_query_spans(
        &row.entry.name,
        highlight_terms,
        fuzzy_highlight_query,
        color,
    ));

    if !suffix.is_empty() {
        spans.push(Span::styled(suffix.to_string(), Style::default().fg(color)));
    }

    ListItem::new(Line::from(spans))
}

fn file_icon(entry: &FileEntry) -> &'static str {
    if entry.is_symlink {
        return "";
    }

    if entry.is_directory {
        return "󰉋";
    }

    let extension = entry
        .path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase());

    match extension.as_deref() {
        Some("rs") => "",
        Some("py") => "",
        Some("sh") | Some("bash") | Some("zsh") | Some("fish") => "",
        Some("md") | Some("txt") | Some("rst") => "󰈙",
        Some("json") | Some("yaml") | Some("yml") | Some("toml") | Some("ini") | Some("conf")
        | Some("config") => "",
        Some("zip") | Some("tar") | Some("gz") | Some("xz") | Some("bz2") | Some("7z")
        | Some("rar") => "",
        Some("png") | Some("jpg") | Some("jpeg") | Some("gif") | Some("webp") | Some("svg")
        | Some("bmp") => "󰈟",
        Some("mp3") | Some("flac") | Some("wav") | Some("ogg") | Some("m4a") => "󰎈",
        Some("mp4") | Some("mkv") | Some("avi") | Some("mov") | Some("webm") => "󰈫",
        Some("pdf") => "",
        Some("c") | Some("h") | Some("cpp") | Some("hpp") | Some("cc") => "",
        Some("js") | Some("ts") => "󰌞",
        Some("html") | Some("htm") | Some("css") => "󰌝",
        _ => "󰈔",
    }
}

fn file_icon_color(entry: &FileEntry, theme: &Theme) -> Color {
    use crate::classify::FileClass;

    if entry.is_directory {
        return theme.icons.directory;
    }

    if entry.is_symlink {
        return theme.icons.symlink;
    }

    match entry.class {
        FileClass::Rust => theme.icons.rust,

        FileClass::Python => theme.icons.python,

        FileClass::ShellScript | FileClass::Executable => theme.icons.shell,

        FileClass::C
        | FileClass::Cpp
        | FileClass::Assembly
        | FileClass::Lua
        | FileClass::Ruby
        | FileClass::Perl
        | FileClass::Php
        | FileClass::Go
        | FileClass::Swift
        | FileClass::Dart
        | FileClass::CSharp
        | FileClass::Scala
        | FileClass::Groovy
        | FileClass::R
        | FileClass::Awk
        | FileClass::Elixir
        | FileClass::Erlang
        | FileClass::FSharp
        | FileClass::VisualBasic
        | FileClass::Clojure
        | FileClass::Zig
        | FileClass::Nim
        | FileClass::Crystal
        | FileClass::Haskell
        | FileClass::Ocaml
        | FileClass::Pascal
        | FileClass::Solidity
        | FileClass::Vala
        | FileClass::SourceCode => theme.icons.source,

        FileClass::Java | FileClass::Kotlin => theme.icons.java,

        FileClass::JavaScript | FileClass::TypeScript => theme.icons.javascript,

        FileClass::Web => theme.icons.web,

        FileClass::Config | FileClass::StructuredData | FileClass::Build => theme.icons.config,

        FileClass::Archive | FileClass::Package => theme.icons.archive,

        FileClass::Document | FileClass::Text => theme.icons.document,

        FileClass::Spreadsheet => theme.icons.spreadsheet,

        FileClass::Presentation => theme.icons.presentation,

        FileClass::Image | FileClass::VectorImage => theme.icons.image,

        FileClass::Audio => theme.icons.audio,

        FileClass::Video => theme.icons.video,

        FileClass::Font => theme.icons.font,

        FileClass::Database => theme.icons.database,

        FileClass::Log => theme.icons.log,

        FileClass::Backup => theme.icons.backup,

        FileClass::Certificate => theme.icons.certificate,

        FileClass::DiskImage => theme.icons.disk_image,

        FileClass::Torrent => theme.icons.torrent,

        FileClass::DesktopEntry | FileClass::Plugin => theme.icons.desktop_plugin,

        FileClass::Binary => theme.icons.binary,

        FileClass::Unknown => theme.icons.unknown,

        FileClass::Directory => theme.icons.directory,

        FileClass::Symlink => theme.icons.symlink,
    }
}

fn entry_list_item(
    entry: &FileEntry,
    highlight_terms: &[QueryHighlightTerm],
    fuzzy_highlight_query: Option<&str>,
    has_content: bool,
    marked: bool,
    show_icons: bool,
    theme: &Theme,
) -> ListItem<'static> {
    let (prefix, color, suffix) = if entry.is_directory {
        ("▸", COLOR_DIRECTORY, if has_content { " →" } else { "/" })
    } else if entry.is_symlink {
        ("↪ ", COLOR_SYMLINK, "@")
    } else {
        (" ", COLOR_FILE, "")
    };

    let mut spans = vec![Span::styled(prefix.to_string(), Style::default().fg(color))];

    if show_icons {
        spans.push(Span::styled(
            format!("{} ", file_icon(entry)),
            Style::default().fg(file_icon_color(entry, theme)),
        ));
    }

    if marked {
        spans.push(Span::styled("✓ ", Style::default().fg(theme.ui.query)));
    }

    let display_path = entry.relative_path.to_string_lossy().into_owned();

    spans.extend(highlighted_query_spans(
        &display_path,
        highlight_terms,
        fuzzy_highlight_query,
        color,
    ));

    if !suffix.is_empty() {
        spans.push(Span::styled(suffix.to_string(), Style::default().fg(color)));
    }

    ListItem::new(Line::from(spans))
}

fn highlighted_query_spans(
    text: &str,
    terms: &[QueryHighlightTerm],
    fuzzy_query: Option<&str>,
    normal_color: Color,
) -> Vec<Span<'static>> {
    let mut ranges = Vec::new();

    /*
     * Preserve Scry's existing scattered-character highlighting for the
     * ordinary fuzzy query.
     */
    if let Some(query) = fuzzy_query {
        let positions = fuzzy_highlight_positions(text, query);

        let character_ranges: Vec<(usize, usize)> = text
            .char_indices()
            .map(|(start, character)| (start, start + character.len_utf8()))
            .collect();

        for position in positions {
            if let Some(range) = character_ranges.get(position) {
                ranges.push(*range);
            }
        }
    }

    for term in terms {
        if term.value.is_empty() {
            continue;
        }

        /*
         * The ordinary fuzzy term was already represented by scattered
         * positions above. Do not additionally paint it as a contiguous
         * substring.
         */
        if fuzzy_query.is_some_and(|query| !term.case_sensitive && term.value == query) {
            continue;
        }

        if term.case_sensitive {
            collect_sensitive_match_ranges(text, &term.value, &mut ranges);
        } else {
            collect_insensitive_match_ranges(text, &term.value, &mut ranges);
        }
    }

    if ranges.is_empty() {
        return vec![Span::styled(
            text.to_string(),
            Style::default().fg(normal_color),
        )];
    }

    /*
     * Combine overlaps such as:
     *
     *     +bas +bash
     *
     * so "bash" becomes one clean highlighted span.
     */
    ranges.sort_unstable_by_key(|range| range.0);

    let mut merged_ranges: Vec<(usize, usize)> = Vec::new();

    for (start, end) in ranges {
        if start >= end {
            continue;
        }

        if let Some((_, previous_end)) = merged_ranges.last_mut()
            && start <= *previous_end
        {
            *previous_end = (*previous_end).max(end);

            continue;
        }

        merged_ranges.push((start, end));
    }

    let mut spans = Vec::new();

    let mut previous_end = 0_usize;

    for (start, end) in merged_ranges {
        if previous_end < start {
            spans.push(Span::styled(
                text[previous_end..start].to_string(),
                Style::default().fg(normal_color),
            ));
        }

        spans.push(Span::styled(
            text[start..end].to_string(),
            Style::default().fg(COLOR_MATCH),
        ));

        previous_end = end;
    }

    if previous_end < text.len() {
        spans.push(Span::styled(
            text[previous_end..].to_string(),
            Style::default().fg(normal_color),
        ));
    }

    spans
}

fn collect_sensitive_match_ranges(text: &str, query: &str, ranges: &mut Vec<(usize, usize)>) {
    if query.is_empty() {
        return;
    }

    for (start, _) in text.match_indices(query) {
        ranges.push((start, start + query.len()));
    }
}

fn collect_insensitive_match_ranges(text: &str, query: &str, ranges: &mut Vec<(usize, usize)>) {
    let folded_text = fold_with_source_ranges(text);

    let folded_query: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();

    if folded_query.is_empty() || folded_query.len() > folded_text.len() {
        return;
    }

    let mut search_index = 0_usize;

    while search_index + folded_query.len() <= folded_text.len() {
        let matches_query = folded_text[search_index..search_index + folded_query.len()]
            .iter()
            .map(|(character, _, _)| *character)
            .eq(folded_query.iter().copied());

        if matches_query {
            let byte_start = folded_text[search_index].1;

            let byte_end = folded_text[search_index + folded_query.len() - 1].2;

            ranges.push((byte_start, byte_end));

            search_index += folded_query.len();
        } else {
            search_index += 1;
        }
    }
}

fn fold_with_source_ranges(text: &str) -> Vec<(char, usize, usize)> {
    let mut folded = Vec::new();

    for (byte_start, character) in text.char_indices() {
        let byte_end = byte_start + character.len_utf8();

        for lowercase_character in character.to_lowercase() {
            folded.push((lowercase_character, byte_start, byte_end));
        }
    }

    folded
}

fn render_selection(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let theme = app.theme;

    let selected_classification = app.selected_classification();

    let content = if let Some(message) = &app.error_message {
        Line::styled(message.clone(), Style::default().fg(theme.ui.error))
    } else if let Some(status) = &app.status_message {
        Line::styled(status.clone(), Style::default().fg(theme.ui.status))
    } else if let Some(entry) = app.selected_entry() {
        Line::styled(
            entry.path.display().to_string(),
            Style::default().fg(if entry.is_directory {
                theme.ui.directory
            } else if entry.is_symlink {
                theme.ui.symlink
            } else {
                theme.ui.file
            }),
        )
    } else {
        Line::styled("No matching entries", Style::default().fg(theme.ui.muted))
    };

    let title = if let Some(class) = selected_classification {
        Line::from(vec![
            Span::styled(" Selection — ", Style::default().fg(theme.frames.selection)),
            Span::styled(
                class.label(),
                Style::default()
                    .fg(theme.ui.classification)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", Style::default().fg(theme.frames.selection)),
        ])
    } else {
        Line::from(Span::styled(
            " Selection ",
            Style::default().fg(theme.frames.selection),
        ))
    };

    let paragraph = Paragraph::new(content).block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.frames.selection)),
    );

    frame.render_widget(paragraph, area);
}

fn render_file_info_overlay(frame: &mut Frame, app: &App, area: Rect) -> Option<Rect> {
    let state = app.file_info.as_ref()?;

    let info = &state.info;

    let theme = &app.theme;

    /*
     * Prefer a broad landscape window without allowing it to consume the
     * complete terminal.
     *
     * On very wide displays the 124-column cap preserves substantial browser
     * context around the popup. On smaller displays, four terminal cells remain
     * visible on each side whenever possible.
     */
    let popup_width = area
        .width
        .saturating_mul(86)
        .saturating_div(100)
        .clamp(72, 124)
        .min(area.width.saturating_sub(4).max(1));

    /*
     * The ordinary landscape layout needs twenty-two rows.
     *
     * Very short terminals surrender only a small outer margin rather than
     * producing invalid geometry.
     */
    let popup_height = 22_u16.min(area.height.saturating_sub(2).max(1));

    let popup_area = centered_rect(popup_width, popup_height, area);

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" File Information ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.frames.details));

    let inner = block.inner(popup_area);

    frame.render_widget(block, popup_area);

    if inner.width == 0 || inner.height == 0 {
        return None;
    }

    /*
     * Layout:
     *
     *     Path
     *     blank
     *     two-column information body
     *     blank
     *     optional directory/cache/link line
     *     status
     *     close button
     */
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(12),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    render_file_info_path(frame, rows[0], &info.path.display().to_string(), theme);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[2]);

    render_file_info_left_column(frame, columns[0], info, theme);

    render_file_info_right_column(frame, columns[1], info, theme);

    render_file_info_context_line(frame, rows[4], info, theme);

    render_file_info_status(frame, rows[5], state, theme);

    let close_width = 11_u16.min(rows[6].width);

    let close_area = Rect {
        x: rows[6]
            .x
            .saturating_add(rows[6].width.saturating_sub(close_width) / 2),

        y: rows[6].y,

        width: close_width,

        height: 1,
    };

    let close = Paragraph::new(Line::from(vec![
        Span::styled("[ ", Style::default().fg(theme.frames.parent_brackets)),
        Span::styled(
            "Close",
            Style::default()
                .fg(theme.frames.parent_text)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ]", Style::default().fg(theme.frames.parent_brackets)),
    ]))
    .alignment(Alignment::Center);

    frame.render_widget(close, close_area);

    Some(close_area)
}

fn render_file_info_path(frame: &mut Frame, area: Rect, path: &str, theme: &Theme) {
    let value_width = area.width.saturating_sub(8) as usize;

    let path = truncate_with_ellipsis(path, value_width);

    let line = Line::from(vec![
        Span::styled(" Path: ", Style::default().fg(theme.ui.muted)),
        Span::styled(path, Style::default().fg(theme.ui.file)),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}

fn render_file_info_left_column(
    frame: &mut Frame,
    area: Rect,
    info: &crate::file_info::FileInfo,
    theme: &Theme,
) {
    let lines = vec![
        file_info_section_line("Identity", theme),
        file_info_value_line("Name", info.name.clone(), theme.ui.file, theme),
        file_info_value_line(
            "Classification",
            info.classification.label().to_string(),
            theme.ui.classification,
            theme,
        ),
        file_info_value_line(
            "Entry kind",
            info.kind_label().to_string(),
            theme.ui.classification,
            theme,
        ),
        file_info_value_line("Extension", info.extension(), theme.ui.query, theme),
        Line::raw(""),
        file_info_section_line("Filesystem", theme),
        file_info_value_line("Size", info.human_size(), theme.ui.size, theme),
        file_info_value_line("Exact size", info.exact_size(), theme.ui.size, theme),
        file_info_permissions_line("Permissions", &info.symbolic_permissions(), theme),
        file_info_value_line(
            "Octal mode",
            info.octal_permissions(),
            theme.permissions.special,
            theme,
        ),
        file_info_value_line("Owner", info.owner(), theme.ui.user, theme),
        file_info_value_line("Group", info.group(), theme.ui.user, theme),
    ];

    frame.render_widget(Paragraph::new(lines), area);
}

fn render_file_info_right_column(
    frame: &mut Frame,
    area: Rect,
    info: &crate::file_info::FileInfo,
    theme: &Theme,
) {
    let mut lines = vec![
        file_info_section_line("Times", theme),
        file_info_value_line("Modified", info.modified(), theme.ui.date, theme),
        file_info_value_line("Accessed", info.accessed(), theme.ui.date, theme),
        file_info_value_line("Created", info.created(), theme.ui.date, theme),
        file_info_value_line("Age", info.age(), theme.ui.query, theme),
        Line::raw(""),
        file_info_section_line("Status", theme),
        file_info_value_line(
            "Executable",
            info.executable().to_string(),
            theme.ui.query,
            theme,
        ),
        file_info_value_line("Hidden", info.hidden().to_string(), theme.ui.query, theme),
        file_info_value_line(
            "Source",
            info.source_label.clone(),
            if info.is_remote {
                theme.ui.symlink
            } else {
                theme.ui.file
            },
            theme,
        ),
        file_info_value_line(
            "Link target",
            info.symlink_target_display(),
            theme.ui.symlink,
            theme,
        ),
        file_info_value_line(
            "Target exists",
            info.symlink_target_exists_display().to_string(),
            theme.ui.query,
            theme,
        ),
    ];

    if let Some(cache_info) = &info.cache_info {
        lines.push(file_info_value_line(
            "Cached copy",
            cache_info.cached_status().to_string(),
            theme.ui.query,
            theme,
        ));

        lines.push(file_info_value_line(
            "Cached size",
            cache_info.cached_size(),
            theme.ui.size,
            theme,
        ));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn render_file_info_context_line(
    frame: &mut Frame,
    area: Rect,
    info: &crate::file_info::FileInfo,
    theme: &Theme,
) {
    let (label, value, color) = if let Some(summary) = info.directory_summary {
        ("Contents", summary.display_line(), theme.ui.directory)
    } else if let Some(cache_info) = &info.cache_info {
        (
            "Cache path",
            cache_info.cache_path.display().to_string(),
            theme.ui.symlink,
        )
    } else if info.kind == crate::entry::EntryKind::Symlink {
        (
            "Resolved link",
            info.symlink_target_display(),
            theme.ui.symlink,
        )
    } else {
        (
            "Location",
            if info.is_remote {
                "Remote filesystem entry".to_string()
            } else {
                "Local filesystem entry".to_string()
            },
            theme.ui.muted,
        )
    };

    /*
     * file_info_value_line() reserves:
     *
     *     two leading spaces
     *     fifteen cells for the label
     *
     * The remainder belongs to the displayed value.
     */
    let value_width = area.width.saturating_sub(17) as usize;

    let value = truncate_with_ellipsis(&value, value_width);

    frame.render_widget(
        Paragraph::new(file_info_value_line(label, value, color, theme)),
        area,
    );
}

fn render_file_info_status(
    frame: &mut Frame,
    area: Rect,
    state: &crate::file_info::FileInfoState,
    theme: &Theme,
) {
    let color = if state.error.is_some() {
        COLOR_ERROR
    } else if state.loading || state.info.notes.is_empty() {
        theme.ui.query
    } else {
        theme.ui.muted
    };

    frame.render_widget(
        Paragraph::new(file_info_value_line(
            "Status",
            state.status_line(),
            color,
            theme,
        )),
        area,
    );
}

fn file_info_section_line(title: &str, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {} ", title),
            Style::default()
                .fg(theme.frames.details)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "────────────────────────",
            Style::default().fg(theme.ui.muted),
        ),
    ])
}

fn file_info_value_line(
    label: &str,
    value: String,
    value_color: Color,
    theme: &Theme,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {:<15}", format!("{}:", label)),
            Style::default().fg(theme.ui.muted),
        ),
        Span::styled(value, Style::default().fg(value_color)),
    ])
}

fn truncate_with_ellipsis(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }

    if width <= 3 {
        return ".".repeat(width);
    }

    let visible_width = width - 3;

    let mut result: String = value.chars().take(visible_width).collect();

    result.push_str("...");

    result
}

fn file_info_permissions_line(label: &str, permissions: &str, theme: &Theme) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!("  {:<15}", format!("{}:", label)),
        Style::default().fg(theme.ui.muted),
    )];

    if permissions == "Loading…" {
        spans.push(Span::styled(
            permissions.to_string(),
            Style::default().fg(theme.ui.query),
        ));
    } else {
        spans.extend(permission_spans(permissions, theme));
    }

    Line::from(spans)
}

fn render_remote_index_setup_overlay(
    frame: &mut Frame,
    app: &App,
    area: Rect,
) -> RemoteIndexSetupUiRegions {
    let Some(setup) = app.remote_index_setup.as_ref() else {
        return RemoteIndexSetupUiRegions::default();
    };

    const POPUP_WIDTH: u16 = 76;

    const NORMAL_HEIGHT: u16 = 17;

    const INVALID_HEIGHT: u16 = 20;

    let popup_height = if setup.invalid_reason.is_some() {
        INVALID_HEIGHT
    } else {
        NORMAL_HEIGHT
    };

    let popup_area = centered_rect(POPUP_WIDTH, popup_height, area);

    let theme = &app.theme;

    let selected_style = Style::default()
        .fg(theme.selection.text)
        .bg(theme.selection.background)
        .add_modifier(Modifier::BOLD);

    let normal_style = Style::default().fg(theme.ui.file);

    let button = |label: &str, focus: RemoteIndexDialogFocus| -> Span<'static> {
        let style = if setup.focus == focus {
            selected_style
        } else {
            normal_style
        };

        Span::styled(format!("  {}  ", label), style)
    };

    let policy_focused = setup.focus == RemoteIndexDialogFocus::Policy;

    let policy_button = |label: &str, selected: bool| -> Span<'static> {
        let marker = if selected { "(•)" } else { "( )" };

        let style = if policy_focused && selected {
            /*
             * The selected policy also owns keyboard focus.
             */
            selected_style
        } else if selected {
            /*
             * Focus has moved to OK or Cancel, but this policy remains
             * visibly selected.
             */
            Style::default()
                .fg(app.theme.ui.query)
                .add_modifier(Modifier::BOLD)
        } else {
            normal_style
        };

        Span::styled(format!(" {} {} ", marker, label), style)
    };

    let mut lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled("Remote computer: ", Style::default().fg(theme.ui.muted)),
            Span::styled(
                setup.identity.display_label(),
                Style::default()
                    .fg(theme.ui.query)
                    .add_modifier(Modifier::BOLD),
            ),
        ])
        .alignment(Alignment::Center),
        Line::raw(""),
        Line::styled(
            "Scry needs a reusable index for recursive remote searches.",
            Style::default().fg(theme.ui.file),
        )
        .alignment(Alignment::Center),
        Line::styled(
            "The accessible remote filesystem will be indexed from /.",
            Style::default().fg(theme.ui.muted),
        )
        .alignment(Alignment::Center),
        Line::styled(
            "/proc, /sys, /dev, and /run are skipped.",
            Style::default().fg(theme.ui.muted),
        )
        .alignment(Alignment::Center),
        Line::raw(""),
    ];

    if let Some(reason) = setup.invalid_reason.as_deref() {
        lines.push(
            Line::styled(
                "The existing remote index is invalid:",
                Style::default()
                    .fg(theme.ui.error)
                    .add_modifier(Modifier::BOLD),
            )
            .alignment(Alignment::Center),
        );

        lines.push(
            Line::styled(reason.to_string(), Style::default().fg(theme.ui.error))
                .alignment(Alignment::Center),
        );

        lines.push(Line::raw(""));
    }

    lines.extend([
        Line::styled(
            "Choose which entries the index should contain:",
            Style::default().fg(theme.ui.file),
        )
        .alignment(Alignment::Center),
        Line::raw(""),
        Line::from(vec![
            policy_button("Standard entries", !setup.includes_hidden),
            Span::raw("   "),
            policy_button("Include dot-entries", setup.includes_hidden),
            Span::raw("   "),
            button("OK", RemoteIndexDialogFocus::Ok),
            Span::raw("   "),
            button("Cancel", RemoteIndexDialogFocus::Cancel),
        ])
        .alignment(Alignment::Center),
        Line::raw(""),
        Line::styled(
            "The choice can later be changed by rebuilding the index.",
            Style::default().fg(theme.ui.muted),
        )
        .alignment(Alignment::Center),
        Line::raw(""),
        Line::styled(
            "←/→ or Tab to choose   Enter to confirm   Esc to cancel",
            Style::default().fg(theme.ui.muted),
        )
        .alignment(Alignment::Center),
    ]);

    /*
     * The choices occupy one fixed line near the bottom of the popup.
     *
     * Their widths include the two padding cells rendered on each side by the
     * button closure above.
     */
    const STANDARD_LABEL: &str = "Standard entries";
    const HIDDEN_LABEL: &str = "Include dot-entries";
    const OK_LABEL: &str = "OK";
    const CANCEL_LABEL: &str = "Cancel";
    const BUTTON_GAP: u16 = 3;

    let standard_width = STANDARD_LABEL.chars().count() as u16 + 4;

    let hidden_width = HIDDEN_LABEL.chars().count() as u16 + 4;

    let ok_width = OK_LABEL.chars().count() as u16 + 4;

    let cancel_width = CANCEL_LABEL.chars().count() as u16 + 4;

    let total_button_width = standard_width
        .saturating_add(hidden_width)
        .saturating_add(ok_width)
        .saturating_add(cancel_width)
        .saturating_add(BUTTON_GAP.saturating_mul(3));

    let button_start_x = popup_area
        .x
        .saturating_add(popup_area.width.saturating_sub(total_button_width) / 2);

    let button_row = popup_area
        .y
        .saturating_add(if setup.invalid_reason.is_some() {
            13
        } else {
            10
        });

    let standard_region = Rect {
        x: button_start_x,
        y: button_row,
        width: standard_width,
        height: 1,
    };

    let include_hidden_region = Rect {
        x: standard_region
            .x
            .saturating_add(standard_region.width)
            .saturating_add(BUTTON_GAP),
        y: button_row,
        width: hidden_width,
        height: 1,
    };

    let ok_region = Rect {
        x: include_hidden_region
            .x
            .saturating_add(include_hidden_region.width)
            .saturating_add(BUTTON_GAP),
        y: button_row,
        width: ok_width,
        height: 1,
    };

    let cancel_region = Rect {
        x: ok_region
            .x
            .saturating_add(ok_region.width)
            .saturating_add(BUTTON_GAP),
        y: button_row,
        width: cancel_width,
        height: 1,
    };

    let popup = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Remote Index Setup ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.frames.popup)),
        )
        .style(Style::default().bg(app.theme.frames.popup_background))
        .wrap(ratatui::widgets::Wrap { trim: true });

    frame.render_widget(Clear, popup_area);

    frame.render_widget(popup, popup_area);

    RemoteIndexSetupUiRegions {
        standard: standard_region,

        include_hidden: include_hidden_region,

        ok: ok_region,

        cancel: cancel_region,
    }
}

fn render_connection_overlay(frame: &mut Frame, app: &App, area: Rect) -> ConnectionUiRegions {
    const POPUP_WIDTH: u16 = 76;

    const POPUP_HEIGHT: u16 = 24;

    let popup_area = centered_rect(POPUP_WIDTH, POPUP_HEIGHT, area);

    let profiles = app.connection_store.profiles();

    let selected_profile = app.connection_dialog.selected_profile;

    let draft = &app.connection_dialog.draft;

    let profile_focused = app.connection_dialog.focus == ConnectionField::Profiles;

    let profile_summary = if profiles.is_empty() {
        Line::styled("  No saved profiles", Style::default().fg(COLOR_MUTED))
    } else {
        let profile_name = profiles
            .get(selected_profile)
            .map(|profile| profile.name.as_str())
            .unwrap_or("—");

        let normal_style = Style::default().fg(COLOR_MUTED);

        let focused_style = Style::default()
            .fg(app.theme.selection.text)
            .bg(app.theme.selection.background)
            .add_modifier(Modifier::BOLD);

        let label_style = if profile_focused {
            focused_style
        } else {
            normal_style
        };

        let value_style = if profile_focused {
            focused_style
        } else {
            Style::default()
                .fg(COLOR_QUERY)
                .add_modifier(Modifier::BOLD)
        };

        Line::from(vec![
            Span::styled(
                if profile_focused {
                    "  Saved profile: ◀ "
                } else {
                    "  Saved profile: "
                },
                label_style,
            ),
            Span::styled(profile_name.to_string(), value_style),
            Span::styled(
                if profile_focused {
                    format!(
                        " ▶  ({}/{})",
                        selected_profile.saturating_add(1),
                        profiles.len(),
                    )
                } else {
                    format!(
                        "  ({}/{})",
                        selected_profile.saturating_add(1),
                        profiles.len(),
                    )
                },
                label_style,
            ),
        ])
    };

    let lines = vec![
        Line::raw(""),
        profile_summary,
        Line::raw(""),
        connection_field_line(
            "Profile name",
            &draft.name,
            None,
            ConnectionField::Name,
            app.connection_dialog.focus,
            &app.theme,
        ),
        connection_field_line(
            "Host / IP",
            &draft.host,
            None,
            ConnectionField::Host,
            app.connection_dialog.focus,
            &app.theme,
        ),
        connection_field_line(
            "Username",
            &draft.username,
            None,
            ConnectionField::Username,
            app.connection_dialog.focus,
            &app.theme,
        ),
        connection_field_line(
            "Port",
            &app.connection_dialog.port_text,
            None,
            ConnectionField::Port,
            app.connection_dialog.focus,
            &app.theme,
        ),
        connection_field_line(
            "Identity file",
            &draft.identity_file,
            Some("Optional — e.g. ~/.ssh/id_ed25519"),
            ConnectionField::IdentityFile,
            app.connection_dialog.focus,
            &app.theme,
        ),
        connection_field_line(
            "Start directory",
            &draft.start_directory,
            None,
            ConnectionField::StartDirectory,
            app.connection_dialog.focus,
            &app.theme,
        ),
        Line::raw(""),
        Line::from(vec![
            connection_button_span(
                if app.connection_in_progress {
                    "Connecting…"
                } else {
                    "Connect"
                },
                ConnectionField::Connect,
                app.connection_dialog.focus,
                !app.connection_in_progress,
                &app.theme,
            ),
            Span::raw("   "),
            connection_button_span(
                "Save",
                ConnectionField::Save,
                app.connection_dialog.focus,
                true,
                &app.theme,
            ),
            Span::raw("   "),
            connection_button_span(
                "Delete",
                ConnectionField::Delete,
                app.connection_dialog.focus,
                !profiles.is_empty(),
                &app.theme,
            ),
            Span::raw("   "),
            connection_button_span(
                "Disconnect",
                ConnectionField::Disconnect,
                app.connection_dialog.focus,
                app.source_is_remote(),
                &app.theme,
            ),
        ])
        .alignment(Alignment::Center),
        Line::raw(""),
        Line::styled(
            "Keyboard",
            Style::default()
                .fg(COLOR_FRAME)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center),
        Line::raw(""),
        connection_help_line("Tab / Shift+Tab", "Move between controls"),
        connection_help_line("Enter", "Advance or activate the selected button"),
        connection_help_line("Backspace", "Delete from the focused field"),
        connection_help_line("Ctrl+U", "Clear the focused field"),
        connection_help_line("F4 / Esc", "Close the connection window"),
        if let Some(message) = &app.connection_dialog.error_message {
            Line::styled(
                message.clone(),
                Style::default().fg(
                    if message == "Profile saved" || app.connection_in_progress {
                        COLOR_QUERY
                    } else {
                        COLOR_ERROR
                    },
                ),
            )
            .alignment(Alignment::Center)
        } else {
            Line::raw("")
        },
        Line::styled(
            "[ Close ]",
            Style::default().fg(COLOR_FILE).add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center),
        Line::raw(""),
    ];

    let popup = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Remote Index Setup ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.frames.popup)),
        )
        .wrap(ratatui::widgets::Wrap { trim: true });

    frame.render_widget(Clear, popup_area);

    frame.render_widget(popup, popup_area);

    /*
     * Coordinates are relative to the popup's outer border.
     *
     * The first content row is popup_area.y + 1.
     */
    let field_x = popup_area.x.saturating_add(18);

    let field_width = popup_area.width.saturating_sub(23);

    let field_rect = |row: u16| Rect {
        x: field_x,

        y: popup_area.y.saturating_add(row),

        width: field_width,

        height: 1,
    };

    /*
     * The button row is centered. These rectangles follow the rendered labels
     * exactly, including their brackets.
     */
    let button_row = popup_area.y.saturating_add(11);

    let button_total_width: u16 = 11 + 3 + 8 + 3 + 10 + 3 + 14;

    let button_start = popup_area
        .x
        .saturating_add(popup_area.width.saturating_sub(button_total_width) / 2);

    /*
     * The Close button occupies the final centered content row.
     */
    let close_width: u16 = 9;

    let close_row = popup_area.y.saturating_add(22);

    let close_start = popup_area
        .x
        .saturating_add(popup_area.width.saturating_sub(close_width) / 2);

    ConnectionUiRegions {
        profiles: Rect {
            x: popup_area.x.saturating_add(2),

            y: popup_area.y.saturating_add(2),

            width: popup_area.width.saturating_sub(4),

            height: 1,
        },

        name: field_rect(4),

        host: field_rect(5),

        username: field_rect(6),

        port: field_rect(7),

        identity_file: field_rect(8),

        start_directory: field_rect(9),

        connect: Rect {
            x: button_start,

            y: button_row,

            width: 11,

            height: 1,
        },

        save: Rect {
            x: button_start.saturating_add(14),

            y: button_row,

            width: 8,

            height: 1,
        },

        delete: Rect {
            x: button_start.saturating_add(25),

            y: button_row,

            width: 10,

            height: 1,
        },

        disconnect: Rect {
            x: button_start.saturating_add(38),

            y: button_row,

            width: 14,

            height: 1,
        },

        close: Rect {
            x: close_start,

            y: close_row,

            width: close_width,

            height: 1,
        },
    }
}

fn connection_field_line(
    label: &str,
    value: &str,
    placeholder: Option<&str>,
    field: ConnectionField,
    focused_field: ConnectionField,
    theme: &Theme,
) -> Line<'static> {
    const FIELD_WIDTH: usize = 47;

    let focused = field == focused_field;

    let showing_placeholder = value.is_empty() && placeholder.is_some() && !focused;

    let source_text = if showing_placeholder {
        placeholder.unwrap_or_default()
    } else {
        value
    };

    let available_width = if focused {
        FIELD_WIDTH.saturating_sub(1)
    } else {
        FIELD_WIDTH
    };

    let mut displayed_value: String = source_text.chars().take(available_width).collect();

    if focused {
        displayed_value.push('▏');
    }

    let border_color = if focused { COLOR_FRAME } else { COLOR_MUTED };

    let value_style = if showing_placeholder {
        Style::default()
            .fg(COLOR_MUTED)
            .add_modifier(Modifier::ITALIC)
    } else if focused {
        Style::default()
            .fg(theme.selection.text)
            .bg(theme.selection.background)
    } else {
        Style::default().fg(COLOR_FILE)
    };

    Line::from(vec![
        Span::styled(
            format!("  {:<16}", format!("{}:", label),),
            Style::default().fg(if focused { COLOR_QUERY } else { COLOR_MUTED }),
        ),
        Span::styled("│", Style::default().fg(border_color)),
        Span::styled(
            format!(" {:<width$}", displayed_value, width = FIELD_WIDTH,),
            value_style,
        ),
        Span::styled("│", Style::default().fg(border_color)),
    ])
}

fn connection_button_span(
    label: &str,
    field: ConnectionField,
    focused_field: ConnectionField,
    enabled: bool,
    theme: &Theme,
) -> Span<'static> {
    let focused = field == focused_field;

    let style = if !enabled {
        Style::default().fg(COLOR_MUTED)
    } else if focused {
        Style::default()
            .fg(theme.selection.text)
            .bg(theme.selection.background)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(COLOR_FILE)
    };

    Span::styled(format!("[ {} ]", label,), style)
}

fn connection_help_line(shortcut: &str, description: &str) -> Line<'static> {
    /*
     * These widths describe the complete visible help table:
     *
     * shortcut column + gap + description column
     */
    const SHORTCUT_WIDTH: usize = 18;

    const DESCRIPTION_WIDTH: usize = 43;

    let shortcut = format!("{:<width$}", shortcut, width = SHORTCUT_WIDTH,);

    let description = format!("{:<width$}", description, width = DESCRIPTION_WIDTH,);

    Line::from(vec![
        Span::styled(
            shortcut,
            Style::default()
                .fg(COLOR_QUERY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(description, Style::default().fg(COLOR_MUTED)),
    ])
    .alignment(Alignment::Center)
}

fn render_transfer_overlay(frame: &mut Frame, app: &App, area: Rect) -> TransferUiRegions {
    const POPUP_WIDTH: u16 = 68;

    /*
     * Batch results need two additional rows for the file counts and
     * destination directory.
     */
    const SINGLE_POPUP_HEIGHT: u16 = 14;

    const BATCH_POPUP_HEIGHT: u16 = 16;

    /*
     * The popup has two border cells and the bar line has one leading space.
     */
    const BAR_WIDTH: usize = 63;

    let Some(transfer) = app.transfer.as_ref() else {
        return TransferUiRegions::default();
    };

    let popup_height = if transfer.is_batch {
        BATCH_POPUP_HEIGHT
    } else {
        SINGLE_POPUP_HEIGHT
    };

    let popup_area = centered_rect(POPUP_WIDTH, popup_height, area);

    let elapsed = app.transfer_elapsed();

    let elapsed_text = format_duration(elapsed);

    let total_bytes = transfer.total_bytes;

    let transferred_bytes = transfer.transferred_bytes.min(total_bytes);

    let total_text = format_file_size(total_bytes);

    let transferred_size_text = format_file_size(transferred_bytes);

    let finished = transfer.finished_elapsed.is_some();

    let single_failed = transfer.error.is_some();

    let batch_failed = transfer.is_batch && transfer.failed_count > 0;

    let failed = single_failed || batch_failed;

    let cancelling = transfer.cancel_requested && !finished;

    let percentage = if total_bytes == 0 {
        if finished && !failed { 100.0 } else { 0.0 }
    } else {
        transferred_bytes as f64 * 100.0 / total_bytes as f64
    }
    .clamp(0.0, 100.0);

    let filled_cells = if total_bytes == 0 {
        if finished && !failed { BAR_WIDTH } else { 0 }
    } else {
        (transferred_bytes as u128 * BAR_WIDTH as u128 / total_bytes as u128) as usize
    }
    .min(BAR_WIDTH);

    let bar = format!(
        "{}{}",
        "█".repeat(filled_cells),
        "░".repeat(BAR_WIDTH.saturating_sub(filled_cells)),
    );

    let seconds = elapsed.as_secs_f64();

    let speed_bytes_per_second = if seconds > 0.0 {
        transferred_bytes as f64 / seconds
    } else {
        0.0
    };

    let speed_text = format_transfer_speed(speed_bytes_per_second);

    let transferred_line = format!("{} / {}", transferred_size_text, total_text);

    let speed_label = if finished {
        " Average speed: "
    } else {
        " Speed: "
    };

    let popup_content = if transfer.is_batch {
        let title = if batch_failed {
            " Batch download completed with errors "
        } else if finished {
            " Batch download complete "
        } else if cancelling {
            " Cancelling batch download "
        } else {
            " Batch download "
        };

        let status = if batch_failed {
            format!(
                "{} file{} downloaded; {} file{} failed.",
                transfer.completed_count,
                if transfer.completed_count == 1 {
                    ""
                } else {
                    "s"
                },
                transfer.failed_count,
                if transfer.failed_count == 1 { "" } else { "s" },
            )
        } else if finished {
            format!(
                "All {} files were downloaded successfully.",
                transfer.completed_count,
            )
        } else if cancelling {
            "Stopping safely and removing the unfinished download…".to_string()
        } else {
            format!(
                "Downloading file {} of {}…",
                transfer
                    .item_index
                    .saturating_add(1)
                    .min(transfer.item_count),
                transfer.item_count,
            )
        };

        let status_color = if batch_failed {
            COLOR_ERROR
        } else if finished {
            COLOR_QUERY
        } else if cancelling {
            COLOR_MUTED
        } else {
            COLOR_FILE
        };

        let destination = transfer
            .destination_root
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "—".to_string());

        /*
         * Keep the destination readable without allowing a long path to
         * distort the popup.
         */
        let destination = truncate_with_ellipsis(&destination, 52);

        let file_progress = if finished {
            format!(
                "{} / {} files",
                transfer.completed_count, transfer.item_count,
            )
        } else {
            format!(
                "{} / {}",
                transfer
                    .item_index
                    .saturating_add(1)
                    .min(transfer.item_count),
                transfer.item_count,
            )
        };

        let mut lines = vec![
            Line::raw(""),
            Line::from(vec![
                Span::styled(" Files: ", Style::default().fg(COLOR_MUTED)),
                Span::styled(
                    file_progress,
                    Style::default()
                        .fg(COLOR_QUERY)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("    Elapsed: ", Style::default().fg(COLOR_MUTED)),
                Span::styled(elapsed_text.clone(), Style::default().fg(COLOR_QUERY)),
            ]),
            Line::from(vec![
                Span::styled(" Destination: ", Style::default().fg(COLOR_MUTED)),
                Span::styled(destination, Style::default().fg(COLOR_FILE)),
            ]),
            Line::raw(""),
            Line::styled(format!(" {}", status), Style::default().fg(status_color)),
            Line::from(vec![
                Span::styled(" Transferred: ", Style::default().fg(COLOR_MUTED)),
                Span::styled(transferred_line.clone(), Style::default().fg(COLOR_QUERY)),
                Span::styled(
                    format!("    {:.1}%", percentage),
                    Style::default()
                        .fg(COLOR_FRAME)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled(speed_label, Style::default().fg(COLOR_MUTED)),
                Span::styled(speed_text.clone(), Style::default().fg(COLOR_QUERY)),
            ]),
            Line::raw(""),
            Line::styled(
                format!(" {}", bar),
                Style::default().fg(if batch_failed {
                    COLOR_ERROR
                } else {
                    COLOR_FRAME
                }),
            ),
            Line::raw(""),
        ];

        if finished && batch_failed {
            lines.push(
                Line::styled(
                    format!(
                        " {} failed file{} remain{} marked for retry.",
                        transfer.failed_count,
                        if transfer.failed_count == 1 { "" } else { "s" },
                        if transfer.failed_count == 1 { "s" } else { "" },
                    ),
                    Style::default().fg(COLOR_ERROR),
                )
                .alignment(Alignment::Center),
            );
        } else if finished {
            lines.push(
                Line::styled(
                    "The batch download directory is ready.",
                    Style::default().fg(COLOR_QUERY),
                )
                .alignment(Alignment::Center),
            );
        } else {
            lines.push(Line::from(vec![
                Span::styled(" Current file: ", Style::default().fg(COLOR_MUTED)),
                Span::styled(transfer.filename.clone(), Style::default().fg(COLOR_FILE)),
            ]));
        }

        lines.push(Line::raw(""));

        let button = if finished || failed {
            Line::styled(
                "[ OK ]",
                Style::default()
                    .fg(app.theme.selection.text)
                    .bg(app.theme.selection.background)
                    .add_modifier(Modifier::BOLD),
            )
            .alignment(Alignment::Center)
        } else if cancelling {
            Line::styled("[ Cancelling… ]", Style::default().fg(COLOR_MUTED))
                .alignment(Alignment::Center)
        } else {
            Line::styled(
                "[ Cancel ]",
                Style::default()
                    .fg(app.theme.selection.text)
                    .bg(app.theme.selection.background)
                    .add_modifier(Modifier::BOLD),
            )
            .alignment(Alignment::Center)
        };

        lines.push(button);

        (title, lines)
    } else {
        let (title, status, status_color) = if single_failed {
            (
                " Transfer failed ",
                "The remote file could not be prepared.",
                COLOR_ERROR,
            )
        } else if finished {
            (
                " Transfer complete ",
                "The file is ready to open.",
                COLOR_QUERY,
            )
        } else if cancelling {
            (
                " Cancelling transfer ",
                "Stopping safely and removing the unfinished download…",
                COLOR_MUTED,
            )
        } else {
            (" Remote transfer ", "Transferring remote file…", COLOR_FILE)
        };

        let lines = vec![
            Line::raw(""),
            Line::from(vec![
                Span::styled(" File: ", Style::default().fg(COLOR_MUTED)),
                Span::styled(
                    transfer.filename.clone(),
                    Style::default().fg(COLOR_FILE).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled(" Size: ", Style::default().fg(COLOR_MUTED)),
                Span::styled(total_text, Style::default().fg(COLOR_QUERY)),
                Span::styled("    Elapsed: ", Style::default().fg(COLOR_MUTED)),
                Span::styled(elapsed_text, Style::default().fg(COLOR_QUERY)),
            ]),
            Line::raw(""),
            Line::styled(format!(" {}", status), Style::default().fg(status_color)),
            Line::from(vec![
                Span::styled(" Transferred: ", Style::default().fg(COLOR_MUTED)),
                Span::styled(transferred_line, Style::default().fg(COLOR_QUERY)),
                Span::styled(
                    format!("    {:.1}%", percentage),
                    Style::default()
                        .fg(COLOR_FRAME)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled(speed_label, Style::default().fg(COLOR_MUTED)),
                Span::styled(speed_text, Style::default().fg(COLOR_QUERY)),
            ]),
            Line::raw(""),
            Line::styled(
                format!(" {}", bar),
                Style::default().fg(if single_failed {
                    COLOR_ERROR
                } else {
                    COLOR_FRAME
                }),
            ),
            Line::raw(""),
            if finished || single_failed {
                Line::styled(
                    "[ OK ]",
                    Style::default()
                        .fg(app.theme.selection.text)
                        .bg(app.theme.selection.background)
                        .add_modifier(Modifier::BOLD),
                )
                .alignment(Alignment::Center)
            } else if cancelling {
                Line::styled("[ Cancelling… ]", Style::default().fg(COLOR_MUTED))
                    .alignment(Alignment::Center)
            } else {
                Line::styled(
                    "[ Cancel ]",
                    Style::default()
                        .fg(app.theme.selection.text)
                        .bg(app.theme.selection.background)
                        .add_modifier(Modifier::BOLD),
                )
                .alignment(Alignment::Center)
            },
        ];

        (title, lines)
    };

    let title = popup_content.0;

    let lines = popup_content.1;

    let popup = Paragraph::new(lines)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.frames.popup)),
        )
        .style(Style::default().bg(Color::Rgb(15, 16, 22)));

    frame.render_widget(Clear, popup_area);

    frame.render_widget(popup, popup_area);

    let button_label = if finished || failed {
        "[ OK ]"
    } else if cancelling {
        "[ Cancelling… ]"
    } else {
        "[ Cancel ]"
    };

    let button_width = button_label.chars().count() as u16;

    TransferUiRegions {
        action: Rect {
            x: popup_area
                .x
                .saturating_add(popup_area.width.saturating_sub(button_width) / 2),

            /*
             * The action button always occupies the final content row above
             * the lower popup border.
             */
            y: popup_area
                .y
                .saturating_add(popup_area.height.saturating_sub(3)),

            width: button_width,

            height: 1,
        },
    }
}

fn format_transfer_speed(bytes_per_second: f64) -> String {
    const KIB: f64 = 1024.0;

    const MIB: f64 = KIB * 1024.0;

    const GIB: f64 = MIB * 1024.0;

    if !bytes_per_second.is_finite() || bytes_per_second <= 0.0 {
        return "0 B/s".to_string();
    }

    if bytes_per_second < KIB {
        format!("{:.0} B/s", bytes_per_second,)
    } else if bytes_per_second < MIB {
        format!("{:.1} KiB/s", bytes_per_second / KIB,)
    } else if bytes_per_second < GIB {
        format!("{:.1} MiB/s", bytes_per_second / MIB,)
    } else {
        format!("{:.1} GiB/s", bytes_per_second / GIB,)
    }
}

fn format_duration(duration: std::time::Duration) -> String {
    let total_seconds = duration.as_secs();

    let minutes = total_seconds / 60;

    let seconds = total_seconds % 60;

    if minutes > 0 {
        format!("{}m {:02}s", minutes, seconds,)
    } else {
        format!("{}.{:01}s", seconds, duration.subsec_millis() / 100,)
    }
}

fn render_deletion_overlay(frame: &mut Frame, app: &App, area: Rect) {
    const POPUP_WIDTH: u16 = 74;

    const FILE_POPUP_HEIGHT: u16 = 12;

    const DIRECTORY_POPUP_HEIGHT: u16 = 14;

    let Some(deletion) = app.deletion.as_ref() else {
        return;
    };

    let popup_height =
        if deletion.is_directory && !deletion.is_symlink && deletion.directory_has_content {
            DIRECTORY_POPUP_HEIGHT
        } else {
            FILE_POPUP_HEIGHT
        };

    let popup_width = POPUP_WIDTH.min(area.width.saturating_sub(4).max(1));

    let popup_height = popup_height.min(area.height.saturating_sub(2).max(1));

    let popup_area = centered_rect(popup_width, popup_height, area);

    let target_kind = if deletion.is_symlink {
        "symbolic link"
    } else if deletion.is_directory {
        "directory"
    } else {
        "file"
    };

    let delete_focused = deletion.choice == DeletionChoice::Delete;

    let cancel_focused = deletion.choice == DeletionChoice::Cancel;

    let button_style = |focused: bool, dangerous: bool| {
        if focused {
            Style::default()
                .fg(app.theme.selection.text)
                .bg(if dangerous {
                    app.theme.ui.error
                } else {
                    app.theme.selection.background
                })
                .add_modifier(Modifier::BOLD)
        } else if dangerous {
            Style::default().fg(app.theme.ui.error)
        } else {
            Style::default().fg(app.theme.ui.file)
        }
    };

    let mut lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled(
                "Delete ",
                Style::default()
                    .fg(app.theme.ui.error)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                target_kind,
                Style::default().fg(app.theme.ui.classification),
            ),
            Span::raw("?"),
        ])
        .alignment(Alignment::Center),
        Line::raw(""),
        Line::from(vec![
            Span::styled("Name: ", Style::default().fg(app.theme.ui.muted)),
            Span::styled(
                deletion.name.clone(),
                Style::default()
                    .fg(app.theme.ui.file)
                    .add_modifier(Modifier::BOLD),
            ),
        ])
        .alignment(Alignment::Center),
        Line::from(vec![
            Span::styled("Path: ", Style::default().fg(app.theme.ui.muted)),
            Span::styled(
                deletion.path.display().to_string(),
                Style::default().fg(app.theme.ui.query),
            ),
        ])
        .alignment(Alignment::Center),
        Line::raw(""),
    ];

    if deletion.is_directory && !deletion.is_symlink && deletion.directory_has_content {
        lines.push(
            Line::styled(
                "This directory is not empty.",
                Style::default()
                    .fg(app.theme.ui.error)
                    .add_modifier(Modifier::BOLD),
            )
            .alignment(Alignment::Center),
        );

        lines.push(
            Line::styled(
                "Its complete contents will be removed.",
                Style::default().fg(app.theme.ui.error),
            )
            .alignment(Alignment::Center),
        );

        lines.push(Line::raw(""));
    }

    lines.push(
        Line::from(vec![
            Span::styled("[ Delete ]", button_style(delete_focused, true)),
            Span::raw("     "),
            Span::styled("[ Cancel ]", button_style(cancel_focused, false)),
        ])
        .alignment(Alignment::Center),
    );

    lines.push(Line::raw(""));

    lines.push(
        Line::styled(
            "←/→ or Tab selects · Enter confirms · Esc cancels",
            Style::default().fg(app.theme.ui.muted),
        )
        .alignment(Alignment::Center),
    );

    let popup = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Confirm Deletion ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.frames.popup)),
        )
        .style(Style::default().bg(Color::Rgb(15, 16, 22)));

    frame.render_widget(Clear, popup_area);

    frame.render_widget(popup, popup_area);
}

fn render_about_overlay(frame: &mut Frame, app: &App, area: Rect) {
    const POPUP_WIDTH: u16 = 76;

    const POPUP_HEIGHT: u16 = 20;

    let popup_area = centered_rect(
        POPUP_WIDTH.min(area.width.saturating_sub(4)).max(40),
        POPUP_HEIGHT.min(area.height.saturating_sub(2)).max(14),
        area,
    );

    let version = env!("CARGO_PKG_VERSION");

    let lines = vec![
        Line::raw(""),
        Line::styled(
            "Scry",
            Style::default()
                .fg(COLOR_QUERY)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center),
        Line::styled(
            "A Terminal File Browser",
            Style::default()
                .fg(COLOR_FRAME)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center),
        Line::raw(""),
        Line::styled(
            "Fast terminal file browser and recursive finder for local and remote filesystems.",
            Style::default().fg(COLOR_MUTED),
        )
        .alignment(Alignment::Center),
        Line::styled(
            "Browse, search, inspect metadata, sort entries, and open files directly.",
            Style::default().fg(COLOR_MUTED),
        )
        .alignment(Alignment::Center),
        Line::raw(""),
        Line::styled(
            "Made with Rust",
            Style::default()
                .fg(COLOR_QUERY)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center),
        Line::raw(""),
        about_information_line("Version", version),
        about_information_line("Author", "Markus Johnsson"),
        about_information_line("Copyright", "© 2026 Markus Johnsson"),
        about_information_line("License", "BSD 3-Clause"),
        about_information_line("SPDX", "BSD-3-Clause"),
        about_information_line("E-mail", "hedningakjetil@gmail.com"),
        about_information_line("Repository", "github.com/ferusx/scry-tui-file-browser"),
        Line::raw(""),
        Line::styled(
            "Alt+A / Esc / Enter to close",
            Style::default().fg(COLOR_MUTED),
        )
        .alignment(Alignment::Center),
    ];

    let popup = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" About Scry ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.frames.popup)),
        )
        .style(Style::default().bg(Color::Rgb(15, 16, 22)));

    frame.render_widget(Clear, popup_area);

    frame.render_widget(popup, popup_area);
}

fn about_information_line(label: &str, value: &str) -> Line<'static> {
    const LABEL_WIDTH: usize = 12;

    Line::from(vec![
        Span::styled(
            format!("{:>width$}: ", label, width = LABEL_WIDTH,),
            Style::default()
                .fg(COLOR_FRAME)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(value.to_string(), Style::default().fg(COLOR_FILE)),
    ])
    .alignment(Alignment::Center)
}

fn render_legend_overlay(frame: &mut Frame, app: &mut App, area: Rect) -> Option<Rect> {
    const POPUP_MAX_WIDTH: u16 = 70;

    const HORIZONTAL_MARGIN: u16 = 4;

    const VERTICAL_MARGIN: u16 = 2;

    let popup_width = area
        .width
        .saturating_sub(HORIZONTAL_MARGIN.saturating_mul(2))
        .clamp(34, POPUP_MAX_WIDTH);

    let popup_height = area
        .height
        .saturating_sub(VERTICAL_MARGIN.saturating_mul(2))
        .max(12);

    let popup_area = centered_rect(popup_width, popup_height, area);

    let mut lines: Vec<Line<'static>> = vec![Line::raw("")];

    let deletion_binding = if app.enable_deletion {
        ("Delete", "Delete the selected local entry")
    } else {
        ("Delete", "Delete sel. entry (enable via scry.toml)")
    };

    /*
     * Normal Mode contains the controls that form Scry's ordinary browsing
     * interface and the global controls that remain useful in every view.
     *
     * The conditional Delete entry must remain at index 6, immediately after
     * Enter and before Ctrl+T.
     */
    let mut normal_bindings = vec![
        ("↑ / ↓", "Move the selection"),
        ("PgUp / PgDn", "Move one visible page"),
        ("Home / End", "Select first or last entry"),
        ("Ctrl+← / Esc", "Enter the parent directory"),
        ("Ctrl+→", "Open the selected directory"),
        ("Enter", "Open or activate the selection"),
        ("Ctrl+T", "Enter Tree mode"),
        ("Alt+H", "Show or hide hidden entries"),
        ("Ctrl+O", "Cycle through sort modes"),
        ("Alt+R", "Toggle recursive mode"),
        ("Ctrl+R", "Reverse the sort direction"),
        ("Ctrl+D", "Show or hide Details"),
        ("Ctrl+S", "Show or hide Selection"),
        ("Alt+M", "Show or hide metadata"),
        ("F7", "Toggle Permissions column"),
        ("F8", "Toggle Size column"),
        ("F9", "Toggle Date column"),
        ("F10", "Toggle User column"),
        ("Ctrl+Y", "Copy the selected entry's full path"),
        ("F2", "Show detailed file information"),
        ("F4", "Open SSH connections manager"),
        ("Ctrl+!", "Open or close this window"),
        ("Alt+A", "Open the About window"),
        ("Ctrl+C", "Exit Scry"),
    ];

    normal_bindings.insert(6, deletion_binding);

    push_shortcut_section(&mut lines, "Normal Mode", &normal_bindings);

    /*
     * Tree Mode lists only the controls whose meaning changes when the
     * hierarchical view is active. All ordinary and global controls remain
     * documented once under Normal Mode above.
     */
    push_shortcut_section(
        &mut lines,
        "Tree Mode",
        &[
            ("↑ / ↓", "Move through visible nodes"),
            ("Ctrl+→", "Expand the selected directory"),
            ("Ctrl+← / Esc", "Collapse or select the parent"),
            ("Enter", "Make directory the new root"),
            ("Ctrl+T", "Return to List mode"),
        ],
    );

    /*
     * Search Mode lists only controls specifically concerned with query editing,
     * search policy, committing modifiers, and returning from search results.
     *
     * Backspace edits the query only and never navigates to the parent directory.
     */
    push_shortcut_section(
        &mut lines,
        "Search Mode",
        &[
            ("Type", "Filter or search entries"),
            ("Backspace", "Delete the character before the caret"),
            ("Ctrl+H", "Delete the character before the caret"),
            ("Ctrl+U", "Clear the complete search"),
            ("←", "Move left in the search field"),
            ("→", "Move right in the search field"),
            ("Ctrl+Home", "Move to the beginning of the search field"),
            ("Ctrl+End", "Move to the end of the search field"),
            ("Ctrl+F", "Toggle Fuzzy search"),
            ("Alt+R", "Toggle Recursive search"),
            ("Enter", "Commit a pending modifier or activate the result"),
            ("← / Esc", "Return to parent or previous search state"),
        ],
    );

    /*
     * Query syntax and type names come directly from query.rs.
     *
     * The parser and this reference therefore cannot drift apart.
     */
    push_shortcut_section(&mut lines, "Query Modifiers", QUERY_SYNTAX_REFERENCE);

    push_query_type_reference(&mut lines);

    /*
     * SSH controls apply regardless of whether the remote listing is currently
     * shown in List, Tree, or Search mode.
     */
    push_shortcut_section(
        &mut lines,
        "SSH",
        &[
            ("Ctrl+Space", "Mark or unmark the selected file"),
            ("Alt+U", "Clear all marked files"),
            ("Alt+D", "Download all marked files"),
        ],
    );

    push_shortcut_section(
        &mut lines,
        "Mouse",
        &[
            ("Wheel", "Move through entries"),
            ("Left-click", "Select an entry"),
            ("Double-click", "Activate the selected entry"),
            ("Middle-click", "Collapse or enter parent"),
            ("Scrollbar drag", "Move through long listings"),
            ("Popup buttons", "Activate visible actions"),
        ],
    );

    lines.push(Line::raw(""));

    lines.push(Line::styled(
        "  ↑/↓ scroll   PgUp/PgDn page   Ctrl+!/Esc close",
        Style::default().fg(COLOR_MUTED),
    ));

    let block = Block::default()
        .title(" Scry Shortcuts ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.frames.popup));

    let content_area = block.inner(popup_area);

    /*
     * The shortcut lines were assembled above specifically for this compact
     * legend. Do not replace them with the full Help document.
     */
    let viewport_height = content_area.height as usize;

    let content_height = lines.len();

    app.legend_max_scroll = content_height.saturating_sub(viewport_height) as u16;

    app.legend_scroll = app.legend_scroll.min(app.legend_max_scroll);

    let paragraph = Paragraph::new(lines)
        .block(block)
        .scroll((app.legend_scroll, 0))
        .style(Style::default().bg(Color::Rgb(15, 16, 22)));

    frame.render_widget(Clear, popup_area);

    frame.render_widget(paragraph, popup_area);

    if app.legend_max_scroll > 0 && content_area.height > 0 {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .thumb_symbol("█")
            .track_style(Style::default().fg(app.theme.scrollbar.track))
            .thumb_style(Style::default().fg(app.theme.scrollbar.thumb));

        let scrollbar_position = if app.legend_max_scroll == 0 {
            0
        } else {
            app.legend_scroll as usize * content_height.saturating_sub(1)
                / app.legend_max_scroll as usize
        };

        let mut scrollbar_state = ScrollbarState::new(content_height)
            .position(scrollbar_position)
            .viewport_content_length(viewport_height);

        frame.render_stateful_widget(scrollbar, content_area, &mut scrollbar_state);
    }

    if app.legend_max_scroll == 0 || content_area.height == 0 {
        None
    } else {
        Some(Rect {
            x: content_area
                .x
                .saturating_add(content_area.width.saturating_sub(1)),

            y: content_area.y,

            width: 1,

            height: content_area.height,
        })
    }
}

fn push_query_type_reference(lines: &mut Vec<Line<'static>>) {
    if lines.iter().any(|line| !line.spans.is_empty()) {
        lines.push(Line::raw(""));
    }

    lines.push(Line::styled(
        "  Type Values",
        Style::default()
            .fg(COLOR_FRAME)
            .add_modifier(Modifier::BOLD),
    ));

    lines.push(Line::styled(
        "  Use these values after type:. Aliases are shown on the right.",
        Style::default().fg(COLOR_MUTED),
    ));

    lines.push(Line::raw(""));

    for reference in QUERY_TYPE_REFERENCES {
        let aliases = if reference.aliases.is_empty() {
            "—".to_string()
        } else {
            format!("aliases: {}", reference.aliases.join(", "))
        };

        lines.push(query_type_reference_line(reference.canonical, &aliases));
    }
}

fn query_type_reference_line(canonical: &str, aliases: &str) -> Line<'static> {
    const TYPE_WIDTH: usize = 16;

    Line::from(vec![
        Span::styled(
            format!("  {:<width$}", canonical, width = TYPE_WIDTH),
            Style::default().fg(COLOR_QUERY),
        ),
        Span::styled(aliases.to_string(), Style::default().fg(COLOR_MUTED)),
    ])
}

fn push_shortcut_section(lines: &mut Vec<Line<'static>>, title: &str, bindings: &[(&str, &str)]) {
    if lines.iter().any(|line| !line.spans.is_empty()) {
        lines.push(Line::raw(""));
    }

    lines.push(Line::styled(
        format!("  {}", title),
        Style::default()
            .fg(COLOR_FRAME)
            .add_modifier(Modifier::BOLD),
    ));

    lines.push(Line::raw(""));

    for (shortcut, description) in bindings {
        lines.push(shortcut_help_line(shortcut, description));
    }
}

fn shortcut_help_line(shortcut: &str, description: &str) -> Line<'static> {
    const SHORTCUT_WIDTH: usize = 16;

    Line::from(vec![
        Span::styled(
            format!("  {:<width$}", shortcut, width = SHORTCUT_WIDTH),
            Style::default().fg(COLOR_QUERY),
        ),
        Span::styled(description.to_string(), Style::default().fg(COLOR_MUTED)),
    ])
}

fn render_help_overlay(frame: &mut Frame, app: &mut App, area: Rect) -> Option<Rect> {
    const POPUP_MAX_WIDTH: u16 = 82;

    const HORIZONTAL_MARGIN: u16 = 4;

    const VERTICAL_MARGIN: u16 = 2;

    let popup_width = area
        .width
        .saturating_sub(HORIZONTAL_MARGIN.saturating_mul(2))
        .clamp(34, POPUP_MAX_WIDTH);

    let popup_height = area
        .height
        .saturating_sub(VERTICAL_MARGIN.saturating_mul(2))
        .max(12);

    let popup_area = centered_rect(popup_width, popup_height, area);

    let block = Block::default()
        .title(" Scry Help ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.frames.popup));

    let content_area = block.inner(popup_area);

    /*
     * Give the document breathing room inside the popup.
     *
     * Two columns are reserved on the left and right, while one row is
     * reserved above and below the document.
     */
    let padded_area = content_area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });

    /*
     * The scrollbar occupies the far-right side of content_area rather
     * than the padded document area.
     */
    let document_area = Rect {
        x: padded_area.x,

        y: padded_area.y,

        width: padded_area.width.saturating_sub(1),

        height: padded_area.height,
    };

    let text_width = document_area.width as usize;

    let lines = help::content(&app.theme, text_width);

    let viewport_height = document_area.height as usize;

    let content_height = lines.len();

    app.help_max_scroll = content_height.saturating_sub(viewport_height) as u16;

    app.help_scroll = app.help_scroll.min(app.help_max_scroll);

    let background = Style::default().bg(Color::Rgb(15, 16, 22));

    let paragraph = Paragraph::new(lines)
        .scroll((app.help_scroll, 0))
        .style(background);

    frame.render_widget(Clear, popup_area);

    /*
     * Draw the popup background and border first.
     */
    frame.render_widget(block.style(background), popup_area);

    /*
     * Draw the scrollable document inside its padded rectangle.
     */
    frame.render_widget(paragraph, document_area);

    if app.help_max_scroll > 0 && document_area.height > 0 {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .thumb_symbol("█")
            .track_style(Style::default().fg(app.theme.scrollbar.track))
            .thumb_style(Style::default().fg(app.theme.scrollbar.thumb));

        let scrollbar_position = if app.help_max_scroll == 0 {
            0
        } else {
            /*
             * Ratatui's scrollbar position spans the complete content length,
             * whereas help_scroll spans only the valid viewport offsets.
             *
             * Scale between those two ranges so the thumb reaches the bottom
             * exactly when the content reaches its final viewport.
             */
            app.help_scroll as usize * content_height.saturating_sub(1)
                / app.help_max_scroll as usize
        };

        let mut scrollbar_state = ScrollbarState::new(content_height)
            .position(scrollbar_position)
            .viewport_content_length(viewport_height);

        frame.render_stateful_widget(scrollbar, content_area, &mut scrollbar_state);
    }

    if app.help_max_scroll == 0 || document_area.height == 0 {
        None
    } else {
        Some(Rect {
            x: content_area
                .x
                .saturating_add(content_area.width.saturating_sub(1)),

            y: content_area.y,

            width: 1,

            height: content_area.height,
        })
    }
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);

    let height = height.min(area.height);

    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,

        y: area.y + area.height.saturating_sub(height) / 2,

        width,

        height,
    }
}

fn render_footer(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let hidden_state = if app.show_hidden {
        "Hidden:on"
    } else {
        "Hidden:off"
    };

    #[allow(unused)]
    let details_state = if app.show_details {
        "details:on"
    } else {
        "details:off"
    };

    #[allow(unused)]
    let selection_state = if app.show_selection {
        "delect:on"
    } else {
        "delect:off"
    };

    let any_columns_enabled =
        app.show_permissions || app.show_size || app.show_date || app.show_user;


    let tree_state = if app.view_mode == ViewMode::Tree {
        "Tree:on"
    } else {
        "Tree:off"
    };

    #[allow(unused)]
    let columns_state = if !app.show_columns {
        "meta:off"
    } else if any_columns_enabled {
        "meta:on"
    } else {
        "meta:empty"
    };

    #[allow(unused)]
    let recursive_state = if app.recursive_mode {
        "recursive:on"
    } else {
        "recursive:off"
    };

    #[allow(unused)]
    let fuzzy_state = if app.search_mode == SearchMode::Fuzzy {
        "Fuzzy:on"
    } else {
        "Fuzzy:off"
    };

    #[allow(unused)]
    let reverse_state = if app.sort_descending {
        "reverse:on"
    } else {
        "reverse:off"
    };

    let footer = if !app.query.is_empty() {
        // Active Search Help Text
        format!(
            " ←/→ Move Cursor  Enter Open  Alt+R {}  Alt+H {}  ^U Clear  ^O Sort Mode  F2 Info  ^Y Copy",
            recursive_state, hidden_state,
        )
    } else if app.view_mode == ViewMode::Tree {
        // Tree View Help Text
        format!(
            " ^? Help  ^! Legend  ↑/↓/^←/^→ Move  Enter Open  F4 SSH  ^T {}  Alt+H {}  Alt+M Meta  ^C Exit",
            tree_state, hidden_state,
        )
    } else if app.source_is_remote() {
        // SSH Normal View Help Text
        format!(
            " ^! Legend  ^Space Select  Alt+U Clear Select  Enter Open  Alt+D Download  F4 SSH  ^T {}  ^C Exit",
            tree_state,
        )
    } else {
        // Normal View Help Text
        format!(
            " ^? Help  ^! Legend  ↑/↓/^←/^→ Move  Enter Open  F4 SSH  ^T {}  Alt+H {}  Alt+M Meta  ^C Exit",
            tree_state, hidden_state,
        )
    };
    let paragraph = Paragraph::new(footer).style(Style::default().fg(COLOR_MUTED));

    frame.render_widget(paragraph, area);
}

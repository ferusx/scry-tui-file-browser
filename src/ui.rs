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

use crate::app::{App, SearchMode, TreeRow, ViewMode};
use crate::connection::ConnectionField;
use crate::fuzzy::fuzzy_highlight_positions;
use crate::scan::FileEntry;

// SCROLLBAR CONSTANTS
const COLOR_SCROLLBAR_THUMB: Color = COLOR_FRAME;

const COLOR_SCROLLBAR_TRACK: Color = Color::Rgb(45, 50, 60);

// COLOR CONSTANTS
const COLOR_FRAME: Color = Color::Rgb(160, 110, 220);

const COLOR_DIRECTORY: Color = Color::Rgb(80, 155, 235);

const COLOR_FILE: Color = Color::Rgb(195, 200, 210);

const COLOR_SYMLINK: Color = Color::Rgb(75, 195, 210);

// const COLOR_QUERY: Color = Color::Rgb(110, 220, 225);

// const COLOR_SELECTED_BACKGROUND: Color = Color::Rgb(55, 40, 75);

const COLOR_SELECTED_TEXT: Color = Color::Rgb(240, 240, 245);

const COLOR_MUTED: Color = Color::Rgb(95, 105, 120);

const COLOR_ERROR: Color = Color::Rgb(220, 55, 70);

const COLOR_QUERY: Color = Color::Rgb(110, 220, 225);

const COLOR_MATCH: Color = Color::Rgb(166, 119, 199);

const COLOR_SELECTED_BACKGROUND: Color = Color::Rgb(55, 40, 75);

const COLOR_CLASSIFICATION: Color = Color::Rgb(240, 240, 245);

// const COLOR_PERMISSIONS: Color = COLOR_FRAME; // Color::Rgb(255, 255, 255);

const COLOR_DATE: Color = COLOR_DIRECTORY; // Color::Rgb(160, 110, 220);

const COLOR_USER: Color = Color::Rgb(91, 93, 99); //rgb(91, 93, 99)

const COLOR_SIZE: Color = COLOR_QUERY;

// INDIVIDUAL PERMISSION COLORS
const COLOR_PERMISSION_READ: Color = COLOR_MUTED;

const COLOR_PERMISSION_WRITE: Color = COLOR_DIRECTORY;

const COLOR_PERMISSION_EXECUTE: Color = COLOR_FRAME;

/*
 * Temporary neutral choices until we test alternatives.
 */
const COLOR_PERMISSION_TYPE: Color = COLOR_FILE;

const COLOR_PERMISSION_MISSING: Color = COLOR_MUTED;

const COLOR_PERMISSION_SPECIAL: Color = COLOR_CLASSIFICATION;

/*
 * Permissions and modification dates use fixed-width formats:
 *
 *     .rwxr-xr-x
 *     2026-07-16 10:42
 */
const PERMISSIONS_COLUMN_WIDTH: u16 = 10;

const DATE_COLUMN_WIDTH: u16 = 16;

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
}

#[derive(Debug, Clone, Copy, Default)]
pub struct UiRegions {
    pub entries: Rect,

    pub help_scrollbar: Option<Rect>,

    pub connection: Option<ConnectionUiRegions>,

    pub transfer: Option<TransferUiRegions>,
}

pub fn render(frame: &mut Frame, app: &mut App) -> UiRegions {
    let mut help_scrollbar_region = None;

    let mut connection_regions = None;

    let mut transfer_regions = None;

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

    if let Some(area) = selection_area {
        render_selection(frame, app, area);
    }

    render_footer(frame, app, footer_area);

    if app.help_visible() {
        help_scrollbar_region = render_help_overlay(frame, app, frame.area());
    }

    if app.about_visible() {
        render_about_overlay(frame, frame.area());
    }

    if app.connection_visible() {
        connection_regions = Some(render_connection_overlay(frame, app, frame.area()));
    }

    if app.transfer_visible() {
        transfer_regions = Some(render_transfer_overlay(frame, app, frame.area()));
    }

    UiRegions {
        entries: entries_area,

        help_scrollbar: help_scrollbar_region,

        connection: connection_regions,

        transfer: transfer_regions,
    }
}

fn render_search(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let title = format!(
        " Scry — {} — {} ",
        app.source_label(),
        app.current_directory.display(),
    );

    let mode_label = match (app.search_mode, app.recursive_mode) {
        (SearchMode::Exact, false) => "Exact",

        (SearchMode::Fuzzy, false) => "Fuzzy",

        (SearchMode::Exact, true) => "Recursive",

        (SearchMode::Fuzzy, true) => "Fuzzy+Recursive",
    };

    let placeholder = match (app.search_mode, app.recursive_mode) {
        (SearchMode::Exact, false) => r#"type to filter — e.g. "hello", "world", "/etc""#,

        (SearchMode::Fuzzy, false) => r#"type to search fuzzily — e.g. "help", "hlep", "hlp""#,

        (SearchMode::Exact, true) => r#"type to filter recursively — e.g. "config", ".rs", "/etc""#,

        (SearchMode::Fuzzy, true) => r#"type to search recursively — e.g. "help", "hlep", "hlp""#,
    };

    let emphasized_mode = app.search_mode == SearchMode::Fuzzy || app.recursive_mode;

    let mode_color = if emphasized_mode {
        COLOR_QUERY
    } else {
        COLOR_MUTED
    };

    let search = Paragraph::new(Line::from(vec![
        Span::styled("Search [", Style::default().fg(COLOR_MUTED)),
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
        Span::styled("]: ", Style::default().fg(COLOR_MUTED)),
        Span::styled(
            if app.query.is_empty() {
                placeholder
            } else {
                &app.query
            },
            Style::default().fg(COLOR_QUERY),
        ),
    ]))
    .block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_FRAME)),
    );

    frame.render_widget(search, area);
}

fn render_details(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .title(" Details ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_FRAME));

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
                Style::default().fg(COLOR_MUTED),
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
        COLOR_DIRECTORY
    } else if entry.is_symlink {
        COLOR_SYMLINK
    } else {
        COLOR_FILE
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

    render_detail_name(frame, first_row[0], &entry, app.show_icons, name_color);

    render_detail_value(
        frame,
        first_row[1],
        "Type",
        classification,
        COLOR_CLASSIFICATION,
    );

    render_detail_value(frame, first_row[2], "Size", size, COLOR_SIZE);

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
        COLOR_DATE,
    );

    render_detail_value(frame, second_row[1], "Age", age, COLOR_QUERY);

    render_detail_value(frame, second_row[2], "Owner", owner, COLOR_USER);

    /*
     * Row three:
     *
     * Permissions | full path
     */
    let third_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(rows[2]);

    render_detail_permissions(frame, third_row[0], &entry.permissions);

    render_detail_value(
        frame,
        third_row[1],
        "Path",
        entry.path.display().to_string(),
        name_color,
    );
}

fn render_detail_name(
    frame: &mut Frame,
    area: Rect,
    entry: &FileEntry,
    show_icons: bool,
    name_color: Color,
) {
    let mut spans = vec![Span::styled(" Name: ", Style::default().fg(COLOR_MUTED))];

    if show_icons {
        spans.push(Span::styled(
            format!("{} ", file_icon(entry)),
            Style::default().fg(file_icon_color(entry)),
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
) {
    let line = Line::from(vec![
        Span::styled(format!(" {}: ", label,), Style::default().fg(COLOR_MUTED)),
        Span::styled(value, Style::default().fg(value_color)),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}

fn render_detail_permissions(frame: &mut Frame, area: Rect, permissions: &str) {
    let mut spans = vec![Span::styled(
        " Permissions: ",
        Style::default().fg(COLOR_MUTED),
    )];

    /*
     * Reuse the same per-character permission palette as the metadata column.
     */
    spans.extend(permission_spans(permissions));

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
        format!("{}", format_age_duration(age,),)
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
     * Two border cells plus one padding cell on each side.
     */
    metadata_content_width(app, widths) + 4
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
         * Wraith's UID cache.
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
                .border_style(Style::default().fg(COLOR_FRAME)),
        )
        .highlight_style(
            Style::default()
                .bg(COLOR_SELECTED_BACKGROUND)
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
        spans.extend(permission_spans(permissions));

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

fn permission_spans(permissions: &str) -> Vec<Span<'static>> {
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
                Style::default().fg(COLOR_PERMISSION_TYPE)
            } else {
                match character {
                    'r' => Style::default().fg(COLOR_PERMISSION_READ),

                    'w' => Style::default().fg(COLOR_PERMISSION_WRITE),

                    'x' => Style::default().fg(COLOR_PERMISSION_EXECUTE),

                    's' | 'S' | 't' | 'T' => Style::default()
                        .fg(COLOR_PERMISSION_SPECIAL)
                        .add_modifier(Modifier::BOLD),

                    '-' => Style::default().fg(COLOR_PERMISSION_MISSING),

                    _ => Style::default().fg(COLOR_PERMISSION_TYPE),
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

    let highlight_query = if app.query == "." {
        String::new()
    } else {
        app.query.clone()
    };

    let search_mode = app.search_mode;

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

        let has_content = entry.is_directory && app.directory_has_content(&entry.path);

        items.push(entry_list_item(
            &entry,
            &highlight_query,
            search_mode,
            has_content,
            app.show_icons,
        ));
    }

    let heading = if app.search_mode == SearchMode::Fuzzy && app.fuzzy_filter_in_progress {
        format!(
            "Fuzzy results — updating… — best {}",
            app.filtered_indices.len(),
        )
    } else if app.recursive_search_active() {
        if app.scan_in_progress {
            format!(
                "Recursive results — scanning {} entries…",
                app.active_entry_count(),
            )
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

    let title = format!(
        " {} — {} shown / {} scanned — {} {} ",
        heading,
        app.filtered_indices.len(),
        app.active_entry_count(),
        app.sort_mode.label(),
        sort_arrow,
    );

    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_FRAME)),
        )
        .highlight_symbol("▶ ")
        .highlight_style(
            Style::default()
                .fg(COLOR_SELECTED_TEXT)
                .bg(COLOR_SELECTED_BACKGROUND)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = ListState::default();

    if !app.filtered_indices.is_empty() {
        state.select(Some(app.selected.saturating_sub(window_start)));
    }

    frame.render_stateful_widget(list, area, &mut state);

    render_entries_scrollbar(
        frame,
        area,
        app.filtered_indices.len(),
        visible_rows,
        app.selected,
    );
}

fn render_entries_scrollbar(
    frame: &mut Frame,
    area: Rect,
    content_length: usize,
    viewport_length: usize,
    position: usize,
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
        .track_style(Style::default().fg(COLOR_SCROLLBAR_TRACK))
        .thumb_style(Style::default().fg(COLOR_SCROLLBAR_THUMB));

    let mut scrollbar_state = ScrollbarState::new(content_length)
        .position(position)
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

    let highlight_query = if app.query == "." {
        String::new()
    } else {
        app.query.clone()
    };

    let mut items: Vec<ListItem> = Vec::new();

    for position in window_start..window_end {
        let Some(row) = app.tree_row_at_filtered_position(position).cloned() else {
            continue;
        };

        let has_content = row.entry.is_directory && app.directory_has_content(&row.entry.path);

        items.push(tree_list_item(
            &row,
            &highlight_query,
            has_content,
            app.show_icons,
        ));
    }

    let sort_arrow = if app.sort_descending { "↓" } else { "↑" };

    let title = if app.scan_in_progress && app.recursive_search_active() {
        format!(
            " Recursive Tree — scanning {} entries… — {} {} ",
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

        format!(
            " {} — {} shown / {} expanded nodes — {} {} ",
            tree_kind,
            app.filtered_tree_indices.len(),
            app.tree_rows.len(),
            app.sort_mode.label(),
            sort_arrow,
        )
    };

    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_FRAME)),
        )
        .highlight_symbol("▶ ")
        .highlight_style(
            Style::default()
                .fg(COLOR_SELECTED_TEXT)
                .bg(COLOR_SELECTED_BACKGROUND)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = ListState::default();

    if !app.filtered_tree_indices.is_empty() {
        state.select(Some(app.selected.saturating_sub(window_start)));
    }

    frame.render_stateful_widget(list, area, &mut state);

    render_entries_scrollbar(
        frame,
        area,
        app.filtered_tree_indices.len(),
        visible_rows,
        app.selected,
    );
}

fn tree_list_item(
    row: &TreeRow,
    query: &str,
    has_content: bool,
    show_icons: bool,
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
            Style::default().fg(file_icon_color(&row.entry)),
        ));
    }

    spans.extend(highlighted_name_spans(&row.entry.name, query, color));

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

fn file_icon_color(entry: &FileEntry) -> Color {
    use crate::classify::FileClass;

    if entry.is_directory {
        return COLOR_DIRECTORY;
    }

    if entry.is_symlink {
        return COLOR_SYMLINK;
    }

    match entry.class {
        FileClass::Rust => Color::Rgb(230, 125, 70),

        FileClass::Python => Color::Rgb(80, 170, 235),

        FileClass::ShellScript | FileClass::Executable => Color::Rgb(90, 200, 125),

        FileClass::C | FileClass::Cpp | FileClass::SourceCode => Color::Rgb(105, 145, 225),

        FileClass::Java | FileClass::Kotlin => Color::Rgb(220, 105, 85),

        FileClass::JavaScript | FileClass::TypeScript => Color::Rgb(235, 205, 65),

        FileClass::Web => Color::Rgb(210, 100, 190),

        FileClass::Config | FileClass::StructuredData | FileClass::Build => {
            Color::Rgb(80, 185, 205)
        }

        FileClass::Archive | FileClass::Package => Color::Rgb(215, 135, 80),

        FileClass::Document | FileClass::Text => Color::Rgb(195, 205, 220),

        FileClass::Spreadsheet => Color::Rgb(70, 195, 115),

        FileClass::Presentation => Color::Rgb(230, 135, 70),

        FileClass::Image | FileClass::VectorImage => Color::Rgb(215, 105, 220),

        FileClass::Audio => Color::Rgb(105, 165, 225),

        FileClass::Video => Color::Rgb(195, 100, 220),

        FileClass::Font => Color::Rgb(195, 145, 225),

        FileClass::Database => Color::Rgb(70, 190, 205),

        FileClass::Log => Color::Rgb(155, 165, 180),

        FileClass::Backup => Color::Rgb(175, 125, 195),

        FileClass::Certificate => Color::Rgb(225, 190, 75),

        FileClass::DiskImage => Color::Rgb(125, 155, 215),

        FileClass::Torrent => Color::Rgb(80, 190, 145),

        FileClass::DesktopEntry | FileClass::Plugin => Color::Rgb(155, 130, 220),

        FileClass::Binary | FileClass::Unknown => COLOR_FILE,

        FileClass::Directory | FileClass::Symlink => COLOR_FILE,
    }
}

fn entry_list_item(
    entry: &FileEntry,
    query: &str,
    search_mode: SearchMode,
    has_content: bool,
    show_icons: bool,
) -> ListItem<'static> {
    let (prefix, color, suffix) = if entry.is_directory {
        ("▸ ", COLOR_DIRECTORY, if has_content { " →" } else { "/" })
    } else if entry.is_symlink {
        ("↪ ", COLOR_SYMLINK, "@")
    } else {
        ("  ", COLOR_FILE, "")
    };

    let mut spans = vec![Span::styled(prefix.to_string(), Style::default().fg(color))];

    if show_icons {
        spans.push(Span::styled(
            format!("{} ", file_icon(entry)),
            Style::default().fg(file_icon_color(entry)),
        ));
    }

    let display_path = entry.relative_path.to_string_lossy().into_owned();

    match search_mode {
        SearchMode::Exact => {
            spans.extend(highlighted_name_spans(&display_path, query, color));
        }

        SearchMode::Fuzzy => {
            let positions = fuzzy_highlight_positions(&display_path, query);

            spans.extend(highlighted_position_spans(&display_path, &positions, color));
        }
    }

    if !suffix.is_empty() {
        spans.push(Span::styled(suffix.to_string(), Style::default().fg(color)));
    }

    ListItem::new(Line::from(spans))
}

fn highlighted_position_spans(
    text: &str,
    highlighted_positions: &[usize],
    normal_color: Color,
) -> Vec<Span<'static>> {
    if highlighted_positions.is_empty() {
        return vec![Span::styled(
            text.to_string(),
            Style::default().fg(normal_color),
        )];
    }

    let highlighted: std::collections::HashSet<usize> =
        highlighted_positions.iter().copied().collect();

    let mut spans = Vec::new();

    let mut current_text = String::new();

    let mut current_is_highlighted = None;

    for (position, character) in text.chars().enumerate() {
        let is_highlighted = highlighted.contains(&position);

        if current_is_highlighted.is_some_and(|current| current != is_highlighted) {
            spans.push(Span::styled(
                std::mem::take(&mut current_text),
                Style::default().fg(if current_is_highlighted == Some(true) {
                    COLOR_MATCH
                } else {
                    normal_color
                }),
            ));
        }

        current_is_highlighted = Some(is_highlighted);

        current_text.push(character);
    }

    if !current_text.is_empty() {
        spans.push(Span::styled(
            current_text,
            Style::default().fg(if current_is_highlighted == Some(true) {
                COLOR_MATCH
            } else {
                normal_color
            }),
        ));
    }

    spans
}

fn highlighted_name_spans(name: &str, query: &str, normal_color: Color) -> Vec<Span<'static>> {
    if query.is_empty() {
        return vec![Span::styled(
            name.to_string(),
            Style::default().fg(normal_color),
        )];
    }

    let folded_name = fold_with_source_ranges(name);
    let folded_query: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();

    if folded_query.is_empty() || folded_query.len() > folded_name.len() {
        return vec![Span::styled(
            name.to_string(),
            Style::default().fg(normal_color),
        )];
    }

    let mut matches = Vec::new();
    let mut search_index = 0;

    while search_index + folded_query.len() <= folded_name.len() {
        let matches_query = folded_name[search_index..search_index + folded_query.len()]
            .iter()
            .map(|(character, _, _)| *character)
            .eq(folded_query.iter().copied());

        if matches_query {
            let byte_start = folded_name[search_index].1;
            let byte_end = folded_name[search_index + folded_query.len() - 1].2;

            matches.push((byte_start, byte_end));

            search_index += folded_query.len();
        } else {
            search_index += 1;
        }
    }

    if matches.is_empty() {
        return vec![Span::styled(
            name.to_string(),
            Style::default().fg(normal_color),
        )];
    }

    let mut spans = Vec::new();
    let mut previous_end = 0;

    for (match_start, match_end) in matches {
        if previous_end < match_start {
            spans.push(Span::styled(
                name[previous_end..match_start].to_string(),
                Style::default().fg(normal_color),
            ));
        }

        spans.push(Span::styled(
            name[match_start..match_end].to_string(),
            Style::default().fg(COLOR_MATCH),
        ));

        previous_end = match_end;
    }

    if previous_end < name.len() {
        spans.push(Span::styled(
            name[previous_end..].to_string(),
            Style::default().fg(normal_color),
        ));
    }

    spans
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
    let selected_classification = app.selected_classification();

    let content = if let Some(error) = &app.error_message {
        Line::styled(error.clone(), Style::default().fg(COLOR_ERROR))
    } else if let Some(entry) = app.selected_entry() {
        Line::styled(
            entry.path.display().to_string(),
            Style::default().fg(if entry.is_directory {
                COLOR_DIRECTORY
            } else {
                COLOR_FILE
            }),
        )
    } else {
        Line::styled("No matching entries", Style::default().fg(COLOR_MUTED))
    };

    let title = if let Some(class) = selected_classification {
        Line::from(vec![
            Span::styled(" Selection — ", Style::default().fg(COLOR_FRAME)),
            Span::styled(
                class.label(),
                Style::default()
                    .fg(COLOR_CLASSIFICATION)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", Style::default().fg(COLOR_FRAME)),
        ])
    } else {
        Line::from(Span::styled(
            " Selection ",
            Style::default().fg(COLOR_FRAME),
        ))
    };

    let paragraph = Paragraph::new(content).block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_FRAME)),
    );

    frame.render_widget(paragraph, area);
}

fn render_connection_overlay(frame: &mut Frame, app: &App, area: Rect) -> ConnectionUiRegions {
    const POPUP_WIDTH: u16 = 76;

    const POPUP_HEIGHT: u16 = 24;

    let popup_area = centered_rect(POPUP_WIDTH, POPUP_HEIGHT, area);

    let profiles = app.connection_store.profiles();

    let selected_profile = app.connection_dialog.selected_profile;

    let draft = &app.connection_dialog.draft;

    let profile_summary = if profiles.is_empty() {
        Line::styled("  No saved profiles", Style::default().fg(COLOR_MUTED))
    } else {
        let profile_name = profiles
            .get(selected_profile)
            .map(|profile| profile.name.as_str())
            .unwrap_or("—");

        Line::from(vec![
            Span::styled("  Saved profile: ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                profile_name.to_string(),
                Style::default()
                    .fg(COLOR_QUERY)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "  ({}/{})",
                    selected_profile.saturating_add(1),
                    profiles.len(),
                ),
                Style::default().fg(COLOR_MUTED),
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
        ),
        connection_field_line(
            "Host / IP",
            &draft.host,
            None,
            ConnectionField::Host,
            app.connection_dialog.focus,
        ),
        connection_field_line(
            "Username",
            &draft.username,
            None,
            ConnectionField::Username,
            app.connection_dialog.focus,
        ),
        connection_field_line(
            "Port",
            &app.connection_dialog.port_text,
            None,
            ConnectionField::Port,
            app.connection_dialog.focus,
        ),
        connection_field_line(
            "Identity file",
            &draft.identity_file,
            Some("Optional — e.g. ~/.ssh/id_ed25519"),
            ConnectionField::IdentityFile,
            app.connection_dialog.focus,
        ),
        connection_field_line(
            "Start directory",
            &draft.start_directory,
            None,
            ConnectionField::StartDirectory,
            app.connection_dialog.focus,
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
            ),
            Span::raw("   "),
            connection_button_span(
                "Save",
                ConnectionField::Save,
                app.connection_dialog.focus,
                true,
            ),
            Span::raw("   "),
            connection_button_span(
                "Delete",
                ConnectionField::Delete,
                app.connection_dialog.focus,
                !profiles.is_empty(),
            ),
            Span::raw("   "),
            connection_button_span(
                "Disconnect",
                ConnectionField::Disconnect,
                app.connection_dialog.focus,
                app.source_is_remote(),
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
        Line::raw(""),
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
    ];

    let popup = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" SSH Connections ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_FRAME)),
        )
        .style(Style::default().bg(Color::Rgb(15, 16, 22)));

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
    }
}

fn connection_field_line(
    label: &str,
    value: &str,
    placeholder: Option<&str>,
    field: ConnectionField,
    focused_field: ConnectionField,
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
            .fg(COLOR_SELECTED_TEXT)
            .bg(Color::Rgb(38, 40, 50))
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
) -> Span<'static> {
    let focused = field == focused_field;

    let style = if !enabled {
        Style::default().fg(COLOR_MUTED)
    } else if focused {
        Style::default()
            .fg(COLOR_SELECTED_TEXT)
            .bg(COLOR_SELECTED_BACKGROUND)
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
    const POPUP_WIDTH: u16 = 64;

    const POPUP_HEIGHT: u16 = 14;

    /*
     * The popup has two border cells and the bar line has one leading space.
     */
    const BAR_WIDTH: usize = 59;

    let popup_area = centered_rect(POPUP_WIDTH, POPUP_HEIGHT, area);

    let Some(transfer) = app.transfer.as_ref() else {
        return TransferUiRegions::default();
    };

    let elapsed = app.transfer_elapsed();

    let elapsed_text = format_duration(elapsed);

    let total_bytes = transfer.total_bytes;

    let transferred_bytes = transfer.transferred_bytes.min(total_bytes);

    let total_text = format_file_size(total_bytes);

    let transferred_size_text = format_file_size(transferred_bytes);

    let finished = transfer.finished_elapsed.is_some();

    let failed = transfer.error.is_some();

    let cancelling = transfer.cancel_requested && !finished;

    let percentage = if total_bytes == 0 {
        if finished && !failed { 100.0 } else { 0.0 }
    } else {
        transferred_bytes as f64 * 100.0 / total_bytes as f64
    };

    let percentage = percentage.clamp(0.0, 100.0);

    let filled_cells = if total_bytes == 0 {
        if finished && !failed { BAR_WIDTH } else { 0 }
    } else {
        (transferred_bytes as u128 * BAR_WIDTH as u128 / total_bytes as u128) as usize
    }
    .min(BAR_WIDTH);

    let bar = format!(
        "{}{}",
        "█".repeat(filled_cells),
        "░".repeat(BAR_WIDTH.saturating_sub(filled_cells,),),
    );

    let seconds = elapsed.as_secs_f64();

    let speed_bytes_per_second = if seconds > 0.0 {
        transferred_bytes as f64 / seconds
    } else {
        0.0
    };

    let speed_text = format_transfer_speed(speed_bytes_per_second);

    let (title, status, status_color) = if failed {
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

    let transferred_line = if failed {
        format!("{} / {}", transferred_size_text, total_text,)
    } else {
        format!("{} / {}", transferred_size_text, total_text,)
    };

    let speed_label = if finished {
        " Average speed: "
    } else {
        " Speed: "
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
                format!("    {:.1}%", percentage,),
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
            Style::default().fg(if failed { COLOR_ERROR } else { COLOR_FRAME }),
        ),
        Line::raw(""),
        if finished || failed {
            Line::styled(
                "[ OK ]",
                Style::default()
                    .fg(COLOR_SELECTED_TEXT)
                    .bg(COLOR_SELECTED_BACKGROUND)
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
                    .fg(COLOR_SELECTED_TEXT)
                    .bg(COLOR_SELECTED_BACKGROUND)
                    .add_modifier(Modifier::BOLD),
            )
            .alignment(Alignment::Center)
        },
    ];

    let popup = Paragraph::new(lines)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(if failed { COLOR_ERROR } else { COLOR_FRAME })),
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
             * The action is the eleventh content line. The popup border occupies
             * the preceding outer row.
             */
            y: popup_area.y.saturating_add(11),

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

fn render_about_overlay(frame: &mut Frame, area: Rect) {
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
                .border_style(Style::default().fg(COLOR_FRAME)),
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

fn render_help_overlay(frame: &mut Frame, app: &mut App, area: Rect) -> Option<Rect> {
    const POPUP_MAX_WIDTH: u16 = 58;

    const HORIZONTAL_MARGIN: u16 = 4;

    const VERTICAL_MARGIN: u16 = 2;

    let popup_width = area
        .width
        .saturating_sub(HORIZONTAL_MARGIN.saturating_mul(2))
        .min(POPUP_MAX_WIDTH)
        .max(34);

    let popup_height = area
        .height
        .saturating_sub(VERTICAL_MARGIN.saturating_mul(2))
        .max(12);

    let popup_area = centered_rect(popup_width, popup_height, area);

    let mut lines: Vec<Line<'static>> = Vec::new();

    push_shortcut_section(
        &mut lines,
        "Normal Mode",
        &[
            ("↑ / ↓", "Move the selection"),
            ("PgUp / PgDn", "Move one visible page"),
            ("Home / End", "Select first or last entry"),
            ("← / Esc", "Enter the parent directory"),
            ("→", "Open the selected directory"),
            ("Enter", "Open or activate the selection"),
            ("Ctrl+T", "Enter Tree mode"),
            ("Alt+H", "Show or hide hidden entries"),
            ("Ctrl+O", "Cycle through sort modes"),
            ("Alt+R", "Toggle recursive mode"),
            ("Ctrl+R", "Reverse the sort direction"),
            ("Ctrl+D", "Show or hide Details"),
            ("Ctrl+S", "Show or hide Selection"),
            ("Alt+M", "Show or hide metadata"),
            ("F4", "Open SSH connections manager"),
            ("?", "Open or close this window"),
            ("Alt+A", "Open the About window"),
            ("Ctrl+C", "Exit Scry"),
        ],
    );

    push_shortcut_section(
        &mut lines,
        "Tree Mode",
        &[
            ("↑ / ↓", "Move through visible nodes"),
            ("PgUp / PgDn", "Move one visible page"),
            ("Home / End", "Select first or last node"),
            ("→", "Expand the selected directory"),
            ("← / Esc", "Collapse or select the parent"),
            ("Enter", "Make directory the new root"),
            ("Ctrl+T", "Return to List mode"),
            ("Alt+H", "Show or hide hidden entries"),
            ("Ctrl+O", "Cycle through sort modes"),
            ("Alt+R", "Toggle recursive mode"),
            ("Ctrl+R", "Reverse the sort direction"),
            ("Ctrl+D", "Show or hide Details"),
            ("Ctrl+S", "Show or hide Selection"),
            ("Alt+M", "Show or hide metadata"),
            ("F4", "Open SSH connections manager"),
            ("?", "Open or close this window"),
            ("Alt+A", "Open the About window"),
            ("Ctrl+C", "Exit Scry"),
        ],
    );

    push_shortcut_section(
        &mut lines,
        "Search Mode",
        &[
            ("Type", "Filter or search entries"),
            ("↑ / ↓", "Move through results"),
            ("PgUp / PgDn", "Move one visible page"),
            ("Home / End", "Select first or last result"),
            ("Backspace", "Delete one search character"),
            ("Ctrl+H", "Delete one character"),
            ("Alt+H", "Show or hide hidden entries"),
            ("Ctrl+U", "Clear the complete search"),
            ("Enter", "Open or locate the result"),
            ("← / Esc", "Enter the parent directory"),
            ("Alt+R", "Toggle recursive mode"),
            ("Ctrl+R", "Reverse the sort direction"),
            ("Ctrl+D", "Show or hide Details"),
            ("Ctrl+S", "Show or hide Selection"),
            ("Alt+M", "Show or hide metadata"),
            ("F4", "Open SSH connections manager"),
            ("?", "Open or close this window"),
            ("Alt+A", "Open the About window"),
            ("Ctrl+C", "Exit Scry"),
        ],
    );

    push_shortcut_section(
        &mut lines,
        "Mouse",
        &[
            ("Wheel", "Move through entries"),
            ("Left-click", "Select an entry"),
            ("Double-click", "Activate the selected entry"),
            ("Right-click", "Collapse or enter parent"),
            ("Scrollbar drag", "Move through long listings"),
            ("Popup buttons", "Activate visible actions"),
        ],
    );

    lines.push(Line::raw(""));

    lines.push(Line::styled(
        "  ↑/↓ scroll   PgUp/PgDn page   ?/Esc close",
        Style::default().fg(COLOR_MUTED),
    ));

    let block = Block::default()
        .title(" Scry Shortcuts ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_FRAME));

    let content_area = block.inner(popup_area);

    let viewport_height = content_area.height as usize;

    let content_height = lines.len();

    app.help_max_scroll = content_height.saturating_sub(viewport_height) as u16;

    app.help_scroll = app.help_scroll.min(app.help_max_scroll);

    let paragraph = Paragraph::new(lines)
        .block(block)
        .scroll((app.help_scroll, 0))
        .style(Style::default().bg(Color::Rgb(15, 16, 22)));

    frame.render_widget(Clear, popup_area);

    frame.render_widget(paragraph, popup_area);

    if app.help_max_scroll > 0 && content_area.height > 0 {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .thumb_symbol("█")
            .track_style(Style::default().fg(COLOR_SCROLLBAR_TRACK))
            .thumb_style(Style::default().fg(COLOR_SCROLLBAR_THUMB));

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

    if app.help_max_scroll == 0 || content_area.height == 0 {
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

fn push_shortcut_section(lines: &mut Vec<Line<'static>>, title: &str, bindings: &[(&str, &str)]) {
    if !lines.is_empty() {
        lines.push(Line::raw(""));
    }

    lines.push(Line::styled(
        format!("  {}", title,),
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
            format!("  {:<width$}", shortcut, width = SHORTCUT_WIDTH,),
            Style::default().fg(COLOR_QUERY),
        ),
        Span::styled(description.to_string(), Style::default().fg(COLOR_MUTED)),
    ])
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

    let details_state = if app.show_details {
        "Details:on"
    } else {
        "Details:off"
    };

    let selection_state = if app.show_selection {
        "Select:on"
    } else {
        "Select:off"
    };

    let any_columns_enabled =
        app.show_permissions || app.show_size || app.show_date || app.show_user;

    let columns_state = if !app.show_columns {
        "Meta:off"
    } else if any_columns_enabled {
        "Meta:on"
    } else {
        "Meta:empty"
    };

    let recursive_state = if app.recursive_mode {
        "recursive:on"
    } else {
        "recursive:off"
    };

    let footer = if app.view_mode == ViewMode::Tree {
        // Tree View Help Text
        format!(
            " ^? More  F4 SSH  ↑/↓ Move  ← Collapse  → Expand  Enter Select/open  Alt+H {}  ^C Exit ",
            hidden_state,
        )
    } else if app.query.is_empty() {
        // Normal View Help Text
        format!(
            " ^? More  F4 SSH  Enter Select/open  Alt+H {}  ^D {} ^S {}  Alt+M {}  ^C Exit ",
            hidden_state, details_state, selection_state, columns_state,
        )
    } else {
        // Active Search Help Text
        format!(
            " ^? More  ↑/↓ Move  PgUp ↑  PgDn ↓  Enter Select/open  Alt+R {}  ^U Clear  ^C Exit ",
            recursive_state,
        )
    };

    let paragraph = Paragraph::new(footer).style(Style::default().fg(COLOR_MUTED));

    frame.render_widget(paragraph, area);
}

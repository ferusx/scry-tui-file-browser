// SPDX-License-Identifier: BSD-3-Clause

use std::env;
use std::io::{self, IsTerminal, Write};
use crossterm::terminal;
use ratatui::{
    layout::Alignment,
    style::Modifier,
    text::Line,
};

use crate::help;
use crate::themes::Theme;
use crate::external_help::{EXAMPLES, OPTIONS};

const MANUAL_ACCENT: &str = "\x1b[36m";
const MANUAL_TEXT: &str = "\x1b[37m";
const MANUAL_EMPHASIS: &str = "\x1b[1;97m";
const MANUAL_TITLE: &str = "\x1b[1;96m";
const MANUAL_TIP: &str = "\x1b[32m";
const MANUAL_BORDER: &str = "\x1b[1;30m";
const MANUAL_RESET: &str = "\x1b[0m";

#[derive(Debug, Clone, Copy)]
struct Palette {
    enabled: bool,
}

impl Palette {
    fn new() -> Self {
        Self {
            enabled: colors_enabled(),
        }
    }


    fn paint(
        self,
        value: &str,
        style: &str,
    ) -> String {
        if self.enabled {
            format!(
                "{}{}{}",
                style,
                value,
                MANUAL_RESET,
            )
        } else {
            value.to_string()
        }
    }

    fn title(
        self,
        value: &str,
    ) -> String {
        self.paint(
            value,
            MANUAL_TITLE,
        )
    }

    fn section(
        self,
        value: &str,
    ) -> String {
        self.paint(
            value,
            MANUAL_ACCENT,
        )
    }

    fn accent(
        self,
        value: &str,
    ) -> String {
        self.paint(
            value,
            MANUAL_ACCENT,
        )
    }

    fn text(
        self,
        value: &str,
    ) -> String {
        self.paint(
            value,
            MANUAL_TEXT,
        )
    }

    fn muted(
        self,
        value: &str,
    ) -> String {
        self.paint(
            value,
            MANUAL_TEXT,
        )
    }

    fn command(
        self,
        value: &str,
    ) -> String {
        self.paint(
            value,
            MANUAL_EMPHASIS,
        )
    }

    fn frame(
        self,
        value: &str,
    ) -> String {
        self.paint(
            value,
            MANUAL_BORDER,
        )
    }

    fn bold_text(
        self,
        value: &str,
    ) -> String {
        self.paint(
            value,
            MANUAL_EMPHASIS,
        )
    }
}

pub fn print_manual(
    theme: &Theme,
) -> io::Result<()> {
    let inner_width = manual_inner_width();

    /*
     * Leave four cells for the two spaces on each side
     * inside the frame.
     */
    let text_width =
        inner_width.saturating_sub(4);

    let lines =
        help::content(theme, text_width);

    let colors_enabled = colors_enabled();

    print_top_border(
        inner_width,
        colors_enabled,
    );

    for line in lines {
        let plain_text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();

        if matches!(
        plain_text.as_str(),
        "[Jump to Tips]"
            | "[Jump back to top]"
            | "↑/↓ scroll  PgUp/PgDn page  Esc/F1 closes"
    ) {
            continue;
        }

        print_help_line(
            &line,
            inner_width,
            theme,
            colors_enabled,
        )?;
    }

    print_bottom_border(
        inner_width,
        colors_enabled,
    );

    Ok(())
}

fn colors_enabled() -> bool {
    io::stdout().is_terminal()
        && env::var_os("NO_COLOR").is_none()
}

fn print_top_border(
    width: usize,
    colors_enabled: bool,
) {
    println!(
        "{}┌{}┐{}",
        border_color(colors_enabled),
        "─".repeat(width + 4),
        reset(colors_enabled),
    );
}

fn print_bottom_border(
    width: usize,
    colors_enabled: bool,
) {
    println!(
        "{}└{}┘{}",
        border_color(colors_enabled),
        "─".repeat(width + 4),
        reset(colors_enabled),
    );
}

fn print_help_line(
    line: &Line<'_>,
    inner_width: usize,
    theme: &Theme,
    colors_enabled: bool,
) -> io::Result<()> {
    let stdout = io::stdout();

    let mut output = stdout.lock();

    write!(
        output,
        "{}│{}  ",
        border_color(colors_enabled),
        reset(colors_enabled),
    )?;

    let visible_width: usize = line
        .spans
        .iter()
        .map(|span| span.content.chars().count())
        .sum();

    let available_padding =
        inner_width.saturating_sub(visible_width);

    let (left_padding, right_padding) =
        match line.alignment {
            Some(Alignment::Center) => {
                let left = available_padding / 2;

                (
                    left,
                    available_padding.saturating_sub(left),
                )
            }

            Some(Alignment::Right) => {
                (available_padding, 0)
            }

            _ => {
                (0, available_padding)
            }
        };

    write!(
        output,
        "{}",
        " ".repeat(left_padding),
    )?;

    for span in &line.spans {
        let ansi =
            ansi_for_span(line, span, theme, colors_enabled);

        write!(
            output,
            "{}{}{}",
            ansi,
            span.content,
            reset(colors_enabled),
        )?;
    }

    write!(
        output,
        "{}",
        " ".repeat(right_padding),
    )?;

    write!(
        output,
        "  {}│{}",
        border_color(colors_enabled),
        reset(colors_enabled),
    )?;

    writeln!(output)?;

    Ok(())
}

fn ansi_for_span(
    line: &Line<'_>,
    span: &ratatui::text::Span<'_>,
    theme: &Theme,
    colors_enabled: bool,
) -> &'static str {
    if !colors_enabled {
        return "";
    }

    /*
     * Ratatui allows styling at both Line and Span level.
     * Help headings and paragraphs are primarily styled through
     * Line::styled(), so the line style must participate here.
     */
    let foreground =
        span.style.fg.or(line.style.fg);

    let bold =
        span.style.add_modifier.contains(Modifier::BOLD)
            || line.style.add_modifier.contains(Modifier::BOLD);

    if foreground == Some(theme.ui.status) {
        return MANUAL_TIP;
    }

    if foreground == Some(theme.ui.classification) {
        return MANUAL_ACCENT;
    }

    if foreground == Some(theme.ui.query) {
        if bold {
            return MANUAL_TITLE;
        }

        return MANUAL_ACCENT;
    }

    if foreground == Some(theme.ui.muted) {
        return MANUAL_TEXT;
    }

    if foreground == Some(theme.ui.file) {
        return MANUAL_TEXT;
    }

    if bold {
        return MANUAL_EMPHASIS;
    }

    MANUAL_TEXT
}

fn border_color(
    enabled: bool,
) -> &'static str {
    if enabled {
        MANUAL_BORDER
    } else {
        ""
    }
}

fn reset(
    enabled: bool,
) -> &'static str {
    if enabled {
        MANUAL_RESET
    } else {
        ""
    }
}

const MIN_INNER_WIDTH: usize = 40;

fn terminal_width() -> usize {
    terminal_width_from_ioctl()
        .or_else(terminal_width_from_environment)
        .unwrap_or(80)
}

fn manual_inner_width() -> usize {
    terminal_width().saturating_sub(6).max(MIN_INNER_WIDTH)
}

fn terminal_width_from_environment() -> Option<usize> {
    env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|width| *width > 0)
}

fn terminal_width_from_ioctl() -> Option<usize> {
    for file_descriptor in [
        libc::STDOUT_FILENO,
        libc::STDERR_FILENO,
        libc::STDIN_FILENO,
    ] {
        let mut window_size = libc::winsize {
            ws_row: 0,
            ws_col: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let result = unsafe {
            libc::ioctl(
                file_descriptor,
                libc::TIOCGWINSZ,
                &mut window_size,
            )
        };

        if result == 0 {
            let width = window_size.ws_col as usize;

            if width > 0 {
                return Some(width);
            }
        }
    }

    None
}

pub fn print_help() -> io::Result<()> {
    let palette = Palette::new();

    let options_widths = option_column_widths();

    let options_table_width = options_widths.0 + options_widths.1 + options_widths.2 + 10;

    let terminal_width = terminal::size()
        .map(|(width, _)| width as usize)
        .unwrap_or(options_table_width);

    let page_width = terminal_width.max(options_table_width).min(100);

    println!();

    println!(
        "{}",
        palette.title(&center_text(
            &format!("Scry {}", env!("CARGO_PKG_VERSION"),),
            page_width,
        ),),
    );

    println!();

    println!(
        "{}{}",
        palette.bold_text("Scry"),
        palette.text(
            " is a fast terminal file browser and recursive finder. Browse directories, search entire trees as you type, sort by metadata, inspect files, and navigate with either the keyboard or mouse.",
        ),
    );

    println!();

    println!(
        "{}",
        palette.text(
            "In Tree mode, Right expands a branch while Enter makes the selected directory the new root and closes the previous hierarchy behind you.",
        ),
    );

    println!();

    println!(
        "{}{}",
        palette.bold_text("Scry"),
        palette.text(" at GitHub: https://github.com/ferusx/scry-tui-file-browser",),
    );

    println!();

    println!("{}", palette.section("Usage:"),);

    println!("  {}", palette.command("scry  [OPTIONS] [PATH]",),);

    println!();

    println!("{}", palette.section("Arguments:"),);

    println!(
        "  {}  {}",
        palette.accent("PATH"),
        palette
            .text("Directory or file from which Scry should begin (default: current directory)",),
    );

    println!();

    println!("{}", palette.section("Options:"),);

    print_options_table(palette, options_widths);

    println!();

    println!(
        "{}",
        palette.muted(
            "Run `scry --manual` for the complete manual and use ? inside Scry \
         for the interactive shortcut legend.",
        ),
    );

    println!();

    println!("{}", palette.section("Examples:",),);

    print_examples_table(palette);

    println!();

    println!(
        "{}",
        palette.muted("Type directly in Scry to begin searching.",),
    );

    println!();

    println!(
        "{}",
        palette.muted("Launch Scry and let the files come to you.",),
    );

    println!(
        "{}",
        palette.muted("The advanced options can wait until everyone is properly caffeinated :)",),
    );

    println!();

    Ok(())
}

fn option_column_widths() -> (usize, usize, usize) {
    let short_width = OPTIONS
        .iter()
        .map(|option| option.short.chars().count())
        .chain(std::iter::once("short".len()))
        .max()
        .unwrap_or(5);

    let long_width = OPTIONS
        .iter()
        .map(|option| option.long.chars().count())
        .chain(std::iter::once("long".len()))
        .max()
        .unwrap_or(4);

    let description_width = OPTIONS
        .iter()
        .map(|option| option.description.chars().count())
        .chain(std::iter::once("description".len()))
        .max()
        .unwrap_or(11);

    (short_width, long_width, description_width)
}

fn print_options_table(
    palette: Palette,
    widths: (usize, usize, usize),
) {
    let (short_width, long_width, description_width) = widths;

    println!(
        "{}",
        palette.frame(&table_border(
            '┌',
            '┬',
            '┐',
            &[short_width, long_width, description_width,],
        ),),
    );

    print_three_column_row(palette, "short", "long", "description", widths, true);

    println!(
        "{}",
        palette.frame(&table_border(
            '├',
            '┼',
            '┤',
            &[short_width, long_width, description_width,],
        ),),
    );

    for option in OPTIONS {
        print_three_column_row(
            palette,
            option.short,
            option.long,
            option.description,
            widths,
            false,
        );
    }

    println!(
        "{}",
        palette.frame(&table_border(
            '└',
            '┴',
            '┘',
            &[short_width, long_width, description_width,],
        ),),
    );
}

fn print_three_column_row(
    palette: Palette,
    first: &str,
    second: &str,
    third: &str,
    widths: (usize, usize, usize),
    heading: bool,
) {
    let (first_width, second_width, third_width) = widths;

    let first = pad_right(first, first_width);

    let second = pad_right(second, second_width);

    let third = pad_right(third, third_width);

    let first = if heading {
        palette.section(&first)
    } else {
        palette.accent(&first)
    };

    let second = if heading {
        palette.section(&second)
    } else {
        palette.accent(&second)
    };

    let third = if heading {
        palette.section(&third)
    } else {
        palette.text(&third)
    };

    println!(
        "{} {} {} {} {} {} {}",
        palette.frame("│"),
        first,
        palette.frame("│"),
        second,
        palette.frame("│"),
        third,
        palette.frame("│"),
    );
}

fn print_examples_table(
    palette: Palette,
) {
    for (index, example) in EXAMPLES.iter().enumerate() {
        println!("  {}", palette.command(example.command),);

        println!("    {}", palette.text(example.description),);

        if index + 1 < EXAMPLES.len() {
            println!();
        }
    }
}

fn table_border(left: char, junction: char, right: char, widths: &[usize]) -> String {
    let mut result = String::new();

    result.push(left);

    for (index, width) in widths.iter().enumerate() {
        /*
         * Two extra cells account for the spaces surrounding each value.
         */
        result.push_str(&"─".repeat(width.saturating_add(2)));

        if index + 1 == widths.len() {
            result.push(right);
        } else {
            result.push(junction);
        }
    }

    result
}

fn pad_right(value: &str, width: usize) -> String {
    let current_width = value.chars().count();

    if current_width >= width {
        value.to_string()
    } else {
        format!("{}{}", value, " ".repeat(width - current_width,),)
    }
}

fn center_text(value: &str, width: usize) -> String {
    let value_width = value.chars().count();

    if value_width >= width {
        return value.to_string();
    }

    let left_padding = width.saturating_sub(value_width) / 2;

    format!("{}{}", " ".repeat(left_padding), value,)
}

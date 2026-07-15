// SPDX-License-Identifier: BSD-3-Clause

use std::env;
use std::io::{self, IsTerminal};

use crossterm::terminal;

/*
 * These definitions are public so the future internal Ratatui help overlay
 * can reuse the same descriptions. External and internal help should never
 * drift apart.
 */
#[derive(Debug, Clone, Copy)]
pub struct HelpOption {
    pub short: &'static str,

    pub long: &'static str,

    pub description: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct KeyBindingHelp {
    pub keys: &'static str,

    pub description: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct HelpExample {
    pub command: &'static str,

    pub description: &'static str,
}

pub const OPTIONS: &[HelpOption] = &[
    HelpOption {
        short: "-h",
        long: "--help",
        description: "Print this help information",
    },
    HelpOption {
        short: "-V",
        long: "--version",
        description: "Print the Scry version",
    },
    HelpOption {
        short: "",
        long: "--ssh TARGET",
        description: "Browse a remote computer through SSH/SFTP",
    },
    HelpOption {
        short: "-a",
        long: "--all",
        description: "Show hidden files and directories",
    },
    HelpOption {
        short: "-r",
        long: "--recursive",
        description: "Start with a recursive listing",
    },
    HelpOption {
        short: "-T",
        long: "--tree",
        description: "Start in Tree mode",
    },
    HelpOption {
        short: "-p",
        long: "--permissions",
        description: "Show the permissions column",
    },
    HelpOption {
        short: "-s",
        long: "--size",
        description: "Show the file-size column",
    },
    HelpOption {
        short: "-d",
        long: "--date",
        description: "Show the modification-date column",
    },
    HelpOption {
        short: "-u",
        long: "--user",
        description: "Show the owner column",
    },
];

pub const KEY_BINDINGS: &[KeyBindingHelp] = &[
    KeyBindingHelp {
        keys: "↑ / ↓",
        description: "Move the selection",
    },
    KeyBindingHelp {
        keys: "PgUp / PgDn",
        description: "Move one visible page",
    },
    KeyBindingHelp {
        keys: "Home / End",
        description: "Select the first or last entry",
    },
    KeyBindingHelp {
        keys: "←",
        description: "Collapse a branch or move to the parent",
    },
    KeyBindingHelp {
        keys: "→",
        description: "Expand the selected directory",
    },
    KeyBindingHelp {
        keys: "Enter",
        description: "Enter as new root or open the selected file",
    },
    KeyBindingHelp {
        keys: "Ctrl+T",
        description: "Switch between List and Tree modes",
    },
    KeyBindingHelp {
        keys: "Ctrl+D",
        description: "Show or hide the Details pane",
    },
    KeyBindingHelp {
        keys: "Ctrl+S",
        description: "Show or hide the Selection panel",
    },
    KeyBindingHelp {
        keys: "Ctrl+M",
        description: "Show or hide the metadata columns",
    },
    KeyBindingHelp {
        keys: "Ctrl+H",
        description: "Show or hide hidden entries",
    },
    KeyBindingHelp {
        keys: "Ctrl+O",
        description: "Cycle through the sort modes",
    },
    KeyBindingHelp {
        keys: "Ctrl+R",
        description: "Reverse the current sort order",
    },
    KeyBindingHelp {
        keys: "Ctrl+U",
        description: "Clear the current search",
    },
    KeyBindingHelp {
        keys: "Ctrl+C",
        description: "Quit Scry",
    },
    KeyBindingHelp {
        keys: "Mouse wheel",
        description: "Scroll through entries",
    },
    KeyBindingHelp {
        keys: "Double-click",
        description: "Activate the selected entry",
    },
    KeyBindingHelp {
        keys: "Right-click",
        description: "Collapse or move to the parent",
    },
];

pub const EXAMPLES: &[HelpExample] = &[
    HelpExample {
        command: "scry -T -p -s ~/Projects",
        description: "Browse ~/Projects in Tree mode with permissions and file sizes",
    },
    HelpExample {
        command: "scry -r ~/",
        description: "Browse home directory in recursive mode",
    },
    HelpExample {
        command: "scry -rT ~/ -pds",
        description: "Browse ~/ recursively in Tree mode with permissions, date, and file size",
    },
    HelpExample {
        command: "scry --ssh 192.168.1.50 -pT",
        description: "Browse remote directory in Tree mode with permissions",
    },
];

/*
 * RGB values follow Scry's main palette.
 */
const ANSI_RESET: &str = "\x1b[0m";

const ANSI_BOLD: &str = "\x1b[1m";

const ANSI_FRAME: &str = "\x1b[38;2;75;80;92m";

const ANSI_PURPLE: &str = "\x1b[38;2;160;110;220m";

const ANSI_CYAN: &str = "\x1b[38;2;110;220;225m";

const ANSI_GRAY: &str = "\x1b[38;2;200;200;200m";

// const ANSI_PURPLE_BACKGROUND: &str = "\x1b[48;2;160;110;220m";

const ANSI_GRAY_BACKGROUND: &str = "\x1b[48;2;60;60;80m";

const ANSI_MUTED: &str = "\x1b[38;2;125;132;145m";

const ANSI_TEXT: &str = "\x1b[38;2;205;210;220m";

#[derive(Debug, Clone, Copy)]
struct Palette {
    enabled: bool,
}

impl Palette {
    fn new() -> Self {
        /*
         * ANSI styling is enabled only when standard output is a terminal.
         *
         * NO_COLOR is also respected. This keeps redirected and piped help
         * output completely free from escape sequences:
         *
         *     scry --help > help.txt
         *     scry --help | less
         */
        Self {
            enabled: io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none(),
        }
    }

    fn paint(self, value: &str, style: &str) -> String {
        if self.enabled {
            format!("{}{}{}", style, value, ANSI_RESET,)
        } else {
            value.to_string()
        }
    }

    fn title(self, value: &str) -> String {
        if self.enabled {
            format!("{}{}{}{}", ANSI_BOLD, ANSI_PURPLE, value, ANSI_RESET,)
        } else {
            value.to_string()
        }
    }

    fn command(self, value: &str) -> String {
        if self.enabled {
            format!(
                "{}{}{}{}",
                ANSI_GRAY, ANSI_GRAY_BACKGROUND, value, ANSI_RESET,
            )
        } else {
            value.to_string()
        }
    }

    fn bold_text(self, value: &str) -> String {
        if self.enabled {
            format!("{}{}{}{}", ANSI_BOLD, ANSI_TEXT, value, ANSI_RESET,)
        } else {
            value.to_string()
        }
    }

    fn section(self, value: &str) -> String {
        self.paint(value, ANSI_PURPLE)
    }

    fn frame(self, value: &str) -> String {
        self.paint(value, ANSI_FRAME)
    }

    fn accent(self, value: &str) -> String {
        self.paint(value, ANSI_CYAN)
    }

    fn muted(self, value: &str) -> String {
        self.paint(value, ANSI_MUTED)
    }

    fn text(self, value: &str) -> String {
        self.paint(value, ANSI_TEXT)
    }
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
        palette.text(" at GitHub: https://github.com/ferusx/scry-file-browser",),
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

    println!("{}", palette.section("Keyboard and mouse:"),);

    print_key_table(palette);

    println!();

    println!("{}", palette.section("Examples:",),);

    print_examples_table(palette);

    println!();

    println!(
        "{}",
        palette.muted("Type directly in Scry to begin a recursive search.",),
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

fn print_options_table(palette: Palette, widths: (usize, usize, usize)) {
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

fn print_key_table(palette: Palette) {
    let key_width = KEY_BINDINGS
        .iter()
        .map(|binding| binding.keys.chars().count())
        .chain(std::iter::once("keys".len()))
        .max()
        .unwrap_or(4);

    let description_width = KEY_BINDINGS
        .iter()
        .map(|binding| binding.description.chars().count())
        .chain(std::iter::once("description".len()))
        .max()
        .unwrap_or(11);

    println!(
        "{}",
        palette.frame(&table_border(
            '┌',
            '┬',
            '┐',
            &[key_width, description_width,],
        ),),
    );

    print_two_column_row(
        palette,
        "keys",
        "description",
        key_width,
        description_width,
        true,
    );

    println!(
        "{}",
        palette.frame(&table_border(
            '├',
            '┼',
            '┤',
            &[key_width, description_width,],
        ),),
    );

    for binding in KEY_BINDINGS {
        print_two_column_row(
            palette,
            binding.keys,
            binding.description,
            key_width,
            description_width,
            false,
        );
    }

    println!(
        "{}",
        palette.frame(&table_border(
            '└',
            '┴',
            '┘',
            &[key_width, description_width,],
        ),),
    );
}

fn print_examples_table(palette: Palette) {
    let command_width = EXAMPLES
        .iter()
        .map(|example| example.command.chars().count())
        .chain(std::iter::once("command".len()))
        .max()
        .unwrap_or(7);

    let description_width = EXAMPLES
        .iter()
        .map(|example| example.description.chars().count())
        .chain(std::iter::once("description".len()))
        .max()
        .unwrap_or(11);

    println!(
        "{}",
        palette.frame(&table_border(
            '┌',
            '┬',
            '┐',
            &[command_width, description_width,],
        ),),
    );

    print_example_row(
        palette,
        "command",
        "description",
        command_width,
        description_width,
        true,
    );

    println!(
        "{}",
        palette.frame(&table_border(
            '├',
            '┼',
            '┤',
            &[command_width, description_width,],
        ),),
    );

    for example in EXAMPLES {
        print_example_row(
            palette,
            example.command,
            example.description,
            command_width,
            description_width,
            false,
        );
    }

    println!(
        "{}",
        palette.frame(&table_border(
            '└',
            '┴',
            '┘',
            &[command_width, description_width,],
        ),),
    );
}

fn print_example_row(
    palette: Palette,
    command: &str,
    description: &str,
    command_width: usize,
    description_width: usize,
    heading: bool,
) {
    let command = pad_right(command, command_width);

    let description = pad_right(description, description_width);

    let command = if heading {
        palette.section(&command)
    } else {
        palette.accent(&command)
    };

    let description = if heading {
        palette.section(&description)
    } else {
        palette.text(&description)
    };

    println!(
        "{} {} {} {} {}",
        palette.frame("│"),
        command,
        palette.frame("│"),
        description,
        palette.frame("│"),
    );
}

fn print_two_column_row(
    palette: Palette,
    first: &str,
    second: &str,
    first_width: usize,
    second_width: usize,
    heading: bool,
) {
    let first = pad_right(first, first_width);

    let second = pad_right(second, second_width);

    let first = if heading {
        palette.section(&first)
    } else {
        palette.accent(&first)
    };

    let second = if heading {
        palette.section(&second)
    } else {
        palette.text(&second)
    };

    println!(
        "{} {} {} {} {}",
        palette.frame("│"),
        first,
        palette.frame("│"),
        second,
        palette.frame("│"),
    );
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

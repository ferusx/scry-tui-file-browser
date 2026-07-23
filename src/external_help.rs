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
        short: "",
        long: "--manual",
        description: "Print the complete explanatory manual",
    },
    HelpOption {
        short: "-V",
        long: "--version",
        description: "Print the Scry version",
    },
    HelpOption {
        short: "",
        long: "--generate-config",
        description: "Generate scry.toml.generated and exit",
    },
    HelpOption {
        short: "",
        long: "--restore-session",
        description: "Restore the most recently saved browser session",
    },
    HelpOption {
        short: "",
        long: "--ssh TARGET",
        description: "Browse a remote computer through SSH/SFTP",
    },
    HelpOption {
        short: "",
        long: "--preserve-hierarchy",
        description: "Preserve remote paths during marked batch downloads",
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
        short: "",
        long: "--fuzzy",
        description: "Start in Fuzzy search mode",
    },
    HelpOption {
        short: "",
        long: "--query TEXT",
        description: "Start with TEXT in the search field",
    },
    HelpOption {
        short: "",
        long: "--files-only",
        description: "Show files and symlinks only",
    },
    HelpOption {
        short: "",
        long: "--dirs-only",
        description: "Show directories only",
    },
    HelpOption {
        short: "-T",
        long: "--tree",
        description: "Start in Tree mode",
    },
    HelpOption {
        short: "",
        long: "--no-open",
        description: "Do not open selected files externally",
    },
    HelpOption {
        short: "",
        long: "--exit-on-open",
        description: "Exit after successfully opening a file",
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
    HelpExample {
        command: "scry --ssh example-host --preserve-hierarchy",
        description: "Preserve remote directory paths during marked batch downloads",
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
            "Run `scry --manual` for the complete manual and use Ctrl+! inside Scry \
         for the interactive shortcut legend.",
        ),
    );

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

fn print_examples_table(palette: Palette) {
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

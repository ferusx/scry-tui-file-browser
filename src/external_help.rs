// SPDX-License-Identifier: BSD-3-Clause

use std::io::{self, Write};

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
        short: "",
        long: "--console-config",
        description: "Print console shell-integration instructions",
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
        short: "",
        long: "--hidden-only",
        description: "Show hidden content exclusively",
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
        command: "scry",
        description: "Launch Scry's interactive interface and begin browsing",
    },
    HelpExample {
        command: "scry -T -p -s ~/Projects",
        description: "Browse ~/Projects in Tree mode with permissions and file sizes",
    },
    HelpExample {
        command: "scry -r --hidden-only ~/",
        description: "Search hidden content recursively beneath the home directory",
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
    HelpExample {
        command: "scry -r --query 'type:source AND (rust OR python) AND NOT test' ~/Projects",
        description: "Recursively search source files using a grouped Boolean query",
    },
    HelpExample {
        command: "scry -r --files-only --query 'ext:rs +session -test' ~/DevX",
        description: "Search recursively for Rust files requiring session and excluding test",
    },
    HelpExample {
        command: "scry --restore-session",
        description: "Restore the most recently saved browsing session",
    },
];

const CONSOLE_CONFIG: &str = include_str!("../docs/SHELL_INTEGRATION.txt");

pub fn print_console_config() -> io::Result<()> {
    let mut output = io::stdout().lock();

    output.write_all(CONSOLE_CONFIG.as_bytes())?;

    if !CONSOLE_CONFIG.ends_with('\n') {
        output.write_all(b"\n")?;
    }

    output.flush()
}


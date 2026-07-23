// SPDX-License-Identifier: BSD-3-Clause

use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "scry",
    version,
    disable_help_flag = true,
    disable_help_subcommand = true,
    about = "Fast terminal file browser and recursive finder",
    long_about = None,
)]
pub struct Cli {
    /*
     * Optional directory or file from which Scry should begin.
     *
     * Local browsing defaults to the current directory.
     *
     * Remote browsing distinguishes between:
     *
     *     no PATH    use the local launch directory on the remote host
     *     .          use the remote account's home directory
     *     PATH       use PATH exactly as supplied
     */
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,

    /*
     * Connect to a remote filesystem through SSH/SFTP.
     *
     * Accepted forms include:
     *
     *     nosferatu
     *     ferusx@nosferatu
     *     ferusx@nosferatu:2222
     */
    #[arg(
        long = "ssh",
        value_name = "TARGET",
        help = "Browse a remote computer through SSH/SFTP"
    )]
    pub ssh: Option<String>,

    /*
     * Preserve remote directory paths inside marked batch-download directories.
     *
     * Without this flag, marked files are gathered directly beneath the batch root.
     */
    #[arg(
        long = "preserve-hierarchy",
        help = "Preserve remote paths when downloading marked files"
    )]
    pub preserve_hierarchy: bool,

    /*
     * Display Scry's custom help page.
     */
    #[arg(short = 'h', long = "help", help = "Print help information")]
    pub help: bool,

    /*
     * Print the complete explanatory manual used by Scry's F1 Help window.
     *
     * The output contains no terminal-control sequences, making it suitable for
     * pagers, redirection, and text editors.
     */
    #[arg(long = "manual", help = "Print the complete Scry manual")]
    pub manual: bool,

    /*
     * Generate a documented configuration template and exit.
     *
     * The template is deliberately written as:
     *
     *     scry.toml.generated
     *
     * The live scry.toml is never created or overwritten by this command.
     */
    #[arg(
        long = "generate-config",
        help = "Generate scry.toml.generated and exit"
    )]
    pub generate_config: bool,

    /*
     * Restore the most recently saved browser session.
     *
     * This enables restoration for the current launch even when the corresponding
     * scry.toml setting is false.
     */
    #[arg(
        long = "restore-session",
        help = "Restore the most recently saved browser session"
    )]
    pub restore_session: bool,

    /*
     * Begin with hidden files and directories visible.
     */
    #[arg(short = 'a', long = "all", help = "Show hidden files and directories")]
    pub all: bool,

    /*
     * Begin with all descendants beneath the starting directory.
     */
    #[arg(
        short = 'r',
        long = "recursive",
        help = "Start with a recursive listing"
    )]
    pub recursive: bool,

    /*
     * Begin in Fuzzy search mode.
     */
    #[arg(long = "fuzzy", help = "Start in Fuzzy search mode")]
    pub fuzzy: bool,

    /*
     * Preload the interactive search field.
     *
     * Hyphen-prefixed values are accepted because valid Scry query modifiers
     * include forms such as:
     *
     *     -java
     *     -.cache
     *     -png +rust
     *
     * Queries containing whitespace must be quoted by the shell.
     */
    #[arg(
        long = "query",
        value_name = "TEXT",
        allow_hyphen_values = true,
        help = "Start with TEXT in the search field"
    )]
    pub query: Option<String>,

    /*
     * Keep Scry open when a regular file is activated.
     *
     * Directory navigation remains available.
     */
    #[arg(long = "no-open", help = "Do not open selected files externally")]
    pub no_open: bool,

    /*
     * Exit Scry after a regular file has been opened successfully.
     *
     * Directory navigation remains available. A failed opener leaves Scry
     * running so the error remains visible.
     */
    #[arg(
        long = "exit-on-open",
        conflicts_with = "no_open",
        help = "Exit after successfully opening a file"
    )]
    pub exit_on_open: bool,

    /*
     * Restrict visible results to non-directory entries.
     */
    #[arg(
        long = "files-only",
        conflicts_with = "dirs_only",
        help = "Show files and symlinks only"
    )]
    pub files_only: bool,

    /*
     * Restrict visible results to directories.
     */
    #[arg(
        long = "dirs-only",
        conflicts_with = "files_only",
        help = "Show directories only"
    )]
    pub dirs_only: bool,

    /*
     * Begin directly in Tree mode.
     */
    #[arg(short = 'T', long = "tree", help = "Start in Tree mode")]
    pub tree: bool,

    /*
     * Display the Unix permissions column.
     */
    #[arg(
        short = 'p',
        long = "permissions",
        help = "Show the permissions column"
    )]
    pub permissions: bool,

    /*
     * Display the modification-date column.
     */
    #[arg(short = 'd', long = "date", help = "Show the modification-date column")]
    pub date: bool,

    /*
     * Display the file-size column.
     */
    #[arg(short = 's', long = "size", help = "Show the file-size column")]
    pub size: bool,

    /*
     * Display the file-owner column.
     */
    #[arg(short = 'u', long = "user", help = "Show the owner column")]
    pub user: bool,
}

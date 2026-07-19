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
     * Display Scry's custom help page.
     */
    #[arg(short = 'h', long = "help", help = "Print help information")]
    pub help: bool,

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

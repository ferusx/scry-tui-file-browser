# Scry TUI File Browser

**Scry** is a stylish, fast terminal file browser and recursive finder written in Rust. It combines keyboard-first navigation, mouse support, rich file metadata, tree browsing, live search, and SSH/SFTP access in a polished terminal interface.

> Scrying through files, locally or across the network.

## Features

- Fast local filesystem browsing
- Remote browsing through SSH/SFTP
- List and expandable Tree views
- Recursive search as you type
- Sorting by name, size, modification date, or type
- Reversible sort order
- Colored Unix permission display
- File size, owner, date, type, age, and path details
- Foldable Details, Selection, and Metadata panels
- Mouse selection, scrolling, double-click activation, and draggable scrollbars
- Remote file transfer with progress, speed, cancellation, caching, and safe temporary files
- Saved SSH connection profiles
- Built-in shortcut reference and About window
- Keyboard-first operation with compact contextual footer hints
- Theme support planned

## Screenshots

Screenshots will be added here.

## Building

Scry requires a recent Rust toolchain.

```sh
git clone https://github.com/ferusx/scry-tui-file-browser.git
cd scry-tui-file-browser
cargo build --release
```

The compiled binary will be available at:

```text
target/release/scry
```

Run it directly:

```sh
./target/release/scry
```

Or install it into Cargo's binary directory:

```sh
cargo install --path .
```

## Usage

```text
scry [OPTIONS] [PATH]
```

Examples:

```sh
# Browse the current directory
scry

# Browse a specific directory
scry ~/Projects

# Start in Tree mode with permissions and sizes
scry -T -p -s ~/Projects

# Start with a recursive listing
scry -r ~

# Browse a remote host through SSH/SFTP
scry --ssh user@example-host
```

## Command-line options

| Option | Description |
|---|---|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print the Scry version |
| `--ssh TARGET` | Browse a remote computer through SSH/SFTP |
| `-a`, `--all` | Show hidden files and directories |
| `-r`, `--recursive` | Start with a recursive listing |
| `-T`, `--tree` | Start in Tree mode |
| `-p`, `--permissions` | Show the permissions column |
| `-s`, `--size` | Show the file-size column |
| `-d`, `--date` | Show the modification-date column |
| `-u`, `--user` | Show the owner column |

## Keyboard and mouse

Press `?` inside Scry to open the complete, scrollable shortcut reference.

Some important controls:

| Shortcut | Action |
|---|---|
| `↑` / `↓` | Move the selection |
| `←` / `Esc` | Move to the parent or collapse a branch |
| `→` | Enter a directory or expand a branch |
| `Enter` | Open or activate the selected entry |
| `Ctrl+T` | Switch between List and Tree views |
| `Ctrl+H` | Toggle hidden entries, or erase during search |
| `Ctrl+O` | Cycle sort mode |
| `Ctrl+R` | Reverse sort order |
| `Ctrl+D` | Toggle the Details panel |
| `Ctrl+S` | Toggle the Selection panel |
| `Alt+M` | Toggle the Metadata panel |
| `F4` | Open SSH connections |
| `Alt+A` | Open About Scry |
| `Ctrl+C` | Exit |

Mouse support includes wheel scrolling, left-click selection, double-click activation, right-click parent/collapse behavior, and draggable scrollbars.

## SSH and remote files

Scry can browse remote filesystems through OpenSSH and SFTP. It supports hostnames, SSH aliases, usernames, custom ports, identity files, start directories, and saved connection profiles.

When a remote file is opened, Scry transfers it into a private local cache and shows real progress information, including bytes transferred, percentage, elapsed time, and transfer speed. Transfers can be cancelled safely. Incomplete `.scry-part` files are removed, and completed cache files are validated before publication.

Examples:

```sh
scry --ssh nosferatu
scry --ssh ferusx@nosferatu
scry --ssh ferusx@nosferatu:2222
```

## Platform support

Scry is being developed and tested on:

- Linux
- FreeBSD

Other Unix-like systems may work but have not yet been tested as thoroughly.

## Project status

Scry is under active development. The core browser, search, tree navigation, metadata display, mouse interaction, SSH/SFTP browsing, connection profiles, and remote transfer workflow are functional.

Planned polish includes configurable themes and further UI refinement.

## License

Scry is licensed under the **BSD 3-Clause License**.

```text
SPDX-License-Identifier: BSD-3-Clause
```

See [`LICENSE`](LICENSE) for the complete license text.

## Author

Created by **Markus Johnsson**.

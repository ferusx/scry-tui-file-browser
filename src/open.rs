// SPDX-License-Identifier: BSD-3-Clause

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const TEXT_PROBE_SIZE: usize = 8 * 1024;

pub fn open_file(path: &Path) -> Result<(), String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("Unable to inspect {}: {}", path.display(), error,))?;

    if metadata.is_dir() {
        return Err(format!("{} is a directory", path.display(),));
    }

    if is_executable(&metadata) {
        return launch_executable_in_terminal(path);
    }

    match open_with_desktop_default(path) {
        Ok(()) => Ok(()),

        Err(desktop_error) if looks_like_text(path) => {
            open_text_in_terminal(path).map_err(|editor_error| {
                format!(
                    "{}; text-editor fallback also failed: {}",
                    desktop_error, editor_error,
                )
            })
        }

        Err(error) => Err(error),
    }
}

fn open_with_desktop_default(path: &Path) -> Result<(), String> {
    let candidates: &[(&str, &[&str])] = &[
        ("xdg-open", &[]),
        ("gio", &["open"]),
        ("exo-open", &[]),
        ("kde-open5", &[]),
        ("kde-open", &[]),
        ("mimeopen", &[]),
    ];

    let mut attempted = Vec::new();

    for (program, leading_arguments) in candidates {
        if !command_exists(program) {
            continue;
        }

        attempted.push(*program);

        let mut command = Command::new(program);

        command.args(*leading_arguments);
        command.arg(path);

        detach_stdio(&mut command);

        match command.spawn() {
            Ok(_) => {
                return Ok(());
            }

            Err(_) => {
                continue;
            }
        }
    }

    if attempted.is_empty() {
        Err("No supported desktop opener was found \
             (tried xdg-open, gio, exo-open, KDE openers, and mimeopen)"
            .to_string())
    } else {
        Err(format!(
            "Unable to start a desktop opener for {} \
             (available candidates tried: {})",
            path.display(),
            attempted.join(", "),
        ))
    }
}

fn launch_executable_in_terminal(path: &Path) -> Result<(), String> {
    let program = path.as_os_str();

    launch_in_terminal(program, &[], path.parent()).map_err(|error| {
        format!(
            "Unable to launch {} in a terminal: {}",
            path.display(),
            error,
        )
    })
}

fn open_text_in_terminal(path: &Path) -> Result<(), String> {
    let editor = find_editor()
        .ok_or_else(|| "No desktop opener or terminal editor was found".to_string())?;

    let argument = path.as_os_str().to_os_string();

    launch_in_terminal(editor.as_os_str(), &[argument], path.parent())
}

fn launch_in_terminal(
    program: &OsStr,
    program_arguments: &[OsString],
    working_directory: Option<&Path>,
) -> Result<(), String> {
    if let Some(configured_terminal) = env::var_os("TERMINAL") {
        let configured_path = PathBuf::from(&configured_terminal);

        if command_path_exists(&configured_path) {
            if let Some(mut command) =
                terminal_command(configured_path.as_os_str(), program, program_arguments)
            {
                if let Some(directory) = working_directory {
                    command.current_dir(directory);
                }

                detach_stdio(&mut command);

                if command.spawn().is_ok() {
                    return Ok(());
                }
            }
        }
    }

    const TERMINALS: &[&str] = &[
        "alacritty",
        "kitty",
        "wezterm",
        "foot",
        "xfce4-terminal",
        "gnome-terminal",
        "konsole",
        "mate-terminal",
        "qterminal",
        "xterm",
        "urxvt",
    ];

    let mut attempted = Vec::new();

    for terminal in TERMINALS {
        if !command_exists(terminal) {
            continue;
        }

        let Some(mut command) = terminal_command(OsStr::new(terminal), program, program_arguments)
        else {
            continue;
        };

        attempted.push(*terminal);

        if let Some(directory) = working_directory {
            command.current_dir(directory);
        }

        detach_stdio(&mut command);

        if command.spawn().is_ok() {
            return Ok(());
        }
    }

    if attempted.is_empty() {
        Err("No supported terminal emulator was found; \
             set $TERMINAL or install a supported terminal"
            .to_string())
    } else {
        Err(format!(
            "Terminal launch failed after trying: {}",
            attempted.join(", "),
        ))
    }
}

fn terminal_command(
    terminal: &OsStr,
    program: &OsStr,
    program_arguments: &[OsString],
) -> Option<Command> {
    let terminal_name = Path::new(terminal)
        .file_name()?
        .to_string_lossy()
        .to_lowercase();

    let mut command = Command::new(terminal);

    match terminal_name.as_str() {
        "alacritty" => {
            command.arg("-e");
        }

        "wezterm" => {
            command.args(["start", "--"]);
        }

        "xfce4-terminal" => {
            command.args(["--disable-server", "--execute"]);
        }

        "gnome-terminal" => {
            command.arg("--");
        }

        "konsole" | "mate-terminal" | "qterminal" | "xterm" | "urxvt" => {
            command.arg("-e");
        }

        /*
         * kitty and foot accept the command directly after their own
         * options, so no execution switch is needed.
         */
        "kitty" | "foot" => {}

        /*
         * A custom $TERMINAL value that we do not recognize receives the
         * conventional -e form.
         */
        _ => {
            command.arg("-e");
        }
    }

    command.arg(program);
    command.args(program_arguments);

    Some(command)
}

fn find_editor() -> Option<PathBuf> {
    for variable in ["VISUAL", "EDITOR"] {
        let Some(value) = env::var_os(variable) else {
            continue;
        };

        let candidate = PathBuf::from(value);

        if command_path_exists(&candidate) {
            return Some(candidate);
        }
    }

    for editor in ["nano", "micro", "vim", "vi"] {
        if command_exists(editor) {
            return Some(PathBuf::from(editor));
        }
    }

    None
}

fn looks_like_text(path: &Path) -> bool {
    let mut file = match File::open(path) {
        Ok(file) => file,

        Err(_) => {
            return false;
        }
    };

    let mut buffer = vec![0_u8; TEXT_PROBE_SIZE];

    let bytes_read = match file.read(&mut buffer) {
        Ok(bytes_read) => bytes_read,

        Err(_) => {
            return false;
        }
    };

    buffer.truncate(bytes_read);

    if buffer.is_empty() {
        return true;
    }

    if buffer.contains(&0) {
        return false;
    }

    if std::str::from_utf8(&buffer).is_ok() {
        return true;
    }

    let text_like_bytes = buffer
        .iter()
        .filter(|byte| byte.is_ascii_graphic() || matches!(**byte, b'\n' | b'\r' | b'\t' | b'\x0C'))
        .count();

    text_like_bytes * 100 / buffer.len() >= 85
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &fs::Metadata) -> bool {
    false
}

fn command_exists(command: &str) -> bool {
    command_path_exists(Path::new(command))
}

fn command_path_exists(command: &Path) -> bool {
    if command.components().count() > 1 {
        return is_executable_path(command);
    }

    let Some(path) = env::var_os("PATH") else {
        return false;
    };

    env::split_paths(&path)
        .map(|directory| directory.join(command))
        .any(|candidate| is_executable_path(&candidate))
}

fn is_executable_path(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };

    metadata.is_file() && is_executable(&metadata)
}

fn detach_stdio(command: &mut Command) {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
}

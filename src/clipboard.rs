// SPDX-License-Identifier: BSD-3-Clause

use std::{
    ffi::OsStr,
    io,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

/*
 * Private command-line marker used when Scry launches itself as a temporary
 * clipboard owner.
 *
 * This is deliberately handled before Clap sees the arguments.
 */
const CLIPBOARD_OWNER_ARGUMENT: &str = "__scry_clipboard_owner";

/*
 * Detect whether this process was launched as Scry's clipboard owner.
 *
 * The returned Some result tells main() not to start the TUI.
 */
pub fn run_owner_if_requested() -> Option<io::Result<()>> {
    let mut arguments = std::env::args_os();

    let _executable = arguments.next();

    if arguments.next().as_deref() != Some(OsStr::new(CLIPBOARD_OWNER_ARGUMENT)) {
        return None;
    }

    let Some(text) = arguments.next() else {
        return Some(Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "clipboard owner was started without clipboard text",
        )));
    };

    let text = text.to_string_lossy().into_owned();

    Some(own_clipboard(text))
}

/*
 * Launch a second copy of Scry which owns the clipboard independently from
 * the browser process.
 */
pub fn spawn_owner(text: &str) -> io::Result<()> {
    Command::new(std::env::current_exe()?)
        .arg(CLIPBOARD_OWNER_ARGUMENT)
        .arg(text)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .current_dir("/")
        .spawn()?;

    /*
     * Give the child a brief opportunity to acquire clipboard ownership before
     * the original App and its ClipboardContext are dropped.
     *
     * This occurs only during Scry shutdown and is not part of normal input or
     * rendering performance.
     */
    thread::sleep(Duration::from_millis(100));

    Ok(())
}

#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "android", target_os = "emscripten"))
))]
fn own_clipboard(text: String) -> io::Result<()> {
    use arboard::{Clipboard, SetExtLinux};

    let mut clipboard = Clipboard::new().map_err(|error| io::Error::other(error.to_string()))?;

    /*
     * Do not resurrect stale Scry text if another program replaced the
     * clipboard between Scry's final check and this helper starting.
     */
    match clipboard.get_text() {
        Ok(current_text) if current_text != text => {
            return Ok(());
        }

        Ok(_) => {}

        /*
         * The old owner may disappear during process handoff. In that case,
         * continue and install the supplied text ourselves.
         */
        Err(_) => {}
    }

    clipboard
        .set()
        .wait()
        .text(text)
        .map_err(|error| io::Error::other(error.to_string()))
}

/*
 * Windows and macOS store clipboard data independently from the process which
 * supplied it. The ordinary clipboard operation is enough there.
 */
#[cfg(any(windows, target_os = "macos"))]
fn own_clipboard(text: String) -> io::Result<()> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|error| io::Error::other(error.to_string()))?;

    clipboard
        .set_text(text)
        .map_err(|error| io::Error::other(error.to_string()))
}

// SPDX-License-Identifier: BSD-3-Clause

#[cfg(target_os = "linux")]
use std::{
    ffi::OsStr,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use std::io;

/*
 * Private command-line marker used when Scry launches itself as a temporary
 * Linux clipboard owner.
 *
 * This is deliberately handled before Clap sees the arguments.
 */
#[cfg(target_os = "linux")]
const CLIPBOARD_OWNER_ARGUMENT: &str = "__scry_clipboard_owner";

/*
 * Detect whether this Linux process was launched as Scry's clipboard owner.
 *
 * The returned Some result tells main() not to start the TUI.
 */
#[cfg(target_os = "linux")]
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

    Some(own_linux_clipboard(text.to_string_lossy().into_owned()))
}

/*
 * FreeBSD uses OSC 52 and therefore never launches a clipboard-owner process.
 */
#[cfg(not(target_os = "linux"))]
pub fn run_owner_if_requested() -> Option<io::Result<()>> {
    None
}

/*
 * Launch a second copy of Scry which owns the Linux clipboard independently
 * from the browser process.
 */
#[cfg(target_os = "linux")]
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
     * This occurs only during Scry shutdown.
     */
    thread::sleep(Duration::from_millis(100));

    Ok(())
}

/*
 * FreeBSD's OSC 52 copy is already stored by the terminal emulator.
 *
 * No process must remain alive after Scry exits.
 */
#[cfg(not(target_os = "linux"))]
pub fn spawn_owner(_text: &str) -> io::Result<()> {
    Ok(())
}

/*
 * Keep the Linux clipboard alive after the main Scry process exits.
 *
 * cli-clipboard uses the X11/Wayland ownership model, so this helper retains
 * its ClipboardContext until another application replaces the clipboard.
 */
#[cfg(target_os = "linux")]
fn own_linux_clipboard(text: String) -> io::Result<()> {
    use cli_clipboard::{ClipboardContext, ClipboardProvider};

    let mut clipboard =
        ClipboardContext::new().map_err(|error| io::Error::other(error.to_string()))?;

    /*
     * Do not resurrect stale Scry text if another application replaced the
     * clipboard between Scry's final check and this helper starting.
     */
    match clipboard.get_contents() {
        Ok(current_text) if current_text != text => {
            return Ok(());
        }

        Ok(_) => {}

        /*
         * The original owner may disappear during handoff. Continue and install
         * the supplied text ourselves.
         */
        Err(_) => {}
    }

    clipboard
        .set_contents(text.clone())
        .map_err(|error| io::Error::other(error.to_string()))?;

    /*
     * Exit after another application replaces Scry's clipboard text.
     *
     * This prevents obsolete clipboard-owner processes from accumulating.
     */
    loop {
        thread::sleep(Duration::from_millis(250));

        match clipboard.get_contents() {
            Ok(current_text) if current_text != text => {
                return Ok(());
            }

            Ok(_) | Err(_) => {}
        }
    }
}

/*
 * Copy text on FreeBSD through the terminal's OSC 52 clipboard protocol.
 *
 * The terminal emulator stores the resulting clipboard contents independently
 * from Scry, so they remain available after Scry exits.
 */
#[cfg(target_os = "freebsd")]
pub fn copy_with_osc52(text: &str) -> io::Result<()> {
    use std::io::Write;

    let encoded = encode_base64(text.as_bytes());

    let sequence = format!("\x1b]52;c;{}\x07", encoded);

    let mut output = io::stdout();

    output.write_all(sequence.as_bytes())?;

    output.flush()
}

/*
 * Small self-contained Base64 encoder used by OSC 52.
 *
 * Keeping this here avoids adding another dependency solely for encoding a
 * copied filesystem path.
 */
#[cfg(target_os = "freebsd")]
fn encode_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let first = chunk[0];

        let second = chunk.get(1).copied().unwrap_or(0);

        let third = chunk.get(2).copied().unwrap_or(0);

        encoded.push(ALPHABET[(first >> 2) as usize] as char);

        encoded.push(ALPHABET[(((first & 0b0000_0011) << 4) | (second >> 4)) as usize] as char);

        if chunk.len() > 1 {
            encoded.push(ALPHABET[(((second & 0b0000_1111) << 2) | (third >> 6)) as usize] as char);
        } else {
            encoded.push('=');
        }

        if chunk.len() > 2 {
            encoded.push(ALPHABET[(third & 0b0011_1111) as usize] as char);
        } else {
            encoded.push('=');
        }
    }

    encoded
}

#[cfg(all(test, target_os = "freebsd"))]
mod tests {
    use super::encode_base64;

    #[test]
    fn base64_encodes_clipboard_text() {
        assert_eq!(
            encode_base64(b"scry-clipboard-test"),
            "c2NyeS1jbGlwYm9hcmQtdGVzdA==",
        );
    }

    #[test]
    fn base64_handles_padding() {
        assert_eq!(encode_base64(b"a"), "YQ==");

        assert_eq!(encode_base64(b"ab"), "YWI=");

        assert_eq!(encode_base64(b"abc"), "YWJj");
    }
}

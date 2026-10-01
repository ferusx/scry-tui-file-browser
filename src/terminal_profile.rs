// SPDX-License-Identifier: BSD-3-Clause

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalProfile {
    Rich,
    Console,
}

impl TerminalProfile {
    pub fn detect() -> Self {
        if system_console_is_active() {
            Self::Console
        } else {
            Self::Rich
        }
    }

    pub fn is_console(self) -> bool {
        matches!(self, Self::Console)
    }

    pub fn supports_icons(self) -> bool {
        !self.is_console()
    }

    pub fn supports_file_opening(self) -> bool {
        !self.is_console()
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]
fn system_console_is_active() -> bool {
    use std::ffi::CStr;

    let mut buffer = [0 as libc::c_char; 256];

    let result = unsafe { libc::ttyname_r(libc::STDOUT_FILENO, buffer.as_mut_ptr(), buffer.len()) };

    if result != 0 {
        return false;
    }

    let terminal_name = unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_string_lossy();

    match std::env::consts::OS {
        "linux" => {
            terminal_name == "/dev/console"
                || terminal_name
                .strip_prefix("/dev/tty")
                .is_some_and(|suffix| {
                    !suffix.is_empty()
                        && suffix.chars().all(|character| character.is_ascii_digit())
                })
        }

        "freebsd" => {
            terminal_name == "/dev/console"
                || terminal_name.starts_with("/dev/ttyv")
        }

        "netbsd" => {
            terminal_name == "/dev/console"
                || terminal_name == "/dev/constty"
                || terminal_name.starts_with("/dev/ttyE")
        }

        "openbsd" => {
            terminal_name == "/dev/console"
                || terminal_name.starts_with("/dev/ttyC")
        }

        _ => false,
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
)))]
fn system_console_is_active() -> bool {
    false
}

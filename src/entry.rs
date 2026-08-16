// SPDX-License-Identifier: BSD-3-Clause

use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,

    Directory,

    Symlink,

    Socket,

    Fifo,

    BlockDevice,

    CharDevice,

    Unknown,
}

impl EntryKind {
    pub fn is_directory(self) -> bool {
        self == Self::Directory
    }

    pub fn is_symlink(self) -> bool {
        self == Self::Symlink
    }

    pub fn permission_type_character(self) -> char {
        match self {
            Self::File => '.',

            Self::Directory => 'd',

            Self::Symlink => 'l',

            Self::Socket => 's',

            Self::Fifo => 'p',

            Self::BlockDevice => 'b',

            Self::CharDevice => 'c',

            Self::Unknown => '?',
        }
    }
}

pub fn format_permissions(kind: EntryKind, mode: u32) -> String {
    let mut permissions = String::with_capacity(10);

    permissions.push(kind.permission_type_character());

    permissions.push(if mode & 0o400 != 0 { 'r' } else { '-' });

    permissions.push(if mode & 0o200 != 0 { 'w' } else { '-' });

    permissions.push(match (mode & 0o100 != 0, mode & 0o4000 != 0) {
        (true, true) => 's',
        (false, true) => 'S',
        (true, false) => 'x',
        (false, false) => '-',
    });

    permissions.push(if mode & 0o040 != 0 { 'r' } else { '-' });

    permissions.push(if mode & 0o020 != 0 { 'w' } else { '-' });

    permissions.push(match (mode & 0o010 != 0, mode & 0o2000 != 0) {
        (true, true) => 's',
        (false, true) => 'S',
        (true, false) => 'x',
        (false, false) => '-',
    });

    permissions.push(if mode & 0o004 != 0 { 'r' } else { '-' });

    permissions.push(if mode & 0o002 != 0 { 'w' } else { '-' });

    permissions.push(match (mode & 0o001 != 0, mode & 0o1000 != 0) {
        (true, true) => 't',
        (false, true) => 'T',
        (true, false) => 'x',
        (false, false) => '-',
    });

    permissions
}

#[derive(Debug, Clone)]
pub struct EntryMetadata {
    pub kind: EntryKind,

    /*
     * Raw Unix mode bits.
     *
     * Local filesystem entries receive this value from std::fs::Metadata.
     * Remote entries will later receive it from SFTP attributes.
     */
    pub permissions_mode: u32,

    pub size_bytes: u64,

    pub modified_time: Option<SystemTime>,

    /*
     * SFTP commonly supplies numeric ownership information but not resolved
     * account names. Both representations are therefore optional.
     */
    pub owner_id: Option<u32>,
}

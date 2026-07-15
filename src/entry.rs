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

// SPDX-License-Identifier: BSD-3-Clause

use std::fmt::Debug;
use std::io;
use std::path::{Path, PathBuf};

use crate::scan::{FileEntry, SortMode, read_directory};

#[derive(Debug, Clone, Copy)]
pub struct TransferProgress {
    pub transferred_bytes: u64,

    pub total_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferControl {
    Continue,

    Cancel,
}

pub trait FileSource: Debug + Send {
    fn read_directory(
        &mut self,
        directory: &Path,
        sort_mode: SortMode,
        sort_descending: bool,
    ) -> io::Result<Vec<FileEntry>>;

    fn directory_has_content(&mut self, directory: &Path) -> io::Result<bool>;

    fn path_is_directory(&mut self, path: &Path) -> io::Result<bool>;

    fn supports_recursive_scan(&self) -> bool;

    fn source_label(&self) -> String;

    fn materialize_file(
        &mut self,
        path: &Path,
        progress: &mut dyn FnMut(TransferProgress) -> io::Result<TransferControl>,
    ) -> io::Result<PathBuf>;

    fn is_remote(&self) -> bool {
        false
    }
}

#[derive(Debug, Default)]
pub struct LocalSource;

impl LocalSource {
    pub fn new() -> Self {
        Self
    }
}

impl FileSource for LocalSource {
    fn read_directory(
        &mut self,
        directory: &Path,
        sort_mode: SortMode,
        sort_descending: bool,
    ) -> io::Result<Vec<FileEntry>> {
        read_directory(directory, sort_mode, sort_descending)
    }

    fn directory_has_content(&mut self, directory: &Path) -> io::Result<bool> {
        let mut entries = std::fs::read_dir(directory)?;

        Ok(entries.next().is_some())
    }

    fn path_is_directory(&mut self, path: &Path) -> io::Result<bool> {
        Ok(std::fs::metadata(path)?.is_dir())
    }

    fn supports_recursive_scan(&self) -> bool {
        true
    }

    fn source_label(&self) -> String {
        "Local".to_string()
    }

    fn materialize_file(
        &mut self,
        path: &Path,
        _progress: &mut dyn FnMut(TransferProgress) -> io::Result<TransferControl>,
    ) -> io::Result<PathBuf> {
        Ok(path.to_path_buf())
    }
}

// SPDX-License-Identifier: BSD-3-Clause
use std::fmt::Debug;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

use crate::scan::{
    FileEntry, RecursiveScanMode, ScanMessage, SortMode, read_directory, start_recursive_scan,
};

use crate::remote_index::{RemoteIndexBuildMessage, RemoteIndexIdentity};

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

    fn start_recursive_scan(
        &mut self,
        _root: PathBuf,
        _show_hidden: bool,
        _generation: u64,
        _mode: RecursiveScanMode,
    ) -> io::Result<Receiver<ScanMessage>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "recursive scanning is not supported by this filesystem source",
        ))
    }

    fn source_label(&self) -> String;

    fn materialize_file(
        &mut self,
        path: &Path,
        progress: &mut dyn FnMut(TransferProgress) -> io::Result<TransferControl>,
    ) -> io::Result<PathBuf>;

    fn is_remote(&self) -> bool {
        false
    }

    /*
     * A remote source may expose a persistent-index identity.
     *
     * Local sources and temporary placeholder sources return None.
     */
    fn remote_index_identity(&self) -> Option<RemoteIndexIdentity> {
        None
    }

    fn start_remote_index_build(
        &mut self,
        _includes_hidden: bool,
    ) -> io::Result<Receiver<RemoteIndexBuildMessage>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "persistent remote indexing is not supported by this filesystem source",
        ))
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

    fn start_recursive_scan(
        &mut self,
        root: PathBuf,
        show_hidden: bool,
        generation: u64,
        mode: RecursiveScanMode,
    ) -> io::Result<Receiver<ScanMessage>> {
        Ok(start_recursive_scan(root, show_hidden, generation, mode))
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

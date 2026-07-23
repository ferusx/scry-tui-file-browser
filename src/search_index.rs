// SPDX-License-Identifier: BSD-3-Clause

use std::sync::Arc;

use crate::scan::FileEntry;

use crate::classify::FileClass;

/*
 * Compact search information prepared once when an entry enters the scan.
 *
 * The strings are Arc-backed and already lowercase. Search workers can clone
 * an Arc<SearchIndex> without copying millions of paths for every query.
 */
#[derive(Debug, Clone)]
pub struct SearchRecord {
    pub entry_index: usize,

    /*
     * Lowercase filename and path used by Scry's normal case-insensitive search.
     */
    pub searchable_name: Arc<str>,

    pub searchable_path: Arc<str>,

    /*
     * Original-case filename and path used only after the query directive:
     *
     *     type:sensitive
     *
     * Keeping these in SearchRecord allows local and remote recursive workers to
     * perform genuine case-sensitive matching without touching the filesystem.
     */
    pub original_path: Arc<str>,

    /*
     * Lowercase extension prepared once when the index is created.
     *
     * An extensionless entry uses an empty string.
     */
    pub extension: Arc<str>,

    pub class: FileClass,

    pub character_mask: u64,

    pub path_length: u32,

    pub is_directory: bool,

    pub is_symlink: bool,

    /*
     * True when any component of the indexed path begins with a dot.
     *
     * The worker can reject hidden descendants without reparsing every path on
     * every keystroke.
     */
    pub contains_hidden_component: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SearchIndex {
    records: Vec<SearchRecord>,
}

impl SearchIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_entries(entries: &[FileEntry]) -> Self {
        let mut index = Self::new();

        index.extend_from_entries(entries, 0);

        index
    }

    pub fn clear(&mut self) {
        self.records.clear();
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn records(&self) -> &[SearchRecord] {
        &self.records
    }

    /*
     * Append records for a newly arrived recursive scanner batch.
     *
     * base_entry_index is the length of recursive_entries before that batch is
     * appended. This keeps every record linked to its corresponding FileEntry.
     */
    pub fn extend_from_entries(&mut self, entries: &[FileEntry], base_entry_index: usize) {
        self.records.reserve(entries.len());

        self.records
            .extend(entries.iter().enumerate().map(|(offset, entry)| {
                let extension = entry
                    .path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .unwrap_or_default()
                    .to_lowercase();

                let contains_hidden_component = entry.relative_path.components().any(|component| {
                    let component = component.as_os_str().to_string_lossy();

                    component != "." && component != ".." && component.starts_with('.')
                });

                let original_path: Arc<str> =
                    Arc::from(entry.relative_path.to_string_lossy().into_owned());

                SearchRecord {
                    entry_index: base_entry_index + offset,

                    searchable_name: Arc::clone(&entry.searchable_name),

                    searchable_path: Arc::clone(&entry.searchable_path),

                    original_path,

                    extension: Arc::from(extension),

                    class: entry.class,

                    character_mask: character_mask(&entry.searchable_path),

                    path_length: saturating_u32(entry.searchable_path.len()),

                    is_directory: entry.is_directory,

                    is_symlink: entry.is_symlink,

                    contains_hidden_component,
                }
            }));
    }

    /*
     * Rebuild after recursive_entries has been reordered.
     *
     * This is required while Exact mode still sorts the backing FileEntry
     * vector. Later, the lightweight scanner refactor can give entries stable
     * IDs and remove this rebuild.
     */
    pub fn rebuild_from_entries(&mut self, entries: &[FileEntry]) {
        self.clear();

        self.extend_from_entries(entries, 0);
    }
}

/*
 * A cheap ASCII-presence signature.
 *
 * It cannot prove that a candidate matches, but it rejects many impossible
 * candidates before subsequence or edit-distance scoring begins.
 */
pub fn character_mask(value: &str) -> u64 {
    let mut mask = 0_u64;

    for byte in value.bytes() {
        let bit = match byte {
            b'a'..=b'z' => u32::from(byte - b'a'),

            b'0'..=b'9' => 26 + u32::from(byte - b'0'),

            b'_' => 36,

            b'-' => 37,

            b'.' => 38,

            _ => {
                continue;
            }
        };

        mask |= 1_u64 << bit;
    }

    mask
}

fn saturating_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::{SearchIndex, character_mask};

    use std::{path::PathBuf, sync::Arc};

    use crate::{classify::FileClass, scan::FileEntry};

    #[test]
    fn mask_contains_repeated_characters_only_once() {
        assert_eq!(character_mask("hello"), character_mask("helo"),);
    }

    #[test]
    fn different_ascii_characters_change_the_mask() {
        assert_ne!(character_mask("help"), character_mask("held"),);
    }

    #[test]
    fn mask_is_case_independent_for_folded_input() {
        assert_eq!(character_mask("help"), character_mask("help"),);
    }

    #[test]
    fn records_store_worker_filter_metadata() {
        let entry = FileEntry {
            path: PathBuf::from("/home/ferusx/.cache/Example.rs"),

            relative_path: PathBuf::from(".cache/Example.rs"),

            searchable_path: Arc::from(".cache/example.rs"),

            searchable_name: Arc::from("example.rs"),

            name: "Example.rs".to_string(),

            is_directory: false,

            is_symlink: true,

            permissions: "lrwxrwxrwx".to_string(),

            modified: "2026-07-21 07:00".to_string(),

            modified_time: None,

            owner_id: 1000,

            size_bytes: 10,

            class: FileClass::Rust,
        };

        let index = SearchIndex::from_entries(&[entry]);

        let record = &index.records()[0];

        assert_eq!(record.extension.as_ref(), "rs");

        assert_eq!(record.original_path.as_ref(), ".cache/Example.rs",);

        assert_eq!(record.class, FileClass::Rust);

        assert!(record.is_symlink);

        assert!(record.contains_hidden_component);
    }
}

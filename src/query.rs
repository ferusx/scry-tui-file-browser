// SPDX-License-Identifier: BSD-3-Clause

use crate::classify::{FileClass, classify_extension};
use crate::scan::FileEntry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueryTypeFilter {
    File,

    Directory,

    Symlink,

    Executable,

    Source,

    Shell,

    Rust,

    Python,

    C,

    Cpp,

    Java,

    Kotlin,

    JavaScript,

    TypeScript,

    Assembly,

    Lua,

    Ruby,

    Perl,

    Php,

    Go,

    Swift,

    Dart,

    CSharp,

    Scala,

    Groovy,

    R,

    Awk,

    Elixir,

    Erlang,

    FSharp,

    VisualBasic,

    Clojure,

    Zig,

    Nim,

    Crystal,

    Haskell,

    Ocaml,

    Pascal,

    Solidity,

    Vala,

    Web,

    Build,

    Config,

    Data,

    Log,

    Archive,

    Package,

    Document,

    Spreadsheet,

    Presentation,

    Image,

    VectorImage,

    Audio,

    Video,

    Font,

    Database,

    Torrent,

    DesktopEntry,

    Backup,

    Certificate,

    DiskImage,

    Plugin,

    Text,

    Binary,

    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SignedQueryTerm {
    Type(QueryTypeFilter),

    Extension(String),

    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedQuery {
    /*
     * Unsigned ordinary filename/path text.
     */
    text: String,

    /*
     * Explicit broad classification:
     *
     *     type:source
     *     type:image
     */
    type_filter: Option<QueryTypeFilter>,

    /*
     * Explicit extension restriction:
     *
     *     ext:rs
     *     ext:.jpg
     */
    extension_filter: Option<String>,

    /*
     * Every positive term must match:
     *
     *     +python
     *     +jpg
     *     +index
     */
    include_terms: Vec<SignedQueryTerm>,

    /*
     * Any matching negative term rejects the entry:
     *
     *     -java
     *     -png
     *     -.cache
     */
    exclude_terms: Vec<SignedQueryTerm>,
}

pub(crate) fn parse_query(query: &str) -> ParsedQuery {
    let pending_trailing_modifier = has_pending_trailing_modifier(query);

    let active_query = if pending_trailing_modifier {
        /*
         * Remove only the unfinished final token.
         *
         * For:
         *
         *     type:dir nethertools -excl
         *
         * this leaves:
         *
         *     type:dir nethertools
         */
        let trailing_token_start = query
            .char_indices()
            .rev()
            .find(|(_, character)| character.is_whitespace())
            .map(|(index, _)| index)
            .unwrap_or(0);

        &query[..trailing_token_start]
    } else {
        query
    };

    let mut text_terms = Vec::new();

    let mut type_filter = None;

    let mut extension_filter = None;

    let mut include_terms = Vec::new();

    let mut exclude_terms = Vec::new();

    let tokens: Vec<&str> = active_query.split_whitespace().collect();

    let mut index = 0_usize;

    while index < tokens.len() {
        let token = tokens[index];

        /*
         * Compact type form:
         *
         *     type:python
         */
        if let Some(value) = token.strip_prefix("type:") {
            if !value.is_empty() {
                if let Some(filter) = parse_query_type_filter(&value.to_lowercase()) {
                    type_filter = Some(filter);

                    index += 1;

                    continue;
                }
            } else if let Some(next_token) = tokens.get(index + 1) {
                if let Some(filter) = parse_query_type_filter(&next_token.to_lowercase()) {
                    type_filter = Some(filter);

                    index += 2;

                    continue;
                }
            }

            if value.is_empty() {
                index += 1;

                continue;
            }
        }

        /*
         * Compact extension form:
         *
         *     ext:rs
         *     ext:.rs
         */
        if let Some(value) = token.strip_prefix("ext:") {
            if let Some(extension) = normalize_query_extension(value) {
                extension_filter = Some(extension);

                index += 1;

                continue;
            }

            if value.is_empty() {
                /*
                 * Spaced extension form:
                 *
                 *     ext: rs
                 *     ext: .rs
                 */
                if let Some(next_token) = tokens.get(index + 1) {
                    if let Some(extension) = normalize_query_extension(next_token) {
                        extension_filter = Some(extension);

                        index += 2;

                        continue;
                    }
                }
            }

            if value.is_empty() {
                index += 1;

                continue;
            }
        }

        /*
         * Unified signed modifiers:
         *
         *     +python
         *     +jpg
         *     +index
         *
         *     -java
         *     -png
         *     -.cache
         */
        if let Some(value) = token.strip_prefix('+') {
            if let Some(term) = parse_signed_query_term(value) {
                include_terms.push(term);

                index += 1;

                continue;
            }
        }

        if let Some(value) = token.strip_prefix('-') {
            if let Some(term) = parse_signed_query_term(value) {
                exclude_terms.push(term);

                index += 1;

                continue;
            }
        }

        /*
         * Unrecognized or ordinary tokens remain part of the free-text query.
         *
         * This prevents malformed modifiers from silently disappearing.
         */
        text_terms.push(token);

        index += 1;
    }

    ParsedQuery {
        text: text_terms.join(" ").to_lowercase(),

        type_filter,

        extension_filter,

        include_terms,

        exclude_terms,
    }
}

pub(crate) fn has_pending_trailing_modifier(query: &str) -> bool {
    /*
     * A trailing space means the final token has already been committed.
     */
    if query.is_empty() || query.chars().last().is_some_and(char::is_whitespace) {
        return false;
    }

    let tokens: Vec<&str> = query.split_whitespace().collect();

    let Some(last_token) = tokens.last() else {
        return false;
    };

    let last_token = last_token.to_lowercase();

    /*
     * Compact modifier forms:
     *
     *     -java
     *     +python
     *     type:source
     *     ext:jpg
     */
    if last_token.starts_with('+')
        || last_token.starts_with('-')
        || last_token.starts_with("type:")
        || last_token.starts_with("ext:")
    {
        return true;
    }

    /*
     * Spaced modifier forms:
     *
     *     type: source
     *     ext: jpg
     *
     * While the value is still the final token, it remains pending too.
     */
    tokens.iter().rev().nth(1).is_some_and(|previous| {
        previous.eq_ignore_ascii_case("type:") || previous.eq_ignore_ascii_case("ext:")
    })
}

fn normalize_query_extension(value: &str) -> Option<String> {
    let extension = value.trim().trim_start_matches('.').to_lowercase();

    if extension.is_empty() {
        None
    } else {
        Some(extension)
    }
}

fn parse_signed_query_term(value: &str) -> Option<SignedQueryTerm> {
    let normalized = value.trim().to_lowercase();

    if normalized.is_empty() {
        return None;
    }

    /*
     * Type and language aliases win over extension aliases.
     *
     * Therefore:
     *
     *     +python
     *     -java
     *     +cpp
     *
     * are classification filters.
     */
    if let Some(filter) = parse_query_type_filter(&normalized) {
        return Some(SignedQueryTerm::Type(filter));
    }

    /*
     * A known extension becomes an extension filter.
     *
     * Both forms are accepted:
     *
     *     +jpg
     *     +.jpg
     *
     * A value such as ".cache" remains text because "cache"
     * is not a recognized file extension.
     */
    if let Some(extension) = normalize_query_extension(&normalized) {
        if classify_extension(&extension).is_some() {
            return Some(SignedQueryTerm::Extension(extension));
        }
    }

    /*

    * Everything else is ordinary filename/path text.
        */
    Some(SignedQueryTerm::Text(normalized))
}

pub(crate) fn parse_query_type_filter(value: &str) -> Option<QueryTypeFilter> {
    match value {
        "file" => Some(QueryTypeFilter::File),

        "directory" | "dir" => Some(QueryTypeFilter::Directory),

        "symlink" | "link" => Some(QueryTypeFilter::Symlink),

        "executable" | "exec" => Some(QueryTypeFilter::Executable),

        "source" | "code" => Some(QueryTypeFilter::Source),

        "shell" | "script" | "sh" => Some(QueryTypeFilter::Shell),

        "rust" | "rs" => Some(QueryTypeFilter::Rust),

        "python" | "py" => Some(QueryTypeFilter::Python),

        "c" => Some(QueryTypeFilter::C),

        "cpp" | "c++" | "cplusplus" => Some(QueryTypeFilter::Cpp),

        "java" => Some(QueryTypeFilter::Java),

        "kotlin" | "kt" => Some(QueryTypeFilter::Kotlin),

        "javascript" | "js" => Some(QueryTypeFilter::JavaScript),

        "typescript" | "ts" => Some(QueryTypeFilter::TypeScript),

        "assembly" | "assembler" | "asm" => Some(QueryTypeFilter::Assembly),

        "lua" => Some(QueryTypeFilter::Lua),

        "ruby" | "rb" => Some(QueryTypeFilter::Ruby),

        "perl" | "pl" => Some(QueryTypeFilter::Perl),

        "php" => Some(QueryTypeFilter::Php),

        "go" | "golang" => Some(QueryTypeFilter::Go),

        "swift" => Some(QueryTypeFilter::Swift),

        "dart" => Some(QueryTypeFilter::Dart),

        "csharp" | "c#" | "cs" => Some(QueryTypeFilter::CSharp),

        "scala" => Some(QueryTypeFilter::Scala),

        "groovy" => Some(QueryTypeFilter::Groovy),

        "r" | "rscript" => Some(QueryTypeFilter::R),

        "awk" => Some(QueryTypeFilter::Awk),

        "elixir" | "ex" => Some(QueryTypeFilter::Elixir),

        "erlang" | "erl" => Some(QueryTypeFilter::Erlang),

        "fsharp" | "f#" | "fs" => Some(QueryTypeFilter::FSharp),

        "visualbasic" | "visual-basic" | "vb" => Some(QueryTypeFilter::VisualBasic),

        "clojure" | "clj" => Some(QueryTypeFilter::Clojure),

        "zig" => Some(QueryTypeFilter::Zig),

        "nim" => Some(QueryTypeFilter::Nim),

        "crystal" | "cr" => Some(QueryTypeFilter::Crystal),

        "haskell" | "hs" => Some(QueryTypeFilter::Haskell),

        "ocaml" | "ml" => Some(QueryTypeFilter::Ocaml),

        "pascal" | "pas" => Some(QueryTypeFilter::Pascal),

        "solidity" | "sol" => Some(QueryTypeFilter::Solidity),

        "vala" => Some(QueryTypeFilter::Vala),

        "web" => Some(QueryTypeFilter::Web),

        "build" => Some(QueryTypeFilter::Build),

        "config" | "configuration" => Some(QueryTypeFilter::Config),

        "data" | "structured-data" | "structureddata" => Some(QueryTypeFilter::Data),

        "log" | "logs" => Some(QueryTypeFilter::Log),

        "archive" => Some(QueryTypeFilter::Archive),

        "package" | "pkg" => Some(QueryTypeFilter::Package),

        "document" | "doc" => Some(QueryTypeFilter::Document),

        "spreadsheet" | "sheet" => Some(QueryTypeFilter::Spreadsheet),

        "presentation" | "slides" => Some(QueryTypeFilter::Presentation),

        "image" | "images" | "img" => Some(QueryTypeFilter::Image),

        "vector" | "vector-image" | "vectorimage" => Some(QueryTypeFilter::VectorImage),

        "audio" => Some(QueryTypeFilter::Audio),

        "video" => Some(QueryTypeFilter::Video),

        "font" | "fonts" => Some(QueryTypeFilter::Font),

        "database" | "db" => Some(QueryTypeFilter::Database),

        "torrent" => Some(QueryTypeFilter::Torrent),

        "desktop" | "desktop-entry" | "desktopentry" => Some(QueryTypeFilter::DesktopEntry),

        "backup" => Some(QueryTypeFilter::Backup),

        "certificate" | "cert" => Some(QueryTypeFilter::Certificate),

        "disk-image" | "diskimage" => Some(QueryTypeFilter::DiskImage),

        "plugin" => Some(QueryTypeFilter::Plugin),

        "text" => Some(QueryTypeFilter::Text),

        "binary" | "bin" => Some(QueryTypeFilter::Binary),

        "unknown" => Some(QueryTypeFilter::Unknown),

        _ => None,
    }
}

fn entry_matches_type_filter(entry: &FileEntry, filter: QueryTypeFilter) -> bool {
    match filter {
        QueryTypeFilter::File => !entry.is_directory && !entry.is_symlink,

        QueryTypeFilter::Directory => entry.is_directory,

        QueryTypeFilter::Symlink => entry.is_symlink,

        QueryTypeFilter::Executable => entry.class == FileClass::Executable,

        QueryTypeFilter::Source => {
            matches!(
                entry.class,
                FileClass::ShellScript
                    | FileClass::Rust
                    | FileClass::Python
                    | FileClass::C
                    | FileClass::Cpp
                    | FileClass::Java
                    | FileClass::Kotlin
                    | FileClass::JavaScript
                    | FileClass::TypeScript
                    | FileClass::Assembly
                    | FileClass::Lua
                    | FileClass::Ruby
                    | FileClass::Perl
                    | FileClass::Php
                    | FileClass::Go
                    | FileClass::Swift
                    | FileClass::Dart
                    | FileClass::CSharp
                    | FileClass::Scala
                    | FileClass::Groovy
                    | FileClass::R
                    | FileClass::Awk
                    | FileClass::Elixir
                    | FileClass::Erlang
                    | FileClass::FSharp
                    | FileClass::VisualBasic
                    | FileClass::Clojure
                    | FileClass::Zig
                    | FileClass::Nim
                    | FileClass::Crystal
                    | FileClass::Haskell
                    | FileClass::Ocaml
                    | FileClass::Pascal
                    | FileClass::Solidity
                    | FileClass::Vala
                    | FileClass::Web
                    | FileClass::SourceCode
                    | FileClass::Build
            )
        }

        QueryTypeFilter::Shell => entry.class == FileClass::ShellScript,

        QueryTypeFilter::Rust => entry.class == FileClass::Rust,

        QueryTypeFilter::Python => entry.class == FileClass::Python,

        QueryTypeFilter::C => entry.class == FileClass::C,

        QueryTypeFilter::Cpp => {
            entry.class == FileClass::Cpp
                || (entry.class == FileClass::C
                    && entry
                        .path
                        .extension()
                        .and_then(|extension| extension.to_str())
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("h")))
        }

        QueryTypeFilter::Java => entry.class == FileClass::Java,

        QueryTypeFilter::Kotlin => entry.class == FileClass::Kotlin,

        QueryTypeFilter::JavaScript => entry.class == FileClass::JavaScript,

        QueryTypeFilter::TypeScript => entry.class == FileClass::TypeScript,

        QueryTypeFilter::Assembly => entry.class == FileClass::Assembly,

        QueryTypeFilter::Lua => entry.class == FileClass::Lua,

        QueryTypeFilter::Ruby => entry.class == FileClass::Ruby,

        QueryTypeFilter::Perl => entry.class == FileClass::Perl,

        QueryTypeFilter::Php => entry.class == FileClass::Php,

        QueryTypeFilter::Go => entry.class == FileClass::Go,

        QueryTypeFilter::Swift => entry.class == FileClass::Swift,

        QueryTypeFilter::Dart => entry.class == FileClass::Dart,

        QueryTypeFilter::CSharp => entry.class == FileClass::CSharp,

        QueryTypeFilter::Scala => entry.class == FileClass::Scala,

        QueryTypeFilter::Groovy => entry.class == FileClass::Groovy,

        QueryTypeFilter::R => entry.class == FileClass::R,

        QueryTypeFilter::Awk => entry.class == FileClass::Awk,

        QueryTypeFilter::Elixir => entry.class == FileClass::Elixir,

        QueryTypeFilter::Erlang => entry.class == FileClass::Erlang,

        QueryTypeFilter::FSharp => entry.class == FileClass::FSharp,

        QueryTypeFilter::VisualBasic => entry.class == FileClass::VisualBasic,

        QueryTypeFilter::Clojure => entry.class == FileClass::Clojure,

        QueryTypeFilter::Zig => entry.class == FileClass::Zig,

        QueryTypeFilter::Nim => entry.class == FileClass::Nim,

        QueryTypeFilter::Crystal => entry.class == FileClass::Crystal,

        QueryTypeFilter::Haskell => entry.class == FileClass::Haskell,

        QueryTypeFilter::Ocaml => entry.class == FileClass::Ocaml,

        QueryTypeFilter::Pascal => entry.class == FileClass::Pascal,

        QueryTypeFilter::Solidity => entry.class == FileClass::Solidity,

        QueryTypeFilter::Vala => entry.class == FileClass::Vala,

        QueryTypeFilter::Web => entry.class == FileClass::Web,

        QueryTypeFilter::Build => entry.class == FileClass::Build,

        QueryTypeFilter::Config => entry.class == FileClass::Config,

        QueryTypeFilter::Data => entry.class == FileClass::StructuredData,

        QueryTypeFilter::Log => entry.class == FileClass::Log,

        QueryTypeFilter::Archive => {
            matches!(entry.class, FileClass::Archive | FileClass::Package)
        }

        QueryTypeFilter::Package => entry.class == FileClass::Package,

        QueryTypeFilter::Document => {
            matches!(
                entry.class,
                FileClass::Document | FileClass::Spreadsheet | FileClass::Presentation
            )
        }

        QueryTypeFilter::Spreadsheet => entry.class == FileClass::Spreadsheet,

        QueryTypeFilter::Presentation => entry.class == FileClass::Presentation,

        QueryTypeFilter::Image => {
            matches!(entry.class, FileClass::Image | FileClass::VectorImage)
        }

        QueryTypeFilter::VectorImage => entry.class == FileClass::VectorImage,

        QueryTypeFilter::Audio => entry.class == FileClass::Audio,

        QueryTypeFilter::Video => entry.class == FileClass::Video,

        QueryTypeFilter::Font => entry.class == FileClass::Font,

        QueryTypeFilter::Database => entry.class == FileClass::Database,

        QueryTypeFilter::Torrent => entry.class == FileClass::Torrent,

        QueryTypeFilter::DesktopEntry => entry.class == FileClass::DesktopEntry,

        QueryTypeFilter::Backup => entry.class == FileClass::Backup,

        QueryTypeFilter::Certificate => entry.class == FileClass::Certificate,

        QueryTypeFilter::DiskImage => entry.class == FileClass::DiskImage,

        QueryTypeFilter::Plugin => entry.class == FileClass::Plugin,

        QueryTypeFilter::Text => entry.class == FileClass::Text,

        QueryTypeFilter::Binary => entry.class == FileClass::Binary,

        QueryTypeFilter::Unknown => entry.class == FileClass::Unknown,
    }
}

fn entry_matches_extension_filter(entry: &FileEntry, extension_filter: &str) -> bool {
    /*
     * Directories have no file extension for query purposes.
     */
    if entry.is_directory {
        return false;
    }

    entry
        .path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(extension_filter))
}

fn entry_matches_signed_query_term(entry: &FileEntry, term: &SignedQueryTerm) -> bool {
    match term {
        SignedQueryTerm::Type(filter) => entry_matches_type_filter(entry, *filter),

        SignedQueryTerm::Extension(extension) => entry_matches_extension_filter(entry, extension),

        SignedQueryTerm::Text(text) => entry.searchable_path.contains(text),
    }
}

pub(crate) fn entry_matches_query(entry: &FileEntry, query: &ParsedQuery) -> bool {
    if query
        .type_filter
        .is_some_and(|filter| !entry_matches_type_filter(entry, filter))
    {
        return false;
    }

    if query
        .extension_filter
        .as_deref()
        .is_some_and(|extension| !entry_matches_extension_filter(entry, extension))
    {
        return false;
    }

    /*
     * All positive signed terms are mandatory.
     */
    if query
        .include_terms
        .iter()
        .any(|term| !entry_matches_signed_query_term(entry, term))
    {
        return false;
    }

    /*
     * A match against any negative term rejects the entry.
     */
    if query
        .exclude_terms
        .iter()
        .any(|term| entry_matches_signed_query_term(entry, term))
    {
        return false;
    }

    /*
     * Unsigned text retains ordinary exact-substring semantics.
     */
    if !query.text.is_empty() && query.text != "." && !entry.searchable_path.contains(&query.text) {
        return false;
    }

    true
}

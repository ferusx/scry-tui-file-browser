// SPDX-License-Identifier: BSD-3-Clause

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::entry::{EntryKind, EntryMetadata};

const CONTENT_PROBE_SIZE: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileClass {
    Directory,

    Symlink,

    Executable,

    ShellScript,

    Rust,

    Python,

    C,

    Cpp,

    Java,

    Kotlin,

    JavaScript,

    TypeScript,

    Web,

    SourceCode,

    Build,

    Config,

    StructuredData,

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

impl FileClass {
    pub fn label(self) -> &'static str {
        match self {
            Self::Directory => "Directory",

            Self::Symlink => "Symlink",

            Self::Executable => "Executable",

            Self::ShellScript => "Shell script",

            Self::Rust => "Rust",

            Self::Python => "Python",

            Self::C => "C",

            Self::Cpp => "C++",

            Self::Java => "Java",

            Self::Kotlin => "Kotlin",

            Self::JavaScript => "JavaScript",

            Self::TypeScript => "TypeScript",

            Self::Web => "Web",

            Self::SourceCode => "Source",

            Self::Build => "Build",

            Self::Config => "Config",

            Self::StructuredData => "Data",

            Self::Log => "Log",

            Self::Archive => "Archive",

            Self::Package => "Package",

            Self::Document => "Document",

            Self::Spreadsheet => "Spreadsheet",

            Self::Presentation => "Presentation",

            Self::Image => "Image",

            Self::VectorImage => "Vector image",

            Self::Audio => "Audio",

            Self::Video => "Video",

            Self::Font => "Font",

            Self::Database => "Database",

            Self::Torrent => "Torrent",

            Self::DesktopEntry => "Desktop entry",

            Self::Backup => "Backup",

            Self::Certificate => "Certificate",

            Self::DiskImage => "Disk image",

            Self::Plugin => "Plugin",

            Self::Text => "Text",

            Self::Binary => "Binary",

            Self::Unknown => "Unknown",
        }
    }
}

pub fn classify(path: &Path, metadata: &EntryMetadata) -> FileClass {
    if metadata.kind == EntryKind::Symlink {
        return FileClass::Symlink;
    }

    if metadata.kind == EntryKind::Directory {
        return FileClass::Directory;
    }

    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();

    let lowercase_filename = filename.to_lowercase();

    if let Some(class) = classify_special_filename(&lowercase_filename) {
        return class;
    }

    if let Some(class) = classify_compound_extension(&lowercase_filename) {
        return class;
    }

    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_lowercase();

    if let Some(class) = classify_extension(&extension) {
        return class;
    }

    if is_executable(metadata) {
        return FileClass::Executable;
    }

    if looks_like_backup_name(&lowercase_filename) {
        return FileClass::Backup;
    }

    FileClass::Unknown
}

pub fn inspect_file(path: &Path, initial_class: FileClass) -> FileClass {
    /*
     * Extension/name classifications are already more specific than a
     * generic content probe. Only unresolved or generic executable files
     * need deeper inspection.
     */
    if !matches!(initial_class, FileClass::Unknown | FileClass::Executable) {
        return initial_class;
    }

    let mut file = match File::open(path) {
        Ok(file) => file,

        Err(_) => {
            return initial_class;
        }
    };

    let mut buffer = vec![0_u8; CONTENT_PROBE_SIZE];

    let bytes_read = match file.read(&mut buffer) {
        Ok(bytes_read) => bytes_read,

        Err(_) => {
            return initial_class;
        }
    };

    buffer.truncate(bytes_read);

    /*
     * Shebang detection comes first because scripts can be executable,
     * extensionless, or contain bytes that otherwise provide little useful
     * classification information.
     */
    if let Some(class) = classify_shebang(&buffer) {
        return class;
    }

    /*
     * An executable without a recognizable shebang remains Executable.
     *
     * This avoids relabeling compiled programs as merely Binary.
     */
    if initial_class == FileClass::Executable {
        return FileClass::Executable;
    }

    if buffer.is_empty() {
        return FileClass::Text;
    }

    if buffer.contains(&0) {
        return FileClass::Binary;
    }

    if std::str::from_utf8(&buffer).is_ok() {
        return FileClass::Text;
    }

    let text_like_bytes = buffer
        .iter()
        .filter(|byte| byte.is_ascii_graphic() || matches!(**byte, b'\n' | b'\r' | b'\t' | b'\x0C'))
        .count();

    if text_like_bytes * 100 / buffer.len() >= 85 {
        FileClass::Text
    } else {
        FileClass::Binary
    }
}

fn classify_shebang(buffer: &[u8]) -> Option<FileClass> {
    let first_line = buffer.split(|byte| *byte == b'\n').next()?;

    let first_line = std::str::from_utf8(first_line).ok()?.trim();

    let interpreter_line = first_line.strip_prefix("#!")?.trim();

    let mut parts = interpreter_line.split_whitespace();

    let executable = parts.next()?;

    let interpreter = if Path::new(executable)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "env")
    {
        /*
         * Handles forms such as:
         *
         *   #!/usr/bin/env python3
         *   #!/usr/bin/env -S node --some-option
         */
        parts.find(|part| !part.starts_with('-'))?
    } else {
        Path::new(executable).file_name()?.to_str()?
    };

    Some(classify_interpreter(interpreter))
}

fn classify_interpreter(interpreter: &str) -> FileClass {
    let interpreter = interpreter.to_lowercase();

    if matches!(
        interpreter.as_str(),
        "sh" | "bash" | "zsh" | "ksh" | "mksh" | "dash" | "ash" | "csh" | "tcsh" | "fish"
    ) {
        FileClass::ShellScript
    } else if interpreter.starts_with("python") {
        FileClass::Python
    } else if matches!(interpreter.as_str(), "node" | "nodejs" | "bun") {
        FileClass::JavaScript
    } else if interpreter == "deno" {
        FileClass::TypeScript
    } else if matches!(
        interpreter.as_str(),
        "ruby" | "perl" | "lua" | "php" | "rscript" | "groovy" | "awk" | "gawk" | "nawk"
    ) {
        FileClass::SourceCode
    } else {
        FileClass::Executable
    }
}

fn classify_special_filename(filename: &str) -> Option<FileClass> {
    let class = match filename {
        "cargo.toml" | "cargo.lock" | "rust-toolchain" | "rust-toolchain.toml" => FileClass::Rust,

        "makefile"
        | "gnumakefile"
        | "bsdmakefile"
        | "cmakelists.txt"
        | "meson.build"
        | "meson_options.txt"
        | "build.gradle"
        | "build.gradle.kts"
        | "settings.gradle"
        | "settings.gradle.kts"
        | "gradlew"
        | "gradlew.bat"
        | "justfile"
        | "rakefile" => FileClass::Build,

        ".bashrc" | ".bash_profile" | ".bash_logout" | ".zshrc" | ".zprofile" | ".zlogin"
        | ".zlogout" | ".profile" | ".kshrc" | ".cshrc" | ".tcshrc" | "nanorc" | ".nanorc"
        | ".vimrc" | ".gvimrc" | ".editorconfig" | ".gitconfig" | ".gitignore"
        | ".gitattributes" | ".gitmodules" | ".npmrc" | ".yarnrc" | ".yarnrc.yml"
        | ".clang-format" | ".clang-tidy" | ".dockerignore" => FileClass::Config,

        ".bash_history" | ".zsh_history" | ".history" | ".sh_history" => FileClass::Log,

        "dockerfile" | "containerfile" | "vagrantfile" | "procfile" => FileClass::Build,

        "readme" | "readme.txt" | "readme.md" | "license" | "license.txt" | "licence"
        | "copying" | "authors" | "contributors" | "changelog" | "changes" | "news" | "todo" => {
            FileClass::Document
        }

        _ => {
            return None;
        }
    };

    Some(class)
}

fn classify_compound_extension(filename: &str) -> Option<FileClass> {
    let class = if filename.ends_with(".tar.gz")
        || filename.ends_with(".tar.bz2")
        || filename.ends_with(".tar.xz")
        || filename.ends_with(".tar.zst")
        || filename.ends_with(".tar.lz")
        || filename.ends_with(".tar.lzma")
        || filename.ends_with(".tar.lzo")
        || filename.ends_with(".tar.br")
    {
        FileClass::Archive
    } else if filename.ends_with(".user.js") || filename.ends_with(".min.js") {
        FileClass::JavaScript
    } else if filename.ends_with(".d.ts") {
        FileClass::TypeScript
    } else if filename.ends_with(".spec.ts")
        || filename.ends_with(".test.ts")
        || filename.ends_with(".spec.tsx")
        || filename.ends_with(".test.tsx")
    {
        FileClass::TypeScript
    } else if filename.ends_with(".spec.js")
        || filename.ends_with(".test.js")
        || filename.ends_with(".spec.jsx")
        || filename.ends_with(".test.jsx")
    {
        FileClass::JavaScript
    } else if filename.ends_with(".blade.php") {
        FileClass::Web
    } else if filename.ends_with(".desktop.in") {
        FileClass::DesktopEntry
    } else if filename.ends_with(".service.in") || filename.ends_with(".conf.in") {
        FileClass::Config
    } else if filename.ends_with(".bak")
        || filename.ends_with(".backup")
        || filename.ends_with(".old")
        || filename.ends_with(".orig")
        || filename.ends_with(".save")
    {
        FileClass::Backup
    } else {
        return None;
    };

    Some(class)
}

fn classify_extension(extension: &str) -> Option<FileClass> {
    let class = match extension {
        /*
         * Programming languages.
         */
        "rs" => FileClass::Rust,

        "py" | "pyw" | "pyi" => FileClass::Python,

        "c" | "h" => FileClass::C,

        "cc" | "cpp" | "cxx" | "c++" | "hh" | "hpp" | "hxx" => FileClass::Cpp,

        "java" => FileClass::Java,

        "kt" | "kts" => FileClass::Kotlin,

        "js" | "jsx" | "mjs" | "cjs" => FileClass::JavaScript,

        "ts" | "tsx" | "mts" | "cts" => FileClass::TypeScript,

        "go" | "swift" | "scala" | "lua" | "rb" | "php" | "pl" | "pm" | "r" | "dart" | "ex"
        | "exs" | "erl" | "hrl" | "fs" | "fsx" | "fsi" | "vb" | "vbs" | "asm" | "s" | "clj"
        | "cljs" | "cljc" | "groovy" | "zig" | "nim" | "cr" | "hs" | "lhs" | "ml" | "mli"
        | "pas" | "pp" | "sol" | "vala" | "vapi" => FileClass::SourceCode,

        /*
         * Shell and command scripts.
         */
        "sh" | "bash" | "zsh" | "ksh" | "csh" | "tcsh" | "fish" | "command" => {
            FileClass::ShellScript
        }

        /*
         * Web.
         */
        "html" | "htm" | "xhtml" | "css" | "scss" | "sass" | "less" | "vue" | "svelte"
        | "astro" => FileClass::Web,

        /*
         * Build and dependency files.
         */
        "mk" | "cmake" | "ninja" | "gradle" | "d" | "dep" | "mak" | "sln" | "vcxproj"
        | "csproj" | "fsproj" | "xcodeproj" => FileClass::Build,

        /*
         * Configuration.
         */
        "conf" | "config" | "cfg" | "ini" | "toml" | "properties" | "prefs" | "rc" | "cnf"
        | "service" | "socket" | "timer" | "mount" | "target" | "rules" | "policy" | "env"
        | "editorconfig" => FileClass::Config,

        /*
         * Structured data.
         */
        "json" | "jsonc" | "json5" | "yaml" | "yml" | "xml" | "xsd" | "xsl" | "xslt" | "csv"
        | "tsv" | "ndjson" | "geojson" | "plist" | "ron" | "msgpack" | "cbor" => {
            FileClass::StructuredData
        }

        /*
         * Logs and diagnostic output.
         */
        "log" | "trace" | "out" | "err" | "dump" | "stacktrace" => FileClass::Log,

        /*
         * Archives and compressed files.
         */
        "zip" | "7z" | "rar" | "tar" | "tgz" | "tbz" | "tbz2" | "txz" | "gz" | "bz2" | "xz"
        | "zst" | "lz" | "lz4" | "lzma" | "lzo" | "cab" | "ar" | "cpio" => FileClass::Archive,

        /*
         * Installable and software packages.
         */
        "deb" | "rpm" | "pkg" | "apk" | "appimage" | "flatpak" | "snap" | "msi" | "exe" | "whl"
        | "gem" | "crate" | "jar" | "war" | "ear" => FileClass::Package,

        /*
         * Documents.
         */
        "txt" | "md" | "markdown" | "rst" | "adoc" | "asciidoc" | "org" | "tex" | "pdf"
        | "djvu" | "epub" | "mobi" | "azw" | "azw3" | "doc" | "docx" | "odt" | "rtf" | "pages"
        | "man" | "info" => FileClass::Document,

        /*
         * Spreadsheets.
         */
        "xls" | "xlsx" | "xlsm" | "ods" | "numbers" | "gnumeric" => FileClass::Spreadsheet,

        /*
         * Presentations.
         */
        "ppt" | "pptx" | "pptm" | "odp" | "key" => FileClass::Presentation,

        /*
         * Raster images.
         */
        "png" | "jpg" | "jpeg" | "jpe" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "avif"
        | "heif" | "heic" | "ico" | "xcf" | "psd" | "raw" | "cr2" | "cr3" | "nef" | "arw"
        | "dng" => FileClass::Image,

        /*
         * Vector and design images.
         */
        "svg" | "svgz" | "eps" | "ai" | "cdr" | "wmf" | "emf" => FileClass::VectorImage,

        /*
         * Audio.
         */
        "mp3" | "ogg" | "oga" | "opus" | "flac" | "wav" | "wave" | "m4a" | "aac" | "wma"
        | "aiff" | "aif" | "mid" | "midi" | "ape" | "alac" => FileClass::Audio,

        /*
         * Video.
         */
        "mp4" | "m4v" | "mkv" | "webm" | "avi" | "mov" | "mpg" | "mpeg" | "mpe" | "wmv" | "flv"
        | "ogv" | "3gp" | "m2ts" | "vob" => FileClass::Video,

        /*
         * Fonts.
         */
        "ttf" | "otf" | "woff" | "woff2" | "eot" | "fon" | "pcf" | "bdf" => FileClass::Font,

        /*
         * Databases.
         */
        "db" | "sqlite" | "sqlite3" | "mdb" | "accdb" | "sql" | "dbf" | "realm" => {
            FileClass::Database
        }

        "torrent" => FileClass::Torrent,

        "desktop" => FileClass::DesktopEntry,

        /*
         * Backups and temporary copies.
         */
        "bak" | "backup" | "old" | "orig" | "save" | "tmp" | "temp" | "swp" | "swo" => {
            FileClass::Backup
        }

        /*
         * Certificates, keys, and signatures.
         */
        "pem" | "crt" | "cer" | "der" | "p12" | "pfx" | "csr" | "pub" | "asc" | "sig" | "gpg"
        | "pgp" => FileClass::Certificate,

        /*
         * Disk and optical images.
         */
        "iso" | "img" | "dmg" | "vhd" | "vhdx" | "vdi" | "vmdk" | "qcow" | "qcow2" | "bin"
        | "cue" => FileClass::DiskImage,

        /*
         * Shared libraries, plugins, and loadable modules.
         */
        "so" | "dylib" | "dll" | "ko" | "a" | "o" | "obj" | "class" | "pyc" | "pyo" | "wasm"
        | "plugin" => FileClass::Plugin,

        /*
         * Plain text whose role is not otherwise known.
         */
        "text" | "nfo" | "dic" | "dict" | "words" => FileClass::Text,

        _ => {
            return None;
        }
    };

    Some(class)
}

fn looks_like_backup_name(filename: &str) -> bool {
    filename.ends_with('~')
        || filename.starts_with(".#")
        || filename.starts_with("#") && filename.ends_with("#")
}

fn is_executable(metadata: &EntryMetadata) -> bool {
    metadata.permissions_mode & 0o111 != 0
}

// SPDX-License-Identifier: BSD-3-Clause

use std::{
    fs,
    path::{Path, PathBuf},
};

use ratatui::style::Color;
use serde::Deserialize;

use crate::config::config_directory_path;

const THEMES_DIRECTORY_NAME: &str = "themes";

const THEME_FILE_EXTENSION: &str = "toml";

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub ui: UiTheme,

    pub frames: FrameTheme,

    pub selection: SelectionTheme,

    pub scrollbar: ScrollbarTheme,

    pub permissions: PermissionTheme,

    pub icons: IconTheme,
}

#[derive(Debug, Clone, Copy)]
pub struct UiTheme {
    pub frame: Color,

    pub directory: Color,

    pub file: Color,

    pub symlink: Color,

    pub muted: Color,

    pub error: Color,

    pub query: Color,

    pub search_match: Color,

    pub classification: Color,

    pub date: Color,

    pub user: Color,

    pub size: Color,
}

#[derive(Debug, Clone, Copy)]
pub struct FrameTheme {
    pub search: Color,

    pub details: Color,

    pub metadata: Color,

    pub entries: Color,

    pub selection: Color,

    /*
     * Shared border color for every modal popup:
     *
     * Help, About, SSH Connections, Transfer, and Deletion.
     */
    pub popup: Color,

    pub parent_brackets: Color,

    pub parent_text: Color,
}

#[derive(Debug, Clone, Copy)]
pub struct SelectionTheme {
    pub text: Color,

    pub background: Color,
}

#[derive(Debug, Clone, Copy)]
pub struct ScrollbarTheme {
    pub thumb: Color,

    pub track: Color,
}

#[derive(Debug, Clone, Copy)]
pub struct PermissionTheme {
    pub read: Color,

    pub write: Color,

    pub execute: Color,

    pub file_type: Color,

    pub missing: Color,

    pub special: Color,
}

#[derive(Debug, Clone, Copy)]
pub struct IconTheme {
    pub directory: Color,

    pub symlink: Color,

    pub rust: Color,

    pub python: Color,

    pub shell: Color,

    pub source: Color,

    pub java: Color,

    pub javascript: Color,

    pub web: Color,

    pub config: Color,

    pub archive: Color,

    pub document: Color,

    pub spreadsheet: Color,

    pub presentation: Color,

    pub image: Color,

    pub audio: Color,

    pub video: Color,

    pub font: Color,

    pub database: Color,

    pub log: Color,

    pub backup: Color,

    pub certificate: Color,

    pub disk_image: Color,

    pub torrent: Color,

    pub desktop_plugin: Color,

    pub binary: Color,

    pub unknown: Color,
}

impl Default for Theme {
    fn default() -> Self {
        let directory = Color::Rgb(80, 155, 235);

        let file = Color::Rgb(195, 200, 210);

        let symlink = Color::Rgb(75, 195, 210);

        let frame = Color::Rgb(160, 110, 220);

        let muted = Color::Rgb(95, 105, 120);

        let query = Color::Rgb(110, 220, 225);

        let classification = Color::Rgb(240, 240, 245);

        Self {
            ui: UiTheme {
                frame,

                directory,

                file,

                symlink,

                muted,

                error: Color::Rgb(220, 55, 70),

                query,

                search_match: Color::Rgb(166, 119, 199),

                classification,

                date: directory,

                user: Color::Rgb(91, 93, 99),

                size: query,
            },

            frames: FrameTheme {
                search: frame,

                details: frame,

                metadata: frame,

                entries: frame,

                selection: frame,

                popup: frame,

                parent_brackets: frame,

                parent_text: query,
            },

            selection: SelectionTheme {
                text: Color::Rgb(240, 240, 245),

                background: Color::Rgb(55, 40, 75),
            },

            scrollbar: ScrollbarTheme {
                thumb: frame,

                track: Color::Rgb(45, 50, 60),
            },

            permissions: PermissionTheme {
                read: muted,

                write: directory,

                execute: frame,

                file_type: file,

                missing: muted,

                special: classification,
            },

            /*
             * These are the stronger icon colors currently used by Scry.
             *
             * File names retain the ordinary file palette. Only the compact
             * classification icons use these more saturated colors.
             */
            icons: IconTheme {
                directory,

                symlink,

                rust: Color::Rgb(230, 125, 70),

                python: Color::Rgb(80, 170, 235),

                shell: Color::Rgb(90, 200, 125),

                source: Color::Rgb(105, 145, 225),

                java: Color::Rgb(220, 105, 85),

                javascript: Color::Rgb(235, 205, 65),

                web: Color::Rgb(210, 100, 190),

                config: Color::Rgb(80, 185, 205),

                archive: Color::Rgb(215, 135, 80),

                document: Color::Rgb(195, 205, 220),

                spreadsheet: Color::Rgb(70, 195, 115),

                presentation: Color::Rgb(230, 135, 70),

                image: Color::Rgb(215, 105, 220),

                audio: Color::Rgb(105, 165, 225),

                video: Color::Rgb(195, 100, 220),

                font: Color::Rgb(195, 145, 225),

                database: Color::Rgb(70, 190, 205),

                log: Color::Rgb(155, 165, 180),

                backup: Color::Rgb(175, 125, 195),

                certificate: Color::Rgb(225, 190, 75),

                disk_image: Color::Rgb(125, 155, 215),

                torrent: Color::Rgb(80, 190, 145),

                desktop_plugin: Color::Rgb(155, 130, 220),

                binary: file,

                unknown: file,
            },
        }
    }
}

impl Theme {
    pub fn load(theme_name: &str) -> Self {
        let mut theme = Self::default();

        let theme_name = match normalized_theme_name(theme_name) {
            Ok(theme_name) => theme_name,

            Err(message) => {
                eprintln!(
                    "scry: invalid theme name '{}': {}; using built-in default",
                    theme_name, message,
                );

                return theme;
            }
        };

        let path = match theme_file_path(&theme_name) {
            Ok(path) => path,

            Err(error) => {
                eprintln!(
                    "scry: unable to determine the theme directory: {}; \
                     using built-in default",
                    error,
                );

                return theme;
            }
        };

        let content = match fs::read_to_string(&path) {
            Ok(content) => content,

            /*
             * The built-in default theme does not require a file.
             *
             * A user may still create themes/default.toml to override parts of
             * the compiled palette.
             */
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound && theme_name == "default" =>
            {
                return theme;
            }

            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                eprintln!(
                    "scry: theme '{}' was not found at {}; \
                     using built-in default",
                    theme_name,
                    path.display(),
                );

                return theme;
            }

            Err(error) => {
                eprintln!(
                    "scry: unable to read theme {}: {}; \
                     using built-in default",
                    path.display(),
                    error,
                );

                return theme;
            }
        };

        let file_theme = match toml::from_str::<ThemeFile>(&content) {
            Ok(file_theme) => file_theme,

            Err(error) => {
                eprintln!(
                    "scry: unable to parse theme {}: {}; \
                     using built-in default",
                    path.display(),
                    error,
                );

                return theme;
            }
        };

        theme.apply_file_theme(file_theme, &path);

        theme
    }

    fn apply_file_theme(&mut self, file_theme: ThemeFile, path: &Path) {
        if let Some(value) = file_theme.ui.frame {
            match parse_hex_color(&value) {
                Ok(color) => {
                    self.ui.frame = color;

                    /*
                     * ui.frame is the common fallback for every principal pane.
                     *
                     * Individual [frames] values below may override any of these.
                     */
                    self.frames.search = color;

                    self.frames.details = color;

                    self.frames.metadata = color;

                    self.frames.entries = color;

                    self.frames.selection = color;

                    self.frames.popup = color;
                }

                Err(message) => {
                    eprintln!(
                        "scry: invalid color '{}' for ui.frame in {}: {}; \
                        keeping the built-in value",
                        value,
                        path.display(),
                        message,
                    );
                }
            }
        }

        apply_optional_color(
            &mut self.ui.directory,
            file_theme.ui.directory,
            path,
            "ui.directory",
        );

        apply_optional_color(
            &mut self.frames.search,
            file_theme.frames.search,
            path,
            "frames.search",
        );

        apply_optional_color(
            &mut self.frames.details,
            file_theme.frames.details,
            path,
            "frames.details",
        );

        apply_optional_color(
            &mut self.frames.metadata,
            file_theme.frames.metadata,
            path,
            "frames.metadata",
        );

        apply_optional_color(
            &mut self.frames.entries,
            file_theme.frames.entries,
            path,
            "frames.entries",
        );

        apply_optional_color(
            &mut self.frames.selection,
            file_theme.frames.selection,
            path,
            "frames.selection",
        );

        apply_optional_color(
            &mut self.frames.popup,
            file_theme.frames.popup,
            path,
            "frames.popup",
        );

        apply_optional_color(
            &mut self.frames.parent_brackets,
            file_theme.frames.parent_brackets,
            path,
            "frames.parent_brackets",
        );

        apply_optional_color(
            &mut self.frames.parent_text,
            file_theme.frames.parent_text,
            path,
            "frames.parent_text",
        );

        apply_optional_color(&mut self.ui.file, file_theme.ui.file, path, "ui.file");

        apply_optional_color(
            &mut self.ui.symlink,
            file_theme.ui.symlink,
            path,
            "ui.symlink",
        );

        apply_optional_color(&mut self.ui.muted, file_theme.ui.muted, path, "ui.muted");

        apply_optional_color(&mut self.ui.error, file_theme.ui.error, path, "ui.error");

        apply_optional_color(&mut self.ui.query, file_theme.ui.query, path, "ui.query");

        apply_optional_color(
            &mut self.ui.search_match,
            file_theme.ui.search_match,
            path,
            "ui.search_match",
        );

        apply_optional_color(
            &mut self.ui.classification,
            file_theme.ui.classification,
            path,
            "ui.classification",
        );

        apply_optional_color(&mut self.ui.date, file_theme.ui.date, path, "ui.date");

        apply_optional_color(&mut self.ui.user, file_theme.ui.user, path, "ui.user");

        apply_optional_color(&mut self.ui.size, file_theme.ui.size, path, "ui.size");

        apply_optional_color(
            &mut self.selection.text,
            file_theme.selection.text,
            path,
            "selection.text",
        );

        apply_optional_color(
            &mut self.selection.background,
            file_theme.selection.background,
            path,
            "selection.background",
        );

        apply_optional_color(
            &mut self.scrollbar.thumb,
            file_theme.scrollbar.thumb,
            path,
            "scrollbar.thumb",
        );

        apply_optional_color(
            &mut self.scrollbar.track,
            file_theme.scrollbar.track,
            path,
            "scrollbar.track",
        );

        apply_optional_color(
            &mut self.permissions.read,
            file_theme.permissions.read,
            path,
            "permissions.read",
        );

        apply_optional_color(
            &mut self.permissions.write,
            file_theme.permissions.write,
            path,
            "permissions.write",
        );

        apply_optional_color(
            &mut self.permissions.execute,
            file_theme.permissions.execute,
            path,
            "permissions.execute",
        );

        apply_optional_color(
            &mut self.permissions.file_type,
            file_theme.permissions.file_type,
            path,
            "permissions.file_type",
        );

        apply_optional_color(
            &mut self.permissions.missing,
            file_theme.permissions.missing,
            path,
            "permissions.missing",
        );

        apply_optional_color(
            &mut self.permissions.special,
            file_theme.permissions.special,
            path,
            "permissions.special",
        );

        apply_optional_color(
            &mut self.icons.directory,
            file_theme.icons.directory,
            path,
            "icons.directory",
        );

        apply_optional_color(
            &mut self.icons.symlink,
            file_theme.icons.symlink,
            path,
            "icons.symlink",
        );

        apply_optional_color(
            &mut self.icons.rust,
            file_theme.icons.rust,
            path,
            "icons.rust",
        );

        apply_optional_color(
            &mut self.icons.python,
            file_theme.icons.python,
            path,
            "icons.python",
        );

        apply_optional_color(
            &mut self.icons.shell,
            file_theme.icons.shell,
            path,
            "icons.shell",
        );

        apply_optional_color(
            &mut self.icons.source,
            file_theme.icons.source,
            path,
            "icons.source",
        );

        apply_optional_color(
            &mut self.icons.java,
            file_theme.icons.java,
            path,
            "icons.java",
        );

        apply_optional_color(
            &mut self.icons.javascript,
            file_theme.icons.javascript,
            path,
            "icons.javascript",
        );

        apply_optional_color(&mut self.icons.web, file_theme.icons.web, path, "icons.web");

        apply_optional_color(
            &mut self.icons.config,
            file_theme.icons.config,
            path,
            "icons.config",
        );

        apply_optional_color(
            &mut self.icons.archive,
            file_theme.icons.archive,
            path,
            "icons.archive",
        );

        apply_optional_color(
            &mut self.icons.document,
            file_theme.icons.document,
            path,
            "icons.document",
        );

        apply_optional_color(
            &mut self.icons.spreadsheet,
            file_theme.icons.spreadsheet,
            path,
            "icons.spreadsheet",
        );

        apply_optional_color(
            &mut self.icons.presentation,
            file_theme.icons.presentation,
            path,
            "icons.presentation",
        );

        apply_optional_color(
            &mut self.icons.image,
            file_theme.icons.image,
            path,
            "icons.image",
        );

        apply_optional_color(
            &mut self.icons.audio,
            file_theme.icons.audio,
            path,
            "icons.audio",
        );

        apply_optional_color(
            &mut self.icons.video,
            file_theme.icons.video,
            path,
            "icons.video",
        );

        apply_optional_color(
            &mut self.icons.font,
            file_theme.icons.font,
            path,
            "icons.font",
        );

        apply_optional_color(
            &mut self.icons.database,
            file_theme.icons.database,
            path,
            "icons.database",
        );

        apply_optional_color(&mut self.icons.log, file_theme.icons.log, path, "icons.log");

        apply_optional_color(
            &mut self.icons.backup,
            file_theme.icons.backup,
            path,
            "icons.backup",
        );

        apply_optional_color(
            &mut self.icons.certificate,
            file_theme.icons.certificate,
            path,
            "icons.certificate",
        );

        apply_optional_color(
            &mut self.icons.disk_image,
            file_theme.icons.disk_image,
            path,
            "icons.disk_image",
        );

        apply_optional_color(
            &mut self.icons.torrent,
            file_theme.icons.torrent,
            path,
            "icons.torrent",
        );

        apply_optional_color(
            &mut self.icons.desktop_plugin,
            file_theme.icons.desktop_plugin,
            path,
            "icons.desktop_plugin",
        );

        apply_optional_color(
            &mut self.icons.binary,
            file_theme.icons.binary,
            path,
            "icons.binary",
        );

        apply_optional_color(
            &mut self.icons.unknown,
            file_theme.icons.unknown,
            path,
            "icons.unknown",
        );
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ThemeFile {
    ui: UiThemeFile,

    frames: FrameThemeFile,

    selection: SelectionThemeFile,

    scrollbar: ScrollbarThemeFile,

    permissions: PermissionThemeFile,

    icons: IconThemeFile,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct UiThemeFile {
    frame: Option<String>,

    directory: Option<String>,

    file: Option<String>,

    symlink: Option<String>,

    muted: Option<String>,

    error: Option<String>,

    query: Option<String>,

    search_match: Option<String>,

    classification: Option<String>,

    date: Option<String>,

    user: Option<String>,

    size: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct FrameThemeFile {
    search: Option<String>,

    details: Option<String>,

    metadata: Option<String>,

    entries: Option<String>,

    selection: Option<String>,

    popup: Option<String>,

    parent_brackets: Option<String>,

    parent_text: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SelectionThemeFile {
    text: Option<String>,

    background: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ScrollbarThemeFile {
    thumb: Option<String>,

    track: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PermissionThemeFile {
    read: Option<String>,

    write: Option<String>,

    execute: Option<String>,

    file_type: Option<String>,

    missing: Option<String>,

    special: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct IconThemeFile {
    directory: Option<String>,

    symlink: Option<String>,

    rust: Option<String>,

    python: Option<String>,

    shell: Option<String>,

    source: Option<String>,

    java: Option<String>,

    javascript: Option<String>,

    web: Option<String>,

    config: Option<String>,

    archive: Option<String>,

    document: Option<String>,

    spreadsheet: Option<String>,

    presentation: Option<String>,

    image: Option<String>,

    audio: Option<String>,

    video: Option<String>,

    font: Option<String>,

    database: Option<String>,

    log: Option<String>,

    backup: Option<String>,

    certificate: Option<String>,

    disk_image: Option<String>,

    torrent: Option<String>,

    desktop_plugin: Option<String>,

    binary: Option<String>,

    unknown: Option<String>,
}

pub fn themes_directory_path() -> std::io::Result<PathBuf> {
    Ok(config_directory_path()?.join(THEMES_DIRECTORY_NAME))
}

pub fn theme_file_path(theme_name: &str) -> std::io::Result<PathBuf> {
    Ok(themes_directory_path()?.join(format!("{}.{}", theme_name, THEME_FILE_EXTENSION,)))
}

fn normalized_theme_name(theme_name: &str) -> Result<String, &'static str> {
    let theme_name = theme_name.trim();

    if theme_name.is_empty() {
        return Ok("default".to_string());
    }

    if !theme_name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err("theme names may contain only letters, numbers, hyphens, and underscores");
    }

    Ok(theme_name.to_string())
}

fn apply_optional_color(target: &mut Color, value: Option<String>, path: &Path, field_name: &str) {
    let Some(value) = value else {
        return;
    };

    match parse_hex_color(&value) {
        Ok(color) => {
            *target = color;
        }

        Err(message) => {
            eprintln!(
                "scry: invalid color '{}' for {} in {}: {}; \
                 keeping the built-in value",
                value,
                field_name,
                path.display(),
                message,
            );
        }
    }
}

fn parse_hex_color(value: &str) -> Result<Color, &'static str> {
    let value = value.trim();

    let hexadecimal = value.strip_prefix('#').unwrap_or(value);

    if hexadecimal.len() != 6 {
        return Err("expected hexadecimal color in #RRGGBB format");
    }

    if !hexadecimal.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("color contains non-hexadecimal characters");
    }

    let red =
        u8::from_str_radix(&hexadecimal[0..2], 16).map_err(|_| "unable to parse red component")?;

    let green = u8::from_str_radix(&hexadecimal[2..4], 16)
        .map_err(|_| "unable to parse green component")?;

    let blue =
        u8::from_str_radix(&hexadecimal[4..6], 16).map_err(|_| "unable to parse blue component")?;

    Ok(Color::Rgb(red, green, blue))
}

#[cfg(test)]
mod tests {
    use super::parse_hex_color;
    use ratatui::style::Color;

    #[test]
    fn parses_hash_prefixed_hexadecimal_color() {
        assert_eq!(parse_hex_color("#A06EDC"), Ok(Color::Rgb(160, 110, 220)),);
    }

    #[test]
    fn parses_unprefixed_hexadecimal_color() {
        assert_eq!(parse_hex_color("509BEB"), Ok(Color::Rgb(80, 155, 235)),);
    }

    #[test]
    fn rejects_short_hexadecimal_color() {
        assert!(parse_hex_color("#FFF").is_err());
    }

    #[test]
    fn rejects_invalid_hexadecimal_characters() {
        assert!(parse_hex_color("#GG0000").is_err());
    }
}

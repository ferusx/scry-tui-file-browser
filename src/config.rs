// SPDX-License-Identifier: BSD-3-Clause

use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::Deserialize;

const CONFIG_FILENAME: &str = "scry.toml";

const GENERATED_CONFIG_FILENAME: &str = "scry.toml.generated";

const DEFAULT_CONFIG: &str = r#"# Scry configuration
#
# Command-line options override settings from this file.
#
# Themes must be put in ~/.config/scry/themes/
 
# Theme
#
# Sets Scry's active theme style 
#
theme = "default"


# DISPLAY
#
# The five first entries enable hidden entries, icons 
# and show/hide Scry's foldable sections: details panel,
# selection panel, and the meta panel which holds the 
# columns.
#
# The last four entries show/hide the metal panel'S
# columns.
#
[display]
show_hidden = false
show_icons = true
show_details = true
show_selection = true
show_columns = true
show_permissions = false
show_size = false
show_date = false
show_user = false

# BROWSER
#
# 'view' has two modes: list mode (normal view) and tree mode
# 'sort' has four fields: name|size|date|type
#
[browser]
view = "list"
recursive = false
fuzzy = false
entry_filter = "all"
sort = "name"
reverse = false

# FEATURES
#
# Here deletion can be enabled, external file opening can
# be disabled, and Scry can optionally exit after opening
# a selected file successfully.
#
[features]
enable_deletion = false
allow_file_opening = true
exit_on_open = false

# SESSION
#
# When enabled, a plain `scry` launch restores the browser state saved when
# Scry last exited. Explicit paths, SSH targets, and command-line startup
# options continue to override restored values.
#
[session]
restore_session = false

# SSH
#
# preserve_hierarchy preserves remote directory paths during marked batch downloads.
#
# false places every downloaded file directly inside the new batch directory.
# true recreates the remote directory hierarchy beneath that directory.
# 
[ssh]
connect_timeout_seconds = 10
server_alive_interval_seconds = 15
preserve_hierarchy = false
"#;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ScryConfig {
    /*
     * This remains a top-level TOML value.
     *
     * It must therefore appear before the first [section] in scry.toml.
     */
    pub theme: String,

    pub display: DisplayConfig,

    pub browser: BrowserConfig,

    pub features: FeatureConfig,

    pub session: SessionConfig,

    pub ssh: SshConfig,
}

impl Default for ScryConfig {
    fn default() -> Self {
        Self {
            theme: "default".to_string(),

            display: DisplayConfig::default(),

            browser: BrowserConfig::default(),

            features: FeatureConfig::default(),

            session: SessionConfig::default(),

            ssh: SshConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    pub show_hidden: bool,

    pub show_icons: bool,

    pub show_details: bool,

    pub show_selection: bool,

    pub show_columns: bool,

    pub show_permissions: bool,

    pub show_size: bool,

    pub show_date: bool,

    pub show_user: bool,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            show_hidden: false,

            show_icons: true,

            show_details: true,

            show_selection: true,

            show_columns: true,

            show_permissions: false,

            show_size: false,

            show_date: false,

            show_user: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BrowserConfig {
    pub view: String,

    pub recursive: bool,

    pub fuzzy: bool,

    pub entry_filter: String,

    pub sort: String,

    pub reverse: bool,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            view: "list".to_string(),

            recursive: false,

            fuzzy: false,

            entry_filter: "all".to_string(),

            sort: "name".to_string(),

            reverse: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default)]
pub struct FeatureConfig {
    /*
     * Permit deletion of selected local filesystem entries.
     *
     * This feature is deliberately disabled by default. When disabled,
     * deletion controls remain absent from Scry's interface.
     */
    pub enable_deletion: bool,

    /*
     * Permit selected files to be opened externally.
     *
     * Directory navigation remains available even when this is false.
     */
    pub allow_file_opening: bool,

    /*
     * Exit Scry after an externally opened file is launched successfully.
     *
     * Directory navigation and failed file-open attempts leave Scry running.
     */
    pub exit_on_open: bool,
}

impl Default for FeatureConfig {
    fn default() -> Self {
        Self {
            enable_deletion: false,

            allow_file_opening: true,

            exit_on_open: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    /*
     * Restore the most recently saved browser session when Scry is launched
     * without an explicit path or SSH target.
     *
     * Disabled by default so ordinary startup behavior remains unchanged unless
     * the user deliberately opts in through scry.toml or --restore-session.
     */
    pub restore_session: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            restore_session: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default)]
pub struct SshConfig {
    /*
     * Maximum time allowed for OpenSSH to establish the connection.
     */
    pub connect_timeout_seconds: u64,

    /*
     * Interval between OpenSSH keepalive messages.
     *
     * A value of zero disables server-alive messages.
     */
    pub server_alive_interval_seconds: u64,

    /*
     * Preserve the complete remote directory hierarchy beneath batch-download
     * directories.
     *
     * When false, every marked file is placed directly inside the batch root.
     * Duplicate filenames are disambiguated without overwriting existing files.
     */
    pub preserve_hierarchy: bool,
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            connect_timeout_seconds: 10,

            server_alive_interval_seconds: 15,

            preserve_hierarchy: false,
        }
    }
}

impl ScryConfig {
    pub fn load() -> Self {
        let path = match config_file_path() {
            Ok(path) => path,

            Err(error) => {
                eprintln!(
                    "scry: unable to determine the configuration path: {}",
                    error,
                );

                return Self::default();
            }
        };

        match fs::read_to_string(&path) {
            Ok(content) => match toml::from_str::<Self>(&content) {
                Ok(mut config) => {
                    config.normalize();

                    config
                }

                Err(error) => {
                    eprintln!(
                        "scry: unable to parse {}: {}; using built-in defaults",
                        path.display(),
                        error,
                    );

                    Self::default()
                }
            },

            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if let Err(error) = create_default_config(&path) {
                    eprintln!("scry: unable to create {}: {}", path.display(), error,);
                }

                Self::default()
            }

            Err(error) => {
                eprintln!(
                    "scry: unable to read {}: {}; using built-in defaults",
                    path.display(),
                    error,
                );

                Self::default()
            }
        }
    }

    fn normalize(&mut self) {
        self.theme = normalized_nonempty(&self.theme, "default");

        self.browser.view = match self.browser.view.trim().to_lowercase().as_str() {
            "list" => "list".to_string(),

            "tree" => "tree".to_string(),

            invalid => {
                eprintln!("scry: unknown browser view '{}'; using 'list'", invalid,);

                "list".to_string()
            }
        };

        self.browser.entry_filter = match self.browser.entry_filter.trim().to_lowercase().as_str() {
            "all" => "all".to_string(),

            "files" | "file" | "files-only" => "files".to_string(),

            "directories" | "directory" | "dirs" | "dir" | "dirs-only" => "directories".to_string(),

            invalid => {
                eprintln!(
                    "scry: unknown browser entry filter '{}'; using 'all'",
                    invalid,
                );

                "all".to_string()
            }
        };

        self.browser.sort = match self.browser.sort.trim().to_lowercase().as_str() {
            "name" => "name".to_string(),

            "size" => "size".to_string(),

            "date" | "modified" => "date".to_string(),

            "type" => "type".to_string(),

            invalid => {
                eprintln!("scry: unknown sort mode '{}'; using 'name'", invalid,);

                "name".to_string()
            }
        };

        /*
         * A zero-second connection timeout would make ordinary SSH connections
         * practically unusable. Restore the documented default instead.
         */
        if self.ssh.connect_timeout_seconds == 0 {
            eprintln!("scry: ssh.connect_timeout_seconds must be greater than zero; using 10",);

            self.ssh.connect_timeout_seconds = 10;
        }

        /*
         * Prevent accidental multi-hour hangs caused by a mistyped configuration
         * value while still allowing reasonably slow networks.
         */
        const MAX_CONNECT_TIMEOUT_SECONDS: u64 = 600;

        if self.ssh.connect_timeout_seconds > MAX_CONNECT_TIMEOUT_SECONDS {
            eprintln!(
                "scry: ssh.connect_timeout_seconds cannot exceed {}; using {}",
                MAX_CONNECT_TIMEOUT_SECONDS, MAX_CONNECT_TIMEOUT_SECONDS,
            );

            self.ssh.connect_timeout_seconds = MAX_CONNECT_TIMEOUT_SECONDS;
        }

        /*
         * Zero deliberately disables keepalive messages. Cap nonzero intervals at
         * one hour so a malformed value cannot overflow or become meaningless.
         */
        const MAX_SERVER_ALIVE_INTERVAL_SECONDS: u64 = 3600;

        if self.ssh.server_alive_interval_seconds > MAX_SERVER_ALIVE_INTERVAL_SECONDS {
            eprintln!(
                "scry: ssh.server_alive_interval_seconds cannot exceed {}; using {}",
                MAX_SERVER_ALIVE_INTERVAL_SECONDS, MAX_SERVER_ALIVE_INTERVAL_SECONDS,
            );

            self.ssh.server_alive_interval_seconds = MAX_SERVER_ALIVE_INTERVAL_SECONDS;
        }
    }
}

pub fn config_directory_path() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path).join("scry"));
    }

    let home = env::var_os("HOME").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "neither XDG_CONFIG_HOME nor HOME is set",
        )
    })?;

    Ok(PathBuf::from(home).join(".config").join("scry"))
}

pub fn config_file_path() -> io::Result<PathBuf> {
    Ok(config_directory_path()?.join(CONFIG_FILENAME))
}

pub fn generated_config_file_path() -> io::Result<PathBuf> {
    Ok(config_directory_path()?.join(GENERATED_CONFIG_FILENAME))
}

pub fn generate_config_copy() -> io::Result<PathBuf> {
    let path = generated_config_file_path()?;

    let Some(parent) = path.parent() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "generated configuration path has no parent directory",
        ));
    };

    fs::create_dir_all(parent)?;

    /*
     * The generated template is disposable and may always be refreshed.
     *
     * This operation never touches the active scry.toml configuration.
     * An existing scry.toml.generated is replaced with the newest built-in
     * template so newly introduced settings appear immediately.
     */
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)?;

    file.write_all(DEFAULT_CONFIG.as_bytes())?;

    file.flush()?;

    Ok(path)
}

fn create_default_config(path: &Path) -> io::Result<()> {
    let Some(parent) = path.parent() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "configuration path has no parent directory",
        ));
    };

    fs::create_dir_all(parent)?;

    /*
     * create_new prevents an existing user configuration from ever being
     * overwritten if another Scry process creates it at the same time.
     */
    let mut file = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => file,

        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Ok(());
        }

        Err(error) => {
            return Err(error);
        }
    };

    file.write_all(DEFAULT_CONFIG.as_bytes())?;

    file.flush()
}

fn normalized_nonempty(value: &str, fallback: &str) -> String {
    let value = value.trim();

    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

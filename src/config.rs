// SPDX-License-Identifier: BSD-3-Clause

use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::Deserialize;

const CONFIG_FILENAME: &str = "scry.toml";

const DEFAULT_CONFIG: &str = r#"# Scry configuration
#
# Command-line options override settings from this file.
#
# Available themes will be read from Scry's themes directory.

theme = "default"

[display]
show_hidden = false
show_details = true
show_selection = true
show_columns = true
show_permissions = false
show_size = false
show_date = false
show_user = false

[browser]
view = "list"
recursive = false
sort = "name"
reverse = false

[features]
enable_deletion = false

[ssh]
connect_timeout_seconds = 10
server_alive_interval_seconds = 15
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

    pub ssh: SshConfig,
}

impl Default for ScryConfig {
    fn default() -> Self {
        Self {
            theme: "default".to_string(),

            display: DisplayConfig::default(),

            browser: BrowserConfig::default(),

            features: FeatureConfig::default(),

            ssh: SshConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    pub show_hidden: bool,

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

    pub sort: String,

    pub reverse: bool,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            view: "list".to_string(),

            recursive: false,

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
}

impl Default for FeatureConfig {
    fn default() -> Self {
        Self {
            enable_deletion: false,
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
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            connect_timeout_seconds: 10,

            server_alive_interval_seconds: 15,
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

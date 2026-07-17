// SPDX-License-Identifier: BSD-3-Clause

use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

const CONNECTIONS_FILENAME: &str = "connections.json";

const CONNECTIONS_PART_SUFFIX: &str = ".scry-part";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionProfile {
    pub name: String,

    pub host: String,

    #[serde(default)]
    pub username: String,

    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default)]
    pub identity_file: String,

    #[serde(default)]
    pub start_directory: String,
}

impl Default for ConnectionProfile {
    fn default() -> Self {
        Self {
            name: String::new(),

            host: String::new(),

            username: String::new(),

            port: default_port(),

            identity_file: String::new(),

            start_directory: String::new(),
        }
    }
}

impl ConnectionProfile {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("Profile name cannot be empty".to_string());
        }

        if self.host.trim().is_empty() {
            return Err("Host cannot be empty".to_string());
        }

        if self.port == 0 {
            return Err("SSH port must be between 1 and 65535".to_string());
        }

        Ok(())
    }

    pub fn normalized(mut self) -> Self {
        self.name = self.name.trim().to_string();

        self.host = self.host.trim().to_string();

        self.username = self.username.trim().to_string();

        self.identity_file = self.identity_file.trim().to_string();

        self.start_directory = self.start_directory.trim().to_string();

        self
    }

    pub fn destination_label(&self) -> String {
        if self.username.is_empty() {
            self.host.clone()
        } else {
            format!("{}@{}", self.username, self.host,)
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct StoredConnections {
    #[serde(default)]
    profiles: Vec<ConnectionProfile>,
}

#[derive(Debug, Clone, Default)]
pub struct ConnectionStore {
    profiles: Vec<ConnectionProfile>,
}

impl ConnectionStore {
    pub fn load() -> io::Result<Self> {
        let path = connections_file_path()?;

        let content = match fs::read_to_string(&path) {
            Ok(content) => content,

            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }

            Err(error) => {
                return Err(error);
            }
        };

        let stored: StoredConnections = serde_json::from_str(&content).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unable to read {}: {}", path.display(), error,),
            )
        })?;

        let mut profiles: Vec<ConnectionProfile> = stored
            .profiles
            .into_iter()
            .map(ConnectionProfile::normalized)
            .filter(|profile| {
                !profile.name.is_empty() && !profile.host.is_empty() && profile.port != 0
            })
            .collect();

        sort_profiles(&mut profiles);

        Ok(Self { profiles })
    }

    pub fn profiles(&self) -> &[ConnectionProfile] {
        &self.profiles
    }

    pub fn profile(&self, index: usize) -> Option<&ConnectionProfile> {
        self.profiles.get(index)
    }

    pub fn find_by_name(&self, name: &str) -> Option<usize> {
        self.profiles
            .iter()
            .position(|profile| profile.name.eq_ignore_ascii_case(name.trim()))
    }

    pub fn save_profile(&mut self, profile: ConnectionProfile) -> Result<usize, String> {
        let profile = profile.normalized();

        profile.validate()?;

        let profile_name = profile.name.clone();

        if let Some(index) = self.find_by_name(&profile_name) {
            self.profiles[index] = profile;
        } else {
            self.profiles.push(profile);
        }

        sort_profiles(&mut self.profiles);

        self.persist().map_err(|error| error.to_string())?;

        self.find_by_name(&profile_name)
            .ok_or_else(|| "saved profile disappeared unexpectedly".to_string())
    }

    #[allow(dead_code)]
    pub fn remove_profile(&mut self, index: usize) -> io::Result<Option<ConnectionProfile>> {
        if index >= self.profiles.len() {
            return Ok(None);
        }

        let removed = self.profiles.remove(index);

        self.persist()?;

        Ok(Some(removed))
    }

    pub fn persist(&self) -> io::Result<()> {
        let path = connections_file_path()?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;

            make_directory_private(parent)?;
        }

        let stored = StoredConnections {
            profiles: self.profiles.clone(),
        };

        let content = serde_json::to_string_pretty(&stored).map_err(io::Error::other)?;

        let part_path = append_suffix(&path, CONNECTIONS_PART_SUFFIX);

        let write_result = write_private_file(&part_path, format!("{content}\n").as_bytes());

        if let Err(error) = write_result {
            let _ = fs::remove_file(&part_path);

            return Err(error);
        }

        replace_file_atomically(&part_path, &path)?;

        make_file_private(&path)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionField {
    Profiles,

    Name,

    Host,

    Username,

    Port,

    IdentityFile,

    StartDirectory,

    Connect,

    Save,

    Delete,

    Disconnect,
}

#[derive(Debug, Clone)]
pub struct ConnectionDialogState {
    pub selected_profile: usize,

    pub focus: ConnectionField,

    pub draft: ConnectionProfile,

    /*
     * Keep the port as editable text.
     *
     * A user must be allowed to temporarily erase "22" while entering another
     * port. It is converted to u16 only when saving or connecting.
     */
    pub port_text: String,

    pub error_message: Option<String>,
}

impl ConnectionDialogState {
    pub fn new(store: &ConnectionStore) -> Self {
        let draft = store.profile(0).cloned().unwrap_or_default();

        let port_text = draft.port.to_string();

        Self {
            selected_profile: 0,

            focus: if store.profiles().is_empty() {
                ConnectionField::Name
            } else {
                ConnectionField::Profiles
            },

            draft,

            port_text,

            error_message: None,
        }
    }

    pub fn load_selected_profile(&mut self, store: &ConnectionStore) {
        if store.profiles().is_empty() {
            self.selected_profile = 0;

            self.draft = ConnectionProfile::default();

            self.port_text = default_port().to_string();

            self.focus = ConnectionField::Name;

            self.error_message = None;

            return;
        }

        self.selected_profile = self
            .selected_profile
            .min(store.profiles().len().saturating_sub(1));

        if let Some(profile) = store.profile(self.selected_profile) {
            self.draft = profile.clone();

            self.port_text = profile.port.to_string();
        }

        self.error_message = None;
    }

    #[allow(dead_code)]
    pub fn begin_new_profile(&mut self) {
        self.selected_profile = 0;

        self.draft = ConnectionProfile::default();

        self.port_text = default_port().to_string();

        self.focus = ConnectionField::Name;

        self.error_message = None;
    }

    pub fn focus_next(&mut self) {
        self.focus = match self.focus {
            ConnectionField::Profiles => ConnectionField::Name,

            ConnectionField::Name => ConnectionField::Host,

            ConnectionField::Host => ConnectionField::Username,

            ConnectionField::Username => ConnectionField::Port,

            ConnectionField::Port => ConnectionField::IdentityFile,

            ConnectionField::IdentityFile => ConnectionField::StartDirectory,

            ConnectionField::StartDirectory => ConnectionField::Connect,

            ConnectionField::Connect => ConnectionField::Save,

            ConnectionField::Save => ConnectionField::Delete,

            ConnectionField::Delete => ConnectionField::Disconnect,

            ConnectionField::Disconnect => ConnectionField::Profiles,
        };

        self.error_message = None;
    }

    pub fn focus_previous(&mut self) {
        self.focus = match self.focus {
            ConnectionField::Profiles => ConnectionField::Disconnect,

            ConnectionField::Name => ConnectionField::Profiles,

            ConnectionField::Host => ConnectionField::Name,

            ConnectionField::Username => ConnectionField::Host,

            ConnectionField::Port => ConnectionField::Username,

            ConnectionField::IdentityFile => ConnectionField::Port,

            ConnectionField::StartDirectory => ConnectionField::IdentityFile,

            ConnectionField::Connect => ConnectionField::StartDirectory,

            ConnectionField::Save => ConnectionField::Connect,

            ConnectionField::Delete => ConnectionField::Save,

            ConnectionField::Disconnect => ConnectionField::Delete,
        };

        self.error_message = None;
    }

    pub fn push_character(&mut self, character: char) {
        match self.focus {
            ConnectionField::Name => {
                self.draft.name.push(character);
            }

            ConnectionField::Host => {
                self.draft.host.push(character);
            }

            ConnectionField::Username => {
                self.draft.username.push(character);
            }

            ConnectionField::Port => {
                if character.is_ascii_digit() && self.port_text.len() < 5 {
                    self.port_text.push(character);
                }
            }

            ConnectionField::IdentityFile => {
                self.draft.identity_file.push(character);
            }

            ConnectionField::StartDirectory => {
                self.draft.start_directory.push(character);
            }

            _ => {}
        }

        self.error_message = None;
    }

    #[allow(dead_code)]
    pub fn pop_character(&mut self) {
        match self.focus {
            ConnectionField::Name => {
                self.draft.name.pop();
            }

            ConnectionField::Host => {
                self.draft.host.pop();
            }

            ConnectionField::Username => {
                self.draft.username.pop();
            }

            ConnectionField::Port => {
                self.port_text.pop();
            }

            ConnectionField::IdentityFile => {
                self.draft.identity_file.pop();
            }

            ConnectionField::StartDirectory => {
                self.draft.start_directory.pop();
            }

            _ => {}
        }

        self.error_message = None;
    }

    pub fn clear_focused_field(&mut self) {
        match self.focus {
            ConnectionField::Name => {
                self.draft.name.clear();
            }

            ConnectionField::Host => {
                self.draft.host.clear();
            }

            ConnectionField::Username => {
                self.draft.username.clear();
            }

            ConnectionField::Port => {
                self.port_text.clear();
            }

            ConnectionField::IdentityFile => {
                self.draft.identity_file.clear();
            }

            ConnectionField::StartDirectory => {
                self.draft.start_directory.clear();
            }

            _ => {}
        }

        self.error_message = None;
    }

    pub fn completed_profile(&self) -> Result<ConnectionProfile, String> {
        let port = self
            .port_text
            .trim()
            .parse::<u16>()
            .map_err(|_| "SSH port must be between 1 and 65535".to_string())?;

        if port == 0 {
            return Err("SSH port must be between 1 and 65535".to_string());
        }

        let mut profile = self.draft.clone();

        profile.port = port;

        let profile = profile.normalized();

        profile.validate()?;

        Ok(profile)
    }
}

fn default_port() -> u16 {
    22
}

fn sort_profiles(profiles: &mut [ConnectionProfile]) {
    profiles.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.name.cmp(&right.name))
    });
}

fn connections_file_path() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path).join("scry").join(CONNECTIONS_FILENAME));
    }

    let home = env::var_os("HOME").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "Neither XDG_CONFIG_HOME nor HOME is set",
        )
    })?;

    Ok(PathBuf::from(home)
        .join(".config")
        .join("scry")
        .join(CONNECTIONS_FILENAME))
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();

    value.push(suffix);

    PathBuf::from(value)
}

fn write_private_file(path: &Path, content: &[u8]) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;

    make_file_private(path)?;

    file.write_all(content)?;

    file.flush()?;

    file.sync_all()
}

fn replace_file_atomically(source: &Path, destination: &Path) -> io::Result<()> {
    match fs::rename(source, destination) {
        Ok(()) => {
            return Ok(());
        }

        Err(error)
            if error.kind() != io::ErrorKind::AlreadyExists
                && error.kind() != io::ErrorKind::PermissionDenied =>
        {
            return Err(error);
        }

        Err(_) => {}
    }

    match fs::remove_file(destination) {
        Ok(()) => {}

        Err(error) if error.kind() == io::ErrorKind::NotFound => {}

        Err(error) => {
            return Err(error);
        }
    }

    fs::rename(source, destination)
}

#[cfg(unix)]
fn make_directory_private(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn make_directory_private(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn make_file_private(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn make_file_private(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_uses_ssh_port() {
        assert_eq!(ConnectionProfile::default().port, 22,);
    }

    #[test]
    fn profile_normalization_trims_fields() {
        let profile = ConnectionProfile {
            name: "  Example Server  ".to_string(),

            host: "  server.example.test  ".to_string(),

            username: "  example-user  ".to_string(),

            port: 22,

            identity_file: "  ~/.ssh/example_key  ".to_string(),

            start_directory: "  /srv/example  ".to_string(),
        }
        .normalized();

        assert_eq!(profile.name, "Example Server",);

        assert_eq!(profile.host, "server.example.test",);

        assert_eq!(profile.username, "example-user",);

        assert_eq!(profile.identity_file, "~/.ssh/example_key",);

        assert_eq!(profile.start_directory, "/srv/example",);
    }

    #[test]
    fn empty_host_is_rejected() {
        let profile = ConnectionProfile {
            name: "Test".to_string(),

            host: String::new(),

            ..ConnectionProfile::default()
        };

        assert!(profile.validate().is_err(),);
    }

    #[test]
    fn destination_includes_username() {
        let profile = ConnectionProfile {
            host: "nosferatu".to_string(),

            username: "ferusx".to_string(),

            ..ConnectionProfile::default()
        };

        assert_eq!(profile.destination_label(), "ferusx@nosferatu",);
    }
}

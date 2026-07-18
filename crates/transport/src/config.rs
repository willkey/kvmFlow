use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use kvmflow_domain::{LocalControlPolicy, NetworkEndpoint, ScreenEdge};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tempfile::Builder;

use crate::TrustedPeer;

const CONFIG_SCHEMA_VERSION: u16 = 3;
pub const DEFAULT_MAC_LISTEN_PORT: u16 = 45_888;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowsRuntimeConfig {
    pub schema_version: u16,
    #[serde(default)]
    pub local_device_id: Option<String>,
    pub endpoint: Option<NetworkEndpoint>,
    pub edge: ScreenEdge,
    pub trusted_peer: Option<TrustedPeer>,
}

impl Default for WindowsRuntimeConfig {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            local_device_id: None,
            endpoint: None,
            edge: ScreenEdge::Bottom,
            trusted_peer: None,
        }
    }
}

impl WindowsRuntimeConfig {
    fn validate(mut self) -> Self {
        if self.schema_version < 2 && self.edge == ScreenEdge::Right {
            self.edge = ScreenEdge::Bottom;
        }
        if self
            .endpoint
            .as_ref()
            .is_some_and(|endpoint| endpoint.validate().is_err())
        {
            self.endpoint = None;
        }
        self.schema_version = CONFIG_SCHEMA_VERSION;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MacRuntimeConfig {
    pub schema_version: u16,
    #[serde(default)]
    pub local_device_id: Option<String>,
    pub listen_port: u16,
    pub local_policy: LocalControlPolicy,
    pub trusted_peer: Option<TrustedPeer>,
}

impl Default for MacRuntimeConfig {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            local_device_id: None,
            listen_port: DEFAULT_MAC_LISTEN_PORT,
            local_policy: LocalControlPolicy::Allowed,
            trusted_peer: None,
        }
    }
}

impl MacRuntimeConfig {
    fn validate(mut self) -> Self {
        if self.listen_port < 1024 {
            self.listen_port = DEFAULT_MAC_LISTEN_PORT;
        }
        self.schema_version = CONFIG_SCHEMA_VERSION;
        self
    }
}

#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            path: data_dir.into().join("config.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load_windows(&self) -> Result<WindowsRuntimeConfig, ConfigError> {
        self.load::<WindowsRuntimeConfig>()
            .map(WindowsRuntimeConfig::validate)
    }

    pub fn save_windows(&self, config: &WindowsRuntimeConfig) -> Result<(), ConfigError> {
        self.save(&config.clone().validate())
    }

    pub fn reconcile_windows_identity(
        &self,
        config: &mut WindowsRuntimeConfig,
        local_device_id: &str,
    ) -> Result<bool, ConfigError> {
        if config.local_device_id.as_deref() == Some(local_device_id) {
            return Ok(false);
        }
        let trust_was_reset = config.trusted_peer.take().is_some();
        config.local_device_id = Some(local_device_id.to_owned());
        self.save_windows(config)?;
        Ok(trust_was_reset)
    }

    pub fn load_macos(&self) -> Result<MacRuntimeConfig, ConfigError> {
        self.load::<MacRuntimeConfig>()
            .map(MacRuntimeConfig::validate)
    }

    pub fn save_macos(&self, config: &MacRuntimeConfig) -> Result<(), ConfigError> {
        self.save(&config.clone().validate())
    }

    pub fn reconcile_macos_identity(
        &self,
        config: &mut MacRuntimeConfig,
        local_device_id: &str,
    ) -> Result<bool, ConfigError> {
        if config.local_device_id.as_deref() == Some(local_device_id) {
            return Ok(false);
        }
        let trust_was_reset = config.trusted_peer.take().is_some();
        config.local_device_id = Some(local_device_id.to_owned());
        self.save_macos(config)?;
        Ok(trust_was_reset)
    }

    fn load<T>(&self) -> Result<T, ConfigError>
    where
        T: DeserializeOwned + Default,
    {
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(T::default()),
            Err(error) => return Err(error.into()),
        };
        let mut contents = Vec::new();
        file.read_to_end(&mut contents)?;
        serde_json::from_slice(&contents).map_err(ConfigError::Deserialize)
    }

    fn save<T>(&self, config: &T) -> Result<(), ConfigError>
    where
        T: Serialize,
    {
        let parent = self.path.parent().ok_or(ConfigError::MissingParent)?;
        fs::create_dir_all(parent)?;
        let serialized = serde_json::to_vec_pretty(config)?;
        let mut temporary = Builder::new()
            .prefix("config-")
            .suffix(".json.tmp")
            .tempfile_in(parent)?;
        temporary.write_all(&serialized)?;
        temporary.flush()?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&self.path)
            .map_err(|error| ConfigError::Io(error.error))?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("configuration I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("configuration serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("configuration deserialization failed: {0}")]
    Deserialize(serde_json::Error),
    #[error("configuration path has no parent directory")]
    MissingParent,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_config_uses_safe_role_defaults() {
        let directory = tempfile::tempdir().expect("create temp directory");
        let store = ConfigStore::new(directory.path());

        assert_eq!(
            store.load_windows().expect("load Windows defaults"),
            WindowsRuntimeConfig::default()
        );
        assert_eq!(
            store.load_macos().expect("load Mac defaults"),
            MacRuntimeConfig::default()
        );
    }

    #[test]
    fn configs_persist_and_reload() {
        let directory = tempfile::tempdir().expect("create temp directory");
        let store = ConfigStore::new(directory.path());
        let windows = WindowsRuntimeConfig {
            endpoint: Some(NetworkEndpoint {
                ip: "192.168.1.9".into(),
                port: 49_000,
            }),
            edge: ScreenEdge::Left,
            ..WindowsRuntimeConfig::default()
        };
        let mac = MacRuntimeConfig {
            listen_port: 49_000,
            local_policy: LocalControlPolicy::Paused,
            ..MacRuntimeConfig::default()
        };

        store.save_windows(&windows).expect("save Windows config");
        assert_eq!(store.load_windows().expect("load Windows config"), windows);
        store.save_macos(&mac).expect("save Mac config");
        assert_eq!(store.load_macos().expect("load Mac config"), mac);
    }

    #[test]
    fn changed_windows_identity_clears_and_persists_stale_peer_trust() {
        let directory = tempfile::tempdir().expect("create temp directory");
        let store = ConfigStore::new(directory.path());
        let peer_identity =
            crate::DeviceIdentity::generate(kvmflow_domain::EndpointRole::MacEndpoint, "Old Mac");
        let mut config = WindowsRuntimeConfig {
            trusted_peer: Some(crate::TrustedPeer::from_identity(&peer_identity)),
            ..WindowsRuntimeConfig::default()
        };

        assert!(
            store
                .reconcile_windows_identity(&mut config, "new-windows-id")
                .expect("reset Windows trust")
        );
        assert_eq!(config.local_device_id.as_deref(), Some("new-windows-id"));
        assert!(config.trusted_peer.is_none());
        let persisted = store.load_windows().expect("reload Windows config");
        assert_eq!(persisted.local_device_id.as_deref(), Some("new-windows-id"));
        assert!(persisted.trusted_peer.is_none());
    }

    #[test]
    fn existing_macos_identity_preserves_peer_trust() {
        let directory = tempfile::tempdir().expect("create temp directory");
        let store = ConfigStore::new(directory.path());
        let peer_identity = crate::DeviceIdentity::generate(
            kvmflow_domain::EndpointRole::WindowsController,
            "Windows",
        );
        let mut config = MacRuntimeConfig {
            local_device_id: Some("current-mac-id".into()),
            trusted_peer: Some(crate::TrustedPeer::from_identity(&peer_identity)),
            ..MacRuntimeConfig::default()
        };

        assert!(
            !store
                .reconcile_macos_identity(&mut config, "current-mac-id")
                .expect("preserve Mac trust")
        );
        assert!(config.trusted_peer.is_some());
        assert!(!store.path().exists());
    }

    #[test]
    fn invalid_values_fall_back_without_resetting_other_fields() {
        let directory = tempfile::tempdir().expect("create temp directory");
        let store = ConfigStore::new(directory.path());
        fs::write(
            store.path(),
            r#"{"schemaVersion":99,"listenPort":80,"localPolicy":"paused","trustedPeer":null}"#,
        )
        .expect("write invalid fixture");

        let config = store.load_macos().expect("load repaired config");

        assert_eq!(config.listen_port, DEFAULT_MAC_LISTEN_PORT);
        assert_eq!(config.local_policy, LocalControlPolicy::Paused);
        assert_eq!(config.schema_version, CONFIG_SCHEMA_VERSION);
    }

    #[test]
    fn legacy_fixed_right_edge_migrates_to_the_current_bottom_layout() {
        let directory = tempfile::tempdir().expect("create temp directory");
        let store = ConfigStore::new(directory.path());
        fs::write(
            store.path(),
            r#"{"schemaVersion":1,"endpoint":null,"edge":"right","trustedPeer":null}"#,
        )
        .expect("write legacy Windows config");

        let config = store.load_windows().expect("load migrated Windows config");

        assert_eq!(config.schema_version, CONFIG_SCHEMA_VERSION);
        assert_eq!(config.edge, ScreenEdge::Bottom);
    }
}

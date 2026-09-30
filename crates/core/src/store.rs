//! Per-device persistent data: the static identity key, paired peers, the
//! vault registry and the sync state. Lives in the app's private data
//! directory on the phone and in `~/.config/scarlet-notes` on the desktop.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::vault::{validate_vault_name, write_atomic};
use crate::{err, from_hex, to_hex, Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub device_name: String,
    pub private_key: String,
    pub public_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Peer {
    pub name: String,
    pub public_key: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Config {
    #[serde(default)]
    peers: Vec<Peer>,
    /// Vault name -> directory. Only used on the desktop; the phone keeps
    /// all its vaults under one directory.
    #[serde(default)]
    vaults: BTreeMap<String, PathBuf>,
    #[serde(default)]
    last_addr: String,
}

/// Path -> content signature ("dir" or the hex SHA-256 of a note) as of the
/// last successful sync with one peer.
pub type BaseState = BTreeMap<String, String>;

#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Store> {
        let dir = dir.into();
        fs::create_dir_all(dir.join("state"))?;
        Ok(Store { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn identity_path(&self) -> PathBuf {
        self.dir.join("identity.json")
    }

    pub fn identity(&self) -> Result<Identity> {
        let path = self.identity_path();
        if !path.exists() {
            return err("this device has no keys yet");
        }
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    /// Returns the device identity, generating the static key pair on first use.
    pub fn identity_or_create(&self, default_name: &str) -> Result<Identity> {
        if let Ok(identity) = self.identity() {
            return Ok(identity);
        }
        let keypair = crate::channel::generate_keypair()?;
        let identity = Identity {
            device_name: default_name.to_string(),
            private_key: to_hex(&keypair.0),
            public_key: to_hex(&keypair.1),
        };
        self.save_identity(&identity)?;
        Ok(identity)
    }

    fn save_identity(&self, identity: &Identity) -> Result<()> {
        let path = self.identity_path();
        write_atomic(&path, &serde_json::to_vec_pretty(identity)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    pub fn set_device_name(&self, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() || name.len() > 64 {
            return err("device name must be 1-64 characters long");
        }
        let mut identity = self.identity()?;
        identity.device_name = name.to_string();
        self.save_identity(&identity)
    }

    fn config(&self) -> Result<Config> {
        let path = self.dir.join("config.json");
        if !path.exists() {
            return Ok(Config::default());
        }
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    fn save_config(&self, config: &Config) -> Result<()> {
        write_atomic(&self.dir.join("config.json"), &serde_json::to_vec_pretty(config)?)
    }

    pub fn peers(&self) -> Result<Vec<Peer>> {
        Ok(self.config()?.peers)
    }

    pub fn find_peer(&self, public_key: &[u8]) -> Result<Option<Peer>> {
        let key = to_hex(public_key);
        Ok(self.config()?.peers.into_iter().find(|p| p.public_key == key))
    }

    /// Adds a paired peer, replacing any earlier entry with the same key.
    pub fn add_peer(&self, name: &str, public_key: &[u8]) -> Result<()> {
        let key = to_hex(public_key);
        let mut config = self.config()?;
        config.peers.retain(|p| p.public_key != key);
        config.peers.push(Peer { name: name.to_string(), public_key: key });
        self.save_config(&config)
    }

    /// Forgets a peer by name or key prefix. Returns whether anything was removed.
    pub fn remove_peer(&self, name_or_key: &str) -> Result<bool> {
        let mut config = self.config()?;
        let before = config.peers.len();
        config.peers.retain(|p| p.name != name_or_key && !p.public_key.starts_with(name_or_key));
        let removed = config.peers.len() != before;
        self.save_config(&config)?;
        Ok(removed)
    }

    pub fn vaults(&self) -> Result<BTreeMap<String, PathBuf>> {
        Ok(self.config()?.vaults)
    }

    pub fn add_vault(&self, name: &str, path: &Path) -> Result<()> {
        validate_vault_name(name)?;
        if !path.is_dir() {
            return err(format!("{} is not a directory", path.display()));
        }
        let mut config = self.config()?;
        config.vaults.insert(name.to_string(), path.canonicalize()?);
        self.save_config(&config)
    }

    pub fn remove_vault(&self, name: &str) -> Result<bool> {
        let mut config = self.config()?;
        let removed = config.vaults.remove(name).is_some();
        self.save_config(&config)?;
        Ok(removed)
    }

    /// The address last used to reach a peer, to pre-fill the next sync.
    pub fn last_addr(&self) -> String {
        self.config().map(|c| c.last_addr).unwrap_or_default()
    }

    pub fn set_last_addr(&self, addr: &str) -> Result<()> {
        let mut config = self.config()?;
        config.last_addr = addr.to_string();
        self.save_config(&config)
    }

    fn state_path(&self, vault: &str, peer_key: &[u8]) -> Result<PathBuf> {
        validate_vault_name(vault)?;
        let key = to_hex(peer_key);
        Ok(self.dir.join("state").join(format!("{vault}--{}.json", &key[..key.len().min(16)])))
    }

    pub fn base_state(&self, vault: &str, peer_key: &[u8]) -> Result<BaseState> {
        let path = self.state_path(vault, peer_key)?;
        if !path.exists() {
            return Ok(BaseState::new());
        }
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    pub fn save_base_state(&self, vault: &str, peer_key: &[u8], state: &BaseState) -> Result<()> {
        write_atomic(&self.state_path(vault, peer_key)?, &serde_json::to_vec(state)?)
    }

    /// Drops the sync state of a vault (for all peers), e.g. when the vault is deleted.
    pub fn forget_vault_state(&self, vault: &str) -> Result<()> {
        let prefix = format!("{vault}--");
        for entry in fs::read_dir(self.dir.join("state"))? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with(&prefix) {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }
}

impl Identity {
    pub fn private_key_bytes(&self) -> Result<Vec<u8>> {
        from_hex(&self.private_key)
    }

    pub fn fingerprint(&self) -> String {
        fingerprint(&self.public_key)
    }
}

/// Short human-readable form of a public key: "ab12-cd34-ef56-7890".
pub fn fingerprint(public_key_hex: &str) -> String {
    let head: String = public_key_hex.chars().take(16).collect();
    head.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join("-")
}

impl From<std::string::FromUtf8Error> for Error {
    fn from(e: std::string::FromUtf8Error) -> Self {
        Error(e.to_string())
    }
}

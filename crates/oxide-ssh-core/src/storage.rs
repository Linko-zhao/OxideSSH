use std::{
    collections::HashSet,
    fs,
    io::{self, Write},
    net::IpAddr,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::fs::File;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tempfile::Builder;
use thiserror::Error;

use crate::model::{
    AppSettings, AuthConfig, ConnectionProfile, CredentialRef, Endpoint, ProfileId,
    ProfileValidationError,
};

pub const CONFIG_FILE_NAME: &str = "config.json";
pub const KNOWN_HOSTS_FILE_NAME: &str = "known_hosts.json";
const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    schema_version: u32,
    pub settings: AppSettings,
    pub profiles: Vec<ConnectionProfile>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            settings: AppSettings::default(),
            profiles: Vec::new(),
        }
    }
}

impl AppConfig {
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownHosts {
    schema_version: u32,
    pub hosts: Vec<KnownHostEntry>,
}

impl Default for KnownHosts {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            hosts: Vec::new(),
        }
    }
}

impl KnownHosts {
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownHostEntry {
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    pub public_key: String,
    pub fingerprint_sha256: String,
    pub accepted_at_unix: i64,
}

impl KnownHostEntry {
    pub fn endpoint(&self) -> Endpoint {
        Endpoint {
            host: self.host.clone(),
            port: self.port,
        }
    }
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("unsupported schema version {found} in {path}")]
    UnsupportedSchema { path: PathBuf, found: u64 },
    #[error("corrupt storage file at {path}")]
    Corrupt {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("unable to access storage file at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("unable to serialize storage file at {path}")]
    Serialize {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid host endpoint")]
    InvalidEndpoint,
    #[error("duplicate profile id {id:?}")]
    DuplicateProfile { id: ProfileId },
    #[error("invalid credential reference in profile {id:?}")]
    InvalidCredentialRef { id: ProfileId },
    #[error("host key changed while it was being accepted")]
    HostKeyConflict,
    #[error(transparent)]
    InvalidProfile(#[from] ProfileValidationError),
}

pub struct AppStore {
    root: PathBuf,
    config_lock: Mutex<()>,
    known_hosts_lock: Mutex<()>,
}

impl AppStore {
    pub fn open(root: PathBuf) -> Result<Self, StorageError> {
        fs::create_dir_all(&root).map_err(|source| StorageError::Io {
            path: root.clone(),
            source,
        })?;
        Ok(Self {
            root,
            config_lock: Mutex::new(()),
            known_hosts_lock: Mutex::new(()),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join(CONFIG_FILE_NAME)
    }

    pub fn known_hosts_path(&self) -> PathBuf {
        self.root.join(KNOWN_HOSTS_FILE_NAME)
    }

    pub fn load_config(&self) -> Result<AppConfig, StorageError> {
        let _guard = self.config_lock.lock();
        let config = read_versioned(&self.config_path(), AppConfig::default)?;
        validate_config(&config)?;
        Ok(config)
    }

    pub fn save_config(&self, config: &AppConfig) -> Result<(), StorageError> {
        let _guard = self.config_lock.lock();
        let path = self.config_path();
        guard_existing(&path, AppConfig::default)?;
        validate_config(config)?;
        atomic_write(&self.root, &path, config)
    }

    pub fn update_config(&self, update: impl FnOnce(&mut AppConfig)) -> Result<(), StorageError> {
        let _guard = self.config_lock.lock();
        let path = self.config_path();
        let mut config = read_versioned(&path, AppConfig::default)?;
        update(&mut config);
        validate_config(&config)?;
        atomic_write(&self.root, &path, &config)
    }

    pub fn load_known_hosts(&self) -> Result<KnownHosts, StorageError> {
        let _guard = self.known_hosts_lock.lock();
        read_versioned(&self.known_hosts_path(), KnownHosts::default)
    }

    pub fn known_host(&self, endpoint: &Endpoint) -> Result<Option<KnownHostEntry>, StorageError> {
        let endpoint = canonicalize_endpoint(endpoint)?;
        let hosts = self.load_known_hosts()?;
        Ok(hosts
            .hosts
            .into_iter()
            .find(|host| host.host == endpoint.host && host.port == endpoint.port))
    }

    pub fn accept_host_key(&self, mut entry: KnownHostEntry) -> Result<(), StorageError> {
        let _guard = self.known_hosts_lock.lock();
        let path = self.known_hosts_path();
        let mut hosts = read_versioned(&path, KnownHosts::default)?;
        let endpoint = canonicalize_endpoint(&entry.endpoint())?;
        entry.host = endpoint.host;
        entry.port = endpoint.port;
        if let Some(existing) = hosts
            .hosts
            .iter()
            .find(|host| host.host == entry.host && host.port == entry.port)
        {
            if existing.algorithm == entry.algorithm
                && existing.public_key == entry.public_key
                && existing.fingerprint_sha256 == entry.fingerprint_sha256
            {
                return Ok(());
            }
            return Err(StorageError::HostKeyConflict);
        }
        hosts.hosts.push(entry);
        atomic_write(&self.root, &path, &hosts)
    }

    pub fn delete_known_host(&self, endpoint: &Endpoint) -> Result<bool, StorageError> {
        let _guard = self.known_hosts_lock.lock();
        let path = self.known_hosts_path();
        let mut hosts = read_versioned(&path, KnownHosts::default)?;
        let endpoint = canonicalize_endpoint(endpoint)?;
        let original_len = hosts.hosts.len();
        hosts
            .hosts
            .retain(|host| host.host != endpoint.host || host.port != endpoint.port);
        if hosts.hosts.len() == original_len {
            return Ok(false);
        }
        atomic_write(&self.root, &path, &hosts)?;
        Ok(true)
    }
}

pub fn canonicalize_endpoint(endpoint: &Endpoint) -> Result<Endpoint, StorageError> {
    if endpoint.port == 0 {
        return Err(StorageError::InvalidEndpoint);
    }

    let unbracketed = endpoint
        .host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(&endpoint.host);
    let bytes = unbracketed.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 253
        || !bytes.is_ascii()
        || bytes
            .iter()
            .any(|byte| byte.is_ascii_whitespace() || matches!(byte, 0 | b'/' | b'\\'))
    {
        return Err(StorageError::InvalidEndpoint);
    }

    let host = match unbracketed.parse::<IpAddr>() {
        Ok(address) => address.to_string(),
        Err(_) => unbracketed.to_ascii_lowercase(),
    };
    Ok(Endpoint {
        host,
        port: endpoint.port,
    })
}

fn validate_config(config: &AppConfig) -> Result<(), StorageError> {
    if config.schema_version != SCHEMA_VERSION {
        return Err(StorageError::UnsupportedSchema {
            path: PathBuf::from(CONFIG_FILE_NAME),
            found: u64::from(config.schema_version),
        });
    }
    let mut seen = HashSet::new();
    for profile in &config.profiles {
        if !seen.insert(profile.id) {
            return Err(StorageError::DuplicateProfile { id: profile.id });
        }
        profile.validate()?;
        match &profile.auth {
            AuthConfig::Password { credential_ref } => {
                if let Some(reference) = credential_ref
                    && *reference != CredentialRef::password(profile.id)
                {
                    return Err(StorageError::InvalidCredentialRef { id: profile.id });
                }
            }
            AuthConfig::PrivateKey { passphrase_ref, .. } => {
                if let Some(reference) = passphrase_ref
                    && *reference != CredentialRef::private_key_passphrase(profile.id)
                {
                    return Err(StorageError::InvalidCredentialRef { id: profile.id });
                }
            }
            AuthConfig::Agent => {}
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SchemaProbe {
    schema_version: u64,
}

fn guard_existing<T>(path: &Path, default: impl FnOnce() -> T) -> Result<(), StorageError>
where
    T: DeserializeOwned,
{
    if path.try_exists().map_err(|source| StorageError::Io {
        path: path.to_path_buf(),
        source,
    })? {
        let _: T = read_versioned(path, default)?;
    }
    Ok(())
}

fn read_versioned<T>(path: &Path, default: impl FnOnce() -> T) -> Result<T, StorageError>
where
    T: DeserializeOwned,
{
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(default()),
        Err(source) => {
            return Err(StorageError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let probe: SchemaProbe =
        serde_json::from_slice(&bytes).map_err(|source| StorageError::Corrupt {
            path: path.to_path_buf(),
            source,
        })?;
    if probe.schema_version != u64::from(SCHEMA_VERSION) {
        return Err(StorageError::UnsupportedSchema {
            path: path.to_path_buf(),
            found: probe.schema_version,
        });
    }
    serde_json::from_slice(&bytes).map_err(|source| StorageError::Corrupt {
        path: path.to_path_buf(),
        source,
    })
}

fn atomic_write<T: Serialize>(root: &Path, path: &Path, value: &T) -> Result<(), StorageError> {
    let mut temporary = Builder::new()
        .prefix(".oxide-ssh-")
        .tempfile_in(root)
        .map_err(|source| StorageError::Io {
            path: root.to_path_buf(),
            source,
        })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|source| StorageError::Io {
                path: temporary.path().to_path_buf(),
                source,
            })?;
    }

    serde_json::to_writer_pretty(temporary.as_file_mut(), value).map_err(|source| {
        StorageError::Serialize {
            path: path.to_path_buf(),
            source,
        }
    })?;
    temporary
        .as_file_mut()
        .write_all(b"\n")
        .map_err(|source| StorageError::Io {
            path: temporary.path().to_path_buf(),
            source,
        })?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|source| StorageError::Io {
            path: temporary.path().to_path_buf(),
            source,
        })?;
    temporary.persist(path).map_err(|error| StorageError::Io {
        path: path.to_path_buf(),
        source: error.error,
    })?;

    #[cfg(unix)]
    File::open(root)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| StorageError::Io {
            path: root.to_path_buf(),
            source,
        })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, sync::Arc, thread};

    use tempfile::tempdir;

    use super::*;
    use crate::model::{
        AuthConfig, ConnectionProfile, CredentialRef, LocaleSetting, ProfileId, ThemeSetting,
    };

    fn profile(name: &str) -> ConnectionProfile {
        let id = ProfileId::new();
        ConnectionProfile {
            id,
            name: name.into(),
            endpoint: Endpoint {
                host: "EXAMPLE.COM".into(),
                port: 22,
            },
            username: "oxide".into(),
            auth: AuthConfig::Password {
                credential_ref: Some(CredentialRef::password(id)),
            },
        }
    }

    fn host(host: &str, port: u16) -> KnownHostEntry {
        KnownHostEntry {
            host: host.into(),
            port,
            algorithm: "ssh-ed25519".into(),
            public_key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITest".into(),
            fingerprint_sha256: "SHA256:test".into(),
            accepted_at_unix: 1,
        }
    }

    #[test]
    fn config_round_trip_uses_atomic_replacement() {
        let directory = tempdir().unwrap();
        let store = AppStore::open(directory.path().to_path_buf()).unwrap();
        let mut config = AppConfig::default();
        config.settings.locale = LocaleSetting::ZhCn;
        config.settings.theme = ThemeSetting::Dark;
        config.profiles.push(profile("first"));

        store.save_config(&config).unwrap();
        config.profiles.push(profile("second"));
        store.save_config(&config).unwrap();

        assert_eq!(store.load_config().unwrap(), config);
        let names: Vec<_> = fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["config.json"]);
    }

    #[test]
    fn unsupported_schema_is_rejected() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join(CONFIG_FILE_NAME),
            r#"{"schemaVersion":2,"settings":{},"profiles":[]}"#,
        )
        .unwrap();
        let store = AppStore::open(directory.path().to_path_buf()).unwrap();

        assert!(matches!(
            store.load_config(),
            Err(StorageError::UnsupportedSchema { found: 2, .. })
        ));
    }

    #[test]
    fn corrupt_config_is_not_overwritten() {
        let directory = tempdir().unwrap();
        let path = directory.path().join(CONFIG_FILE_NAME);
        let corrupt = b"{ definitely not json";
        fs::write(&path, corrupt).unwrap();
        let store = AppStore::open(directory.path().to_path_buf()).unwrap();

        assert!(matches!(
            store.update_config(|config| config.profiles.push(profile("new"))),
            Err(StorageError::Corrupt { .. })
        ));
        assert_eq!(fs::read(path).unwrap(), corrupt);
    }

    #[test]
    fn host_endpoint_is_canonicalized() {
        let directory = tempdir().unwrap();
        let store = AppStore::open(directory.path().to_path_buf()).unwrap();

        store.accept_host_key(host("EXAMPLE.COM", 22)).unwrap();
        store
            .accept_host_key(host("[2001:0DB8:0:0:0:0:0:1]", 2200))
            .unwrap();

        let hosts = store.load_known_hosts().unwrap().hosts;
        assert_eq!(hosts[0].host, "example.com");
        assert_eq!(hosts[1].host, "2001:db8::1");
        assert!(
            store
                .known_host(&Endpoint {
                    host: "Example.Com".into(),
                    port: 22,
                })
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn replacing_a_host_key_requires_deletion_first() {
        let directory = tempdir().unwrap();
        let store = AppStore::open(directory.path().to_path_buf()).unwrap();
        store.accept_host_key(host("example.com", 22)).unwrap();

        let mut replacement = host("EXAMPLE.COM", 22);
        replacement.fingerprint_sha256 = "SHA256:replacement".into();
        assert!(matches!(
            store.accept_host_key(replacement),
            Err(StorageError::HostKeyConflict)
        ));

        store
            .delete_known_host(&Endpoint {
                host: "example.com".into(),
                port: 22,
            })
            .unwrap();
        let mut fresh = host("example.com", 22);
        fresh.fingerprint_sha256 = "SHA256:replacement".into();
        store.accept_host_key(fresh).unwrap();

        let hosts = store.load_known_hosts().unwrap().hosts;
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].fingerprint_sha256, "SHA256:replacement");
    }

    #[test]
    fn reaccepting_the_same_key_is_idempotent() {
        let directory = tempdir().unwrap();
        let store = AppStore::open(directory.path().to_path_buf()).unwrap();

        store.accept_host_key(host("example.com", 22)).unwrap();
        store.accept_host_key(host("EXAMPLE.COM", 22)).unwrap();

        let hosts = store.load_known_hosts().unwrap().hosts;
        assert_eq!(hosts.len(), 1);
    }

    #[test]
    fn config_with_foreign_credential_reference_is_rejected_on_load() {
        let directory = tempdir().unwrap();
        let tampered = r#"{"schemaVersion":1,"settings":{"locale":"system","theme":"system"},"profiles":[{"id":"00000000-0000-0000-0000-000000000000","name":"borrowed","endpoint":{"host":"example.com","port":22},"username":"oxide","auth":{"method":"password","credentialRef":"profile/11111111-1111-1111-1111-111111111111/password"}}]}"#;
        fs::write(directory.path().join(CONFIG_FILE_NAME), tampered).unwrap();
        let store = AppStore::open(directory.path().to_path_buf()).unwrap();

        assert!(matches!(
            store.load_config(),
            Err(StorageError::InvalidCredentialRef { .. })
        ));
    }

    #[test]
    fn config_with_duplicate_profile_ids_is_rejected_on_load() {
        let directory = tempdir().unwrap();
        let tampered = r#"{"schemaVersion":1,"settings":{"locale":"system","theme":"system"},"profiles":[{"id":"00000000-0000-0000-0000-000000000000","name":"first","endpoint":{"host":"example.com","port":22},"username":"oxide","auth":{"method":"agent"}},{"id":"00000000-0000-0000-0000-000000000000","name":"second","endpoint":{"host":"example.org","port":22},"username":"oxide","auth":{"method":"agent"}}]}"#;
        fs::write(directory.path().join(CONFIG_FILE_NAME), tampered).unwrap();
        let store = AppStore::open(directory.path().to_path_buf()).unwrap();

        assert!(matches!(
            store.load_config(),
            Err(StorageError::DuplicateProfile { .. })
        ));
    }

    #[test]
    fn concurrent_config_updates_do_not_lose_profiles() {
        let directory = tempdir().unwrap();
        let store = Arc::new(AppStore::open(directory.path().to_path_buf()).unwrap());
        let writers: Vec<_> = (0..16)
            .map(|index| {
                let store = store.clone();
                thread::spawn(move || {
                    store
                        .update_config(|config| {
                            config.profiles.push(profile(&format!("profile-{index}")))
                        })
                        .unwrap();
                })
            })
            .collect();

        for writer in writers {
            writer.join().unwrap();
        }

        assert_eq!(store.load_config().unwrap().profiles.len(), 16);
    }

    #[test]
    fn storage_never_serializes_secrets() {
        let directory = tempdir().unwrap();
        let store = AppStore::open(directory.path().to_path_buf()).unwrap();
        let input_password = "test-only-password-never-persist";
        let mut config = AppConfig::default();
        config.profiles.push(profile("secret check"));

        store.save_config(&config).unwrap();

        let serialized = fs::read_to_string(directory.path().join(CONFIG_FILE_NAME)).unwrap();
        assert!(!serialized.contains(input_password));
        assert!(serialized.contains("credentialRef"));
        assert!(!serialized.contains("password-never-persist"));
    }

    #[test]
    fn empty_store_uses_schema_one_defaults_without_writing() {
        let directory = tempdir().unwrap();
        let store = AppStore::open(directory.path().to_path_buf()).unwrap();

        assert_eq!(store.load_config().unwrap(), AppConfig::default());
        assert_eq!(store.load_known_hosts().unwrap(), KnownHosts::default());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn private_key_paths_round_trip_without_interpretation() {
        let directory = tempdir().unwrap();
        let store = AppStore::open(directory.path().to_path_buf()).unwrap();
        let mut config = AppConfig::default();
        let mut key_profile = profile("key");
        key_profile.auth = AuthConfig::PrivateKey {
            path: PathBuf::from("keys/id_ed25519"),
            passphrase_ref: None,
        };
        config.profiles.push(key_profile);

        store.save_config(&config).unwrap();

        assert_eq!(store.load_config().unwrap(), config);
    }
}

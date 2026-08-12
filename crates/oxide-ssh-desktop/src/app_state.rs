use std::{path::PathBuf, sync::Arc};

use oxide_ssh_core::{
    model::{
        AppSettings, AuthConfig, ConnectionProfile, Endpoint, LocaleSetting, ProfileId,
        ProfileValidationError, ThemeSetting,
    },
    storage::{AppConfig, AppStore, KnownHostEntry, KnownHosts, StorageError},
};
use oxide_ssh_terminal::{RgbColor, TerminalColors};
use secrecy::SecretString;

use crate::{credentials::SaveProfileRequest, i18n::ResolvedLocale};

pub enum AppLoadOutcome {
    Ready(AppState),
    Recovery(RecoveryState),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryState {
    pub root: PathBuf,
}

pub struct AppState {
    store: Arc<AppStore>,
    config: AppConfig,
    known_hosts: KnownHosts,
    search_query: String,
}

impl AppState {
    pub fn load(root: PathBuf) -> AppLoadOutcome {
        let store = match AppStore::open(root.clone()) {
            Ok(store) => Arc::new(store),
            Err(_) => return AppLoadOutcome::Recovery(RecoveryState { root }),
        };
        let config = match store.load_config() {
            Ok(config) => config,
            Err(_) => return AppLoadOutcome::Recovery(RecoveryState { root }),
        };
        let known_hosts = match store.load_known_hosts() {
            Ok(hosts) => hosts,
            Err(_) => return AppLoadOutcome::Recovery(RecoveryState { root }),
        };

        AppLoadOutcome::Ready(Self {
            store,
            config,
            known_hosts,
            search_query: String::new(),
        })
    }

    pub fn store(&self) -> &Arc<AppStore> {
        &self.store
    }

    pub fn settings(&self) -> &AppSettings {
        &self.config.settings
    }

    pub fn profiles(&self) -> &[ConnectionProfile] {
        &self.config.profiles
    }

    pub fn known_hosts(&self) -> &[KnownHostEntry] {
        &self.known_hosts.hosts
    }

    pub fn set_search_query(&mut self, query: impl Into<String>) {
        self.search_query = query.into();
    }

    pub fn filtered_profiles(&self) -> Vec<&ConnectionProfile> {
        let query = self.search_query.trim().to_lowercase();
        let mut profiles: Vec<_> = self
            .config
            .profiles
            .iter()
            .filter(|profile| {
                query.is_empty()
                    || profile.name.to_lowercase().contains(&query)
                    || profile.endpoint.host.to_lowercase().contains(&query)
                    || profile.username.to_lowercase().contains(&query)
            })
            .collect();
        profiles.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.id.0.cmp(&right.id.0))
        });
        profiles
    }

    pub fn set_locale(&mut self, locale: LocaleSetting) -> Result<(), StorageError> {
        let mut config = self.config.clone();
        config.settings.locale = locale;
        self.store.save_config(&config)?;
        self.config = config;
        Ok(())
    }

    pub fn set_theme(&mut self, theme: ThemeSetting) -> Result<(), StorageError> {
        let mut config = self.config.clone();
        config.settings.theme = theme;
        self.store.save_config(&config)?;
        self.config = config;
        Ok(())
    }

    pub fn reload_config(&mut self) -> Result<(), StorageError> {
        self.config = self.store.load_config()?;
        Ok(())
    }

    pub fn reload_known_hosts(&mut self) -> Result<(), StorageError> {
        self.known_hosts = self.store.load_known_hosts()?;
        Ok(())
    }

    pub fn delete_known_host(&mut self, endpoint: &Endpoint) -> Result<bool, StorageError> {
        let deleted = self.store.delete_known_host(endpoint)?;
        if deleted {
            self.reload_known_hosts()?;
        }
        Ok(deleted)
    }

    pub fn resolved_locale(&self, system_locale: Option<&str>) -> ResolvedLocale {
        ResolvedLocale::resolve(self.config.settings.locale, system_locale)
    }

    pub fn resolved_theme(&self, system_is_dark: bool) -> ResolvedTheme {
        ResolvedTheme::resolve(self.config.settings.theme, system_is_dark)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolvedTheme {
    Light,
    Dark,
}

impl ResolvedTheme {
    pub fn resolve(setting: ThemeSetting, system_is_dark: bool) -> Self {
        match setting {
            ThemeSetting::Light => Self::Light,
            ThemeSetting::Dark => Self::Dark,
            ThemeSetting::System if system_is_dark => Self::Dark,
            ThemeSetting::System => Self::Light,
        }
    }

    pub fn tokens(self) -> ThemeTokens {
        match self {
            Self::Dark => ThemeTokens {
                surface: 0x111318,
                text: 0xe6e8eb,
                muted: 0x9aa0aa,
                border: 0x2a2e37,
                accent: 0xe58a3a,
                danger: 0xe06c75,
                selection: 0x315a78,
            },
            Self::Light => ThemeTokens {
                surface: 0xfafaf8,
                text: 0x202124,
                muted: 0x68707a,
                border: 0xd8dadd,
                accent: 0xc45d00,
                danger: 0xb4232c,
                selection: 0xb8d8f0,
            },
        }
    }

    pub fn terminal_colors(self) -> TerminalColors {
        let tokens = self.tokens();
        let ansi = match self {
            Self::Dark => [
                0x1b1d23, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
                0x5c6370, 0xff7b86, 0xb3e98c, 0xffd68a, 0x76c7ff, 0xd99bff, 0x70e1ed, 0xf5f7fa,
            ],
            Self::Light => [
                0x202124, 0xb4232c, 0x2f7d32, 0x8a5a00, 0x005fb8, 0x7a3e9d, 0x007a7a, 0xdadce0,
                0x5f6368, 0xd93025, 0x188038, 0xb06000, 0x1967d2, 0x9334e6, 0x008b8b, 0xffffff,
            ],
        };
        TerminalColors {
            foreground: rgb_color(tokens.text),
            background: rgb_color(tokens.surface),
            cursor: rgb_color(tokens.text),
            ansi: ansi.map(rgb_color),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ThemeTokens {
    pub surface: u32,
    pub text: u32,
    pub muted: u32,
    pub border: u32,
    pub accent: u32,
    pub danger: u32,
    pub selection: u32,
}

const fn rgb_color(hex: u32) -> RgbColor {
    RgbColor::new(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthMethod {
    Password,
    PrivateKey,
    Agent,
}

pub struct ConnectionForm {
    pub name: String,
    pub host: String,
    pub port: String,
    pub username: String,
    pub auth_method: AuthMethod,
    pub private_key_path: PathBuf,
    pub secret: String,
    pub remember: bool,
    previous: Option<ConnectionProfile>,
    profile_id: ProfileId,
}

impl ConnectionForm {
    pub fn new() -> Self {
        Self {
            name: String::new(),
            host: String::new(),
            port: "22".into(),
            username: String::new(),
            auth_method: AuthMethod::Password,
            private_key_path: PathBuf::new(),
            secret: String::new(),
            remember: false,
            previous: None,
            profile_id: ProfileId::new(),
        }
    }

    pub fn edit(profile: ConnectionProfile) -> Self {
        let (auth_method, private_key_path, remember) = match &profile.auth {
            AuthConfig::Password { credential_ref } => (
                AuthMethod::Password,
                PathBuf::new(),
                credential_ref.is_some(),
            ),
            AuthConfig::PrivateKey {
                path,
                passphrase_ref,
            } => (
                AuthMethod::PrivateKey,
                path.clone(),
                passphrase_ref.is_some(),
            ),
            AuthConfig::Agent => (AuthMethod::Agent, PathBuf::new(), false),
        };
        Self {
            name: profile.name.clone(),
            host: profile.endpoint.host.clone(),
            port: profile.endpoint.port.to_string(),
            username: profile.username.clone(),
            auth_method,
            private_key_path,
            secret: String::new(),
            remember,
            profile_id: profile.id,
            previous: Some(profile),
        }
    }

    pub fn is_editing(&self) -> bool {
        self.previous.is_some()
    }

    pub fn save_request(&self) -> Result<SaveProfileRequest, FormError> {
        let port = self
            .port
            .trim()
            .parse::<u16>()
            .ok()
            .filter(|port| *port != 0)
            .ok_or(FormError::Port)?;
        let auth =
            match self.auth_method {
                AuthMethod::Password => AuthConfig::Password {
                    credential_ref: self.previous.as_ref().and_then(|profile| {
                        match &profile.auth {
                            AuthConfig::Password { credential_ref } => credential_ref.clone(),
                            _ => None,
                        }
                    }),
                },
                AuthMethod::PrivateKey => AuthConfig::PrivateKey {
                    path: self.private_key_path.clone(),
                    passphrase_ref: self.previous.as_ref().and_then(|profile| {
                        match &profile.auth {
                            AuthConfig::PrivateKey { passphrase_ref, .. } => passphrase_ref.clone(),
                            _ => None,
                        }
                    }),
                },
                AuthMethod::Agent => AuthConfig::Agent,
            };
        let profile = ConnectionProfile {
            id: self.profile_id,
            name: self.name.trim().into(),
            endpoint: Endpoint {
                host: self.host.trim().into(),
                port,
            },
            username: self.username.trim().into(),
            auth,
        };
        profile.validate().map_err(FormError::from)?;

        Ok(SaveProfileRequest {
            profile,
            previous: self.previous.clone(),
            secret: (!self.secret.is_empty()).then(|| SecretString::from(self.secret.clone())),
            remember: self.remember && self.auth_method != AuthMethod::Agent,
        })
    }
}

impl Default for ConnectionForm {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FormError {
    Name,
    Host,
    Username,
    Port,
    PrivateKeyPath,
}

impl From<ProfileValidationError> for FormError {
    fn from(error: ProfileValidationError) -> Self {
        match error {
            ProfileValidationError::Name => Self::Name,
            ProfileValidationError::Host => Self::Host,
            ProfileValidationError::Username => Self::Username,
            ProfileValidationError::Port => Self::Port,
            ProfileValidationError::PrivateKeyPath => Self::PrivateKeyPath,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use oxide_ssh_core::{
        model::{AuthConfig, Endpoint, ProfileId},
        storage::CONFIG_FILE_NAME,
    };
    use tempfile::tempdir;

    use super::*;

    fn profile(name: &str, host: &str, username: &str) -> ConnectionProfile {
        ConnectionProfile {
            id: ProfileId::new(),
            name: name.into(),
            endpoint: Endpoint {
                host: host.into(),
                port: 22,
            },
            username: username.into(),
            auth: AuthConfig::Agent,
        }
    }

    #[test]
    fn corrupt_storage_opens_recovery_without_overwriting() {
        let directory = tempdir().unwrap();
        let path = directory.path().join(CONFIG_FILE_NAME);
        fs::write(&path, b"not json").unwrap();

        let outcome = AppState::load(directory.path().to_path_buf());
        assert!(matches!(outcome, AppLoadOutcome::Recovery(_)));
        assert_eq!(fs::read(path).unwrap(), b"not json");
    }

    #[test]
    fn search_is_trimmed_unicode_lowercase_and_sorted() {
        let directory = tempdir().unwrap();
        let store = AppStore::open(directory.path().to_path_buf()).unwrap();
        let mut config = store.load_config().unwrap();
        config.profiles = vec![
            profile("Zulu", "z.example", "root"),
            profile("北京", "beijing.example", "开发者"),
            profile("Alpha", "a.example", "oxide"),
        ];
        store.save_config(&config).unwrap();

        let AppLoadOutcome::Ready(mut state) = AppState::load(directory.path().to_path_buf())
        else {
            panic!("expected ready state");
        };
        assert_eq!(
            state
                .filtered_profiles()
                .into_iter()
                .map(|profile| profile.name.as_str())
                .collect::<Vec<_>>(),
            ["Alpha", "Zulu", "北京"],
        );

        state.set_search_query("  开发  ");
        assert_eq!(state.filtered_profiles()[0].name, "北京");
        state.set_search_query("EXAMPLE");
        assert_eq!(state.filtered_profiles().len(), 3);
    }

    #[test]
    fn settings_persist_immediately_and_resolve_system_defaults() {
        let directory = tempdir().unwrap();
        let AppLoadOutcome::Ready(mut state) = AppState::load(directory.path().to_path_buf())
        else {
            panic!("expected ready state");
        };

        state.set_locale(LocaleSetting::ZhCn).unwrap();
        state.set_theme(ThemeSetting::Dark).unwrap();
        let stored = state.store().load_config().unwrap();
        assert_eq!(stored.settings.locale, LocaleSetting::ZhCn);
        assert_eq!(stored.settings.theme, ThemeSetting::Dark);
        assert_eq!(state.resolved_locale(Some("en-US")), ResolvedLocale::ZhCn);
        assert_eq!(state.resolved_theme(false), ResolvedTheme::Dark);
    }

    #[test]
    fn connection_form_is_shared_for_add_and_edit() {
        let mut add = ConnectionForm::new();
        add.name = " Fixture ".into();
        add.host = "EXAMPLE.COM".into();
        add.username = "oxide".into();
        add.auth_method = AuthMethod::Password;
        add.secret = "secret".into();
        add.remember = true;
        let request = add.save_request().unwrap();
        assert_eq!(request.profile.endpoint.port, 22);
        assert_eq!(request.profile.name, "Fixture");
        assert!(request.previous.is_none());
        assert!(request.remember);

        let id = request.profile.id;
        let edit = ConnectionForm::edit(request.profile);
        let edited = edit.save_request().unwrap();
        assert_eq!(edited.profile.id, id);
        assert!(edited.previous.is_some());
    }

    #[test]
    fn connection_form_reports_boundary_errors() {
        let mut form = ConnectionForm::new();
        form.name = "fixture".into();
        form.host = "host".into();
        form.username = "oxide".into();
        form.port = "0".into();
        assert_eq!(form.save_request().unwrap_err(), FormError::Port);

        form.port = "22".into();
        form.auth_method = AuthMethod::PrivateKey;
        assert_eq!(form.save_request().unwrap_err(), FormError::PrivateKeyPath);
    }
}

//! Loaded application state: config store, profiles, known hosts, search.

use std::{path::PathBuf, sync::Arc};

use oxide_ssh_core::{
    model::{AppSettings, ConnectionProfile, Endpoint, LocaleSetting, ThemeSetting},
    storage::{AppConfig, AppStore, KnownHostEntry, KnownHosts, StorageError},
};

use crate::i18n::ResolvedLocale;
use crate::state::theme::ResolvedTheme;

pub mod connection_form;
pub mod theme;

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

#[cfg(test)]
mod tests {
    use std::fs;

    use oxide_ssh_core::{
        model::{AuthConfig, ProfileId},
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
}

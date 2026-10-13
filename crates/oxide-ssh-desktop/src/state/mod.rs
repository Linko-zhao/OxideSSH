//! Loaded application state: config store, profiles, known hosts, search.

use std::{collections::HashSet, path::PathBuf, sync::Arc};

use oxide_ssh_core::{
    model::{
        AppSettings, ConnectionProfile, Endpoint, LocaleSetting, Tag, TagColor, TagId, ThemeSetting,
    },
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
    tag_filter: TagFilter,
}

/// Sidebar tag selection. Empty selection with `untagged == false` shows all
/// profiles; otherwise a profile matches if any of its tags is selected, or
/// it has no tags while `untagged` is selected.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TagFilter {
    pub tags: HashSet<TagId>,
    pub untagged: bool,
}

impl TagFilter {
    pub fn is_active(&self) -> bool {
        self.untagged || !self.tags.is_empty()
    }

    fn matches(&self, profile: &ConnectionProfile) -> bool {
        if !self.is_active() {
            return true;
        }
        (self.untagged && profile.tag_ids.is_empty())
            || profile.tag_ids.iter().any(|id| self.tags.contains(id))
    }
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
            tag_filter: TagFilter::default(),
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
                self.tag_filter.matches(profile)
                    && (query.is_empty()
                        || profile.name.to_lowercase().contains(&query)
                        || profile.endpoint.host.to_lowercase().contains(&query)
                        || profile.username.to_lowercase().contains(&query)
                        || profile.tag_ids.iter().any(|id| {
                            self.config
                                .tags
                                .iter()
                                .find(|tag| tag.id == *id)
                                .is_some_and(|tag| tag.name.to_lowercase().contains(&query))
                        }))
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

    /// Registry entries sorted by case-insensitive name for display.
    pub fn tags(&self) -> Vec<&Tag> {
        let mut tags: Vec<_> = self.config.tags.iter().collect();
        tags.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.id.0.cmp(&right.id.0))
        });
        tags
    }

    pub fn tag(&self, id: TagId) -> Option<&Tag> {
        self.config.tags.iter().find(|tag| tag.id == id)
    }

    pub fn tag_filter(&self) -> &TagFilter {
        &self.tag_filter
    }

    pub fn toggle_tag_filter(&mut self, id: TagId) {
        if !self.tag_filter.tags.remove(&id) {
            self.tag_filter.tags.insert(id);
        }
    }

    pub fn toggle_untagged_filter(&mut self) {
        self.tag_filter.untagged = !self.tag_filter.untagged;
    }

    pub fn clear_tag_filter(&mut self) {
        self.tag_filter = TagFilter::default();
    }

    /// Profiles currently referencing the tag; used for delete confirmation.
    pub fn profiles_with_tag(&self, id: TagId) -> Vec<&ConnectionProfile> {
        self.config
            .profiles
            .iter()
            .filter(|profile| profile.tag_ids.contains(&id))
            .collect()
    }

    pub fn create_tag(
        &mut self,
        name: impl Into<String>,
        color: Option<TagColor>,
    ) -> Result<Tag, StorageError> {
        let mut tag = Tag::new(name.into().trim().to_string());
        tag.color = color;
        let mut config = self.config.clone();
        config.tags.push(tag.clone());
        self.store.save_config(&config)?;
        self.config = config;
        Ok(tag)
    }

    pub fn update_tag(
        &mut self,
        id: TagId,
        name: impl Into<String>,
        color: Option<TagColor>,
    ) -> Result<(), StorageError> {
        let mut config = self.config.clone();
        let Some(tag) = config.tags.iter_mut().find(|tag| tag.id == id) else {
            return Ok(());
        };
        tag.name = name.into().trim().to_string();
        tag.color = color;
        self.store.save_config(&config)?;
        self.config = config;
        Ok(())
    }

    /// Deletes the registry entry and removes the reference from every profile.
    pub fn delete_tag(&mut self, id: TagId) -> Result<(), StorageError> {
        let mut config = self.config.clone();
        config.tags.retain(|tag| tag.id != id);
        for profile in &mut config.profiles {
            profile.tag_ids.retain(|tag_id| *tag_id != id);
        }
        self.store.save_config(&config)?;
        self.config = config;
        self.tag_filter.tags.remove(&id);
        Ok(())
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
        model::{AuthConfig, ProfileId, Tag, TagColor},
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
            tag_ids: Vec::new(),
        }
    }

    #[test]
    fn tag_filter_is_union_and_intersects_with_search() {
        let directory = tempdir().unwrap();
        let store = AppStore::open(directory.path().to_path_buf()).unwrap();
        let mut config = store.load_config().unwrap();
        let prod = Tag::new("Prod");
        let db = Tag::new("Database");
        let mut web = profile("Web", "web.example", "oxide");
        web.tag_ids = vec![prod.id];
        let mut pg = profile("Postgres", "db.example", "oxide");
        pg.tag_ids = vec![prod.id, db.id];
        let plain = profile("Plain", "plain.example", "oxide");
        config.tags = vec![prod.clone(), db.clone()];
        config.profiles = vec![web, pg, plain];
        store.save_config(&config).unwrap();

        let AppLoadOutcome::Ready(mut state) = AppState::load(directory.path().to_path_buf())
        else {
            panic!("expected ready state");
        };
        assert_eq!(state.filtered_profiles().len(), 3);

        state.toggle_tag_filter(prod.id);
        assert_eq!(
            state
                .filtered_profiles()
                .iter()
                .map(|profile| profile.name.as_str())
                .collect::<Vec<_>>(),
            ["Postgres", "Web"]
        );

        // Union: adding Database keeps both, no new matches beyond Postgres.
        state.toggle_tag_filter(db.id);
        assert_eq!(state.filtered_profiles().len(), 2);

        // Untagged selection adds Plain (union).
        state.toggle_untagged_filter();
        assert_eq!(state.filtered_profiles().len(), 3);

        // Untagged alone hides tagged profiles.
        state.clear_tag_filter();
        state.toggle_untagged_filter();
        assert_eq!(state.filtered_profiles()[0].name, "Plain");

        // Search intersects with the tag filter and matches tag names.
        state.clear_tag_filter();
        state.set_search_query("database");
        assert_eq!(state.filtered_profiles()[0].name, "Postgres");
        state.set_search_query("web");
        state.toggle_tag_filter(db.id);
        assert!(state.filtered_profiles().is_empty());
    }

    #[test]
    fn tag_crud_cascades_delete_and_rejects_duplicate_names() {
        let directory = tempdir().unwrap();
        let store = AppStore::open(directory.path().to_path_buf()).unwrap();
        let mut config = store.load_config().unwrap();
        config.profiles = vec![profile("Web", "web.example", "oxide")];
        store.save_config(&config).unwrap();

        let AppLoadOutcome::Ready(mut state) = AppState::load(directory.path().to_path_buf())
        else {
            panic!("expected ready state");
        };
        let tag = state.create_tag(" Prod ", Some(TagColor::Green)).unwrap();
        assert_eq!(tag.name, "Prod");
        assert!(state.create_tag("prod", None).is_err());

        let profile_id = state.profiles()[0].id;
        state
            .store()
            .update_config(|config| config.profiles[0].tag_ids = vec![tag.id])
            .unwrap();
        state.reload_config().unwrap();
        assert_eq!(state.profiles_with_tag(tag.id).len(), 1);

        state.update_tag(tag.id, "Production", None).unwrap();
        assert_eq!(state.tag(tag.id).unwrap().name, "Production");
        assert_eq!(state.tag(tag.id).unwrap().color, None);

        state.toggle_tag_filter(tag.id);
        state.delete_tag(tag.id).unwrap();
        assert!(state.tags().is_empty());
        assert!(!state.tag_filter().is_active());
        let reloaded = store.load_config().unwrap();
        assert!(reloaded.tags.is_empty());
        assert_eq!(reloaded.profiles[0].id, profile_id);
        assert!(reloaded.profiles[0].tag_ids.is_empty());
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

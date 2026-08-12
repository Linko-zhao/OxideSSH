use std::sync::Arc;

use keyring::v1::{Entry, Error as KeyringError};
use oxide_ssh_core::{
    credentials::{CredentialError, CredentialStore},
    model::{AuthConfig, ConnectionProfile, CredentialRef, ProfileId, ProfileValidationError},
    storage::{AppStore, StorageError},
};
use secrecy::{ExposeSecret, SecretString};

use crate::i18n::MessageId;

pub const APPLICATION_IDENTIFIER: &str = "io.github.linko-zhao.OxideSSH";

#[derive(Default)]
pub struct SystemCredentialStore;

impl SystemCredentialStore {
    pub fn new() -> Self {
        Self
    }

    fn entry(reference: &CredentialRef) -> Result<Entry, CredentialError> {
        if reference.as_str().is_empty() {
            return Err(CredentialError::InvalidReference);
        }
        Entry::new(APPLICATION_IDENTIFIER, reference.as_str()).map_err(map_keyring_error)
    }
}

impl CredentialStore for SystemCredentialStore {
    fn get(&self, reference: &CredentialRef) -> Result<Option<SecretString>, CredentialError> {
        match Self::entry(reference)?.get_password() {
            Ok(password) => Ok(Some(SecretString::from(password))),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(error) => Err(map_keyring_error(error)),
        }
    }

    fn put(&self, reference: &CredentialRef, secret: &SecretString) -> Result<(), CredentialError> {
        Self::entry(reference)?
            .set_password(secret.expose_secret())
            .map_err(map_keyring_error)
    }

    fn delete(&self, reference: &CredentialRef) -> Result<(), CredentialError> {
        match Self::entry(reference)?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(error) => Err(map_keyring_error(error)),
        }
    }
}

fn map_keyring_error(error: KeyringError) -> CredentialError {
    match error {
        KeyringError::NoStorageAccess(_) => CredentialError::AccessDenied,
        KeyringError::TooLong(_, _) | KeyringError::Invalid(_, _) => {
            CredentialError::InvalidReference
        }
        KeyringError::BadEncoding(_)
        | KeyringError::BadDataFormat(_, _)
        | KeyringError::BadStoreFormat(_) => CredentialError::Corrupt,
        KeyringError::Ambiguous(_) => CredentialError::Ambiguous,
        KeyringError::NotSupportedByStore(_) => CredentialError::Unsupported,
        KeyringError::PlatformFailure(_) | KeyringError::NoDefaultStore | KeyringError::NoEntry => {
            CredentialError::Unavailable
        }
        _ => CredentialError::Unavailable,
    }
}

pub fn credential_error_message_id(error: CredentialError) -> MessageId {
    match error {
        CredentialError::Unavailable => MessageId::CredentialUnavailable,
        CredentialError::AccessDenied => MessageId::CredentialAccessDenied,
        CredentialError::InvalidReference => MessageId::CredentialInvalidReference,
        CredentialError::Corrupt => MessageId::CredentialCorrupt,
        CredentialError::Ambiguous => MessageId::CredentialAmbiguous,
        CredentialError::Unsupported => MessageId::CredentialUnsupported,
    }
}

pub struct SaveProfileRequest {
    pub profile: ConnectionProfile,
    pub previous: Option<ConnectionProfile>,
    pub secret: Option<SecretString>,
    pub remember: bool,
}

impl std::fmt::Debug for SaveProfileRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SaveProfileRequest")
            .field("profile", &self.profile)
            .field("previous", &self.previous)
            .field("has_secret", &self.secret.is_some())
            .field("remember", &self.remember)
            .finish()
    }
}

pub struct SaveProfileOutcome {
    pub profile: ConnectionProfile,
    pub connect_secret: Option<SecretString>,
}

#[derive(Debug, thiserror::Error)]
pub enum CredentialTransactionError {
    #[error(transparent)]
    Credential(#[from] CredentialError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    InvalidProfile(#[from] ProfileValidationError),
    #[error("a secret is required before this credential can be remembered")]
    MissingSecret,
    #[error("the previous profile does not match the edited profile")]
    ProfileMismatch,
    #[error("profile does not exist")]
    ProfileNotFound,
    #[error("credential rollback failed")]
    RollbackFailed,
}

pub struct ProfileCredentialCoordinator<C: CredentialStore + ?Sized> {
    app_store: Arc<AppStore>,
    credential_store: Arc<C>,
}

impl<C: CredentialStore + ?Sized> ProfileCredentialCoordinator<C> {
    pub fn new(app_store: Arc<AppStore>, credential_store: Arc<C>) -> Self {
        Self {
            app_store,
            credential_store,
        }
    }

    pub fn save_profile(
        &self,
        mut request: SaveProfileRequest,
    ) -> Result<SaveProfileOutcome, CredentialTransactionError> {
        request.profile.validate()?;
        if request
            .previous
            .as_ref()
            .is_some_and(|previous| previous.id != request.profile.id)
        {
            return Err(CredentialTransactionError::ProfileMismatch);
        }

        let old_reference = request
            .previous
            .as_ref()
            .and_then(|profile| credential_reference(&profile.auth))
            .cloned();
        let desired_reference = request
            .remember
            .then(|| expected_reference(request.profile.id, &request.profile.auth))
            .flatten();
        let requires_secret = !matches!(request.profile.auth, AuthConfig::Agent);
        let connect_secret = (!request.remember && requires_secret)
            .then(|| request.secret.take())
            .flatten();

        let mut written_credential = None;
        if let Some(reference) = &desired_reference {
            if let Some(secret) = request.secret.take() {
                let previous_value = self.credential_store.get(reference)?;
                self.credential_store.put(reference, &secret)?;
                written_credential = Some((reference.clone(), previous_value));
            } else {
                let retains_existing_reference = credential_reference(&request.profile.auth)
                    .or_else(|| {
                        request
                            .previous
                            .as_ref()
                            .and_then(|profile| credential_reference(&profile.auth))
                    })
                    .is_some_and(|existing| existing == reference);
                if !retains_existing_reference || self.credential_store.get(reference)?.is_none() {
                    return Err(CredentialTransactionError::MissingSecret);
                }
            }
        }
        set_credential_reference(&mut request.profile.auth, desired_reference.clone());

        let reference_to_remove =
            old_reference.filter(|old| desired_reference.as_ref() != Some(old));
        let mut deleted_credential = None;
        if let Some(reference) = &reference_to_remove {
            let previous_value = self.credential_store.get(reference)?;
            if let Err(error) = self.credential_store.delete(reference) {
                if !self.rollback_written(written_credential) {
                    return Err(CredentialTransactionError::RollbackFailed);
                }
                return Err(error.into());
            }
            deleted_credential = Some((reference.clone(), previous_value));
        }

        let saved_profile = request.profile.clone();
        if let Err(error) = self.app_store.update_config(|config| {
            if let Some(existing) = config
                .profiles
                .iter_mut()
                .find(|profile| profile.id == saved_profile.id)
            {
                *existing = saved_profile.clone();
            } else {
                config.profiles.push(saved_profile.clone());
            }
        }) {
            let restored_deleted = self.restore_deleted(deleted_credential);
            let restored_written = self.rollback_written(written_credential);
            if !restored_deleted || !restored_written {
                return Err(CredentialTransactionError::RollbackFailed);
            }
            return Err(error.into());
        }

        Ok(SaveProfileOutcome {
            profile: request.profile,
            connect_secret,
        })
    }

    pub fn delete_profile(
        &self,
        profile: &ConnectionProfile,
    ) -> Result<(), CredentialTransactionError> {
        let deleted_credential = if let Some(reference) = credential_reference(&profile.auth) {
            let previous_value = self.credential_store.get(reference)?;
            self.credential_store.delete(reference)?;
            Some((reference.clone(), previous_value))
        } else {
            None
        };

        let mut found = false;
        let result = self.app_store.update_config(|config| {
            let original_len = config.profiles.len();
            config.profiles.retain(|stored| stored.id != profile.id);
            found = config.profiles.len() != original_len;
        });
        if let Err(error) = result {
            if !self.restore_deleted(deleted_credential) {
                return Err(CredentialTransactionError::RollbackFailed);
            }
            return Err(error.into());
        }
        if !found {
            if !self.restore_deleted(deleted_credential) {
                return Err(CredentialTransactionError::RollbackFailed);
            }
            return Err(CredentialTransactionError::ProfileNotFound);
        }
        Ok(())
    }

    fn rollback_written(&self, credential: Option<(CredentialRef, Option<SecretString>)>) -> bool {
        let Some((reference, previous_value)) = credential else {
            return true;
        };
        match previous_value {
            Some(secret) => self.credential_store.put(&reference, &secret).is_ok(),
            None => self.credential_store.delete(&reference).is_ok(),
        }
    }

    fn restore_deleted(&self, credential: Option<(CredentialRef, Option<SecretString>)>) -> bool {
        let Some((reference, previous_value)) = credential else {
            return true;
        };
        previous_value.is_none_or(|secret| self.credential_store.put(&reference, &secret).is_ok())
    }
}

fn credential_reference(auth: &AuthConfig) -> Option<&CredentialRef> {
    match auth {
        AuthConfig::Password { credential_ref } => credential_ref.as_ref(),
        AuthConfig::PrivateKey { passphrase_ref, .. } => passphrase_ref.as_ref(),
        AuthConfig::Agent => None,
    }
}

fn expected_reference(profile_id: ProfileId, auth: &AuthConfig) -> Option<CredentialRef> {
    match auth {
        AuthConfig::Password { .. } => Some(CredentialRef::password(profile_id)),
        AuthConfig::PrivateKey { .. } => Some(CredentialRef::private_key_passphrase(profile_id)),
        AuthConfig::Agent => None,
    }
}

fn set_credential_reference(auth: &mut AuthConfig, reference: Option<CredentialRef>) {
    match auth {
        AuthConfig::Password { credential_ref } => *credential_ref = reference,
        AuthConfig::PrivateKey { passphrase_ref, .. } => *passphrase_ref = reference,
        AuthConfig::Agent => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::{
        collections::HashMap,
        fs,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };

    use oxide_ssh_core::{
        model::{AuthConfig, ConnectionProfile, Endpoint, ProfileId},
        storage::{AppStore, CONFIG_FILE_NAME},
    };
    use tempfile::tempdir;

    #[derive(Default)]
    struct FakeCredentialStore {
        values: Mutex<HashMap<String, String>>,
        fail_delete: AtomicBool,
    }

    impl FakeCredentialStore {
        fn value(&self, reference: &CredentialRef) -> Option<String> {
            self.values.lock().get(reference.as_str()).cloned()
        }
    }

    impl CredentialStore for FakeCredentialStore {
        fn get(&self, reference: &CredentialRef) -> Result<Option<SecretString>, CredentialError> {
            Ok(self.value(reference).map(SecretString::from))
        }

        fn put(
            &self,
            reference: &CredentialRef,
            secret: &SecretString,
        ) -> Result<(), CredentialError> {
            self.values
                .lock()
                .insert(reference.0.clone(), secret.expose_secret().to_owned());
            Ok(())
        }

        fn delete(&self, reference: &CredentialRef) -> Result<(), CredentialError> {
            if self.fail_delete.load(Ordering::Relaxed) {
                return Err(CredentialError::AccessDenied);
            }
            self.values.lock().remove(reference.as_str());
            Ok(())
        }
    }

    fn password_profile() -> ConnectionProfile {
        ConnectionProfile {
            id: ProfileId::new(),
            name: "fixture".into(),
            endpoint: Endpoint {
                host: "example.com".into(),
                port: 22,
            },
            username: "oxide".into(),
            auth: AuthConfig::Password {
                credential_ref: None,
            },
        }
    }

    #[test]
    fn remembered_secret_is_written_before_profile_reference() {
        let directory = tempdir().unwrap();
        let app_store = Arc::new(AppStore::open(directory.path().to_path_buf()).unwrap());
        let credential_store = Arc::new(FakeCredentialStore::default());
        let coordinator =
            ProfileCredentialCoordinator::new(app_store.clone(), credential_store.clone());
        let profile = password_profile();
        let reference = CredentialRef::password(profile.id);

        let outcome = coordinator
            .save_profile(SaveProfileRequest {
                profile,
                previous: None,
                secret: Some(SecretString::from("remembered-secret")),
                remember: true,
            })
            .unwrap();

        assert!(outcome.connect_secret.is_none());
        assert_eq!(
            credential_store.value(&reference).as_deref(),
            Some("remembered-secret")
        );
        assert!(matches!(
            &app_store.load_config().unwrap().profiles[0].auth,
            AuthConfig::Password {
                credential_ref: Some(stored)
            } if stored == &reference
        ));
        let json = fs::read_to_string(directory.path().join(CONFIG_FILE_NAME)).unwrap();
        assert!(!json.contains("remembered-secret"));
    }

    #[test]
    fn unremembered_secret_stays_transient_and_deletes_old_entry() {
        let directory = tempdir().unwrap();
        let app_store = Arc::new(AppStore::open(directory.path().to_path_buf()).unwrap());
        let credential_store = Arc::new(FakeCredentialStore::default());
        let coordinator =
            ProfileCredentialCoordinator::new(app_store.clone(), credential_store.clone());
        let mut previous = password_profile();
        let reference = CredentialRef::password(previous.id);
        previous.auth = AuthConfig::Password {
            credential_ref: Some(reference.clone()),
        };
        credential_store
            .put(&reference, &SecretString::from("old-secret"))
            .unwrap();
        app_store
            .update_config(|config| config.profiles.push(previous.clone()))
            .unwrap();
        let mut edited = previous.clone();
        edited.auth = AuthConfig::Password {
            credential_ref: Some(reference.clone()),
        };

        let outcome = coordinator
            .save_profile(SaveProfileRequest {
                profile: edited,
                previous: Some(previous),
                secret: Some(SecretString::from("connect-once")),
                remember: false,
            })
            .unwrap();

        assert_eq!(
            outcome.connect_secret.unwrap().expose_secret(),
            "connect-once"
        );
        assert!(credential_store.value(&reference).is_none());
        assert!(matches!(
            app_store.load_config().unwrap().profiles[0].auth,
            AuthConfig::Password {
                credential_ref: None
            }
        ));
    }

    #[test]
    fn profile_save_failure_rolls_back_new_keyring_entry() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join(CONFIG_FILE_NAME),
            b"{ corrupt configuration",
        )
        .unwrap();
        let app_store = Arc::new(AppStore::open(directory.path().to_path_buf()).unwrap());
        let credential_store = Arc::new(FakeCredentialStore::default());
        let coordinator = ProfileCredentialCoordinator::new(app_store, credential_store.clone());
        let profile = password_profile();
        let reference = CredentialRef::password(profile.id);

        assert!(matches!(
            coordinator.save_profile(SaveProfileRequest {
                profile,
                previous: None,
                secret: Some(SecretString::from("must-rollback")),
                remember: true,
            }),
            Err(CredentialTransactionError::Storage(_))
        ));
        assert!(credential_store.value(&reference).is_none());
    }

    #[test]
    fn auth_switch_stops_when_old_entry_cannot_be_deleted() {
        let directory = tempdir().unwrap();
        let app_store = Arc::new(AppStore::open(directory.path().to_path_buf()).unwrap());
        let credential_store = Arc::new(FakeCredentialStore::default());
        let coordinator =
            ProfileCredentialCoordinator::new(app_store.clone(), credential_store.clone());
        let mut previous = password_profile();
        let reference = CredentialRef::password(previous.id);
        previous.auth = AuthConfig::Password {
            credential_ref: Some(reference.clone()),
        };
        credential_store
            .put(&reference, &SecretString::from("old-secret"))
            .unwrap();
        app_store
            .update_config(|config| config.profiles.push(previous.clone()))
            .unwrap();
        credential_store.fail_delete.store(true, Ordering::Relaxed);
        let mut edited = previous.clone();
        edited.auth = AuthConfig::Agent;

        assert!(matches!(
            coordinator.save_profile(SaveProfileRequest {
                profile: edited,
                previous: Some(previous),
                secret: None,
                remember: false,
            }),
            Err(CredentialTransactionError::Credential(
                CredentialError::AccessDenied
            ))
        ));
        assert!(matches!(
            app_store.load_config().unwrap().profiles[0].auth,
            AuthConfig::Password { .. }
        ));
    }

    #[test]
    fn unremembering_fails_atomically_when_deletion_is_denied() {
        let directory = tempdir().unwrap();
        let app_store = Arc::new(AppStore::open(directory.path().to_path_buf()).unwrap());
        let credential_store = Arc::new(FakeCredentialStore::default());
        let coordinator =
            ProfileCredentialCoordinator::new(app_store.clone(), credential_store.clone());
        let mut previous = password_profile();
        let reference = CredentialRef::password(previous.id);
        previous.auth = AuthConfig::Password {
            credential_ref: Some(reference.clone()),
        };
        credential_store
            .put(&reference, &SecretString::from("old-secret"))
            .unwrap();
        app_store
            .update_config(|config| config.profiles.push(previous.clone()))
            .unwrap();
        credential_store.fail_delete.store(true, Ordering::Relaxed);
        let edited = previous.clone();

        assert!(matches!(
            coordinator.save_profile(SaveProfileRequest {
                profile: edited,
                previous: Some(previous),
                secret: None,
                remember: false,
            }),
            Err(CredentialTransactionError::Credential(
                CredentialError::AccessDenied
            ))
        ));

        // The failed transaction must leave both halves intact: the config
        // still references the credential and the keyring entry is untouched.
        assert_eq!(
            credential_store.value(&reference).as_deref(),
            Some("old-secret")
        );
        assert!(matches!(
            &app_store.load_config().unwrap().profiles[0].auth,
            AuthConfig::Password {
                credential_ref: Some(stored)
            } if stored == &reference
        ));
    }

    #[test]
    fn deleting_profile_deletes_credential_but_preserves_host_trust_store() {
        let directory = tempdir().unwrap();
        let app_store = Arc::new(AppStore::open(directory.path().to_path_buf()).unwrap());
        let credential_store = Arc::new(FakeCredentialStore::default());
        let coordinator =
            ProfileCredentialCoordinator::new(app_store.clone(), credential_store.clone());
        let mut profile = password_profile();
        let reference = CredentialRef::password(profile.id);
        profile.auth = AuthConfig::Password {
            credential_ref: Some(reference.clone()),
        };
        credential_store
            .put(&reference, &SecretString::from("old-secret"))
            .unwrap();
        app_store
            .update_config(|config| config.profiles.push(profile.clone()))
            .unwrap();

        coordinator.delete_profile(&profile).unwrap();

        assert!(app_store.load_config().unwrap().profiles.is_empty());
        assert!(credential_store.value(&reference).is_none());
        assert!(!app_store.known_hosts_path().exists());
    }

    #[test]
    fn application_identifier_is_stable() {
        assert_eq!(APPLICATION_IDENTIFIER, "io.github.linko-zhao.OxideSSH");
    }

    #[test]
    fn empty_reference_is_rejected_without_platform_access() {
        assert!(matches!(
            SystemCredentialStore::entry(&CredentialRef(String::new())),
            Err(CredentialError::InvalidReference)
        ));
    }

    #[test]
    fn keyring_errors_are_sanitized() {
        assert_eq!(
            map_keyring_error(KeyringError::BadEncoding(b"sensitive bytes".to_vec())),
            CredentialError::Corrupt
        );
        assert_eq!(format!("{:?}", CredentialError::Corrupt), "Corrupt");
    }
}

//! Add/edit connection form: field state, validation, and save requests.

use std::path::PathBuf;

use oxide_ssh_core::model::{
    AuthConfig, ConnectionProfile, Endpoint, ProfileId, ProfileValidationError,
};
use secrecy::SecretString;

use crate::credentials::SaveProfileRequest;

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
    use super::*;

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

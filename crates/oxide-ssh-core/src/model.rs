use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ProfileId(pub Uuid);

impl ProfileId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ProfileId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SessionId(pub Uuid);

impl SessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct CredentialRef(pub String);

impl CredentialRef {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn password(profile_id: ProfileId) -> Self {
        Self(format!("profile/{}/password", profile_id.0))
    }

    pub fn private_key_passphrase(profile_id: ProfileId) -> Self {
        Self(format!("profile/{}/private-key-passphrase", profile_id.0))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionProfile {
    pub id: ProfileId,
    pub name: String,
    pub endpoint: Endpoint,
    pub username: String,
    pub auth: AuthConfig,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ProfileValidationError {
    #[error("profile name is invalid")]
    Name,
    #[error("host is invalid")]
    Host,
    #[error("username is invalid")]
    Username,
    #[error("port is invalid")]
    Port,
    #[error("private key path is empty")]
    PrivateKeyPath,
}

impl ConnectionProfile {
    pub fn validate(&self) -> Result<(), ProfileValidationError> {
        let name_len = self.name.trim().chars().count();
        if !(1..=64).contains(&name_len) {
            return Err(ProfileValidationError::Name);
        }

        let host = self.endpoint.host.as_bytes();
        if host.is_empty()
            || host.len() > 253
            || !host.is_ascii()
            || host
                .iter()
                .any(|byte| byte.is_ascii_whitespace() || matches!(byte, 0 | b'/' | b'\\'))
        {
            return Err(ProfileValidationError::Host);
        }

        let username = self.username.as_bytes();
        if username.is_empty()
            || username.len() > 255
            || username
                .iter()
                .any(|byte| matches!(byte, 0 | b'\r' | b'\n'))
        {
            return Err(ProfileValidationError::Username);
        }

        if self.endpoint.port == 0 {
            return Err(ProfileValidationError::Port);
        }

        if let AuthConfig::PrivateKey { path, .. } = &self.auth
            && path.as_os_str().is_empty()
        {
            return Err(ProfileValidationError::PrivateKeyPath);
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "method",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AuthConfig {
    Password {
        credential_ref: Option<CredentialRef>,
    },
    PrivateKey {
        path: PathBuf,
        passphrase_ref: Option<CredentialRef>,
    },
    Agent,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSize {
    pub columns: u32,
    pub rows: u32,
    pub pixel_width: u32,
    pub pixel_height: u32,
}

impl TerminalSize {
    pub fn is_valid(self) -> bool {
        self.columns > 0 && self.rows > 0
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum LocaleSetting {
    #[default]
    #[serde(rename = "system")]
    System,
    #[serde(rename = "en-US")]
    EnUs,
    #[serde(rename = "zh-CN")]
    ZhCn,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ThemeSetting {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub locale: LocaleSetting,
    pub theme: ThemeSetting,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_profile() -> ConnectionProfile {
        ConnectionProfile {
            id: ProfileId::new(),
            name: "Local fixture".into(),
            endpoint: Endpoint {
                host: "example.com".into(),
                port: 22,
            },
            username: "oxide".into(),
            auth: AuthConfig::Agent,
        }
    }

    #[test]
    fn valid_profile_accepts_dns_ipv4_and_unbracketed_ipv6() {
        for host in ["example.com", "127.0.0.1", "2001:db8::1"] {
            let mut profile = valid_profile();
            profile.endpoint.host = host.into();
            assert_eq!(profile.validate(), Ok(()));
        }
    }

    #[test]
    fn name_uses_trimmed_unicode_scalar_length() {
        let mut profile = valid_profile();
        profile.name = format!("  {}  ", "界".repeat(64));
        assert_eq!(profile.validate(), Ok(()));

        profile.name = "界".repeat(65);
        assert_eq!(profile.validate(), Err(ProfileValidationError::Name));
        profile.name = " \t ".into();
        assert_eq!(profile.validate(), Err(ProfileValidationError::Name));
    }

    #[test]
    fn host_rejects_non_ascii_length_and_forbidden_bytes() {
        for host in [
            "",
            "host name",
            "host/name",
            "host\\name",
            "host\0name",
            "münchen.example",
        ] {
            let mut profile = valid_profile();
            profile.endpoint.host = host.into();
            assert_eq!(profile.validate(), Err(ProfileValidationError::Host));
        }

        let mut profile = valid_profile();
        profile.endpoint.host = "a".repeat(254);
        assert_eq!(profile.validate(), Err(ProfileValidationError::Host));
    }

    #[test]
    fn username_port_and_private_key_path_are_validated() {
        let mut profile = valid_profile();
        profile.username = "a".repeat(255);
        assert_eq!(profile.validate(), Ok(()));

        profile.username = "a".repeat(256);
        assert_eq!(profile.validate(), Err(ProfileValidationError::Username));
        profile.username = "bad\rname".into();
        assert_eq!(profile.validate(), Err(ProfileValidationError::Username));

        profile = valid_profile();
        profile.endpoint.port = 0;
        assert_eq!(profile.validate(), Err(ProfileValidationError::Port));

        profile = valid_profile();
        profile.auth = AuthConfig::PrivateKey {
            path: PathBuf::new(),
            passphrase_ref: None,
        };
        assert_eq!(
            profile.validate(),
            Err(ProfileValidationError::PrivateKeyPath)
        );
    }
}

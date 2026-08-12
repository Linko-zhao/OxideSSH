use secrecy::SecretString;
use thiserror::Error;

use crate::model::CredentialRef;

pub trait CredentialStore: Send + Sync {
    fn get(&self, reference: &CredentialRef) -> Result<Option<SecretString>, CredentialError>;
    fn put(&self, reference: &CredentialRef, secret: &SecretString) -> Result<(), CredentialError>;
    fn delete(&self, reference: &CredentialRef) -> Result<(), CredentialError>;
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CredentialError {
    #[error("credential store is unavailable")]
    Unavailable,
    #[error("credential store access was denied")]
    AccessDenied,
    #[error("credential reference is invalid")]
    InvalidReference,
    #[error("credential data is corrupt")]
    Corrupt,
    #[error("credential reference is ambiguous")]
    Ambiguous,
    #[error("credential operation is unsupported")]
    Unsupported,
}

#[cfg(test)]
mod tests {
    use crate::model::ProfileId;

    use super::*;

    #[test]
    fn credential_references_are_stable_and_contain_no_secret() {
        let id = ProfileId::new();

        assert_eq!(
            CredentialRef::password(id).as_str(),
            format!("profile/{}/password", id.0)
        );
        assert_eq!(
            CredentialRef::private_key_passphrase(id).as_str(),
            format!("profile/{}/private-key-passphrase", id.0)
        );
    }
}

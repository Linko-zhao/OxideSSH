mod en;
mod zh_cn;

use oxide_ssh_core::model::LocaleSetting;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MessageId {
    AppName,
    SearchConnections,
    AddConnection,
    NoConnections,
    NoSearchResults,
    SelectConnection,
    Settings,
    Sessions,
    Language,
    Theme,
    TrustedHosts,
    System,
    Light,
    English,
    SimplifiedChinese,
    Dark,
    Delete,
    Retry,
    Quit,
    OpenConfigDirectory,
    StorageCorrupt,
    NoTrustedHosts,
    Close,
    Copy,
    Name,
    Host,
    Port,
    Username,
    AuthMethod,
    Password,
    PrivateKey,
    Agent,
    Remember,
    Passphrase,
    Browse,
    Cancel,
    Save,
    AgentDescription,
    CredentialRequired,
    Connect,
    ConnectOnce,
    Edit,
    ConfirmDeleteProfile,
    ConfirmDeleteHost,
    ConfirmCloseTab,
    ConfirmQuit,
    Connecting,
    VerifyingHostKey,
    Authenticating,
    OpeningShell,
    Connected,
    Disconnected,
    AwaitingHostKey,
    AwaitingSecret,
    UnknownHostKey,
    ChangedHostKey,
    ExpectedFingerprint,
    PresentedFingerprint,
    Reject,
    AcceptAndStore,
    OpenTrustedHosts,
    Bell,
    InputQueueFull,
    PasteTooLarge,
    CredentialUnavailable,
    CredentialAccessDenied,
    CredentialInvalidReference,
    CredentialCorrupt,
    CredentialAmbiguous,
    CredentialUnsupported,
    InvalidProfile,
    ConnectTimeout,
    ConnectFailed,
    HostKeyRejected,
    HostKeyChanged,
    HostKeyStoreFailed,
    PrivateKeyUnreadable,
    PrivateKeyPassphraseRejected,
    AgentUnavailable,
    AgentEmpty,
    AuthenticationRejected,
    PtyRejected,
    SessionClosed,
    ShellRejected,
}

impl MessageId {
    pub const ALL: &[Self] = &[
        Self::AppName,
        Self::SearchConnections,
        Self::AddConnection,
        Self::NoConnections,
        Self::NoSearchResults,
        Self::SelectConnection,
        Self::Settings,
        Self::Sessions,
        Self::Language,
        Self::Theme,
        Self::TrustedHosts,
        Self::System,
        Self::Light,
        Self::English,
        Self::SimplifiedChinese,
        Self::Dark,
        Self::Delete,
        Self::Retry,
        Self::OpenConfigDirectory,
        Self::StorageCorrupt,
        Self::Quit,
        Self::Name,
        Self::NoTrustedHosts,
        Self::Close,
        Self::Copy,
        Self::Host,
        Self::Port,
        Self::Username,
        Self::AuthMethod,
        Self::Password,
        Self::PrivateKey,
        Self::Agent,
        Self::Remember,
        Self::Passphrase,
        Self::Browse,
        Self::Cancel,
        Self::Save,
        Self::AgentDescription,
        Self::CredentialRequired,
        Self::Connect,
        Self::ConnectOnce,
        Self::Edit,
        Self::ConfirmDeleteProfile,
        Self::ConfirmDeleteHost,
        Self::ConfirmCloseTab,
        Self::ConfirmQuit,
        Self::Connecting,
        Self::VerifyingHostKey,
        Self::Authenticating,
        Self::OpeningShell,
        Self::Connected,
        Self::Disconnected,
        Self::AwaitingHostKey,
        Self::AwaitingSecret,
        Self::UnknownHostKey,
        Self::ChangedHostKey,
        Self::ExpectedFingerprint,
        Self::PresentedFingerprint,
        Self::Reject,
        Self::AcceptAndStore,
        Self::OpenTrustedHosts,
        Self::Bell,
        Self::InputQueueFull,
        Self::PasteTooLarge,
        Self::CredentialUnavailable,
        Self::CredentialAccessDenied,
        Self::CredentialInvalidReference,
        Self::CredentialCorrupt,
        Self::CredentialAmbiguous,
        Self::CredentialUnsupported,
        Self::InvalidProfile,
        Self::ConnectTimeout,
        Self::ConnectFailed,
        Self::HostKeyRejected,
        Self::HostKeyChanged,
        Self::HostKeyStoreFailed,
        Self::PrivateKeyUnreadable,
        Self::PrivateKeyPassphraseRejected,
        Self::AgentUnavailable,
        Self::AgentEmpty,
        Self::AuthenticationRejected,
        Self::PtyRejected,
        Self::SessionClosed,
        Self::ShellRejected,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolvedLocale {
    EnUs,
    ZhCn,
}

impl ResolvedLocale {
    pub fn resolve(setting: LocaleSetting, system_locale: Option<&str>) -> Self {
        match setting {
            LocaleSetting::EnUs => Self::EnUs,
            LocaleSetting::ZhCn => Self::ZhCn,
            LocaleSetting::System => {
                let locale = system_locale
                    .unwrap_or_default()
                    .replace('_', "-")
                    .to_ascii_lowercase();
                if locale == "zh" || locale.starts_with("zh-") {
                    Self::ZhCn
                } else {
                    Self::EnUs
                }
            }
        }
    }
}

pub struct Catalog;

impl Catalog {
    pub fn text(locale: ResolvedLocale, id: MessageId) -> &'static str {
        match locale {
            ResolvedLocale::EnUs => en::text(id),
            ResolvedLocale::ZhCn => zh_cn::text(id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_id_has_english_and_chinese_text() {
        for id in MessageId::ALL {
            assert!(!Catalog::text(ResolvedLocale::EnUs, *id).is_empty());
            assert!(!Catalog::text(ResolvedLocale::ZhCn, *id).is_empty());
        }
    }

    #[test]
    fn system_locale_uses_chinese_only_for_zh_locales() {
        assert_eq!(
            ResolvedLocale::resolve(LocaleSetting::System, Some("zh-Hans-CN")),
            ResolvedLocale::ZhCn
        );
        assert_eq!(
            ResolvedLocale::resolve(LocaleSetting::System, Some("en-US")),
            ResolvedLocale::EnUs
        );
        assert_eq!(
            ResolvedLocale::resolve(LocaleSetting::System, None),
            ResolvedLocale::EnUs
        );
    }
}

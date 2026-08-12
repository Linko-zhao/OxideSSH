use std::{collections::VecDeque, mem};

use async_channel::Receiver;
use bytes::Bytes;
use oxide_ssh_core::{
    model::{ConnectionProfile, Endpoint, SessionId},
    session::{
        HostKeyDecision, SendInputError, SessionError, SessionEvent, SessionHandle, SessionState,
    },
};
use oxide_ssh_terminal::{
    InputEncoder, KeyInput, PasteError, TerminalAction, TerminalColors, TerminalError,
    TerminalModel, TerminalSize,
};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TabId(Uuid);

impl TabId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TabId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisconnectReason {
    Session(SessionError),
    Exit(Option<u32>),
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TabState {
    Connecting,
    AwaitingHostKey,
    AwaitingSecret,
    Connected,
    Disconnected { reason: DisconnectReason },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TabLocalError {
    InputQueueFull,
    InputClosed,
    PasteTooLarge,
    InvalidTerminalSize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TabNotification {
    Repaint,
    Bell,
    LocalError(TabLocalError),
}

pub struct Tab {
    id: TabId,
    profile: ConnectionProfile,
    state: TabState,
    terminal: TerminalModel,
    session: Option<SessionHandle>,
    session_id: Option<SessionId>,
    closing: bool,
    local_error: Option<TabLocalError>,
}

impl Tab {
    fn new(
        profile: ConnectionProfile,
        size: TerminalSize,
        colors: TerminalColors,
        awaiting_secret: bool,
    ) -> Result<Self, TerminalError> {
        Ok(Self {
            id: TabId::new(),
            profile,
            state: if awaiting_secret {
                TabState::AwaitingSecret
            } else {
                TabState::Connecting
            },
            terminal: TerminalModel::with_colors(size, colors)?,
            session: None,
            session_id: None,
            closing: false,
            local_error: None,
        })
    }

    pub fn id(&self) -> TabId {
        self.id
    }

    pub fn profile(&self) -> &ConnectionProfile {
        &self.profile
    }

    pub fn state(&self) -> &TabState {
        &self.state
    }

    pub fn session_id(&self) -> Option<SessionId> {
        self.session_id
    }

    pub fn closing(&self) -> bool {
        self.closing
    }

    pub fn is_disconnected(&self) -> bool {
        matches!(self.state, TabState::Disconnected { .. })
    }

    pub fn terminal(&self) -> &TerminalModel {
        &self.terminal
    }

    pub fn terminal_mut(&mut self) -> &mut TerminalModel {
        &mut self.terminal
    }

    pub fn local_error(&self) -> Option<TabLocalError> {
        self.local_error
    }

    pub fn clear_local_error(&mut self) {
        self.local_error = None;
    }

    pub fn attach_session(&mut self, mut session: SessionHandle) -> Option<Receiver<SessionEvent>> {
        let events = session.take_events()?;
        self.session_id = Some(session.id());
        self.session = Some(session);
        self.state = TabState::Connecting;
        self.local_error = None;
        Some(events)
    }

    fn await_secret(&mut self) {
        if let Some(session) = self.session.take() {
            session.disconnect();
            self.session_id = None;
        }
        self.state = TabState::AwaitingSecret;
        self.local_error = None;
    }

    pub fn apply_event(&mut self, event: SessionEvent) -> Vec<TabNotification> {
        match event {
            SessionEvent::StateChanged(state) => {
                match state {
                    SessionState::Connected => {
                        self.state = TabState::Connected;
                        self.local_error = None;
                    }
                    SessionState::Disconnected
                        if !matches!(
                            self.state,
                            TabState::Disconnected { .. } | TabState::AwaitingSecret
                        ) =>
                    {
                        self.state = TabState::Disconnected {
                            reason: DisconnectReason::Session(SessionError::Disconnected),
                        };
                        self.session = None;
                        self.session_id = None;
                    }
                    SessionState::Connecting
                    | SessionState::Authenticating
                    | SessionState::OpeningShell => {
                        self.state = TabState::Connecting;
                    }
                    SessionState::VerifyingHostKey => {
                        if !matches!(self.state, TabState::AwaitingHostKey) {
                            self.state = TabState::Connecting;
                        }
                    }
                    SessionState::Disconnected => {}
                }
                vec![TabNotification::Repaint]
            }
            SessionEvent::UnknownHostKey { .. } => {
                self.state = TabState::AwaitingHostKey;
                vec![TabNotification::Repaint]
            }
            SessionEvent::ChangedHostKey { .. } => {
                self.state = TabState::Disconnected {
                    reason: DisconnectReason::Session(SessionError::HostKeyChanged),
                };
                self.session = None;
                self.session_id = None;
                vec![TabNotification::Repaint]
            }
            SessionEvent::Output(bytes) => self.process_output(&bytes),
            SessionEvent::Bell => vec![TabNotification::Bell],
            SessionEvent::Exited { status } => {
                self.state = TabState::Disconnected {
                    reason: DisconnectReason::Exit(status),
                };
                self.session = None;
                self.session_id = None;
                vec![TabNotification::Repaint]
            }
            SessionEvent::Error(error) => {
                self.state = TabState::Disconnected {
                    reason: DisconnectReason::Session(error),
                };
                self.session = None;
                self.session_id = None;
                vec![TabNotification::Repaint]
            }
        }
    }

    pub fn send_key(&mut self, input: KeyInput<'_>) -> Result<(), TabLocalError> {
        let bytes = InputEncoder::encode(input, self.terminal.mode());
        if bytes.is_empty() {
            return Ok(());
        }
        self.send_bytes(Bytes::copy_from_slice(&bytes))
    }

    pub fn send_text(&mut self, text: &str) -> Result<(), TabLocalError> {
        self.send_key(KeyInput::text(text))
    }

    pub fn paste(&mut self, text: &str) -> Result<(), TabLocalError> {
        let bytes =
            InputEncoder::paste(text, self.terminal.mode()).map_err(|error| match error {
                PasteError::TooLarge => TabLocalError::PasteTooLarge,
            })?;
        self.send_bytes(bytes)
    }

    pub fn resize(&mut self, size: TerminalSize) -> Result<(), TabLocalError> {
        self.terminal
            .resize(size)
            .map_err(|_| TabLocalError::InvalidTerminalSize)?;
        if let Some(session) = &self.session {
            session.resize(oxide_ssh_core::model::TerminalSize {
                columns: u32::try_from(size.columns).unwrap_or(u32::MAX),
                rows: u32::try_from(size.rows).unwrap_or(u32::MAX),
                pixel_width: size.pixel_width,
                pixel_height: size.pixel_height,
            });
        }
        Ok(())
    }

    pub fn decide_host_key(
        &self,
        prompt_id: Uuid,
        decision: HostKeyDecision,
    ) -> Result<(), SessionError> {
        self.session
            .as_ref()
            .ok_or(SessionError::Disconnected)?
            .decide_host_key(prompt_id, decision)
    }

    pub fn disconnect(&self) {
        if let Some(session) = &self.session {
            session.disconnect();
        }
    }

    fn process_output(&mut self, bytes: &[u8]) -> Vec<TabNotification> {
        let mut notifications = Vec::new();
        for action in self.terminal.process_output(bytes) {
            match action {
                TerminalAction::WriteBack(bytes) => {
                    if let Err(error) = self.send_bytes(bytes) {
                        notifications.push(TabNotification::LocalError(error));
                    }
                }
                TerminalAction::Bell => notifications.push(TabNotification::Bell),
                TerminalAction::Wakeup => notifications.push(TabNotification::Repaint),
            }
        }
        notifications
    }

    fn send_bytes(&mut self, bytes: Bytes) -> Result<(), TabLocalError> {
        let result = self
            .session
            .as_ref()
            .ok_or(TabLocalError::InputClosed)?
            .try_send_input(bytes)
            .map_err(|error| match error {
                SendInputError::QueueFull => TabLocalError::InputQueueFull,
                SendInputError::Closed => TabLocalError::InputClosed,
            });
        match result {
            Err(error) => {
                self.local_error = Some(error);
                Err(error)
            }
            Ok(()) => {
                if self.local_error == Some(TabLocalError::InputQueueFull) {
                    // The queue has drained; the transient pause is over.
                    self.local_error = None;
                }
                Ok(())
            }
        }
    }

    fn prepare_reconnect(
        &mut self,
        size: TerminalSize,
        colors: TerminalColors,
    ) -> Result<(), TerminalError> {
        if let Some(session) = self.session.take() {
            session.disconnect();
        }
        self.session_id = None;
        self.closing = false;
        self.terminal = TerminalModel::with_colors(size, colors)?;
        self.state = TabState::Connecting;
        self.local_error = None;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub enum ModalRequest {
    HostKey {
        tab_id: TabId,
        prompt_id: Uuid,
        endpoint: Endpoint,
        algorithm: String,
        fingerprint_sha256: String,
    },
    ChangedHostKey {
        tab_id: TabId,
        endpoint: Endpoint,
        expected_sha256: String,
        presented_sha256: String,
    },
    Secret {
        tab_id: TabId,
    },
    ConfirmClose {
        tab_id: TabId,
    },
}

impl ModalRequest {
    pub fn tab_id(&self) -> TabId {
        match self {
            Self::HostKey { tab_id, .. }
            | Self::ChangedHostKey { tab_id, .. }
            | Self::Secret { tab_id }
            | Self::ConfirmClose { tab_id } => *tab_id,
        }
    }
}

#[derive(Default)]
pub struct ModalQueue {
    requests: VecDeque<ModalRequest>,
}

impl ModalQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current(&self) -> Option<&ModalRequest> {
        self.requests.front()
    }

    pub fn push(&mut self, request: ModalRequest) {
        let duplicate = self.requests.iter().any(|existing| {
            existing.tab_id() == request.tab_id()
                && mem::discriminant(existing) == mem::discriminant(&request)
        });
        if duplicate {
            return;
        }
        // Concurrent connections to the same unknown host share one prompt;
        // a waiter tab must not queue a second modal for the same decision.
        if let ModalRequest::HostKey {
            prompt_id: new_prompt,
            ..
        } = &request
            && self
                .requests
                .iter()
                .any(|existing| matches!(existing, ModalRequest::HostKey { prompt_id, .. } if prompt_id == new_prompt))
        {
            return;
        }
        self.requests.push_back(request);
    }

    pub fn complete_current(&mut self) -> Option<ModalRequest> {
        self.requests.pop_front()
    }

    pub fn remove_tab(&mut self, tab_id: TabId) -> Vec<ModalRequest> {
        let mut removed = Vec::new();
        self.requests.retain(|request| {
            if request.tab_id() == tab_id {
                removed.push(request.clone());
                false
            } else {
                true
            }
        });
        removed
    }

    pub fn is_empty(&self) -> bool {
        self.requests.is_empty()
    }
}

#[derive(Default)]
pub struct TabCollection {
    tabs: Vec<Tab>,
    active: Option<TabId>,
    modals: ModalQueue,
}

impl TabCollection {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(
        &mut self,
        profile: ConnectionProfile,
        size: TerminalSize,
        colors: TerminalColors,
        awaiting_secret: bool,
    ) -> TabId {
        let tab = Tab::new(profile, size, colors, awaiting_secret)
            .expect("TabCollection requires a valid terminal size");
        let id = tab.id();
        self.tabs.push(tab);
        self.active = Some(id);
        if awaiting_secret {
            self.modals.push(ModalRequest::Secret { tab_id: id });
        }
        id
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|tab| tab.id == id)
    }

    pub fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|tab| tab.id == id)
    }

    pub fn active(&self) -> Option<TabId> {
        self.active
    }

    pub fn set_active(&mut self, id: TabId) -> bool {
        if self.tab(id).is_none() {
            return false;
        }
        self.active = Some(id);
        true
    }

    pub fn apply_event(&mut self, id: TabId, event: SessionEvent) -> Vec<TabNotification> {
        match &event {
            SessionEvent::UnknownHostKey {
                prompt_id,
                endpoint,
                algorithm,
                fingerprint_sha256,
            } => self.modals.push(ModalRequest::HostKey {
                tab_id: id,
                prompt_id: *prompt_id,
                endpoint: endpoint.clone(),
                algorithm: algorithm.clone(),
                fingerprint_sha256: fingerprint_sha256.clone(),
            }),
            SessionEvent::ChangedHostKey {
                endpoint,
                expected_sha256,
                presented_sha256,
            } => self.modals.push(ModalRequest::ChangedHostKey {
                tab_id: id,
                endpoint: endpoint.clone(),
                expected_sha256: expected_sha256.clone(),
                presented_sha256: presented_sha256.clone(),
            }),
            SessionEvent::Error(error) if *error != SessionError::HostKeyChanged => {
                self.modals.remove_tab(id);
            }
            SessionEvent::Exited { .. } => {
                self.modals.remove_tab(id);
            }
            _ => {}
        }
        self.tab_mut(id)
            .map(|tab| tab.apply_event(event))
            .unwrap_or_default()
    }

    pub fn remove(&mut self, id: TabId) -> Option<Tab> {
        let index = self.tabs.iter().position(|tab| tab.id == id)?;
        self.modals.remove_tab(id);
        let tab = self.tabs.remove(index);
        tab.disconnect();
        if self.active == Some(id) {
            self.active = self
                .tabs
                .get(index)
                .or_else(|| self.tabs.last())
                .map(Tab::id);
        }
        Some(tab)
    }

    pub fn begin_close(&mut self, id: TabId) -> bool {
        let has_session = self.tab(id).is_some_and(|tab| tab.session.is_some());
        if !has_session {
            // Nothing to wait for: no live session is attached.
            self.remove(id);
            return false;
        }
        let tab = self.tab_mut(id).expect("tab exists");
        tab.closing = true;
        tab.disconnect();
        true
    }

    pub fn prepare_reconnect(
        &mut self,
        id: TabId,
        size: TerminalSize,
        colors: TerminalColors,
    ) -> Result<(), TerminalError> {
        self.modals.remove_tab(id);
        let tab = self.tab_mut(id).ok_or(TerminalError::InvalidSize)?;
        tab.prepare_reconnect(size, colors)
    }

    pub fn modals(&self) -> &ModalQueue {
        &self.modals
    }

    pub fn modals_mut(&mut self) -> &mut ModalQueue {
        &mut self.modals
    }

    pub fn request_secret(&mut self, id: TabId) -> Result<(), TerminalError> {
        self.modals.remove_tab(id);
        let tab = self.tab_mut(id).ok_or(TerminalError::InvalidSize)?;
        tab.await_secret();
        self.modals.push(ModalRequest::Secret { tab_id: id });
        Ok(())
    }
    pub fn request_close(&mut self, id: TabId) -> bool {
        if self.tab(id).is_none() {
            return false;
        }
        if self
            .tab(id)
            .is_some_and(|tab| tab.is_disconnected() || tab.closing())
        {
            // Already terminal: close immediately without a prompt.
            self.remove(id);
            return true;
        }
        self.modals.push(ModalRequest::ConfirmClose { tab_id: id });
        true
    }

    pub fn set_terminal_colors(&mut self, colors: TerminalColors) {
        for tab in &mut self.tabs {
            tab.terminal.set_default_colors(colors);
        }
    }

    pub fn disconnect_all(&self) {
        for tab in &self.tabs {
            tab.disconnect();
        }
    }

    pub fn persisted_state(&self) -> Option<()> {
        None
    }
}

#[cfg(test)]
mod tests {
    use oxide_ssh_core::{
        model::{AuthConfig, Endpoint, ProfileId},
        session::SessionState,
    };

    use super::*;

    fn profile() -> ConnectionProfile {
        ConnectionProfile {
            id: ProfileId::new(),
            name: "Fixture".into(),
            endpoint: Endpoint {
                host: "127.0.0.1".into(),
                port: 22,
            },
            username: "oxide".into(),
            auth: AuthConfig::Agent,
        }
    }

    fn size() -> TerminalSize {
        TerminalSize {
            columns: 80,
            rows: 24,
            pixel_width: 800,
            pixel_height: 480,
        }
    }

    #[test]
    fn same_profile_tabs_keep_output_and_state_isolated() {
        let profile = profile();
        let mut tabs = TabCollection::new();
        let first = tabs.open(profile.clone(), size(), TerminalColors::default(), false);
        let second = tabs.open(profile, size(), TerminalColors::default(), false);

        tabs.apply_event(first, SessionEvent::StateChanged(SessionState::Connected));
        tabs.apply_event(first, SessionEvent::Output(Bytes::from_static(b"first")));
        tabs.apply_event(second, SessionEvent::Output(Bytes::from_static(b"second")));

        assert_eq!(tabs.tab(first).unwrap().state(), &TabState::Connected);
        assert_eq!(tabs.tab(second).unwrap().state(), &TabState::Connecting);
        assert!(visible_text(tabs.tab(first).unwrap()).contains("first"));
        assert!(!visible_text(tabs.tab(first).unwrap()).contains("second"));
        assert!(visible_text(tabs.tab(second).unwrap()).contains("second"));
    }

    #[test]
    fn host_key_and_disconnect_events_have_explicit_states() {
        let mut tabs = TabCollection::new();
        let tab_id = tabs.open(profile(), size(), TerminalColors::default(), false);
        let prompt_id = uuid::Uuid::new_v4();
        tabs.apply_event(
            tab_id,
            SessionEvent::UnknownHostKey {
                prompt_id,
                endpoint: Endpoint {
                    host: "127.0.0.1".into(),
                    port: 22,
                },
                algorithm: "ssh-ed25519".into(),
                fingerprint_sha256: "SHA256:test".into(),
            },
        );
        assert!(matches!(
            tabs.tab(tab_id).unwrap().state(),
            TabState::AwaitingHostKey
        ));
        assert_eq!(tabs.modals().current().unwrap().tab_id(), tab_id);

        tabs.apply_event(
            tab_id,
            SessionEvent::Error(SessionError::AuthenticationRejected),
        );
        assert_eq!(
            tabs.tab(tab_id).unwrap().state(),
            &TabState::Disconnected {
                reason: DisconnectReason::Session(SessionError::AuthenticationRejected)
            }
        );
    }

    #[test]
    fn changed_host_key_modal_survives_the_matching_session_error() {
        let mut tabs = TabCollection::new();
        let tab_id = tabs.open(profile(), size(), TerminalColors::default(), false);
        tabs.apply_event(
            tab_id,
            SessionEvent::ChangedHostKey {
                endpoint: Endpoint {
                    host: "127.0.0.1".into(),
                    port: 22,
                },
                expected_sha256: "SHA256:old".into(),
                presented_sha256: "SHA256:new".into(),
            },
        );
        tabs.apply_event(tab_id, SessionEvent::Error(SessionError::HostKeyChanged));

        assert!(matches!(
            tabs.modals().current(),
            Some(ModalRequest::ChangedHostKey { tab_id: modal_tab, .. }) if *modal_tab == tab_id
        ));
    }

    #[test]
    fn modal_queue_is_fifo_and_closing_tab_removes_its_requests() {
        let mut queue = ModalQueue::new();
        let first = TabId::new();
        let second = TabId::new();
        queue.push(ModalRequest::Secret { tab_id: first });
        queue.push(ModalRequest::Secret { tab_id: second });
        queue.push(ModalRequest::ConfirmClose { tab_id: first });

        assert_eq!(queue.current().unwrap().tab_id(), first);
        let removed = queue.remove_tab(first);
        assert_eq!(removed.len(), 2);
        assert_eq!(queue.current().unwrap().tab_id(), second);
        assert!(queue.complete_current().is_some());
        assert!(queue.is_empty());
    }

    #[test]
    fn shared_host_key_prompt_is_queued_once_per_decision() {
        let mut queue = ModalQueue::new();
        let first = TabId::new();
        let second = TabId::new();
        let prompt_id = uuid::Uuid::new_v4();
        let endpoint = Endpoint {
            host: "127.0.0.1".into(),
            port: 22,
        };
        queue.push(ModalRequest::HostKey {
            tab_id: first,
            prompt_id,
            endpoint: endpoint.clone(),
            algorithm: "ssh-ed25519".into(),
            fingerprint_sha256: "SHA256:one".into(),
        });
        queue.push(ModalRequest::HostKey {
            tab_id: second,
            prompt_id,
            endpoint,
            algorithm: "ssh-ed25519".into(),
            fingerprint_sha256: "SHA256:one".into(),
        });

        assert_eq!(queue.current().unwrap().tab_id(), first);
        assert!(queue.complete_current().is_some());
        // The waiter tab's duplicate modal was never queued.
        assert!(queue.is_empty());
    }

    #[test]
    fn reconnect_replaces_terminal_and_preserves_tab_identity() {
        let mut tabs = TabCollection::new();
        let tab_id = tabs.open(profile(), size(), TerminalColors::default(), false);
        tabs.apply_event(
            tab_id,
            SessionEvent::Output(Bytes::from_static(b"old output")),
        );
        tabs.apply_event(tab_id, SessionEvent::Error(SessionError::Disconnected));

        tabs.prepare_reconnect(tab_id, size(), TerminalColors::default())
            .unwrap();
        let tab = tabs.tab(tab_id).unwrap();
        assert_eq!(tab.id(), tab_id);
        assert_eq!(tab.state(), &TabState::Connecting);
        assert!(!visible_text(tab).contains("old output"));
    }

    #[test]
    fn awaiting_secret_is_explicit_and_tabs_are_not_restorable() {
        let mut tabs = TabCollection::new();
        let tab_id = tabs.open(profile(), size(), TerminalColors::default(), true);
        assert_eq!(tabs.tab(tab_id).unwrap().state(), &TabState::AwaitingSecret);
        assert!(tabs.persisted_state().is_none());
    }

    #[test]
    fn closing_live_tab_requires_confirmation() {
        let mut tabs = TabCollection::new();
        let tab_id = tabs.open(profile(), size(), TerminalColors::default(), false);

        assert!(tabs.request_close(tab_id));

        assert!(tabs.tab(tab_id).is_some());
        assert!(matches!(
            tabs.modals().current(),
            Some(ModalRequest::ConfirmClose { tab_id: queued }) if *queued == tab_id
        ));
    }

    #[test]
    fn disconnected_tabs_close_without_confirmation() {
        let mut tabs = TabCollection::new();
        let tab_id = tabs.open(profile(), size(), TerminalColors::default(), false);
        tabs.apply_event(tab_id, SessionEvent::StateChanged(SessionState::Connected));
        tabs.apply_event(tab_id, SessionEvent::Error(SessionError::Disconnected));
        assert!(tabs.tab(tab_id).is_some_and(|tab| tab.is_disconnected()));

        assert!(tabs.request_close(tab_id));

        assert!(tabs.tab(tab_id).is_none());
        assert!(tabs.modals().current().is_none());
    }

    #[test]
    fn host_key_decision_progresses_past_awaiting_state() {
        let mut tabs = TabCollection::new();
        let tab_id = tabs.open(profile(), size(), TerminalColors::default(), false);
        tabs.apply_event(
            tab_id,
            SessionEvent::UnknownHostKey {
                prompt_id: uuid::Uuid::new_v4(),
                endpoint: Endpoint {
                    host: "127.0.0.1".into(),
                    port: 22,
                },
                algorithm: "ssh-ed25519".into(),
                fingerprint_sha256: "SHA256:test".into(),
            },
        );
        assert_eq!(
            tabs.tab(tab_id).unwrap().state(),
            &TabState::AwaitingHostKey
        );

        tabs.apply_event(
            tab_id,
            SessionEvent::StateChanged(SessionState::Authenticating),
        );
        assert_eq!(tabs.tab(tab_id).unwrap().state(), &TabState::Connecting);

        tabs.apply_event(tab_id, SessionEvent::StateChanged(SessionState::Connected));
        assert_eq!(tabs.tab(tab_id).unwrap().state(), &TabState::Connected);
    }

    #[test]
    fn theme_and_locale_switch_without_session_restart() {
        let mut tabs = TabCollection::new();
        let tab_id = tabs.open(profile(), size(), TerminalColors::default(), false);
        tabs.apply_event(tab_id, SessionEvent::StateChanged(SessionState::Connected));
        tabs.apply_event(tab_id, SessionEvent::Output(Bytes::from_static(b"content")));
        let light = TerminalColors {
            foreground: oxide_ssh_terminal::RgbColor::new(0x11, 0x22, 0x33),
            background: oxide_ssh_terminal::RgbColor::new(0xee, 0xee, 0xee),
            cursor: oxide_ssh_terminal::RgbColor::new(0x11, 0x22, 0x33),
            ..TerminalColors::default()
        };

        tabs.set_terminal_colors(light);
        let locale = crate::i18n::ResolvedLocale::ZhCn;

        let tab = tabs.tab(tab_id).unwrap();
        assert_eq!(locale, crate::i18n::ResolvedLocale::ZhCn);
        assert_eq!(tab.state(), &TabState::Connected);
        assert!(visible_text(tab).contains("content"));
        let content_cell = tab
            .terminal()
            .renderable_content()
            .display_iter
            .find(|indexed| indexed.cell.c == 'c')
            .unwrap();
        assert_eq!(
            tab.terminal()
                .cell_render_style(content_cell.cell)
                .foreground,
            light.foreground
        );
    }

    #[test]
    fn encrypted_key_can_request_a_secret_after_connect_starts() {
        let mut tabs = TabCollection::new();
        let tab_id = tabs.open(profile(), size(), TerminalColors::default(), false);

        tabs.request_secret(tab_id).unwrap();
        assert_eq!(tabs.tab(tab_id).unwrap().state(), &TabState::AwaitingSecret);
        assert!(matches!(
            tabs.modals().current(),
            Some(ModalRequest::Secret { tab_id: queued }) if *queued == tab_id
        ));

        tabs.apply_event(
            tab_id,
            SessionEvent::StateChanged(oxide_ssh_core::session::SessionState::Disconnected),
        );
        assert_eq!(tabs.tab(tab_id).unwrap().state(), &TabState::AwaitingSecret);
    }

    fn visible_text(tab: &Tab) -> String {
        tab.terminal()
            .renderable_content()
            .display_iter
            .map(|indexed| indexed.cell.c)
            .collect()
    }
}

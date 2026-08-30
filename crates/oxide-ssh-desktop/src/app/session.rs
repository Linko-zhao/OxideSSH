use super::*;
use super::dialog::ConfirmAction;
use super::messages::{
    credential_reference, local_error_message_id, requires_one_time_secret,
    session_error_message_id, transaction_error_message_id,
};

const OUTPUT_COALESCE_BYTES: usize = 64 * 1024;

impl AppView {
    pub(super) fn connect_profile(
        &mut self,
        profile: ConnectionProfile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.main_view = MainView::Sessions;
        if !self.connecting_profiles.insert(profile.id) {
            return;
        }
        let reference = credential_reference(&profile.auth).cloned();
        let store = self.credentials.clone();
        if let (Some(reference), Some(store)) = (reference, store) {
            cx.spawn_in(window, async move |weak, cx| {
                // Keyring access is blocking on some platforms; run it on the
                // background executor so the UI thread never freezes.
                let secret = cx
                    .background_executor()
                    .spawn(async move { store.get(&reference) })
                    .await;
                weak.update_in(cx, |this, window, cx| {
                    this.connect_profile_resolved(profile, secret, window, cx)
                })
                .ok();
            })
            .detach();
            return;
        }
        self.connect_profile_resolved(profile, Ok(None), window, cx);
    }

    fn connect_profile_resolved(
        &mut self,
        profile: ConnectionProfile,
        keyring_secret: Result<Option<SecretString>, CredentialError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.connecting_profiles.remove(&profile.id);
        let one_shot = self.connect_secrets.remove(&profile.id);
        let secret = match (one_shot, keyring_secret) {
            (Some(secret), _) => Some(secret),
            (None, Ok(secret)) => secret,
            (None, Err(_)) => {
                // The keyring could not be read: fall back to a one-time
                // in-memory prompt instead of blocking the connection.
                let needs_secret = requires_one_time_secret(&profile);
                let _ = self.tabs.open(
                    profile,
                    self.estimated_terminal_size(window),
                    self.theme.terminal_colors(),
                    needs_secret,
                );
                cx.notify();
                return;
            }
        };
        let needs_secret = requires_one_time_secret(&profile) && secret.is_none();
        let tab_id = self.tabs.open(
            profile,
            self.estimated_terminal_size(window),
            self.theme.terminal_colors(),
            needs_secret,
        );
        if !needs_secret {
            self.start_session(tab_id, secret, cx);
        }
        cx.notify();
    }

    fn clear_secret_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.secret
            .update(cx, |field, cx| field.set_value("", window, cx));
    }

    fn saved_secret(
        &self,
        profile: &ConnectionProfile,
    ) -> Result<Option<SecretString>, CredentialError> {
        let reference = match &profile.auth {
            AuthConfig::Password { credential_ref } => credential_ref.as_ref(),
            AuthConfig::PrivateKey { passphrase_ref, .. } => passphrase_ref.as_ref(),
            AuthConfig::Agent => None,
        };
        match (reference, &self.credentials) {
            (Some(reference), Some(store)) => store.get(reference),
            _ => Ok(None),
        }
    }

    pub(super) fn submit_secret(
        &mut self,
        expected_tab_id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .tabs
            .modals()
            .current()
            .is_some_and(|request| request.is_secret_for(expected_tab_id))
        {
            return;
        }
        let value = self.secret.read(cx).value().to_string();
        if value.is_empty() {
            self.status_message = Some(MessageId::CredentialRequired);
            cx.notify();
            return;
        }
        if self
            .tabs
            .modals_mut()
            .complete_current_if(|request| request.is_secret_for(expected_tab_id))
            .is_none()
        {
            return;
        }
        self.clear_secret_field(window, cx);
        self.start_session(expected_tab_id, Some(SecretString::from(value)), cx);
    }

    fn start_session(
        &mut self,
        tab_id: TabId,
        secret: Option<SecretString>,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.tab(tab_id) else {
            return;
        };
        if let AuthConfig::PrivateKey {
            path,
            passphrase_ref: None,
        } = &tab.profile().auth
            && secret.is_none()
        {
            // Preflight the key before any network activity so an encrypted
            // key prompts for its passphrase before host trust or handshake.
            let path = path.clone();
            cx.spawn(async move |weak, cx| {
                let requires = private_key_requires_passphrase(&path);
                weak.update(cx, |this, cx| {
                    match requires {
                        Ok(true) => {
                            if this.tabs.request_secret(tab_id).is_ok() {
                                this.status_message = None;
                            } else {
                                this.status_message = Some(MessageId::InvalidProfile);
                            }
                        }
                        Ok(false) => this.start_session_inner(tab_id, None, cx),
                        Err(error) => {
                            this.status_message = Some(session_error_message_id(error));
                            this.tabs.apply_event(tab_id, SessionEvent::Error(error));
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
            return;
        }
        self.start_session_inner(tab_id, secret, cx);
    }

    fn start_session_inner(
        &mut self,
        tab_id: TabId,
        secret: Option<SecretString>,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.tab(tab_id) else {
            return;
        };
        let terminal_size = tab.terminal().size();
        let request = ConnectRequest {
            profile: tab.profile().clone(),
            secret,
            initial_size: CoreTerminalSize {
                columns: terminal_size.columns as u32,
                rows: terminal_size.rows as u32,
                pixel_width: terminal_size.pixel_width,
                pixel_height: terminal_size.pixel_height,
            },
        };
        let result = self
            .ssh
            .as_ref()
            .ok_or(SessionError::ConnectFailed)
            .and_then(|service| service.connect(request));
        match result {
            Ok(session) => {
                let session_id = session.id();
                let Some(events) = self
                    .tabs
                    .tab_mut(tab_id)
                    .and_then(|tab| tab.attach_session(session))
                else {
                    self.status_message = Some(MessageId::ConnectFailed);
                    return;
                };
                cx.spawn(async move |weak, cx| {
                    let mut pending: Option<SessionEvent> = None;
                    loop {
                        let event = match pending.take() {
                            Some(event) => event,
                            None => match events.recv().await {
                                Ok(event) => event,
                                Err(_) => break,
                            },
                        };
                        if let SessionEvent::Output(first) = &event {
                            let mut total = first.len();
                            let mut batch = vec![event];
                            while total < OUTPUT_COALESCE_BYTES {
                                match events.try_recv() {
                                    Ok(SessionEvent::Output(bytes)) => {
                                        total += bytes.len();
                                        batch.push(SessionEvent::Output(bytes));
                                    }
                                    Ok(other) => {
                                        pending = Some(other);
                                        break;
                                    }
                                    Err(_) => break,
                                }
                            }
                            if weak
                                .update(cx, |this, cx| {
                                    this.handle_output_batch(tab_id, session_id, batch, cx)
                                })
                                .is_err()
                            {
                                break;
                            }
                        } else if weak
                            .update(cx, |this, cx| {
                                this.handle_session_event(tab_id, session_id, event, cx)
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .detach();
            }
            Err(error) => {
                self.tabs.apply_event(tab_id, SessionEvent::Error(error));
                self.status_message = Some(session_error_message_id(error));
            }
        }
    }

    fn handle_output_batch(
        &mut self,
        tab_id: TabId,
        session_id: SessionId,
        events: Vec<SessionEvent>,
        cx: &mut Context<Self>,
    ) {
        if self
            .tabs
            .tab(tab_id)
            .is_none_or(|tab| tab.session_id() != Some(session_id))
        {
            return;
        }
        let mut notifications = Vec::new();
        let mut repainted = false;
        for event in events {
            let batch_notifications = self.tabs.apply_event(tab_id, event);
            for notification in batch_notifications {
                match notification {
                    TabNotification::LocalError(error) => {
                        notifications.push(error);
                    }
                    TabNotification::Bell => self.bell = true,
                    TabNotification::Repaint => repainted = true,
                }
            }
        }
        if repainted {
            self.bell = false;
        }
        for error in notifications {
            self.status_message = Some(local_error_message_id(error));
        }
        cx.notify();
    }

    fn handle_session_event(
        &mut self,
        tab_id: TabId,
        session_id: SessionId,
        event: SessionEvent,
        cx: &mut Context<Self>,
    ) {
        if self
            .tabs
            .tab(tab_id)
            .is_none_or(|tab| tab.session_id() != Some(session_id))
        {
            // Events from a replaced session must never mutate the tab.
            return;
        }
        if matches!(
            &event,
            SessionEvent::Error(SessionError::PrivateKeyPassphraseRequired)
        ) {
            if self.tabs.request_secret(tab_id).is_err() {
                self.status_message = Some(MessageId::InvalidProfile);
            } else {
                self.status_message = None;
            }
            cx.notify();
            return;
        }
        let notifications = self.tabs.apply_event(tab_id, event);
        let mut repainted = false;
        for notification in notifications {
            match notification {
                TabNotification::LocalError(error) => {
                    self.status_message = Some(local_error_message_id(error));
                }
                TabNotification::Bell => self.bell = true,
                TabNotification::Repaint => repainted = true,
            }
        }
        if repainted {
            self.bell = false;
        }
        if matches!(
            self.tabs.tab(tab_id).map(|tab| *tab.state()),
            Some(TabState::Connected)
        ) {
            self.pending_terminal_focus = true;
            // Host-key acceptance is persisted by the time the session is up.
            if let Some(state) = &mut self.state
                && state.reload_known_hosts().is_err()
            {
                self.status_message = Some(MessageId::StorageCorrupt);
            }
        }
        self.after_tab_event(tab_id, cx);
        cx.notify();
    }

    fn after_tab_event(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        if self
            .tabs
            .tab(tab_id)
            .is_some_and(|tab| tab.closing() && tab.is_disconnected())
        {
            self.tabs.remove(tab_id);
            cx.notify();
        }
    }

    pub(super) fn reconnect(&mut self, tab_id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let profile = self.tabs.tab(tab_id).map(|tab| tab.profile().clone());
        if self
            .tabs
            .prepare_reconnect(
                tab_id,
                self.estimated_terminal_size(window),
                self.theme.terminal_colors(),
            )
            .is_err()
        {
            self.status_message = Some(MessageId::ConnectFailed);
            return;
        }
        let Some(profile) = profile else {
            return;
        };
        match self.saved_secret(&profile) {
            Ok(secret) => {
                if requires_one_time_secret(&profile) && secret.is_none() {
                    if self.tabs.request_secret(tab_id).is_err() {
                        self.status_message = Some(MessageId::InvalidProfile);
                    } else {
                        self.status_message = None;
                    }
                } else {
                    self.start_session(tab_id, secret, cx);
                }
            }
            Err(error) => self.status_message = Some(credential_error_message_id(error)),
        }
        cx.notify();
    }

    pub(super) fn close_tab(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        self.tabs.request_close(tab_id);
        cx.notify();
    }

    pub(super) fn cancel_tab_close(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        if self
            .tabs
            .modals_mut()
            .complete_current_if(|request| request.is_confirm_close_for(tab_id))
            .is_none()
        {
            return;
        }
        cx.notify();
    }

    pub(super) fn confirm_tab_close(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        if self
            .tabs
            .modals_mut()
            .complete_current_if(|request| request.is_confirm_close_for(tab_id))
            .is_none()
        {
            return;
        }
        if !self.tabs.begin_close(tab_id) {
            return;
        }
        cx.spawn(async move |weak, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(2))
                .await;
            weak.update(cx, |this, cx| {
                if this.tabs.tab(tab_id).is_some_and(|tab| tab.closing()) {
                    this.tabs.remove(tab_id);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn perform_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(action) = self.confirm.take() else {
            return;
        };
        match action {
            ConfirmAction::DeleteProfile(id) => {
                let profile = self
                    .state
                    .as_ref()
                    .and_then(|state| state.profiles().iter().find(|profile| profile.id == id))
                    .cloned();
                if let Some(profile) = profile {
                    self.connect_secrets.remove(&id);
                    if let Some(coordinator) = self.coordinator.clone() {
                        cx.spawn(async move |weak, cx| {
                            let result = coordinator.delete_profile(&profile);
                            weak.update(cx, |this, cx| {
                                if let Err(error) = result {
                                    this.status_message =
                                        Some(transaction_error_message_id(&error));
                                } else if let Some(state) = &mut this.state
                                    && state.reload_config().is_err()
                                {
                                    this.status_message = Some(MessageId::StorageCorrupt);
                                }
                                cx.notify();
                            })
                            .ok();
                        })
                        .detach();
                    } else {
                        self.status_message = Some(MessageId::InvalidProfile);
                    }
                }
            }
            ConfirmAction::DeleteHost(endpoint) => {
                if self
                    .state
                    .as_mut()
                    .is_some_and(|state| state.delete_known_host(&endpoint).is_err())
                {
                    self.status_message = Some(MessageId::StorageCorrupt);
                }
            }
            ConfirmAction::Quit => {
                self.quitting = true;
                self.tabs.disconnect_all();
                window.remove_window();
            }
        }
        cx.notify();
    }

    pub(super) fn decide_host_key(
        &mut self,
        tab_id: TabId,
        prompt_id: uuid::Uuid,
        decision: HostKeyDecision,
        cx: &mut Context<Self>,
    ) {
        if self
            .tabs
            .modals_mut()
            .complete_current_if(|request| request.is_host_key_for(tab_id, prompt_id))
            .is_none()
        {
            return;
        }
        if let Err(error) = self
            .tabs
            .tab(tab_id)
            .ok_or(SessionError::Disconnected)
            .and_then(|tab| tab.decide_host_key(prompt_id, decision))
        {
            self.status_message = Some(session_error_message_id(error));
        }
        cx.notify();
    }

    pub(super) fn open_trusted_hosts(&mut self, tab_id: TabId, request_id: Uuid, cx: &mut Context<Self>) {
        if self
            .tabs
            .modals_mut()
            .complete_current_if(|request| request.is_changed_host_key_for(tab_id, request_id))
            .is_none()
        {
            return;
        }
        self.main_view = MainView::Settings;
        cx.notify();
    }

    pub(super) fn cancel_secret(&mut self, tab_id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .tabs
            .modals_mut()
            .complete_current_if(|request| request.is_secret_for(tab_id))
            .is_none()
        {
            return;
        }
        self.clear_secret_field(window, cx);
        self.tabs.remove(tab_id);
        cx.notify();
    }

}

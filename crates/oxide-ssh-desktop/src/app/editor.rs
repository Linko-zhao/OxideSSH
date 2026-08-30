use super::*;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct EditorId(pub(super) Uuid);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum EditorSaveState {
    Idle,
    SavingThis,
    BlockedByOther,
}

pub(super) fn editor_save_state(saving: Option<EditorId>, editor_id: EditorId) -> EditorSaveState {
    match saving {
        None => EditorSaveState::Idle,
        Some(id) if id == editor_id => EditorSaveState::SavingThis,
        Some(_) => EditorSaveState::BlockedByOther,
    }
}

pub(super) struct ConnectionEditor {
    pub(super) id: EditorId,
    pub(super) form: ConnectionForm,
    pub(super) name: Entity<InputState>,
    pub(super) host: Entity<InputState>,
    pub(super) port: Entity<InputState>,
    pub(super) username: Entity<InputState>,
    pub(super) private_key_path: Entity<InputState>,
    pub(super) secret: Entity<InputState>,
}

impl ConnectionEditor {
    fn new(
        form: ConnectionForm,
        locale: ResolvedLocale,
        window: &mut Window,
        cx: &mut Context<AppView>,
    ) -> Self {
        let name = new_input(window, cx, MessageId::Name, &form.name, false, locale);
        let host = new_input(window, cx, MessageId::Host, &form.host, false, locale);
        let port = new_input(window, cx, MessageId::Port, &form.port, false, locale);
        let username = new_input(
            window,
            cx,
            MessageId::Username,
            &form.username,
            false,
            locale,
        );
        let private_key_path = new_input(
            window,
            cx,
            MessageId::PrivateKey,
            &form.private_key_path.to_string_lossy(),
            false,
            locale,
        );
        let secret_id = if form.auth_method == AuthMethod::PrivateKey {
            MessageId::Passphrase
        } else {
            MessageId::Password
        };
        let secret = new_input(window, cx, secret_id, "", true, locale);
        Self {
            id: EditorId(Uuid::new_v4()),
            form,
            name,
            host,
            port,
            username,
            private_key_path,
            secret,
        }
    }

    fn request(&mut self, cx: &App) -> Result<crate::credentials::SaveProfileRequest, FormError> {
        self.form.name = self.name.read(cx).value().to_string();
        self.form.host = self.host.read(cx).value().to_string();
        self.form.port = self.port.read(cx).value().to_string();
        self.form.username = self.username.read(cx).value().to_string();
        self.form.private_key_path =
            PathBuf::from(self.private_key_path.read(cx).value().to_string());
        self.form.secret = self.secret.read(cx).value().to_string();
        self.form.save_request()
    }
}

impl AppView {
    pub(super) fn show_add_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor = Some(ConnectionEditor::new(
            ConnectionForm::new(),
            self.locale,
            window,
            cx,
        ));
        cx.notify();
    }

    pub(super) fn show_edit_connection(
        &mut self,
        profile: ConnectionProfile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.connect_secrets.remove(&profile.id);
        self.editor = Some(ConnectionEditor::new(
            ConnectionForm::edit(profile),
            self.locale,
            window,
            cx,
        ));
        cx.notify();
    }

    pub(super) fn save_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor_id) = self.editor.as_ref().map(|editor| editor.id) else {
            return;
        };
        if self.saving_editor.is_some() {
            return;
        }
        let request = match self.editor.as_mut().map(|editor| editor.request(cx)) {
            Some(Ok(request)) => request,
            Some(Err(error)) => {
                self.status_message = Some(form_error_message_id(error));
                cx.notify();
                return;
            }
            None => return,
        };
        let Some(coordinator) = self.coordinator.clone() else {
            self.status_message = Some(MessageId::InvalidProfile);
            cx.notify();
            return;
        };
        self.saving_editor = Some(editor_id);
        cx.spawn_in(window, async move |weak, cx| {
            let result = coordinator.save_profile(request);
            weak.update_in(cx, |this, _window, cx| {
                // A newer editor may be open; only reconcile the editor that
                // started this transaction.
                if this.saving_editor == Some(editor_id) {
                    this.saving_editor = None;
                }
                match result {
                    Ok(outcome) => {
                        if let Some(secret) = outcome.connect_secret {
                            this.connect_secrets.insert(outcome.profile.id, secret);
                        }
                        if let Some(state) = &mut this.state
                            && state.reload_config().is_err()
                        {
                            this.status_message = Some(MessageId::StorageCorrupt);
                            return;
                        }
                        if this
                            .editor
                            .as_ref()
                            .is_some_and(|editor| editor.id == editor_id)
                        {
                            this.editor = None;
                        }
                        this.status_message = None;
                    }
                    Err(error) => this.status_message = Some(transaction_error_message_id(&error)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn cancel_editor(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
        cx.notify();
    }

    pub(super) fn browse_private_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor_id) = self.editor.as_ref().map(|editor| editor.id) else {
            return;
        };
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |weak, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = weak.update_in(cx, |this, window, cx| {
                this.apply_browse_result(editor_id, path, window, cx);
            });
        })
        .detach();
    }

    fn apply_browse_result(
        &mut self,
        editor_id: EditorId,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = &self.editor
            && editor.id == editor_id
        {
            editor.private_key_path.update(cx, |field, cx| {
                field.set_value(path.to_string_lossy().into_owned(), window, cx);
            });
        }
        cx.notify();
    }

    pub(super) fn select_auth_method(
        &mut self,
        method: AuthMethod,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = &mut self.editor {
            editor.form.auth_method = method;
            let placeholder = if method == AuthMethod::PrivateKey {
                MessageId::Passphrase
            } else {
                MessageId::Password
            };
            editor.secret.update(cx, |field, cx| {
                field.set_value("", window, cx);
                field.set_placeholder(Catalog::text(self.locale, placeholder), window, cx);
            });
        }
        cx.notify();
    }

    pub(super) fn toggle_remember(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = &mut self.editor {
            editor.form.remember = !editor.form.remember;
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_save_state_distinguishes_current_and_blocked_editors() {
        let current = EditorId(Uuid::new_v4());
        let other = EditorId(Uuid::new_v4());

        assert_eq!(editor_save_state(None, current), EditorSaveState::Idle);
        assert_eq!(
            editor_save_state(Some(current), current),
            EditorSaveState::SavingThis
        );
        assert_eq!(
            editor_save_state(Some(other), current),
            EditorSaveState::BlockedByOther
        );
    }
}

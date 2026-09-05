use super::editor::{EditorId, EditorSaveState, editor_save_state};
use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ConfirmAction {
    DeleteProfile(ProfileId),
    DeleteHost(Endpoint),
    Quit,
}

#[derive(Clone, Debug, PartialEq)]
struct DialogIdentity {
    locale: ResolvedLocale,
    kind: DialogIdentityKind,
}

#[derive(Clone, Debug, PartialEq)]
enum DialogIdentityKind {
    Editor {
        editor_id: EditorId,
        auth_method: AuthMethod,
        remember: bool,
        save_state: EditorSaveState,
        is_editing: bool,
    },
    Confirm(ConfirmAction),
    Secret(TabId),
    HostKey {
        tab_id: TabId,
        prompt_id: Uuid,
    },
    ChangedHostKey {
        tab_id: TabId,
        request_id: Uuid,
    },
    ConfirmClose(TabId),
}

/// Stable logical identity used to decide whether a deferred focus pass is
/// still current; excludes locale and editor view-state.
#[derive(Clone, Debug, Eq, PartialEq)]
enum DialogFocusIdentity {
    Editor(EditorId),
    Confirm(ConfirmAction),
    Secret(TabId),
    HostKey { tab_id: TabId, prompt_id: Uuid },
    ChangedHostKey { tab_id: TabId, request_id: Uuid },
    ConfirmClose(TabId),
}

impl DialogIdentityKind {
    fn focus_identity(&self) -> DialogFocusIdentity {
        match self {
            Self::Editor { editor_id, .. } => DialogFocusIdentity::Editor(*editor_id),
            Self::Confirm(action) => DialogFocusIdentity::Confirm(action.clone()),
            Self::Secret(tab_id) => DialogFocusIdentity::Secret(*tab_id),
            Self::HostKey { tab_id, prompt_id } => DialogFocusIdentity::HostKey {
                tab_id: *tab_id,
                prompt_id: *prompt_id,
            },
            Self::ChangedHostKey { tab_id, request_id } => DialogFocusIdentity::ChangedHostKey {
                tab_id: *tab_id,
                request_id: *request_id,
            },
            Self::ConfirmClose(tab_id) => DialogFocusIdentity::ConfirmClose(*tab_id),
        }
    }
}

#[derive(Clone)]
enum DialogSnapshot {
    Editor(EditorDialogSnapshot),
    Confirm(ConfirmAction),
    Modal {
        request: ModalRequest,
        /// The shared one-time credential input; rendered by the presenter for
        /// Secret requests. Carried in the payload because the AppView is
        /// leased while the dialog layer renders and cannot be read then.
        secret_input: Entity<InputState>,
    },
}

#[derive(Clone)]
struct EditorDialogSnapshot {
    editor_id: EditorId,
    is_editing: bool,
    auth_method: AuthMethod,
    remember: bool,
    save_state: EditorSaveState,
    name: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    username: Entity<InputState>,
    private_key_path: Entity<InputState>,
    secret: Entity<InputState>,
}

struct DialogPayload {
    identity: DialogIdentity,
    snapshot: DialogSnapshot,
}

struct DialogFocusBoundary {
    scope: FocusHandle,
    start: FocusHandle,
    end: FocusHandle,
}

impl DialogFocusBoundary {
    fn new(cx: &mut Context<DialogPresenter>) -> Self {
        Self {
            scope: cx.focus_handle(),
            start: cx.focus_handle(),
            end: cx.focus_handle(),
        }
    }

    fn contains_focused(&self, window: &Window, cx: &App) -> bool {
        self.scope.contains_focused(window, cx)
    }
}

pub(super) struct DialogPresenter {
    owner: WeakEntity<AppView>,
    payload: DialogPayload,
    focus: DialogFocusBoundary,
}

impl DialogPresenter {
    fn new(owner: WeakEntity<AppView>, payload: DialogPayload, cx: &mut Context<Self>) -> Self {
        Self {
            owner,
            payload,
            focus: DialogFocusBoundary::new(cx),
        }
    }

    fn text(&self, id: MessageId) -> &'static str {
        Catalog::text(self.payload.identity.locale, id)
    }

    fn dialog_width(&self) -> Pixels {
        match &self.payload.snapshot {
            DialogSnapshot::Editor(_) => px(560.),
            _ => px(520.),
        }
    }

    fn focus_initial(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.payload.snapshot {
            DialogSnapshot::Editor(editor) => {
                editor.name.update(cx, |input, cx| input.focus(window, cx));
                return;
            }
            DialogSnapshot::Modal {
                request: ModalRequest::Secret { .. },
                secret_input,
            } => {
                secret_input.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                    input.focus(window, cx);
                });
                return;
            }
            _ => {}
        }
        // Non-input dialogs: land on the first/last real component by
        // focusing the boundary anchor and advancing once.
        self.focus.start.focus(window);
        window.focus_next();
    }

    fn on_focus_next(&mut self, window: &mut Window, cx: &mut App) {
        window.focus_next();
        if !self.focus.contains_focused(window, cx) {
            self.focus.start.focus(window);
            window.focus_next();
        }
        cx.stop_propagation();
    }

    fn on_focus_prev(&mut self, window: &mut Window, cx: &mut App) {
        window.focus_prev();
        if !self.focus.contains_focused(window, cx) {
            self.focus.end.focus(window);
            window.focus_prev();
        }
        cx.stop_propagation();
    }

    /// Enter triggers the dialog's primary action; ignored while the action
    /// is in a disabled/in-progress state (e.g. an editor save in flight).
    fn on_submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.payload.snapshot {
            DialogSnapshot::Editor(editor) => {
                if editor.save_state != EditorSaveState::Idle {
                    return;
                }
                self.owner
                    .update(cx, |app, cx| app.save_editor(window, cx))
                    .ok();
            }
            DialogSnapshot::Confirm(_) => {
                self.owner
                    .update(cx, |app, cx| app.perform_confirm(window, cx))
                    .ok();
            }
            DialogSnapshot::Modal { request, .. } => match request {
                ModalRequest::Secret { tab_id } => {
                    let tab_id = *tab_id;
                    self.owner
                        .update(cx, |app, cx| app.submit_secret(tab_id, window, cx))
                        .ok();
                }
                ModalRequest::HostKey {
                    tab_id, prompt_id, ..
                } => {
                    let (tab_id, prompt_id) = (*tab_id, *prompt_id);
                    self.owner
                        .update(cx, |app, cx| {
                            app.decide_host_key(
                                tab_id,
                                prompt_id,
                                HostKeyDecision::AcceptAndStore,
                                cx,
                            )
                        })
                        .ok();
                }
                ModalRequest::ChangedHostKey {
                    tab_id, request_id, ..
                } => {
                    let (tab_id, request_id) = (*tab_id, *request_id);
                    self.owner
                        .update(cx, |app, cx| app.open_trusted_hosts(tab_id, request_id, cx))
                        .ok();
                }
                ModalRequest::ConfirmClose { tab_id } => {
                    let tab_id = *tab_id;
                    self.owner
                        .update(cx, |app, cx| app.confirm_tab_close(tab_id, cx))
                        .ok();
                }
            },
        }
    }

    /// Esc dismisses the dialog through the same path as its cancel/close
    /// button; ignored while an editor save is in flight.
    fn on_cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.payload.snapshot {
            DialogSnapshot::Editor(editor) => {
                if editor.save_state != EditorSaveState::Idle {
                    return;
                }
                self.owner.update(cx, |app, cx| app.cancel_editor(cx)).ok();
            }
            DialogSnapshot::Confirm(_) => {
                self.owner.update(cx, |app, cx| app.cancel_confirm(cx)).ok();
            }
            DialogSnapshot::Modal { request, .. } => match request {
                ModalRequest::Secret { tab_id } => {
                    let tab_id = *tab_id;
                    self.owner
                        .update(cx, |app, cx| app.cancel_secret(tab_id, window, cx))
                        .ok();
                }
                ModalRequest::HostKey {
                    tab_id, prompt_id, ..
                } => {
                    let (tab_id, prompt_id) = (*tab_id, *prompt_id);
                    self.owner
                        .update(cx, |app, cx| {
                            app.decide_host_key(tab_id, prompt_id, HostKeyDecision::Reject, cx)
                        })
                        .ok();
                }
                ModalRequest::ChangedHostKey {
                    tab_id, request_id, ..
                } => {
                    let (tab_id, request_id) = (*tab_id, *request_id);
                    self.owner
                        .update(cx, |app, cx| {
                            app.close_changed_host_key(tab_id, request_id, cx)
                        })
                        .ok();
                }
                ModalRequest::ConfirmClose { tab_id } => {
                    let tab_id = *tab_id;
                    self.owner
                        .update(cx, |app, cx| app.cancel_tab_close(tab_id, cx))
                        .ok();
                }
            },
        }
    }

    fn render_editor(
        &self,
        editor: &EditorDialogSnapshot,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let text = |id: MessageId| self.text(id).to_owned();
        let name_label = text(MessageId::Name);
        let host_label = text(MessageId::Host);
        let port_label = text(MessageId::Port);
        let username_label = text(MessageId::Username);
        let auth_label = text(MessageId::AuthMethod);
        let private_key_label = text(MessageId::PrivateKey);
        let password_label = text(MessageId::Password);
        let passphrase_label = text(MessageId::Passphrase);
        let remember_label = text(MessageId::Remember);
        let agent_description = text(MessageId::AgentDescription);
        let auth_index = match editor.auth_method {
            AuthMethod::Password => 0,
            AuthMethod::PrivateKey => 1,
            AuthMethod::Agent => 2,
        };
        let secret_label = if editor.auth_method == AuthMethod::PrivateKey {
            passphrase_label
        } else {
            password_label.clone()
        };
        let heading = if editor.is_editing {
            text(MessageId::Edit)
        } else {
            text(MessageId::AddConnection)
        };
        // The snapshot carries the editor identity; assert the projection
        // agrees with the dialog identity.
        debug_assert!(matches!(
            &self.payload.identity.kind,
            DialogIdentityKind::Editor { editor_id, .. } if *editor_id == editor.editor_id
        ));
        let secret_is_visible = editor.auth_method != AuthMethod::Agent;
        let cancel_listener = cx.listener(|this, _, _, cx| {
            this.owner.update(cx, |app, cx| app.cancel_editor(cx)).ok();
        });
        let save_listener = cx.listener(|this, _, window, cx| {
            this.owner
                .update(cx, |app, cx| app.save_editor(window, cx))
                .ok();
        });
        let browse_listener = cx.listener(|this, _, window, cx| {
            this.owner
                .update(cx, |app, cx| app.browse_private_key(window, cx))
                .ok();
        });
        let auth_listener = cx.listener(|this, index, window, cx| {
            let method = match *index {
                1 => AuthMethod::PrivateKey,
                2 => AuthMethod::Agent,
                _ => AuthMethod::Password,
            };
            let _ = this
                .owner
                .update(cx, |app, cx| app.select_auth_method(method, window, cx));
            if let DialogSnapshot::Editor(editor) = &mut this.payload.snapshot {
                editor.auth_method = method;
                let locale = this.payload.identity.locale;
                let placeholder = if method == AuthMethod::PrivateKey {
                    MessageId::Passphrase
                } else {
                    MessageId::Password
                };
                editor.secret.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                    input.set_placeholder(Catalog::text(locale, placeholder), window, cx);
                });
            }
            cx.notify();
        });
        let remember_listener = cx.listener(|this, checked, _, cx| {
            let _ = this.owner.update(cx, |app, cx| app.toggle_remember(cx));
            if let DialogSnapshot::Editor(editor) = &mut this.payload.snapshot {
                editor.remember = *checked;
            }
            cx.notify();
        });
        let browse_button = Button::new("browse-private-key")
            .label(text(MessageId::Browse))
            .on_click(browse_listener);
        v_form()
            .w(px(560.))
            .label_width(px(120.))
            .child(
                field().child(
                    div()
                        .text_size(px(20.))
                        .font_weight(gpui::FontWeight::BOLD)
                        .child(heading),
                ),
            )
            .child(field().label(name_label).child(Input::new(&editor.name)))
            .child(field().label(host_label).child(Input::new(&editor.host)))
            .child(field().label(port_label).child(Input::new(&editor.port)))
            .child(
                field()
                    .label(username_label)
                    .child(Input::new(&editor.username)),
            )
            .child(
                field().label(auth_label).child(
                    RadioGroup::horizontal("editor-auth")
                        .selected_index(Some(auth_index))
                        .child(Radio::new("auth-password").label(password_label))
                        .child(Radio::new("auth-private-key").label(private_key_label.clone()))
                        .child(Radio::new("auth-agent").label(text(MessageId::Agent)))
                        .on_click(auth_listener),
                ),
            )
            // NOTE: Field::visible is a no-op in gpui-component 0.5.1 (the
            // flag is stored but never read at render), so conditional
            // fields must be included with `when`, not `.visible(false)`.
            .when(editor.auth_method == AuthMethod::PrivateKey, |form| {
                form.child(
                    field()
                        .label(private_key_label)
                        .child(
                            h_flex()
                                .gap(px(8.))
                                .child(Input::new(&editor.private_key_path))
                                .child(browse_button),
                        ),
                )
            })
            .when(secret_is_visible, |form| {
                form.child(field().label(secret_label).child(Input::new(&editor.secret)))
            })
            .when(secret_is_visible, |form| {
                form.child(
                    field()
                        .label(remember_label)
                        .child(
                            Checkbox::new("editor-remember")
                                .checked(editor.remember)
                                .on_click(remember_listener),
                        ),
                )
            })
            .child(
                field().child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(
                            Button::new("cancel-editor")
                                .label(self.text(MessageId::Cancel))
                                .on_click(cancel_listener),
                        )
                        .child(
                            Button::new("save-editor")
                                .label(self.text(MessageId::Save))
                                .primary()
                                .loading(editor.save_state == EditorSaveState::SavingThis)
                                .disabled(editor.save_state != EditorSaveState::Idle)
                                .on_click(save_listener),
                        ),
                ),
            )
            .when(editor.auth_method == AuthMethod::Agent, |form| {
                form.child(
                    field().child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(agent_description),
                    ),
                )
            })
            .into_any_element()
    }

    fn render_confirm(&self, action: &ConfirmAction, cx: &mut Context<Self>) -> gpui::AnyElement {
        let (message, action_label) = match action {
            ConfirmAction::DeleteProfile(_) => (MessageId::ConfirmDeleteProfile, MessageId::Delete),
            ConfirmAction::DeleteHost(_) => (MessageId::ConfirmDeleteHost, MessageId::Delete),
            ConfirmAction::Quit => (MessageId::ConfirmQuit, MessageId::Quit),
        };
        let cancel_listener = cx.listener(|this, _, _, cx| {
            this.owner.update(cx, |app, cx| app.cancel_confirm(cx)).ok();
        });
        let accept_listener = cx.listener(move |this, _, window, cx| {
            this.owner
                .update(cx, |app, cx| app.perform_confirm(window, cx))
                .ok();
        });
        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(div().child(self.text(message)))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        Button::new("cancel-confirm")
                            .label(self.text(MessageId::Cancel))
                            .on_click(cancel_listener),
                    )
                    .child(
                        Button::new("accept-confirm")
                            .label(self.text(action_label))
                            .danger()
                            .on_click(accept_listener),
                    ),
            )
            .into_any_element()
    }

    fn render_modal(
        &self,
        request: &ModalRequest,
        secret_input: &Entity<InputState>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        match request {
            ModalRequest::Secret { tab_id } => {
                let tab_id = *tab_id;
                let secret_input = secret_input.clone();
                let cancel_listener = cx.listener(move |this, _, window, cx| {
                    this.owner
                        .update(cx, |app, cx| app.cancel_secret(tab_id, window, cx))
                        .ok();
                });
                let submit_listener = cx.listener(move |this, _, window, cx| {
                    this.owner
                        .update(cx, |app, cx| app.submit_secret(tab_id, window, cx))
                        .ok();
                });
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(self.text(MessageId::CredentialRequired)),
                    )
                    .child(Input::new(&secret_input))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                Button::new("cancel-secret")
                                    .label(self.text(MessageId::Cancel))
                                    .on_click(cancel_listener),
                            )
                            .child(
                                Button::new("connect-once")
                                    .label(self.text(MessageId::ConnectOnce))
                                    .primary()
                                    .on_click(submit_listener),
                            ),
                    )
                    .into_any_element()
            }
            ModalRequest::HostKey {
                tab_id,
                prompt_id,
                endpoint,
                algorithm,
                fingerprint_sha256,
            } => {
                let tab_id = *tab_id;
                let prompt_id = *prompt_id;
                let reject_listener = cx.listener(move |this, _, _, cx| {
                    this.owner
                        .update(cx, |app, cx| {
                            app.decide_host_key(tab_id, prompt_id, HostKeyDecision::Reject, cx)
                        })
                        .ok();
                });
                let accept_listener = cx.listener(move |this, _, _, cx| {
                    this.owner
                        .update(cx, |app, cx| {
                            app.decide_host_key(
                                tab_id,
                                prompt_id,
                                HostKeyDecision::AcceptAndStore,
                                cx,
                            )
                        })
                        .ok();
                });
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(self.text(MessageId::UnknownHostKey)),
                    )
                    .child(format!("{}:{}", endpoint.host, endpoint.port))
                    .child(algorithm.clone())
                    .child(fingerprint_sha256.clone())
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                Button::new("reject-host-key")
                                    .label(self.text(MessageId::Reject))
                                    .danger()
                                    .on_click(reject_listener),
                            )
                            .child(
                                Button::new("accept-host-key")
                                    .label(self.text(MessageId::AcceptAndStore))
                                    .primary()
                                    .on_click(accept_listener),
                            ),
                    )
                    .into_any_element()
            }
            ModalRequest::ChangedHostKey {
                tab_id,
                request_id,
                endpoint,
                expected_sha256,
                presented_sha256,
            } => {
                let tab_id = *tab_id;
                let request_id = *request_id;
                let close_listener = cx.listener(move |this, _, _, cx| {
                    this.owner
                        .update(cx, |app, cx| {
                            app.close_changed_host_key(tab_id, request_id, cx)
                        })
                        .ok();
                });
                let hosts_listener = cx.listener(move |this, _, _, cx| {
                    this.owner
                        .update(cx, |app, cx| app.open_trusted_hosts(tab_id, request_id, cx))
                        .ok();
                });
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(self.text(MessageId::ChangedHostKey)),
                    )
                    .child(format!("{}:{}", endpoint.host, endpoint.port))
                    .child(format!(
                        "{}: {}",
                        self.text(MessageId::ExpectedFingerprint),
                        expected_sha256
                    ))
                    .child(format!(
                        "{}: {}",
                        self.text(MessageId::PresentedFingerprint),
                        presented_sha256
                    ))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                Button::new("close-changed-host-key")
                                    .label(self.text(MessageId::Close))
                                    .on_click(close_listener),
                            )
                            .child(
                                Button::new("open-trusted-hosts")
                                    .label(self.text(MessageId::OpenTrustedHosts))
                                    .primary()
                                    .on_click(hosts_listener),
                            ),
                    )
                    .into_any_element()
            }
            ModalRequest::ConfirmClose { tab_id } => {
                let tab_id = *tab_id;
                let cancel_listener = cx.listener(move |this, _, _, cx| {
                    this.owner
                        .update(cx, |app, cx| app.cancel_tab_close(tab_id, cx))
                        .ok();
                });
                let confirm_listener = cx.listener(move |this, _, _, cx| {
                    this.owner
                        .update(cx, |app, cx| app.confirm_tab_close(tab_id, cx))
                        .ok();
                });
                div()
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .child(div().child(self.text(MessageId::ConfirmCloseTab)))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                Button::new("cancel-tab-close")
                                    .label(self.text(MessageId::Cancel))
                                    .on_click(cancel_listener),
                            )
                            .child(
                                Button::new("confirm-tab-close")
                                    .label(self.text(MessageId::Close))
                                    .danger()
                                    .on_click(confirm_listener),
                            ),
                    )
                    .into_any_element()
            }
        }
    }
}

impl Render for DialogPresenter {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match &self.payload.snapshot {
            DialogSnapshot::Editor(editor) => self.render_editor(editor, cx),
            DialogSnapshot::Confirm(action) => self.render_confirm(action, cx),
            DialogSnapshot::Modal {
                request,
                secret_input,
            } => self.render_modal(request, secret_input, cx),
        };
        div()
            .id("oxide-ssh-dialog")
            .key_context("OxideSSHDialog")
            .track_focus(&self.focus.scope)
            .on_action(
                cx.listener(|this, _: &DialogFocusNext, window, cx| this.on_focus_next(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &DialogFocusPrev, window, cx| this.on_focus_prev(window, cx)),
            )
            .on_action(cx.listener(|this, _: &DialogSubmit, window, cx| this.on_submit(window, cx)))
            .on_action(cx.listener(|this, _: &DialogCancel, window, cx| this.on_cancel(window, cx)))
            .child(div().track_focus(&self.focus.start))
            .child(content)
            .child(div().track_focus(&self.focus.end))
    }
}

impl AppView {
    fn requested_dialog(&self) -> Option<DialogPayload> {
        let locale = self.locale;
        if let Some(editor) = &self.editor {
            let save_state = editor_save_state(self.saving_editor, editor.id);
            return Some(DialogPayload {
                identity: DialogIdentity {
                    locale,
                    kind: DialogIdentityKind::Editor {
                        editor_id: editor.id,
                        auth_method: editor.form.auth_method,
                        remember: editor.form.remember,
                        save_state,
                        is_editing: editor.form.is_editing(),
                    },
                },
                snapshot: DialogSnapshot::Editor(EditorDialogSnapshot {
                    editor_id: editor.id,
                    is_editing: editor.form.is_editing(),
                    auth_method: editor.form.auth_method,
                    remember: editor.form.remember,
                    save_state,
                    name: editor.name.clone(),
                    host: editor.host.clone(),
                    port: editor.port.clone(),
                    username: editor.username.clone(),
                    private_key_path: editor.private_key_path.clone(),
                    secret: editor.secret.clone(),
                }),
            });
        }
        if let Some(action) = &self.confirm {
            return Some(DialogPayload {
                identity: DialogIdentity {
                    locale,
                    kind: DialogIdentityKind::Confirm(action.clone()),
                },
                snapshot: DialogSnapshot::Confirm(action.clone()),
            });
        }
        let request = self.tabs.modals().current()?.clone();
        let kind = match &request {
            ModalRequest::HostKey {
                tab_id, prompt_id, ..
            } => DialogIdentityKind::HostKey {
                tab_id: *tab_id,
                prompt_id: *prompt_id,
            },
            ModalRequest::ChangedHostKey {
                tab_id, request_id, ..
            } => DialogIdentityKind::ChangedHostKey {
                tab_id: *tab_id,
                request_id: *request_id,
            },
            ModalRequest::Secret { tab_id } => DialogIdentityKind::Secret(*tab_id),
            ModalRequest::ConfirmClose { tab_id } => DialogIdentityKind::ConfirmClose(*tab_id),
        };
        Some(DialogPayload {
            identity: DialogIdentity { locale, kind },
            snapshot: DialogSnapshot::Modal {
                request,
                secret_input: self.secret.clone(),
            },
        })
    }

    fn schedule_dialog_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let identity = self
            .requested_dialog()
            .map(|payload| payload.identity.kind.focus_identity());
        cx.defer_in(window, move |this, window, cx| {
            let current = this
                .requested_dialog()
                .map(|payload| payload.identity.kind.focus_identity());
            if current != identity {
                return;
            }
            if let Some(presenter) = &this.dialog_presenter {
                presenter.update(cx, |presenter, cx| presenter.focus_initial(window, cx));
            }
        });
    }

    pub(super) fn sync_dialog_layer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let requested = self.requested_dialog();
        match (requested, &self.dialog_presenter) {
            (None, None) => {
                if self.pending_terminal_focus {
                    self.pending_terminal_focus = false;
                    self.terminal_focus.focus(window);
                }
            }
            (None, Some(_)) => {
                window.close_dialog(cx);
                self.dialog_presenter = None;
                if self.pending_terminal_focus {
                    self.pending_terminal_focus = false;
                    self.terminal_focus.focus(window);
                }
                cx.notify();
            }
            (Some(payload), None) => {
                let owner = cx.entity().downgrade();
                let presenter = cx.new(|cx| DialogPresenter::new(owner, payload, cx));
                let presenter_entity = presenter.clone();
                self.dialog_presenter = Some(presenter);
                window.open_dialog(cx, move |dialog, _window, cx| {
                    let presenter = presenter_entity.clone();
                    let width = presenter.read(cx).dialog_width();
                    dialog
                        .overlay_closable(false)
                        .keyboard(false)
                        .close_button(false)
                        .width(width)
                        .child(presenter)
                });
                self.schedule_dialog_focus(window, cx);
            }
            (Some(payload), Some(existing)) => {
                let needs_focus = existing.read(cx).payload.identity.kind.focus_identity()
                    != payload.identity.kind.focus_identity();
                existing.update(cx, |presenter, cx| {
                    presenter.payload = payload;
                    cx.notify();
                });
                if needs_focus {
                    self.schedule_dialog_focus(window, cx);
                }
            }
        }
    }

    fn cancel_confirm(&mut self, cx: &mut Context<Self>) {
        self.confirm = None;
        cx.notify();
    }

    fn close_changed_host_key(&mut self, tab_id: TabId, request_id: Uuid, cx: &mut Context<Self>) {
        if self
            .tabs
            .modals_mut()
            .complete_current_if(|request| request.is_changed_host_key_for(tab_id, request_id))
            .is_none()
        {
            return;
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_focus_identity_survives_payload_updates() {
        let editor_id = EditorId(Uuid::new_v4());
        let initial = DialogIdentityKind::Editor {
            editor_id,
            auth_method: AuthMethod::Password,
            remember: false,
            save_state: EditorSaveState::Idle,
            is_editing: false,
        };
        let updated = DialogIdentityKind::Editor {
            editor_id,
            auth_method: AuthMethod::PrivateKey,
            remember: true,
            save_state: EditorSaveState::SavingThis,
            is_editing: false,
        };

        assert_eq!(initial.focus_identity(), updated.focus_identity());
    }

    #[test]
    fn request_focus_identity_requires_the_exact_request() {
        let tab_id = TabId::new();
        let first = DialogIdentityKind::HostKey {
            tab_id,
            prompt_id: Uuid::new_v4(),
        };
        let second = DialogIdentityKind::HostKey {
            tab_id,
            prompt_id: Uuid::new_v4(),
        };

        assert_ne!(first.focus_identity(), second.focus_identity());
    }
}

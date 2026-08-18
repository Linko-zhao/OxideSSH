use std::{collections::HashMap, path::PathBuf, sync::Arc};

use gpui::{
    App, Bounds, ClipboardItem, Context, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, InspectorElementId, KeyBinding,
    KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    Pixels, Point, Render, ScrollWheelEvent, ShapedLine, SharedString, StrikethroughStyle, Style,
    TextRun, UnderlineStyle, WeakEntity, Window, WindowAppearance, actions, div, fill, point,
    prelude::*, px, relative, rgb, size,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Root, Theme, ThemeMode, WindowExt as _,
    alert::Alert,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    form::{field, v_form},
    group_box::GroupBox,
    h_flex,
    input::{Input, InputState},
    list::ListItem,
    radio::{Radio, RadioGroup},
    tab::{Tab, TabBar},
};
use oxide_ssh_core::{
    credentials::{CredentialError, CredentialStore},
    model::{
        AuthConfig, ConnectionProfile, Endpoint, LocaleSetting, ProfileId, SessionId,
        TerminalSize as CoreTerminalSize, ThemeSetting,
    },
    session::{
        ConnectRequest, HostKeyDecision, SessionError, SessionEvent, SshService,
        private_key_requires_passphrase,
    },
};
use oxide_ssh_terminal::{CellRenderStyle, CellSide, Key, KeyInput, TerminalSize};
use secrecy::SecretString;
use uuid::Uuid;

use crate::{
    app_state::{AppLoadOutcome, AppState, AuthMethod, ConnectionForm, FormError, ResolvedTheme},
    credentials::{
        CredentialTransactionError, ProfileCredentialCoordinator, SystemCredentialStore,
        credential_error_message_id,
    },
    i18n::{Catalog, MessageId, ResolvedLocale},
    tabs::{
        DisconnectReason, ModalRequest, TabCollection, TabId, TabLocalError, TabNotification,
        TabState,
    },
};

actions!(
    oxide_ssh,
    [
        AddConnection,
        OpenSettings,
        NextTab,
        PreviousTab,
        CloseTab,
        Copy,
        Paste,
        TerminalTab,
        TerminalShiftTab,
        DialogFocusNext,
        DialogFocusPrev,
    ]
);

#[cfg(target_os = "macos")]
const TERMINAL_FONT: &str = "Menlo";
#[cfg(target_os = "windows")]
const TERMINAL_FONT: &str = "Cascadia Mono";
#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
const TERMINAL_FONT: &str = "monospace";

pub fn init(cx: &mut App) {
    let mut bindings = vec![
        KeyBinding::new("tab", TerminalTab, Some("Terminal")),
        KeyBinding::new("shift-tab", TerminalShiftTab, Some("Terminal")),
        // Registered after gpui_component::init so these override the Input
        // bindings only inside dialogs (deepest-context precedence).
        KeyBinding::new("tab", DialogFocusNext, Some("OxideSSHDialog")),
        KeyBinding::new("shift-tab", DialogFocusPrev, Some("OxideSSHDialog")),
        KeyBinding::new("tab", DialogFocusNext, Some("OxideSSHDialog > Input")),
        KeyBinding::new("shift-tab", DialogFocusPrev, Some("OxideSSHDialog > Input")),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, None),
    ];
    #[cfg(target_os = "macos")]
    {
        bindings.extend([
            KeyBinding::new("cmd-n", AddConnection, None),
            KeyBinding::new("cmd-,", OpenSettings, None),
            KeyBinding::new("cmd-w", CloseTab, None),
            KeyBinding::new("cmd-c", Copy, None),
            KeyBinding::new("cmd-v", Paste, None),
            KeyBinding::new("cmd-shift-[", PreviousTab, None),
            KeyBinding::new("cmd-shift-]", NextTab, None),
        ]);
    }
    #[cfg(not(target_os = "macos"))]
    {
        bindings.extend([
            KeyBinding::new("ctrl-n", AddConnection, None),
            KeyBinding::new("ctrl-,", OpenSettings, None),
            KeyBinding::new("ctrl-shift-w", CloseTab, None),
            KeyBinding::new("ctrl-shift-c", Copy, None),
            KeyBinding::new("ctrl-shift-v", Paste, None),
        ]);
    }
    cx.bind_keys(bindings);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MainView {
    Sessions,
    Settings,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ConfirmAction {
    DeleteProfile(ProfileId),
    DeleteHost(Endpoint),
    Quit,
}

#[derive(Clone, Copy)]
struct TerminalGeometry {
    bounds: Bounds<Pixels>,
    cell_width: Pixels,
    cell_height: Pixels,
    columns: usize,
    rows: usize,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct EditorId(Uuid);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EditorSaveState {
    Idle,
    SavingThis,
    BlockedByOther,
}

fn editor_save_state(saving: Option<EditorId>, editor_id: EditorId) -> EditorSaveState {
    match saving {
        None => EditorSaveState::Idle,
        Some(id) if id == editor_id => EditorSaveState::SavingThis,
        Some(_) => EditorSaveState::BlockedByOther,
    }
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

struct ConnectionEditor {
    id: EditorId,
    form: ConnectionForm,
    name: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    username: Entity<InputState>,
    private_key_path: Entity<InputState>,
    secret: Entity<InputState>,
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

fn new_input(
    window: &mut Window,
    cx: &mut Context<AppView>,
    placeholder: MessageId,
    value: &str,
    masked: bool,
    locale: ResolvedLocale,
) -> Entity<InputState> {
    let placeholder = Catalog::text(locale, placeholder);
    let value = value.to_owned();
    cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(placeholder)
            .default_value(value)
            .masked(masked)
    })
}

pub struct AppView {
    root: PathBuf,
    state: Option<AppState>,
    credentials: Option<Arc<SystemCredentialStore>>,
    coordinator: Option<Arc<ProfileCredentialCoordinator<SystemCredentialStore>>>,
    ssh: Option<SshService>,
    tabs: TabCollection,
    search: Entity<InputState>,
    secret: Entity<InputState>,
    connect_secrets: HashMap<ProfileId, SecretString>,
    connecting_profiles: std::collections::HashSet<ProfileId>,
    editor: Option<ConnectionEditor>,
    confirm: Option<ConfirmAction>,
    saving_editor: Option<EditorId>,
    dialog_presenter: Option<Entity<DialogPresenter>>,
    main_view: MainView,
    locale: ResolvedLocale,
    system_locale: Option<String>,
    theme: ResolvedTheme,
    focus_handle: FocusHandle,
    terminal_focus: FocusHandle,
    pending_terminal_focus: bool,
    composing: bool,
    compose_tab: Option<TabId>,
    scroll_accumulator: f32,
    bell: bool,
    status_message: Option<MessageId>,
    status_rendered: Option<MessageId>,
    terminal_geometry: Option<TerminalGeometry>,
    terminal_selecting: bool,
    quitting: bool,
    _search_subscription: gpui::Subscription,
}

impl AppView {
    pub fn new(
        root: PathBuf,
        outcome: AppLoadOutcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let system_locale = system_locale();
        let system_is_dark = is_dark(window.appearance());
        let (state, credentials, coordinator, ssh, status_message) = match outcome {
            AppLoadOutcome::Ready(state) => {
                let store = state.store().clone();
                let credentials = Arc::new(SystemCredentialStore::new());
                let coordinator = Arc::new(ProfileCredentialCoordinator::new(
                    store.clone(),
                    credentials.clone(),
                ));
                let ssh = SshService::new(store);
                let status_message = ssh
                    .as_ref()
                    .err()
                    .map(|error| session_error_message_id(*error));
                (
                    Some(state),
                    Some(credentials),
                    Some(coordinator),
                    ssh.ok(),
                    status_message,
                )
            }
            AppLoadOutcome::Recovery(_) => (None, None, None, None, None),
        };
        let locale = state
            .as_ref()
            .map(|state| state.resolved_locale(system_locale.as_deref()))
            .unwrap_or_else(|| {
                ResolvedLocale::resolve(LocaleSetting::System, system_locale.as_deref())
            });
        let theme = state
            .as_ref()
            .map(|state| state.resolved_theme(system_is_dark))
            .unwrap_or(ResolvedTheme::Dark);
        Theme::change(
            match theme {
                ResolvedTheme::Light => ThemeMode::Light,
                ResolvedTheme::Dark => ThemeMode::Dark,
            },
            Some(window),
            cx,
        );
        gpui_component::set_locale(match locale {
            ResolvedLocale::EnUs => "en",
            ResolvedLocale::ZhCn => "zh-CN",
        });
        let search = new_input(window, cx, MessageId::SearchConnections, "", false, locale);
        let secret = new_input(window, cx, MessageId::CredentialRequired, "", true, locale);
        let search_subscription = cx.observe(&search, |this, _, cx| {
            let _ = this;
            cx.notify();
        });
        Self {
            root,
            state,
            credentials,
            coordinator,
            ssh,
            tabs: TabCollection::new(),
            search,
            secret,
            connect_secrets: HashMap::new(),
            connecting_profiles: std::collections::HashSet::new(),
            editor: None,
            confirm: None,
            saving_editor: None,
            dialog_presenter: None,
            main_view: MainView::Sessions,
            locale,
            system_locale,
            theme,
            focus_handle: cx.focus_handle(),
            terminal_focus: cx.focus_handle(),
            pending_terminal_focus: false,
            composing: false,
            compose_tab: None,
            scroll_accumulator: 0.0,
            bell: false,
            status_message,
            status_rendered: None,
            terminal_geometry: None,
            terminal_selecting: false,
            quitting: false,
            _search_subscription: search_subscription,
        }
    }

    pub fn has_live_tabs(&self) -> bool {
        !self.quitting && self.tabs.tabs().iter().any(|tab| !tab.is_disconnected())
    }

    pub fn request_quit(&mut self, cx: &mut Context<Self>) {
        if self.has_live_tabs() {
            self.confirm = Some(ConfirmAction::Quit);
            cx.notify();
        }
    }

    fn text(&self, id: MessageId) -> &'static str {
        Catalog::text(self.locale, id)
    }

    fn refresh_preferences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = &self.state else {
            return;
        };
        let locale = state.resolved_locale(self.system_locale.as_deref());
        let theme = state.resolved_theme(is_dark(window.appearance()));
        if self.locale != locale {
            self.locale = locale;
            gpui_component::set_locale(match locale {
                ResolvedLocale::EnUs => "en",
                ResolvedLocale::ZhCn => "zh-CN",
            });
            self.update_input_placeholders(window, cx);
        }
        if self.theme != theme {
            self.theme = theme;
            Theme::change(
                match theme {
                    ResolvedTheme::Light => ThemeMode::Light,
                    ResolvedTheme::Dark => ThemeMode::Dark,
                },
                Some(window),
                cx,
            );
            self.tabs.set_terminal_colors(self.theme.terminal_colors());
        }
    }

    fn update_input_placeholders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let locale = self.locale;
        self.search.update(cx, |field, cx| {
            field.set_placeholder(
                Catalog::text(locale, MessageId::SearchConnections),
                window,
                cx,
            );
        });
        self.secret.update(cx, |field, cx| {
            field.set_placeholder(
                Catalog::text(locale, MessageId::CredentialRequired),
                window,
                cx,
            );
        });
        if let Some(editor) = &self.editor {
            let fields = [
                (&editor.name, MessageId::Name),
                (&editor.host, MessageId::Host),
                (&editor.port, MessageId::Port),
                (&editor.username, MessageId::Username),
                (&editor.private_key_path, MessageId::PrivateKey),
                (
                    &editor.secret,
                    if editor.form.auth_method == AuthMethod::PrivateKey {
                        MessageId::Passphrase
                    } else {
                        MessageId::Password
                    },
                ),
            ];
            for (entity, placeholder) in fields {
                entity.update(cx, |field, cx| {
                    field.set_placeholder(Catalog::text(locale, placeholder), window, cx);
                });
            }
        }
    }

    fn show_add_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor = Some(ConnectionEditor::new(
            ConnectionForm::new(),
            self.locale,
            window,
            cx,
        ));
        cx.notify();
    }

    fn show_edit_connection(
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

    fn save_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn cancel_editor(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
        cx.notify();
    }
    fn browse_private_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn connect_profile(
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

    fn submit_secret(
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

    fn reconnect(&mut self, tab_id: TabId, window: &mut Window, cx: &mut Context<Self>) {
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

    fn close_tab(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        self.tabs.request_close(tab_id);
        cx.notify();
    }

    fn cancel_tab_close(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
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

    fn confirm_tab_close(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
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

    fn perform_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn decide_host_key(
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

    fn open_trusted_hosts(&mut self, tab_id: TabId, request_id: Uuid, cx: &mut Context<Self>) {
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

    fn cancel_secret(&mut self, tab_id: TabId, window: &mut Window, cx: &mut Context<Self>) {
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

    fn set_locale(&mut self, setting: LocaleSetting, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .state
            .as_mut()
            .is_some_and(|state| state.set_locale(setting).is_err())
        {
            self.status_message = Some(MessageId::StorageCorrupt);
        }
        self.refresh_preferences(window, cx);
        cx.notify();
    }

    fn set_theme(&mut self, setting: ThemeSetting, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .state
            .as_mut()
            .is_some_and(|state| state.set_theme(setting).is_err())
        {
            self.status_message = Some(MessageId::StorageCorrupt);
        }
        self.refresh_preferences(window, cx);
        cx.notify();
    }

    fn estimated_terminal_size(&self, window: &Window) -> TerminalSize {
        let bounds = window.bounds();
        let width = (bounds.size.width - px(260.)).max(px(80.));
        let height = (bounds.size.height - px(40.)).max(px(40.));
        let text_style = window.text_style();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let cell_width = (font_size * 0.6).max(px(1.));
        let cell_height = window.line_height().max(px(1.));
        TerminalSize {
            columns: (f32::from(width) / f32::from(cell_width)).floor() as usize,
            rows: (f32::from(height) / f32::from(cell_height)).floor() as usize,
            pixel_width: u32::from(width),
            pixel_height: u32::from(height),
        }
    }

    fn retry_storage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let outcome = AppState::load(self.root.clone());
        let AppLoadOutcome::Ready(state) = outcome else {
            return;
        };
        let store = state.store().clone();
        let credentials = Arc::new(SystemCredentialStore::new());
        let coordinator = Arc::new(ProfileCredentialCoordinator::new(
            store.clone(),
            credentials.clone(),
        ));
        match SshService::new(store) {
            Ok(ssh) => {
                self.state = Some(state);
                self.credentials = Some(credentials);
                self.coordinator = Some(coordinator);
                self.ssh = Some(ssh);
                self.refresh_preferences(window, cx);
                self.status_message = None;
            }
            Err(error) => self.status_message = Some(session_error_message_id(error)),
        }
        cx.notify();
    }

    fn open_config_directory(&self, cx: &mut Context<Self>) {
        cx.open_with_system(&self.root);
    }

    fn cycle_tab(&mut self, backwards: bool, cx: &mut Context<Self>) {
        let tabs = self.tabs.tabs();
        if tabs.len() < 2 {
            return;
        }
        let current = self
            .tabs
            .active()
            .and_then(|active| tabs.iter().position(|tab| tab.id() == active))
            .unwrap_or(0);
        let next = if backwards {
            current.checked_sub(1).unwrap_or(tabs.len() - 1)
        } else {
            (current + 1) % tabs.len()
        };
        let id = tabs[next].id();
        self.tabs.set_active(id);
        cx.notify();
    }

    fn copy_terminal_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self
            .tabs
            .active()
            .and_then(|id| self.tabs.tab(id))
            .and_then(|tab| tab.terminal().selected_text())
        else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    fn paste_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let result = self
            .tabs
            .active()
            .and_then(|id| self.tabs.tab_mut(id))
            .map(|tab| tab.paste(&text));
        if let Some(Err(error)) = result {
            self.status_message = Some(local_error_message_id(error));
        }
        cx.notify();
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        #[cfg(target_os = "macos")]
        if keystroke.modifiers.platform {
            // Command shortcuts are handled by the global action bindings.
            return;
        }
        #[cfg(not(target_os = "linux"))]
        let plain_text = keystroke.key_char.is_some()
            && !keystroke.modifiers.control
            && !keystroke.modifiers.alt;
        #[cfg(not(target_os = "linux"))]
        if plain_text {
            // On Windows and macOS, unmodified printable keys are translated
            // by the platform (WM_CHAR / insertText) and arrive at the
            // terminal's input handler, which forwards them to the session.
            // Letting them propagate is what feeds the IME its first
            // composition keystroke.
            return;
        }
        let key = match keystroke.key.as_str() {
            "enter" => Key::Enter,
            "backspace" => Key::Backspace,
            "tab" => Key::Tab,
            "escape" => Key::Escape,
            "up" => Key::ArrowUp,
            "down" => Key::ArrowDown,
            "right" => Key::ArrowRight,
            "left" => Key::ArrowLeft,
            "home" => Key::Home,
            "end" => Key::End,
            "insert" => Key::Insert,
            "delete" => Key::Delete,
            "pageup" => Key::PageUp,
            "pagedown" => Key::PageDown,
            key if key.starts_with('f') => {
                let Some(function) = key.get(1..).and_then(|number| number.parse::<u8>().ok())
                else {
                    return;
                };
                Key::Function(function)
            }
            _ => {
                let Some(text) = keystroke.key_char.as_deref() else {
                    return;
                };
                Key::Text(text)
            }
        };
        let mut input = KeyInput::new(key);
        if keystroke.modifiers.control {
            input = input.control();
        }
        if keystroke.modifiers.alt {
            input = input.alt();
        }
        if keystroke.modifiers.shift {
            input = input.shift();
        }
        let result = self
            .tabs
            .active()
            .and_then(|id| self.tabs.tab_mut(id))
            .map(|tab| tab.send_key(input));
        match result {
            Some(Ok(())) => {
                if self.status_message == Some(MessageId::InputQueueFull) {
                    self.status_message = None;
                }
            }
            Some(Err(error)) => {
                self.status_message = Some(local_error_message_id(error));
            }
            None => {}
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn update_terminal_geometry(
        &mut self,
        tab_id: TabId,
        geometry: TerminalGeometry,
        cx: &mut Context<Self>,
    ) {
        self.terminal_geometry = Some(geometry);
        let size = TerminalSize {
            columns: geometry.columns,
            rows: geometry.rows,
            pixel_width: u32::from(geometry.bounds.size.width),
            pixel_height: u32::from(geometry.bounds.size.height),
        };
        let Some(tab) = self.tabs.tab_mut(tab_id) else {
            return;
        };
        if tab.terminal().size() != size {
            if tab.resize(size).is_err() {
                self.status_message = Some(MessageId::InvalidProfile);
            }
            cx.notify();
        }
    }

    fn terminal_cell_at(&self, position: Point<Pixels>) -> Option<(usize, usize, CellSide)> {
        let geometry = self.terminal_geometry?;
        if position.x < geometry.bounds.left()
            || position.x >= geometry.bounds.right()
            || position.y < geometry.bounds.top()
            || position.y >= geometry.bounds.bottom()
        {
            return None;
        }
        let local_x = position.x - geometry.bounds.left();
        let local_y = position.y - geometry.bounds.top();
        let column = (f32::from(local_x) / f32::from(geometry.cell_width)).floor() as usize;
        let row = (f32::from(local_y) / f32::from(geometry.cell_height)).floor() as usize;
        let cell_x = local_x - geometry.cell_width * column as f32;
        let side = if cell_x < geometry.cell_width * 0.5 {
            CellSide::Left
        } else {
            CellSide::Right
        };
        Some((
            row.min(geometry.rows.saturating_sub(1)),
            column.min(geometry.columns.saturating_sub(1)),
            side,
        ))
    }

    fn terminal_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal_focus.focus(window);
        let Some((row, column, side)) = self.terminal_cell_at(event.position) else {
            return;
        };
        if let Some(tab) = self.tabs.active().and_then(|id| self.tabs.tab_mut(id)) {
            tab.terminal_mut().start_selection(row, column, side);
            self.terminal_selecting = true;
            cx.notify();
        }
    }

    fn terminal_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.terminal_selecting || !event.dragging() {
            return;
        }
        let Some((row, column, side)) = self.terminal_cell_at(event.position) else {
            return;
        };
        if let Some(tab) = self.tabs.active().and_then(|id| self.tabs.tab_mut(id)) {
            tab.terminal_mut().update_selection(row, column, side);
            cx.notify();
        }
    }

    fn terminal_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.terminal_selecting = false;
    }

    fn terminal_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(geometry) = self.terminal_geometry else {
            return;
        };
        let delta = event.delta.pixel_delta(geometry.cell_height);
        self.scroll_accumulator += f32::from(delta.y) / f32::from(geometry.cell_height);
        let lines = self.scroll_accumulator.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_accumulator -= lines as f32;
        if let Some(tab) = self.tabs.active().and_then(|id| self.tabs.tab_mut(id)) {
            tab.terminal_mut().scroll_display(lines);
            cx.notify();
        }
    }

    fn dismiss_status(&mut self, cx: &mut Context<Self>) {
        self.status_message = None;
        cx.notify();
    }

    fn sync_status(&mut self, cx: &mut Context<Self>) {
        if self.status_message == self.status_rendered {
            return;
        }
        self.status_rendered = self.status_message;
        let Some(message) = self.status_message else {
            return;
        };
        // Transient banner: clear it automatically unless a newer message
        // replaced it in the meantime.
        cx.spawn(async move |weak, cx| {
            cx.background_executor().timer(STATUS_TIMEOUT).await;
            weak.update(cx, |this, cx| {
                if this.status_message == Some(message) {
                    this.status_message = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn render_recovery(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(560.))
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .child(
                        Alert::error("recovery-alert", self.text(MessageId::StorageCorrupt))
                            .title("OxideSSH"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.root.to_string_lossy().into_owned()),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(10.))
                            .child(
                                Button::new("recovery-open")
                                    .label(self.text(MessageId::OpenConfigDirectory))
                                    .on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.open_config_directory(cx)
                                        }),
                                    ),
                            )
                            .child(
                                Button::new("recovery-retry")
                                    .label(self.text(MessageId::Retry))
                                    .primary()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.retry_storage(window, cx)
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let query = self.search.read(cx).value().to_string();
        let Some(state) = &mut self.state else {
            return div().into_any_element();
        };
        state.set_search_query(query);
        let profiles: Vec<_> = state.filtered_profiles().into_iter().cloned().collect();
        let empty_message = if state.profiles().is_empty() {
            MessageId::NoConnections
        } else {
            MessageId::NoSearchResults
        };
        let mut list = div()
            .id("profile-list")
            .flex_1()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(6.));
        if profiles.is_empty() {
            list = list.child(
                div()
                    .p(px(12.))
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.text(empty_message)),
            );
        }
        for profile in profiles {
            let profile_for_connect = profile.clone();
            let profile_for_edit = profile.clone();
            let profile_for_delete = profile.clone();
            let endpoint = format!(
                "{}@{}:{}",
                profile.username, profile.endpoint.host, profile.endpoint.port
            );
            let status = self
                .profile_status(profile.id)
                .map(|status| status.to_owned());
            let edit_label = self.text(MessageId::Edit).to_owned();
            let delete_label = self.text(MessageId::Delete).to_owned();
            let edit_id = SharedString::from(format!("edit-{}", profile.id.0));
            let delete_id = SharedString::from(format!("delete-{}", profile.id.0));
            let view = cx.entity();
            list = list.child(
                ListItem::new(SharedString::from(format!("profile-{}", profile.id.0)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(profile.name.clone())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(endpoint),
                            )
                            .when_some(status, |element, status| {
                                element.child(
                                    div().text_xs().text_color(cx.theme().primary).child(status),
                                )
                            }),
                    )
                    .suffix(move |_, _| {
                        let edit_view = view.clone();
                        let delete_view = view.clone();
                        let edit_profile = profile_for_edit.clone();
                        let delete_profile = profile_for_delete.clone();
                        h_flex()
                            .gap(px(6.))
                            .child(
                                Button::new(edit_id.clone())
                                    .label(edit_label.clone())
                                    .ghost()
                                    .compact()
                                    .on_click(move |_, window, cx| {
                                        edit_view.update(cx, |this, cx| {
                                            cx.stop_propagation();
                                            this.show_edit_connection(
                                                edit_profile.clone(),
                                                window,
                                                cx,
                                            );
                                        });
                                    }),
                            )
                            .child(
                                Button::new(delete_id.clone())
                                    .label(delete_label.clone())
                                    .ghost()
                                    .compact()
                                    .on_click(move |_, _, cx| {
                                        delete_view.update(cx, |this, cx| {
                                            cx.stop_propagation();
                                            this.confirm = Some(ConfirmAction::DeleteProfile(
                                                delete_profile.id,
                                            ));
                                            cx.notify();
                                        });
                                    }),
                            )
                            .into_any_element()
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.connect_profile(profile_for_connect.clone(), window, cx)
                    })),
            );
        }
        div()
            .w(px(260.))
            .h_full()
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(12.))
            .border_r_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(self.text(MessageId::AppName)),
                    )
                    .child(
                        Button::new("settings")
                            .label(self.text(MessageId::Settings))
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.main_view = MainView::Settings;
                                cx.notify();
                            })),
                    ),
            )
            .child(Input::new(&self.search).prefix(Icon::new(IconName::Search)))
            .child(
                Button::new("add-connection")
                    .label(self.text(MessageId::AddConnection))
                    .icon(IconName::Plus)
                    .primary()
                    .on_click(
                        cx.listener(|this, _, window, cx| this.show_add_connection(window, cx)),
                    ),
            )
            .child(list)
            .into_any_element()
    }

    fn profile_status(&self, profile_id: ProfileId) -> Option<&'static str> {
        let tab = self
            .tabs
            .tabs()
            .iter()
            .rev()
            .find(|tab| tab.profile().id == profile_id)?;
        Some(self.text(tab_state_message_id(tab.state())))
    }

    fn render_sessions(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        if self.tabs.tabs().is_empty() {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child(self.text(MessageId::SelectConnection))
                .into_any_element();
        }
        let active = self.tabs.active();
        let tab_data: Vec<_> = self
            .tabs
            .tabs()
            .iter()
            .map(|tab| (tab.id(), tab.profile().name.clone(), *tab.state()))
            .collect();
        let selected_index = tab_data.iter().position(|(id, _, _)| Some(*id) == active);
        let tab_ids: Vec<_> = tab_data.iter().map(|(id, _, _)| *id).collect();
        let theme = cx.theme();
        let component_tabs = tab_data.into_iter().map(|(id, name, state)| {
            let dot_color = match state {
                TabState::Connected => theme.primary,
                TabState::Disconnected { .. } => theme.danger,
                TabState::AwaitingHostKey | TabState::AwaitingSecret | TabState::Connecting => {
                    theme.muted
                }
            };
            Tab::new()
                .label(name)
                .prefix(div().size(px(8.)).rounded_full().bg(dot_color))
                .suffix(
                    Button::new(SharedString::from(format!("close-{id:?}")))
                        .label("×")
                        .ghost()
                        .compact()
                        .on_click(cx.listener(move |this, _, _, cx| this.close_tab(id, cx))),
                )
        });
        let mut tab_bar = TabBar::new("tab-strip")
            .h(px(40.))
            .children(component_tabs)
            .on_click(cx.listener(move |this, index, _, cx| {
                if let Some(id) = tab_ids.get(*index) {
                    this.tabs.set_active(*id);
                    cx.notify();
                }
            }));
        if let Some(index) = selected_index {
            tab_bar = tab_bar.selected_index(index);
        }
        let body = self.render_active_tab(cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(tab_bar)
            .child(body)
            .into_any_element()
    }

    fn render_active_tab(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(tab_id) = self.tabs.active() else {
            return div().into_any_element();
        };
        let state = self.tabs.tab(tab_id).map(|tab| *tab.state());
        match state {
            Some(TabState::Connected) | Some(TabState::Disconnected { .. }) => {
                let disconnected = matches!(state, Some(TabState::Disconnected { .. }));
                div()
                    .id("terminal")
                    .size_full()
                    .overflow_hidden()
                    .relative()
                    .when(!disconnected, |element| element.key_context("Terminal"))
                    .bg(rgb(self.theme.terminal_colors().background.to_hex()))
                    .text_color(rgb(self.theme.terminal_colors().foreground.to_hex()))
                    .font_family(TERMINAL_FONT)
                    .text_size(px(13.))
                    .line_height(px(17.))
                    .track_focus(&self.terminal_focus)
                    .when(!disconnected, |element| {
                        element.on_key_down(cx.listener(Self::handle_key_down))
                    })
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::terminal_mouse_down))
                    .on_mouse_move(cx.listener(Self::terminal_mouse_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::terminal_mouse_up))
                    .on_mouse_up_out(MouseButton::Left, cx.listener(Self::terminal_mouse_up))
                    .on_scroll_wheel(cx.listener(Self::terminal_scroll))
                    .child(TerminalElement {
                        view: cx.entity(),
                        tab_id,
                    })
                    .when_some(disconnected.then_some(state), |element, state| {
                        let Some(TabState::Disconnected { reason }) = state else {
                            return element;
                        };
                        element.child(
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .top(px(12.))
                                .flex()
                                .justify_center()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(10.))
                                        .px(px(14.))
                                        .py(px(8.))
                                        .rounded(px(6.))
                                        .border_1()
                                        .border_color(cx.theme().border)
                                        .bg(cx.theme().background)
                                        .text_color(cx.theme().foreground)
                                        .text_size(px(13.))
                                        .child(self.text(disconnect_reason_message_id(reason)))
                                        .child(
                                            Button::new("retry-session")
                                                .label(self.text(MessageId::Retry))
                                                .primary()
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.reconnect(tab_id, window, cx)
                                                    },
                                                )),
                                        ),
                                ),
                        )
                    })
                    .into_any_element()
            }
            Some(state) => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child(self.text(tab_state_message_id(&state)))
                .into_any_element(),
            None => div().into_any_element(),
        }
    }

    fn render_settings(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let settings = self.state.as_ref().map(|state| state.settings().clone());
        let known_hosts = self
            .state
            .as_ref()
            .map(|state| state.known_hosts().to_vec())
            .unwrap_or_default();
        let Some(settings) = settings else {
            return div().into_any_element();
        };
        let mut trusted = div().flex().flex_col().gap(px(8.));
        if known_hosts.is_empty() {
            trusted = trusted.child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.text(MessageId::NoTrustedHosts)),
            );
        }
        for host in known_hosts {
            let endpoint = host.endpoint();
            let summary = format!(
                "{}:{}  {}  {}  {}",
                host.host,
                host.port,
                host.algorithm,
                host.fingerprint_sha256,
                host.accepted_at_unix
            );
            let delete_label = self.text(MessageId::Delete).to_owned();
            let view = cx.entity();
            trusted = trusted.child(
                ListItem::new(SharedString::from(format!(
                    "delete-host-{}-{}",
                    endpoint.host, endpoint.port
                )))
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(summary),
                )
                .suffix(move |_, _| {
                    let delete_view = view.clone();
                    let delete_endpoint = endpoint.clone();
                    Button::new("delete-host-action")
                        .label(delete_label.clone())
                        .ghost()
                        .compact()
                        .on_click(move |_, _, cx| {
                            delete_view.update(cx, |this, cx| {
                                this.confirm =
                                    Some(ConfirmAction::DeleteHost(delete_endpoint.clone()));
                                cx.notify();
                            });
                        })
                        .into_any_element()
                }),
            );
        }
        let locale_index = match settings.locale {
            LocaleSetting::System => 0,
            LocaleSetting::EnUs => 1,
            LocaleSetting::ZhCn => 2,
        };
        let theme_index = match settings.theme {
            ThemeSetting::System => 0,
            ThemeSetting::Light => 1,
            ThemeSetting::Dark => 2,
        };
        let language_label = self.text(MessageId::Language).to_owned();
        let system_label = self.text(MessageId::System).to_owned();
        let english_label = self.text(MessageId::English).to_owned();
        let simplified_label = self.text(MessageId::SimplifiedChinese).to_owned();
        let theme_label = self.text(MessageId::Theme).to_owned();
        let light_label = self.text(MessageId::Light).to_owned();
        let dark_label = self.text(MessageId::Dark).to_owned();
        let trusted_label = self.text(MessageId::TrustedHosts).to_owned();
        div()
            .id("settings-scroll")
            .size_full()
            .overflow_y_scroll()
            .p(px(28.))
            .flex()
            .flex_col()
            .gap(px(22.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(24.))
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(self.text(MessageId::Settings)),
                    )
                    .child(
                        Button::new("back-to-sessions")
                            .label(self.text(MessageId::Sessions))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.main_view = MainView::Sessions;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                GroupBox::new().title(language_label).child(
                    RadioGroup::horizontal("setting-locale")
                        .selected_index(Some(locale_index))
                        .child(Radio::new("locale-system").label(system_label.clone()))
                        .child(Radio::new("locale-english").label(english_label))
                        .child(Radio::new("locale-chinese").label(simplified_label))
                        .on_click(cx.listener(|this, index, window, cx| {
                            let setting = match *index {
                                1 => LocaleSetting::EnUs,
                                2 => LocaleSetting::ZhCn,
                                _ => LocaleSetting::System,
                            };
                            this.set_locale(setting, window, cx);
                        })),
                ),
            )
            .child(
                GroupBox::new().title(theme_label).child(
                    RadioGroup::horizontal("setting-theme")
                        .selected_index(Some(theme_index))
                        .child(Radio::new("theme-system").label(system_label))
                        .child(Radio::new("theme-light").label(light_label))
                        .child(Radio::new("theme-dark").label(dark_label))
                        .on_click(cx.listener(|this, index, window, cx| {
                            let setting = match *index {
                                1 => ThemeSetting::Light,
                                2 => ThemeSetting::Dark,
                                _ => ThemeSetting::System,
                            };
                            this.set_theme(setting, window, cx);
                        })),
                ),
            )
            .child(GroupBox::new().title(trusted_label).child(trusted))
            .into_any_element()
    }

    fn select_auth_method(
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

    fn toggle_remember(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = &mut self.editor {
            editor.form.remember = !editor.form.remember;
        }
        cx.notify();
    }

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

    fn sync_dialog_layer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn on_add_connection(
        &mut self,
        _: &AddConnection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editor.is_none() {
            self.show_add_connection(window, cx);
        }
        cx.stop_propagation();
    }

    fn on_open_settings(&mut self, _: &OpenSettings, _: &mut Window, cx: &mut Context<Self>) {
        self.main_view = MainView::Settings;
        cx.notify();
        cx.stop_propagation();
    }

    fn on_next_tab(&mut self, _: &NextTab, _: &mut Window, cx: &mut Context<Self>) {
        self.cycle_tab(false, cx);
        cx.stop_propagation();
    }

    fn on_previous_tab(&mut self, _: &PreviousTab, _: &mut Window, cx: &mut Context<Self>) {
        self.cycle_tab(true, cx);
        cx.stop_propagation();
    }

    fn on_close_tab(&mut self, _: &CloseTab, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.tabs.active() {
            self.close_tab(id, cx);
        }
        cx.stop_propagation();
    }

    fn on_terminal_tab(&mut self, _: &TerminalTab, _: &mut Window, cx: &mut Context<Self>) {
        self.send_terminal_input(KeyInput::new(Key::Tab), cx);
        cx.stop_propagation();
    }

    fn on_terminal_shift_tab(
        &mut self,
        _: &TerminalShiftTab,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_terminal_input(KeyInput::new(Key::Tab).shift(), cx);
        cx.stop_propagation();
    }

    fn send_terminal_input(&mut self, input: KeyInput<'_>, cx: &mut Context<Self>) {
        let result = self
            .tabs
            .active()
            .and_then(|id| self.tabs.tab_mut(id))
            .map(|tab| tab.send_key(input));
        match result {
            Some(Ok(())) => {
                if self.status_message == Some(MessageId::InputQueueFull) {
                    self.status_message = None;
                }
            }
            Some(Err(error)) => {
                self.status_message = Some(local_error_message_id(error));
            }
            None => {}
        }
        cx.notify();
    }

    fn on_copy(&mut self, _: &Copy, window: &mut Window, cx: &mut Context<Self>) {
        if self.focus_handle.is_focused(window) {
            self.copy_terminal_selection(cx);
            cx.stop_propagation();
        }
    }

    fn on_paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if self.focus_handle.is_focused(window) {
            self.paste_terminal(cx);
            cx.stop_propagation();
        }
    }
}

struct DialogPresenter {
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
            .child(
                field()
                    .label(private_key_label)
                    .visible(editor.auth_method == AuthMethod::PrivateKey)
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .child(Input::new(&editor.private_key_path))
                            .child(browse_button),
                    ),
            )
            .child(
                field()
                    .label(secret_label)
                    .visible(secret_is_visible)
                    .child(Input::new(&editor.secret)),
            )
            .child(
                field()
                    .label(remember_label)
                    .visible(secret_is_visible)
                    .child(
                        Checkbox::new("editor-remember")
                            .checked(editor.remember)
                            .on_click(remember_listener),
                    ),
            )
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
            .child(div().track_focus(&self.focus.start))
            .child(content)
            .child(div().track_focus(&self.focus.end))
    }
}

struct TerminalElement {
    view: Entity<AppView>,
    tab_id: TabId,
}

struct TerminalPrepaint {
    backgrounds: Vec<PaintQuad>,
    cursor: Vec<PaintQuad>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct RunKey {
    style: CellRenderStyle,
    selected: bool,
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = TerminalPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let text_style = window.text_style();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let base_font = text_style.font();
        let sample_run = TextRun {
            len: 1,
            font: base_font.clone(),
            color: text_style.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let sample = window
            .text_system()
            .shape_line("M".into(), font_size, &[sample_run], None);
        let cell_width = sample.x_for_index(1).max(px(1.));
        let cell_height = window.line_height().max(px(1.));
        let columns = (f32::from(bounds.size.width) / f32::from(cell_width)).floor() as usize;
        let rows = (f32::from(bounds.size.height) / f32::from(cell_height)).floor() as usize;
        let geometry = TerminalGeometry {
            bounds,
            cell_width,
            cell_height,
            columns: columns.max(2),
            rows: rows.max(1),
        };
        self.view.update(cx, |view, cx| {
            view.update_terminal_geometry(self.tab_id, geometry, cx)
        });

        let view = self.view.read(cx);
        let Some(tab) = view.tabs.tab(self.tab_id) else {
            return TerminalPrepaint {
                backgrounds: Vec::new(),
                cursor: Vec::new(),
                lines: Vec::new(),
            };
        };
        let model = tab.terminal();
        let terminal_colors = view.theme.terminal_colors();
        let selection_color = terminal_selection_color(view.theme);
        let content = model.renderable_content();
        let selection = content.selection;
        let cursor = content.cursor;
        let display_offset = content.display_offset as i32;

        let mut row_text = Vec::with_capacity(geometry.rows);
        let mut row_runs = Vec::with_capacity(geometry.rows);
        let mut row_keys = Vec::with_capacity(geometry.rows);
        for _ in 0..geometry.rows {
            row_text.push(String::with_capacity(geometry.columns));
            row_runs.push(Vec::<TextRun>::new());
            row_keys.push(None::<RunKey>);
        }

        let mut backgrounds = Vec::with_capacity(geometry.rows * 4);
        let mut background_segment: Option<(usize, usize, usize, u32)> = None;
        let default_background = terminal_colors.background.to_hex();
        for indexed in content.display_iter {
            let Ok(row) = usize::try_from(indexed.point.line.0 + display_offset) else {
                continue;
            };
            let column = indexed.point.column.0;
            if row >= geometry.rows || column >= geometry.columns {
                continue;
            }
            let style = model.cell_render_style(indexed.cell);
            let selected = selection.is_some_and(|range| range.contains(indexed.point));
            let background = if selected {
                selection_color
            } else {
                style.background.to_hex()
            };
            if selected || background != default_background {
                match background_segment {
                    Some((segment_row, start, end, color))
                        if segment_row == row && end == column && color == background =>
                    {
                        background_segment = Some((segment_row, start, column + 1, color));
                    }
                    Some(segment) => {
                        push_terminal_background(
                            &mut backgrounds,
                            segment,
                            bounds,
                            cell_width,
                            cell_height,
                        );
                        background_segment = Some((row, column, column + 1, background));
                    }
                    None => {
                        background_segment = Some((row, column, column + 1, background));
                    }
                }
            } else if let Some(segment) = background_segment.take() {
                push_terminal_background(
                    &mut backgrounds,
                    segment,
                    bounds,
                    cell_width,
                    cell_height,
                );
            }

            if style.wide_spacer || style.hidden {
                continue;
            }
            let text = &mut row_text[row];
            let before = text.len();
            text.push(indexed.cell.c);
            if let Some(zerowidth) = indexed.cell.zerowidth() {
                text.extend(zerowidth);
            }
            let byte_len = text.len() - before;
            let key = RunKey { style, selected };
            if row_keys[row] == Some(key) {
                if let Some(run) = row_runs[row].last_mut() {
                    run.len += byte_len;
                }
            } else {
                row_keys[row] = Some(key);
                let mut font = base_font.clone();
                if style.bold {
                    font = font.bold();
                }
                if style.italic {
                    font = font.italic();
                }
                let foreground = if selected {
                    terminal_colors.foreground.to_hex()
                } else {
                    style.foreground.to_hex()
                };
                row_runs[row].push(TextRun {
                    len: byte_len,
                    font,
                    color: rgb(foreground).into(),
                    background_color: None,
                    underline: style.underline.then_some(UnderlineStyle {
                        thickness: px(1.),
                        color: None,
                        wavy: false,
                    }),
                    strikethrough: style.strikeout.then_some(StrikethroughStyle {
                        thickness: px(1.),
                        color: None,
                    }),
                });
            }
        }
        if let Some(segment) = background_segment {
            push_terminal_background(&mut backgrounds, segment, bounds, cell_width, cell_height);
        }

        let mut lines = Vec::with_capacity(geometry.rows);
        for (row, (text, runs)) in row_text.into_iter().zip(row_runs).enumerate() {
            let line = window
                .text_system()
                .shape_line(text.into(), font_size, &runs, None);
            lines.push((
                point(bounds.left(), bounds.top() + cell_height * row as f32),
                line,
            ));
        }

        let cursor_row = usize::try_from(cursor.point.line.0 + display_offset).ok();
        let cursor = if cursor_row.is_none_or(|row| row >= geometry.rows)
            || cursor.point.column.0 >= geometry.columns
        {
            Vec::new()
        } else {
            let cursor_color = rgb(terminal_colors.cursor.to_hex());
            let cursor_x = bounds.left() + cell_width * cursor.point.column.0 as f32;
            let cursor_y = bounds.top() + cell_height * cursor_row.unwrap_or_default() as f32;
            match cursor.shape {
                alacritty_terminal::vte::ansi::CursorShape::Hidden => Vec::new(),
                alacritty_terminal::vte::ansi::CursorShape::Beam => vec![fill(
                    Bounds::new(point(cursor_x, cursor_y), size(px(2.), cell_height)),
                    cursor_color,
                )],
                alacritty_terminal::vte::ansi::CursorShape::Underline => vec![fill(
                    Bounds::new(
                        point(cursor_x, cursor_y + cell_height - px(2.)),
                        size(cell_width, px(2.)),
                    ),
                    cursor_color,
                )],
                alacritty_terminal::vte::ansi::CursorShape::HollowBlock => {
                    terminal_outline(cursor_x, cursor_y, cell_width, cell_height, cursor_color)
                }
                alacritty_terminal::vte::ansi::CursorShape::Block => vec![fill(
                    Bounds::new(point(cursor_x, cursor_y), size(cell_width, cell_height)),
                    gpui::Rgba {
                        a: 0.45,
                        ..cursor_color
                    },
                )],
            }
        };

        TerminalPrepaint {
            backgrounds,
            cursor,
            lines,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let view = self.view.clone();
        let focus = view.read(cx).terminal_focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, view), cx);
        for quad in prepaint.backgrounds.drain(..) {
            window.paint_quad(quad);
        }
        for quad in prepaint.cursor.drain(..) {
            window.paint_quad(quad);
        }
        for (origin, line) in prepaint.lines.drain(..) {
            let _ = line.paint(origin, window.line_height(), window, cx);
        }
    }
}

fn push_terminal_background(
    backgrounds: &mut Vec<PaintQuad>,
    (row, start, end, color): (usize, usize, usize, u32),
    bounds: Bounds<Pixels>,
    cell_width: Pixels,
    cell_height: Pixels,
) {
    backgrounds.push(fill(
        Bounds::new(
            point(
                bounds.left() + cell_width * start as f32,
                bounds.top() + cell_height * row as f32,
            ),
            size(cell_width * (end - start) as f32, cell_height),
        ),
        rgb(color),
    ));
}

fn terminal_outline(
    x: Pixels,
    y: Pixels,
    width: Pixels,
    height: Pixels,
    color: gpui::Rgba,
) -> Vec<PaintQuad> {
    let stroke = px(1.);
    vec![
        fill(Bounds::new(point(x, y), size(width, stroke)), color),
        fill(
            Bounds::new(point(x, y + height - stroke), size(width, stroke)),
            color,
        ),
        fill(Bounds::new(point(x, y), size(stroke, height)), color),
        fill(
            Bounds::new(point(x + width - stroke, y), size(stroke, height)),
            color,
        ),
    ]
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The dialog layer must never be mutated while AppView is on the
        // render stack; sync it on the next deferred pass.
        cx.defer_in(window, |this, window, cx| {
            this.sync_dialog_layer(window, cx)
        });
        self.refresh_preferences(window, cx);
        self.sync_status(cx);
        let body = if self.state.is_none() {
            self.render_recovery(cx)
        } else {
            let main = match self.main_view {
                MainView::Sessions => self.render_sessions(cx),
                MainView::Settings => self.render_settings(cx),
            };
            div()
                .size_full()
                .flex()
                .child(self.render_sidebar(cx))
                .child(div().flex_1().h_full().child(main))
                .into_any_element()
        };
        let status = self.status_message.map(|message| {
            div()
                .absolute()
                .left(px(276.))
                .bottom(px(14.))
                .max_w(px(620.))
                .child(
                    Alert::error("status-alert", self.text(message))
                        .on_close(cx.listener(|this, _, _, cx| this.dismiss_status(cx))),
                )
        });
        let bell = self.bell.then(|| {
            div()
                .absolute()
                .right(px(16.))
                .bottom(px(14.))
                .child(Alert::info("bell-alert", self.text(MessageId::Bell)))
        });
        div()
            .id("oxide-ssh-root")
            .size_full()
            .relative()
            .key_context("OxideSSH")
            .track_focus(&self.focus_handle)
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .font_family(".SystemUIFont")
            .on_action(cx.listener(Self::on_add_connection))
            .on_action(cx.listener(Self::on_open_settings))
            .on_action(cx.listener(Self::on_next_tab))
            .on_action(cx.listener(Self::on_previous_tab))
            .on_action(cx.listener(Self::on_close_tab))
            .on_action(cx.listener(Self::on_copy))
            .on_action(cx.listener(Self::on_paste))
            .on_action(cx.listener(Self::on_terminal_tab))
            .on_action(cx.listener(Self::on_terminal_shift_tab))
            .child(body)
            .children(status)
            .children(bell)
            .children(Root::render_dialog_layer(window, cx))
    }
}

impl Focusable for AppView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for AppView {
    fn text_for_range(
        &mut self,
        _range: std::ops::Range<usize>,
        _adjusted_range: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::UTF16Selection> {
        None
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        // A non-None range signals the platform that an IME composition is in
        // progress, which gates further keystrokes into the input context.
        self.composing.then_some(0..0)
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.composing = false;
        self.compose_tab = None;
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // IME final commit (or WM_CHAR / insertText passthrough): forward the
        // text to the tab that owns the composition, falling back to the
        // active tab.
        let target = self.compose_tab.or_else(|| self.tabs.active());
        let result = target
            .and_then(|id| self.tabs.tab_mut(id))
            .map(|tab| tab.send_text(text));
        self.composing = false;
        self.compose_tab = None;
        match result {
            Some(Ok(())) => {
                if self.status_message == Some(MessageId::InputQueueFull) {
                    self.status_message = None;
                }
            }
            Some(Err(error)) => {
                self.status_message = Some(local_error_message_id(error));
            }
            None => {}
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        _new_text: &str,
        _new_selected_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // Composition update: remember the owning tab so the final commit is
        // delivered to the session that started composing, even if the user
        // switches tabs mid-composition.
        if !self.composing {
            self.compose_tab = self.tabs.active();
        }
        self.composing = true;
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: std::ops::Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        None
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Drop for AppView {
    fn drop(&mut self) {
        self.tabs.disconnect_all();
    }
}

const OUTPUT_COALESCE_BYTES: usize = 64 * 1024;
const STATUS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

fn system_locale() -> Option<String> {
    sys_locale::get_locale()
}

fn is_dark(appearance: WindowAppearance) -> bool {
    matches!(
        appearance,
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}

fn credential_reference(auth: &AuthConfig) -> Option<&oxide_ssh_core::model::CredentialRef> {
    match auth {
        AuthConfig::Password { credential_ref } => credential_ref.as_ref(),
        AuthConfig::PrivateKey { passphrase_ref, .. } => passphrase_ref.as_ref(),
        AuthConfig::Agent => None,
    }
}

fn requires_one_time_secret(profile: &ConnectionProfile) -> bool {
    matches!(profile.auth, AuthConfig::Password { .. })
}

fn session_error_message_id(error: SessionError) -> MessageId {
    match error {
        SessionError::InvalidProfile => MessageId::InvalidProfile,
        SessionError::ConnectTimeout => MessageId::ConnectTimeout,
        SessionError::ConnectFailed | SessionError::Disconnected => MessageId::ConnectFailed,
        SessionError::HostKeyRejected => MessageId::HostKeyRejected,
        SessionError::HostKeyChanged => MessageId::HostKeyChanged,
        SessionError::HostKeyStoreFailed => MessageId::HostKeyStoreFailed,
        SessionError::CredentialUnavailable => MessageId::CredentialUnavailable,
        SessionError::PrivateKeyUnreadable => MessageId::PrivateKeyUnreadable,
        SessionError::PrivateKeyPassphraseRequired => MessageId::CredentialRequired,
        SessionError::PrivateKeyPassphraseRejected => MessageId::PrivateKeyPassphraseRejected,
        SessionError::AgentUnavailable => MessageId::AgentUnavailable,
        SessionError::AgentEmpty => MessageId::AgentEmpty,
        SessionError::AuthenticationRejected => MessageId::AuthenticationRejected,
        SessionError::PtyRejected => MessageId::PtyRejected,
        SessionError::ShellRejected => MessageId::ShellRejected,
    }
}

fn transaction_error_message_id(error: &CredentialTransactionError) -> MessageId {
    match error {
        CredentialTransactionError::Credential(error) => credential_error_message_id(*error),
        CredentialTransactionError::Storage(_) => MessageId::StorageCorrupt,
        CredentialTransactionError::InvalidProfile(_)
        | CredentialTransactionError::MissingSecret
        | CredentialTransactionError::ProfileMismatch
        | CredentialTransactionError::ProfileNotFound
        | CredentialTransactionError::RollbackFailed => MessageId::InvalidProfile,
    }
}

fn form_error_message_id(error: FormError) -> MessageId {
    match error {
        FormError::Name => MessageId::Name,
        FormError::Host => MessageId::Host,
        FormError::Username => MessageId::Username,
        FormError::Port => MessageId::Port,
        FormError::PrivateKeyPath => MessageId::PrivateKey,
    }
}

fn local_error_message_id(error: TabLocalError) -> MessageId {
    match error {
        TabLocalError::InputQueueFull => MessageId::InputQueueFull,
        TabLocalError::InputClosed => MessageId::SessionClosed,
        TabLocalError::PasteTooLarge => MessageId::PasteTooLarge,
        TabLocalError::InvalidTerminalSize => MessageId::InvalidProfile,
    }
}

fn tab_state_message_id(state: &TabState) -> MessageId {
    match state {
        TabState::Connecting => MessageId::Connecting,
        TabState::AwaitingHostKey => MessageId::AwaitingHostKey,
        TabState::AwaitingSecret => MessageId::AwaitingSecret,
        TabState::Connected => MessageId::Connected,
        TabState::Disconnected { .. } => MessageId::Disconnected,
    }
}

fn disconnect_reason_message_id(reason: DisconnectReason) -> MessageId {
    match reason {
        DisconnectReason::Session(error) => session_error_message_id(error),
        DisconnectReason::Exit(_) | DisconnectReason::Closed => MessageId::Disconnected,
    }
}

/// Selection tint for the custom terminal renderer, independent of the
/// component widget theme.
fn terminal_selection_color(theme: ResolvedTheme) -> u32 {
    match theme {
        ResolvedTheme::Dark => 0x315a78,
        ResolvedTheme::Light => 0xb8d8f0,
    }
}

trait RgbColorExt {
    fn to_hex(self) -> u32;
}

impl RgbColorExt for oxide_ssh_terminal::RgbColor {
    fn to_hex(self) -> u32 {
        ((self.red as u32) << 16) | ((self.green as u32) << 8) | self.blue as u32
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

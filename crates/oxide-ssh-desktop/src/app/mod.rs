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

mod messages;

use self::messages::{
    form_error_message_id, is_dark,
    local_error_message_id, session_error_message_id,
    system_locale, transaction_error_message_id,
};

mod editor;

use self::editor::{ConnectionEditor, EditorId};

mod dialog;

use self::dialog::{ConfirmAction, DialogPresenter};

mod session;

mod terminal;

use self::terminal::TerminalGeometry;

mod views;

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
    Workspace,
    Sessions,
    Settings,
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
            main_view: MainView::Workspace,
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
        self.main_view = MainView::Sessions;
        cx.notify();
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

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The dialog layer must never be mutated while AppView is on the
        // render stack; sync it on the next deferred pass.
        cx.defer_in(window, |this, window, cx| {
            this.sync_dialog_layer(window, cx)
        });
        self.refresh_preferences(window, cx);
        self.sync_status(cx);
        let sidebar_visible = self.main_view == MainView::Workspace
            || (self.main_view == MainView::Sessions && self.tabs.tabs().is_empty());
        let body = if self.state.is_none() {
            self.render_recovery(cx)
        } else {
            let main = match self.main_view {
                MainView::Workspace => self.render_workspace(cx),
                MainView::Sessions if self.tabs.tabs().is_empty() => self.render_workspace(cx),
                MainView::Sessions => self.render_sessions(cx),
                MainView::Settings => self.render_settings(cx),
            };
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(self.render_tab_strip(cx))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .when(sidebar_visible, |element| {
                            element.child(self.render_sidebar(cx))
                        })
                        .child(div().flex_1().h_full().child(main)),
                )
                .into_any_element()
        };
        let status = self.status_message.map(|message| {
            div()
                .absolute()
                .left(px(if sidebar_visible { 216. } else { 16. }))
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

impl Drop for AppView {
    fn drop(&mut self) {
        self.tabs.disconnect_all();
    }
}

const STATUS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);


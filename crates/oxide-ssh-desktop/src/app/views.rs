use super::*;
use super::dialog::ConfirmAction;
use super::messages::{disconnect_reason_message_id, tab_state_message_id};
use super::terminal::{RgbColorExt, TERMINAL_FONT, TerminalElement};

impl AppView {
    pub(super) fn render_recovery(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
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

    fn nav_item(
        &self,
        id: &'static str,
        icon: IconName,
        label: &'static str,
        active: bool,
        cx: &App,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = cx.theme();
        let item = h_flex()
            .id(id)
            .items_center()
            .gap(px(10.))
            .px(px(10.))
            .py(px(8.))
            .rounded(px(8.))
            .text_sm()
            .child(Icon::new(icon).size(px(16.)))
            .child(label);
        if active {
            item.bg(theme.accent).text_color(theme.accent_foreground)
        } else {
            let hover_bg = theme.list_hover;
            item.text_color(theme.muted_foreground)
                .hover(move |style| style.bg(hover_bg))
        }
    }

    pub(super) fn render_sidebar(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let workspace_active = self.main_view == MainView::Workspace;
        let settings_active = self.main_view == MainView::Settings;
        let placeholder_items = [
            ("nav-keychain", IconName::Asterisk, MessageId::Keychain),
            (
                "nav-port-forwarding",
                IconName::ArrowRight,
                MessageId::PortForwarding,
            ),
            ("nav-snippets", IconName::File, MessageId::Snippets),
            (
                "nav-known-hosts",
                IconName::CircleCheck,
                MessageId::KnownHosts,
            ),
            ("nav-logs", IconName::BookOpen, MessageId::Logs),
        ];
        let mut nav = div().flex().flex_col().gap(px(4.));
        nav = nav.child(
            self.nav_item(
                "nav-hosts",
                IconName::Globe,
                self.text(MessageId::Hosts),
                workspace_active,
                cx,
            )
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.main_view = MainView::Workspace;
                cx.notify();
            })),
        );
        for (id, icon, label) in placeholder_items {
            nav = nav.child(
                self.nav_item(id, icon, self.text(label), false, cx)
                    .opacity(0.45),
            );
        }
        div()
            .w(px(200.))
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
                    .px(px(10.))
                    .py(px(8.))
                    .text_size(px(18.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(self.text(MessageId::AppName)),
            )
            .child(nav)
            .child(div().flex_1())
            .child(
                self.nav_item(
                    "nav-settings",
                    IconName::Settings,
                    self.text(MessageId::Settings),
                    settings_active,
                    cx,
                )
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.main_view = MainView::Settings;
                    cx.notify();
                })),
            )
            .into_any_element()
    }

    fn render_host_card(
        &mut self,
        profile: ConnectionProfile,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        const PALETTE: [u32; 8] = [
            0x00f9_7316,
            0x00ea_b308,
            0x00ef_4444,
            0x003b_82f6,
            0x008b_5cf6,
            0x00ec_4899,
            0x0022_c55e,
            0x0014_b8a6,
        ];
        let hash: usize = profile
            .id
            .0
            .as_bytes()
            .iter()
            .map(|byte| *byte as usize)
            .sum();
        let tile_color = rgb(PALETTE[hash % PALETTE.len()]);
        let initial: String = profile
            .name
            .trim()
            .chars()
            .next()
            .map(|ch| ch.to_uppercase().collect())
            .unwrap_or_else(|| "?".into());
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
        let profile_for_connect = profile.clone();
        let profile_for_edit = profile.clone();
        let profile_for_delete = profile.clone();
        let view = cx.entity();
        let theme = cx.theme();
        let border = theme.border;
        let hover_border = theme.primary;
        let card_bg = theme.secondary;
        let muted_foreground = theme.muted_foreground;
        div()
            .id(SharedString::from(format!("host-card-{}", profile.id.0)))
            .w(px(300.))
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .rounded(px(10.))
            .border_1()
            .border_color(border)
            .bg(card_bg)
            .cursor_pointer()
            .hover(move |style| style.border_color(hover_border))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.connect_profile(profile_for_connect.clone(), window, cx)
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .size(px(36.))
                            .flex_none()
                            .rounded(px(8.))
                            .bg(tile_color)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(rgb(0x00ff_ffff))
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(initial),
                    )
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .whitespace_nowrap()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_sm()
                                    .child(profile.name.clone()),
                            )
                            .child(
                                div()
                                    .whitespace_nowrap()
                                    .text_xs()
                                    .text_color(muted_foreground)
                                    .child(endpoint),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().primary)
                            .child(status.unwrap_or_default()),
                    )
                    .child(
                        h_flex()
                            .gap(px(6.))
                            .child(
                                Button::new(edit_id)
                                    .label(edit_label)
                                    .ghost()
                                    .compact()
                                    .on_click(move |_, window, cx| {
                                        view.update(cx, |this, cx| {
                                            cx.stop_propagation();
                                            this.show_edit_connection(
                                                profile_for_edit.clone(),
                                                window,
                                                cx,
                                            );
                                        });
                                    }),
                            )
                            .child({
                                let view = cx.entity();
                                Button::new(delete_id)
                                    .label(delete_label)
                                    .ghost()
                                    .compact()
                                    .on_click(move |_, _, cx| {
                                        view.update(cx, |this, cx| {
                                            cx.stop_propagation();
                                            this.confirm = Some(ConfirmAction::DeleteProfile(
                                                profile_for_delete.id,
                                            ));
                                            cx.notify();
                                        });
                                    })
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_group_card(&self, host_count: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme();
        let border = theme.border;
        let card_bg = theme.secondary;
        let tile_bg = theme.primary;
        let muted_foreground = theme.muted_foreground;
        div()
            .w(px(300.))
            .p(px(14.))
            .rounded(px(10.))
            .border_1()
            .border_color(border)
            .bg(card_bg)
            .flex()
            .items_center()
            .gap(px(12.))
            .child(
                div()
                    .size(px(36.))
                    .flex_none()
                    .rounded(px(8.))
                    .bg(tile_bg)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(0x00ff_ffff))
                    .child(Icon::new(IconName::Folder).size(px(18.))),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .whitespace_nowrap()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_sm()
                            .child(self.text(MessageId::AllHosts)),
                    )
                    .child(div().text_xs().text_color(muted_foreground).child(format!(
                        "{} {}",
                        host_count,
                        self.text(MessageId::Hosts)
                    ))),
            )
            .into_any_element()
    }

    pub(super) fn render_workspace(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let query = self.search.read(cx).value().to_string();
        let Some(state) = &mut self.state else {
            return div().into_any_element();
        };
        state.set_search_query(query);
        let profiles: Vec<_> = state.filtered_profiles().into_iter().cloned().collect();
        let total_profiles = state.profiles().len();
        let empty_message = if total_profiles == 0 {
            MessageId::NoConnections
        } else {
            MessageId::NoSearchResults
        };
        let is_empty = profiles.is_empty();
        let mut grid = div().flex().flex_wrap().gap(px(12.));
        for profile in profiles {
            grid = grid.child(self.render_host_card(profile, cx));
        }
        let content = if is_empty {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child(self.text(empty_message))
                .into_any_element()
        } else {
            div()
                .id("workspace-hosts")
                .size_full()
                .overflow_y_scroll()
                .child(grid)
                .into_any_element()
        };
        let groups_section = (total_profiles > 0).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(self.text(MessageId::Groups)),
                )
                .child(self.render_group_card(total_profiles, cx))
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(18.))
            .p(px(20.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        Button::new("add-connection")
                            .label(self.text(MessageId::NewHost))
                            .icon(IconName::Plus)
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_add_connection(window, cx)
                            })),
                    )
                    .child(
                        div()
                            .w(px(280.))
                            .child(Input::new(&self.search).prefix(Icon::new(IconName::Search))),
                    ),
            )
            .children(groups_section)
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(self.text(MessageId::Hosts)),
            )
            .child(div().flex_1().child(content))
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

    /// Permanent top tab bar: index 0 is the always-present Workspace tab;
    /// each open connection follows as its own closable tab.
    pub(super) fn render_tab_strip(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let active = self.tabs.active();
        let tab_data: Vec<_> = self
            .tabs
            .tabs()
            .iter()
            .map(|tab| (tab.id(), tab.profile().name.clone(), *tab.state()))
            .collect();
        let theme = cx.theme();
        let dot = |state: &TabState| match state {
            TabState::Connected => theme.primary,
            TabState::Disconnected { .. } => theme.danger,
            TabState::AwaitingHostKey | TabState::AwaitingSecret | TabState::Connecting => {
                theme.muted
            }
        };
        let mut component_tabs = vec![
            Tab::new().w(px(240.)).child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(Icon::new(IconName::LayoutDashboard).size(px(14.)))
                    .child(self.text(MessageId::Workspace)),
            ),
        ];
        for (id, name, state) in &tab_data {
            let id = *id;
            component_tabs.push(
                Tab::new().w(px(240.)).child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            h_flex()
                                .gap_2()
                                .overflow_hidden()
                                .child(div().size(px(8.)).rounded_full().flex_none().bg(dot(state)))
                                .child(SharedString::from(name.clone())),
                        )
                        .child(
                            Button::new(SharedString::from(format!("close-{id:?}")))
                                .label("×")
                                .ghost()
                                .compact()
                                .flex_none()
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.close_tab(id, cx)),
                                ),
                        ),
                ),
            );
        }
        let selected_index = match self.main_view {
            MainView::Sessions if !tab_data.is_empty() => tab_data
                .iter()
                .position(|(id, _, _)| Some(*id) == active)
                .map(|index| index + 1),
            // Sessions with no open connections renders the workspace.
            MainView::Workspace | MainView::Sessions => Some(0),
            MainView::Settings => None,
        };
        let tab_ids: Vec<_> = tab_data.iter().map(|(id, _, _)| *id).collect();
        let mut tab_bar = TabBar::new("tab-strip")
            .segmented()
            .children(component_tabs)
            .on_click(cx.listener(move |this, index, _, cx| {
                if *index == 0 {
                    this.main_view = MainView::Workspace;
                } else if let Some(id) = tab_ids.get(*index - 1) {
                    this.tabs.set_active(*id);
                    this.main_view = MainView::Sessions;
                }
                cx.notify();
            }))
            .suffix(
                Button::new("new-tab")
                    .icon(IconName::Plus)
                    .ghost()
                    .compact()
                    .on_click(
                        cx.listener(|this, _, window, cx| this.show_add_connection(window, cx)),
                    ),
            );
        if let Some(index) = selected_index {
            tab_bar = tab_bar.selected_index(index);
        }
        div()
            .flex_none()
            .px(px(8.))
            .py(px(6.))
            .border_b_1()
            .border_color(cx.theme().border)
            .child(tab_bar)
            .into_any_element()
    }

    pub(super) fn render_sessions(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        self.render_active_tab(cx)
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

    pub(super) fn render_settings(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
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
                        Button::new("back-to-workspace")
                            .label(self.text(MessageId::Workspace))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.main_view = MainView::Workspace;
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

}

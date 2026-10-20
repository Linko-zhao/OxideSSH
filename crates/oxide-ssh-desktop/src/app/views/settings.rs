use super::*;

impl AppView {
    pub(in crate::app) fn render_settings(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let settings = self.state.as_ref().map(|state| state.settings().clone());
        let Some(settings) = settings else {
            return div().into_any_element();
        };
        let category = self.settings_category;

        let mut side_nav = div().flex().flex_col().gap(px(4.));
        let categories = [
            (
                SettingsCategory::General,
                "settings-cat-general",
                IconName::Settings2,
                self.text(MessageId::General),
            ),
            (
                SettingsCategory::Appearance,
                "settings-cat-appearance",
                IconName::Palette,
                self.text(MessageId::Appearance),
            ),
            (
                SettingsCategory::Security,
                "settings-cat-security",
                IconName::CircleCheck,
                self.text(MessageId::Security),
            ),
        ];
        for (item_category, id, icon, label) in categories {
            side_nav = side_nav.child(
                self.nav_item(id, icon, label, category == item_category, cx)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings_category = item_category;
                        cx.notify();
                    })),
            );
        }

        let system_label = self.text(MessageId::System).to_owned();
        let content = match category {
            SettingsCategory::General => {
                let locale_index = match settings.locale {
                    LocaleSetting::System => 0,
                    LocaleSetting::EnUs => 1,
                    LocaleSetting::ZhCn => 2,
                };
                GroupBox::new()
                    .title(self.text(MessageId::Language).to_owned())
                    .child(
                        RadioGroup::horizontal("setting-locale")
                            .selected_index(Some(locale_index))
                            .child(Radio::new("locale-system").label(system_label.clone()))
                            .child(
                                Radio::new("locale-english").label(self.text(MessageId::English)),
                            )
                            .child(
                                Radio::new("locale-chinese")
                                    .label(self.text(MessageId::SimplifiedChinese)),
                            )
                            .on_click(cx.listener(|this, index, window, cx| {
                                let setting = match *index {
                                    1 => LocaleSetting::EnUs,
                                    2 => LocaleSetting::ZhCn,
                                    _ => LocaleSetting::System,
                                };
                                this.set_locale(setting, window, cx);
                            })),
                    )
                    .into_any_element()
            }
            SettingsCategory::Appearance => {
                let theme_index = match settings.theme {
                    ThemeSetting::System => 0,
                    ThemeSetting::Light => 1,
                    ThemeSetting::Dark => 2,
                };
                GroupBox::new()
                    .title(self.text(MessageId::Theme).to_owned())
                    .child(
                        RadioGroup::horizontal("setting-theme")
                            .selected_index(Some(theme_index))
                            .child(Radio::new("theme-system").label(system_label.clone()))
                            .child(Radio::new("theme-light").label(self.text(MessageId::Light)))
                            .child(Radio::new("theme-dark").label(self.text(MessageId::Dark)))
                            .on_click(cx.listener(|this, index, window, cx| {
                                let setting = match *index {
                                    1 => ThemeSetting::Light,
                                    2 => ThemeSetting::Dark,
                                    _ => ThemeSetting::System,
                                };
                                this.set_theme(setting, window, cx);
                            })),
                    )
                    .into_any_element()
            }
            SettingsCategory::Security => {
                let known_hosts = self
                    .state
                    .as_ref()
                    .map(|state| state.known_hosts().to_vec())
                    .unwrap_or_default();
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
                                        this.confirm = Some(ConfirmAction::DeleteHost(
                                            delete_endpoint.clone(),
                                        ));
                                        cx.notify();
                                    });
                                })
                                .into_any_element()
                        }),
                    );
                }
                GroupBox::new()
                    .title(self.text(MessageId::TrustedHosts).to_owned())
                    .child(trusted)
                    .into_any_element()
            }
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                h_flex()
                    .flex_1()
                    .min_h(px(0.))
                    .gap(px(CARD_GAP))
                    .child(
                        div()
                            .w(px(200.))
                            .h_full()
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .p(px(12.))
                            .rounded(cx.theme().radius_lg)
                            .border_1()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().secondary)
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .child(
                                        Button::new("back-to-workspace")
                                            .icon(IconName::ArrowLeft)
                                            .ghost()
                                            .compact()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.main_view = MainView::Workspace;
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        div()
                                            .px(px(10.))
                                            .py(px(8.))
                                            .text_size(px(18.))
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .child(self.text(MessageId::Settings)),
                                    ),
                            )
                            .child(side_nav),
                    )
                    .child(
                        div()
                            .id("settings-scroll")
                            .flex_1()
                            .h_full()
                            .overflow_y_scroll()
                            .p(px(CARD_PADDING))
                            .child(content),
                    ),
            )
            .into_any_element()
    }
}

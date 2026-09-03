use super::*;

impl AppView {
    pub(in crate::app) fn render_settings(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
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

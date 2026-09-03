use super::*;

impl AppView {
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

    pub(in crate::app) fn render_sidebar(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
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
}

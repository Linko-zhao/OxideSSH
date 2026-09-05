use super::*;

impl AppView {
    /// Pill-shaped tab base: the selected pill lifts one step above the
    /// strip; hovering an unselected pill shows a fainter step of the same
    /// color, so both states read consistently in light and dark mode.
    fn tab_pill(id: impl Into<ElementId>, selected: bool, cx: &App) -> gpui::Stateful<gpui::Div> {
        let theme = cx.theme();
        let base = h_flex()
            .id(id)
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .py(px(5.))
            .rounded(theme.radius)
            .text_sm()
            .cursor_pointer();
        if selected {
            base.bg(theme.secondary).text_color(theme.foreground)
        } else {
            let hover_bg = theme.secondary.opacity(0.55);
            let hover_fg = theme.foreground;
            base.text_color(theme.muted_foreground)
                .hover(move |style| style.bg(hover_bg).text_color(hover_fg))
        }
    }

    /// Permanent top tab strip: the always-present Workspace pill comes
    /// first; each open connection follows as its own closable pill.
    pub(in crate::app) fn render_tab_strip(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
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
        let workspace_selected = match self.main_view {
            MainView::Workspace => true,
            // Sessions with no open connections renders the workspace.
            MainView::Sessions => tab_data.is_empty(),
            MainView::Settings => false,
        };
        let mut strip = h_flex().items_center().gap(px(4.)).child(
            Self::tab_pill("tab-workspace", workspace_selected, cx)
                .child(Icon::new(IconName::LayoutDashboard).size(px(14.)))
                .child(self.text(MessageId::Workspace))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.main_view = MainView::Workspace;
                    cx.notify();
                })),
        );
        for (id, name, state) in &tab_data {
            let tab_id = *id;
            let selected = self.main_view == MainView::Sessions && active == Some(tab_id);
            strip = strip.child(
                Self::tab_pill(SharedString::from(format!("tab-{tab_id:?}")), selected, cx)
                    .child(div().size(px(8.)).rounded_full().flex_none().bg(dot(state)))
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .max_w(px(180.))
                            .child(SharedString::from(name.clone())),
                    )
                    .child(
                        Button::new(SharedString::from(format!("close-{tab_id:?}")))
                            .icon(IconName::Close)
                            .ghost()
                            .compact()
                            .flex_none()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_tab(tab_id, cx);
                            })),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tabs.set_active(tab_id);
                        this.main_view = MainView::Sessions;
                        cx.notify();
                    })),
            );
        }
        div()
            .flex_none()
            .px(px(8.))
            .py(px(6.))
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                strip.child(
                    Button::new("new-tab")
                        .icon(IconName::Plus)
                        .ghost()
                        .compact()
                        .on_click(
                            cx.listener(|this, _, window, cx| this.show_add_connection(window, cx)),
                        ),
                ),
            )
            .into_any_element()
    }
}

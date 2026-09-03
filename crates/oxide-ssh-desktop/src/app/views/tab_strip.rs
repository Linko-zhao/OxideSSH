use super::*;

impl AppView {
    /// Permanent top tab bar: index 0 is the always-present Workspace tab;
    /// each open connection follows as its own closable tab.
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
}

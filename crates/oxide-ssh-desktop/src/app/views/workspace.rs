use super::*;

impl AppView {
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

    pub(in crate::app) fn render_workspace(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
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
}

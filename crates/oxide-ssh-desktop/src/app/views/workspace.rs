use super::*;

impl AppView {
    fn render_host_card(
        &mut self,
        profile: ConnectionProfile,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // Muted avatar hues that sit comfortably on both light and dark
        // cards; keyed by profile id so a host keeps its color.
        const PALETTE: [u32; 8] = [
            0x006b_8cae,
            0x007b_a389,
            0x00b5_936b,
            0x009a_7bb5,
            0x005f_9ea8,
            0x00b5_7e7e,
            0x007e_93b8,
            0x00a8_927c,
        ];
        let hash: usize = profile.id.0.as_bytes().iter().fold(0usize, |acc, byte| {
            acc.wrapping_mul(31).wrapping_add(*byte as usize)
        });
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
        let edit_id = SharedString::from(format!("edit-{}", profile.id.0));
        let delete_id = SharedString::from(format!("delete-{}", profile.id.0));
        let profile_for_connect = profile.clone();
        let profile_for_edit = profile.clone();
        let profile_for_delete = profile.clone();
        let view = cx.entity();
        let theme = cx.theme();
        let border = theme.border;
        let card_bg = theme.secondary;
        // Hover lifts the card one step above its resting background.
        let hover_bg = theme.accent;
        let radius_lg = theme.radius_lg;
        let muted_foreground = theme.muted_foreground;
        let status_color = theme.primary;
        div()
            .id(SharedString::from(format!("host-card-{}", profile.id.0)))
            .group("host-card")
            .w(px(300.))
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .rounded(radius_lg)
            .border_1()
            .border_color(border)
            .bg(card_bg)
            .cursor_pointer()
            .hover(move |style| style.bg(hover_bg))
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
                            .rounded_full()
                            .bg(tile_color)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(rgb(0x00f5_f6f8))
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
                    )
                    .child(
                        h_flex()
                            .gap(px(2.))
                            .opacity(0.)
                            .group_hover("host-card", |style| style.opacity(1.))
                            .child(
                                Button::new(edit_id)
                                    .icon(IconName::Settings2)
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
                                    .icon(IconName::Delete)
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
            .when_some(status, |card, status| {
                card.child(div().text_xs().text_color(status_color).child(status))
            })
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
        // Groups are display-only for now (see CONTEXT.md): render them as a
        // filter chip strip above the hosts grid instead of full-size cards.
        let filter_chips = (total_profiles > 0).then(|| {
            let theme = cx.theme();
            h_flex().gap(px(8.)).child(
                h_flex()
                    .id("filter-all-hosts")
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(5.))
                    .rounded_full()
                    .bg(theme.secondary)
                    .border_1()
                    .border_color(theme.border)
                    .text_sm()
                    .child(self.text(MessageId::AllHosts))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("{total_profiles}")),
                    ),
            )
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
            .children(filter_chips)
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

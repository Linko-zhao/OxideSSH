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
            // py(3) keeps the pill compact; the title bar adds the
            // client-wide CARD_GAP margin around it.
            .py(px(3.))
            .border_1()
            .border_color(theme.border)
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
    #[cfg_attr(not(target_os = "windows"), allow(unused_variables))]
    pub(in crate::app) fn render_tab_strip(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let active = self.tabs.active();
        let tab_data: Vec<_> = self
            .tabs
            .tabs()
            .iter()
            .map(|tab| {
                let title = match tab.kind() {
                    TabKind::Ssh(profile) => profile.name.clone(),
                    TabKind::Local => self.text(MessageId::LocalTerminal).to_string(),
                };
                (tab.id(), title, *tab.state())
            })
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
        // Fixed separator between the permanent workspace pill and the
        // session area (connection pills + new-tab button).
        strip = strip.child(
            div()
                .text_color(theme.muted_foreground.opacity(0.6))
                .text_sm()
                .child("|"),
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
                        // xsmall (20px) keeps the pill slim; the default
                        // medium icon button is 32px and would inflate both
                        // the pill and the title bar wrapping it.
                        Button::new(SharedString::from(format!("close-{tab_id:?}")))
                            .icon(IconName::Close)
                            .ghost()
                            .compact()
                            .xsmall()
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
        let strip = strip.child(
            Button::new("new-tab")
                .icon(IconName::Plus)
                .ghost()
                .compact()
                .xsmall()
                .on_click(cx.listener(|this, _, window, cx| this.show_add_connection(window, cx))),
        );
        // Linux keeps the native title bar, so the strip stays a standalone row.
        #[cfg(target_os = "linux")]
        {
            div()
                .flex_none()
                .px(px(CARD_GAP))
                .py(px(CARD_GAP))
                .border_b_1()
                .border_color(cx.theme().border)
                .child(strip)
                .into_any_element()
        }
        // macOS: the strip merges into the custom-drawn title bar; TitleBar
        // renders the native window controls itself. The padding both matches
        // the CARD_GAP margins around the content below and gives the traffic
        // lights enough height to avoid clipping.
        #[cfg(target_os = "macos")]
        {
            TitleBar::new()
                .child(div().py(px(CARD_GAP)).child(strip))
                .into_any_element()
        }
        // Windows: route caption interactions through gpui's native
        // WM_NCHITTEST path (`window_control_area`): the spacer reports
        // HTCAPTION (native move, drag-to-restore-when-maximized,
        // double-click toggle, snap layouts) and the caption buttons report
        // HTMINBUTTON/HTMAXBUTTON/HTCLOSE, with gpui's nc_button_pressed
        // handling performing minimize / maximize-toggle / close. Close
        // funnels through WM_CLOSE → `on_window_should_close`, keeping the
        // quit confirmation. The event-driven alternatives are dead ends in
        // gpui 0.2.2: `start_window_move()` is a no-op outside Wayland/X11
        // and `zoom_window()` only ever maximizes (SW_MAXIMIZE).
        //
        // Every control hitbox MUST `occlude()`: the NC mouse down is also
        // forwarded into the app, and the root view's focus-transfer
        // listener would otherwise call `prevent_default()`, making gpui
        // treat the click as app-handled and skip the native caption
        // behavior entirely.
        #[cfg(target_os = "windows")]
        {
            let theme = cx.theme();
            let (fg, hover_bg, danger, danger_fg) = (
                theme.foreground,
                theme.secondary_hover,
                theme.danger,
                theme.danger_foreground,
            );
            let caption =
                move |id: &'static str, icon: IconName, area: WindowControlArea, is_close: bool| {
                    div()
                        .id(id)
                        .window_control_area(area)
                        .occlude()
                        .flex_none()
                        .flex()
                        .w(px(34.))
                        .h_full()
                        .items_center()
                        .justify_center()
                        .text_color(fg)
                        .hover(move |s| {
                            if is_close {
                                s.bg(danger).text_color(danger_fg)
                            } else {
                                s.bg(hover_bg)
                            }
                        })
                        .child(Icon::new(icon).size(px(14.)))
                };
            let max_icon = if window.is_maximized() {
                IconName::WindowRestore
            } else {
                IconName::WindowMaximize
            };
            h_flex()
                .w_full()
                // Height derives from the pill plus the client-wide CARD_GAP
                // margin, so the tabs' top gap matches the content padding.
                .py(px(CARD_GAP))
                .flex_none()
                .items_center()
                .pl(px(CARD_GAP))
                .border_b_1()
                .border_color(theme.border)
                .child(strip)
                .child(
                    div()
                        .id("titlebar-drag")
                        .window_control_area(WindowControlArea::Drag)
                        .occlude()
                        .flex_1()
                        .h_full(),
                )
                .child(caption(
                    "win-min",
                    IconName::WindowMinimize,
                    WindowControlArea::Min,
                    false,
                ))
                .child(caption("win-max", max_icon, WindowControlArea::Max, false))
                .child(caption(
                    "win-close",
                    IconName::WindowClose,
                    WindowControlArea::Close,
                    true,
                ))
                .into_any_element()
        }
    }
}

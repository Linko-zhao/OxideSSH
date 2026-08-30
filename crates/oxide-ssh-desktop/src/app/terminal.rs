use super::messages::local_error_message_id;
use super::*;

#[cfg(target_os = "macos")]
pub(super) const TERMINAL_FONT: &str = "Menlo";

#[cfg(target_os = "windows")]
pub(super) const TERMINAL_FONT: &str = "Cascadia Mono";

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
pub(super) const TERMINAL_FONT: &str = "monospace";

#[derive(Clone, Copy)]
pub(super) struct TerminalGeometry {
    bounds: Bounds<Pixels>,
    cell_width: Pixels,
    cell_height: Pixels,
    columns: usize,
    rows: usize,
}

pub(super) struct TerminalElement {
    pub(super) view: Entity<AppView>,
    pub(super) tab_id: TabId,
}

pub(super) struct TerminalPrepaint {
    backgrounds: Vec<PaintQuad>,
    cursor: Vec<PaintQuad>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct RunKey {
    style: CellRenderStyle,
    selected: bool,
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = TerminalPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let text_style = window.text_style();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let base_font = text_style.font();
        let sample_run = TextRun {
            len: 1,
            font: base_font.clone(),
            color: text_style.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let sample = window
            .text_system()
            .shape_line("M".into(), font_size, &[sample_run], None);
        let cell_width = sample.x_for_index(1).max(px(1.));
        let cell_height = window.line_height().max(px(1.));
        let columns = (f32::from(bounds.size.width) / f32::from(cell_width)).floor() as usize;
        let rows = (f32::from(bounds.size.height) / f32::from(cell_height)).floor() as usize;
        let geometry = TerminalGeometry {
            bounds,
            cell_width,
            cell_height,
            columns: columns.max(2),
            rows: rows.max(1),
        };
        self.view.update(cx, |view, cx| {
            view.update_terminal_geometry(self.tab_id, geometry, cx)
        });

        let view = self.view.read(cx);
        let Some(tab) = view.tabs.tab(self.tab_id) else {
            return TerminalPrepaint {
                backgrounds: Vec::new(),
                cursor: Vec::new(),
                lines: Vec::new(),
            };
        };
        let model = tab.terminal();
        let terminal_colors = view.theme.terminal_colors();
        let selection_color = terminal_selection_color(view.theme);
        let content = model.renderable_content();
        let selection = content.selection;
        let cursor = content.cursor;
        let display_offset = content.display_offset as i32;

        let mut row_text = Vec::with_capacity(geometry.rows);
        let mut row_runs = Vec::with_capacity(geometry.rows);
        let mut row_keys = Vec::with_capacity(geometry.rows);
        for _ in 0..geometry.rows {
            row_text.push(String::with_capacity(geometry.columns));
            row_runs.push(Vec::<TextRun>::new());
            row_keys.push(None::<RunKey>);
        }

        let mut backgrounds = Vec::with_capacity(geometry.rows * 4);
        let mut background_segment: Option<(usize, usize, usize, u32)> = None;
        let default_background = terminal_colors.background.to_hex();
        for indexed in content.display_iter {
            let Ok(row) = usize::try_from(indexed.point.line.0 + display_offset) else {
                continue;
            };
            let column = indexed.point.column.0;
            if row >= geometry.rows || column >= geometry.columns {
                continue;
            }
            let style = model.cell_render_style(indexed.cell);
            let selected = selection.is_some_and(|range| range.contains(indexed.point));
            let background = if selected {
                selection_color
            } else {
                style.background.to_hex()
            };
            if selected || background != default_background {
                match background_segment {
                    Some((segment_row, start, end, color))
                        if segment_row == row && end == column && color == background =>
                    {
                        background_segment = Some((segment_row, start, column + 1, color));
                    }
                    Some(segment) => {
                        push_terminal_background(
                            &mut backgrounds,
                            segment,
                            bounds,
                            cell_width,
                            cell_height,
                        );
                        background_segment = Some((row, column, column + 1, background));
                    }
                    None => {
                        background_segment = Some((row, column, column + 1, background));
                    }
                }
            } else if let Some(segment) = background_segment.take() {
                push_terminal_background(
                    &mut backgrounds,
                    segment,
                    bounds,
                    cell_width,
                    cell_height,
                );
            }

            if style.wide_spacer || style.hidden {
                continue;
            }
            let text = &mut row_text[row];
            let before = text.len();
            text.push(indexed.cell.c);
            if let Some(zerowidth) = indexed.cell.zerowidth() {
                text.extend(zerowidth);
            }
            let byte_len = text.len() - before;
            let key = RunKey { style, selected };
            if row_keys[row] == Some(key) {
                if let Some(run) = row_runs[row].last_mut() {
                    run.len += byte_len;
                }
            } else {
                row_keys[row] = Some(key);
                let mut font = base_font.clone();
                if style.bold {
                    font = font.bold();
                }
                if style.italic {
                    font = font.italic();
                }
                let foreground = if selected {
                    terminal_colors.foreground.to_hex()
                } else {
                    style.foreground.to_hex()
                };
                row_runs[row].push(TextRun {
                    len: byte_len,
                    font,
                    color: rgb(foreground).into(),
                    background_color: None,
                    underline: style.underline.then_some(UnderlineStyle {
                        thickness: px(1.),
                        color: None,
                        wavy: false,
                    }),
                    strikethrough: style.strikeout.then_some(StrikethroughStyle {
                        thickness: px(1.),
                        color: None,
                    }),
                });
            }
        }
        if let Some(segment) = background_segment {
            push_terminal_background(&mut backgrounds, segment, bounds, cell_width, cell_height);
        }

        let mut lines = Vec::with_capacity(geometry.rows);
        for (row, (text, runs)) in row_text.into_iter().zip(row_runs).enumerate() {
            let line = window
                .text_system()
                .shape_line(text.into(), font_size, &runs, None);
            lines.push((
                point(bounds.left(), bounds.top() + cell_height * row as f32),
                line,
            ));
        }

        let cursor_row = usize::try_from(cursor.point.line.0 + display_offset).ok();
        let cursor = if cursor_row.is_none_or(|row| row >= geometry.rows)
            || cursor.point.column.0 >= geometry.columns
        {
            Vec::new()
        } else {
            let cursor_color = rgb(terminal_colors.cursor.to_hex());
            let cursor_x = bounds.left() + cell_width * cursor.point.column.0 as f32;
            let cursor_y = bounds.top() + cell_height * cursor_row.unwrap_or_default() as f32;
            match cursor.shape {
                alacritty_terminal::vte::ansi::CursorShape::Hidden => Vec::new(),
                alacritty_terminal::vte::ansi::CursorShape::Beam => vec![fill(
                    Bounds::new(point(cursor_x, cursor_y), size(px(2.), cell_height)),
                    cursor_color,
                )],
                alacritty_terminal::vte::ansi::CursorShape::Underline => vec![fill(
                    Bounds::new(
                        point(cursor_x, cursor_y + cell_height - px(2.)),
                        size(cell_width, px(2.)),
                    ),
                    cursor_color,
                )],
                alacritty_terminal::vte::ansi::CursorShape::HollowBlock => {
                    terminal_outline(cursor_x, cursor_y, cell_width, cell_height, cursor_color)
                }
                alacritty_terminal::vte::ansi::CursorShape::Block => vec![fill(
                    Bounds::new(point(cursor_x, cursor_y), size(cell_width, cell_height)),
                    gpui::Rgba {
                        a: 0.45,
                        ..cursor_color
                    },
                )],
            }
        };

        TerminalPrepaint {
            backgrounds,
            cursor,
            lines,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let view = self.view.clone();
        let focus = view.read(cx).terminal_focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, view), cx);
        for quad in prepaint.backgrounds.drain(..) {
            window.paint_quad(quad);
        }
        for quad in prepaint.cursor.drain(..) {
            window.paint_quad(quad);
        }
        for (origin, line) in prepaint.lines.drain(..) {
            let _ = line.paint(origin, window.line_height(), window, cx);
        }
    }
}

fn push_terminal_background(
    backgrounds: &mut Vec<PaintQuad>,
    (row, start, end, color): (usize, usize, usize, u32),
    bounds: Bounds<Pixels>,
    cell_width: Pixels,
    cell_height: Pixels,
) {
    backgrounds.push(fill(
        Bounds::new(
            point(
                bounds.left() + cell_width * start as f32,
                bounds.top() + cell_height * row as f32,
            ),
            size(cell_width * (end - start) as f32, cell_height),
        ),
        rgb(color),
    ));
}

fn terminal_outline(
    x: Pixels,
    y: Pixels,
    width: Pixels,
    height: Pixels,
    color: gpui::Rgba,
) -> Vec<PaintQuad> {
    let stroke = px(1.);
    vec![
        fill(Bounds::new(point(x, y), size(width, stroke)), color),
        fill(
            Bounds::new(point(x, y + height - stroke), size(width, stroke)),
            color,
        ),
        fill(Bounds::new(point(x, y), size(stroke, height)), color),
        fill(
            Bounds::new(point(x + width - stroke, y), size(stroke, height)),
            color,
        ),
    ]
}

impl EntityInputHandler for AppView {
    fn text_for_range(
        &mut self,
        _range: std::ops::Range<usize>,
        _adjusted_range: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::UTF16Selection> {
        None
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        // A non-None range signals the platform that an IME composition is in
        // progress, which gates further keystrokes into the input context.
        self.composing.then_some(0..0)
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.composing = false;
        self.compose_tab = None;
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // IME final commit (or WM_CHAR / insertText passthrough): forward the
        // text to the tab that owns the composition, falling back to the
        // active tab.
        let target = self.compose_tab.or_else(|| self.tabs.active());
        let result = target
            .and_then(|id| self.tabs.tab_mut(id))
            .map(|tab| tab.send_text(text));
        self.composing = false;
        self.compose_tab = None;
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

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        _new_text: &str,
        _new_selected_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // Composition update: remember the owning tab so the final commit is
        // delivered to the session that started composing, even if the user
        // switches tabs mid-composition.
        if !self.composing {
            self.compose_tab = self.tabs.active();
        }
        self.composing = true;
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: std::ops::Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        None
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

/// Selection tint for the custom terminal renderer, independent of the
/// component widget theme.
fn terminal_selection_color(theme: ResolvedTheme) -> u32 {
    match theme {
        ResolvedTheme::Dark => 0x315a78,
        ResolvedTheme::Light => 0xb8d8f0,
    }
}

pub(super) trait RgbColorExt {
    fn to_hex(self) -> u32;
}

impl RgbColorExt for oxide_ssh_terminal::RgbColor {
    fn to_hex(self) -> u32 {
        ((self.red as u32) << 16) | ((self.green as u32) << 8) | self.blue as u32
    }
}

impl AppView {
    pub(super) fn copy_terminal_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self
            .tabs
            .active()
            .and_then(|id| self.tabs.tab(id))
            .and_then(|tab| tab.terminal().selected_text())
        else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    pub(super) fn paste_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let result = self
            .tabs
            .active()
            .and_then(|id| self.tabs.tab_mut(id))
            .map(|tab| tab.paste(&text));
        if let Some(Err(error)) = result {
            self.status_message = Some(local_error_message_id(error));
        }
        cx.notify();
    }

    pub(super) fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        #[cfg(target_os = "macos")]
        if keystroke.modifiers.platform {
            // Command shortcuts are handled by the global action bindings.
            return;
        }
        #[cfg(not(target_os = "linux"))]
        let plain_text = keystroke.key_char.is_some()
            && !keystroke.modifiers.control
            && !keystroke.modifiers.alt;
        #[cfg(not(target_os = "linux"))]
        if plain_text {
            // On Windows and macOS, unmodified printable keys are translated
            // by the platform (WM_CHAR / insertText) and arrive at the
            // terminal's input handler, which forwards them to the session.
            // Letting them propagate is what feeds the IME its first
            // composition keystroke.
            return;
        }
        let key = match keystroke.key.as_str() {
            "enter" => Key::Enter,
            "backspace" => Key::Backspace,
            "tab" => Key::Tab,
            "escape" => Key::Escape,
            "up" => Key::ArrowUp,
            "down" => Key::ArrowDown,
            "right" => Key::ArrowRight,
            "left" => Key::ArrowLeft,
            "home" => Key::Home,
            "end" => Key::End,
            "insert" => Key::Insert,
            "delete" => Key::Delete,
            "pageup" => Key::PageUp,
            "pagedown" => Key::PageDown,
            key if key.starts_with('f') => {
                let Some(function) = key.get(1..).and_then(|number| number.parse::<u8>().ok())
                else {
                    return;
                };
                Key::Function(function)
            }
            _ => {
                let Some(text) = keystroke.key_char.as_deref() else {
                    return;
                };
                Key::Text(text)
            }
        };
        let mut input = KeyInput::new(key);
        if keystroke.modifiers.control {
            input = input.control();
        }
        if keystroke.modifiers.alt {
            input = input.alt();
        }
        if keystroke.modifiers.shift {
            input = input.shift();
        }
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
        cx.stop_propagation();
        cx.notify();
    }

    fn update_terminal_geometry(
        &mut self,
        tab_id: TabId,
        geometry: TerminalGeometry,
        cx: &mut Context<Self>,
    ) {
        self.terminal_geometry = Some(geometry);
        let size = TerminalSize {
            columns: geometry.columns,
            rows: geometry.rows,
            pixel_width: u32::from(geometry.bounds.size.width),
            pixel_height: u32::from(geometry.bounds.size.height),
        };
        let Some(tab) = self.tabs.tab_mut(tab_id) else {
            return;
        };
        if tab.terminal().size() != size {
            if tab.resize(size).is_err() {
                self.status_message = Some(MessageId::InvalidProfile);
            }
            cx.notify();
        }
    }

    fn terminal_cell_at(&self, position: Point<Pixels>) -> Option<(usize, usize, CellSide)> {
        let geometry = self.terminal_geometry?;
        if position.x < geometry.bounds.left()
            || position.x >= geometry.bounds.right()
            || position.y < geometry.bounds.top()
            || position.y >= geometry.bounds.bottom()
        {
            return None;
        }
        let local_x = position.x - geometry.bounds.left();
        let local_y = position.y - geometry.bounds.top();
        let column = (f32::from(local_x) / f32::from(geometry.cell_width)).floor() as usize;
        let row = (f32::from(local_y) / f32::from(geometry.cell_height)).floor() as usize;
        let cell_x = local_x - geometry.cell_width * column as f32;
        let side = if cell_x < geometry.cell_width * 0.5 {
            CellSide::Left
        } else {
            CellSide::Right
        };
        Some((
            row.min(geometry.rows.saturating_sub(1)),
            column.min(geometry.columns.saturating_sub(1)),
            side,
        ))
    }

    pub(super) fn terminal_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal_focus.focus(window);
        let Some((row, column, side)) = self.terminal_cell_at(event.position) else {
            return;
        };
        if let Some(tab) = self.tabs.active().and_then(|id| self.tabs.tab_mut(id)) {
            tab.terminal_mut().start_selection(row, column, side);
            self.terminal_selecting = true;
            cx.notify();
        }
    }

    pub(super) fn terminal_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.terminal_selecting || !event.dragging() {
            return;
        }
        let Some((row, column, side)) = self.terminal_cell_at(event.position) else {
            return;
        };
        if let Some(tab) = self.tabs.active().and_then(|id| self.tabs.tab_mut(id)) {
            tab.terminal_mut().update_selection(row, column, side);
            cx.notify();
        }
    }

    pub(super) fn terminal_mouse_up(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.terminal_selecting = false;
    }

    pub(super) fn terminal_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(geometry) = self.terminal_geometry else {
            return;
        };
        let delta = event.delta.pixel_delta(geometry.cell_height);
        self.scroll_accumulator += f32::from(delta.y) / f32::from(geometry.cell_height);
        let lines = self.scroll_accumulator.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_accumulator -= lines as f32;
        if let Some(tab) = self.tabs.active().and_then(|id| self.tabs.tab_mut(id)) {
            tab.terminal_mut().scroll_display(lines);
            cx.notify();
        }
    }
}

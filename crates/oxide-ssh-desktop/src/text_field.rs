//! Desktop single-line text input built from GPUI 0.2.2's official input example.
//! Source: https://docs.rs/crate/gpui/0.2.2/source/examples/input.rs

use std::{borrow::Cow, ops::Range};

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId, Hsla, InspectorElementId,
    IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PaintQuad, Pixels, Point, ShapedLine, SharedString, Style, TextRun, UTF16Selection,
    UnderlineStyle, Window, actions, div, fill, hsla, point, prelude::*, px, relative, size,
};
use unicode_segmentation::UnicodeSegmentation;

actions!(
    text_field,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        SelectHome,
        SelectEnd,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
    ]
);

/// Registers the keyboard actions used by every [`TextField`].
///
/// Call this once while initializing the desktop application.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("TextField")),
        KeyBinding::new("delete", Delete, Some("TextField")),
        KeyBinding::new("left", Left, Some("TextField")),
        KeyBinding::new("right", Right, Some("TextField")),
        KeyBinding::new("shift-left", SelectLeft, Some("TextField")),
        KeyBinding::new("shift-right", SelectRight, Some("TextField")),
        KeyBinding::new("home", Home, Some("TextField")),
        KeyBinding::new("end", End, Some("TextField")),
        KeyBinding::new("shift-home", SelectHome, Some("TextField")),
        KeyBinding::new("shift-end", SelectEnd, Some("TextField")),
        KeyBinding::new("cmd-a", SelectAll, Some("TextField")),
        KeyBinding::new("cmd-c", Copy, Some("TextField")),
        KeyBinding::new("cmd-x", Cut, Some("TextField")),
        KeyBinding::new("cmd-v", Paste, Some("TextField")),
        KeyBinding::new("ctrl-a", SelectAll, Some("TextField")),
        KeyBinding::new("ctrl-c", Copy, Some("TextField")),
        KeyBinding::new("ctrl-x", Cut, Some("TextField")),
        KeyBinding::new("ctrl-v", Paste, Some("TextField")),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, Some("TextField")),
    ]);
}

/// Colors used to draw a [`TextField`].
#[derive(Clone, Copy, PartialEq)]
pub struct TextFieldAppearance {
    pub background: Hsla,
    pub text: Hsla,
    pub placeholder: Hsla,
    pub border: Hsla,
    pub focus_border: Hsla,
    pub selection: Hsla,
    pub cursor: Hsla,
}

impl Default for TextFieldAppearance {
    fn default() -> Self {
        Self {
            background: hsla(0.0, 0.0, 0.99, 1.0),
            text: hsla(0.62, 0.08, 0.14, 1.0),
            placeholder: hsla(0.61, 0.06, 0.45, 1.0),
            border: hsla(0.62, 0.07, 0.82, 1.0),
            focus_border: hsla(0.08, 0.74, 0.50, 1.0),
            selection: hsla(0.57, 0.55, 0.65, 0.45),
            cursor: hsla(0.08, 0.74, 0.50, 1.0),
        }
    }
}

/// An independent, single-line GPUI text input.
///
/// Masking only changes the shaped display string. The value retained by this
/// entity and returned by [`Self::value`] is never altered by masking.
pub struct TextField {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    masked: bool,
    appearance: TextFieldAppearance,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    last_mapping: Option<DisplayMapping>,
    last_scroll_offset: Pixels,
    is_selecting: bool,
}

impl TextField {
    pub fn new(
        _window: &mut Window,
        cx: &mut Context<Self>,
        placeholder: impl Into<SharedString>,
        value: impl Into<SharedString>,
        masked: bool,
    ) -> Self {
        let content = sanitize_shared_string(value.into());
        let cursor = content.len();
        Self {
            focus_handle: cx.focus_handle().tab_stop(true),
            content,
            placeholder: sanitize_shared_string(placeholder.into()),
            selected_range: cursor..cursor,
            selection_reversed: false,
            marked_range: None,
            masked,
            appearance: TextFieldAppearance::default(),
            last_layout: None,
            last_bounds: None,
            last_mapping: None,
            last_scroll_offset: px(0.0),
            is_selecting: false,
        }
    }

    pub fn value(&self) -> &str {
        &self.content
    }

    pub fn set_value(&mut self, value: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = sanitize_shared_string(value.into());
        let cursor = self.content.len();
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.marked_range = None;
        self.clear_layout_cache();
        cx.notify();
    }

    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let placeholder = sanitize_shared_string(placeholder.into());
        if self.placeholder != placeholder {
            self.placeholder = placeholder;
            self.clear_layout_cache();
            cx.notify();
        }
    }

    pub fn set_masked(&mut self, masked: bool, cx: &mut Context<Self>) {
        if self.masked != masked {
            self.masked = masked;
            self.clear_layout_cache();
            cx.notify();
        }
    }

    pub fn set_appearance(&mut self, appearance: TextFieldAppearance, cx: &mut Context<Self>) {
        if self.appearance != appearance {
            self.appearance = appearance;
            self.clear_layout_cache();
            cx.notify();
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    fn clear_layout_cache(&mut self) {
        self.last_layout = None;
        self.last_bounds = None;
        self.last_mapping = None;
        self.last_scroll_offset = px(0.0);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx);
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.end, cx);
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        cx.notify();
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(0, cx);
    }

    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_owned(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_owned(),
            ));
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        self.is_selecting = true;
        let offset = self.index_for_mouse_position(event.position);
        if event.modifiers.shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            let offset = self.index_for_mouse_position(event.position);
            self.select_to(offset, cx);
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = self.nearest_grapheme_boundary(offset.min(self.content.len()));
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = self.nearest_grapheme_boundary(offset.min(self.content.len()));
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.marked_range = None;
        cx.notify();
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(line), Some(mapping)) = (
            self.last_bounds.as_ref(),
            self.last_layout.as_ref(),
            self.last_mapping.as_ref(),
        ) else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        let display_index =
            line.closest_index_for_x(position.x - bounds.left() + self.last_scroll_offset);
        self.nearest_grapheme_boundary(
            mapping
                .display_to_content(display_index)
                .min(self.content.len()),
        )
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(index, _)| (index < offset).then_some(index))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(index, _)| (index > offset).then_some(index))
            .unwrap_or(self.content.len())
    }

    fn nearest_grapheme_boundary(&self, offset: usize) -> usize {
        if offset == 0 || self.content.is_empty() {
            return 0;
        }
        if offset >= self.content.len() {
            return self.content.len();
        }

        let mut previous = 0;
        for (index, _) in self.content.grapheme_indices(true) {
            if index == offset {
                return index;
            }
            if index > offset {
                return if offset - previous <= index - offset {
                    previous
                } else {
                    index
                };
            }
            previous = index;
        }

        if offset - previous <= self.content.len() - offset {
            previous
        } else {
            self.content.len()
        }
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        range_from_utf16(&self.content, range)
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        range_to_utf16(&self.content, range)
    }

    fn replacement_range(&self, range_utf16: Option<&Range<usize>>) -> Range<usize> {
        range_utf16
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone())
    }

    fn replace_content(&mut self, range: Range<usize>, new_text: &str) -> Range<usize> {
        let mut replacement =
            String::with_capacity(self.content.len() - (range.end - range.start) + new_text.len());
        replacement.push_str(&self.content[..range.start]);
        replacement.push_str(new_text);
        replacement.push_str(&self.content[range.end..]);
        let inserted_range = range.start..range.start + new_text.len();
        self.content = replacement.into();
        inserted_range
    }
}

impl EntityInputHandler for TextField {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked_range.take().is_some() {
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.replacement_range(range_utf16.as_ref());
        let new_text = sanitize_text(new_text);
        let inserted_range = self.replace_content(range, &new_text);
        self.selected_range = inserted_range.end..inserted_range.end;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.replacement_range(range_utf16.as_ref());
        let new_text = sanitize_text(new_text);
        let relative_selection = new_selected_range_utf16
            .as_ref()
            .map(|range| range_from_utf16(&new_text, range));
        let inserted_range = self.replace_content(range, &new_text);

        self.marked_range = (!inserted_range.is_empty()).then_some(inserted_range.clone());
        self.selected_range = relative_selection
            .map(|range| inserted_range.start + range.start..inserted_range.start + range.end)
            .unwrap_or_else(|| inserted_range.end..inserted_range.end);
        self.selection_reversed = false;
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.last_layout.as_ref()?;
        let mapping = self.last_mapping.as_ref()?;
        let bounds = self.last_bounds.unwrap_or(element_bounds);
        let range = self.range_from_utf16(&range_utf16);
        let start = mapping.content_to_display(range.start);
        let end = mapping.content_to_display(range.end);
        Some(Bounds::from_corners(
            point(
                bounds.left() + line.x_for_index(start) - self.last_scroll_offset,
                bounds.top(),
            ),
            point(
                bounds.left() + line.x_for_index(end) - self.last_scroll_offset,
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let line = self.last_layout.as_ref()?;
        let mapping = self.last_mapping.as_ref()?;
        let local = bounds.localize(&point)?;
        let display_index = line.closest_index_for_x(local.x + self.last_scroll_offset);
        let content_index = self.nearest_grapheme_boundary(
            mapping
                .display_to_content(display_index)
                .min(self.content.len()),
        );
        Some(offset_to_utf16(&self.content, content_index))
    }
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border = if self.focus_handle.is_focused(window) {
            self.appearance.focus_border
        } else {
            self.appearance.border
        };

        div()
            .key_context("TextField")
            .track_focus(&self.focus_handle)
            .flex()
            .items_center()
            .w_full()
            .h(px(36.0))
            .px(px(10.0))
            .overflow_hidden()
            .rounded(px(6.0))
            .border_1()
            .border_color(border)
            .bg(self.appearance.background)
            .text_color(self.appearance.text)
            .text_size(px(14.0))
            .line_height(px(20.0))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(TextFieldElement { field: cx.entity() })
    }
}

impl Focusable for TextField {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

struct TextFieldElement {
    field: Entity<TextField>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    mapping: Option<DisplayMapping>,
    scroll_offset: Pixels,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextFieldElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextFieldElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

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
        style.size.width = relative(1.0).into();
        style.size.height = window.line_height().into();
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
        let field = self.field.read(cx);
        let mapping = DisplayMapping::new(&field.content, field.masked);
        let display_text = mapping.display_text(&field.content, &field.placeholder);
        let text_color = if field.content.is_empty() {
            field.appearance.placeholder
        } else {
            field.appearance.text
        };
        let selected_range = field.selected_range.clone();
        let cursor_offset = field.cursor_offset();
        let marked_range = field.marked_range.clone();
        let appearance = field.appearance;
        let previous_scroll_offset = field.last_scroll_offset;

        let base_run = TextRun {
            len: display_text.len(),
            font: window.text_style().font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = marked_range {
            let marked_start = mapping.content_to_display(marked_range.start);
            let marked_end = mapping.content_to_display(marked_range.end);
            vec![
                TextRun {
                    len: marked_start,
                    ..base_run.clone()
                },
                TextRun {
                    len: marked_end.saturating_sub(marked_start),
                    underline: Some(UnderlineStyle {
                        color: Some(base_run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..base_run.clone()
                },
                TextRun {
                    len: display_text.len().saturating_sub(marked_end),
                    ..base_run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![base_run]
        };

        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);
        let cursor_x = line.x_for_index(mapping.content_to_display(cursor_offset));
        let caret_width = px(2.0);
        let visible_width = if bounds.size.width > caret_width {
            bounds.size.width - caret_width
        } else {
            px(0.0)
        };
        let mut scroll_offset = previous_scroll_offset;
        if cursor_x < scroll_offset {
            scroll_offset = cursor_x;
        } else if cursor_x > scroll_offset + visible_width {
            scroll_offset = cursor_x - visible_width;
        }
        let maximum_scroll = if line.width > visible_width {
            line.width - visible_width
        } else {
            px(0.0)
        };
        if scroll_offset > maximum_scroll {
            scroll_offset = maximum_scroll;
        }

        let cursor_x = bounds.left() + cursor_x - scroll_offset;
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(cursor_x, bounds.top()),
                        size(caret_width, bounds.size.height),
                    ),
                    appearance.cursor,
                )),
            )
        } else {
            let start = mapping.content_to_display(selected_range.start);
            let end = mapping.content_to_display(selected_range.end);
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(start) - scroll_offset,
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(end) - scroll_offset,
                            bounds.bottom(),
                        ),
                    ),
                    appearance.selection,
                )),
                None,
            )
        };

        PrepaintState {
            line: Some(line),
            mapping: Some(mapping),
            scroll_offset,
            cursor,
            selection,
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
        let focus_handle = self.field.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.field.clone()),
            cx,
        );

        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }
        let line = prepaint
            .line
            .take()
            .expect("text field line was not shaped");
        if line
            .paint(
                point(bounds.left() - prepaint.scroll_offset, bounds.top()),
                window.line_height(),
                window,
                cx,
            )
            .is_err()
        {
            return;
        }
        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        let mapping = prepaint
            .mapping
            .take()
            .expect("text field display mapping was not prepared");
        self.field.update(cx, |field, _| {
            field.last_layout = Some(line);
            field.last_bounds = Some(bounds);
            field.last_mapping = Some(mapping);
            field.last_scroll_offset = prepaint.scroll_offset;
        });
    }
}

enum DisplayMapping {
    Identity,
    Placeholder,
    Masked(Vec<(usize, usize)>),
}

impl DisplayMapping {
    fn new(content: &str, masked: bool) -> Self {
        if content.is_empty() {
            Self::Placeholder
        } else if masked {
            let mut boundaries = Vec::new();
            boundaries.push((0, 0));
            let mut display_offset = 0;
            for (content_offset, grapheme) in content.grapheme_indices(true) {
                display_offset += '•'.len_utf8();
                boundaries.push((content_offset + grapheme.len(), display_offset));
            }
            Self::Masked(boundaries)
        } else {
            Self::Identity
        }
    }

    fn display_text(&self, content: &SharedString, placeholder: &SharedString) -> SharedString {
        match self {
            Self::Identity => content.clone(),
            Self::Placeholder => placeholder.clone(),
            Self::Masked(boundaries) => "•".repeat(boundaries.len().saturating_sub(1)).into(),
        }
    }

    fn content_to_display(&self, offset: usize) -> usize {
        match self {
            Self::Identity => offset,
            Self::Placeholder => 0,
            Self::Masked(boundaries) => {
                match boundaries.binary_search_by_key(&offset, |(content, _)| *content) {
                    Ok(index) => boundaries[index].1,
                    Err(0) => 0,
                    Err(index) => boundaries[index - 1].1,
                }
            }
        }
    }

    fn display_to_content(&self, offset: usize) -> usize {
        match self {
            Self::Identity => offset,
            Self::Placeholder => 0,
            Self::Masked(boundaries) => {
                match boundaries.binary_search_by_key(&offset, |(_, display)| *display) {
                    Ok(index) => boundaries[index].0,
                    Err(0) => 0,
                    Err(index) => boundaries[index - 1].0,
                }
            }
        }
    }
}

fn sanitize_shared_string(value: SharedString) -> SharedString {
    if value.bytes().any(|byte| byte == b'\r' || byte == b'\n') {
        sanitize_text(&value).into_owned().into()
    } else {
        value
    }
}

fn sanitize_text(value: &str) -> Cow<'_, str> {
    if value.bytes().any(|byte| byte == b'\r' || byte == b'\n') {
        Cow::Owned(
            value
                .chars()
                .map(|character| match character {
                    '\r' | '\n' => ' ',
                    character => character,
                })
                .collect(),
        )
    } else {
        Cow::Borrowed(value)
    }
}

fn offset_from_utf16(text: &str, offset: usize) -> usize {
    let mut utf8_offset = 0;
    let mut utf16_offset = 0;
    for character in text.chars() {
        if utf16_offset >= offset {
            break;
        }
        utf16_offset += character.len_utf16();
        utf8_offset += character.len_utf8();
    }
    utf8_offset
}

fn offset_to_utf16(text: &str, offset: usize) -> usize {
    let mut utf8_offset = 0;
    let mut utf16_offset = 0;
    for character in text.chars() {
        if utf8_offset >= offset {
            break;
        }
        utf8_offset += character.len_utf8();
        utf16_offset += character.len_utf16();
    }
    utf16_offset
}

fn range_from_utf16(text: &str, range: &Range<usize>) -> Range<usize> {
    let start = offset_from_utf16(text, range.start);
    let end = offset_from_utf16(text, range.end);
    if start <= end { start..end } else { end..start }
}

fn range_to_utf16(text: &str, range: &Range<usize>) -> Range<usize> {
    let start = offset_to_utf16(text, range.start);
    let end = offset_to_utf16(text, range.end);
    if start <= end { start..end } else { end..start }
}

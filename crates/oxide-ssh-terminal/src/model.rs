use std::{mem, sync::Arc};

use alacritty_terminal::{
    event::{Event, EventListener, WindowSize},
    grid::{Dimensions, Scroll},
    index::{Column, Point, Side},
    selection::{Selection, SelectionType},
    term::{
        Config, MIN_COLUMNS, MIN_SCREEN_LINES, Osc52, RenderableContent, Term, TermDamage,
        TermMode,
        cell::{Cell, Flags},
        color,
    },
    vte::ansi::{Color, Processor, Rgb},
};
use bytes::Bytes;
use parking_lot::Mutex;
use smallvec::SmallVec;

pub const SCROLLBACK_HISTORY_LINES: usize = 10_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalSize {
    pub columns: usize,
    pub rows: usize,
    pub pixel_width: u32,
    pub pixel_height: u32,
}

impl TerminalSize {
    pub fn is_valid(self) -> bool {
        self.columns >= MIN_COLUMNS && self.rows >= MIN_SCREEN_LINES
    }
}

impl Dimensions for TerminalSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RgbColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl RgbColor {
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }
}

impl From<RgbColor> for Rgb {
    fn from(color: RgbColor) -> Self {
        Self {
            r: color.red,
            g: color.green,
            b: color.blue,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalColors {
    pub foreground: RgbColor,
    pub background: RgbColor,
    pub cursor: RgbColor,
    pub ansi: [RgbColor; 16],
}

impl Default for TerminalColors {
    fn default() -> Self {
        Self {
            foreground: RgbColor::new(0xe6, 0xe8, 0xeb),
            background: RgbColor::new(0x11, 0x13, 0x18),
            cursor: RgbColor::new(0xe6, 0xe8, 0xeb),
            ansi: [
                RgbColor::new(0x1b, 0x1d, 0x23),
                RgbColor::new(0xe0, 0x6c, 0x75),
                RgbColor::new(0x98, 0xc3, 0x79),
                RgbColor::new(0xe5, 0xc0, 0x7b),
                RgbColor::new(0x61, 0xaf, 0xef),
                RgbColor::new(0xc6, 0x78, 0xdd),
                RgbColor::new(0x56, 0xb6, 0xc2),
                RgbColor::new(0xab, 0xb2, 0xbf),
                RgbColor::new(0x5c, 0x63, 0x70),
                RgbColor::new(0xff, 0x7b, 0x86),
                RgbColor::new(0xb3, 0xe9, 0x8c),
                RgbColor::new(0xff, 0xd6, 0x8a),
                RgbColor::new(0x76, 0xc7, 0xff),
                RgbColor::new(0xd9, 0x9b, 0xff),
                RgbColor::new(0x70, 0xe1, 0xed),
                RgbColor::new(0xf5, 0xf7, 0xfa),
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CellRenderStyle {
    pub foreground: RgbColor,
    pub background: RgbColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
    pub hidden: bool,
    pub wide: bool,
    pub wide_spacer: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalAction {
    WriteBack(Bytes),
    Bell,
    Wakeup,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalError {
    InvalidSize,
}

impl std::fmt::Display for TerminalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("invalid terminal size")
    }
}

impl std::error::Error for TerminalError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CellSide {
    Left,
    Right,
}

impl From<CellSide> for Side {
    fn from(side: CellSide) -> Self {
        match side {
            CellSide::Left => Side::Left,
            CellSide::Right => Side::Right,
        }
    }
}

#[derive(Clone)]
struct TerminalEventProxy {
    pending: Arc<Mutex<Vec<Event>>>,
}

impl EventListener for TerminalEventProxy {
    fn send_event(&self, event: Event) {
        self.pending.lock().push(event);
    }
}

pub struct TerminalModel {
    term: Term<TerminalEventProxy>,
    processor: Processor,
    pending_events: Arc<Mutex<Vec<Event>>>,
    size: TerminalSize,
    default_colors: TerminalColors,
}

impl TerminalModel {
    pub fn new(size: TerminalSize) -> Result<Self, TerminalError> {
        Self::with_colors(size, TerminalColors::default())
    }

    pub fn with_colors(
        size: TerminalSize,
        default_colors: TerminalColors,
    ) -> Result<Self, TerminalError> {
        if !size.is_valid() {
            return Err(TerminalError::InvalidSize);
        }

        let pending_events = Arc::new(Mutex::new(Vec::new()));
        let event_proxy = TerminalEventProxy {
            pending: pending_events.clone(),
        };
        let config = Config {
            scrolling_history: SCROLLBACK_HISTORY_LINES,
            kitty_keyboard: false,
            osc52: Osc52::Disabled,
            ..Default::default()
        };

        Ok(Self {
            term: Term::new(config, &size, event_proxy),
            processor: Processor::new(),
            pending_events,
            size,
            default_colors,
        })
    }

    pub fn process_output(&mut self, bytes: &[u8]) -> SmallVec<[TerminalAction; 4]> {
        self.processor.advance(&mut self.term, bytes);
        self.processor.stop_sync(&mut self.term);

        let mut pending = {
            let mut shared = self.pending_events.lock();
            mem::take(&mut *shared)
        };
        let mut actions = SmallVec::new();
        for event in pending.drain(..) {
            match event {
                Event::PtyWrite(text) => {
                    actions.push(TerminalAction::WriteBack(Bytes::from(text)));
                }
                Event::ColorRequest(index, formatter) => {
                    let response = formatter(self.color(index));
                    actions.push(TerminalAction::WriteBack(Bytes::from(response)));
                }
                Event::TextAreaSizeRequest(formatter) => {
                    let response = formatter(self.window_size());
                    actions.push(TerminalAction::WriteBack(Bytes::from(response)));
                }
                Event::Bell => actions.push(TerminalAction::Bell),
                Event::Wakeup => actions.push(TerminalAction::Wakeup),
                Event::MouseCursorDirty
                | Event::Title(_)
                | Event::ResetTitle
                | Event::ClipboardStore(_, _)
                | Event::ClipboardLoad(_, _)
                | Event::CursorBlinkingChange
                | Event::Exit
                | Event::ChildExit(_) => {}
            }
        }
        *self.pending_events.lock() = pending;

        if !bytes.is_empty()
            && !actions
                .iter()
                .any(|action| matches!(action, TerminalAction::Wakeup))
        {
            actions.push(TerminalAction::Wakeup);
        }
        actions
    }

    pub fn resize(&mut self, size: TerminalSize) -> Result<(), TerminalError> {
        if !size.is_valid() {
            return Err(TerminalError::InvalidSize);
        }

        self.term.resize(size);
        self.size = size;
        Ok(())
    }

    pub fn size(&self) -> TerminalSize {
        self.size
    }

    pub fn set_default_colors(&mut self, colors: TerminalColors) {
        self.default_colors = colors;
    }

    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    pub fn history_size(&self) -> usize {
        self.term.grid().history_size()
    }

    pub fn renderable_content(&self) -> RenderableContent<'_> {
        self.term.renderable_content()
    }

    pub fn cell_render_style(&self, cell: &Cell) -> CellRenderStyle {
        let bold = cell.flags.contains(Flags::BOLD);
        let dim = cell.flags.contains(Flags::DIM);
        let mut foreground = self.resolve_color(cell.fg, bold, dim);
        let mut background = self.resolve_color(cell.bg, false, false);
        if cell.flags.contains(Flags::INVERSE) {
            mem::swap(&mut foreground, &mut background);
        }
        CellRenderStyle {
            foreground,
            background,
            bold,
            italic: cell.flags.contains(Flags::ITALIC),
            underline: cell
                .flags
                .intersects(Flags::UNDERLINE | Flags::DOUBLE_UNDERLINE | Flags::UNDERCURL),
            strikeout: cell.flags.contains(Flags::STRIKEOUT),
            hidden: cell.flags.contains(Flags::HIDDEN),
            wide: cell.flags.contains(Flags::WIDE_CHAR),
            wide_spacer: cell.flags.contains(Flags::WIDE_CHAR_SPACER),
        }
    }

    pub fn damage(&mut self) -> TermDamage<'_> {
        self.term.damage()
    }

    pub fn reset_damage(&mut self) {
        self.term.reset_damage();
    }

    pub fn scroll_display(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
    }

    pub fn start_selection(&mut self, row: usize, column: usize, side: CellSide) {
        let point = self.viewport_point(row, column);
        self.term.selection = Some(Selection::new(SelectionType::Simple, point, side.into()));
    }

    pub fn update_selection(&mut self, row: usize, column: usize, side: CellSide) {
        let point = self.viewport_point(row, column);
        if let Some(selection) = self.term.selection.as_mut() {
            selection.update(point, side.into());
        }
    }

    pub fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    pub fn selected_text(&self) -> Option<String> {
        self.term.selection_to_string()
    }

    fn viewport_point(&self, row: usize, column: usize) -> Point {
        let row = row.min(self.size.rows - 1);
        let column = Column(column.min(self.size.columns - 1));
        alacritty_terminal::term::viewport_to_point(
            self.term.grid().display_offset(),
            Point::new(row, column),
        )
    }

    fn window_size(&self) -> WindowSize {
        let columns = u16::try_from(self.size.columns).unwrap_or(u16::MAX);
        let rows = u16::try_from(self.size.rows).unwrap_or(u16::MAX);
        let cell_width = self.size.pixel_width / self.size.columns as u32;
        let cell_height = self.size.pixel_height / self.size.rows as u32;
        WindowSize {
            num_lines: rows,
            num_cols: columns,
            cell_width: u16::try_from(cell_width).unwrap_or(u16::MAX),
            cell_height: u16::try_from(cell_height).unwrap_or(u16::MAX),
        }
    }

    fn resolve_color(&self, color: Color, bold: bool, dim: bool) -> RgbColor {
        let rgb = match color {
            Color::Spec(color) if dim => color * 0.66,
            Color::Spec(color) => color,
            Color::Indexed(index) => self.color(index as usize),
            Color::Named(mut color) => {
                if bold {
                    color = color.to_bright();
                } else if dim {
                    color = color.to_dim();
                }
                self.color(color as usize)
            }
        };
        RgbColor::new(rgb.r, rgb.g, rgb.b)
    }

    fn color(&self, index: usize) -> Rgb {
        if index < color::COUNT
            && let Some(color) = self.term.colors()[index]
        {
            return color;
        }

        match index {
            0..=15 => self.default_colors.ansi[index].into(),
            16..=255 => xterm_color(index),
            256 | 267 => self.default_colors.foreground.into(),
            257 | 268 => self.default_colors.background.into(),
            258 => self.default_colors.cursor.into(),
            259..=266 => self.default_colors.foreground.into(),
            _ => self.default_colors.background.into(),
        }
    }
}

impl Dimensions for TerminalModel {
    fn total_lines(&self) -> usize {
        self.term.total_lines()
    }

    fn screen_lines(&self) -> usize {
        self.term.screen_lines()
    }

    fn columns(&self) -> usize {
        self.term.columns()
    }
}

fn xterm_color(index: usize) -> Rgb {
    const ANSI: [Rgb; 16] = [
        Rgb {
            r: 0x00,
            g: 0x00,
            b: 0x00,
        },
        Rgb {
            r: 0xcd,
            g: 0x00,
            b: 0x00,
        },
        Rgb {
            r: 0x00,
            g: 0xcd,
            b: 0x00,
        },
        Rgb {
            r: 0xcd,
            g: 0xcd,
            b: 0x00,
        },
        Rgb {
            r: 0x00,
            g: 0x00,
            b: 0xee,
        },
        Rgb {
            r: 0xcd,
            g: 0x00,
            b: 0xcd,
        },
        Rgb {
            r: 0x00,
            g: 0xcd,
            b: 0xcd,
        },
        Rgb {
            r: 0xe5,
            g: 0xe5,
            b: 0xe5,
        },
        Rgb {
            r: 0x7f,
            g: 0x7f,
            b: 0x7f,
        },
        Rgb {
            r: 0xff,
            g: 0x00,
            b: 0x00,
        },
        Rgb {
            r: 0x00,
            g: 0xff,
            b: 0x00,
        },
        Rgb {
            r: 0xff,
            g: 0xff,
            b: 0x00,
        },
        Rgb {
            r: 0x5c,
            g: 0x5c,
            b: 0xff,
        },
        Rgb {
            r: 0xff,
            g: 0x00,
            b: 0xff,
        },
        Rgb {
            r: 0x00,
            g: 0xff,
            b: 0xff,
        },
        Rgb {
            r: 0xff,
            g: 0xff,
            b: 0xff,
        },
    ];
    const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

    match index {
        0..=15 => ANSI[index],
        16..=231 => {
            let offset = index - 16;
            Rgb {
                r: CUBE[offset / 36],
                g: CUBE[(offset / 6) % 6],
                b: CUBE[offset % 6],
            }
        }
        232..=255 => {
            let value = 8 + ((index - 232) * 10) as u8;
            Rgb {
                r: value,
                g: value,
                b: value,
            }
        }
        _ => unreachable!("xterm color index must be in 0..=255"),
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::{
        grid::Dimensions,
        index::{Column, Line},
        term::{TermMode, cell::Flags},
    };

    use super::*;

    fn size(columns: usize, rows: usize) -> TerminalSize {
        TerminalSize {
            columns,
            rows,
            pixel_width: (columns * 10) as u32,
            pixel_height: (rows * 20) as u32,
        }
    }

    fn visible_text(model: &TerminalModel) -> String {
        model
            .renderable_content()
            .display_iter
            .filter_map(|indexed| {
                (!indexed
                    .cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER))
                .then_some(indexed.cell.c)
            })
            .collect()
    }

    #[test]
    fn output_updates_borrowed_renderable_content() {
        let mut model = TerminalModel::new(size(12, 3)).unwrap();
        assert_eq!(
            model.process_output("hello 你".as_bytes()).as_slice(),
            &[TerminalAction::Wakeup],
        );

        let text = visible_text(&model);
        assert!(text.contains("hello 你"));
        assert_eq!(model.renderable_content().mode, TermMode::default());
    }

    #[test]
    fn bell_queries_and_unsafe_osc_events_are_bounded() {
        let mut model = TerminalModel::new(size(80, 24)).unwrap();
        let actions =
            model.process_output(b"\x07\x1b[6n\x1b]0;ignored title\x07\x1b]52;c;YQ==\x07");

        assert!(actions.contains(&TerminalAction::Bell));
        assert!(
            actions.contains(&TerminalAction::WriteBack(bytes::Bytes::from_static(
                b"\x1b[1;1R"
            )))
        );
        assert_eq!(
            actions
                .iter()
                .filter(|action| matches!(action, TerminalAction::WriteBack(_)))
                .count(),
            1,
        );
        assert_eq!(
            actions
                .iter()
                .filter(|action| matches!(action, TerminalAction::Wakeup))
                .count(),
            1,
        );
    }

    #[test]
    fn color_and_text_area_queries_write_back_terminal_values() {
        let mut model = TerminalModel::with_colors(
            size(80, 24),
            TerminalColors {
                foreground: RgbColor::new(0xe6, 0xe8, 0xeb),
                background: RgbColor::new(0x11, 0x13, 0x18),
                cursor: RgbColor::new(0xe6, 0xe8, 0xeb),
                ..TerminalColors::default()
            },
        )
        .unwrap();
        let actions = model.process_output(b"\x1b]10;?\x07\x1b[14t");
        let replies: Vec<_> = actions
            .into_iter()
            .filter_map(|action| match action {
                TerminalAction::WriteBack(bytes) => Some(bytes),
                _ => None,
            })
            .collect();

        assert!(
            replies
                .iter()
                .any(|reply| { reply.as_ref() == b"\x1b]10;rgb:e6e6/e8e8/ebeb\x07" })
        );
        assert!(
            replies
                .iter()
                .any(|reply| reply.as_ref() == b"\x1b[4;480;800t")
        );
    }

    #[test]
    fn alternate_screen_and_256_colors_render() {
        let mut model = TerminalModel::new(size(20, 3)).unwrap();
        model.process_output(b"\x1b[38;5;196mred\x1b[0m");
        let red_cell = model
            .renderable_content()
            .display_iter
            .find(|indexed| indexed.cell.c == 'r')
            .unwrap();
        assert_eq!(
            model.cell_render_style(red_cell.cell).foreground,
            RgbColor::new(255, 0, 0)
        );

        model.process_output(b"\x1b[?1049halt");
        assert!(model.mode().contains(TermMode::ALT_SCREEN));
        assert!(visible_text(&model).contains("alt"));
        model.process_output(b"\x1b[?1049l");
        assert!(!model.mode().contains(TermMode::ALT_SCREEN));
        assert!(visible_text(&model).contains("red"));
    }

    #[test]
    fn wide_cjk_cells_survive_resize() {
        let mut model = TerminalModel::new(size(10, 2)).unwrap();
        model.process_output("A你B".as_bytes());

        model.resize(size(20, 4)).unwrap();

        assert!(visible_text(&model).contains("A你B"));
        let cjk_cells = model
            .renderable_content()
            .display_iter
            .filter(|indexed| indexed.cell.c == '你')
            .count();
        assert_eq!(cjk_cells, 1);
    }

    #[test]
    fn theme_change_preserves_content_and_updates_default_colors() {
        let mut model = TerminalModel::new(size(10, 2)).unwrap();
        model.process_output(b"content");
        let light = TerminalColors {
            foreground: RgbColor::new(0x11, 0x22, 0x33),
            background: RgbColor::new(0xee, 0xee, 0xee),
            cursor: RgbColor::new(0x11, 0x22, 0x33),
            ..TerminalColors::default()
        };

        model.set_default_colors(light);

        assert!(visible_text(&model).contains("content"));
        let cell = model
            .renderable_content()
            .display_iter
            .find(|indexed| indexed.cell.c == 'c')
            .unwrap();
        assert_eq!(
            model.cell_render_style(cell.cell).foreground,
            light.foreground
        );
    }

    #[test]
    fn history_alt_screen_resize_and_modes_are_preserved() {
        let mut model = TerminalModel::new(size(10, 2)).unwrap();
        model.process_output(b"one\r\ntwo\r\nthree");
        assert!(model.history_size() > 0);
        assert!(model.history_size() <= SCROLLBACK_HISTORY_LINES);

        model.process_output(b"\x1b[?1049halt");
        assert!(model.mode().contains(TermMode::ALT_SCREEN));
        assert!(visible_text(&model).contains("alt"));
        model.process_output(b"\x1b[?1049l");
        assert!(!model.mode().contains(TermMode::ALT_SCREEN));
        assert!(visible_text(&model).contains("three"));

        model.process_output(b"\x1b[?2004h\x1b[?1h");
        assert!(model.mode().contains(TermMode::BRACKETED_PASTE));
        assert!(model.mode().contains(TermMode::APP_CURSOR));

        model.resize(size(132, 43)).unwrap();
        assert_eq!(model.columns(), 132);
        assert_eq!(model.screen_lines(), 43);
        assert_eq!(model.size(), size(132, 43));
    }

    #[test]
    fn selection_copies_visible_text() {
        let mut model = TerminalModel::new(size(10, 2)).unwrap();
        model.process_output(b"hello");
        model.start_selection(0, 0, CellSide::Left);
        model.update_selection(0, 4, CellSide::Right);

        assert_eq!(model.selected_text().as_deref(), Some("hello"));
        model.clear_selection();
        assert_eq!(model.selected_text(), None);
    }

    #[test]
    fn invalid_resize_preserves_existing_dimensions() {
        let mut model = TerminalModel::new(size(80, 24)).unwrap();
        let invalid = TerminalSize {
            columns: 1,
            rows: 0,
            pixel_width: 0,
            pixel_height: 0,
        };
        assert_eq!(model.resize(invalid), Err(TerminalError::InvalidSize));
        assert_eq!(model.columns(), 80);
        assert_eq!(model.screen_lines(), 24);

        let content = model.renderable_content();
        assert_eq!(content.cursor.point.line, Line(0));
        assert_eq!(content.cursor.point.column, Column(0));
    }
}

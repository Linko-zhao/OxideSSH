//! Portable terminal state, input encoding, selection, and render snapshots.

mod input;
mod model;

pub use input::{InputEncoder, Key, KeyInput, MAX_PASTE_BYTES, Modifiers, PasteError};
pub use model::{
    CellRenderStyle, CellSide, RgbColor, SCROLLBACK_HISTORY_LINES, TerminalAction, TerminalColors,
    TerminalError, TerminalModel, TerminalSize,
};

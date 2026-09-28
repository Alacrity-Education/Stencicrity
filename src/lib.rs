//! stencicrity: merge the solder-paste gerbers of several KiCad projects into
//! one stencil order, with a TUI to decide on pads that have no paste.
//!
//! Module map (mirrors the pipeline in `cli`):
//! `gerber` parses RS-274X into geometry, `writer` emits it again,
//! `project` discovers gerber sets, `pads` finds pads and applies the rules,
//! `config` reads and writes `.stencicrity`, `layout` packs cells on the
//! sheet, `render` draws the preview PNG, `tui` is the terminal UI, and
//! `model` holds the shared data types.

pub mod ascii;
pub mod cli;
pub mod config;
pub mod gerber;
pub mod layout;
pub mod model;
pub mod pads;
pub mod project;
pub mod render;
pub mod tui;
pub mod util;
pub mod writer;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

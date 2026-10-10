//! Rendering primitives shared by the kit's components and cells.
//!
//! Phase 3 moves the render layer here from `crates/tui` in dependency order:
//! leaf utilities first, then cells, then the App-coupled panels (which require
//! the `Ctx` abstraction).

pub mod bar;
pub mod cells;
pub mod ctx;
pub mod input;
pub mod log;
pub mod log_column;
pub mod mermaid_sequence;
pub mod popups;
pub mod pulldown;
pub mod render_md;
pub mod renderable;
pub mod scroll_pill;
pub mod selectable_text;
pub mod slash_style;
pub mod stats_line;
pub mod sticky_host;
pub mod util;

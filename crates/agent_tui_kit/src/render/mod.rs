//! Rendering primitives shared by the kit's components and cells.
//!
//! Phase 3 moves the render layer here from `crates/tui` in dependency order:
//! leaf utilities first, then cells, then the App-coupled panels (which require
//! the `Ctx` abstraction).

pub mod cells;
pub mod ctx;
pub mod log;
pub mod log_column;
pub mod mermaid_sequence;
pub mod pulldown;
pub mod render_md;
pub mod renderable;
pub mod selectable_text;
pub mod util;

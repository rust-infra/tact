//! Render module — split by panel type.

mod bar;
pub(crate) mod cells;
mod input;
mod layout;
mod log;
mod log_column;
mod plan;
pub(crate) mod popups;
pub(crate) mod render_md;
pub(crate) mod renderable;
mod util;
pub(crate) mod welcome;

pub(super) use bar::{render_bottom_bar, render_status_bar};
pub(super) use input::render_input_box;
pub(super) use layout::render_main_area;
pub(super) use popups::command_palette::render_command_palette;
pub(super) use popups::select::render_select_popup;

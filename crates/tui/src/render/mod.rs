//! Render module — split by panel type.

mod bar;
pub(crate) mod cells;
mod input;
mod layout;
mod log;
mod log_column;
mod log_style;
mod plan;
pub(crate) mod popups;
pub(crate) mod render_md;
pub(crate) mod renderable;
pub(crate) mod slash_style;
pub(crate) mod util;

pub(super) use bar::{render_bottom_bar, render_status_bar};
pub(super) use input::render_input_box;
pub(super) use layout::render_main_area;
pub(crate) use log::effective_max_logical_scroll;
pub(crate) use log_style::is_user_message_line;
pub(super) use popups::command_palette::render_command_palette;
pub(super) use popups::file_picker::render_file_picker;
pub(super) use popups::select::render_select_popup;
pub(super) use popups::slash_command::render_slash_command_popup;

#[cfg(test)]
mod log_render_tests;
#[cfg(test)]
mod popup_scene_tests;
#[cfg(test)]
mod render_gap_tests;
#[cfg(test)]
mod scene_tests;

#[cfg(any(test, feature = "test-support"))]
pub mod test_harness;

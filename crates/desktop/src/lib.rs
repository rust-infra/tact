//! Tact Desktop on Native Shell: the app's controller bridge and the JSON
//! view of its snapshot that the TSX shell renders.

pub mod app;
pub mod bridge;
pub mod json;
mod surface;

use nsh_core::prelude::*;

/// The app's UI set (`nsh.json` `"rust"`): what a TSX entry may import from
/// `tact-desktop/native`.
#[native_module("tact-desktop/native")]
fn native(registry: &mut Registry) {
    registry.class::<bridge::Controller>();
}

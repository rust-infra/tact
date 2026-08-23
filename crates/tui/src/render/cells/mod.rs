//! Re-exports of kit cell renderers still consumed by app-layer tui code.
//!
//! `code`, `text`, and `tool` moved wholesale into `agent_tui_kit::render::log`
//! (the pure log renderer) and are no longer referenced here; only the cells
//! that app-layer code touches directly (`separator`, `thinking`) keep a
//! convenience re-export.

pub(crate) mod separator {
    pub(crate) use agent_tui_kit::render::cells::separator::*;
}
pub(crate) mod thinking {
    pub(crate) use agent_tui_kit::render::cells::thinking::*;
}

#[cfg(test)]
mod code_overlay_tests;
#[cfg(test)]
mod markdown_integration_tests;

//! Popup family — app-layer wrappers.
//!
//! Pure popup renderers live in `agent_tui_kit::render::popups`; this module
//! keeps the `&mut App` wrappers (mouse hit-area application, prepare phases)
//! plus the app-layer popups that own Tact-specific state:
//! `command_palette`, `file_picker`, `slash_command`, `task_dag_popup`.

pub(crate) mod code_popup;
pub(crate) mod command_palette;
pub(crate) mod diff_popup;
pub(crate) mod file_picker;
pub(crate) mod help;
pub(crate) mod history;
pub(crate) mod mermaid_popup;
pub(crate) mod select;
pub(crate) mod selectable_text {}
pub(crate) mod slash_command;
pub(crate) mod subagent_popup;
pub(crate) mod system_prompt_popup;
pub(crate) mod task_dag_popup;
pub(crate) mod thinking_popup;

// Chrome helpers moved to the kit; re-export for the app-layer popups above.
pub(crate) use agent_tui_kit::render::popups::{
    FooterHint, centered_list_popup_area, centered_popup_area, render_list_popup_chrome,
    render_popup_chrome,
};

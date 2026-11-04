#![allow(dead_code)]

//! Desktop shell surfaces.
//!
//! Rendering lives in `shell` and the NSH components; these modules own the
//! small, testable decisions that must stay consistent across the shell:
//! layout, transcript identity, focus restoration, and command-surface
//! metadata.

pub mod ansi;
pub mod background;
pub mod balance;
pub mod command_center;
pub mod extensions;
pub mod mermaid;
pub mod overlays;
pub mod session_manager;
pub mod shell;
pub mod transcript;
pub mod web_search;
pub mod command_forms;
pub mod composer;
pub mod diff_review;
pub mod goals;
pub mod inspector;
pub mod layout;
pub mod readers;
pub mod session_rail;
pub mod settings;
pub mod slash_overlay;
pub mod sticky;
pub mod subagent_model;
pub mod subagents;
pub mod system_prompt;
pub mod task_dag;
pub mod title_bar;
pub mod trajectory;
pub mod transcript_row;

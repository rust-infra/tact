#![allow(dead_code)]

//! Desktop shell surfaces.
//!
//! Rendering stays in `root`; these modules own the small, testable decisions
//! that must stay consistent across the shell: layout, transcript identity,
//! focus restoration, and command-surface metadata.

pub mod background;
pub mod balance;
pub mod command_center;
pub mod command_forms;
pub mod composer;
pub mod extensions;
pub mod goals;
pub mod inspector;
pub mod layout;
pub mod overlays;
pub mod readers;
pub mod session_manager;
pub mod session_rail;
pub mod settings;
pub mod shell;
pub mod slash_overlay;
pub mod sticky;
pub mod subagent_model;
pub mod subagents;
pub mod system_prompt;
pub mod task_dag;
pub mod title_bar;
pub mod transcript;
pub mod transcript_row;

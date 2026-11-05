#![allow(dead_code)]

//! Desktop shell surfaces.
//!
//! Rendering stays in `root`; these modules own the small, testable decisions
//! that must stay consistent across the shell: layout, transcript identity,
//! focus restoration, and command-surface metadata.

pub mod ansi;
pub mod background;
pub mod balance;
pub mod command_center;
pub mod command_forms;
pub mod command_search;
pub mod composer;
pub mod diff_review;
pub mod diff_table;
pub mod extensions;
pub mod filter_table;
pub mod goals;
pub mod inspector;
pub mod layout;
pub mod leaf;
pub mod mermaid;
pub mod overlays;
pub mod plan_card;
pub mod prompt_bar;
pub mod question_flow;
pub mod reader_components;
pub mod readers;
pub mod row_scroll;
pub mod selection_actions;
pub mod session_manager;
pub mod session_rail;
pub mod settings;
pub mod shell;
pub mod sidebar_nav;
pub mod slash_overlay;
pub mod sticky;
pub mod subagent_model;
pub mod subagents;
pub mod surface;
pub mod system_prompt;
pub mod task_dag;
pub mod title_bar;
pub mod trajectory;
pub mod transcript;
pub mod transcript_components;
pub mod transcript_row;
pub mod web_search;

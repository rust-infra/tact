//! Accessibility contract for the public gpui-ai components.
//!
//! One module per component family under test, so a failure and the probe that
//! produced it stay in the same file, plus a `harness` for the input helpers
//! more than one family needs. Cargo builds this directory as the single
//! `component_accessibility` test binary, so every `--test
//! component_accessibility` invocation — and every substring filter run against
//! it — keeps working unchanged.

mod harness;

mod chat;
mod command_search;
mod content_semantics;
mod fine_tune;
mod prompt_bar;
mod selection_actions;
mod sidebar_nav;
mod streaming_text;

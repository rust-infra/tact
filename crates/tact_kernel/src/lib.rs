//! Tact's runtime kernel.
//!
//! The kernel owns mechanisms that every extension and view must share:
//! plugin lifecycle, capability identity and filtering, registration
//! catalogs, the tool execution pipeline, and the neutral contracts those
//! mechanisms pass across boundaries.
//!
//! It deliberately does not own a concrete tool, command, hook, prompt
//! section, agent loop, chat turn, session store, MCP client, provider client
//! or view. Those live in the extensions and view crates and are assembled by
//! the host's explicit composition root.
//!
//! The crate is a leaf above [`tact_protocol`]: it must not depend on the
//! extensions crate, a view crate, `tact_llm`, SQLite, MCP or ratatui. The
//! boundary is enforced by `scripts/check-architecture.sh`.

pub mod capability;
pub mod catalog;
pub mod permission;
pub mod plugin;
pub mod runtime;
pub mod service;
pub mod tool;

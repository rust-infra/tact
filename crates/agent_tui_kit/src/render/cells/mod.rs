//! Log cell renderers — pure per-row draw units (no `App` dependency).
//!
//! `text` / `separator` moved in Phase 3 (T3.1c). `thinking`, `markdown`,
//! `tool`, `code` remain in `crates/tui` until their state/cluster
//! dependencies are extracted (see the plan's Phase 3 next-slice findings).

pub mod code;
pub mod markdown;
pub mod separator;
pub mod text;
pub mod thinking;
pub mod tool;

//! Log cell renderers — pure per-row draw units (no `App` dependency).
//!
//! All log cells live here: `text`, `separator`, `thinking`, `markdown`,
//! `tool`, `tool_run`, `activity_region`, and `code`. `crates/tui` keeps only
//! a `render::cells` convenience re-export for `separator` and `thinking`,
//! because app-layer code reaches those two directly for row geometry.
//!
//! A consumer should import from this module directly. A `crate::…` path that
//! resolves to a kit item means a re-export shim is hiding the real owner.

pub mod activity_region;
pub mod code;
pub mod markdown;
pub mod separator;
pub mod text;
pub mod thinking;
pub mod tool;
pub mod tool_run;

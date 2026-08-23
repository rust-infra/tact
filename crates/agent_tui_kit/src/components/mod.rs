//! Real `Component` implementations (step 9 of the plan).
//!
//! Each component owns its state, feeds shared surfaces through `Ctx`, and
//! renders into a plain ratatui `Buffer`. The shell (host) routes updates /
//! keys / renders in `priority()` order.
//!
//! `ThinkingComponent` is the first real implementation (replaces the
//! compile-only trait draft with a working pattern); more components follow
//! as the app layer migrates.

pub mod thinking;

pub use thinking::ThinkingComponent;

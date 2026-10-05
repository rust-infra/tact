//! Poison-tolerant lock accessors — re-exported, not re-defined.
//!
//! Every lock in this crate guards state that is a cache, a counter, a
//! registry, or a config snapshot — not an invariant that a panic could leave
//! torn. A panic elsewhere while one is held poisons the lock, and
//! `unwrap()`/`expect()` on it would then turn a recoverable state glitch into
//! a process abort (in the TUI's case, tearing down the whole session).
//!
//! The two traits carrying that rule live in [`tact_llm::lock`], which is the
//! crate below this one and needs the same pair; see it for the reasoning. They
//! used to be defined here as well, down to the same bodies — and a trait
//! definition is not the kind of duplication that can be left to drift
//! cosmetically: two `read_recover`s that disagreed about whether to recover
//! would make one call mean different things depending on which crate's import
//! a file happened to reach for.

pub use tact_llm::lock::{LockExt, RwLockExt};

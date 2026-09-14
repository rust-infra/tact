//! Poison-tolerant lock accessors.
//!
//! Every lock in this crate guards state that is a cache, a counter, a
//! registry, or a config snapshot — not an invariant that a panic could leave
//! torn. A panic elsewhere while one of these is held poisons the lock, and
//! `unwrap()`/`expect()` on it would then turn a recoverable state glitch into
//! a process abort (in the TUI's case, tearing down the whole session).
//!
//! These helpers recover the guarded value instead. Rust guarantees the data
//! is still memory-safe after a poisoning panic; at worst it is logically
//! stale, which for a cache/counter/registry is exactly the situation the
//! next read already handles.

use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// `Mutex::lock` that recovers from poisoning instead of panicking.
pub trait LockExt<T> {
    fn lock_recover(&self) -> MutexGuard<'_, T>;
}

impl<T> LockExt<T> for Mutex<T> {
    fn lock_recover(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// `RwLock` readers/writers that recover from poisoning instead of panicking.
pub trait RwLockExt<T> {
    fn read_recover(&self) -> RwLockReadGuard<'_, T>;
    fn write_recover(&self) -> RwLockWriteGuard<'_, T>;
}

impl<T> RwLockExt<T> for RwLock<T> {
    fn read_recover(&self) -> RwLockReadGuard<'_, T> {
        self.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write_recover(&self) -> RwLockWriteGuard<'_, T> {
        self.write().unwrap_or_else(PoisonError::into_inner)
    }
}

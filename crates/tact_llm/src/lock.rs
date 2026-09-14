//! Poison-tolerant lock accessors.
//!
//! The statics here (`PROVIDER`, `CREDENTIALS`, the models cache) hold
//! snapshots and caches, not invariants a panic could tear. A panic elsewhere
//! while one is held poisons the lock; `expect()` would then abort the process
//! on the next unrelated read. These helpers recover the value instead — Rust
//! guarantees it is still memory-safe, and at worst it is the previous
//! snapshot, which is exactly what a re-read would have produced.

use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

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

/// `Mutex::lock` that recovers from poisoning instead of panicking.
pub trait LockExt<T> {
    fn lock_recover(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> LockExt<T> for std::sync::Mutex<T> {
    fn lock_recover(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

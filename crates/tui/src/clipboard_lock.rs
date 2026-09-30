//! Serializes real system-clipboard use across the whole test binary.
//!
//! `arboard` drives a process-global pasteboard (AppKit on macOS, X11/Wayland
//! on Linux). Cargo runs one binary's tests in parallel threads, and two of
//! them driving that pasteboard at once **crashes the process** (measured:
//! `SIGSEGV` in the `tui` lib test binary) instead of failing an assertion.
//!
//! The guard is taken wherever Tact actually talks to the pasteboard
//! ([`crate::widgets::state::App::write_system_clipboard`]), so any test that
//! copies is serialized without opting in — including the ones that reach a
//! copy through a keybinding or a render scene.
//!
//! A test that needs the clipboard to stay put across a copy *and* a read-back
//! — the copy tests do — takes the same guard itself. Because
//! `std::sync::Mutex` is not reentrant, the copy path only takes it when its
//! caller has not already done so; [`held_by_this_thread`] is what makes that
//! check exact, rather than a guess based on `try_lock` timing.

// True while *this* thread holds the clipboard guard.
thread_local! {
    static HELD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

static CLIPBOARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Held for as long as the pasteboard is being driven.
pub(crate) struct ClipboardGuard {
    _inner: std::sync::MutexGuard<'static, ()>,
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        HELD.with(|held| held.set(false));
    }
}

/// Takes the process-wide clipboard lock, blocking until it is free.
pub(crate) fn take() -> ClipboardGuard {
    let inner = CLIPBOARD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    HELD.with(|held| held.set(true));
    ClipboardGuard { _inner: inner }
}

/// Whether this thread already holds the clipboard guard.
pub(crate) fn held_by_this_thread() -> bool {
    HELD.with(std::cell::Cell::get)
}

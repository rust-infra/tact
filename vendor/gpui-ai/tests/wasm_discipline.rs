//! Calls that work on every desktop and take the browser down.
//!
//! This library ships to WebAssembly — the whole component catalogue runs
//! there, and it is how most people will first see it. A handful of `std`
//! facilities are simply absent on `wasm32-unknown-unknown`, and reaching one
//! is not a degraded experience: it panics, the panic unwinds into
//! `RuntimeError: unreachable`, and the canvas stops responding to anything at
//! all. A visitor's only way out is a reload.
//!
//! None of it is visible from a desktop build, a desktop test, or a review.
//! The compiler is happy, every native gate is green, and the failure only
//! exists on the target nobody runs locally.
//!
//! ## The clock
//!
//! `std::time::Instant::now()` is unimplemented on wasm32. GPUI delimits
//! scroll gestures with one, so `restrict_scroll_to_axis` panics on the first
//! wheel event over the element that asked for it. gpui-component wraps that
//! in `lock_scroll_axis`, which is a no-op on wasm and the axis lock
//! everywhere else; three surfaces here had called the raw one, so a wheel
//! over a comparison table, a filter table, or the selection actions killed
//! the demo.
//!
//! For a clock of our own, `crate::motion::Instant` is `web_time::Instant` on
//! wasm and `std::time::Instant` everywhere else.

mod source_audit;

use source_audit::{blanked, rust_sources};
use std::path::Path;

/// What may not appear in library source, and what to reach for instead.
const FORBIDDEN: &[(&str, &str)] = &[
    (
        "restrict_scroll_to_axis",
        "lock_scroll_axis, which is the same axis lock off wasm and a no-op on it",
    ),
    (
        "std::time::Instant",
        "crate::motion::Instant, which is web_time's on wasm",
    ),
    (
        "SystemTime::now",
        "a timestamp the application passes in; wasm has no system clock",
    ),
];

#[test]
fn nothing_reaches_for_a_facility_the_browser_does_not_have() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();

    for (relative, path) in rust_sources(&root) {
        let source = std::fs::read_to_string(&path).expect("library source should be readable");
        // Literals and comments blanked: this file names every one of these
        // in its own prose, and a doc comment explaining the rule is not a
        // call that breaks it.
        let code = blanked(&source, true);

        for (call, instead) in FORBIDDEN {
            let mut from = 0;
            while let Some(at) = code[from..].find(call) {
                let line = code[..from + at].lines().count();
                if !guarded_off_wasm(&code, from + at) {
                    found.push(format!("{relative}:{line}: {call} — use {instead}"));
                }
                from += at + call.len();
            }
        }
    }

    found.sort();
    assert!(
        found.is_empty(),
        "{} call(s) that panic on wasm32 and take the whole canvas with them:\n  {}",
        found.len(),
        found.join("\n  ")
    );
}

/// Whether the call at `at` sits on the non-wasm arm of a platform switch.
///
/// `motion.rs` is where that switch lives, and it has to name both clocks to
/// choose between them. Read from the line above rather than from a list of
/// blessed files: an arm is recognisable, and a file name goes stale.
fn guarded_off_wasm(code: &str, at: usize) -> bool {
    // From the start of this line, not from the match: everything before the
    // match on the same line is itself a non-empty "line", and reading that
    // instead of the attribute above it is how this first read every arm as
    // unguarded.
    let line_start = code[..at].rfind('\n').map_or(0, |at| at + 1);
    code[..line_start]
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        // The scan blanks string literals, so the "wasm" inside the
        // attribute is gone by the time this reads it; the attribute
        // around it is not.
        .is_some_and(|line| line.contains("cfg(not(target_family"))
}

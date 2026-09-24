//! Process-level knobs shared by the tools that fork a shell.
//!
//! `bash` and `background_run` both put the shell in its own process group, so
//! cancellation can signal the whole tree the command spawns. Both also lower
//! that group's scheduling priority: a `cargo test` the agent started must not
//! make the interface it reports into unresponsive.

/// Lower the scheduling priority of a child's whole process group.
///
/// `nice` is the caller's `[tools] bash_nice`; a value of zero or less leaves
/// the group at the priority it inherited.
///
/// Best-effort by design: `setpriority(2)` fails without a privilege the parent
/// may not hold, and a command that runs at the inherited priority is a far
/// better outcome than a tool call that fails for a scheduling hint.
#[cfg(unix)]
pub fn set_process_group_priority(process_group_id: u32, nice: i32) {
    if nice > 0 {
        // SAFETY: `process_group_id` comes from `Child::id()`, which returns a
        // valid OS pid for the live child; the group signalled is that child's
        // own (`process_group(0)` was set before the spawn). `setpriority` is
        // not a memory-safety operation, and its result is deliberately ignored.
        unsafe {
            libc::setpriority(libc::PRIO_PGRP, process_group_id, nice);
        }
    }
}

#[cfg(not(unix))]
pub fn set_process_group_priority(_process_group_id: u32, _nice: i32) {}

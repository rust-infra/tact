//! Process liveness and PID-reuse detection, shared by the session lock and
//! the Goal execution lease.
//!
//! Both locks persist "who holds me" and both must answer the same question
//! after a restart: is the recorded holder still that same live process? A bare
//! pid cannot answer it (pids are reused); the start-time identity is what
//! separates "the same process" from "a new process that inherited the pid".

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
use std::process::Command;

/// Prefix marking identity strings produced by *this* platform.
fn local_prefix() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "linux:"
    }
    #[cfg(target_os = "macos")]
    {
        "macos:"
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        "ps:"
    }
}

/// Opaque per-process identity used to detect PID reuse.
///
/// `None` means *unknown*, not *absent*: it covers both "no such process" and
/// "the process exists but the kernel will not describe it to us" (another
/// uid's process answers `EPERM` on macOS and `EACCES` on Linux). Callers must
/// pair it with [`is_process_alive`], which is the only thing here that can tell
/// the two apart, and must never read `None` as "the holder is dead".
pub fn process_identity(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        linux_starttime(pid).map(|t| format!("linux:{t}"))
    }
    #[cfg(target_os = "macos")]
    {
        macos_starttime(pid).map(|t| format!("macos:{t}"))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        ps_lstart(pid).map(|s| format!("ps:{s}"))
    }
}

/// Whether an identity string was produced by this platform.
///
/// A persisted row can outlive the binary that wrote it, and a database can sit
/// on a filesystem shared with another host. A foreign-platform identity cannot
/// be verified locally, so it must never be read as "the holder is dead".
pub fn is_local_identity(identity: &str) -> bool {
    identity.starts_with(local_prefix())
}

/// The current process as `(pid, identity)` for a lock or lease write.
///
/// The identity is the empty string when the platform cannot report it;
/// [`holder_is_alive`] treats that row as unverifiable rather than dead, so the
/// lock still falls back to time-based expiry.
pub fn current_process() -> (u32, String) {
    let pid = std::process::id();
    (pid, process_identity(pid).unwrap_or_default())
}

/// Whether a recorded `(pid, identity)` still denotes the same live process.
///
/// Unverifiable holders conservatively report alive: an empty or
/// foreign-platform identity cannot be checked locally, and calling it dead
/// would let this process steal a lock that a live holder still owns. Callers
/// treat `true` here as "keep waiting for expiry".
///
/// A *same-host* holder on another uid counts as unverifiable too. The identity
/// lives in kernel state that only the owner (or root) may read, so probing
/// another user's process yields `EPERM` and no start time: the holder is
/// provably alive, but its identity cannot be compared. PID reuse across that
/// boundary therefore falls back to time-based expiry rather than being called
/// dead — the safe direction, since a false "dead" breaks mutual exclusion while
/// a false "alive" only delays a takeover until the TTL lapses.
pub fn holder_is_alive(pid: u32, identity: &str) -> bool {
    if pid == 0 || identity.is_empty() || !is_local_identity(identity) {
        return true;
    }
    if !is_process_alive(pid) {
        return false;
    }
    // The process exists. A readable identity that disagrees means pid reuse;
    // an unreadable one means we are not allowed to compare at all, which is
    // not evidence that the holder is gone.
    match process_identity(pid) {
        Some(actual) => actual == identity,
        None => true,
    }
}

/// Whether `pid` exists at all, regardless of which process it is.
///
/// A live process owned by another uid is still alive: probing it returns
/// `EPERM`, not `ESRCH`. That distinction is the whole point of this function —
/// collapsing it to "the probe did not succeed" is what lets one user's runner
/// steal a lease from another user's (or root's) live runner.
pub fn is_process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        // A pid is a *positive* `pid_t`. Reject anything outside that range
        // before the cast: `u32::MAX as pid_t` is -1, and `kill(-1, 0)` means
        // "every process I am allowed to signal", which succeeds — so an
        // out-of-range value would be reported as a live process. Negative
        // pids are process groups, which is never what a recorded holder is.
        if pid > i32::MAX as u32 {
            return false;
        }
        // `kill(pid, 0)` performs the permission check and returns 0 when the
        // process exists and is signalable, EPERM when it exists but belongs to
        // another uid, and ESRCH when there is no such process. Only ESRCH is
        // evidence of absence; EPERM is positive evidence of existence.
        // SAFETY: signal 0 sends nothing; it only runs the kernel's existence
        // and permission checks. Any positive pid value is valid input.
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if result == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
    #[cfg(windows)]
    {
        use std::{ffi::c_void, ptr};

        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        // ERROR_INVALID_PARAMETER is "no such process"; ERROR_ACCESS_DENIED is
        // "exists, but not ours" and must read alive for the same reason EPERM
        // does on unix.
        const ERROR_INVALID_PARAMETER: i32 = 87;

        // SAFETY: These are standard Windows kernel32 FFI declarations with
        // well-defined behavior. OpenProcess returns null on failure (which we
        // check); CloseHandle is always safe to call with any value including null.
        unsafe extern "system" {
            safe fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
            safe fn CloseHandle(handle: *mut c_void) -> i32;
            safe fn GetLastError() -> u32;
        }

        // SAFETY: The PID argument comes from a valid u32. A null handle means
        // the call failed; GetLastError then separates "no such process" from
        // "access denied", and only the former is absence. CloseHandle releases
        // the handle when one was returned.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() || handle == ptr::null_mut() {
                return GetLastError() as i32 != ERROR_INVALID_PARAMETER;
            }
            CloseHandle(handle);
            true
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        true
    }
}

// ── Linux: read from /proc ──────────────────────────────────────

#[cfg(target_os = "linux")]
fn linux_starttime(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let end = stat.rfind(')')?;
    let fields: Vec<&str> = stat[end + 2..].split_whitespace().collect();
    fields.get(19)?.parse().ok()
}

// ── macOS: use libproc (no SIP issues, no fork) ────────────────

#[cfg(target_os = "macos")]
fn macos_starttime(pid: u32) -> Option<String> {
    const PROC_PIDTBSDINFO: i32 = 3;

    #[repr(C)]
    #[allow(non_camel_case_types)]
    struct proc_bsdinfo {
        pbi_flags: u32,
        pbi_status: u32,
        pbi_xstatus: u32,
        pbi_pid: u32,
        pbi_ppid: u32,
        pbi_uid: u32,
        pbi_gid: u32,
        pbi_ruid: u32,
        pbi_rgid: u32,
        pbi_svuid: u32,
        pbi_svgid: u32,
        rfu_1: u32,
        pbi_comm: [u8; 16],
        pbi_name: [u8; 32],
        pbi_nfiles: u32,
        pbi_pgid: u32,
        pbi_pjobc: u32,
        e_tdev: u32,
        e_tpgid: u32,
        pbi_nice: i32,
        pbi_start_tvsec: u64,
        pbi_start_tvusec: u64,
    }

    // SAFETY: proc_pidinfo is a well-documented macOS libproc function
    // (part of the system library, not going away). It reads kernel task
    // info and writes into the provided buffer — no side effects beyond
    // filling the struct.
    unsafe extern "C" {
        fn proc_pidinfo(
            pid: i32,
            flavor: i32,
            arg: u64,
            buffer: *mut proc_bsdinfo,
            buffersize: i32,
        ) -> i32;
    }

    // SAFETY: proc_bsdinfo is a POD struct containing only integers and
    // fixed-size byte arrays. All-zeroes is a valid representation for
    // every field (no enum discriminants, no NonZero, no bool, no references).
    // After the call we check ret <= 0 and return None, so unmodified zeroed
    // fields are never misinterpreted.
    let mut info: proc_bsdinfo = unsafe { std::mem::zeroed() };
    // SAFETY: info is a stack-allocated proc_bsdinfo of the exact size
    // passed as buffersize. pid is cast from u32 to i32; proc_pidinfo
    // accepts -1 for self or a valid pid, and we've guarded pid == 0
    // above. The function writes into the buffer and returns the number
    // of bytes written (or <= 0 on error).
    let ret = unsafe {
        proc_pidinfo(
            pid as i32,
            PROC_PIDTBSDINFO,
            0,
            &mut info as *mut proc_bsdinfo,
            std::mem::size_of::<proc_bsdinfo>() as i32,
        )
    };

    if ret <= 0 {
        // `None` covers both ESRCH (no such process) and EPERM (a live process
        // owned by another uid that we may not describe). Callers must not read
        // it as "gone" — see `process_identity` and `holder_is_alive`.
        return None;
    }

    Some(format!(
        "{}.{}",
        info.pbi_start_tvsec, info.pbi_start_tvusec
    ))
}

// ── Fallback (non-Linux, non-macOS): ps command ─────────────────

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn ps_lstart(pid: u32) -> Option<String> {
    let pid_str = pid.to_string();
    let output = Command::new("ps")
        .args(["-p", pid_str.as_str(), "-o", "lstart="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() { None } else { Some(value) }
}

#[cfg(test)]
mod tests {
    use super::{
        current_process, holder_is_alive, is_process_alive, local_prefix, process_identity,
    };

    /// A pid no real process can hold (Linux caps at 4 194 304, macOS at 99 998).
    const ABSENT_PID: u32 = u32::MAX;

    #[test]
    fn current_process_has_identity() {
        let (pid, identity) = current_process();
        assert_eq!(pid, std::process::id());
        assert_eq!(process_identity(pid).as_deref(), Some(identity.as_str()));
        assert!(holder_is_alive(pid, &identity));
    }

    #[test]
    fn absent_process_with_local_identity_is_dead() {
        let identity = format!("{}123.456", local_prefix());
        assert!(!holder_is_alive(ABSENT_PID, &identity));
    }

    #[test]
    fn unverifiable_holders_report_alive() {
        assert!(holder_is_alive(ABSENT_PID, ""));
        assert!(holder_is_alive(0, &format!("{}123.456", local_prefix())));
        assert!(holder_is_alive(ABSENT_PID, "foreign-platform:1"));
    }

    #[test]
    fn live_pid_with_a_different_identity_is_dead() {
        let mut child = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("spawn sleep helper");
        let pid = child.id();
        // A real start time is never the epoch: pid reuse is what this models.
        let reused_pid_identity = format!("{}0.0", local_prefix());
        let verdict = holder_is_alive(pid, &reused_pid_identity);
        let _ = child.kill();
        let _ = child.wait();
        assert!(!verdict, "a live pid with a mismatched identity is dead");
    }

    #[test]
    fn a_process_we_may_not_describe_is_alive() {
        // pid 1 belongs to root on every unix we support, so describing it takes
        // a permission this test does not have: `kill(pid, 0)` reports EPERM
        // while both the identity probe and a spawned `kill -0` subprocess only
        // see failure. Reading that as "dead" is exactly what let a runner steal
        // a lease from a live holder on another uid — the regression this test
        // exists to catch. It is portable and needs no second uid: the point is
        // that existence and describability are different questions.
        const INIT: u32 = 1;
        assert!(
            is_process_alive(INIT),
            "a live process owned by another uid is alive (EPERM is not absence)"
        );

        // And the pair must fall back to the TTL rather than claim it is gone.
        // Any identity string will do: the holder is unverifiable here.
        let holder = format!("{}irrelevant", local_prefix());
        assert!(
            holder_is_alive(INIT, &holder),
            "an unreadable identity for a live pid is 'unverifiable', so keep waiting"
        );
    }
}

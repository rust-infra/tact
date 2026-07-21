#[cfg(not(any(target_os = "linux", target_os = "macos")))]
use std::process::Command;

/// Opaque per-process identity used to detect PID reuse.
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

    unsafe extern "C" {
        fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut proc_bsdinfo, buffersize: i32) -> i32;
    }

    let mut info: proc_bsdinfo = unsafe { std::mem::zeroed() };
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
        return None;
    }

    Some(format!("{}.{}", info.pbi_start_tvsec, info.pbi_start_tvusec))
}

// ── Fallback (non-Linux, non-macOS): ps command ─────────────────

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn ps_lstart(pid: u32) -> Option<String> {
    let pid_str = pid.to_string();
    let output = Command::new("ps").args(["-p", pid_str.as_str(), "-o", "lstart="]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() { None } else { Some(value) }
}

#[cfg(test)]
mod tests {
    use super::process_identity;

    #[test]
    fn current_process_has_identity() {
        assert!(process_identity(std::process::id()).is_some());
    }
}

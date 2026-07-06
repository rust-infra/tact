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
    #[cfg(not(target_os = "linux"))]
    {
        ps_lstart(pid).map(|s| format!("ps:{s}"))
    }
}

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
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

#[cfg(target_os = "linux")]
fn linux_starttime(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let end = stat.rfind(')')?;
    let fields: Vec<&str> = stat[end + 2..].split_whitespace().collect();
    fields.get(19)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::process_identity;

    #[test]
    fn current_process_has_identity() {
        assert!(process_identity(std::process::id()).is_some());
    }
}

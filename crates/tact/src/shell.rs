//! Shared shell command validation.

use anyhow::Result;

const BLOCKED_SUBSTRINGS: &[&str] = &[
    "sudo",
    "shutdown",
    "reboot",
    "> /dev/",
    ">> /dev/",
    "rm -rf /",
    "rm -fr /",
    "rm -rf /*",
    "rm -fr /*",
    "rm -rf ~",
    "rm -fr ~",
    "rm -rf ~/",
    "rm -fr ~/",
    "rm -rf $home",
    "rm -fr $home",
];

/// Returns true for commands that must always be blocked from execution.
pub fn validate_shell_command(command: &str) -> Result<()> {
    if is_high_risk_shell_command(command) {
        anyhow::bail!("Error: Dangerous command blocked");
    }
    Ok(())
}

/// Returns true for shell commands that require explicit user approval.
pub fn is_high_risk_shell_command(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    BLOCKED_SUBSTRINGS
        .iter()
        .any(|pattern| lower.contains(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_rm_rf_root_variants() {
        for cmd in ["rm -rf /", "rm -rf /*", "rm -rf ~", "rm -fr $HOME"] {
            assert!(
                validate_shell_command(cmd).is_err(),
                "expected block: {cmd}"
            );
        }
    }

    #[test]
    fn allows_benign_commands() {
        assert!(validate_shell_command("ls -la").is_ok());
        assert!(validate_shell_command("rm -rf ./build").is_ok());
    }
}

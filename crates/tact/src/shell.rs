//! Shared shell command validation.

use anyhow::Result;

pub fn validate_shell_command(command: &str) -> Result<()> {
    let dangerous = ["rm -rf /", "sudo", "shutdown", "reboot", "> /dev/"];
    if dangerous.iter().any(|item| command.contains(item)) {
        anyhow::bail!("Error: Dangerous command blocked");
    }
    Ok(())
}

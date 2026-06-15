// Sandbox tools module
// Provides restricted file I/O and command execution capabilities,
// preventing the Agent from escaping the designated working directory.

use anyhow::{Result, anyhow};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::process::Command;

/// Sandbox struct — restricts all file and command operations within `workspace_root`.
pub struct Sandbox {
    /// Workspace root directory (original path)
    workspace_root: PathBuf,
    /// Canonicalized absolute path (used for final validation)
    canonical_root: PathBuf,
    /// Allowlist of permitted commands
    allowed_commands: Vec<String>,
}

impl Sandbox {
    pub fn new(workspace_root: PathBuf, allowed_commands: Vec<String>) -> Self {
        // canonicalize resolves symlinks to get the real absolute path
        let canonical_root = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.clone());
        Self {
            workspace_root,
            canonical_root,
            allowed_commands,
        }
    }

    /// Path safety check:
    /// 1. Filter `..` and root/prefix components to prevent directory traversal attacks
    /// 2. Resolve symlinks to prevent sandbox escape via symbolic links
    /// 3. Final validation: ensure the resolved path stays within `canonical_root`
    fn safe_path(&self, relative_path: &str) -> Result<PathBuf> {
        let path = Path::new(relative_path);
        let mut components = Vec::new();
        for comp in path.components() {
            match comp {
                std::path::Component::Normal(c) => components.push(c),
                std::path::Component::ParentDir => {
                    components.pop();
                }
                _ => {}
            }
        }
        let cleaned = components.iter().collect::<PathBuf>();
        let full = self.workspace_root.join(&cleaned);

        // Resolve symlinks to prevent sandbox escape
        let resolved = if full.exists() {
            full.canonicalize()?
        } else if let Some(parent) = full.parent() {
            if parent.exists() {
                parent
                    .canonicalize()?
                    .join(full.file_name().unwrap_or_default())
            } else {
                // Parent directory chain does not exist — fall back to prefix check
                if !full.starts_with(&self.workspace_root) {
                    return Err(anyhow!("Path escapes workspace: {}", relative_path));
                }
                return Ok(full);
            }
        } else {
            return Err(anyhow!("Invalid path: {}", relative_path));
        };

        if !resolved.starts_with(&self.canonical_root) {
            return Err(anyhow!("Path escapes workspace: {}", relative_path));
        }
        Ok(full)
    }

    /// Safely read file contents.
    pub async fn read_file(&self, path: &str) -> Result<String> {
        let full = self.safe_path(path)?;
        let content = fs::read_to_string(&full).await?;
        Ok(content)
    }

    /// Safely write a file. Creates a `.bak` backup if the file already exists;
    /// creates missing parent directories automatically.
    pub async fn write_file(&self, path: &str, content: &str) -> Result<()> {
        let full = self.safe_path(path)?;
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).await?;
        }
        if full.exists() {
            let backup = full.with_extension("bak");
            fs::copy(&full, &backup).await?;
        }
        fs::write(&full, content).await?;
        Ok(())
    }

    /// Execute a command within the sandbox working directory.
    /// Only allowlisted commands are permitted. Returns stdout or a stderr error.
    pub async fn run_command(&self, cmd: &str, args: &[&str]) -> Result<String> {
        let base_cmd = cmd.split_whitespace().next().unwrap_or(cmd);
        if !self.allowed_commands.contains(&base_cmd.to_string()) {
            return Err(anyhow!("Command not allowed: {}", base_cmd));
        }

        let output = Command::new(cmd)
            .args(args)
            .current_dir(&self.workspace_root)
            .output()
            .await?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(anyhow!("Command failed: {}", stderr))
        }
    }
}

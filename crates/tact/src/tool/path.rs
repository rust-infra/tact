//! Workspace path validation — prevents path-escape outside the work directory.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub fn safe_path(work_dir: &Path, path: &str) -> Result<PathBuf> {
    resolve_safe_path(work_dir, path, false)
}

pub fn safe_path_allow_missing(work_dir: &Path, path: &str) -> Result<PathBuf> {
    resolve_safe_path(work_dir, path, true)
}

fn resolve_safe_path(work_dir: &Path, path: &str, allow_missing: bool) -> Result<PathBuf> {
    let work_dir = work_dir.canonicalize()?;
    let candidate = work_dir.join(path);

    let full = if candidate.exists() || !allow_missing {
        candidate.canonicalize()?
    } else {
        let parent = candidate
            .parent()
            .context("Path has no parent")?
            .canonicalize()?;

        if !parent.starts_with(&work_dir) {
            return Err(anyhow::anyhow!("Path escapes workspace"));
        }

        let file_name = candidate.file_name().context("Path has no file name")?;

        parent.join(file_name)
    };

    if !full.starts_with(&work_dir) {
        return Err(anyhow::anyhow!("Path escapes workspace"));
    }

    Ok(full)
}

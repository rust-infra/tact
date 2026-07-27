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

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs as unix_fs};

    use super::*;

    fn setup_workspace() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let work_dir = dir.path().canonicalize().unwrap();

        // Create a file inside the workspace
        fs::write(work_dir.join("inside.txt"), "hello").unwrap();

        // Create a subdirectory with a file
        fs::create_dir_all(work_dir.join("sub")).unwrap();
        fs::write(work_dir.join("sub/nested.txt"), "nested").unwrap();

        (dir, work_dir)
    }

    #[test]
    fn resolves_relative_path_inside_workspace() {
        let (_dir, work_dir) = setup_workspace();
        let result = safe_path(&work_dir, "inside.txt").unwrap();
        assert_eq!(result, work_dir.join("inside.txt"));
    }

    #[test]
    fn resolves_path_in_subdirectory() {
        let (_dir, work_dir) = setup_workspace();
        let result = safe_path(&work_dir, "sub/nested.txt").unwrap();
        assert_eq!(result, work_dir.join("sub/nested.txt"));
    }

    #[test]
    fn allows_dot_dot_that_stays_inside_workspace() {
        let (_dir, work_dir) = setup_workspace();
        // sub/../inside.txt  →  inside.txt
        let result = safe_path(&work_dir, "sub/../inside.txt").unwrap();
        assert_eq!(result, work_dir.join("inside.txt"));
    }

    #[test]
    fn rejects_dot_dot_escape() {
        let (_dir, work_dir) = setup_workspace();
        let result = safe_path(&work_dir, "../outside.txt");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        // On some platforms canonicalize fails with "No such file" before
        // reaching the workspace check. Either error is acceptable.
        assert!(
            err.contains("escapes workspace") || err.contains("No such file"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn rejects_multiple_dot_dot_escape() {
        let (_dir, work_dir) = setup_workspace();
        let result = safe_path(&work_dir, "../../etc/passwd");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("escapes workspace") || err.contains("No such file"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn rejects_absolute_path_outside_workspace() {
        let (_dir, work_dir) = setup_workspace();
        let result = safe_path(&work_dir, "/etc/passwd");
        assert!(result.is_err());
    }

    #[test]
    fn safe_path_errors_on_nonexistent_file() {
        let (_dir, work_dir) = setup_workspace();
        let result = safe_path(&work_dir, "nonexistent.txt");
        assert!(result.is_err());
    }

    #[test]
    fn safe_path_allow_missing_succeeds_on_nonexistent_file() {
        let (_dir, work_dir) = setup_workspace();
        let result = safe_path_allow_missing(&work_dir, "nonexistent.txt").unwrap();
        assert_eq!(result, work_dir.join("nonexistent.txt"));
    }

    #[test]
    fn safe_path_allow_missing_rejects_escape() {
        let (_dir, work_dir) = setup_workspace();
        let result = safe_path_allow_missing(&work_dir, "../outside.txt");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("escapes workspace") || err.contains("No such file"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn safe_path_allow_missing_allows_nonexistent_in_subdir() {
        let (_dir, work_dir) = setup_workspace();
        let result = safe_path_allow_missing(&work_dir, "sub/nonexistent.txt").unwrap();
        assert_eq!(result, work_dir.join("sub/nonexistent.txt"));
    }

    #[test]
    fn safe_path_allow_missing_rejects_escape_via_nonexistent_parent() {
        let (_dir, work_dir) = setup_workspace();
        let result = safe_path_allow_missing(&work_dir, "nonexistent_dir/../outside.txt");
        assert!(result.is_err());
    }

    #[cfg(target_family = "unix")]
    #[test]
    fn rejects_symlink_escape() {
        let (_dir, work_dir) = setup_workspace();

        // Create a file outside the workspace
        let outside = std::env::temp_dir().join("tact-path-test-outside.txt");
        fs::write(&outside, "outside").unwrap();

        // Create a symlink inside workspace pointing outside
        let symlink_path = work_dir.join("escape_link");
        unix_fs::symlink(&outside, &symlink_path).unwrap();

        let result = safe_path(&work_dir, "escape_link");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("escapes workspace") || err.contains("No such file"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn dot_path_resolves_to_work_dir() {
        let (_dir, work_dir) = setup_workspace();
        let result = safe_path(&work_dir, ".").unwrap();
        assert_eq!(result, work_dir);
    }
}

// ApplyPatch tool: apply a unified diff patch to files.
//
// Parses standard unified diff format (as produced by `git diff` or `diff -u`).
// Supports dry_run mode which validates the patch without writing any files.

use crate::tool::{ToolContext, safe_path};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::PathBuf;
use tool_refactor_macros::tool;
use tracing::debug;

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ApplyPatchInput {
    #[schemars(description = "Unified diff patch to apply.")]
    pub patch: String,
    #[schemars(description = "If true, validate without writing files.")]
    #[serde(default)]
    pub dry_run: bool,
}

// ---------------------------------------------------------------------------
// Internal diff representation
// ---------------------------------------------------------------------------

/// A single `@@` hunk within a file diff.
#[derive(Debug)]
struct Hunk {
    /// Starting line in the *original* file (0-based index).
    orig_start: usize,
    /// Lines in this hunk: `' '` = context, `'-'` = remove, `'+'` = add.
    lines: Vec<(char, String)>,
}

/// All hunks for a single file.
#[derive(Debug)]
struct FilePatch {
    /// Target path (from `+++ b/<path>` or `+++ <path>`).
    path: String,
    hunks: Vec<Hunk>,
}

// ---------------------------------------------------------------------------
// Unified diff parser
// ---------------------------------------------------------------------------

/// Parse a unified diff string into a list of `FilePatch` objects.
fn parse_unified_diff(patch: &str) -> Result<Vec<FilePatch>, String> {
    let mut file_patches: Vec<FilePatch> = Vec::new();
    let mut current_file: Option<FilePatch> = None;
    let mut current_hunk: Option<Hunk> = None;

    for line in patch.lines() {
        if line.starts_with("--- ") {
            // Start of a new file section; finalise previous hunk/file.
            if let Some(h) = current_hunk.take() {
                if let Some(ref mut f) = current_file {
                    f.hunks.push(h);
                }
            }
            if let Some(f) = current_file.take() {
                file_patches.push(f);
            }
        } else if line.starts_with("+++ ") {
            // Extract target path, stripping the "b/" prefix if present.
            let raw = &line[4..];
            let path = raw.trim_start_matches("b/").trim().to_string();
            current_file = Some(FilePatch {
                path,
                hunks: Vec::new(),
            });
        } else if line.starts_with("@@ ") {
            // Finalise the previous hunk.
            if let Some(h) = current_hunk.take() {
                if let Some(ref mut f) = current_file {
                    f.hunks.push(h);
                }
            }
            let orig_start = parse_hunk_header(line)?;
            current_hunk = Some(Hunk {
                orig_start,
                lines: Vec::new(),
            });
        } else if let Some(ref mut hunk) = current_hunk {
            if line.starts_with('+') {
                hunk.lines.push(('+', line[1..].to_string()));
            } else if line.starts_with('-') {
                hunk.lines.push(('-', line[1..].to_string()));
            } else if line.starts_with(' ') {
                hunk.lines.push((' ', line[1..].to_string()));
            } else if line.starts_with('\\') {
                // "\ No newline at end of file" — ignore.
            }
        }
    }

    // Flush remaining hunk / file.
    if let Some(h) = current_hunk.take() {
        if let Some(ref mut f) = current_file {
            f.hunks.push(h);
        }
    }
    if let Some(f) = current_file.take() {
        file_patches.push(f);
    }

    Ok(file_patches)
}

/// Parse `@@ -<orig_start>,<count> +<new_start>,<count> @@` to get orig_start (0-based).
fn parse_hunk_header(line: &str) -> Result<usize, String> {
    let ranges = line
        .split("@@")
        .nth(1)
        .ok_or_else(|| format!("invalid hunk header: {}", line))?
        .trim();
    let orig_range = ranges
        .split_whitespace()
        .next()
        .ok_or_else(|| format!("invalid hunk header: {}", line))?;
    // orig_range is "-l,s" or "-l"
    let orig_start_str = orig_range
        .trim_start_matches('-')
        .split(',')
        .next()
        .ok_or_else(|| format!("invalid hunk header: {}", line))?;
    let start: isize = orig_start_str
        .parse()
        .map_err(|_| format!("invalid hunk header: {}", line))?;
    // orig_start is 1-based in unified diff; convert to 0-based.
    if start <= 0 {
        Ok(0)
    } else {
        Ok((start - 1) as usize)
    }
}

// ---------------------------------------------------------------------------
// Hunk application
// ---------------------------------------------------------------------------

/// Apply a single hunk to `lines`.  Returns the modified lines on success,
/// or an error string when context lines don't match.
fn apply_hunk(lines: Vec<String>, hunk: &Hunk) -> Result<Vec<String>, String> {
    let hunk_lines = &hunk.lines;
    let orig_start = hunk.orig_start;

    // Build expected context: lines that are ' ' or '-' in the hunk.
    let mut expected: Vec<&str> = Vec::new();
    let mut new_lines: Vec<String> = Vec::new();
    let mut context_positions: Vec<usize> = Vec::new(); // indices in `lines` being matched

    for (kind, content) in hunk_lines {
        match kind {
            ' ' | '-' => {
                expected.push(content);
                context_positions.push(orig_start + expected.len() - 1);
            }
            _ => {}
        }
    }

    if orig_start + expected.len() > lines.len() {
        return Err(format!(
            "patch hunk extends past end of file (orig_start={}, expected={}, file_lines={})",
            orig_start,
            expected.len(),
            lines.len()
        ));
    }

    // Verify context matches.
    for (i, exp) in expected.iter().enumerate() {
        if lines[orig_start + i] != **exp {
            return Err(format!(
                "context mismatch at line {}: expected '{}', got '{}'",
                orig_start + i + 1,
                exp,
                lines[orig_start + i]
            ));
        }
    }

    // Build result: copy lines before hunk, then apply.
    new_lines.extend_from_slice(&lines[..orig_start]);

    for (kind, content) in hunk_lines {
        match kind {
            ' ' | '+' => new_lines.push(content.clone()),
            '-' => {} // skip removed lines
            _ => {}
        }
    }

    // Copy lines after hunk (skip the lines consumed by the hunk).
    let consumed = expected.len();
    new_lines.extend_from_slice(&lines[orig_start + consumed..]);

    Ok(new_lines)
}

// ---------------------------------------------------------------------------
// Tool implementation
// ---------------------------------------------------------------------------

#[tool(
    name = "apply_patch",
    description = "Apply a unified diff patch to files. Accepts standard unified diff \
                    format. Use dry_run=true to validate without modifying files."
)]
pub async fn apply_patch(ctx: ToolContext, input: ApplyPatchInput) -> Result<String> {
    debug!(dry_run = input.dry_run, "Applying patch");

    // Parse the patch.
    let file_patches = parse_unified_diff(&input.patch)
        .map_err(|e| anyhow::anyhow!("Failed to parse patch: {}", e))?;

    if file_patches.is_empty() {
        return Err(anyhow::anyhow!("No files found in patch"));
    }

    let mut total_added = 0usize;
    let mut total_removed = 0usize;
    let mut file_summaries: Vec<serde_json::Value> = Vec::new();
    let mut to_write: Vec<(PathBuf, Vec<u8>, String)> = Vec::new();

    for fp in &file_patches {
        let path = safe_path(&ctx.work_dir, &fp.path)
            .map_err(|e| anyhow::anyhow!("Invalid path '{}': {}", fp.path, e))?;

        let original_content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| anyhow::anyhow!("Cannot read {}: {}", path.display(), e))?;

        let mut lines: Vec<String> = original_content.lines().map(|s| s.to_string()).collect();
        let mut file_added = 0usize;
        let mut file_removed = 0usize;

        for hunk in &fp.hunks {
            for (kind, _) in &hunk.lines {
                match kind {
                    '+' => {
                        file_added += 1;
                        total_added += 1;
                    }
                    '-' => {
                        file_removed += 1;
                        total_removed += 1;
                    }
                    _ => {}
                }
            }
            lines = apply_hunk(lines, hunk)
                .map_err(|e| anyhow::anyhow!("Hunk failed in {}: {}", fp.path, e))?;
        }

        let new_content = lines.join("\n");

        file_summaries.push(serde_json::json!({
            "path": fp.path,
            "hunks": fp.hunks.len(),
            "lines_added": file_added,
            "lines_removed": file_removed,
        }));

        to_write.push((path, original_content.into_bytes(), new_content));
    }

    // Dry-run: return summary without writing
    if input.dry_run {
        return Ok(format!(
            "Dry run: patch would modify {} file(s) (+{} -{} lines).",
            to_write.len(),
            total_added,
            total_removed,
        ));
    }

    // Write all modified files
    for (path, _original_bytes, new_content) in &to_write {
        tokio::fs::write(path, new_content)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to write {}: {}", path.display(), e))?;
    }

    Ok(format!(
        "Applied patch to {} file(s) (+{} -{} lines).",
        to_write.len(),
        total_added,
        total_removed,
    ))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hunk_header() {
        assert_eq!(parse_hunk_header("@@ -12,5 +12,6 @@").unwrap(), 11);
        assert_eq!(parse_hunk_header("@@ -1,3 +1,4 @@ fn foo()").unwrap(), 0);
        assert_eq!(parse_hunk_header("@@ -0,0 +1 @@").unwrap(), 0);
    }

    #[test]
    fn test_apply_hunk_simple() {
        let lines: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        let hunk = Hunk {
            orig_start: 1,
            lines: vec![(' ', "b".into()), ('-', "c".into()), ('+', "C".into())],
        };
        let result = apply_hunk(lines, &hunk).unwrap();
        assert_eq!(result, vec!["a", "b", "C"]);
    }

    #[test]
    fn test_apply_hunk_context_mismatch() {
        let lines: Vec<String> = vec!["x".into(), "y".into()];
        let hunk = Hunk {
            orig_start: 0,
            lines: vec![('-', "z".into())],
        };
        assert!(apply_hunk(lines, &hunk).is_err());
    }

    #[test]
    fn test_parse_unified_diff_basic() {
        let patch = "\
--- a/foo.txt
+++ b/foo.txt
@@ -1,2 +1,2 @@
 hello
-world
+rust
";
        let fps = parse_unified_diff(patch).unwrap();
        assert_eq!(fps.len(), 1);
        assert_eq!(fps[0].path, "foo.txt");
        assert_eq!(fps[0].hunks.len(), 1);
        let hunk = &fps[0].hunks[0];
        assert_eq!(hunk.orig_start, 0);
        assert_eq!(hunk.lines.len(), 3);
    }

    #[test]
    fn test_parse_unified_diff_two_files() {
        let patch = "\
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old
+new
--- a/b.rs
+++ b/b.rs
@@ -1 +1 @@
-foo
+bar
";
        let fps = parse_unified_diff(patch).unwrap();
        assert_eq!(fps.len(), 2);
        assert_eq!(fps[0].path, "a.rs");
        assert_eq!(fps[1].path, "b.rs");
    }
}

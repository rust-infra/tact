// BatchEdit tool: apply multiple file edits atomically.
//
// All edits are validated before any change is written.  If any pre-check
// fails the tool returns an error and leaves every file untouched.

use std::collections::BTreeMap;

use crate::tool::{ToolContext, safe_path};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use tool_refactor_macros::tool;
use tracing::debug;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SingleEdit {
    #[schemars(description = "Absolute or workspace-relative path to the file.")]
    pub file_path: String,
    #[schemars(
        description = "Exact text to replace. Must occur exactly once in the file's original \
                       content. For multiple edits on the same file, old_string values must not \
                       overlap or nest (e.g. do not combine \"alpha beta\" and \"beta\")."
    )]
    pub old_string: String,
    #[schemars(description = "Replacement text.")]
    pub new_string: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BatchEditInput {
    #[schemars(
        description = "Edits to apply atomically. All old_string values are validated against \
                       the original file contents before any write. Same-file edits must use \
                       non-overlapping, uniquely matching old_string values; chained edits that \
                       depend on prior replacements belong in edit_file or a single merged edit."
    )]
    pub edits: Vec<SingleEdit>,
    #[schemars(description = "Optional human-readable description of what this batch edit does.")]
    #[serde(default)]
    #[allow(dead_code)]
    pub description: Option<String>,
}

#[tool(
    name = "batch_edit",
    description = "Apply multiple file edits atomically. All edits are validated against the \
                    original file contents before any file is modified; if any edit fails, the \
                    entire batch is rejected and no files are changed. Rules: each old_string \
                    must match exactly once; same-file edits must not overlap or nest (e.g. \
                    \"alpha beta\" and \"beta\" together); edits on one file cannot depend on \
                    another edit's new_string—use edit_file sequentially or merge into one edit \
                    instead."
)]
pub async fn batch_edit(ctx: ToolContext, input: BatchEditInput) -> Result<String> {
    if input.edits.is_empty() {
        return Err(anyhow::anyhow!("edits array must not be empty"));
    }

    // Phase 1: read all files, validate every edit, and group edits by file.
    //
    // For each file we keep the original content and a list of (byte_offset,
    // old_string, new_string) tuples.  Byte offsets are used only for stable
    // ordering in Phase 2 — applying edits from end to start so that earlier
    // targets are not displaced.
    let mut files: BTreeMap<String, (String, Vec<(usize, String, String)>)> = BTreeMap::new();
    let mut pre_check_errors: Vec<String> = Vec::new();

    for (i, edit) in input.edits.iter().enumerate() {
        let path = match safe_path(&ctx.work_dir, &edit.file_path) {
            Ok(p) => p,
            Err(e) => {
                pre_check_errors.push(format!(
                    "Edit {}: invalid path {}: {}",
                    i, edit.file_path, e
                ));
                continue;
            }
        };
        debug!(path = %path.display(), index = i, "BatchEdit pre-check");

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => {
                pre_check_errors.push(format!(
                    "Edit {}: cannot read {}: {}",
                    i, path.display(), e
                ));
                continue;
            }
        };

        let count = content.matches(&edit.old_string).count();
        if count == 0 {
            pre_check_errors.push(format!(
                "Edit {}: old_string not found in {}",
                i,
                path.display()
            ));
            continue;
        }
        if count > 1 {
            pre_check_errors.push(format!(
                "Edit {}: old_string appears {} times in {} (must be unique)",
                i,
                count,
                path.display()
            ));
            continue;
        }

        let pos = content.find(&edit.old_string).unwrap();
        let path_str = path.display().to_string();
        let entry = files
            .entry(path_str)
            .or_insert_with(|| (content.clone(), Vec::new()));
        debug_assert_eq!(
            entry.0, content,
            "original content should be identical for the same file"
        );
        entry
            .1
            .push((pos, edit.old_string.clone(), edit.new_string.clone()));
    }

    if !pre_check_errors.is_empty() {
        return Err(anyhow::anyhow!(
            "BatchEdit aborted — {} validation error(s):\n{}",
            pre_check_errors.len(),
            pre_check_errors.join("\n")
        ));
    }

    // Phase 2: for each file, apply edits sorted by descending byte offset
    // (end → start), and write each file exactly once.
    let file_count = files.len();
    let mut total_edits = 0usize;
    let mut per_file: Vec<(String, String, String)> = Vec::with_capacity(files.len());

    for (path_str, (original, mut edits)) in files {
        // Sort descending by position: apply later-position edits first so
        // that earlier targets are not shifted.
        edits.sort_by(|a, b| b.0.cmp(&a.0));

        let mut content = original.clone();
        for (_pos, old, new) in &edits {
            // Safety net: if a previous edit accidentally consumed old_string,
            // this is a user error — detect and abort.
            if !content.contains(old.as_str()) {
                pre_check_errors.push(format!(
                    "File {}: old_string {:?} was consumed by a prior edit in \
                     the same batch (edits that overlap are not supported)",
                    path_str, old
                ));
                break;
            }
            content = content.replacen(old, new, 1);
        }

        total_edits += edits.len();
        per_file.push((path_str, original, content));
    }

    if !pre_check_errors.is_empty() {
        return Err(anyhow::anyhow!(
            "BatchEdit aborted — {} validation error(s):\n{}",
            pre_check_errors.len(),
            pre_check_errors.join("\n")
        ));
    }

    // Write every file concurrently; roll back on any failure.
    let handles: Vec<_> = per_file
        .into_iter()
        .map(|(path_str, original, final_content)| {
            tokio::spawn(async move {
                let path = std::path::Path::new(&path_str);
                match tokio::fs::write(path, &final_content).await {
                    Ok(()) => Ok((path_str, original)),
                    Err(e) => Err((path_str, original, e)),
                }
            })
        })
        .collect();

    let results = futures_util::future::join_all(handles).await;

    let mut written: Vec<(String, String)> = Vec::new();
    let mut first_error: Option<(String, std::io::Error)> = None;

    for result in results {
        // tokio::spawn JoinError only occurs on task panic — propagate it.
        match result.map_err(|e| anyhow::anyhow!("BatchEdit task panicked: {e}"))? {
            Ok(pair) => written.push(pair),
            Err((path_str, _original, e)) => {
                if first_error.is_none() {
                    first_error = Some((path_str, e));
                }
            }
        }
    }

    if let Some((failed_path, e)) = first_error {
        // Rollback already-written files asynchronously.
        let mut rollback_errors: Vec<String> = Vec::new();
        for (rb_path, rb_original) in &written {
            if let Err(re) = tokio::fs::write(rb_path, rb_original).await {
                rollback_errors.push(format!("  rollback {}: {}", rb_path, re));
            }
        }

        let mut msg = format!(
            "BatchEdit failed while writing {} ({}). Rolled back {} file(s).",
            failed_path,
            e,
            written.len()
        );
        if !rollback_errors.is_empty() {
            msg.push_str(&format!(
                "\nRollback errors:\n{}",
                rollback_errors.join("\n")
            ));
        }
        return Err(anyhow::anyhow!("{}", msg));
    }

    Ok(format!(
        "BatchEdit applied {} edit{} across {} file{}.",
        total_edits,
        if total_edits != 1 { "s" } else { "" },
        file_count,
        if file_count != 1 { "s" } else { "" },
    ))
}

#[cfg(test)]
mod tests {
    use crate::tool::{test_support::{test_context, write_workspace_file}, ToolRouter};

    use super::*;

    async fn run_batch_edit(
        context: &ToolContext,
        edits: serde_json::Value,
    ) -> Result<String> {
        ToolRouter::new()
            .route(BatchEditTool)
            .call(context, "batch_edit", serde_json::json!({ "edits": edits }))
            .await
    }

    #[tokio::test]
    async fn batch_edit_rejects_empty_edits() {
        let context = test_context("batch_edit_rejects_empty_edits");

        let error = run_batch_edit(&context, serde_json::json!([]))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("edits array must not be empty"));
    }

    #[tokio::test]
    async fn batch_edit_leaves_files_unchanged_on_validation_failure() {
        let context = test_context("batch_edit_leaves_files_unchanged_on_validation_failure");
        write_workspace_file(&context.work_dir, "one.txt", "keep one");
        write_workspace_file(&context.work_dir, "two.txt", "keep two");

        let error = run_batch_edit(
            &context,
            serde_json::json!([
                {
                    "file_path": "one.txt",
                    "old_string": "keep one",
                    "new_string": "changed one"
                },
                {
                    "file_path": "two.txt",
                    "old_string": "missing",
                    "new_string": "changed two"
                }
            ]),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("BatchEdit aborted"));
        assert!(error.to_string().contains("old_string not found"));
        assert_eq!(
            std::fs::read_to_string(context.work_dir.join("one.txt")).unwrap(),
            "keep one"
        );
        assert_eq!(
            std::fs::read_to_string(context.work_dir.join("two.txt")).unwrap(),
            "keep two"
        );
    }

    #[tokio::test]
    async fn batch_edit_rejects_non_unique_old_string() {
        let context = test_context("batch_edit_rejects_non_unique_old_string");
        write_workspace_file(&context.work_dir, "dup.txt", "same same");

        let error = run_batch_edit(
            &context,
            serde_json::json!([{
                "file_path": "dup.txt",
                "old_string": "same",
                "new_string": "changed"
            }]),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("must be unique"));
        assert_eq!(
            std::fs::read_to_string(context.work_dir.join("dup.txt")).unwrap(),
            "same same"
        );
    }

    #[tokio::test]
    async fn batch_edit_applies_multiple_edits_to_same_file() {
        let context = test_context("batch_edit_applies_multiple_edits_to_same_file");
        write_workspace_file(&context.work_dir, "multi.txt", "aaa bbb ccc");

        let output = run_batch_edit(
            &context,
            serde_json::json!([
                {
                    "file_path": "multi.txt",
                    "old_string": "aaa",
                    "new_string": "AAA"
                },
                {
                    "file_path": "multi.txt",
                    "old_string": "bbb",
                    "new_string": "BBB"
                }
            ]),
        )
        .await
        .unwrap();

        assert!(output.contains("2 edits across 1 file"));
        assert_eq!(
            std::fs::read_to_string(context.work_dir.join("multi.txt")).unwrap(),
            "AAA BBB ccc"
        );
    }

    #[tokio::test]
    async fn batch_edit_rejects_overlapping_edits_in_same_file() {
        let context = test_context("batch_edit_rejects_overlapping_edits_in_same_file");
        write_workspace_file(&context.work_dir, "overlap.txt", "alpha beta gamma");

        let error = run_batch_edit(
            &context,
            serde_json::json!([
                {
                    "file_path": "overlap.txt",
                    "old_string": "alpha beta",
                    "new_string": "ALPHA"
                },
                {
                    "file_path": "overlap.txt",
                    "old_string": "beta",
                    "new_string": "BETA"
                }
            ]),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("BatchEdit aborted"));
        assert!(error.to_string().contains("consumed by a prior edit"));
        assert_eq!(
            std::fs::read_to_string(context.work_dir.join("overlap.txt")).unwrap(),
            "alpha beta gamma"
        );
    }
}

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
    #[schemars(description = "Text to replace (must occur exactly once in the file).")]
    pub old_string: String,
    #[schemars(description = "Replacement text.")]
    pub new_string: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BatchEditInput {
    #[schemars(description = "List of edits to apply atomically.")]
    pub edits: Vec<SingleEdit>,
    #[schemars(description = "Optional human-readable description of what this batch edit does.")]
    #[serde(default)]
    #[allow(dead_code)]
    pub description: Option<String>,
}

#[tool(
    name = "batch_edit",
    description = "Apply multiple file edits atomically. All edits are validated before any \
                    file is modified. If any edit would fail (old_string not found or not \
                    unique) the entire batch is rejected with no changes made."
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

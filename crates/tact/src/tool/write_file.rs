use std::time::{Duration, Instant};

use crate::tool::{
    ArgumentSummaryPolicy, DetailPolicy, LiveOutputPolicy, OutputPolicy, PermissionPolicy,
    PermissionPromptPolicy, PopupPolicy, ResourcePolicy, ToolDomain, ToolMetadata,
    ToolPresentation,
};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use tact_protocol::ToolVisualKind;
use tact_protocol::{AgentUpdate, format_bytes};
use tokio::{fs, io::AsyncWriteExt};
use tool_refactor_macros::tool;

use crate::tool::{ToolContext, safe_path_allow_missing};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WriteFileInput {
    #[schemars(
        description = "**Required.** Path to the file to write, relative to the current workspace. Must not be empty."
    )]
    pub path: String,
    #[schemars(
        description = "File content. Keep one call to roughly 500 lines / 20 KB. \
                       For a longer file, write the first chunk with `append=false`, \
                       then add each further chunk with `append=true` on the same \
                       path. A call that exceeds the model's output budget is \
                       dropped whole — no file is created and nothing already \
                       written is kept — so chunk a long file instead of widening \
                       one call."
    )]
    pub content: String,
    #[schemars(
        description = "Append to the file instead of replacing it. Default false: \
                       the write truncates any existing file. Set true for every \
                       chunk after the first when building a file across calls."
    )]
    #[serde(default)]
    pub append: bool,
}

/// Chunk size for incremental file writes. Tokio's async file I/O runs on the
/// blocking pool; smaller chunks let us emit progress updates without blocking
/// the executor for too long.
const WRITE_CHUNK_SIZE: usize = 64 * 1024;
/// Files smaller than this are written in a single operation to avoid the
/// overhead of chunking and progress tracking.
const SINGLE_WRITE_THRESHOLD: usize = 256 * 1024;

pub const WRITE_FILE_METADATA: ToolMetadata = ToolMetadata {
    name: "write_file",
    description: "Write or append a file. Keep one call small (~500 lines / 20 KB); \
                  build a longer file by writing the first chunk, then appending the \
                  rest with `append=true`.",
    permission: PermissionPolicy::WritePath { path_field: "path" },
    permission_prompt: PermissionPromptPolicy::Path { field: "path" },
    resources: ResourcePolicy::WritePath { field: "path" },
    domain: ToolDomain::Generic,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::FileWrite,
        display_name: "📝 Write",
        live_output: LiveOutputPolicy::Standard,
        detail: DetailPolicy::InputField("content"),
        popup: PopupPolicy::None,
        compact_result_to_meta: false,
    },
    output: OutputPolicy::KeepInline,
    argument_summary: ArgumentSummaryPolicy::Path { field: "path" },
};

#[tool]
/// # Errors
///
/// Returns an error if:
/// - The file path is invalid or escapes the workspace.
/// - The parent directories cannot be created.
/// - The file cannot be written or flushed (I/O error).
pub async fn write_file(ctx: ToolContext, input: WriteFileInput) -> Result<String> {
    let path = safe_path_allow_missing(&ctx.work_dir, &input.path)?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| anyhow::anyhow!("Error creating parent directories: {}", e))?;
    }

    let total = input.content.len();
    let bytes = input.content.as_bytes();
    let line_count = input.content.lines().count();

    // The mode is the whole difference between the two calls: `append` keeps
    // what is already on disk (and creates the file when missing), the default
    // truncates. Everything after the open is identical, so the progress loop
    // below has one implementation rather than one per mode.
    let mut file = if input.append {
        fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .await
            .map_err(|e| anyhow::anyhow!("Error opening file: {}", e))?
    } else {
        fs::File::create(&path)
            .await
            .map_err(|e| anyhow::anyhow!("Error creating file: {}", e))?
    };

    if total <= SINGLE_WRITE_THRESHOLD {
        file.write_all(bytes)
            .await
            .map_err(|e| anyhow::anyhow!("Error writing file: {}", e))?;
        file.flush()
            .await
            .map_err(|e| anyhow::anyhow!("Error flushing file: {}", e))?;
    } else {
        let mut written = 0usize;
        let mut next_milestone = total / 10;
        let mut last_update = Instant::now();
        let update_interval = Duration::from_millis(200);

        for chunk in bytes.chunks(WRITE_CHUNK_SIZE) {
            file.write_all(chunk)
                .await
                .map_err(|e| anyhow::anyhow!("Error writing file: {}", e))?;
            written += chunk.len();

            if written < total {
                let now = Instant::now();
                let time_elapsed = now.duration_since(last_update) >= update_interval;
                let milestone_reached = written >= next_milestone;

                if milestone_reached || time_elapsed {
                    let pct = (written * 100 / total) as u64;
                    ctx.events.send(AgentUpdate::Info(format!(
                        "Writing {}... {}% ({} / {})",
                        path.display(),
                        pct,
                        format_bytes(written),
                        format_bytes(total)
                    )));
                    last_update = now;
                    if milestone_reached {
                        next_milestone += total / 10;
                        if next_milestone > total {
                            next_milestone = total;
                        }
                    }
                }
            }
        }

        file.flush()
            .await
            .map_err(|e| anyhow::anyhow!("Error flushing file: {}", e))?;
    }

    // The verb carries the mode: "Wrote" over an existing file and "Appended"
    // differ in what survived, and a model that cannot tell them apart may
    // re-issue a write that silently drops the earlier chunk.
    let verb = if input.append { "Appended" } else { "Wrote" };
    Ok(format!(
        "{verb} {} / {} lines to {}",
        format_bytes(total),
        line_count,
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::test_support::{run_tool, test_context};

    #[tokio::test]
    async fn write_file_writes_into_existing_subdirectory() {
        let context = test_context("write_file_writes_into_existing_subdirectory");
        std::fs::create_dir_all(context.work_dir.join("nested/dir")).unwrap();

        run_tool(
            &context,
            WriteFileTool,
            "write_file",
            serde_json::json!({
                "path": "nested/dir/file.txt",
                "content": "nested content\n"
            }),
        )
        .await
        .unwrap();

        let written =
            std::fs::read_to_string(context.work_dir.join("nested/dir/file.txt")).unwrap();
        assert_eq!(written, "nested content\n");
    }

    #[tokio::test]
    async fn append_keeps_the_content_already_on_disk() {
        let context = test_context("append_keeps_the_content_already_on_disk");

        run_tool(
            &context,
            WriteFileTool,
            "write_file",
            serde_json::json!({ "path": "chunks.txt", "content": "first\n" }),
        )
        .await
        .unwrap();
        let output = run_tool(
            &context,
            WriteFileTool,
            "write_file",
            serde_json::json!({ "path": "chunks.txt", "content": "second\n", "append": true }),
        )
        .await
        .unwrap();

        assert!(output.starts_with("Appended"), "got {output}");
        let written = std::fs::read_to_string(context.work_dir.join("chunks.txt")).unwrap();
        assert_eq!(written, "first\nsecond\n");
    }

    #[tokio::test]
    async fn a_default_write_replaces_the_file_instead_of_appending() {
        let context = test_context("a_default_write_replaces_the_file_instead_of_appending");

        run_tool(
            &context,
            WriteFileTool,
            "write_file",
            serde_json::json!({ "path": "replace.txt", "content": "old\n" }),
        )
        .await
        .unwrap();
        let output = run_tool(
            &context,
            WriteFileTool,
            "write_file",
            serde_json::json!({ "path": "replace.txt", "content": "new\n" }),
        )
        .await
        .unwrap();

        assert!(output.starts_with("Wrote"), "got {output}");
        let written = std::fs::read_to_string(context.work_dir.join("replace.txt")).unwrap();
        assert_eq!(written, "new\n");
    }

    #[tokio::test]
    async fn append_creates_the_file_when_it_is_missing() {
        let context = test_context("append_creates_the_file_when_it_is_missing");

        let output = run_tool(
            &context,
            WriteFileTool,
            "write_file",
            serde_json::json!({ "path": "fresh.txt", "content": "only\n", "append": true }),
        )
        .await
        .unwrap();

        assert!(output.starts_with("Appended"), "got {output}");
        let written = std::fs::read_to_string(context.work_dir.join("fresh.txt")).unwrap();
        assert_eq!(written, "only\n");
    }

    /// The streaming branch has its own open call, so append has to be proven
    /// there too: a `File::create` slipping through would truncate the earlier
    /// chunk only for files above `SINGLE_WRITE_THRESHOLD`.
    #[tokio::test]
    async fn append_keeps_the_file_across_the_streaming_branch() {
        let context = test_context("append_keeps_the_file_across_the_streaming_branch");
        let first = "a".repeat(SINGLE_WRITE_THRESHOLD + 1);
        let second = "b".repeat(SINGLE_WRITE_THRESHOLD + 1);

        run_tool(
            &context,
            WriteFileTool,
            "write_file",
            serde_json::json!({ "path": "large.txt", "content": first }),
        )
        .await
        .unwrap();
        run_tool(
            &context,
            WriteFileTool,
            "write_file",
            serde_json::json!({ "path": "large.txt", "content": second, "append": true }),
        )
        .await
        .unwrap();

        let written = std::fs::read_to_string(context.work_dir.join("large.txt")).unwrap();
        assert_eq!(written.len(), first.len() + second.len());
        assert!(written.starts_with('a') && written.ends_with('b'));
    }
}

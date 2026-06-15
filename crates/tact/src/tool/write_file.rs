use crate::tool::{ToolContext, safe_path_allow_missing};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::time::{Duration, Instant};
use tact_core::AgentUpdate;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tool_refactor_macros::tool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WriteFileInput {
    #[schemars(description = "Path to the file to write, relative to the current workspace.")]
    pub path: String,
    #[schemars(description = "Complete file content to write.")]
    pub content: String,
}

/// Chunk size for incremental file writes. Tokio's async file I/O runs on the
/// blocking pool; smaller chunks let us emit progress updates without blocking
/// the executor for too long.
const WRITE_CHUNK_SIZE: usize = 64 * 1024;
/// Files smaller than this are written in a single operation to avoid the
/// overhead of chunking and progress tracking.
const SINGLE_WRITE_THRESHOLD: usize = 256 * 1024;

#[tool(name = "write_file", description = "Write content to file.")]
pub async fn write_file(ctx: ToolContext, input: WriteFileInput) -> Result<String> {
    let path = safe_path_allow_missing(&ctx.work_dir, &input.path)?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await.ok();
    }

    let total = input.content.len();
    let bytes = input.content.as_bytes();
    let line_count = input.content.lines().count();

    if total <= SINGLE_WRITE_THRESHOLD {
        fs::write(&path, bytes)
            .await
            .map_err(|e| anyhow::anyhow!("Error writing file: {}", e))?;
    } else {
        let mut file = fs::File::create(&path)
            .await
            .map_err(|e| anyhow::anyhow!("Error creating file: {}", e))?;

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
                    if let Some(ref tx) = ctx.ui_tx {
                        let _ = tx.send(AgentUpdate::Info(format!(
                            "Writing {}... {}% ({} / {} bytes)",
                            path.display(),
                            pct,
                            written,
                            total
                        )));
                    }
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

    Ok(format!(
        "Wrote {} bytes / {} lines to {}",
        total,
        line_count,
        path.display()
    ))
}

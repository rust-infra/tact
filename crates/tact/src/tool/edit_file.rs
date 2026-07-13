use crate::tool::{ToolContext, safe_path};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::fs;
use tool_refactor_macros::tool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EditFileInput {
    #[schemars(description = "Path to the file to edit, relative to the current workspace.")]
    pub path: String,
    #[schemars(description = "Exact text to find in the file.")]
    pub old_text: String,
    #[schemars(description = "Replacement text for matched old_text.")]
    pub new_text: String,
    #[schemars(
        description = "If true, replace every occurrence of old_text. If false (default), \
                       replace only the first match."
    )]
    #[serde(default)]
    pub replace_all: bool,
}

#[tool(
    name = "edit_file",
    description = "Replace exact text in a file. By default replaces only the first match; \
                    set replace_all=true to replace every occurrence."
)]
pub async fn edit_file(ctx: ToolContext, input: EditFileInput) -> Result<String> {
    let path = safe_path(&ctx.work_dir, &input.path)?;

    let content = fs::read_to_string(&path)
        .await
        .map_err(|e| anyhow::anyhow!("Error: {}", e))?;

    if input.old_text.is_empty() {
        return Err(anyhow::anyhow!("Error: old_text must not be empty"));
    }

    let count = content.matches(&input.old_text).count();
    if count == 0 {
        return Err(anyhow::anyhow!(
            "Error: Text not found in {}. The file may have changed since you last read it. Consider re-reading the file.",
            path.display()
        ));
    }

    let (updated, replaced) = if input.replace_all {
        (
            content.replace(&input.old_text, &input.new_text),
            count,
        )
    } else {
        (
            content.replacen(&input.old_text, &input.new_text, 1),
            1,
        )
    };

    fs::write(&path, updated)
        .await
        .map_err(|e| anyhow::anyhow!("Error: {}", e))?;

    Ok(format!(
        "Edited {}: replaced {} occurrence{}",
        path.display(),
        replaced,
        if replaced == 1 { "" } else { "s" }
    ))
}

#[cfg(test)]
mod tests {
    use crate::tool::test_support::{run_tool, test_context, write_workspace_file};

    use super::*;

    #[tokio::test]
    async fn edit_file_replaces_only_first_match_by_default() {
        let context = test_context("edit_file_replaces_only_first_match_by_default");
        write_workspace_file(&context.work_dir, "dup.txt", "foo bar foo");

        let output = run_tool(
            &context,
            EditFileTool,
            "edit_file",
            serde_json::json!({
                "path": "dup.txt",
                "old_text": "foo",
                "new_text": "FOO"
            }),
        )
        .await
        .unwrap();

        assert!(output.contains("replaced 1 occurrence"));
        assert_eq!(
            std::fs::read_to_string(context.work_dir.join("dup.txt")).unwrap(),
            "FOO bar foo"
        );
    }

    #[tokio::test]
    async fn edit_file_replace_all_replaces_every_match() {
        let context = test_context("edit_file_replace_all_replaces_every_match");
        write_workspace_file(&context.work_dir, "dup.txt", "foo bar foo");

        let output = run_tool(
            &context,
            EditFileTool,
            "edit_file",
            serde_json::json!({
                "path": "dup.txt",
                "old_text": "foo",
                "new_text": "FOO",
                "replace_all": true
            }),
        )
        .await
        .unwrap();

        assert!(output.contains("replaced 2 occurrences"));
        assert_eq!(
            std::fs::read_to_string(context.work_dir.join("dup.txt")).unwrap(),
            "FOO bar FOO"
        );
    }

    #[tokio::test]
    async fn edit_file_rejects_empty_old_text() {
        let context = test_context("edit_file_rejects_empty_old_text");
        write_workspace_file(&context.work_dir, "a.txt", "hello");

        let error = run_tool(
            &context,
            EditFileTool,
            "edit_file",
            serde_json::json!({
                "path": "a.txt",
                "old_text": "",
                "new_text": "x"
            }),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("old_text must not be empty"));
    }
}

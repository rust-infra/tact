// AskUserQuestion tool: ask the human operator a question.
//
// In the claurst original, the TUI layer intercepted this tool to display an
// interactive prompt.  Tact does not have this interception layer yet, so the
// tool returns the question as text; the model should interpret the user's
// next message as the answer.

use crate::tool::ToolContext;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use tool_refactor_macros::tool;
use tracing::debug;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AskUserInput {
    #[schemars(description = "The question to ask the user.")]
    pub question: String,
    #[schemars(description = "Optional list of choices for multiple-choice questions.")]
    #[serde(default)]
    pub options: Option<Vec<String>>,
}

#[tool(
    name = "ask_user",
    description = "Ask the user a question and wait for their response. Use this when you \
                    need clarification, confirmation, or additional information from the \
                    user. The question will be displayed and the user can type their answer."
)]
pub async fn ask_user(_ctx: ToolContext, input: AskUserInput) -> Result<String> {
    debug!(question = %input.question, "Asking user");

    let mut response = format!("Question: {}", input.question);
    if let Some(ref options) = input.options
        && !options.is_empty()
    {
        response.push_str("\nOptions:\n");
        for (i, opt) in options.iter().enumerate() {
            response.push_str(&format!("  {}. {}\n", i + 1, opt));
        }
    }

    Ok(response)
}

#[cfg(test)]
mod tests {
    use crate::tool::test_support::{run_tool, test_context};

    use super::*;

    #[tokio::test]
    async fn ask_user_formats_question() {
        let context = test_context("ask_user_formats_question");

        let output = run_tool(
            &context,
            AskUserTool,
            "ask_user",
            serde_json::json!({ "question": "Continue?" }),
        )
        .await
        .unwrap();

        assert_eq!(output, "Question: Continue?");
    }

    #[tokio::test]
    async fn ask_user_includes_numbered_options() {
        let context = test_context("ask_user_includes_numbered_options");

        let output = run_tool(
            &context,
            AskUserTool,
            "ask_user",
            serde_json::json!({
                "question": "Pick one",
                "options": ["A", "B"]
            }),
        )
        .await
        .unwrap();

        assert!(output.contains("Question: Pick one"));
        assert!(output.contains("1. A"));
        assert!(output.contains("2. B"));
    }
}

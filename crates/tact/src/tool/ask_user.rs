//! Ask the human operator a question.
//!
//! Interactive mode (TUI `ui_tx` present):
//! - With **options** + `multi_select: false` (default) → [`AgentUpdate::RequestSelect`]
//! - With **options** + `multi_select: true` → [`AgentUpdate::RequestMultiSelect`]
//!   (Space toggles, Enter confirms; permission / model pick still use single-select)
//! - Without options → [`AgentUpdate::Info`]; next chat message is the answer
//!
//! Headless / no `ui_tx`: return a formatted question string (tests / CI).

use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use tact_protocol::AgentUpdate;
use tool_refactor_macros::tool;
use tracing::debug;

use crate::tool::ToolContext;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AskUserInput {
    #[schemars(description = "The question to ask the user.")]
    pub question: String,
    #[schemars(description = "Optional list of choices. In the TUI these open a selection popup.")]
    #[serde(default)]
    pub options: Option<Vec<String>>,
    #[schemars(
        description = "When true with options, allow selecting multiple choices (Space to toggle). Default false (single-select)."
    )]
    #[serde(default)]
    pub multi_select: bool,
}

#[tool(
    name = "ask_user",
    description = "Ask the user a question and wait for their response. Prefer passing \
                    `options` so the TUI shows a selection popup. Set `multi_select: true` \
                    to let the user pick multiple options (Space toggles, Enter confirms). \
                    Without options, the question is shown in the log and the user's next \
                    message is treated as the answer."
)]
pub async fn ask_user(ctx: ToolContext, input: AskUserInput) -> Result<String> {
    debug!(question = %input.question, multi = input.multi_select, "Asking user");
    let question = input.question;
    let multi = input.multi_select;
    let options = input
        .options
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();

    if let Some(tx) = &ctx.ui_tx {
        if !options.is_empty() {
            if multi {
                let (respond_tx, respond_rx) = tokio::sync::oneshot::channel();
                let _ = tx.send(AgentUpdate::RequestMultiSelect {
                    prompt: question.clone(),
                    options: options.clone(),
                    respond: respond_tx,
                });
                return match respond_rx.await {
                    Ok(Some(idxs)) => Ok(format_multi_selection(&options, &idxs)),
                    Ok(None) => Ok("User cancelled the question.".to_string()),
                    Err(_) => Ok(
                        "User interface closed before answering; treat as cancelled.".to_string(),
                    ),
                };
            }

            let (respond_tx, respond_rx) = tokio::sync::oneshot::channel();
            let _ = tx.send(AgentUpdate::RequestSelect {
                prompt: question.clone(),
                options: options.clone(),
                respond: respond_tx,
                // Meta row already shows the choice; skip duplicate system line.
                log_confirm: false,
            });
            return match respond_rx.await {
                Ok(Some(idx)) => {
                    let chosen = options
                        .get(idx)
                        .cloned()
                        .unwrap_or_else(|| format!("option {idx}"));
                    Ok(format!("User selected: {chosen}"))
                }
                Ok(None) => Ok("User cancelled the question.".to_string()),
                Err(_) => {
                    Ok("User interface closed before answering; treat as cancelled.".to_string())
                }
            };
        }

        let _ = tx.send(AgentUpdate::Info(format!("❓ {question}")));
        return Ok(format!(
            "Question shown to the user:\n{question}\n\n\
             No choices were provided, so there was no selection popup. \
             Treat the user's next message as their answer."
        ));
    }

    Ok(format_headless_question(&question, &options, multi))
}

fn format_multi_selection(options: &[String], idxs: &[usize]) -> String {
    if idxs.is_empty() {
        return "User selected: (none)".to_string();
    }
    let labels: Vec<&str> = idxs
        .iter()
        .filter_map(|&i| options.get(i).map(String::as_str))
        .collect();
    if labels.is_empty() {
        format!("User selected indices: {idxs:?}")
    } else {
        format!("User selected: {}", labels.join(", "))
    }
}

fn format_headless_question(question: &str, options: &[String], multi: bool) -> String {
    let mut response = format!("Question: {question}");
    if multi && !options.is_empty() {
        response.push_str("\n(multi-select)");
    }
    if !options.is_empty() {
        response.push_str("\nOptions:\n");
        for (i, opt) in options.iter().enumerate() {
            response.push_str(&format!("  {}. {opt}\n", i + 1));
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use tact_protocol::AgentUpdate;
    use tokio::sync::mpsc::unbounded_channel;

    use super::*;
    use crate::tool::test_support::{run_tool, test_context};

    #[tokio::test]
    async fn ask_user_formats_question_headless() {
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
    async fn ask_user_includes_numbered_options_headless() {
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

    #[tokio::test]
    async fn ask_user_popup_returns_selected_option() {
        let mut context = test_context("ask_user_popup_returns_selected_option");
        let (tx, mut rx) = unbounded_channel();
        context.ui_tx = Some(tx);

        let tool = tokio::spawn(async move {
            run_tool(
                &context,
                AskUserTool,
                "ask_user",
                serde_json::json!({
                    "question": "Pick a color",
                    "options": ["red", "blue"]
                }),
            )
            .await
        });

        let update = rx.recv().await.expect("RequestSelect");
        match update {
            AgentUpdate::RequestSelect {
                prompt,
                options,
                respond,
                log_confirm,
            } => {
                assert_eq!(prompt, "Pick a color");
                assert_eq!(options, vec!["red", "blue"]);
                assert!(
                    !log_confirm,
                    "selection renders on tool meta, not a system line"
                );
                respond.send(Some(1)).unwrap();
            }
            other => panic!("expected RequestSelect, got {other:?}"),
        }

        let output = tool.await.unwrap().unwrap();
        assert_eq!(output, "User selected: blue");
    }

    #[tokio::test]
    async fn ask_user_multi_popup_returns_several() {
        let mut context = test_context("ask_user_multi_popup_returns_several");
        let (tx, mut rx) = unbounded_channel();
        context.ui_tx = Some(tx);

        let tool = tokio::spawn(async move {
            run_tool(
                &context,
                AskUserTool,
                "ask_user",
                serde_json::json!({
                    "question": "Pick toppings",
                    "options": ["onion", "cheese", "olive"],
                    "multi_select": true
                }),
            )
            .await
        });

        match rx.recv().await.expect("RequestMultiSelect") {
            AgentUpdate::RequestMultiSelect {
                prompt,
                options,
                respond,
            } => {
                assert_eq!(prompt, "Pick toppings");
                assert_eq!(options.len(), 3);
                respond.send(Some(vec![0, 2])).unwrap();
            }
            other => panic!("expected RequestMultiSelect, got {other:?}"),
        }

        let output = tool.await.unwrap().unwrap();
        assert_eq!(output, "User selected: onion, olive");
    }

    #[tokio::test]
    async fn ask_user_popup_cancel() {
        let mut context = test_context("ask_user_popup_cancel");
        let (tx, mut rx) = unbounded_channel();
        context.ui_tx = Some(tx);

        let tool = tokio::spawn(async move {
            run_tool(
                &context,
                AskUserTool,
                "ask_user",
                serde_json::json!({
                    "question": "Sure?",
                    "options": ["yes", "no"]
                }),
            )
            .await
        });

        match rx.recv().await.expect("RequestSelect") {
            AgentUpdate::RequestSelect { respond, .. } => {
                respond.send(None).unwrap();
            }
            other => panic!("expected RequestSelect, got {other:?}"),
        }

        let output = tool.await.unwrap().unwrap();
        assert!(output.contains("cancelled"));
    }

    #[tokio::test]
    async fn ask_user_free_text_emits_info() {
        let mut context = test_context("ask_user_free_text_emits_info");
        let (tx, mut rx) = unbounded_channel();
        context.ui_tx = Some(tx);

        let output = run_tool(
            &context,
            AskUserTool,
            "ask_user",
            serde_json::json!({ "question": "What is your name?" }),
        )
        .await
        .unwrap();

        assert!(output.contains("Question shown"));
        assert!(output.contains("What is your name?"));
        match rx.try_recv().expect("Info") {
            AgentUpdate::Info(msg) => assert!(msg.contains("What is your name?")),
            other => panic!("expected Info, got {other:?}"),
        }
    }
}

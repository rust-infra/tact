// Agent core module
// Receives user tasks, calls the OpenAI API to generate execution plans,
// and executes them step by step inside a sandbox.
// Communicates with the TUI module over channels, reporting execution status in real time.

use anyhow::{Result, anyhow};
use async_openai::{
    Client,
    config::OpenAIConfig,
    types::{
        ChatCompletionRequestMessage, ChatCompletionRequestUserMessage,
        CreateChatCompletionRequest, Role,
    },
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::time::{Duration, sleep, timeout};
use tools::Sandbox;

/// Execution status of a step.
#[derive(Debug, Clone)]
pub enum StepStatus {
    Success,
    Failed,
}

/// Structured result of a step execution.
#[derive(Debug, Clone)]
pub struct StepResult {
    pub tool: String,
    pub arg_summary: String,
    pub status: StepStatus,
    pub message: String,
    /// Additional details, e.g. full content of a written file or raw command output.
    pub detail: Option<String>,
    /// Tool execution duration in milliseconds. None for non-tool steps.
    pub duration_ms: Option<u64>,
}

/// Parameters for a model API call.
#[derive(Debug, Clone)]
pub struct ModelCallParams {
    pub model: String,
    pub max_tokens: u32,
    pub thinking_budget: Option<u32>,
    pub reasoning_effort: Option<String>,
    pub extra_body: Option<String>,
}

/// Error classification — lets the TUI distinguish fatal errors (displayed as ❌ Error)
/// from non-fatal situations (shown as Info).
#[derive(Debug, Clone)]
pub enum AgentErrorKind {
    /// Balance query failed (network or API error)
    BalanceQueryFailed(String),
    /// Balance query is only supported for DeepSeek provider
    BalanceNotSupported,
    /// Generic error (catch-all)
    Other(String),
}

impl AgentErrorKind {
    /// Returns a human-readable error description.
    pub fn display(&self) -> &str {
        match self {
            AgentErrorKind::BalanceQueryFailed(e) => e,
            AgentErrorKind::BalanceNotSupported => {
                "Balance query is only available for DeepSeek provider"
            }
            AgentErrorKind::Other(msg) => msg,
        }
    }
}

/// Status update messages sent from the Agent to the TUI.
#[derive(Debug)]
pub enum AgentUpdate {
    /// Plan generated, with list of steps
    PlanGenerated(Vec<PlanStep>),
    /// Step `idx` has started execution
    StepStarted(usize),
    /// Step `idx` succeeded, with structured result
    StepFinished(usize, StepResult),
    /// Step `idx` failed, with error message
    StepFailed(usize, String),
    /// Requires user approval: prompt text, step index, approval channel (true=accept, false=reject)
    NeedApproval(String, usize, oneshot::Sender<bool>),
    /// The entire task is complete
    TaskComplete(String),
    /// Agent error, with classification for the TUI to decide display style
    Error(AgentErrorKind),
    /// Token usage stats
    TokenUsage {
        prompt: u32,
        completion: u32,
        total: u32,
        /// DeepSeek KV cache hit prompt tokens (0 for non-DeepSeek providers)
        prompt_cache_hit_tokens: u32,
        /// DeepSeek KV cache miss prompt tokens
        prompt_cache_miss_tokens: u32,
    },
    /// Account balance info (DeepSeek only)
    Balance(BalanceInfo),
    /// Model call parameters (name, max_tokens, thinking budget, etc.)
    ModelInfo(ModelCallParams),
    /// Informational notice (does not change state)
    Info(String),
    /// Dynamically append a step to the existing plan (does not reset selection state)
    StepAdded(PlanStep),
    /// Request user to choose from a list of options; returns option index (None = cancelled)
    RequestSelect {
        prompt: String,
        options: Vec<String>,
        respond: oneshot::Sender<Option<usize>>,
    },
    /// Streaming output text fragment (appended to Log in real time)
    StreamChunk(String),
    /// Streaming thinking / reasoning content fragment
    ThinkingChunk(String),
}

/// User commands sent from the TUI to the Agent.
#[derive(Debug)]
pub enum UserCommand {
    /// Submit a new natural-language task
    SubmitTask(String),
    /// Cancel the current task (full cancellation logic not yet implemented)
    Cancel,
    /// Query account balance (DeepSeek only)
    QueryBalance,
}

/// A single step in the execution plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    /// Human-readable step description
    pub description: String,
    /// Tool name: read_file / write_file / run_command
    pub tool: String,
    /// Tool arguments (key-value pairs)
    pub args: HashMap<String, String>,
    /// Whether user manual approval is required before execution
    pub need_approval: bool,
    /// Output after execution (populated by TUI; defaults to None on JSON deserialization)
    #[serde(default)]
    pub output: Option<String>,
}

/// A single currency entry in DeepSeek account balance info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceEntry {
    /// Currency type: CNY or USD
    pub currency: String,
    /// Total available balance (granted + topped up)
    pub total_balance: String,
    /// Unexpired granted balance
    pub granted_balance: String,
    /// Topped-up balance
    pub topped_up_balance: String,
}

/// DeepSeek account balance query result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceInfo {
    /// Whether the account has available balance
    pub is_available: bool,
    /// Per-currency balance details
    pub balance_infos: Vec<BalanceEntry>,
}

/// Agent struct — holds the sandbox, OpenAI client, and communication channels.
pub struct Agent {
    sandbox: Arc<Sandbox>,
    openai_client: Client<OpenAIConfig>,
    /// Channel for sending status updates to the TUI
    ui_tx: UnboundedSender<AgentUpdate>,
    /// Channel for receiving user commands from the TUI
    cmd_rx: UnboundedReceiver<UserCommand>,
    /// Task cancellation flag — set by the TUI, checked by the Agent between steps
    cancel_flag: Arc<AtomicBool>,
}

impl Agent {
    pub fn new(
        ui_tx: UnboundedSender<AgentUpdate>,
        cmd_rx: UnboundedReceiver<UserCommand>,
    ) -> Self {
        // Use the current directory as workspace and build the sandbox
        let workspace = PathBuf::from(".");
        // Allowlist of permitted commands
        let allowed_commands = vec![
            "cargo".to_string(),
            "git".to_string(),
            "python".to_string(),
            "npm".to_string(),
        ];
        let sandbox = Sandbox::new(workspace, allowed_commands);
        let openai_client = Client::new();
        Self {
            sandbox: Arc::new(sandbox),
            openai_client,
            ui_tx,
            cmd_rx,
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Agent main loop: continuously listens for user commands until the channel closes.
    pub async fn run(mut self) -> Result<()> {
        while let Some(cmd) = self.cmd_rx.recv().await {
            match cmd {
                UserCommand::SubmitTask(task) => {
                    if let Err(e) = self.handle_task(task).await {
                        let _ = self
                            .ui_tx
                            .send(AgentUpdate::Error(AgentErrorKind::Other(e.to_string())));
                    }
                }
                UserCommand::Cancel => {
                    self.cancel_flag.store(true, Ordering::Relaxed);
                    let _ = self
                        .ui_tx
                        .send(AgentUpdate::Info("Cancelling current task...".into()));
                }
                UserCommand::QueryBalance => {
                    let _ = self
                        .ui_tx
                        .send(AgentUpdate::Error(AgentErrorKind::BalanceNotSupported));
                }
            }
        }
        Ok(())
    }

    /// Handle a single task: generate plan → execute step by step → report results.
    async fn handle_task(&self, task: String) -> Result<()> {
        self.cancel_flag.store(false, Ordering::Relaxed);

        // 1. Call LLM to generate an execution plan
        let plan = self.generate_plan(&task).await?;
        if self.cancel_flag.load(Ordering::Relaxed) {
            self.ui_tx.send(AgentUpdate::StepFailed(
                0,
                "Cancelled by user before execution".into(),
            ))?;
            return Ok(());
        }
        self.ui_tx.send(AgentUpdate::PlanGenerated(plan.clone()))?;

        // 2. Execute each step sequentially
        for (idx, step) in plan.iter().enumerate() {
            if self.cancel_flag.load(Ordering::Relaxed) {
                self.ui_tx
                    .send(AgentUpdate::StepFailed(idx, "Cancelled by user".into()))?;
                return Ok(());
            }
            self.ui_tx.send(AgentUpdate::StepStarted(idx))?;

            // If the step requires approval, wait for user confirmation via a oneshot channel
            if step.need_approval {
                let (tx, mut rx) = oneshot::channel();
                self.ui_tx
                    .send(AgentUpdate::NeedApproval(step.description.clone(), idx, tx))?;
                // Poll every 100ms, balancing cancel responsiveness and CPU usage
                let approved = loop {
                    if self.cancel_flag.load(Ordering::Relaxed) {
                        self.ui_tx.send(AgentUpdate::StepFailed(
                            idx,
                            "Cancelled by user during approval".into(),
                        ))?;
                        return Ok(());
                    }
                    match rx.try_recv() {
                        Ok(result) => break result,
                        Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                            sleep(Duration::from_millis(100)).await;
                            continue;
                        }
                        Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                            return Err(anyhow!("User approval cancelled"));
                        }
                    }
                };
                if !approved {
                    self.ui_tx
                        .send(AgentUpdate::StepFailed(idx, "Rejected by user".into()))?;
                    return Ok(());
                }
            }

            // Execute the tool call inside the sandbox
            let result = self.execute_step(step).await;
            match result {
                Ok(output) => {
                    let arg_summary = match step.tool.as_str() {
                        "read_file" | "write_file" => {
                            step.args.get("path").cloned().unwrap_or_default()
                        }
                        "run_command" => step.args.get("command").cloned().unwrap_or_default(),
                        _ => step.args.values().next().cloned().unwrap_or_default(),
                    };
                    let preview = output.chars().take(200).collect::<String>();
                    let detail = match step.tool.as_str() {
                        "write_file" => step.args.get("content").cloned(),
                        "run_command" => Some(output),
                        _ => None,
                    };
                    let step_result = StepResult {
                        tool: step.tool.clone(),
                        arg_summary,
                        status: StepStatus::Success,
                        message: preview,
                        detail,
                        duration_ms: None,
                    };
                    self.ui_tx
                        .send(AgentUpdate::StepFinished(idx, step_result))?;
                }
                Err(e) => {
                    self.ui_tx
                        .send(AgentUpdate::StepFailed(idx, e.to_string()))?;
                    return Ok(());
                }
            }
            // Brief pause between steps so the TUI can show animation
            sleep(Duration::from_millis(200)).await;
        }

        self.ui_tx.send(AgentUpdate::TaskComplete(
            "All steps completed successfully!".into(),
        ))?;
        Ok(())
    }

    /// Call the OpenAI ChatCompletion API, requesting a fixed-format JSON plan array from the model.
    async fn generate_plan(&self, task: &str) -> Result<Vec<PlanStep>> {
        let model = "gpt-3.5-turbo".to_string();
        let _ = self.ui_tx.send(AgentUpdate::ModelInfo(ModelCallParams {
            model: model.clone(),
            max_tokens: 0,
            thinking_budget: None,
            reasoning_effort: None,
            extra_body: None,
        }));
        let json_request = CreateChatCompletionRequest {
            model,
            messages: vec![ChatCompletionRequestMessage::User(
                ChatCompletionRequestUserMessage {
                    content: format!(
                        "You must output ONLY a valid JSON array of steps. Each step has fields: description (str), tool (one of read_file, write_file, run_command), args (object with keys appropriate for the tool), need_approval (bool).\n\nTask: {}",
                        task
                    ).into(),
                    role: Role::User,
                    name: None,
                },
            )],
            temperature: Some(0.2),
            ..Default::default()
        };
        let _ = self
            .ui_tx
            .send(AgentUpdate::Info("Calling LLM API...".into()));
        let resp = timeout(
            Duration::from_secs(30),
            self.openai_client.chat().create(json_request),
        )
        .await
        .map_err(|_| anyhow!("OpenAI API timed out after 30s"))??;
        let _ = self.ui_tx.send(AgentUpdate::StepFinished(
            0,
            StepResult {
                tool: "generate_plan".to_string(),
                arg_summary: String::new(),
                status: StepStatus::Success,
                message: "API response received".to_string(),
                detail: None,
                duration_ms: None,
            },
        ));
        if let Some(usage) = resp.usage {
            let _ = self.ui_tx.send(AgentUpdate::TokenUsage {
                prompt: usage.prompt_tokens,
                completion: usage.completion_tokens,
                total: usage.total_tokens,
                prompt_cache_hit_tokens: 0,
                prompt_cache_miss_tokens: 0,
            });
        }
        let content = resp
            .choices
            .first()
            .and_then(|c| c.message.content.clone())
            .unwrap_or_default();
        // Clean up any markdown code blocks the LLM may have wrapped the output in
        let cleaned = content
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();
        let plan: Vec<PlanStep> = serde_json::from_str(cleaned)?;
        Ok(plan)
    }

    /// Execute a specific tool operation in the sandbox based on the step specification.
    async fn execute_step(&self, step: &PlanStep) -> Result<String> {
        match step.tool.as_str() {
            "read_file" => {
                let path = step.args.get("path").ok_or(anyhow!("Missing path"))?;
                let content = self.sandbox.read_file(path).await?;
                Ok(format!("Read {} bytes", content.len()))
            }
            "write_file" => {
                let path = step.args.get("path").ok_or(anyhow!("Missing path"))?;
                let content = step.args.get("content").ok_or(anyhow!("Missing content"))?;
                self.sandbox.write_file(path, content).await?;
                Ok(format!("Written to {}", path))
            }
            "run_command" => {
                let command = step.args.get("command").ok_or(anyhow!("Missing command"))?;
                let default_args = "[]".to_string();
                let args_str = step.args.get("args").unwrap_or(&default_args);
                let args: Vec<&str> = serde_json::from_str(args_str)?;
                let output = self.sandbox.run_command(command, &args).await?;
                Ok(format!("Command output:\n{}", output))
            }
            _ => Err(anyhow!("Unknown tool: {}", step.tool)),
        }
    }
}

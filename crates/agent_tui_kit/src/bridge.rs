//! The in/out contract between the kit and the host project.

use tact_protocol::UserCommand;

/// A user/agent command the kit sends to the host.
///
/// Generic variants only — the subset any agent host can serve. Tact-specific
/// commands (`QueryBalance`, …) ride the extension channel (see
/// [`ExtensionCommand`]) so the kit contract stays host-agnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Submit a new natural-language task.
    SubmitTask(String),
    /// Cancel the current in-flight task (cooperative; the agent loop exits
    /// at the next check point and emits `TaskCancelled`).
    Cancel,
    /// Compact the session history (`/compact`).
    Compact,
    /// Query session statistics (`/stats`).
    QueryStats,
    /// Query background task status; `None` lists all tasks, `Some(id)`
    /// shows a single task as pretty JSON.
    QueryBackground(Option<String>),
    /// Set the active permission mode (Default / Plan / Auto).
    SetPermissionMode(String),
    /// Set the active session's thinking budget (tokens).
    SetThinkingBudget(usize),
    /// Set the active session's reasoning effort; `Some(level)` sets it,
    /// `None` clears (wire omits effort).
    SetReasoningEffort(Option<String>),
    /// Set the active agent session's model.
    SetModel(String),
}

/// Tact-only commands that ride the extension channel (T4.4 split).
///
/// The host's [`BridgeExtension`] implementation routes these to Tact-specific
/// services (account balance/quota) instead of the generic agent loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionCommand {
    /// Query account balance (DeepSeek/Kimi).
    QueryBalance,
}

impl From<Command> for UserCommand {
    fn from(cmd: Command) -> Self {
        match cmd {
            Command::SubmitTask(task) => UserCommand::SubmitTask(task),
            Command::Cancel => UserCommand::Cancel,
            Command::Compact => UserCommand::RunCommand {
                name: "compact".to_string(),
                args: String::new(),
            },
            Command::QueryStats => UserCommand::RunCommand {
                name: "stats".to_string(),
                args: String::new(),
            },
            Command::QueryBackground(id) => UserCommand::RunCommand {
                name: "background".to_string(),
                args: id.unwrap_or_default(),
            },
            Command::SetPermissionMode(mode) => UserCommand::RunCommand {
                name: "permission".to_string(),
                args: mode,
            },
            Command::SetThinkingBudget(budget) => UserCommand::RunCommand {
                name: "thinking".to_string(),
                args: budget.to_string(),
            },
            Command::SetReasoningEffort(effort) => UserCommand::RunCommand {
                name: "effort".to_string(),
                args: effort
                    .map(|e| e.as_str().to_string())
                    .unwrap_or_else(|| "none".to_string()),
            },
            Command::SetModel(model) => UserCommand::RunCommand {
                name: "model".to_string(),
                args: model,
            },
        }
    }
}

impl TryFrom<UserCommand> for Command {
    type Error = ();

    /// Maps the generic protocol commands; [`UserCommand::QueryBalance`] is
    /// extension-only and fails the conversion (route via [`ExtensionCommand`]).
    fn try_from(cmd: UserCommand) -> Result<Self, Self::Error> {
        match cmd {
            UserCommand::SubmitTask(task) => Ok(Self::SubmitTask(task)),
            // The kit names the commands it can drive. `/compact` is one of
            // them, so it maps back to the closed variant; anything else is a
            // name this kit does not know and cannot drive.
            UserCommand::RunCommand { name, args } if name == "compact" && args.is_empty() => {
                Ok(Self::Compact)
            }
            UserCommand::RunCommand { name, args } if name == "permission" => {
                Ok(Self::SetPermissionMode(args))
            }
            UserCommand::RunCommand { name, args } if name == "thinking" => {
                args.parse().map(Self::SetThinkingBudget).map_err(|_| ())
            }
            UserCommand::RunCommand { name, args } if name == "effort" => {
                if args == "none" {
                    Ok(Self::SetReasoningEffort(None))
                } else {
                    args.parse()
                        .map(|effort| Self::SetReasoningEffort(Some(effort)))
                        .map_err(|_| ())
                }
            }
            UserCommand::RunCommand { name, args } if name == "model" => Ok(Self::SetModel(args)),
            // Every other name is one this kit does not know and cannot drive.
            UserCommand::RunCommand { .. } => Err(()),
            UserCommand::Cancel => Ok(Self::Cancel),
            UserCommand::QueryBalance => Err(()),
            // A wake-up signal routed from the TUI to the driver when a
            // background subagent finishes; not a host command for the kit.
            UserCommand::SubagentFinishedNotification { .. } => Err(()),
            // Cancellation is handled by the driver (it owns the manager's
            // cancel handles), not a host command for the kit.
            UserCommand::CancelSubagent { .. } => Err(()),
            // OAuth authorization for a remote MCP server is a Tact driver
            // flow (loopback callback + router reload), not a kit command.
            UserCommand::McpAuth { .. } => Err(()),
            // Listing MCP servers reads the agent's live router, which only the
            // Tact driver owns; not a kit command. `/mcp list` and `/mcp prompts`
            // travel as `RunCommand { name: "mcp" }`, which the catch-all below
            // already refuses. Running a prompt still needs this variant: it
            // submits a Tact user turn.
            UserCommand::RunMcpPrompt { .. } => Err(()),
            // Hook review reads Tact's hook sources and writes its review store
            // (`~/.tact/hooks-state.json`) — Tact-specific, and deliberately not a
            // generic kit command. It travels as `RunCommand { name: "hooks" }`,
            // which the catch-all below already refuses.
            // Durable Goal lifecycle is a Tact host capability backed by its
            // SQLite stores and bounded runner, not a generic kit command.
            UserCommand::ResumeGoal { .. } => Err(()),
        }
    }
}

/// What the host project implements to drive the kit.
pub trait AgentBridge {
    /// Send a user/agent command out (maps to Tact's `user_cmd_tx`).
    fn send_command(&mut self, cmd: Command);

    /// Optional host extension surface (balance, quota, plugins, voice, …).
    fn extension(&mut self) -> Option<&mut dyn BridgeExtension> {
        None
    }
}

/// Host-specific capabilities the shell renders — NOT part of the kit core.
///
/// Tact implements this for plugins / voice / skills / account balance. Only
/// events whose types live in `tact_protocol` appear in [`ExtensionEvent`];
/// plugin/voice payloads are host-internal and stay inside the app layer.
pub trait BridgeExtension {
    /// Handle a host-specific event produced by the app layer.
    fn on_event(&mut self, event: ExtensionEvent);

    /// Handle a Tact-only command (e.g. balance query).
    fn on_command(&mut self, cmd: ExtensionCommand) {
        let _ = cmd;
    }

    /// Extra keybindings the extension contributes.
    fn keybindings(&self) -> &'static [(&'static str, &'static str)] {
        &[]
    }
}

/// Host-specific events handed from the app layer to the extension.
///
/// Filled from the Phase 0 T0.3 protocol audit: the account channel is the
/// only extension event whose payload type is protocol-level; plugin and
/// voice events stay inside the Tact app layer (`widgets/state/app/extensions.rs`
/// and `app/voice.rs`).
#[derive(Debug, Clone)]
pub enum ExtensionEvent {
    /// Account balance / usage quota update (DeepSeek/Kimi).
    AccountUpdate(tact_protocol::AccountUpdate),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_roundtrips_through_protocol() {
        let cases = [
            Command::SubmitTask("hi".into()),
            Command::Cancel,
            Command::Compact,
            Command::QueryStats,
            Command::QueryBackground(Some("id-1".into())),
            Command::QueryBackground(None),
            Command::SetPermissionMode("plan".into()),
            Command::SetThinkingBudget(32_000),
            Command::SetReasoningEffort(Some("high".into())),
            Command::SetReasoningEffort(None),
            Command::SetModel("gpt-4o".into()),
        ];
        for cmd in cases {
            let protocol: UserCommand = cmd.clone().into();
            assert_eq!(Command::try_from(protocol).unwrap(), cmd);
        }
    }

    #[test]
    fn query_balance_is_extension_only() {
        assert!(Command::try_from(UserCommand::QueryBalance).is_err());
    }
}

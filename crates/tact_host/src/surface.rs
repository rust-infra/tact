//! Typed runtime surface queries for the desktop and TUI readers.
//!
//! The host owns the live `Agent`; this module translates a [`SurfaceQuery`]
//! into an immutable [`SurfaceSnapshot`] so a frontend never parses TUI
//! Markdown or reaches into the agent directly.

use tact::{Agent, extract_text, utils::RwLockExt};
use tact_llm::Role;
use tact_protocol::{
    AgentUpdate, HistoryEntrySnapshot, HookItemSnapshot, HookSnapshot, McpPromptSnapshot,
    McpServerSnapshot, McpServerStatusSnapshot, StatsSnapshot, SurfaceQuery, SurfaceSnapshot,
};

pub(crate) async fn query_surface(agent: &mut Agent, request_id: u64, query: SurfaceQuery) {
    let snapshot = match query {
        SurfaceQuery::McpServers => SurfaceSnapshot::McpServers(
            agent
                .mcp_router
                .server_summaries()
                .into_iter()
                .map(|(name, tools)| McpServerSnapshot {
                    name,
                    transport: "runtime".to_string(),
                    source: "connected".to_string(),
                    status: McpServerStatusSnapshot::Connected { tools },
                })
                .collect(),
        ),
        SurfaceQuery::McpPrompts { server } => {
            match agent.mcp_router.list_prompt_infos(server.as_deref()).await {
                Ok(prompts) => SurfaceSnapshot::McpPrompts(
                    prompts
                        .into_iter()
                        .map(|prompt| McpPromptSnapshot {
                            server: prompt.server,
                            name: prompt.name,
                            description: prompt.description,
                            arguments: prompt
                                .arguments
                                .into_iter()
                                .map(|argument| tact_protocol::McpPromptArgumentSnapshot {
                                    name: argument.name,
                                    description: argument.description,
                                    required: argument.required,
                                })
                                .collect(),
                            error: prompt.error,
                        })
                        .collect(),
                ),
                Err(error) => {
                    agent.emit_update(AgentUpdate::Info(format!("MCP prompts failed: {error:#}")));
                    return;
                }
            }
        }
        SurfaceQuery::Hooks => match tact::plugin::survey_hooks(&agent.tool_context.work_dir) {
            Ok(report) => {
                let map = |hook: tact::plugin::HookSummary| HookItemSnapshot {
                    source: hook.source,
                    event: hook.event,
                    matcher: hook.matcher,
                    command: hook.command,
                };
                SurfaceSnapshot::Hooks(HookSnapshot {
                    trusted: report.trusted.into_iter().map(map).collect(),
                    pending: report.pending.into_iter().map(map).collect(),
                })
            }
            Err(error) => {
                agent.emit_update(AgentUpdate::Info(format!("Hooks list failed: {error:#}")));
                return;
            }
        },
        SurfaceQuery::Stats => {
            let stats = agent.stats();
            let stats = stats.read_recover();
            SurfaceSnapshot::Stats(StatsSnapshot {
                turns_taken: stats.prompt_count.min(u32::MAX as u64) as u32,
                prompt_tokens: stats.prompt_tokens.min(u32::MAX as u64) as u32,
                completion_tokens: stats.completion_tokens.min(u32::MAX as u64) as u32,
                total_tokens: stats.total_tokens.min(u32::MAX as u64) as u32,
                cache_hit_tokens: stats.cache_hit_tokens.min(u32::MAX as u64) as u32,
                reasoning_tokens: stats.reasoning_tokens.min(u32::MAX as u64) as u32,
                elapsed_ms: stats.start_time.elapsed().as_millis().min(u64::MAX as u128) as u64,
            })
        }
        SurfaceQuery::SystemPrompt { assembled } => match agent.preview_system_prompt(assembled) {
            Ok(prompt) => SurfaceSnapshot::SystemPrompt(prompt),
            Err(error) => {
                agent.emit_update(AgentUpdate::Info(format!(
                    "System prompt failed: {error:#}"
                )));
                return;
            }
        },
        SurfaceQuery::History => {
            let mut entries = Vec::new();
            for message in agent.context().iter().rev().take(50).rev() {
                let text = extract_text(&message.content);
                if text.trim().is_empty() {
                    continue;
                }
                let (task, summary) = match message.role {
                    Role::User => (text, String::new()),
                    Role::Assistant => (String::new(), text),
                };
                entries.push(HistoryEntrySnapshot {
                    task,
                    timestamp: String::new(),
                    summary,
                });
            }
            SurfaceSnapshot::History(entries)
        }
    };
    agent.emit_update(AgentUpdate::SurfaceSnapshot {
        request_id,
        snapshot,
    });
}

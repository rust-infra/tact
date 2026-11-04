//! Typed surfaces the controller asks the shell to show: system prompt,
//! statistics, history, MCP and hooks.

use tact_desktop_core::SurfaceSnapshot;

pub(crate) fn render(surface: &SurfaceSnapshot) -> (String, String) {
    match surface {
        SurfaceSnapshot::SystemPrompt(prompt) => ("System prompt".to_owned(), prompt.clone()),
        SurfaceSnapshot::Stats(stats) => (
            "Session statistics".to_owned(),
            format!(
                "- Turns: {}\n- Prompt tokens: {}\n- Completion tokens: {}\n- Total tokens: {}\n- Cache hit: {}\n- Reasoning: {}\n- Elapsed: {} ms",
                stats.turns_taken,
                stats.prompt_tokens,
                stats.completion_tokens,
                stats.total_tokens,
                stats.cache_hit_tokens,
                stats.reasoning_tokens,
                stats.elapsed_ms
            ),
        ),
        SurfaceSnapshot::History(entries) => (
            "History".to_owned(),
            entries
                .iter()
                .map(|entry| {
                    format!(
                        "### {}\n\n{} · {}",
                        entry.task, entry.timestamp, entry.summary
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
        ),
        SurfaceSnapshot::McpServers(servers) => (
            "MCP servers".to_owned(),
            servers
                .iter()
                .map(|server| {
                    format!(
                        "- **{}** · {} · {} · {:?}",
                        server.name, server.transport, server.source, server.status
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        SurfaceSnapshot::McpPrompts(prompts) => (
            "MCP prompts".to_owned(),
            prompts
                .iter()
                .map(|prompt| {
                    format!(
                        "- **{}** · {}{}",
                        prompt.name,
                        prompt.server,
                        prompt
                            .description
                            .as_deref()
                            .map(|description| format!(" · {description}"))
                            .unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        SurfaceSnapshot::Hooks(hooks) => (
            "Hooks".to_owned(),
            format!(
                "## Trusted\n\n{}\n\n## Pending\n\n{}",
                hooks
                    .trusted
                    .iter()
                    .map(|hook| format!("- {} · {} · {}", hook.source, hook.event, hook.command))
                    .collect::<Vec<_>>()
                    .join("\n"),
                hooks
                    .pending
                    .iter()
                    .map(|hook| format!("- {} · {} · {}", hook.source, hook.event, hook.command))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_prompt_surface_keeps_its_markdown() {
        let (title, body) = render(&SurfaceSnapshot::SystemPrompt("# Prompt".to_owned()));
        assert_eq!(title, "System prompt");
        assert_eq!(body, "# Prompt");
    }
}

//! Typed surfaces the controller asks the shell to show: system prompt,
//! statistics, history, MCP and hooks.

use nsh_base::{text_view, theme};
use nsh_core::element::{AccessibilityRole, AnyElement};
use nsh_core::prelude::*;
use tact_desktop_core::SurfaceSnapshot;

use crate::app::SnapshotRef;

use super::{RADIUS, token};

pub fn layer(snapshot: Signal<SnapshotRef>) -> AnyElement {
    let dismissed = signal(None::<u64>);
    dynamic(move || {
        let current = snapshot.get();
        let Some(surface) = current.0.surface.clone() else {
            return view().into_element();
        };
        if dismissed.get() == Some(current.0.revision) {
            return view().into_element();
        }
        let revision = current.0.revision;
        let (title, body) = render(&surface);
        view()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .items_center()
            .justify_center()
            .p_6()
            .bg(move || token("scrim"))
            .child(
                view()
                    .test_id("surface")
                    .w_full()
                    .max_w(px(760.0))
                    .max_h(px(600.0))
                    .flex_col()
                    .rounded(px(RADIUS + 4.0))
                    .bg(move || theme().popover)
                    .border_1()
                    .border_color(move || theme().border)
                    .child(header(title, move || dismissed.set(Some(revision))))
                    .child(
                        scroll().flex_1().child(
                            view()
                                .px_5()
                                .py_4()
                                .child(text_view().markdown(body).selectable(true)),
                        ),
                    ),
            )
            .into_element()
    })
}

fn header(title: String, close: impl Fn() + 'static) -> impl IntoElement {
    view()
        .h(px(44.0))
        .flex_row()
        .items_center()
        .gap_2()
        .px_4()
        .border_b_1()
        .border_b_color(move || theme().border)
        .child(
            view().flex_1().child(
                text(title)
                    .number_of_lines(1)
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD),
            ),
        )
        .child(
            view()
                .flex_shrink_0()
                .px_2()
                .py(px(3.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .role(AccessibilityRole::Button)
                .label("Close")
                .on_press(close)
                .child(
                    text("Close")
                        .text_size(px(11.0))
                        .text_color(move || theme().muted_foreground),
                ),
        )
}

fn render(surface: &SurfaceSnapshot) -> (String, String) {
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

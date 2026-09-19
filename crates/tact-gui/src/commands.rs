//! Tact actions, shortcut bindings, and the command palette registry.

use gpui_kit::component::IconName;
use gpui_kit::component::command::{CommandGroup, CommandItem};
use gpui_kit::{App, KeyBinding, actions};

/// Key context for Tact's window-level commands.
pub(crate) const CONTEXT: &str = "TactApp";

actions!([
    OpenCommandPalette,
    NewSession,
    StopTask,
    ToggleWorkPane,
    FocusComposer,
    CycleTranscriptDetail,
    ToggleSidebar,
    OpenDiff,
    OpenTasks,
    OpenSettings,
    CycleSessions,
    CycleSessionsBackward,
    RemoveAttachment,
    CompactSession,
    SessionStats,
    McpServers,
    ToggleTheme,
]);

/// Register the global keyboard contract.
///
/// Both macOS and non-macOS bindings are registered unconditionally: GPUI only
/// delivers the modifier the platform actually produces, so the alternate
/// binding is harmless and keeps the source list self-documenting.
pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-k", OpenCommandPalette, Some(CONTEXT)),
        KeyBinding::new("ctrl-k", OpenCommandPalette, Some(CONTEXT)),
        KeyBinding::new("cmd-n", NewSession, Some(CONTEXT)),
        KeyBinding::new("ctrl-n", NewSession, Some(CONTEXT)),
        KeyBinding::new("escape", StopTask, Some(CONTEXT)),
        KeyBinding::new("cmd-\\", ToggleWorkPane, Some(CONTEXT)),
        KeyBinding::new("ctrl-\\", ToggleWorkPane, Some(CONTEXT)),
        KeyBinding::new("cmd-l", FocusComposer, Some(CONTEXT)),
        KeyBinding::new("ctrl-l", FocusComposer, Some(CONTEXT)),
        KeyBinding::new("ctrl-o", CycleTranscriptDetail, Some(CONTEXT)),
        KeyBinding::new("cmd-b", ToggleSidebar, Some(CONTEXT)),
        KeyBinding::new("ctrl-b", ToggleSidebar, Some(CONTEXT)),
        KeyBinding::new("cmd-shift-d", OpenDiff, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-d", OpenDiff, Some(CONTEXT)),
        KeyBinding::new("cmd-shift-t", OpenTasks, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-t", OpenTasks, Some(CONTEXT)),
        KeyBinding::new("cmd-,", OpenSettings, Some(CONTEXT)),
        KeyBinding::new("ctrl-,", OpenSettings, Some(CONTEXT)),
        KeyBinding::new("ctrl-tab", CycleSessions, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-tab", CycleSessionsBackward, Some(CONTEXT)),
        KeyBinding::new("cmd-shift-backspace", RemoveAttachment, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-backspace", RemoveAttachment, Some(CONTEXT)),
        KeyBinding::new("cmd-shift-l", ToggleTheme, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-l", ToggleTheme, Some(CONTEXT)),
    ]);
}

/// Groups shown in the command palette.
pub(crate) fn groups() -> Vec<CommandGroup> {
    vec![
        CommandGroup::new().label("Navigate").items([
            command("Command palette", IconName::Search, OpenCommandPalette)
                .keywords(["palette", "actions", "shortcut"]),
            command("Toggle sidebar", IconName::PanelLeft, ToggleSidebar)
                .keywords(["navigation", "collapse"]),
            command("Toggle work pane", IconName::PanelRight, ToggleWorkPane)
                .keywords(["plan", "diff", "tasks", "files", "drawer"]),
            command("Open diff", IconName::File, OpenDiff).keywords(["changes", "review", "patch"]),
            command("Open tasks", IconName::CircleCheck, OpenTasks).keywords(["todo", "progress"]),
        ]),
        CommandGroup::new().label("Session").items([
            command("New session", IconName::Plus, NewSession).keywords([
                "fresh",
                "clear",
                "workspace",
            ]),
            command("Focus composer", IconName::ArrowUp, FocusComposer)
                .keywords(["message", "prompt", "input"]),
            command("Stop task", IconName::Pause, StopTask).keywords([
                "cancel",
                "interrupt",
                "escape",
            ]),
            command(
                "Cycle transcript detail",
                IconName::Eye,
                CycleTranscriptDetail,
            )
            .keywords(["verbose", "thinking", "normal", "ctrl-o"]),
            command("Cycle sessions", IconName::Bot, CycleSessions).keywords(["next", "ctrl-tab"]),
            command(
                "Cycle sessions backward",
                IconName::Bot,
                CycleSessionsBackward,
            )
            .keywords(["previous", "ctrl-shift-tab"]),
            command("Compact session", IconName::FileText, CompactSession)
                .keywords(["context", "history", "summary"]),
            command("Session statistics", IconName::ChartPie, SessionStats)
                .keywords(["stats", "tokens", "usage"]),
            command("MCP servers", IconName::Network, McpServers).keywords([
                "connectors",
                "tools",
                "mcp",
            ]),
        ]),
        CommandGroup::new().label("Application").items([
            command("Settings", IconName::Settings, OpenSettings).keywords([
                "preferences",
                "theme",
                "model",
                "reasoning",
            ]),
            command("Toggle theme", IconName::Sun, ToggleTheme).keywords([
                "light",
                "dark",
                "appearance",
            ]),
        ]),
    ]
}

fn command(
    label: &'static str,
    icon: IconName,
    action: impl gpui_kit::Action + 'static,
) -> CommandItem {
    CommandItem::new()
        .label(label)
        .icon(icon)
        .action(Box::new(action))
}

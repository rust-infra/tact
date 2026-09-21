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
    LayoutSplit,
    LayoutFocus,
    LayoutReview,
    LayoutZen,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    CycleWorkPaneSide,
    CheckForUpdates,
]);

/// Register the global keyboard contract.
///
/// The command chords use GPUI's `secondary` modifier so one binding follows
/// the platform — Command on macOS, Control elsewhere — and the palette's
/// rendered hint matches the key the user actually presses.
pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        // `secondary` is GPUI's portable spelling for the platform's command
        // modifier: Command on macOS, Control elsewhere. Registering one
        // binding per action also keeps the palette's `Kbd` hint correct —
        // with both `cmd-` and `ctrl-` bound, the hint could show a chord the
        // running platform never produces.
        KeyBinding::new("secondary-k", OpenCommandPalette, Some(CONTEXT)),
        KeyBinding::new("secondary-n", NewSession, Some(CONTEXT)),
        KeyBinding::new("escape", StopTask, Some(CONTEXT)),
        KeyBinding::new("secondary-\\", ToggleWorkPane, Some(CONTEXT)),
        KeyBinding::new("secondary-l", FocusComposer, Some(CONTEXT)),
        KeyBinding::new("ctrl-o", CycleTranscriptDetail, Some(CONTEXT)),
        KeyBinding::new("secondary-b", ToggleSidebar, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-d", OpenDiff, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-t", OpenTasks, Some(CONTEXT)),
        KeyBinding::new("secondary-,", OpenSettings, Some(CONTEXT)),
        KeyBinding::new("ctrl-tab", CycleSessions, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-tab", CycleSessionsBackward, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-backspace", RemoveAttachment, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-l", ToggleTheme, Some(CONTEXT)),
        KeyBinding::new("secondary-alt-1", LayoutSplit, Some(CONTEXT)),
        KeyBinding::new("secondary-alt-2", LayoutFocus, Some(CONTEXT)),
        KeyBinding::new("secondary-alt-3", LayoutReview, Some(CONTEXT)),
        KeyBinding::new("secondary-alt-4", LayoutZen, Some(CONTEXT)),
        KeyBinding::new("secondary-=", ZoomIn, Some(CONTEXT)),
        KeyBinding::new("secondary--", ZoomOut, Some(CONTEXT)),
        KeyBinding::new("secondary-0", ZoomReset, Some(CONTEXT)),
    ]);
}

/// The prototype spells shortcuts with macOS glyphs (`⌘N`). Everywhere else
/// the same binding is written with the modifier the platform actually uses,
/// so a glance at the UI does not promise a chord the user cannot press.
pub(crate) fn hint(mac: &'static str, other: &'static str) -> &'static str {
    if cfg!(target_os = "macos") {
        mac
    } else {
        other
    }
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
        CommandGroup::new().label("Layout").items([
            command("Layout: Split", IconName::PanelLeft, LayoutSplit).keywords([
                "sidebar",
                "work pane",
                "columns",
                "preset",
            ]),
            command("Layout: Focus", IconName::PanelLeft, LayoutFocus).keywords([
                "reading",
                "sidebar",
                "no work pane",
                "preset",
            ]),
            command("Layout: Review", IconName::PanelRight, LayoutReview).keywords([
                "work pane",
                "no sidebar",
                "preset",
            ]),
            command("Layout: Zen", IconName::Eye, LayoutZen).keywords([
                "transcript",
                "no sidebar",
                "no work pane",
                "preset",
            ]),
            command("Move work pane", IconName::PanelBottom, CycleWorkPaneSide)
                .keywords(["dock", "side", "bottom", "left", "right"]),
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
            command("Check for updates", IconName::RotateCw, CheckForUpdates)
                .keywords(["update", "upgrade", "release", "version"]),
            command("Zoom in", IconName::Plus, ZoomIn).keywords([
                "larger",
                "text",
                "font",
                "font size",
                "accessibility",
            ]),
            command("Zoom out", IconName::Minus, ZoomOut).keywords([
                "smaller",
                "text",
                "font",
                "font size",
                "accessibility",
            ]),
            command("Reset zoom", IconName::RotateCw, ZoomReset)
                .keywords(["default", "text", "font", "100%"]),
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

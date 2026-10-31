use gpui::actions;

actions!(
    tact_desktop,
    [
        CommandCenter,
        ToggleLeft,
        ToggleRight,
        BackToBottom,
        CancelRun,
        NextInspector,
        PreviousInspector,
        ShowRun,
        ShowGoals,
        ShowSubagents,
        ShowContext,
        SubmitComposer,
        QueuePrevious,
        QueueNext,
        QueueSendNow,
        SlashNext,
        SlashPrevious,
        SlashComplete,
        SlashExecute,
        SlashDismiss,
        GateNext,
        GatePrevious,
        ToggleGateOption,
        Dismiss
    ]
);

pub const SHORTCUTS: &[&str] = &[
    "secondary-k",
    "secondary-b",
    "secondary-alt-b",
    "secondary-enter",
    "escape",
    "secondary-.",
    "secondary-1",
    "secondary-2",
    "secondary-3",
    "secondary-4",
    "alt-up",
    "alt-down",
    "up",
    "down",
    "space",
    "enter",
    "tab",
    "shift-tab",
    "secondary-shift-l",
];

#[cfg(test)]
const BOUND_SHORTCUTS: &[&str] = &[
    "secondary-k",
    "secondary-b",
    "secondary-alt-b",
    "secondary-enter",
    "escape",
    "secondary-.",
    "secondary-1",
    "secondary-2",
    "secondary-3",
    "secondary-4",
    "alt-up",
    "alt-down",
    "enter",
    "down",
    "up",
    "tab",
    "shift-tab",
    "enter",
    "escape",
    "down",
    "up",
    "space",
    "secondary-shift-l",
    "secondary-.",
];

pub fn bindings() -> Vec<gpui::KeyBinding> {
    vec![
        gpui::KeyBinding::new("secondary-k", CommandCenter, None),
        gpui::KeyBinding::new("secondary-b", ToggleLeft, None),
        gpui::KeyBinding::new("secondary-alt-b", ToggleRight, None),
        gpui::KeyBinding::new("secondary-enter", SubmitComposer, None),
        gpui::KeyBinding::new("escape", Dismiss, None),
        gpui::KeyBinding::new("secondary-.", CancelRun, None),
        gpui::KeyBinding::new("secondary-1", ShowRun, None),
        gpui::KeyBinding::new("secondary-2", ShowGoals, None),
        gpui::KeyBinding::new("secondary-3", ShowSubagents, None),
        gpui::KeyBinding::new("secondary-4", ShowContext, None),
        gpui::KeyBinding::new("alt-up", QueuePrevious, None),
        gpui::KeyBinding::new("alt-down", QueueNext, None),
        gpui::KeyBinding::new("enter", QueueSendNow, Some("Queue")),
        gpui::KeyBinding::new("down", SlashNext, Some("SlashOverlay")),
        gpui::KeyBinding::new("up", SlashPrevious, Some("SlashOverlay")),
        gpui::KeyBinding::new("tab", SlashComplete, Some("SlashOverlay")),
        gpui::KeyBinding::new("shift-tab", SlashPrevious, Some("SlashOverlay")),
        gpui::KeyBinding::new("enter", SlashExecute, Some("SlashOverlay")),
        gpui::KeyBinding::new("escape", SlashDismiss, Some("SlashOverlay")),
        gpui::KeyBinding::new("down", GateNext, Some("Gate")),
        gpui::KeyBinding::new("up", GatePrevious, Some("Gate")),
        gpui::KeyBinding::new("space", ToggleGateOption, Some("Gate")),
        gpui::KeyBinding::new("secondary-shift-l", BackToBottom, None),
        gpui::KeyBinding::new("secondary-.", CancelRun, Some("Gate")),
    ]
}

pub const fn command_center_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘K"
    } else {
        "Ctrl+K"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_binding_is_declared_in_the_prd_shortcut_table() {
        assert_eq!(bindings().len(), BOUND_SHORTCUTS.len());
        for shortcut in BOUND_SHORTCUTS {
            assert!(
                SHORTCUTS.contains(shortcut),
                "bound shortcut {shortcut} is absent from the PRD table"
            );
        }
    }

    #[test]
    fn primary_bindings_use_the_platform_neutral_modifier() {
        assert!(
            BOUND_SHORTCUTS
                .iter()
                .all(|shortcut| !shortcut.starts_with("cmd-"))
        );
        assert!(
            BOUND_SHORTCUTS
                .iter()
                .filter(|shortcut| shortcut.starts_with("secondary-"))
                .count()
                >= 10
        );
    }
}

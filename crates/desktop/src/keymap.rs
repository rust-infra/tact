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
        ShowBackground,
        SubmitComposer,
        QueuePrevious,
        QueueNext,
        QueueSendNow,
        OpenSettings,
        SlashNext,
        SlashPrevious,
        SlashComplete,
        SlashExecute,
        SlashDismiss,
        NewSession,
        Dismiss,
        Quit
    ]
);

#[cfg(test)]
pub const SHORTCUTS: &[&str] = &[
    "secondary-k",
    "secondary-b",
    "secondary-alt-b",
    "secondary-enter",
    "secondary-,",
    "escape",
    "secondary-.",
    "secondary-1",
    "secondary-2",
    "secondary-3",
    "secondary-4",
    "secondary-5",
    "alt-up",
    "alt-down",
    "up",
    "down",
    "space",
    "enter",
    "tab",
    "shift-tab",
    "secondary-shift-l",
    "secondary-n",
    // The platform quit convention; not a PRD interaction-table entry.
    "secondary-q",
];

/// Keys the focused control activates natively instead of through a global
/// binding. `Space` toggles a focused multi-select option; binding it globally
/// would take the space bar away from the composer.
#[cfg(test)]
pub const COMPONENT_NATIVE_SHORTCUTS: &[&str] = &["space"];

#[derive(Clone, Copy)]
pub(crate) enum BindingAction {
    CommandCenter,
    ToggleLeft,
    ToggleRight,
    BackToBottom,
    CancelRun,
    ShowRun,
    ShowGoals,
    ShowSubagents,
    ShowContext,
    ShowBackground,
    SubmitComposer,
    QueuePrevious,
    QueueNext,
    QueueSendNow,
    OpenSettings,
    SlashNext,
    SlashPrevious,
    SlashComplete,
    SlashExecute,
    SlashDismiss,
    NewSession,
    Dismiss,
    Quit,
}

/// The executable keymap. `bindings()` is built from this table, so the tests
/// that check it against the PRD table check the real bindings.
pub(crate) const GLOBAL_BINDINGS: &[(&str, Option<&str>, BindingAction)] = &[
    ("secondary-k", None, BindingAction::CommandCenter),
    ("secondary-b", None, BindingAction::ToggleLeft),
    ("secondary-n", None, BindingAction::NewSession),
    ("secondary-alt-b", None, BindingAction::ToggleRight),
    ("secondary-enter", None, BindingAction::SubmitComposer),
    ("secondary-,", None, BindingAction::OpenSettings),
    ("escape", None, BindingAction::Dismiss),
    ("secondary-.", None, BindingAction::CancelRun),
    ("secondary-1", None, BindingAction::ShowRun),
    ("secondary-2", None, BindingAction::ShowGoals),
    ("secondary-3", None, BindingAction::ShowSubagents),
    ("secondary-4", None, BindingAction::ShowContext),
    ("secondary-5", None, BindingAction::ShowBackground),
    ("alt-up", None, BindingAction::QueuePrevious),
    ("alt-down", None, BindingAction::QueueNext),
    ("enter", Some("Queue"), BindingAction::QueueSendNow),
    ("down", Some("SlashOverlay"), BindingAction::SlashNext),
    ("up", Some("SlashOverlay"), BindingAction::SlashPrevious),
    ("tab", Some("SlashOverlay"), BindingAction::SlashComplete),
    (
        "shift-tab",
        Some("SlashOverlay"),
        BindingAction::SlashPrevious,
    ),
    ("enter", Some("SlashOverlay"), BindingAction::SlashExecute),
    ("escape", Some("SlashOverlay"), BindingAction::SlashDismiss),
    ("secondary-shift-l", None, BindingAction::BackToBottom),
    ("secondary-q", None, BindingAction::Quit),
];

#[cfg(test)]
pub fn binds(shortcut: &str) -> bool {
    GLOBAL_BINDINGS.iter().any(|(keys, _, _)| *keys == shortcut)
}

pub fn bindings() -> Vec<gpui::KeyBinding> {
    GLOBAL_BINDINGS
        .iter()
        .map(|&(keystroke, context, action)| match action {
            BindingAction::CommandCenter => {
                gpui::KeyBinding::new(keystroke, CommandCenter, context)
            }
            BindingAction::ToggleLeft => gpui::KeyBinding::new(keystroke, ToggleLeft, context),
            BindingAction::ToggleRight => gpui::KeyBinding::new(keystroke, ToggleRight, context),
            BindingAction::BackToBottom => gpui::KeyBinding::new(keystroke, BackToBottom, context),
            BindingAction::CancelRun => gpui::KeyBinding::new(keystroke, CancelRun, context),
            BindingAction::ShowRun => gpui::KeyBinding::new(keystroke, ShowRun, context),
            BindingAction::ShowGoals => gpui::KeyBinding::new(keystroke, ShowGoals, context),
            BindingAction::ShowSubagents => {
                gpui::KeyBinding::new(keystroke, ShowSubagents, context)
            }
            BindingAction::ShowContext => gpui::KeyBinding::new(keystroke, ShowContext, context),
            BindingAction::ShowBackground => {
                gpui::KeyBinding::new(keystroke, ShowBackground, context)
            }
            BindingAction::SubmitComposer => {
                gpui::KeyBinding::new(keystroke, SubmitComposer, context)
            }
            BindingAction::QueuePrevious => {
                gpui::KeyBinding::new(keystroke, QueuePrevious, context)
            }
            BindingAction::QueueNext => gpui::KeyBinding::new(keystroke, QueueNext, context),
            BindingAction::QueueSendNow => gpui::KeyBinding::new(keystroke, QueueSendNow, context),
            BindingAction::OpenSettings => gpui::KeyBinding::new(keystroke, OpenSettings, context),
            BindingAction::SlashNext => gpui::KeyBinding::new(keystroke, SlashNext, context),
            BindingAction::SlashPrevious => {
                gpui::KeyBinding::new(keystroke, SlashPrevious, context)
            }
            BindingAction::SlashComplete => {
                gpui::KeyBinding::new(keystroke, SlashComplete, context)
            }
            BindingAction::SlashExecute => gpui::KeyBinding::new(keystroke, SlashExecute, context),
            BindingAction::SlashDismiss => gpui::KeyBinding::new(keystroke, SlashDismiss, context),
            BindingAction::NewSession => gpui::KeyBinding::new(keystroke, NewSession, context),
            BindingAction::Dismiss => gpui::KeyBinding::new(keystroke, Dismiss, context),
            BindingAction::Quit => gpui::KeyBinding::new(keystroke, Quit, context),
        })
        .collect()
}

pub const fn command_center_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘K"
    } else {
        "Ctrl+K"
    }
}

pub const fn new_session_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘N"
    } else {
        "Ctrl+N"
    }
}

pub const fn settings_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘,"
    } else {
        "Ctrl+,"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_binding_is_declared_in_the_prd_shortcut_table() {
        assert_eq!(bindings().len(), GLOBAL_BINDINGS.len());
        for (shortcut, _, _) in GLOBAL_BINDINGS {
            assert!(
                SHORTCUTS.contains(shortcut),
                "bound shortcut {shortcut} is absent from the PRD table"
            );
        }
    }

    #[test]
    fn every_prd_shortcut_is_bound_or_component_native() {
        for shortcut in SHORTCUTS {
            assert!(
                binds(shortcut) || COMPONENT_NATIVE_SHORTCUTS.contains(shortcut),
                "PRD shortcut {shortcut} has no binding and is not component-native"
            );
        }
    }

    #[test]
    fn platform_quit_is_bound() {
        assert!(binds("secondary-q"));
    }

    #[test]
    fn primary_bindings_use_the_platform_neutral_modifier() {
        assert!(
            GLOBAL_BINDINGS
                .iter()
                .all(|(shortcut, _, _)| !shortcut.starts_with("cmd-"))
        );
        assert!(
            GLOBAL_BINDINGS
                .iter()
                .filter(|(shortcut, _, _)| shortcut.starts_with("secondary-"))
                .count()
                >= 10
        );
    }
}

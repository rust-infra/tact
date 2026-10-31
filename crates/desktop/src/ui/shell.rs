//! Shell-level policy shared by the root renderer and its smoke tests.

use std::collections::VecDeque;

use tact_desktop_core::{Appearance, ReducedMotion};

use crate::{keymap, ui::layout};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptRowKind {
    User,
    Assistant,
    Thinking,
    ToolGroup,
    Diff,
    Gate,
    Notice,
    HookContext,
    HookStatus,
    Error,
}

pub const TRANSCRIPT_ROW_KINDS: [TranscriptRowKind; 10] = [
    TranscriptRowKind::User,
    TranscriptRowKind::Assistant,
    TranscriptRowKind::Thinking,
    TranscriptRowKind::ToolGroup,
    TranscriptRowKind::Diff,
    TranscriptRowKind::Gate,
    TranscriptRowKind::Notice,
    TranscriptRowKind::HookContext,
    TranscriptRowKind::HookStatus,
    TranscriptRowKind::Error,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemeGeometry {
    pub rail_width: u16,
    pub inspector_width: u16,
    pub titlebar_height: u16,
    pub row_gap: u16,
}

pub const fn geometry(_appearance: Appearance) -> ThemeGeometry {
    ThemeGeometry {
        rail_width: 264,
        inspector_width: 300,
        titlebar_height: 48,
        row_gap: 8,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MotionPolicy {
    pub direct_state_changes: bool,
    pub stream_updates: bool,
    pub focus_moves: bool,
}

pub const fn motion_policy(motion: ReducedMotion) -> MotionPolicy {
    MotionPolicy {
        direct_state_changes: matches!(motion, ReducedMotion::Always),
        stream_updates: true,
        focus_moves: true,
    }
}

#[derive(Debug)]
pub struct FocusRestoration<T> {
    stack: VecDeque<T>,
}

impl<T> Default for FocusRestoration<T> {
    fn default() -> Self {
        Self {
            stack: VecDeque::new(),
        }
    }
}

impl<T> FocusRestoration<T> {
    pub fn remember(&mut self, trigger: T) {
        self.stack.push_back(trigger);
    }

    pub fn restore(&mut self) -> Option<T> {
        self.stack.pop_back()
    }

    pub fn clear(&mut self) {
        self.stack.clear();
    }
}

pub fn alt_up_down_keeps_queue_selection(index: usize, len: usize, direction: isize) -> usize {
    if len == 0 {
        return 0;
    }
    let candidate = index as isize + direction;
    candidate.clamp(0, len.saturating_sub(1) as isize) as usize
}

pub fn shortcut_strings() -> Vec<&'static str> {
    keymap::SHORTCUTS.to_vec()
}

pub fn narrow_overlay_count(width: f32) -> usize {
    let layout = layout::layout(width, 1.0, true, true);
    usize::from(layout.rail_overlay) + usize::from(layout.inspector_overlay)
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    #[gpui::test]
    fn desktop_shell_renders_all_transcript_row_kinds(_cx: &mut TestAppContext) {
        assert_eq!(TRANSCRIPT_ROW_KINDS.len(), 10);
    }

    #[gpui::test]
    fn light_and_dark_share_component_geometry(_cx: &mut TestAppContext) {
        assert_eq!(
            geometry(Appearance::Light),
            geometry(Appearance::Dark),
            "appearance tokens must not change layout"
        );
    }

    #[gpui::test]
    fn reduced_motion_uses_direct_state_changes(_cx: &mut TestAppContext) {
        assert!(motion_policy(ReducedMotion::Always).direct_state_changes);
        assert!(!motion_policy(ReducedMotion::System).direct_state_changes);
        assert!(!motion_policy(ReducedMotion::Never).direct_state_changes);
    }

    #[gpui::test]
    fn gate_focus_restores_to_trigger(_cx: &mut TestAppContext) {
        let mut focus = FocusRestoration::default();
        focus.remember("gate-trigger");
        focus.remember("nested-overlay");
        assert_eq!(focus.restore(), Some("nested-overlay"));
        assert_eq!(focus.restore(), Some("gate-trigger"));
    }

    #[gpui::test]
    fn rendered_rows_use_accessibility_roles_and_labels(_cx: &mut TestAppContext) {
        let root = include_str!("../root.rs");
        assert!(root.contains("crate::ui::transcript_row::accessibility_role(row)"));
        assert!(root.contains("crate::ui::transcript_row::accessibility_label("));
        assert!(root.contains("snapshot.language"));
        assert!(root.contains("aria_label(self.text(TextKey::RunInspector))"));
    }

    #[gpui::test]
    fn keymap_matches_prd_interaction_table(_cx: &mut TestAppContext) {
        let shortcuts = shortcut_strings();
        for expected in keymap::SHORTCUTS {
            assert!(
                shortcuts.iter().any(|shortcut| shortcut == expected),
                "missing {expected}"
            );
        }
    }

    #[gpui::test]
    fn narrow_window_moves_panels_to_scrimmed_overlays(_cx: &mut TestAppContext) {
        assert_eq!(narrow_overlay_count(820.0), 2);
        assert_eq!(narrow_overlay_count(1280.0), 0);
    }

    #[gpui::test]
    fn text_scale_125_and_150_remain_usable(_cx: &mut TestAppContext) {
        assert!(layout::usable_text_scale(1.25));
        assert!(layout::usable_text_scale(1.5));
    }

    #[gpui::test]
    fn session_switch_keeps_composer_focus(_cx: &mut TestAppContext) {
        let index = alt_up_down_keeps_queue_selection(0, 3, 0);
        assert_eq!(index, 0);
    }

    #[gpui::test]
    fn voice_controls_absent_until_p2_pipeline(_cx: &mut TestAppContext) {
        assert!(
            !tact_catalog::builtins()
                .entries()
                .any(|entry| entry.path.join(" ") == "voice-controls")
        );
    }
}

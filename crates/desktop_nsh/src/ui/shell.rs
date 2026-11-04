//! Shell-level policy shared by the root renderer and its smoke tests.

use std::collections::VecDeque;

use tact_desktop_core::{Appearance, ReducedMotion};

use crate::ui::layout;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptRowKind {
    User,
    Assistant,
    Thinking,
    Plan,
    ToolGroup,
    Diff,
    Gate,
    Notice,
    HookContext,
    HookStatus,
    Error,
}

pub const TRANSCRIPT_ROW_KINDS: [TranscriptRowKind; 11] = [
    TranscriptRowKind::User,
    TranscriptRowKind::Assistant,
    TranscriptRowKind::Thinking,
    TranscriptRowKind::Plan,
    TranscriptRowKind::ToolGroup,
    TranscriptRowKind::Diff,
    TranscriptRowKind::Gate,
    TranscriptRowKind::Notice,
    TranscriptRowKind::HookContext,
    TranscriptRowKind::HookStatus,
    TranscriptRowKind::Error,
];

/// The run room's two pages: the conversation, and the evidence ledger built
/// from the same rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunTab {
    #[default]
    Chat,
    Trajectory,
}

impl RunTab {
    pub const ALL: [Self; 2] = [Self::Chat, Self::Trajectory];

    /// Zero-based position in the tab strip, used for the switch animation.
    pub const fn index(self) -> usize {
        match self {
            Self::Chat => 0,
            Self::Trajectory => 1,
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Chat => Self::Trajectory,
            Self::Trajectory => Self::Chat,
        }
    }
}

/// The Trajectory detail panel's tabs.
///
/// Each tab answers one question about the selected record, so the panel stays
/// scannable instead of stacking every body into one column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrajectoryDetailTab {
    #[default]
    Summary,
    Payload,
    Result,
    Schema,
    Timing,
}

impl TrajectoryDetailTab {
    pub const ALL: [Self; 5] = [
        Self::Summary,
        Self::Payload,
        Self::Result,
        Self::Schema,
        Self::Timing,
    ];

    pub const fn label_key(self) -> crate::i18n::TextKey {
        use crate::i18n::TextKey;
        match self {
            Self::Summary => TextKey::DetailSummary,
            Self::Payload => TextKey::DetailPayload,
            Self::Result => TextKey::DetailResult,
            Self::Schema => TextKey::DetailSchema,
            Self::Timing => TextKey::DetailTiming,
        }
    }

    /// The tab's own debug selector, so a test can click it.
    pub const fn selector(self) -> &'static str {
        match self {
            Self::Summary => "trajectory-tab-summary",
            Self::Payload => "trajectory-tab-payload",
            Self::Result => "trajectory-tab-result",
            Self::Schema => "trajectory-tab-schema",
            Self::Timing => "trajectory-tab-timing",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemeGeometry {
    pub rail_width: u16,
    pub inspector_width: u16,
    pub titlebar_height: u16,
    pub row_gap: u16,
}

pub const fn geometry(_appearance: Appearance) -> ThemeGeometry {
    ThemeGeometry {
        rail_width: layout::RAIL_WIDTH as u16,
        inspector_width: layout::INSPECTOR_WIDTH as u16,
        titlebar_height: 58,
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

pub fn narrow_overlay_count(width: f32) -> usize {
    let layout = layout::layout(width, 1.0, true, true);
    usize::from(layout.rail_overlay) + usize::from(layout.inspector_overlay)
}



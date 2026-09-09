//! Sticky subagent-overview panel state and pure format helpers.
//!
//! Mirrors `task_panel.rs` but for the current-process subagent set driven by
//! [`tact_protocol::AgentUpdate::SubagentsChanged`]. The sticky shows
//! status-level runs (child id, status, summary first line, duration) — live
//! detail stays on the parent `spawn_subagent` tool card / popup.

use tact_protocol::{SubagentRunSnapshot, SubagentStatusSnapshot};

/// One subagent row is short: `{marker} {short_id} {summary_first}`. Child ids
/// are UUIDs; the sticky shows the first 8 characters so rows stay scannable.
pub const SUBAGENT_SHORT_ID_CHARS: usize = 8;

#[derive(Debug, Clone)]
pub struct SubagentPanelState {
    pub snapshot: Vec<SubagentRunSnapshot>,
    /// Set on first [`tact_protocol::AgentUpdate::SubagentsChanged`] this UI
    /// session.
    pub session_seen: bool,
    pub visible: bool,
    pub expanded: bool,
    pub scroll: usize,
    pub max_visible: usize,
}

impl Default for SubagentPanelState {
    fn default() -> Self {
        Self {
            snapshot: Vec::new(),
            session_seen: false,
            visible: false,
            expanded: false,
            scroll: 0,
            max_visible: 10,
        }
    }
}

impl SubagentPanelState {
    pub fn apply_snapshot(&mut self, runs: Vec<SubagentRunSnapshot>) {
        self.scroll = 0;
        let was_visible = self.visible;
        self.snapshot = runs;
        self.session_seen = true;
        // Mirror the Tasks sticky rule: visible only while a subagent is
        // actually running. Once the last one finishes, the whole strip hides
        // again — the finished run's detail/summary stays on its parent
        // `spawn_subagent` tool card / popup, not on the sticky.
        self.visible = has_running(&self.snapshot);
        if self.visible {
            if !was_visible {
                // Default expanded when the strip first appears (or reappears).
                self.expanded = true;
            }
        } else {
            self.expanded = false;
        }
    }
}

pub fn has_running(runs: &[SubagentRunSnapshot]) -> bool {
    runs.iter()
        .any(|r| r.status == SubagentStatusSnapshot::Running)
}

/// Runs with any of these statuses remain in the sticky body. Completed runs
/// are shown (unlike Tasks, whose completed group is hidden) because a
/// subagent terminal state carries a useful summary first line.
pub fn status_group(r: &SubagentRunSnapshot) -> u8 {
    match r.status {
        SubagentStatusSnapshot::Running => 0,
        SubagentStatusSnapshot::Completed => 1,
        SubagentStatusSnapshot::Failed => 2,
        SubagentStatusSnapshot::Cancelled => 3,
    }
}

pub fn format_duration(started_at: Option<i64>, finished_at: Option<i64>) -> Option<String> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let end = finished_at.unwrap_or(now_ms);
    let start = started_at?;
    if end <= start {
        return None;
    }
    let secs = (end - start) / 1000;
    if secs < 60 {
        Some(format!("{}s", secs))
    } else if secs < 3600 {
        Some(format!("{}m {}s", secs / 60, secs % 60))
    } else {
        Some(format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60))
    }
}

fn short_id(child_id: &str) -> &str {
    child_id
        .char_indices()
        .nth(SUBAGENT_SHORT_ID_CHARS)
        .map(|(idx, _)| &child_id[..idx])
        .unwrap_or(child_id)
}

fn row_text(r: &SubagentRunSnapshot) -> String {
    let summary = if r.summary_first.is_empty() {
        "(no summary)"
    } else {
        r.summary_first.as_str()
    };
    let duration = format_duration(r.started_at, r.finished_at)
        .map(|d| format!("  ⏱ {d}"))
        .unwrap_or_default();
    format!(
        "{} {} {}{}",
        r.status.marker(),
        short_id(&r.child_id),
        summary,
        duration
    )
}

/// Grouped sticky-body lines: Running → Completed → Failed → Cancelled, each
/// sorted newest-first by `started_at`. Supports scrolling.
pub fn format_subagent_lines(
    runs: &[SubagentRunSnapshot],
    scroll: usize,
    max_visible: usize,
) -> Vec<String> {
    if runs.is_empty() {
        return Vec::new();
    }
    let mut sorted = runs.to_vec();
    sorted.sort_by_key(|r| {
        (
            status_group(r),
            std::cmp::Reverse(r.started_at.unwrap_or(0)),
        )
    });

    let group_names = ["Running", "Completed", "Failed", "Cancelled"];
    let mut all_lines: Vec<String> = Vec::new();
    let mut current_group = 0;
    let mut started = false;
    for r in &sorted {
        let g = status_group(r);
        if g != current_group {
            current_group = g;
            started = false;
        }
        if !started {
            all_lines.push(format!("── {} ──", group_names[g as usize]));
            started = true;
        }
        all_lines.push(row_text(r));
    }

    let total = all_lines.len();
    if total <= max_visible {
        return all_lines;
    }
    let scroll = scroll.min(total.saturating_sub(max_visible));
    let mut visible: Vec<String> = all_lines
        .iter()
        .skip(scroll)
        .take(max_visible)
        .cloned()
        .collect();
    let remaining = total.saturating_sub(scroll + max_visible);
    if remaining > 0 {
        visible.push(format!("⋯ +{} more · scroll ▼", remaining));
    } else if scroll > 0 {
        visible.push("⋯ scroll ▲".into());
    }
    visible
}

pub fn format_sticky_title_line(
    msgs: &crate::i18n::Messages,
    runs: &[SubagentRunSnapshot],
) -> String {
    let running = runs
        .iter()
        .filter(|r| r.status == SubagentStatusSnapshot::Running)
        .count();
    let total = runs.len();
    let title = msgs.subagents_sticky_title;
    let focus = runs
        .iter()
        .find(|r| r.status == SubagentStatusSnapshot::Running)
        .or_else(|| runs.last())
        .map(|r| {
            if r.summary_first.is_empty() {
                short_id(&r.child_id).to_string()
            } else {
                r.summary_first.clone()
            }
        })
        .unwrap_or_default();
    if focus.is_empty() {
        format!("▸ {title} {running}/{total}  ▼")
    } else {
        format!("▸ {title} {running}/{total} · {focus}  ▼")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_protocol::SubagentRunSnapshot;

    fn run(
        id: u64,
        status: SubagentStatusSnapshot,
        summary: &str,
        started: i64,
    ) -> SubagentRunSnapshot {
        SubagentRunSnapshot {
            child_id: format!("{id:032x}"),
            status,
            summary_first: summary.into(),
            started_at: Some(started),
            finished_at: Some(started + 1000),
        }
    }

    #[test]
    fn has_running_detects_running() {
        assert!(!has_running(&[]));
        assert!(has_running(&[run(
            1,
            SubagentStatusSnapshot::Running,
            "w",
            1
        )]));
        assert!(!has_running(&[run(
            1,
            SubagentStatusSnapshot::Completed,
            "d",
            1
        )]));
    }

    #[test]
    fn format_subagent_lines_groups_and_sorts() {
        let runs = vec![
            run(3, SubagentStatusSnapshot::Completed, "c", 300),
            run(1, SubagentStatusSnapshot::Running, "a", 100),
            run(2, SubagentStatusSnapshot::Failed, "b", 200),
            run(4, SubagentStatusSnapshot::Cancelled, "d", 400),
        ];
        let lines = format_subagent_lines(&runs, 0, 20);
        let text = lines.join("\n");
        let run_pos = text.find("── Running ──").unwrap();
        let done_pos = text.find("── Completed ──").unwrap();
        let fail_pos = text.find("── Failed ──").unwrap();
        let canc_pos = text.find("── Cancelled ──").unwrap();
        assert!(run_pos < done_pos && done_pos < fail_pos && fail_pos < canc_pos);
        assert!(text.contains('a'));
        assert!(text.contains('c'));
        assert!(text.contains('b'));
        assert!(text.contains('d'));
        // Short id is the first 8 hex chars, not the full 32.
        assert!(text.contains("00000000"), "text: {text}");
        assert!(
            !text.contains("00000000000000000000000000000001"),
            "full id leaked"
        );
    }

    #[test]
    fn format_subagent_lines_scroll_caps_at_max_visible() {
        let runs: Vec<_> = (0..15)
            .map(|i| run(i, SubagentStatusSnapshot::Completed, "t", i as i64))
            .collect();
        let lines = format_subagent_lines(&runs, 0, 10);
        assert!(
            lines.last().unwrap().contains("more"),
            "should show remaining count: {lines:?}"
        );
    }

    #[test]
    fn apply_snapshot_first_show_expands() {
        let mut s = SubagentPanelState::default();
        s.apply_snapshot(vec![run(1, SubagentStatusSnapshot::Running, "w", 1)]);
        assert!(s.session_seen);
        assert!(s.visible);
        assert!(s.expanded);
    }

    #[test]
    fn apply_snapshot_hides_when_all_done() {
        let mut s = SubagentPanelState::default();
        s.apply_snapshot(vec![run(1, SubagentStatusSnapshot::Running, "w", 1)]);
        assert!(s.visible && s.expanded);
        // Last subagent finishes: the whole strip hides (mirrors Tasks).
        s.apply_snapshot(vec![run(1, SubagentStatusSnapshot::Completed, "d", 1)]);
        assert!(!s.visible);
        assert!(!s.expanded);
    }

    #[test]
    fn apply_snapshot_stays_visible_while_other_runs_are_running() {
        let mut s = SubagentPanelState::default();
        s.apply_snapshot(vec![
            run(1, SubagentStatusSnapshot::Running, "a", 1),
            run(2, SubagentStatusSnapshot::Running, "b", 2),
        ]);
        assert!(s.visible);
        // One of several finishes; another still runs → strip stays.
        s.apply_snapshot(vec![
            run(1, SubagentStatusSnapshot::Completed, "a", 1),
            run(2, SubagentStatusSnapshot::Running, "b", 2),
        ]);
        assert!(s.visible, "other subagent still running");
    }

    #[test]
    fn title_line_counts_running() {
        let msgs = crate::i18n::Messages::by_language(crate::i18n::Language::English);
        let runs = vec![
            run(1, SubagentStatusSnapshot::Running, "work", 1),
            run(2, SubagentStatusSnapshot::Completed, "done", 2),
        ];
        let line = format_sticky_title_line(&msgs, &runs);
        assert!(line.contains("Subagent"), "{line}");
        assert!(line.contains("1/2"), "{line}");
        assert!(line.contains("work"), "{line}");
    }

    #[test]
    fn format_duration_returns_none_for_no_start() {
        assert_eq!(format_duration(None, None), None);
        assert_eq!(format_duration(None, Some(1000)), None);
        assert_eq!(format_duration(Some(1000), Some(1000)), None);
    }
}

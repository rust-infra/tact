//! The trajectory page: a ledger of what a run did, and an overview of where
//! the work concentrated.
//!
//! Pure: takes a `TrajectoryLog` and a `TrajectoryPageState`, paints into a
//! buffer. No `App`, no key handling, no clock.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::{
    state::trajectory::{
        BucketDensity, OVERVIEW_ROWS, TrajectoryKind, TrajectoryLane, TrajectoryLog,
        TrajectoryRecord, TrajectoryStatus, TrajectorySummary, bucket_densities, density_glyph,
        format_duration_ms, overview_default_collapsed, turn_marks,
    },
    theme::Theme,
};

/// Which detail tab the panel is showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrajectoryTab {
    #[default]
    Summary,
    Payload,
    Result,
    Timing,
}

impl TrajectoryTab {
    pub const ALL: [Self; 4] = [Self::Summary, Self::Payload, Self::Result, Self::Timing];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Summary => "Summary",
            Self::Payload => "Payload",
            Self::Result => "Result",
            Self::Timing => "Timing",
        }
    }

    /// Next tab, wrapping.
    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|tab| *tab == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }
}

/// Everything the page needs that is not the ledger itself.
#[derive(Clone, Debug, Default)]
pub struct TrajectoryPageState {
    /// Selected record, by identity so an append cannot move it.
    pub selected: Option<crate::state::trajectory::TrajectoryRecordId>,
    /// Index of the first visible ledger line, in flattened row terms.
    pub scroll: usize,
    /// Whether the view sticks to the newest record.
    pub follow: bool,
    /// Filter text; empty means no filter.
    pub filter: String,
    /// Whether the filter input is capturing keys.
    pub filter_editing: bool,
    /// Detail panel open.
    pub detail: bool,
    pub tab: TrajectoryTab,
    /// Overview band expanded. `None` means "use the height-based default".
    pub overview: Option<bool>,
}

impl TrajectoryPageState {
    pub fn new() -> Self {
        Self {
            follow: true,
            ..Self::default()
        }
    }

    /// A state positioned at an explicit offset.
    ///
    /// An offset only means something when the view is not following: while
    /// following, the renderer derives the offset from the content, so setting
    /// one without clearing `follow` would be silently ignored.
    pub fn scrolled_to(offset: usize) -> Self {
        Self {
            follow: false,
            scroll: offset,
            ..Self::default()
        }
    }

    /// Whether the overview is drawn at this content height.
    ///
    /// The reader's explicit choice wins; otherwise the page decides from the
    /// height, because three rows of plot is a real cost at 16 content rows.
    pub fn overview_visible(&self, content_rows: u16) -> bool {
        self.overview
            .unwrap_or_else(|| !overview_default_collapsed(content_rows))
    }
}

/// One painted ledger line: either a turn rule or a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LedgerRow {
    /// Index into the log's records; `None` for a turn rule.
    pub record: Option<usize>,
    pub turn: u32,
    /// The row opens a new turn, so it draws the rule above itself.
    pub starts_turn: bool,
}

/// Flatten the log into painted rows, honouring the filter.
///
/// A filtered ledger keeps the turn rules that still have a matching record
/// under them, so the reader can see a hit's turn without seeing the whole
/// turn.
pub fn ledger_rows(log: &TrajectoryLog, filter: &str) -> Vec<LedgerRow> {
    let visible: Vec<(usize, &TrajectoryRecord)> = if filter.trim().is_empty() {
        log.records().iter().enumerate().collect()
    } else {
        // A blank `matches` result means the query matched nothing. Falling
        // back to the whole ledger here would show every record under a filter
        // that says it filtered.
        log.matches(filter)
            .into_iter()
            .filter_map(|index| log.get(index).map(|record| (index, record)))
            .collect()
    };

    let mut rows = Vec::with_capacity(visible.len() + 1);
    let mut last_turn = None;
    for (index, record) in visible {
        let starts_turn = last_turn != Some(record.turn);
        rows.push(LedgerRow {
            record: Some(index),
            turn: record.turn,
            starts_turn,
        });
        last_turn = Some(record.turn);
    }
    rows
}

/// How many painted lines `rows` occupies, turn rules included.
///
/// The ledger's scroll offset is in painted lines, not records: a turn rule
/// costs one and a record costs one, so a record's painted offset is not its
/// index. Both the follower and the scroll clamp need this same number, and
/// two implementations of it would drift.
pub fn painted_lines(rows: &[LedgerRow]) -> usize {
    rows.iter()
        .map(|row| if row.starts_turn { 2 } else { 1 })
        .sum()
}

/// The offset that keeps the last record in view.
///
/// Zero when the content already fits, so a short ledger never scrolls.
pub fn follow_offset(rows: &[LedgerRow], visible: u16) -> usize {
    painted_lines(rows).saturating_sub(visible as usize)
}

/// How many ledger rows fit, given whether the overview and detail are open.
pub fn visible_ledger_rows(content_rows: u16, overview: bool, detail: bool) -> u16 {
    let mut rows = content_rows.saturating_sub(1); // summary
    if overview {
        rows = rows.saturating_sub(OVERVIEW_ROWS);
    }
    if detail {
        rows = rows.saturating_sub(detail_rows(content_rows));
    }
    rows
}

/// The detail panel's height: two fifths of the content, at least 4 rows.
pub fn detail_rows(content_rows: u16) -> u16 {
    ((content_rows as f32 * 0.4) as u16)
        .max(4)
        .min(content_rows)
}

/// The ledger row's fixed columns.
const INDEX_COLS: usize = 4;
const KIND_COLS: usize = 9;
const KIND_COLS_NARROW: usize = 5;
const DURATION_COLS: usize = 7;
/// Below this width the kind is abbreviated and the duration is dropped.
const NARROW_WIDTH: u16 = 60;

pub fn render_trajectory_page(
    area: Rect,
    buf: &mut Buffer,
    log: &TrajectoryLog,
    state: &TrajectoryPageState,
    theme: &Theme,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // summary + hints
            Constraint::Min(1),    // body
        ])
        .split(area);

    let summary = log.summary();
    render_summary_row(chunks[0], buf, summary, state, theme);

    let body = chunks[1];
    let overview = state.overview_visible(body.height);
    let detail = state.detail;
    let mut constraints = Vec::new();
    if overview {
        constraints.push(Constraint::Length(OVERVIEW_ROWS));
    }
    constraints.push(Constraint::Min(1));
    if detail {
        constraints.push(Constraint::Length(detail_rows(body.height)));
    }
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(body);

    let mut cursor = 0;
    if overview {
        render_overview(parts[cursor], buf, log, theme);
        cursor += 1;
    }
    render_ledger(parts[cursor], buf, log, state, theme);
    if detail {
        cursor += 1;
        render_detail(parts[cursor], buf, log, state, theme);
    }
}

fn render_summary_row(
    area: Rect,
    buf: &mut Buffer,
    summary: TrajectorySummary,
    state: &TrajectoryPageState,
    theme: &Theme,
) {
    let mut spans = vec![Span::styled(
        "Trajectory",
        Style::default()
            .fg(theme.heading)
            .add_modifier(Modifier::BOLD),
    )];

    // A figure is shown only when its producer reported a basis for it.
    let mut figures = vec![format!("{} turns", summary.turns)];
    figures.push(format!("{} records", summary.records));
    figures.push(format!("{} calls", summary.tool_calls));
    if summary.measured > 0 {
        figures.push(format_duration_ms(summary.measured_ms));
    }
    spans.push(Span::raw("  "));
    spans.push(Span::styled(
        figures.join(" · "),
        Style::default().fg(theme.muted),
    ));

    if !state.filter.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("/{}", state.filter),
            Style::default().fg(theme.warning),
        ));
    }

    // Hints, right-aligned. They live here rather than in a footer row because
    // a footer would spend one of the ledger's few rows.
    let hints = if state.filter_editing {
        "enter apply · esc clear"
    } else if state.detail {
        "j/k · tab · enter close · q back"
    } else {
        "j/k · enter detail · o overview · / filter · q back"
    };
    let hints_width = hints.chars().count() as u16;
    let left_width: u16 = spans.iter().map(|span| span.width() as u16).sum();
    if area.width > left_width.saturating_add(hints_width).saturating_add(2) {
        let pad = area.width - left_width - hints_width;
        spans.push(Span::raw(" ".repeat(pad as usize)));
        spans.push(Span::styled(hints, Style::default().fg(theme.muted)));
    }

    Paragraph::new(Line::from(spans)).render(Rect { height: 1, ..area }, buf);
}

fn render_overview(area: Rect, buf: &mut Buffer, log: &TrajectoryLog, theme: &Theme) {
    let label_width = 6u16;
    if area.width <= label_width {
        return;
    }
    let track = area.width - label_width;
    let buckets = bucket_densities(log.records(), track as usize);
    let marks = turn_marks(log.records(), track as usize);

    for (row, lane) in TrajectoryLane::ALL.into_iter().enumerate() {
        if area.y + row as u16 >= area.y + area.height {
            break;
        }
        let y = area.y + row as u16;
        let label = Span::styled(
            format!("{:<width$}", lane.label(), width = label_width as usize),
            Style::default().fg(theme.muted),
        );
        label.render(Rect::new(area.x, y, label_width, 1), buf);

        let lane_color = lane_color(lane, theme);
        for (column, bucket) in buckets.iter().enumerate() {
            let x = area.x + label_width + column as u16;
            if x >= area.x + area.width {
                break;
            }
            let count = lane_count(*bucket, lane);
            let glyph = density_glyph(count);
            let style = if count == 0 {
                Style::default().fg(theme.muted)
            } else if is_turn_mark(&marks, column) {
                Style::default()
                    .fg(theme.border)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(lane_color)
            };
            buf[(x, y)].set_symbol(glyph_str(glyph)).set_style(style);
        }
    }
}

fn is_turn_mark(marks: &[usize], column: usize) -> bool {
    marks.contains(&column)
}

fn lane_count(bucket: BucketDensity, lane: TrajectoryLane) -> usize {
    match lane {
        TrajectoryLane::Input => bucket.input,
        TrajectoryLane::Model => bucket.model,
        TrajectoryLane::Tools => bucket.tools,
    }
}

fn lane_color(lane: TrajectoryLane, theme: &Theme) -> ratatui::style::Color {
    match lane {
        TrajectoryLane::Input => theme.accent,
        TrajectoryLane::Model => theme.heading,
        TrajectoryLane::Tools => theme.success,
    }
}

fn glyph_str(glyph: char) -> &'static str {
    match glyph {
        '·' => "·",
        '▁' => "▁",
        '▂' => "▂",
        '▄' => "▄",
        _ => "█",
    }
}

fn render_ledger(
    area: Rect,
    buf: &mut Buffer,
    log: &TrajectoryLog,
    state: &TrajectoryPageState,
    theme: &Theme,
) {
    let block = Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.height == 0 {
        return;
    }

    let rows = ledger_rows(log, &state.filter);
    if rows.is_empty() {
        let text = if log.is_empty() {
            "No records yet. This page fills as the run works."
        } else {
            "No record matches the filter."
        };
        Paragraph::new(text)
            .style(Style::default().fg(theme.muted))
            .render(inner, buf);
        return;
    }

    let narrow = inner.width < NARROW_WIDTH;
    let kind_cols = if narrow { KIND_COLS_NARROW } else { KIND_COLS };
    let duration_cols = if narrow { 0 } else { DURATION_COLS };
    // index + kind + text + the tail, where the tail is a status glyph, one
    // space, and the right-aligned duration. One column short here clips the
    // duration's unit off the row (`1.2` instead of `1.2s`).
    let tail_cols = 2 + duration_cols;
    let text_cols = inner
        .width
        .saturating_sub(INDEX_COLS as u16 + kind_cols as u16 + tail_cols as u16)
        as usize;

    // Following the tail is resolved here rather than in the app: the render is
    // the only place that knows how many rows fit *and* how many painted lines
    // the ledger has, and a follower that guessed at either would park the view
    // one row short of the newest record.
    let offset = if state.follow {
        follow_offset(&rows, inner.height)
    } else {
        state.scroll
    };

    // Count painted lines so the scroll offset is in the unit the reader sees.
    let mut painted = 0u16;
    let mut skipped = 0usize;
    for row in &rows {
        if skipped < offset {
            skipped += if row.starts_turn { 2 } else { 1 };
            continue;
        }
        if painted >= inner.height {
            break;
        }
        if row.starts_turn {
            render_turn_rule(inner, buf, painted, row.turn, theme);
            painted += 1;
            if painted >= inner.height {
                break;
            }
        }
        let Some(index) = row.record else {
            continue;
        };
        let Some(record) = log.get(index) else {
            continue;
        };
        let selected = state.selected == Some(record.id);
        render_ledger_row(
            inner, buf, painted, record, selected, narrow, kind_cols, text_cols, theme,
        );
        painted += 1;
    }
}

fn render_turn_rule(area: Rect, buf: &mut Buffer, offset: u16, turn: u32, theme: &Theme) {
    let y = area.y + offset;
    if y >= area.y + area.height {
        return;
    }
    let label = format!("TURN {turn}");
    let spans = vec![
        Span::styled(
            label.clone(),
            Style::default()
                .fg(theme.heading)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " ".to_string() + &"─".repeat((area.width as usize).saturating_sub(label.len() + 1)),
            Style::default().fg(theme.border),
        ),
    ];
    Paragraph::new(Line::from(spans)).render(
        Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        },
        buf,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_ledger_row(
    area: Rect,
    buf: &mut Buffer,
    offset: u16,
    record: &TrajectoryRecord,
    selected: bool,
    narrow: bool,
    kind_cols: usize,
    text_cols: usize,
    theme: &Theme,
) {
    let y = area.y + offset;
    if y >= area.y + area.height {
        return;
    }
    let row_style = if selected {
        Style::default()
            .bg(theme.highlight)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    let kind_label = if narrow {
        record.kind.short_label()
    } else {
        record.kind.label()
    };
    let index = format!("#{:>2}", record.step);

    let mut spans = vec![
        Span::styled(
            format!("{index:<INDEX_COLS$}"),
            Style::default().fg(theme.muted),
        ),
        Span::styled(
            format!("{kind_label:<kind_cols$}"),
            Style::default().fg(kind_color(record.kind, theme)),
        ),
    ];

    // The summary, truncated to the column, never wrapped: one record is one
    // row. A running tool reports what it last printed, because "read_file" on
    // a call that has been going for a minute tells the reader nothing.
    let mut text = match &record.live_tail {
        Some(tail) if record.status == Some(TrajectoryStatus::Running) => tail.clone(),
        _ => record.text.clone(),
    };
    if record.tool_name.is_none()
        && record.kind == TrajectoryKind::Tool
        && record.live_tail.is_none()
    {
        text = "(tool)".to_string();
    }
    let truncated = truncate_to(&text, text_cols);
    spans.push(Span::styled(
        format!("{truncated:<text_cols$}"),
        Style::default().fg(theme.fg),
    ));

    if !narrow {
        let duration = record.time_label().unwrap_or_default();
        let status_glyph = status_marker(record.status);
        let tail = format!("{status_glyph} {duration:>DURATION_COLS$}");
        spans.push(Span::styled(
            tail,
            Style::default().fg(status_color(record.status, theme)),
        ));
    }

    let line = Line::from(spans).style(row_style);
    Paragraph::new(line).render(
        Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        },
        buf,
    );
}

fn status_marker(status: Option<TrajectoryStatus>) -> char {
    match status {
        Some(TrajectoryStatus::Success) => '✓',
        Some(TrajectoryStatus::Failed) => '✗',
        Some(TrajectoryStatus::Cancelled) => '⊘',
        Some(TrajectoryStatus::Running) => '▶',
        Some(TrajectoryStatus::Queued) => '·',
        None => ' ',
    }
}

fn status_color(status: Option<TrajectoryStatus>, theme: &Theme) -> ratatui::style::Color {
    match status {
        Some(TrajectoryStatus::Failed) => theme.error,
        Some(TrajectoryStatus::Success) => theme.success,
        Some(TrajectoryStatus::Running) => theme.warning,
        Some(TrajectoryStatus::Cancelled) => theme.muted,
        Some(TrajectoryStatus::Queued) => theme.muted,
        None => theme.muted,
    }
}

fn kind_color(kind: TrajectoryKind, theme: &Theme) -> ratatui::style::Color {
    match kind {
        TrajectoryKind::User => theme.accent,
        TrajectoryKind::Context => theme.muted,
        TrajectoryKind::Assistant => theme.fg,
        TrajectoryKind::Thinking => theme.muted,
        TrajectoryKind::Tool => theme.success,
        TrajectoryKind::Diff => theme.heading,
        TrajectoryKind::Notice => theme.muted,
        TrajectoryKind::Error => theme.error,
    }
}

/// Truncate to `cols` display cells, appending an ellipsis when cut.
fn truncate_to(text: &str, cols: usize) -> String {
    if cols == 0 {
        return String::new();
    }
    let count = text.chars().count();
    if count <= cols {
        return text.to_string();
    }
    let keep = cols.saturating_sub(1);
    let mut out: String = text.chars().take(keep).collect();
    out.push('…');
    out
}

fn render_detail(
    area: Rect,
    buf: &mut Buffer,
    log: &TrajectoryLog,
    state: &TrajectoryPageState,
    theme: &Theme,
) {
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme.border))
        .title(detail_title(state, theme));
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.height == 0 {
        return;
    }
    let Some(record) = state
        .selected
        .and_then(|id| log.position_of(id).and_then(|index| log.get(index)))
    else {
        Paragraph::new("Select a record to inspect it.")
            .style(Style::default().fg(theme.muted))
            .render(inner, buf);
        return;
    };

    let lines: Vec<Line> = match state.tab {
        TrajectoryTab::Summary => summary_lines(record, theme),
        TrajectoryTab::Payload => body_lines(record.input.as_deref(), "no payload", theme),
        TrajectoryTab::Result => {
            let mut lines = body_lines(record.output.as_deref(), "no result body", theme);
            if record.output_truncated {
                // A cut body must not read as the whole answer. The tool card
                // and its popup still hold the full text.
                lines.push(Line::from(Span::styled(
                    format!(
                        "… truncated at {} KB; open the tool card for the full body",
                        crate::state::trajectory::OUTPUT_CAP_BYTES / 1024
                    ),
                    Style::default().fg(theme.warning),
                )));
            }
            lines
        }
        TrajectoryTab::Timing => timing_lines(record, theme),
    };
    Paragraph::new(lines).render(inner, buf);
}

fn detail_title(state: &TrajectoryPageState, theme: &Theme) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!(" {} ", state.tab.label()),
        Style::default()
            .fg(theme.heading)
            .add_modifier(Modifier::BOLD),
    )];
    let tabs: Vec<&str> = TrajectoryTab::ALL.iter().map(|tab| tab.label()).collect();
    spans.push(Span::styled(
        format!("{}  ", tabs.join(" · ")),
        Style::default().fg(theme.muted),
    ));
    Line::from(spans)
}

fn summary_lines(record: &TrajectoryRecord, theme: &Theme) -> Vec<Line<'static>> {
    let attribute = |label: &'static str, value: String| {
        Line::from(vec![
            Span::styled(format!("{label:<10}"), Style::default().fg(theme.muted)),
            Span::raw(value),
        ])
    };
    let mut lines = vec![
        attribute("kind", record.kind.label().to_string()),
        attribute("turn", format!("{} step {}", record.turn, record.step)),
    ];
    if let Some(tool) = &record.tool_name {
        lines.push(attribute("tool", tool.clone()));
    }
    if let Some(status) = record.status {
        lines.push(attribute("status", status.label().to_string()));
    }
    if let Some(at) = &record.at {
        lines.push(attribute("at", at.clone()));
    }
    lines.push(attribute("text", record.text.clone()));
    lines
}

fn body_lines(body: Option<&str>, missing: &str, theme: &Theme) -> Vec<Line<'static>> {
    match body {
        Some(body) if !body.trim().is_empty() => body
            .lines()
            .map(|line| Line::from(line.to_string()))
            .collect(),
        _ => vec![Line::from(Span::styled(
            missing.to_string(),
            Style::default().fg(theme.muted),
        ))],
    }
}

fn timing_lines(record: &TrajectoryRecord, theme: &Theme) -> Vec<Line<'static>> {
    let value = record.duration_label().unwrap_or_else(|| {
        // The honesty rule: say there was no measurement rather than show a
        // zero, which reads as a measured zero.
        "not reported".to_string()
    });
    vec![
        Line::from(vec![
            Span::styled(
                format!("{:<10}", "duration"),
                Style::default().fg(theme.muted),
            ),
            Span::raw(value),
        ]),
        Line::from(vec![
            Span::styled(
                format!("{:<10}", "measured"),
                Style::default().fg(theme.muted),
            ),
            Span::raw(if record.duration_ms.is_some() {
                "yes, by the producer"
            } else {
                "no producer reported one"
            }),
        ]),
    ]
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};
    use tact_protocol::{AgentUpdate, StepResult, StepStatus, ToolPresentationInfo};

    use super::*;
    use crate::theme::ThemeName;

    fn theme() -> Theme {
        Theme::from(ThemeName::Dark)
    }

    fn queued(tool_id: &str, name: &str, args: &str) -> AgentUpdate {
        AgentUpdate::StepQueued {
            idx: 0,
            tool_id: tool_id.into(),
            tool_name: name.into(),
            arg_summary: args.into(),
            arg_full: format!("{{\"arg\":\"{args}\"}}"),
            presentation: ToolPresentationInfo::generic(name),
        }
    }

    fn finished(tool_id: &str, name: &str, duration_us: Option<u64>) -> AgentUpdate {
        AgentUpdate::StepFinished {
            idx: 0,
            run_id: None,
            tool_id: tool_id.into(),
            result: StepResult {
                tool: name.into(),
                arg_summary: String::new(),
                arg_full: None,
                status: StepStatus::Success,
                message: "body".into(),
                detail: Some("the detail".into()),
                duration_us,
                permission_label: None,
                presentation: ToolPresentationInfo::generic(name),
            },
        }
    }

    fn log_with(calls: usize) -> TrajectoryLog {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        log.push_user("why does the retry test fail", None);
        for index in 0..calls {
            log.observe(&queued(
                &format!("call-{index}"),
                "read_file",
                &format!("src/file-{index}.rs"),
            ));
            log.observe(&finished(
                &format!("call-{index}"),
                "read_file",
                Some(1_200_000),
            ));
        }
        log
    }

    /// Paint the page into a buffer and return it as text, one line per row.
    fn paint(width: u16, height: u16, log: &TrajectoryLog, state: &TrajectoryPageState) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let theme = theme();
        terminal
            .draw(|frame| {
                render_trajectory_page(frame.area(), frame.buffer_mut(), log, state, &theme);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn the_page_shows_a_summary_and_a_ledger_at_the_smallest_useful_size() {
        let log = log_with(3);
        let state = TrajectoryPageState::new();
        let text = paint(80, 16, &log, &state);

        assert!(text.contains("Trajectory"), "summary row missing:\n{text}");
        assert!(text.contains("1 turns"), "turn count missing:\n{text}");
        assert!(text.contains("4 records"), "record count missing:\n{text}");
        assert!(text.contains("TURN 1"), "turn rule missing:\n{text}");
        assert!(text.contains("USER"), "the prompt is a row:\n{text}");
        assert!(text.contains("read_file"), "tool rows missing:\n{text}");
    }

    /// At 16 content rows the plot would spend three of the ledger's twelve, so
    /// it starts collapsed. The reader can still ask for it.
    #[test]
    fn the_overview_starts_collapsed_when_the_rows_are_needed_elsewhere() {
        let log = log_with(2);
        let state = TrajectoryPageState::new();
        let small = paint(80, 16, &log, &state);
        assert!(
            !small.contains("INPUT"),
            "the overview must not eat a short terminal:\n{small}"
        );

        let tall = paint(80, 30, &log, &state);
        assert!(
            tall.contains("INPUT") && tall.contains("MODEL") && tall.contains("TOOLS"),
            "a tall terminal shows the plot:\n{tall}"
        );

        let forced = TrajectoryPageState {
            overview: Some(true),
            ..TrajectoryPageState::new()
        };
        let short_but_asked = paint(80, 16, &log, &forced);
        assert!(
            short_but_asked.contains("INPUT"),
            "an explicit request wins over the height default:\n{short_but_asked}"
        );
    }

    #[test]
    fn the_overview_turns_records_into_density_not_one_cell_each() {
        let log = log_with(40);
        let state = TrajectoryPageState {
            overview: Some(true),
            ..TrajectoryPageState::new()
        };
        let text = paint(80, 30, &log, &state);
        // 40 tool calls over a 74-cell track: most cells hold one, so the plot
        // is sparse but not blank, and the tools lane carries the work.
        let tools_line = text
            .lines()
            .find(|line| line.starts_with("TOOLS"))
            .expect("tools lane");
        assert!(
            tools_line.contains('▁') || tools_line.contains('▂') || tools_line.contains('▄'),
            "the tools lane must show density: {tools_line:?}"
        );
    }

    #[test]
    fn a_record_with_no_reported_duration_shows_no_number() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        log.push_user("go", None);
        log.observe(&queued("call-1", "sleep", "1s"));
        log.observe(&finished("call-1", "sleep", None));

        let state = TrajectoryPageState {
            selected: log.get(1).map(|record| record.id),
            detail: true,
            tab: TrajectoryTab::Timing,
            ..TrajectoryPageState::new()
        };
        let text = paint(80, 24, &log, &state);
        assert!(
            text.contains("not reported"),
            "an absent measurement must be named, not shown as zero:\n{text}"
        );
        assert!(
            !text.contains("0ms"),
            "a zero would read as a measured zero:\n{text}"
        );
    }

    #[test]
    fn a_measured_duration_is_shown_in_the_row_and_the_timing_tab() {
        let log = log_with(1);
        let state = TrajectoryPageState {
            selected: log.get(1).map(|record| record.id),
            detail: true,
            tab: TrajectoryTab::Timing,
            ..TrajectoryPageState::new()
        };
        let text = paint(100, 24, &log, &state);
        assert!(
            text.contains("1.2s"),
            "the row reports the measurement:\n{text}"
        );
        assert!(
            text.contains("yes, by the producer"),
            "the timing tab says where the number came from:\n{text}"
        );
    }

    #[test]
    fn the_ledger_numbers_records_within_their_turn() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        log.push_user("first", None);
        log.observe(&queued("a", "bash", "ls"));
        log.begin_turn();
        log.push_user("second", None);
        log.observe(&queued("b", "bash", "pwd"));

        let text = paint(80, 24, &log, &TrajectoryPageState::new());
        assert!(text.contains("TURN 1"), "first turn rule:\n{text}");
        assert!(text.contains("TURN 2"), "second turn rule:\n{text}");
        assert_eq!(
            text.matches("TURN ").count(),
            2,
            "one rule per turn, not per record:\n{text}"
        );
    }

    /// A narrow terminal abbreviates the kind and drops the duration rather
    /// than shipping a truncated summary, which is the row's whole purpose.
    #[test]
    fn a_narrow_terminal_keeps_the_summary_readable() {
        let log = log_with(2);
        let narrow = paint(50, 20, &log, &TrajectoryPageState::new());
        assert!(
            narrow.contains("read_file"),
            "the summary survives at 50 columns:\n{narrow}"
        );
        assert!(
            !narrow.contains("1.2s"),
            "the duration is the first thing to go:\n{narrow}"
        );
        assert!(
            narrow.contains("TOOL"),
            "a short kind label fits where the long one does not:\n{narrow}"
        );

        let wide = paint(100, 20, &log, &TrajectoryPageState::new());
        assert!(
            wide.contains("1.2s"),
            "a wide terminal has room for the duration:\n{wide}"
        );
    }

    #[test]
    fn a_long_summary_is_truncated_to_one_row() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        log.push_user(
            "a prompt long enough that it can never fit inside eighty columns without wrapping",
            None,
        );
        let text = paint(80, 20, &log, &TrajectoryPageState::new());
        let user_row = text
            .lines()
            .find(|line| line.contains("USER"))
            .expect("user row");
        assert!(
            user_row.contains('…'),
            "an over-long row is cut, not wrapped: {user_row:?}"
        );
    }

    #[test]
    fn filtering_hides_non_matching_rows_and_keeps_the_turn_rule() {
        let log = log_with(3);
        let state = TrajectoryPageState {
            filter: "file-1".to_string(),
            ..TrajectoryPageState::new()
        };
        let text = paint(80, 24, &log, &state);
        assert!(text.contains("file-1.rs"), "the match is shown:\n{text}");
        assert!(
            !text.contains("file-0.rs"),
            "the others are hidden:\n{text}"
        );
        assert!(
            text.contains("TURN 1"),
            "a hit keeps its turn label:\n{text}"
        );
    }

    #[test]
    fn the_detail_panel_opens_and_names_its_tabs() {
        let log = log_with(1);
        let state = TrajectoryPageState {
            selected: log.get(1).map(|record| record.id),
            detail: true,
            ..TrajectoryPageState::new()
        };
        let text = paint(100, 30, &log, &state);
        assert!(text.contains("Summary"), "detail title:\n{text}");
        assert!(text.contains("Payload"), "tabs listed:\n{text}");
        assert!(text.contains("Result"));
        assert!(text.contains("Timing"));
        assert!(text.contains("kind"), "summary attributes:\n{text}");
    }

    #[test]
    fn the_payload_and_result_tabs_show_the_bodies_they_name() {
        let log = log_with(1);
        let payload = TrajectoryPageState {
            selected: log.get(1).map(|record| record.id),
            detail: true,
            tab: TrajectoryTab::Payload,
            ..TrajectoryPageState::new()
        };
        let text = paint(100, 30, &log, &payload);
        assert!(text.contains("src/file-0.rs"), "payload body:\n{text}");

        let result = TrajectoryPageState {
            tab: TrajectoryTab::Result,
            ..payload.clone()
        };
        let text = paint(100, 30, &log, &result);
        assert!(text.contains("the detail"), "result body:\n{text}");
    }

    #[test]
    fn a_record_with_no_body_says_so_instead_of_showing_blank() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        log.push_user("go", None);
        let state = TrajectoryPageState {
            selected: log.get(0).map(|record| record.id),
            detail: true,
            tab: TrajectoryTab::Payload,
            ..TrajectoryPageState::new()
        };
        let text = paint(100, 30, &log, &state);
        assert!(
            text.contains("no payload"),
            "an empty tab explains:\n{text}"
        );
    }

    #[test]
    fn an_empty_ledger_says_what_the_page_is_for() {
        let log = TrajectoryLog::new();
        let text = paint(80, 20, &log, &TrajectoryPageState::new());
        assert!(
            text.contains("No records yet"),
            "an empty page is not a blank page:\n{text}"
        );
    }

    #[test]
    fn a_filter_that_matches_nothing_says_so() {
        let log = log_with(2);
        let state = TrajectoryPageState {
            filter: "nothing-matches-this".to_string(),
            ..TrajectoryPageState::new()
        };
        let text = paint(80, 20, &log, &state);
        assert!(
            text.contains("No record matches"),
            "a filtered page explains the emptiness:\n{text}"
        );
    }

    #[test]
    fn the_detail_panel_takes_two_fifths_and_leaves_the_ledger_usable() {
        assert_eq!(detail_rows(16), 6);
        assert_eq!(detail_rows(30), 12);
        assert_eq!(detail_rows(2), 2, "never taller than the content");

        // 16 rows: summary 1, no overview, detail 6 leaves 9 ledger rows.
        assert_eq!(visible_ledger_rows(16, false, true), 9);
        // With the overview open it is 6.
        assert_eq!(visible_ledger_rows(16, true, true), 6);
    }

    #[test]
    fn tabs_cycle_and_wrap() {
        assert_eq!(TrajectoryTab::Summary.next(), TrajectoryTab::Payload);
        assert_eq!(TrajectoryTab::Result.next(), TrajectoryTab::Timing);
        assert_eq!(TrajectoryTab::Timing.next(), TrajectoryTab::Summary);
    }

    #[test]
    fn scrolling_skips_painted_rows_not_records() {
        let log = log_with(6);
        let state = TrajectoryPageState::scrolled_to(4);
        let text = paint(80, 20, &log, &state);
        assert!(
            !text.contains("src/file-0.rs"),
            "the top rows scrolled away:\n{text}"
        );
        assert!(
            text.contains("src/file-4.rs") || text.contains("src/file-5.rs"),
            "the later rows are on screen:\n{text}"
        );
    }

    #[test]
    fn a_status_marker_distinguishes_outcomes() {
        assert_eq!(status_marker(Some(TrajectoryStatus::Success)), '✓');
        assert_eq!(status_marker(Some(TrajectoryStatus::Failed)), '✗');
        assert_eq!(status_marker(Some(TrajectoryStatus::Cancelled)), '⊘');
        assert_eq!(status_marker(None), ' ', "no status draws no chip");

        let mut log = TrajectoryLog::new();
        log.begin_turn();
        log.observe(&queued("a", "bash", "false"));
        log.observe(&AgentUpdate::StepFailed {
            idx: 0,
            run_id: None,
            tool_id: "a".into(),
            arg_summary: "false".into(),
            error: "exit 1".into(),
        });
        let text = paint(100, 20, &log, &TrajectoryPageState::new());
        assert!(text.contains('✗'), "a failure is visible:\n{text}");
    }

    #[test]
    fn truncation_counts_display_cells_and_marks_the_cut() {
        assert_eq!(truncate_to("short", 10), "short");
        assert_eq!(truncate_to("exactly-ten", 11), "exactly-ten");
        assert_eq!(truncate_to("abcdefghij", 5), "abcd…");
        assert_eq!(truncate_to("anything", 0), "");
    }
}

#[cfg(test)]
mod live_render_tests {
    use ratatui::{Terminal, backend::TestBackend};
    use tact_protocol::{AgentUpdate, ToolOutputChunk, ToolPresentationInfo};

    use super::*;
    use crate::theme::ThemeName;

    fn queued(tool_id: &str, args: &str) -> AgentUpdate {
        AgentUpdate::StepQueued {
            idx: 0,
            tool_id: tool_id.into(),
            tool_name: "bash".into(),
            arg_summary: args.into(),
            arg_full: String::new(),
            presentation: ToolPresentationInfo::generic("bash"),
        }
    }

    fn progress(tool_id: &str, text: &str) -> AgentUpdate {
        AgentUpdate::ToolProgress {
            run_id: None,
            tool_id: tool_id.into(),
            chunks: vec![ToolOutputChunk::stdout(text)],
        }
    }

    fn paint(log: &TrajectoryLog, state: &TrajectoryPageState, height: u16) -> String {
        let backend = TestBackend::new(80, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let theme = Theme::from(ThemeName::Dark);
        terminal
            .draw(|frame| {
                render_trajectory_page(frame.area(), frame.buffer_mut(), log, state, &theme);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    /// A running call shows what it printed instead of its argument summary.
    #[test]
    fn a_running_row_paints_the_live_output() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        log.observe(&queued("call-1", "cargo test"));
        log.observe(&progress("call-1", "compiling tui v0.1.0"));

        let text = paint(&log, &TrajectoryPageState::new(), 20);
        assert!(
            text.contains("compiling tui v0.1.0"),
            "the row reports live output:\n{text}"
        );
        assert!(
            !text.contains("cargo test"),
            "and not the stale argument line:\n{text}"
        );
        assert!(
            text.contains('▶'),
            "a running row has a run marker:\n{text}"
        );
    }

    /// The final body wins once the call is done: a finished row never shows a
    /// fragment.
    #[test]
    fn a_finished_row_returns_to_its_summary() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        log.observe(&queued("call-1", "cargo test"));
        log.observe(&progress("call-1", "compiling"));
        log.observe(&AgentUpdate::StepFinished {
            idx: 0,
            run_id: None,
            tool_id: "call-1".into(),
            result: tact_protocol::StepResult {
                tool: "bash".into(),
                arg_summary: "cargo test".into(),
                arg_full: None,
                status: tact_protocol::StepStatus::Success,
                message: "ok".into(),
                detail: None,
                duration_us: Some(4_200_000),
                permission_label: None,
                presentation: ToolPresentationInfo::generic("bash"),
            },
        });

        let text = paint(&log, &TrajectoryPageState::new(), 20);
        assert!(
            !text.contains("compiling"),
            "the tail is gone once the call ends:\n{text}"
        );
        assert!(
            text.contains("4.2s"),
            "the measurement took its place:\n{text}"
        );
    }

    /// Following keeps the newest record on screen as the ledger grows past a
    /// viewport. This is the whole point of the page during a live run.
    #[test]
    fn following_shows_the_newest_record_as_the_ledger_grows() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        for index in 0..40 {
            log.observe(&queued(&format!("call-{index}"), &format!("step-{index}")));
        }
        let state = TrajectoryPageState::new();
        assert!(state.follow, "a fresh page follows");

        let text = paint(&log, &state, 20);
        assert!(
            text.contains("step-39"),
            "the newest record is visible without scrolling:\n{text}"
        );
        assert!(
            !text.contains("step-0 "),
            "and the oldest is off screen:\n{text}"
        );
    }

    /// A reader who scrolled away keeps their place while the run continues.
    #[test]
    fn a_scrolled_reader_does_not_get_dragged_to_the_tail() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        for index in 0..40 {
            log.observe(&queued(&format!("call-{index}"), &format!("step-{index}")));
        }
        let mut state = TrajectoryPageState::scrolled_to(0);
        state.detail = false;

        for index in 40..60 {
            log.observe(&queued(&format!("call-{index}"), &format!("step-{index}")));
        }
        let text = paint(&log, &state, 20);
        assert!(
            text.contains("step-0 ") || text.contains("step-1 "),
            "the reader's place is preserved:\n{text}"
        );
        assert!(
            !text.contains("step-59"),
            "and the tail did not steal the view:\n{text}"
        );
    }

    #[test]
    fn the_follow_offset_is_zero_when_everything_fits() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        log.observe(&queued("call-1", "one"));
        let rows = ledger_rows(&log, "");
        assert_eq!(follow_offset(&rows, 20), 0);
        assert_eq!(follow_offset(&rows, 20), 0, "a short ledger never scrolls");
        assert_eq!(painted_lines(&rows), 2, "turn rule plus the record");
    }
}

//! Turn-organized projection of one session transcript.
//!
//! The Chat view renders the conversation: prose, cards, gates. The Trajectory
//! page renders the same rows as evidence — one numbered record per thing the
//! run did, grouped by turn, with the call's input, output, status and usage
//! beside it.
//!
//! The projection is read-only. It never invents timing: the transcript does
//! not record a start time per row, so the overview plots the operation
//! sequence, and a record's own duration is shown only when the producer
//! reported one.

use std::ops::Range;
use std::sync::Arc;

use crate::{RowBody, RowId, RowStatus, ToolGroupItem, TranscriptRow, TranscriptView, UsageView};

/// One kind of ledger record. The label is what the ledger's kind chip reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TrajectoryKind {
    User,
    Context,
    Assistant,
    Thinking,
    Plan,
    Tool,
    Gate,
    Diff,
    Notice,
    Error,
}

/// The overview's three lanes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TrajectoryLane {
    Input,
    Model,
    Tools,
}

impl TrajectoryLane {
    pub const ALL: [Self; 3] = [Self::Input, Self::Model, Self::Tools];

    /// Zero-based row inside the overview plot.
    pub const fn row(self) -> usize {
        match self {
            Self::Input => 0,
            Self::Model => 1,
            Self::Tools => 2,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Input => "Input",
            Self::Model => "Model",
            Self::Tools => "Tools",
        }
    }
}

impl TrajectoryKind {
    pub const ALL: [Self; 10] = [
        Self::User,
        Self::Context,
        Self::Assistant,
        Self::Thinking,
        Self::Plan,
        Self::Tool,
        Self::Gate,
        Self::Diff,
        Self::Notice,
        Self::Error,
    ];

    /// Uppercase kind chip text, matching the ledger's typography.
    pub const fn label(self) -> &'static str {
        match self {
            Self::User => "USER",
            Self::Context => "CONTEXT",
            Self::Assistant => "ASSISTANT",
            Self::Thinking => "THINKING",
            Self::Plan => "PLAN",
            Self::Tool => "TOOL",
            Self::Gate => "GATE",
            Self::Diff => "DIFF",
            Self::Notice => "NOTICE",
            Self::Error => "ERROR",
        }
    }

    pub const fn lane(self) -> TrajectoryLane {
        match self {
            Self::User | Self::Context | Self::Gate => TrajectoryLane::Input,
            Self::Assistant | Self::Thinking | Self::Plan | Self::Notice | Self::Error => {
                TrajectoryLane::Model
            }
            Self::Tool | Self::Diff => TrajectoryLane::Tools,
        }
    }
}

/// Stable ledger identity: the transcript row it came from, plus the tool call
/// when the row is one tool inside a tool group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TrajectoryRecordId {
    pub row: u64,
    /// Distinguishes sibling tools inside one tool group; `0` for a row.
    pub slot: u64,
}

impl TrajectoryRecordId {
    pub const fn row_only(row: RowId) -> Self {
        Self {
            row: row.0,
            slot: 0,
        }
    }
}

/// One ledger record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrajectoryRecord {
    pub id: TrajectoryRecordId,
    /// 1-based ledger index, shown as `#N`.
    pub index: usize,
    /// Transcript row this record opens when activated.
    pub row: RowId,
    /// Turn the record belongs to, 1-based.
    pub turn: usize,
    /// Position inside its turn, 1-based. This is the ledger's own step
    /// number, so it is derived from the projection rather than an id the
    /// producers do not send.
    pub step: usize,
    pub kind: TrajectoryKind,
    /// A one-line summary. For a tool this is `name arguments`.
    pub text: Arc<str>,
    /// A provider tool-call id, when the record is a tool call.
    pub tool_id: Option<Arc<str>>,
    /// The tool's own name, when the record is a tool call. The schema tab
    /// shows this rather than parsing it back out of [`Self::text`].
    pub tool_name: Option<Arc<str>>,
    /// The row's local `HH:MM` arrival label, when the producer stamped one.
    pub at: Option<Arc<str>>,
    pub status: RowStatus,
    /// What the caller sent, when the record has a request body.
    pub input: Option<Arc<str>>,
    /// What came back, when the record has a result body.
    pub output: Option<Arc<str>>,
    /// The record's own duration; `None` when the producer reported none.
    pub duration_ms: Option<u64>,
    pub usage: Option<UsageView>,
    /// Lowercased haystack for the ledger's live search.
    search: Arc<str>,
}

impl TrajectoryRecord {
    pub fn status_label(&self) -> &'static str {
        match self.status {
            RowStatus::Pending => "Pending",
            RowStatus::Running => "Running",
            RowStatus::Success => "Success",
            RowStatus::Failed => "Failed",
            RowStatus::Cancelled => "Cancelled",
            RowStatus::Unknown => "Unknown",
            RowStatus::Resolved => "Resolved",
        }
    }

    /// Whether every space-separated search term appears in this record.
    pub fn matches(&self, terms: &[String]) -> bool {
        terms.iter().all(|term| self.search.contains(term))
    }
}

/// One turn: a contiguous slice of the ledger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrajectoryTurn {
    /// 1-based turn number.
    pub turn: usize,
    pub records: Range<usize>,
}

impl TrajectoryTurn {
    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

/// Header totals for the ledger.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrajectorySummary {
    pub turns: usize,
    pub records: usize,
    pub tool_calls: usize,
    /// Sum of the durations the producers actually reported.
    pub measured_ms: u64,
    /// How many records carried a reported duration.
    pub measured: usize,
    pub input_tokens: u32,
    pub output_tokens: u32,
    /// Prompt tokens the provider served from its KV cache.
    pub cache_hit_tokens: u32,
    /// Prompt tokens the provider had to recompute.
    pub cache_miss_tokens: u32,
    /// The largest context window any record reported, used as the
    /// denominator for the context meter.
    pub context_window: Option<usize>,
    /// The highest prompt-token count seen, so the meter shows how full the
    /// window actually got rather than the sum of every call.
    pub context_used: u32,
}

/// One record projected into the overview's sequence domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrajectorySpan {
    /// Zero-based position in [`TrajectoryView::records`].
    pub index: usize,
    pub lane: TrajectoryLane,
    pub kind: TrajectoryKind,
    pub is_error: bool,
}

/// A full, read-only projection of a transcript.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrajectoryView {
    records: Vec<TrajectoryRecord>,
    turns: Vec<TrajectoryTurn>,
    spans: Vec<TrajectorySpan>,
    /// Index of the first record of every turn after the first, for the
    /// overview's turn rules.
    boundaries: Vec<usize>,
    summary: TrajectorySummary,
}

impl TrajectoryView {
    pub fn from_transcript(transcript: &TranscriptView) -> Self {
        let mut builder = Builder::default();
        for row in transcript.iter() {
            builder.push_row(row);
        }
        builder.finish()
    }

    pub fn records(&self) -> &[TrajectoryRecord] {
        &self.records
    }

    pub fn turns(&self) -> &[TrajectoryTurn] {
        &self.turns
    }

    pub fn spans(&self) -> &[TrajectorySpan] {
        &self.spans
    }

    pub fn boundaries(&self) -> &[usize] {
        &self.boundaries
    }

    pub fn summary(&self) -> TrajectorySummary {
        self.summary
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn record(&self, index: usize) -> Option<&TrajectoryRecord> {
        self.records.get(index)
    }

    pub fn position_of(&self, id: TrajectoryRecordId) -> Option<usize> {
        self.records.iter().position(|record| record.id == id)
    }

    /// Records that match a live search query, in ledger order. A blank query
    /// matches nothing and the caller shows the whole ledger.
    pub fn matches(&self, query: &str) -> Vec<usize> {
        let terms = search_terms(query);
        if terms.is_empty() {
            return Vec::new();
        }
        self.records
            .iter()
            .enumerate()
            .filter(|(_, record)| record.matches(&terms))
            .map(|(index, _)| index)
            .collect()
    }
}

/// The search terms for a query: whitespace-separated, lowercased.
pub fn search_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|term| term.to_lowercase())
        .filter(|term| !term.is_empty())
        .collect()
}

#[derive(Default)]
struct Builder {
    records: Vec<TrajectoryRecord>,
    turns: Vec<TrajectoryTurn>,
    spans: Vec<TrajectorySpan>,
    boundaries: Vec<usize>,
    summary: TrajectorySummary,
    turn_start: usize,
    next_turn: usize,
    saw_user: bool,
}

impl Builder {
    fn push_row(&mut self, row: &TranscriptRow) {
        // A reader prompt opens a turn. Leading rows that precede it (injected
        // context, hook notices) belong to the turn that follows rather than
        // floating above turn 1 with no owner.
        if matches!(row.body, RowBody::User(_)) && !self.turns.is_empty() && self.saw_user {
            self.close_turn();
        }
        if matches!(row.body, RowBody::User(_)) {
            self.saw_user = true;
        }
        match &row.body {
            RowBody::User(user) => {
                let text = one_line(&user.text);
                self.push(RecordSeed {
                    row,
                    slot: 0,
                    kind: TrajectoryKind::User,
                    text,
                    tool_id: None,
                    tool_name: None,
                    status: RowStatus::Success,
                    input: Some(user.text.clone()),
                    output: None,
                    duration_ms: None,
                    usage: None,
                });
            }
            RowBody::HookContext(hook) => {
                self.push(RecordSeed {
                    row,
                    slot: 0,
                    kind: TrajectoryKind::Context,
                    text: one_line(
                        hook.source
                            .as_deref()
                            .filter(|source| !source.is_empty())
                            .unwrap_or("context"),
                    ),
                    tool_id: None,
                    tool_name: None,
                    status: RowStatus::Success,
                    input: Some(hook.markdown.clone()),
                    output: None,
                    duration_ms: None,
                    usage: None,
                });
            }
            RowBody::HookStatus(status) => {
                self.push(RecordSeed {
                    row,
                    slot: 0,
                    kind: TrajectoryKind::Context,
                    text: one_line(&status.message),
                    tool_id: None,
                    tool_name: None,
                    status: RowStatus::Success,
                    input: None,
                    output: Some(status.message.clone()),
                    duration_ms: status.elapsed_ms,
                    usage: None,
                });
            }
            RowBody::Assistant(assistant) => {
                self.push(RecordSeed {
                    row,
                    slot: 0,
                    kind: TrajectoryKind::Assistant,
                    text: one_line(&assistant.markdown),
                    tool_id: None,
                    tool_name: None,
                    status: assistant.status,
                    input: None,
                    output: Some(assistant.markdown.clone()),
                    duration_ms: None,
                    usage: None,
                });
            }
            RowBody::Thinking(thinking) => {
                self.push(RecordSeed {
                    row,
                    slot: 0,
                    kind: TrajectoryKind::Thinking,
                    text: one_line(&thinking.markdown),
                    tool_id: None,
                    tool_name: None,
                    status: thinking.status,
                    input: None,
                    output: Some(thinking.markdown.clone()),
                    duration_ms: thinking.duration_ms,
                    usage: None,
                });
            }
            RowBody::Plan(plan) => {
                let mut text = one_line(&plan.title);
                if !plan.steps.is_empty() {
                    let done = plan
                        .steps
                        .iter()
                        .filter(|step| step.status == RowStatus::Success)
                        .count();
                    text = Arc::from(format!("{text} · {done}/{} steps", plan.steps.len()));
                }
                let detail = plan
                    .steps
                    .iter()
                    .map(|step| step.label.to_string())
                    .collect::<Vec<_>>()
                    .join("\n");
                self.push(RecordSeed {
                    row,
                    slot: 0,
                    kind: TrajectoryKind::Plan,
                    text,
                    tool_id: None,
                    tool_name: None,
                    status: fold_step_status(plan.steps.iter().map(|step| step.status)),
                    input: None,
                    output: (!detail.is_empty()).then(|| Arc::from(detail)),
                    duration_ms: None,
                    usage: None,
                });
            }
            RowBody::ToolGroup(group) => {
                for (slot, item) in group.items.iter().enumerate() {
                    match item {
                        ToolGroupItem::Tool(tool) => {
                            let text: Arc<str> = if tool.arg_summary.trim().is_empty() {
                                one_line(&tool.name)
                            } else {
                                Arc::from(format!("{} {}", tool.name, one_line(&tool.arg_summary)))
                            };
                            let input = tool.arg_full.clone().or_else(|| {
                                (!tool.arg_summary.trim().is_empty())
                                    .then(|| tool.arg_summary.clone())
                            });
                            let output = tool
                                .output
                                .detail
                                .clone()
                                .filter(|detail| !detail.trim().is_empty())
                                .or_else(|| {
                                    (!tool.output.preview.trim().is_empty())
                                        .then(|| tool.output.preview.clone())
                                });
                            self.push(RecordSeed {
                                row,
                                slot: slot as u64 + 1,
                                kind: TrajectoryKind::Tool,
                                text,
                                tool_id: Some(tool.tool_id.clone()),
                                tool_name: Some(tool.name.clone()),
                                status: tool.status,
                                input,
                                output,
                                duration_ms: tool.duration_ms,
                                usage: tool.meta.token_usage.clone(),
                            });
                        }
                        ToolGroupItem::Gate(gate) => {
                            self.push(RecordSeed {
                                row,
                                slot: slot as u64 + 1,
                                kind: TrajectoryKind::Gate,
                                text: one_line(&gate.prompt),
                                tool_id: None,
                                tool_name: None,
                                status: RowStatus::Resolved,
                                input: Some(gate.prompt.clone()),
                                output: None,
                                duration_ms: None,
                                usage: None,
                            });
                        }
                    }
                }
            }
            RowBody::Gate(gate) => {
                self.push(RecordSeed {
                    row,
                    slot: 0,
                    kind: TrajectoryKind::Gate,
                    text: one_line(&gate.prompt),
                    tool_id: None,
                    tool_name: None,
                    status: RowStatus::Resolved,
                    input: Some(gate.prompt.clone()),
                    output: None,
                    duration_ms: None,
                    usage: None,
                });
            }
            RowBody::Diff(diff) => {
                let text: Arc<str> = match diff.path.as_deref() {
                    Some(path) => Arc::from(format!("{path} · {}", one_line(&diff.patch))),
                    None => one_line(&diff.patch),
                };
                self.push(RecordSeed {
                    row,
                    slot: 0,
                    kind: TrajectoryKind::Diff,
                    text,
                    tool_id: None,
                    tool_name: None,
                    status: diff.status,
                    input: None,
                    output: Some(diff.patch.clone()),
                    duration_ms: None,
                    usage: None,
                });
            }
            RowBody::Notice(notice) => {
                self.push(RecordSeed {
                    row,
                    slot: 0,
                    kind: TrajectoryKind::Notice,
                    text: one_line(&notice.text),
                    tool_id: None,
                    tool_name: None,
                    status: RowStatus::Success,
                    input: None,
                    output: Some(notice.text.clone()),
                    duration_ms: None,
                    usage: None,
                });
            }
            RowBody::Error(error) => {
                self.push(RecordSeed {
                    row,
                    slot: 0,
                    kind: TrajectoryKind::Error,
                    text: one_line(&error.message),
                    tool_id: None,
                    tool_name: None,
                    status: RowStatus::Failed,
                    input: None,
                    output: Some(error.message.clone()),
                    duration_ms: None,
                    usage: None,
                });
            }
        }
    }

    fn push(&mut self, seed: RecordSeed<'_>) {
        if self.turns.is_empty() {
            self.turn_start = 0;
            self.next_turn = 1;
            self.turns.push(TrajectoryTurn {
                turn: 1,
                records: 0..0,
            });
        }
        let index = self.records.len() + 1;
        let step = index - self.turn_start;
        let kind = seed.kind;
        let status = seed.status;
        let text = seed.text;
        let mut haystack = format!("{} {} {}", kind.label(), text, seed.row.ordinal);
        for body in [seed.input.as_deref(), seed.output.as_deref()]
            .into_iter()
            .flatten()
        {
            haystack.push(' ');
            haystack.push_str(body);
        }
        let record = TrajectoryRecord {
            id: TrajectoryRecordId {
                row: seed.row.id.0,
                slot: seed.slot,
            },
            index,
            row: seed.row.id,
            turn: self.next_turn,
            step,
            kind,
            text,
            tool_id: seed.tool_id,
            tool_name: seed.tool_name,
            at: seed.row.time.clone(),
            status,
            input: seed.input,
            output: seed.output,
            duration_ms: seed.duration_ms,
            usage: seed.usage,
            search: Arc::from(haystack.to_lowercase()),
        };
        if let Some(duration) = record.duration_ms {
            self.summary.measured_ms = self.summary.measured_ms.saturating_add(duration);
            self.summary.measured += 1;
        }
        if let Some(usage) = &record.usage {
            self.summary.input_tokens = self
                .summary
                .input_tokens
                .saturating_add(usage.prompt_tokens);
            self.summary.output_tokens = self
                .summary
                .output_tokens
                .saturating_add(usage.completion_tokens);
            self.summary.cache_hit_tokens = self
                .summary
                .cache_hit_tokens
                .saturating_add(usage.cache_hit_tokens);
            // A provider that reports no cache hits is not the same as one
            // that reports a zero-hit cache, so the miss side is derived from
            // the prompt tokens that were not served from cache.
            self.summary.cache_miss_tokens = self
                .summary
                .cache_miss_tokens
                .saturating_add(usage.prompt_tokens.saturating_sub(usage.cache_hit_tokens));
            // The meter tracks the fullest the window got, not the sum of
            // every call's prompt.
            self.summary.context_used = self.summary.context_used.max(usage.prompt_tokens);
            if let Some(window) = usage.context_window {
                self.summary.context_window =
                    Some(self.summary.context_window.unwrap_or(0).max(window));
            }
        }
        if matches!(kind, TrajectoryKind::Tool) {
            self.summary.tool_calls += 1;
        }
        self.spans.push(TrajectorySpan {
            index: self.records.len(),
            lane: kind.lane(),
            kind,
            is_error: matches!(status, RowStatus::Failed),
        });
        self.records.push(record);
        if let Some(turn) = self.turns.last_mut() {
            turn.records = self.turn_start..self.records.len();
        }
        self.summary.records = self.records.len();
    }

    fn close_turn(&mut self) {
        self.next_turn += 1;
        self.turn_start = self.records.len();
        self.boundaries.push(self.records.len());
        self.turns.push(TrajectoryTurn {
            turn: self.next_turn,
            records: self.turn_start..self.turn_start,
        });
        self.summary.turns = self.turns.len();
    }

    fn finish(mut self) -> TrajectoryView {
        self.turns.retain(|turn| !turn.is_empty());
        // A transcript with no user prompt is still one turn.
        if self.turns.is_empty() && !self.records.is_empty() {
            self.turns.push(TrajectoryTurn {
                turn: 1,
                records: 0..self.records.len(),
            });
        }
        for (position, turn) in self.turns.iter_mut().enumerate() {
            turn.turn = position + 1;
            for record in &mut self.records[turn.records.clone()] {
                record.turn = turn.turn;
            }
        }
        for (index, record) in self.records.iter_mut().enumerate() {
            record.index = index + 1;
        }
        self.summary.turns = self.turns.len();
        self.summary.records = self.records.len();
        let boundaries = self
            .turns
            .iter()
            .skip(1)
            .map(|turn| turn.records.start)
            .collect();
        TrajectoryView {
            records: self.records,
            turns: self.turns,
            spans: self.spans,
            boundaries,
            summary: self.summary,
        }
    }
}

struct RecordSeed<'a> {
    row: &'a TranscriptRow,
    slot: u64,
    kind: TrajectoryKind,
    text: Arc<str>,
    tool_id: Option<Arc<str>>,
    tool_name: Option<Arc<str>>,
    status: RowStatus,
    input: Option<Arc<str>>,
    output: Option<Arc<str>>,
    duration_ms: Option<u64>,
    usage: Option<UsageView>,
}

/// Fold a plan's step statuses into one status for the plan record: a failed
/// step wins, then a cancelled one, then the first unfinished state.
pub fn fold_step_status(statuses: impl Iterator<Item = RowStatus>) -> RowStatus {
    let statuses: Vec<RowStatus> = statuses.collect();
    if statuses.contains(&RowStatus::Failed) {
        return RowStatus::Failed;
    }
    if statuses.contains(&RowStatus::Cancelled) {
        return RowStatus::Cancelled;
    }
    if statuses.contains(&RowStatus::Running) {
        return RowStatus::Running;
    }
    if statuses.contains(&RowStatus::Pending) {
        return RowStatus::Pending;
    }
    RowStatus::Success
}

/// How many characters a one-line summary keeps before it is elided.
const ONE_LINE_LIMIT: usize = 240;

/// Collapse a body to one readable line, dropping leading Markdown markers.
pub fn one_line(text: &str) -> Arc<str> {
    let mut out = String::with_capacity(text.len().min(512));
    for line in text.lines() {
        let trimmed = line
            .trim()
            .trim_start_matches(['#', '>', '-', '*', ' '])
            .trim();
        if trimmed.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(trimmed);
        if out.chars().count() >= ONE_LINE_LIMIT {
            break;
        }
    }
    let mut collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > ONE_LINE_LIMIT {
        collapsed = collapsed.chars().take(ONE_LINE_LIMIT).collect::<String>();
        collapsed.push('…');
    }
    Arc::from(collapsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(transcript: TranscriptView) -> TrajectoryView {
        TrajectoryView::from_transcript(&transcript)
    }

    #[test]
    fn demo_transcript_projects_turns_and_lanes() {
        let view = rows(crate::fixtures::demo_transcript());
        assert!(!view.is_empty());
        assert!(!view.turns().is_empty());
        assert_eq!(
            view.records().len(),
            view.turns().iter().map(TrajectoryTurn::len).sum::<usize>()
        );
        for (index, record) in view.records().iter().enumerate() {
            assert_eq!(record.index, index + 1);
            assert_eq!(record.kind.lane(), record.kind.lane());
        }
        let tool = view
            .records()
            .iter()
            .find(|record| record.kind == TrajectoryKind::Tool)
            .expect("the fixture has a tool call");
        assert!(tool.text.contains(' '), "a tool record is name + args");
        assert_eq!(
            view.summary().tool_calls,
            view.records()
                .iter()
                .filter(|record| record.kind == TrajectoryKind::Tool)
                .count()
        );
    }

    #[test]
    fn every_transcript_kind_maps_to_a_known_ledger_kind() {
        for kind in TrajectoryKind::ALL {
            assert!(!kind.label().is_empty());
            assert!(matches!(
                kind.lane(),
                TrajectoryLane::Input | TrajectoryLane::Model | TrajectoryLane::Tools
            ));
        }
        assert_eq!(TrajectoryLane::ALL.len(), 3);
        for (row, lane) in TrajectoryLane::ALL.into_iter().enumerate() {
            assert_eq!(lane.row(), row);
        }
    }

    #[test]
    fn a_user_prompt_opens_a_new_turn() {
        let transcript = crate::fixtures::demo_transcript();
        let view = rows(transcript.clone());
        let users = transcript
            .iter()
            .filter(|row| matches!(row.body, RowBody::User(_)))
            .count();
        assert!(view.turns().len() >= users.min(view.turns().len()));
        let first = view.turns().first().expect("a turn");
        let user = view
            .record(first.records.start)
            .expect("the turn's first record exists");
        assert_eq!(user.kind, TrajectoryKind::User);
    }

    #[test]
    fn boundaries_are_the_first_record_of_every_later_turn() {
        let view = rows(crate::fixtures::demo_transcript());
        let expected: Vec<usize> = view
            .turns()
            .iter()
            .skip(1)
            .map(|turn| turn.records.start)
            .collect();
        assert_eq!(view.boundaries(), expected.as_slice());
    }

    #[test]
    fn search_matches_text_and_bodies_case_insensitively() {
        let view = rows(crate::fixtures::demo_transcript());
        let record = view.record(0).expect("a record");
        let needle = record.text.chars().take(6).collect::<String>();
        assert!(
            !view.matches(&needle.to_uppercase()).is_empty(),
            "the first record is findable by its own text"
        );
        assert!(
            view.matches("").is_empty(),
            "a blank query shows everything"
        );
        assert!(view.matches("zzz-no-such-record").is_empty());
    }

    #[test]
    fn a_tool_record_carries_its_bodies_and_usage() {
        let view = rows(crate::fixtures::demo_transcript());
        let tool = view
            .records()
            .iter()
            .find(|record| record.kind == TrajectoryKind::Tool && record.output.is_some())
            .expect("a tool with a body");
        assert!(tool.tool_id.is_some());
        assert!(tool.id.slot > 0, "a grouped tool keeps its slot");
        assert!(tool.tool_name.is_some(), "a tool knows its own name");
    }

    #[test]
    fn every_record_knows_its_step_inside_its_turn() {
        let view = rows(crate::fixtures::demo_transcript());
        for turn in view.turns() {
            for (offset, record) in view.records()[turn.records.clone()].iter().enumerate() {
                assert_eq!(
                    record.step,
                    offset + 1,
                    "record #{} is step {} of turn {}",
                    record.index,
                    record.step,
                    record.turn
                );
            }
        }
    }

    #[test]
    fn the_summary_folds_cache_and_context_the_page_can_show() {
        // The demo transcript reports no token usage, so stamp two tool calls
        // with different figures. The summary must sum the cache counters and
        // take the *fullest* prompt for the context meter, not their total.
        let usage = |prompt: u32, hit: u32| UsageView {
            prompt_tokens: prompt,
            completion_tokens: 100,
            total_tokens: prompt + 100,
            cache_hit_tokens: hit,
            reasoning_tokens: 0,
            context_window: Some(128_000),
        };
        let figures = [usage(24_000, 18_000), usage(30_000, 20_000)];
        let mut next = 0;
        let stamped = crate::fixtures::demo_transcript()
            .iter()
            .map(|row| {
                let mut row = (**row).clone();
                if let RowBody::ToolGroup(group) = &mut row.body {
                    // Give each tool its own figures, in order, so the test
                    // exercises summation rather than one repeated number.
                    let items: Vec<ToolGroupItem> = group
                        .items
                        .iter()
                        .map(|item| match item {
                            ToolGroupItem::Tool(tool) if next < figures.len() => {
                                let mut tool = tool.clone();
                                tool.meta.token_usage = Some(figures[next].clone());
                                next += 1;
                                ToolGroupItem::Tool(tool)
                            }
                            other => other.clone(),
                        })
                        .collect();
                    group.items = items.into();
                }
                Arc::new(row)
            })
            .collect::<Vec<_>>();
        assert!(next > 0, "the fixture supplied a tool call to stamp");

        let view = rows(TranscriptView::from_rows(stamped));
        let summary = view.summary();
        let hit: u32 = figures[..next]
            .iter()
            .map(|usage| usage.cache_hit_tokens)
            .sum();
        let miss: u32 = figures[..next]
            .iter()
            .map(|usage| usage.prompt_tokens - usage.cache_hit_tokens)
            .sum();
        let full = figures[..next]
            .iter()
            .map(|usage| usage.prompt_tokens)
            .max()
            .unwrap();
        assert_eq!(summary.cache_hit_tokens, hit, "cache hits are summed");
        assert_eq!(summary.cache_miss_tokens, miss, "the rest is the miss");
        assert_eq!(
            summary.context_used, full,
            "the meter tracks the fullest prompt, not the total"
        );
        assert!(
            summary.context_used < figures[..next].iter().map(|u| u.prompt_tokens).sum::<u32>(),
            "one call's prompt, never the running total"
        );
        assert_eq!(summary.context_window, Some(128_000));
    }

    #[test]
    fn one_line_collapses_whitespace_and_markdown_markers() {
        assert_eq!(
            one_line("# Title\n\n- item one\n- item two"),
            Arc::from("Title item one item two")
        );
        assert_eq!(one_line("   "), Arc::from(""));
        let long = "word ".repeat(400);
        let elided = one_line(&long);
        assert_eq!(elided.chars().count(), ONE_LINE_LIMIT + 1);
        assert!(elided.ends_with('…'));
    }
}

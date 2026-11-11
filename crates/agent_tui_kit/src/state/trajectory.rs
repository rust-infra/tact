//! The ledger behind the trajectory page: one record per thing a run did.
//!
//! The log is a conversation; this is evidence. A record is appended as the
//! update that produced it arrives, because several fields the ledger needs die
//! at ingestion: `run_id` is discarded everywhere in the TUI, streamed tool
//! output is dropped on finalize, assistant text has no row object at all, and
//! a thinking block's elapsed time exists only on the live state that is
//! discarded when the block settles. Recovering any of those afterwards would
//! mean re-parsing painted text.
//!
//! Pure state: no `App`, no terminal, no clock of its own. Time is passed in by
//! the caller, which is what lets the tests assert durations without sleeping.

use std::time::Duration;

use tact_protocol::{AgentUpdate, StepStatus, ThinkingChunk, ToolVisualKind};

/// Stable ledger identity.
///
/// Monotonic per session and never reused, so a selection keyed on it survives
/// an append. An index would not: appending during a live run would move the
/// reader's place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TrajectoryRecordId(pub u64);

/// What one record is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TrajectoryKind {
    /// A prompt the reader sent.
    User,
    /// Injected context: a hook briefing, a system notice.
    Context,
    /// The model's text, one record per message.
    Assistant,
    /// One reasoning block, coalesced from its deltas.
    Thinking,
    /// One tool call.
    Tool,
    /// A patch the run produced or reviewed.
    Diff,
    /// Tact's own informational notice.
    Notice,
    /// A failure, from the loop or from a tool.
    Error,
}

impl TrajectoryKind {
    /// Uppercase chip text, at most nine cells so a ledger row's columns hold.
    pub const fn label(self) -> &'static str {
        match self {
            Self::User => "USER",
            Self::Context => "CONTEXT",
            Self::Assistant => "ASSISTANT",
            Self::Thinking => "THINKING",
            Self::Tool => "TOOL",
            Self::Diff => "DIFF",
            Self::Notice => "NOTICE",
            Self::Error => "ERROR",
        }
    }

    /// Four-cell abbreviation, for terminals too narrow for [`Self::label`].
    pub const fn short_label(self) -> &'static str {
        match self {
            Self::User => "USR",
            Self::Context => "CTX",
            Self::Assistant => "ASST",
            Self::Thinking => "THNK",
            Self::Tool => "TOOL",
            Self::Diff => "DIFF",
            Self::Notice => "NOTE",
            Self::Error => "ERR",
        }
    }

    /// The three overview lanes, so the plot's rows are derived from the kind
    /// rather than a second table that could disagree with it.
    pub const fn lane(self) -> TrajectoryLane {
        match self {
            Self::User | Self::Context => TrajectoryLane::Input,
            Self::Assistant | Self::Thinking | Self::Notice | Self::Error => TrajectoryLane::Model,
            Self::Tool | Self::Diff => TrajectoryLane::Tools,
        }
    }
}

/// The overview's three rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TrajectoryLane {
    Input,
    Model,
    Tools,
}

impl TrajectoryLane {
    pub const ALL: [Self; 3] = [Self::Input, Self::Model, Self::Tools];

    pub const fn row(self) -> usize {
        match self {
            Self::Input => 0,
            Self::Model => 1,
            Self::Tools => 2,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Input => "INPUT",
            Self::Model => "MODEL",
            Self::Tools => "TOOLS",
        }
    }
}

/// A record's completion state.
///
/// `None` on a record means it has no outcome to report — a prompt, a notice,
/// a reasoning block. That is different from "we do not know", which is why
/// this is an `Option` rather than an `Unknown` variant: an `Unknown` would let
/// a status-less row claim it had been examined.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TrajectoryStatus {
    /// Accepted, waiting for an earlier wave.
    Queued,
    Running,
    Success,
    Failed,
    Cancelled,
}

impl TrajectoryStatus {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Running => "Running",
            Self::Success => "Success",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }

    const fn from_step(status: StepStatus) -> Self {
        match status {
            StepStatus::Success => Self::Success,
            StepStatus::Failed => Self::Failed,
            StepStatus::Cancelled => Self::Cancelled,
        }
    }
}

/// One ledger record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrajectoryRecord {
    pub id: TrajectoryRecordId,
    /// 1-based turn this record belongs to.
    pub turn: u32,
    /// 1-based position inside its turn.
    pub step: u32,
    pub kind: TrajectoryKind,
    /// `None` when the record has no completion state to report.
    pub status: Option<TrajectoryStatus>,
    /// One-line summary: what this record is about.
    pub text: String,
    /// The provider's tool-call id, when this record is a tool call.
    pub tool_id: Option<String>,
    /// The tool's own name, kept so the detail panel does not parse it back out
    /// of `text`.
    pub tool_name: Option<String>,
    /// Local `HH:MM` arrival label, when the caller stamped one.
    pub at: Option<String>,
    /// The record's own duration, only when a producer reported one.
    pub duration_ms: Option<u64>,
    /// What the caller sent, when this record has a request body.
    pub input: Option<String>,
    /// What came back, when this record has a result body.
    pub output: Option<String>,
    /// Lowercased haystack for the ledger's filter.
    search: String,
}

impl TrajectoryRecord {
    fn new(
        id: TrajectoryRecordId,
        turn: u32,
        step: u32,
        kind: TrajectoryKind,
        text: impl Into<String>,
    ) -> Self {
        let text = text.into();
        let search = text.to_lowercase();
        Self {
            id,
            turn,
            step,
            kind,
            status: None,
            text,
            tool_id: None,
            tool_name: None,
            at: None,
            duration_ms: None,
            input: None,
            output: None,
            search,
        }
    }

    /// Whether `terms` (already lowercased) all appear in this record.
    fn matches(&self, terms: &[String]) -> bool {
        terms.iter().all(|term| self.search.contains(term.as_str()))
    }

    /// The duration as ledger text, or `None` when no producer reported one.
    ///
    /// This is the honesty rule: a stopwatch the UI invented would be
    /// indistinguishable from a measured one.
    pub fn duration_label(&self) -> Option<String> {
        let ms = self.duration_ms?;
        Some(format_duration_ms(ms))
    }
}

/// `1.2s`, `45ms`, `2m04s`. One decimal below ten seconds, none above.
pub fn format_duration_ms(ms: u64) -> String {
    if ms < 1_000 {
        return format!("{ms}ms");
    }
    let seconds = ms as f64 / 1_000.;
    if seconds < 10. {
        return format!("{seconds:.1}s");
    }
    if seconds < 60. {
        return format!("{}s", seconds.round() as u64);
    }
    let minutes = (seconds / 60.).floor() as u64;
    let rest = (seconds - minutes as f64 * 60.).round() as u64;
    format!("{minutes}m{rest:02}s")
}

/// The session's ledger: append-only, ordered, and cheap to filter.
#[derive(Debug, Default)]
pub struct TrajectoryLog {
    records: Vec<TrajectoryRecord>,
    next_id: u64,
    turn: u32,
    step: u32,
    /// Index of the reasoning record still collecting deltas, if any.
    open_thinking: Option<usize>,
    /// Index of the assistant record still collecting deltas, if any.
    open_assistant: Option<usize>,
    /// Index of each in-flight tool record, keyed by its provider call id.
    open_tools: std::collections::HashMap<String, usize>,
}

impl TrajectoryLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn records(&self) -> &[TrajectoryRecord] {
        &self.records
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&TrajectoryRecord> {
        self.records.get(index)
    }

    pub fn position_of(&self, id: TrajectoryRecordId) -> Option<usize> {
        self.records.iter().position(|record| record.id == id)
    }

    /// Indexes matching `query`, in ledger order. A blank query matches
    /// nothing, and the caller shows the whole ledger.
    pub fn matches(&self, query: &str) -> Vec<usize> {
        let terms: Vec<String> = query
            .split_whitespace()
            .map(str::to_lowercase)
            .filter(|term| !term.is_empty())
            .collect();
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

    /// How many distinct turns the ledger holds.
    pub fn turns(&self) -> u32 {
        self.records.last().map_or(0, |record| record.turn)
    }

    /// Totals the page's summary row reports.
    pub fn summary(&self) -> TrajectorySummary {
        let mut summary = TrajectorySummary {
            records: self.records.len(),
            ..TrajectorySummary::default()
        };
        for record in &self.records {
            if record.kind == TrajectoryKind::Tool {
                summary.tool_calls += 1;
            }
            if let Some(ms) = record.duration_ms {
                summary.measured_ms += ms;
                summary.measured += 1;
            }
        }
        summary.turns = self.turns();
        summary
    }

    /// Start a new turn: the caller invokes this when a prompt is dispatched,
    /// which is the same moment the status bar increments its own counter.
    pub fn begin_turn(&mut self) {
        self.close_open_thinking(None);
        self.close_open_assistant();
        self.turn += 1;
        self.step = 0;
    }

    /// Record the prompt that opened the current turn.
    pub fn push_user(&mut self, text: impl Into<String>, at: Option<String>) -> TrajectoryRecordId {
        if self.turn == 0 {
            self.turn = 1;
        }
        let id = self.append(TrajectoryKind::User, text).id;
        self.records.last_mut().expect("just pushed").at = at;
        id
    }

    /// Close a reasoning block, stamping the elapsed time the caller measured.
    ///
    /// The update stream does not carry a thinking duration: `Finished` is a
    /// bare marker, and the live state that held `started_at` is discarded when
    /// the block settles. The caller therefore passes what it already computed.
    pub fn finish_thinking(&mut self, elapsed: Duration) {
        self.close_open_thinking(Some(elapsed));
    }

    /// Close the assistant record when its message finishes.
    pub fn finish_assistant(&mut self) {
        self.close_open_assistant();
    }

    /// Append whatever one update contributes. Updates that carry no ledger
    /// meaning (token counts, model parameters, run boundaries) contribute
    /// nothing rather than an empty row.
    pub fn observe(&mut self, update: &AgentUpdate) {
        match update {
            AgentUpdate::StepQueued {
                tool_id,
                tool_name,
                arg_summary,
                arg_full,
                ..
            } => {
                self.close_open_thinking(None);
                self.close_open_assistant();
                let text = summary_line(tool_name, arg_summary);
                let record = self.append(TrajectoryKind::Tool, text);
                record.status = Some(TrajectoryStatus::Queued);
                record.tool_id = Some(tool_id.clone());
                record.tool_name = Some(tool_name.clone());
                record.input = non_empty(arg_full);
                let index = self.records.len() - 1;
                self.open_tools.insert(tool_id.clone(), index);
            }
            AgentUpdate::StepStarted { tool_id, .. } => {
                self.close_open_thinking(None);
                self.close_open_assistant();
                if let Some(index) = self.open_tools.get(tool_id).copied() {
                    self.records[index].status = Some(TrajectoryStatus::Running);
                }
            }
            AgentUpdate::StepFinished {
                tool_id, result, ..
            } => {
                if let Some(index) = self.open_tools.remove(tool_id) {
                    let record = &mut self.records[index];
                    record.status = Some(TrajectoryStatus::from_step(result.status));
                    record.duration_ms = result.duration_us.map(|us| us / 1_000);
                    if let Some(detail) = &result.detail {
                        record.output = Some(detail.clone());
                    } else if !result.message.is_empty() {
                        record.output = Some(result.message.clone());
                    }
                    if record.tool_name.is_none() {
                        record.tool_name = Some(result.tool.clone());
                    }
                    record.search = search_haystack(record);
                } else {
                    // A finish without a matching start: still evidence.
                    let text = summary_line(&result.tool, &result.arg_summary);
                    let record = self.append(TrajectoryKind::Tool, text);
                    record.status = Some(TrajectoryStatus::from_step(result.status));
                    record.duration_ms = result.duration_us.map(|us| us / 1_000);
                    record.tool_id = Some(tool_id.clone());
                    record.tool_name = Some(result.tool.clone());
                    record.input = result.arg_full.clone();
                    record.output = result.detail.clone().or_else(|| non_empty(&result.message));
                }
            }
            AgentUpdate::StepFailed {
                tool_id,
                arg_summary,
                error,
                ..
            } => {
                if let Some(index) = self.open_tools.remove(tool_id) {
                    let record = &mut self.records[index];
                    record.status = Some(TrajectoryStatus::Failed);
                    record.output = Some(error.clone());
                    record.search = search_haystack(record);
                } else {
                    let record = self.append(TrajectoryKind::Error, arg_summary.clone());
                    record.status = Some(TrajectoryStatus::Failed);
                    record.tool_id = Some(tool_id.clone());
                    record.output = Some(error.clone());
                }
            }
            AgentUpdate::ThinkingChunk(chunk) => match chunk {
                ThinkingChunk::Started => {
                    self.close_open_thinking(None);
                    self.close_open_assistant();
                    let record = self.append(TrajectoryKind::Thinking, "");
                    record.status = Some(TrajectoryStatus::Running);
                    self.open_thinking = Some(self.records.len() - 1);
                }
                ThinkingChunk::Delta(text) => {
                    self.close_open_assistant();
                    let index = match self.open_thinking {
                        Some(index) => index,
                        None => {
                            let record = self.append(TrajectoryKind::Thinking, "");
                            record.status = Some(TrajectoryStatus::Running);
                            self.open_thinking = Some(self.records.len() - 1);
                            self.records.len() - 1
                        }
                    };
                    let record = &mut self.records[index];
                    if !record.text.is_empty() {
                        record.text.push(' ');
                    }
                    record.text.push_str(text.trim());
                    record.search = search_haystack(record);
                }
                ThinkingChunk::Finished => self.close_open_thinking(None),
            },
            AgentUpdate::StreamChunk(text) => {
                self.close_open_thinking(None);
                let index = match self.open_assistant {
                    Some(index) => index,
                    None => {
                        let record = self.append(TrajectoryKind::Assistant, "");
                        record.status = Some(TrajectoryStatus::Running);
                        self.open_assistant = Some(self.records.len() - 1);
                        self.records.len() - 1
                    }
                };
                let record = &mut self.records[index];
                record.text.push_str(text);
                record.search = search_haystack(record);
            }
            AgentUpdate::MessageFinished { .. } => self.close_open_assistant(),
            AgentUpdate::TaskComplete(_) => {
                self.close_open_thinking(None);
                self.close_open_assistant();
            }
            AgentUpdate::TaskCancelled => {
                self.close_open_thinking(None);
                self.close_open_assistant();
                for index in self.open_tools.values().copied().collect::<Vec<_>>() {
                    let record = &mut self.records[index];
                    if record.status == Some(TrajectoryStatus::Running)
                        || record.status == Some(TrajectoryStatus::Queued)
                    {
                        record.status = Some(TrajectoryStatus::Cancelled);
                    }
                }
                self.open_tools.clear();
            }
            AgentUpdate::Error(kind) => {
                let text = format!("{kind:?}");
                self.append(TrajectoryKind::Error, text);
            }
            AgentUpdate::Info(text) => {
                let record = self.append(TrajectoryKind::Notice, text.clone());
                record.search = search_haystack(record);
            }
            AgentUpdate::MdInfo(text) => {
                self.append(TrajectoryKind::Notice, first_line(text));
            }
            AgentUpdate::HookContext { source, text } => {
                let label = match source {
                    Some(source) => format!("{source}: {}", first_line(text)),
                    None => first_line(text),
                };
                self.append(TrajectoryKind::Context, label);
            }
            AgentUpdate::ToolTranscript { .. }
            | AgentUpdate::ToolTranscriptStream { .. }
            | AgentUpdate::ToolProgress { .. }
            | AgentUpdate::TokenUsage(_)
            | AgentUpdate::ModelInfo(_)
            | AgentUpdate::TurnStats { .. }
            | AgentUpdate::RunStarted { .. }
            | AgentUpdate::RunFinished { .. }
            | AgentUpdate::RunFailed { .. }
            | AgentUpdate::MessageStarted { .. }
            | AgentUpdate::TaskIdle
            | AgentUpdate::StepAdded(_)
            | AgentUpdate::TasksChanged { .. }
            | AgentUpdate::GoalsChanged { .. }
            | AgentUpdate::SubagentsChanged { .. }
            | AgentUpdate::BackgroundTasksChanged { .. }
            | AgentUpdate::ToolMeta { .. }
            | AgentUpdate::HookStatus { .. }
            | AgentUpdate::UiResponseRejected { .. }
            | AgentUpdate::RequestSelect { .. }
            | AgentUpdate::RequestMultiSelect { .. }
            | AgentUpdate::PopupMarkdown { .. }
            | AgentUpdate::SurfaceSnapshot { .. }
            | AgentUpdate::BackgroundTaskFinished { .. }
            | AgentUpdate::SubagentFinished { .. } => {}
        }
    }

    /// Append a record for a diff the run produced.
    pub fn push_diff(&mut self, path: &str, patch_lines: usize) -> TrajectoryRecordId {
        let text = if patch_lines == 0 {
            path.to_string()
        } else {
            format!("{path} · {patch_lines} lines")
        };
        self.append(TrajectoryKind::Diff, text).id
    }

    fn append(&mut self, kind: TrajectoryKind, text: impl Into<String>) -> &mut TrajectoryRecord {
        self.next_id += 1;
        self.step = self.step.saturating_add(1);
        let turn = self.turn.max(1);
        let record = TrajectoryRecord::new(
            TrajectoryRecordId(self.next_id),
            turn,
            self.step,
            kind,
            text,
        );
        self.records.push(record);
        self.records.last_mut().expect("just pushed")
    }

    fn close_open_thinking(&mut self, elapsed: Option<Duration>) {
        if let Some(index) = self.open_thinking.take() {
            let record = &mut self.records[index];
            record.status = None;
            if let Some(elapsed) = elapsed {
                record.duration_ms = Some(elapsed.as_millis() as u64);
            }
            record.text = collapse_whitespace(&record.text);
            record.search = search_haystack(record);
        }
    }

    fn close_open_assistant(&mut self) {
        if let Some(index) = self.open_assistant.take() {
            let record = &mut self.records[index];
            record.status = None;
            record.text = collapse_whitespace(&record.text);
            record.search = search_haystack(record);
        }
    }
}

/// Session totals, as the page's summary row reports them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrajectorySummary {
    pub turns: u32,
    pub records: usize,
    pub tool_calls: usize,
    /// Sum of the durations producers actually reported.
    pub measured_ms: u64,
    /// How many records carried one, so the row can omit a total with no basis.
    pub measured: usize,
}

fn summary_line(tool_name: &str, arg_summary: &str) -> String {
    let name = tool_name.trim();
    let args = arg_summary.trim();
    match (name.is_empty(), args.is_empty()) {
        (true, true) => "(tool)".to_string(),
        (false, true) => name.to_string(),
        (true, false) => args.to_string(),
        (false, false) => format!("{name} {args}"),
    }
}

fn non_empty(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_string()
}

/// One line, runs of whitespace collapsed, so a streamed body cannot make a
/// ledger row taller than one cell.
fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn search_haystack(record: &TrajectoryRecord) -> String {
    let mut haystack = record.text.to_lowercase();
    for extra in [&record.tool_name, &record.input, &record.output]
        .into_iter()
        .flatten()
    {
        haystack.push(' ');
        haystack.push_str(&extra.to_lowercase());
    }
    haystack
}

/// The overview's cell for one bucket.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BucketDensity {
    pub input: usize,
    pub model: usize,
    pub tools: usize,
}

impl BucketDensity {
    pub fn total(self) -> usize {
        self.input + self.model + self.tools
    }

    /// The lane with the most records in this bucket, so a cell can be tinted
    /// by what dominated it.
    pub fn dominant(self) -> Option<TrajectoryLane> {
        let lanes = [
            (TrajectoryLane::Input, self.input),
            (TrajectoryLane::Model, self.model),
            (TrajectoryLane::Tools, self.tools),
        ];
        lanes
            .into_iter()
            .filter(|(_, count)| *count > 0)
            .max_by_key(|(_, count)| *count)
            .map(|(lane, _)| lane)
    }
}

/// Distribute `records` into `cells` buckets, one density per lane.
///
/// One cell per record is the desktop rule and it does not survive a terminal:
/// at 120 records and 80 columns each record would get zero cells. Bucketing
/// keeps the plot readable at any length, at the cost of resolution, which is
/// the honest trade for a 72-cell track.
///
/// Every record lands in exactly one bucket, and the last bucket takes the
/// remainder, so no record is silently dropped at the right edge.
pub fn bucket_densities(records: &[TrajectoryRecord], cells: usize) -> Vec<BucketDensity> {
    let mut buckets = vec![BucketDensity::default(); cells];
    if cells == 0 || records.is_empty() {
        return buckets;
    }
    let count = records.len();
    for (index, record) in records.iter().enumerate() {
        let cell = (index * cells) / count;
        let cell = cell.min(cells - 1);
        let bucket = &mut buckets[cell];
        match record.kind.lane() {
            TrajectoryLane::Input => bucket.input += 1,
            TrajectoryLane::Model => bucket.model += 1,
            TrajectoryLane::Tools => bucket.tools += 1,
        }
    }
    buckets
}

/// Turn boundaries as bucket indexes, for the overview's rules.
pub fn turn_marks(records: &[TrajectoryRecord], cells: usize) -> Vec<usize> {
    if cells == 0 || records.is_empty() {
        return Vec::new();
    }
    let count = records.len();
    let mut marks = Vec::new();
    let mut last_turn = None;
    for (index, record) in records.iter().enumerate() {
        if last_turn.is_some_and(|turn| turn != record.turn) {
            marks.push(((index * cells) / count).min(cells - 1));
        }
        last_turn = Some(record.turn);
    }
    marks
}

/// The glyph ramp: how many records a bucket holds, as one cell.
pub fn density_glyph(total: usize) -> char {
    match total {
        0 => '·',
        1 => '▁',
        2 => '▂',
        3..=4 => '▄',
        _ => '█',
    }
}

/// Whether the overview is worth its three rows at this content height.
///
/// The page's own rule, kept here so a test can assert it without a terminal.
pub const OVERVIEW_ROWS: u16 = 3;
/// Below this many content rows the overview starts collapsed.
pub const OVERVIEW_MIN_CONTENT_ROWS: u16 = 20;

pub fn overview_default_collapsed(content_rows: u16) -> bool {
    content_rows < OVERVIEW_MIN_CONTENT_ROWS
}

/// Whether a record kind is worth showing a status chip for.
pub fn kind_has_status(kind: TrajectoryKind) -> bool {
    matches!(kind, TrajectoryKind::Tool | TrajectoryKind::Error)
}

/// Whether this kind's rows carry a tool's visual kind, used to pick an icon.
pub fn is_subagent_tool(visual: ToolVisualKind) -> bool {
    matches!(visual, ToolVisualKind::Subagent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_protocol::{
        AgentErrorKind, StepResult, StepStatus, ThinkingChunk, TokenUsageInfo, ToolPresentationInfo,
    };

    fn presentation(name: &str) -> ToolPresentationInfo {
        ToolPresentationInfo::generic(name)
    }

    fn queued(tool_id: &str, name: &str, args: &str, full: &str) -> AgentUpdate {
        AgentUpdate::StepQueued {
            idx: 0,
            tool_id: tool_id.into(),
            tool_name: name.into(),
            arg_summary: args.into(),
            arg_full: full.into(),
            presentation: presentation(name),
        }
    }

    fn started(tool_id: &str, name: &str) -> AgentUpdate {
        AgentUpdate::StepStarted {
            idx: 0,
            tool_id: tool_id.into(),
            tool_name: name.into(),
            arg_summary: String::new(),
            arg_full: String::new(),
            presentation: presentation(name),
        }
    }

    fn finished(
        tool_id: &str,
        name: &str,
        status: StepStatus,
        duration_us: Option<u64>,
    ) -> AgentUpdate {
        AgentUpdate::StepFinished {
            idx: 0,
            run_id: None,
            tool_id: tool_id.into(),
            result: StepResult {
                tool: name.into(),
                arg_summary: String::new(),
                arg_full: None,
                status,
                message: "output body".into(),
                detail: Some("full detail".into()),
                duration_us,
                permission_label: None,
                presentation: presentation(name),
            },
        }
    }

    #[test]
    fn a_tool_call_becomes_one_record_that_keeps_name_args_and_output() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        log.observe(&queued(
            "call-1",
            "read_file",
            "src/lib.rs",
            "{\"path\":\"src/lib.rs\"}",
        ));
        log.observe(&started("call-1", "read_file"));
        log.observe(&finished(
            "call-1",
            "read_file",
            StepStatus::Success,
            Some(2_500_000),
        ));

        assert_eq!(log.records().len(), 1, "one call is one record, not three");
        let record = log.get(0).expect("record");
        assert_eq!(record.kind, TrajectoryKind::Tool);
        assert_eq!(record.tool_id.as_deref(), Some("call-1"));
        assert_eq!(record.tool_name.as_deref(), Some("read_file"));
        assert_eq!(record.text, "read_file src/lib.rs");
        assert_eq!(record.input.as_deref(), Some("{\"path\":\"src/lib.rs\"}"));
        assert_eq!(record.output.as_deref(), Some("full detail"));
        assert_eq!(record.duration_ms, Some(2_500));
        assert_eq!(record.status, Some(TrajectoryStatus::Success));
    }

    #[test]
    fn a_tool_with_no_reported_duration_shows_none() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        log.observe(&queued("call-1", "sleep", "1s", ""));
        log.observe(&finished("call-1", "sleep", StepStatus::Success, None));

        let record = log.get(0).expect("record");
        assert_eq!(record.duration_ms, None, "no producer, no number");
        assert_eq!(
            record.duration_label(),
            None,
            "the page must render an absence, not a zero"
        );
    }

    #[test]
    fn a_queued_call_is_running_before_it_is_done() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        log.observe(&queued("call-1", "grep", "needle", ""));
        assert_eq!(
            log.get(0).expect("record").status,
            Some(TrajectoryStatus::Queued)
        );
        log.observe(&started("call-1", "grep"));
        assert_eq!(
            log.get(0).expect("record").status,
            Some(TrajectoryStatus::Running)
        );
    }

    #[test]
    fn a_finished_tool_without_a_start_is_still_evidence() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        log.observe(&finished(
            "orphan",
            "bash",
            StepStatus::Success,
            Some(1_000),
        ));

        assert_eq!(log.len(), 1);
        let record = log.get(0).expect("record");
        assert_eq!(record.tool_name.as_deref(), Some("bash"));
        assert_eq!(record.duration_ms, Some(1));
    }

    #[test]
    fn a_failed_step_records_the_reason() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        log.observe(&queued("call-1", "bash", "false", ""));
        log.observe(&AgentUpdate::StepFailed {
            idx: 0,
            run_id: None,
            tool_id: "call-1".into(),
            arg_summary: "bash false".into(),
            error: "exit code 1".into(),
        });

        let record = log.get(0).expect("record");
        assert_eq!(record.status, Some(TrajectoryStatus::Failed));
        assert_eq!(record.output.as_deref(), Some("exit code 1"));
    }

    #[test]
    fn reasoning_is_coalesced_into_one_record_and_carries_the_measured_elapsed() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        log.observe(&AgentUpdate::ThinkingChunk(ThinkingChunk::Started));
        log.observe(&AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
            "the guard is ".into(),
        )));
        log.observe(&AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
            "inverted".into(),
        )));
        log.finish_thinking(Duration::from_millis(1_234));

        assert_eq!(log.len(), 1, "deltas belong to one record");
        let record = log.get(0).expect("record");
        assert_eq!(record.kind, TrajectoryKind::Thinking);
        assert_eq!(record.text, "the guard is inverted");
        assert_eq!(record.duration_ms, Some(1_234));
        assert_eq!(record.status, None, "a settled thought has no outcome chip");
    }

    #[test]
    fn assistant_text_is_one_record_per_message() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        log.observe(&AgentUpdate::StreamChunk("the mock ".into()));
        log.observe(&AgentUpdate::StreamChunk("returns early".into()));
        log.observe(&AgentUpdate::MessageFinished {
            message_id: "m1".into(),
            phase: None,
        });
        log.observe(&AgentUpdate::StreamChunk("second answer".into()));
        log.observe(&AgentUpdate::MessageFinished {
            message_id: "m2".into(),
            phase: None,
        });

        assert_eq!(log.len(), 2);
        assert_eq!(log.get(0).expect("first").text, "the mock returns early");
        assert_eq!(log.get(1).expect("second").text, "second answer");
        assert!(
            log.get(0).expect("first").status.is_none(),
            "an answer is not a status"
        );
    }

    #[test]
    fn a_streamed_body_never_makes_a_row_multi_line() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        log.observe(&AgentUpdate::StreamChunk(
            "first line\n\n  second   line".into(),
        ));
        // The body is raw while it streams; it is flattened when the message
        // closes, which is the only moment a row can be measured.
        log.observe(&AgentUpdate::MessageFinished {
            message_id: "m1".into(),
            phase: None,
        });
        let record = log.get(0).expect("record");
        assert!(
            !record.text.contains('\n'),
            "a ledger row is one cell tall: {:?}",
            record.text
        );
        assert_eq!(record.text, "first line second line");
    }

    #[test]
    fn a_cancel_marks_in_flight_tools_cancelled_without_losing_them() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        log.observe(&queued("call-1", "bash", "sleep 100", ""));
        log.observe(&started("call-1", "bash"));
        log.observe(&AgentUpdate::TaskCancelled);

        let record = log.get(0).expect("record");
        assert_eq!(record.status, Some(TrajectoryStatus::Cancelled));
        assert_eq!(
            record.tool_name.as_deref(),
            Some("bash"),
            "the cancelled call is still evidence of what was attempted"
        );
    }

    #[test]
    fn turns_and_steps_number_from_the_prompt() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        log.push_user("first prompt", None);
        log.observe(&queued("call-1", "read_file", "a", ""));
        log.begin_turn();
        log.push_user("second prompt", None);
        log.observe(&queued("call-2", "read_file", "b", ""));

        let turns: Vec<u32> = log.records().iter().map(|record| record.turn).collect();
        let steps: Vec<u32> = log.records().iter().map(|record| record.step).collect();
        assert_eq!(turns, vec![1, 1, 2, 2]);
        assert_eq!(steps, vec![1, 2, 1, 2], "steps restart inside each turn");
        assert_eq!(log.turns(), 2);
    }

    #[test]
    fn a_record_pushed_before_any_turn_still_belongs_to_turn_one() {
        let mut log = TrajectoryLog::new();
        log.observe(&AgentUpdate::Info("loaded 3 skills".into()));
        assert_eq!(log.get(0).expect("record").turn, 1);
    }

    #[test]
    fn the_summary_counts_only_what_was_measured() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        log.push_user("go", None);
        log.observe(&queued("call-1", "bash", "a", ""));
        log.observe(&finished(
            "call-1",
            "bash",
            StepStatus::Success,
            Some(1_000_000),
        ));
        log.observe(&queued("call-2", "bash", "b", ""));
        log.observe(&finished("call-2", "bash", StepStatus::Success, None));

        let summary = log.summary();
        assert_eq!(summary.turns, 1);
        assert_eq!(summary.records, 3);
        assert_eq!(summary.tool_calls, 2);
        assert_eq!(summary.measured, 1, "one tool reported a duration");
        assert_eq!(summary.measured_ms, 1_000, "and only that one was summed");
    }

    #[test]
    fn notices_and_context_are_records_but_not_tools() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        log.observe(&AgentUpdate::Info("[compacting]".into()));
        log.observe(&AgentUpdate::HookContext {
            source: Some("plugin codex".into()),
            text: "**Project:** tact\n\nmore".into(),
        });
        log.observe(&AgentUpdate::Error(AgentErrorKind::Other("boom".into())));

        assert_eq!(log.get(0).expect("notice").kind, TrajectoryKind::Notice);
        let context = log.get(1).expect("context");
        assert_eq!(context.kind, TrajectoryKind::Context);
        assert_eq!(context.text, "plugin codex: **Project:** tact");
        assert_eq!(log.get(2).expect("error").kind, TrajectoryKind::Error);
        assert_eq!(log.summary().tool_calls, 0);
    }

    #[test]
    fn an_id_is_never_reused_so_a_selection_survives_an_append() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        for index in 0..5 {
            log.observe(&queued(
                &format!("call-{index}"),
                "read_file",
                &format!("file-{index}"),
                "",
            ));
        }
        let selected = log.get(2).expect("third record").id;

        log.observe(&queued("call-99", "read_file", "later", ""));

        let position = log.position_of(selected).expect("selection still resolves");
        assert_eq!(position, 2, "an append must not move an existing record");
        assert_eq!(log.len(), 6);
    }

    #[test]
    fn filtering_matches_text_tool_name_input_and_output() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        log.observe(&queued(
            "call-1",
            "read_file",
            "src/lib.rs",
            "{\"secret\":\"CANARY\"}",
        ));
        log.observe(&finished("call-1", "read_file", StepStatus::Success, None));
        log.observe(&queued("call-2", "bash", "ls", ""));
        log.observe(&finished("call-2", "bash", StepStatus::Success, None));

        assert_eq!(log.matches("read_file"), vec![0], "matches the tool name");
        assert_eq!(log.matches("lib.rs"), vec![0], "matches the arguments");
        assert_eq!(log.matches("canary"), vec![0], "matches the request body");
        assert_eq!(log.matches("detail"), vec![0, 1], "matches the result body");
        assert!(
            log.matches("   ").is_empty(),
            "a blank query matches nothing"
        );
        assert!(log.matches("nothing-here").is_empty());
    }

    #[test]
    fn updates_with_no_ledger_meaning_add_no_rows() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        log.observe(&AgentUpdate::TurnStats {
            run_id: None,
            turns_taken: 3,
            max_turns: None,
        });
        log.observe(&AgentUpdate::TokenUsage(TokenUsageInfo::default()));
        log.observe(&AgentUpdate::RunStarted {
            run_id: "r1".into(),
        });
        log.observe(&AgentUpdate::RunFinished {
            run_id: "r1".into(),
        });

        assert!(
            log.is_empty(),
            "accounting is not a record: {:?}",
            log.records()
        );
    }

    #[test]
    fn a_duration_reads_in_the_units_it_deserves() {
        assert_eq!(format_duration_ms(45), "45ms");
        assert_eq!(format_duration_ms(1_234), "1.2s");
        assert_eq!(format_duration_ms(9_950), "9.9s");
        assert_eq!(format_duration_ms(10_000), "10s");
        assert_eq!(format_duration_ms(45_000), "45s");
        assert_eq!(format_duration_ms(124_000), "2m04s");
    }
}

#[cfg(test)]
mod overview_tests {
    use super::*;
    use tact_protocol::{AgentUpdate, ToolPresentationInfo};

    fn fill(log: &mut TrajectoryLog, count: usize) {
        log.begin_turn();
        for index in 0..count {
            log.observe(&AgentUpdate::StepQueued {
                idx: index,
                tool_id: format!("call-{index}"),
                tool_name: "read_file".into(),
                arg_summary: format!("file-{index}"),
                arg_full: String::new(),
                presentation: ToolPresentationInfo::generic("read_file"),
            });
        }
    }

    /// The plot's contract: every record lands in a bucket, none is dropped,
    /// and the densities add up to the record count.
    #[test]
    fn bucketing_places_every_record_exactly_once() {
        let mut log = TrajectoryLog::new();
        fill(&mut log, 300);

        for cells in [1usize, 7, 72, 112, 299, 300, 400] {
            let buckets = bucket_densities(log.records(), cells);
            assert_eq!(buckets.len(), cells);
            let total: usize = buckets.iter().map(|bucket| bucket.total()).sum();
            assert_eq!(
                total,
                log.len(),
                "cells={cells}: the plot must account for every record, got {total} of {}",
                log.len()
            );
        }
    }

    /// A terminal track is narrower than the record count, so most cells hold
    /// several records. None may be empty in the middle while a later one has
    /// work: that would draw a gap where none exists.
    #[test]
    fn no_bucket_is_empty_before_a_full_one() {
        let mut log = TrajectoryLog::new();
        fill(&mut log, 300);
        let buckets = bucket_densities(log.records(), 72);

        let mut seen_empty = false;
        for (index, bucket) in buckets.iter().enumerate() {
            if bucket.total() == 0 {
                seen_empty = true;
            } else {
                assert!(
                    !seen_empty,
                    "cell {index} has work but an earlier cell was empty: {buckets:?}"
                );
            }
        }
    }

    #[test]
    fn the_last_bucket_takes_the_remainder() {
        let mut log = TrajectoryLog::new();
        fill(&mut log, 10);
        let buckets = bucket_densities(log.records(), 3);
        assert_eq!(
            buckets.iter().map(|bucket| bucket.total()).sum::<usize>(),
            10
        );
        assert!(
            buckets[2].total() > 0,
            "the right edge must not be silently dropped: {buckets:?}"
        );
    }

    #[test]
    fn lanes_are_derived_from_the_kind() {
        let mut log = TrajectoryLog::new();
        log.begin_turn();
        log.push_user("prompt", None);
        log.observe(&AgentUpdate::StreamChunk("answer".into()));
        log.observe(&AgentUpdate::MessageFinished {
            message_id: "m".into(),
            phase: None,
        });
        log.observe(&AgentUpdate::StepQueued {
            idx: 0,
            tool_id: "t".into(),
            tool_name: "bash".into(),
            arg_summary: "ls".into(),
            arg_full: String::new(),
            presentation: ToolPresentationInfo::generic("bash"),
        });

        let buckets = bucket_densities(log.records(), 3);
        assert_eq!(buckets[0].input, 1, "the prompt is input");
        assert_eq!(buckets[1].model, 1, "the answer is model");
        assert_eq!(buckets[2].tools, 1, "the call is tools");
        assert_eq!(buckets[1].dominant(), Some(TrajectoryLane::Model));
        assert_eq!(BucketDensity::default().dominant(), None);
    }

    #[test]
    fn turn_marks_land_on_a_bucket_inside_the_track() {
        let mut log = TrajectoryLog::new();
        fill(&mut log, 4);
        log.begin_turn();
        log.push_user("second", None);
        log.observe(&AgentUpdate::Info("done".into()));

        let marks = turn_marks(log.records(), 4);
        assert_eq!(marks.len(), 1, "one boundary between two turns");
        assert!(marks[0] < 4, "a mark must be inside the track: {marks:?}");

        // A single turn has no boundary, so no rule is drawn.
        let mut single = TrajectoryLog::new();
        fill(&mut single, 3);
        assert!(turn_marks(single.records(), 8).is_empty());
    }

    #[test]
    fn the_glyph_ramp_never_claims_more_than_it_knows() {
        assert_eq!(density_glyph(0), '·');
        assert_eq!(density_glyph(1), '▁');
        assert_eq!(density_glyph(2), '▂');
        assert_eq!(density_glyph(4), '▄');
        assert_eq!(density_glyph(9), '█');
    }

    #[test]
    fn the_overview_collapses_only_when_the_rows_are_needed_elsewhere() {
        assert!(overview_default_collapsed(16), "24-row terminal");
        assert!(overview_default_collapsed(19));
        assert!(!overview_default_collapsed(20), "the floor itself expands");
        assert!(!overview_default_collapsed(22), "30-row terminal");
        assert!(!overview_default_collapsed(42), "50-row terminal");
    }

    #[test]
    fn an_empty_ledger_has_no_overview() {
        assert!(bucket_densities(&[], 10).iter().all(|b| b.total() == 0));
        assert!(bucket_densities(&[], 0).is_empty());
        assert_eq!(TrajectoryLog::new().summary(), TrajectorySummary::default());
        assert_eq!(TrajectoryLog::new().turns(), 0);
    }
}

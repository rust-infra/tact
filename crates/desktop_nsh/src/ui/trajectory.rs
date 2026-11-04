//! Geometry for the Trajectory page's overview timeline.
//!
//! The overview plots one record per sequence slot in three lanes, so the
//! page's painters and its hit test agree on one projection and a click can
//! select the block it landed on.

use tact_desktop_core::{
    TrajectoryLane, TrajectoryRecord, TrajectoryRecordId, TrajectorySpan, TrajectorySummary,
    TrajectoryView,
};

/// Height of the overview plot, in pixels.
pub const PLOT_HEIGHT: f32 = 50.;
/// Height of one lane inside the plot.
pub const LANE_HEIGHT: f32 = 14.;
/// The lane labels column width.
pub const LABELS_WIDTH: f32 = 48.;
/// A span is at least this wide so a single record stays clickable.
pub const MIN_SPAN_WIDTH: f32 = 2.5;
/// Vertical inset of the first lane.
pub const LANE_TOP: f32 = 7.;
/// Height of each individual block inside a lane.
pub const BLOCK_HEIGHT: f32 = 8.;
/// Gap kept between neighbouring blocks.
pub const BLOCK_GAP: f32 = 1.5;

/// Where a span sits in the plot's coordinate space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpanBox {
    pub index: usize,
    pub lane: TrajectoryLane,
    /// Left edge, in pixels from the track's left edge.
    pub left: f32,
    /// Width, in pixels.
    pub width: f32,
    /// Top edge, in pixels from the track's top edge.
    pub top: f32,
}

impl SpanBox {
    pub fn contains_x(&self, x: f32) -> bool {
        x >= self.left && x <= self.left + self.width
    }
}

/// The plot's layout for one track width.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TimelineLayout {
    pub boxes: Vec<SpanBox>,
    pub slot_width: f32,
    pub width: f32,
    /// Record indexes that open a turn, in plot order.
    pub turn_marks: Vec<usize>,
}

impl TimelineLayout {
    /// Project every span into the track, one equal-width column per record.
    pub fn new(view: &TrajectoryView, width: f32) -> Self {
        let width = width.max(0.);
        let count = view.spans().len();
        if count == 0 || width <= 0. {
            return Self {
                boxes: Vec::new(),
                slot_width: 0.,
                width,
                turn_marks: view.boundaries().to_vec(),
            };
        }
        let slot_width = width / count as f32;
        let boxes = view
            .spans()
            .iter()
            .map(|span| span_box(span, slot_width))
            .collect();
        Self {
            boxes,
            slot_width,
            width,
            turn_marks: view.boundaries().to_vec(),
        }
    }

    /// The record under a pointer at `x` pixels from the track's left edge.
    pub fn hit(&self, x: f32) -> Option<usize> {
        self.boxes
            .iter()
            .find(|span| span.contains_x(x))
            .map(|span| span.index)
    }

    /// The left edge of the record at `index`, in pixels.
    pub fn offset_of(&self, index: usize) -> f32 {
        index as f32 * self.slot_width
    }

    /// The first record whose column starts at or after `x`.
    pub fn nearest(&self, x: f32) -> Option<usize> {
        if self.boxes.is_empty() {
            return None;
        }
        let last = self.boxes.len() - 1;
        let index = if self.slot_width > 0. {
            ((x / self.slot_width).round() as isize).clamp(0, last as isize) as usize
        } else {
            0
        };
        Some(index)
    }
}

fn span_box(span: &TrajectorySpan, slot_width: f32) -> SpanBox {
    SpanBox {
        index: span.index,
        lane: span.lane,
        left: span.index as f32 * slot_width,
        width: (slot_width - BLOCK_GAP).max(MIN_SPAN_WIDTH),
        top: LANE_TOP + span.lane.row() as f32 * LANE_HEIGHT,
    }
}

/// The header's totals line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SummaryLine {
    pub turns: String,
    pub records: String,
    pub calls: String,
    pub measured: String,
}

impl SummaryLine {
    /// Build the four header figures. Measured time is shown only when the
    /// producers reported a duration, so the page never implies a total it
    /// cannot back with evidence.
    pub fn new(summary: TrajectorySummary, measured_label: &str) -> Self {
        Self {
            turns: summary.turns.to_string(),
            records: summary.records.to_string(),
            calls: summary.tool_calls.to_string(),
            measured: if summary.measured == 0 {
                "—".to_string()
            } else {
                let seconds = summary.measured_ms as f64 / 1_000.;
                if seconds >= 60. {
                    let minutes = (seconds / 60.).floor();
                    let rest = (seconds - minutes * 60.).round() as u64 % 60;
                    format!("{minutes}m {rest:02}s · {measured_label}")
                } else if seconds >= 1. {
                    format!("{seconds:.1}s · {measured_label}")
                } else {
                    format!("{}ms · {measured_label}", summary.measured_ms)
                }
            },
        }
    }
}

/// One ledger row as the page draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerRow {
    /// Stable ledger identity; this is what selecting a row stores.
    pub id: TrajectoryRecordId,
    /// 1-based ledger index, printed as `#N`.
    pub index: usize,
    pub turn: usize,
    /// Whether this row opens its turn.
    pub starts_turn: bool,
    pub kind_label: &'static str,
    pub text: String,
    pub status: &'static str,
    pub failed: bool,
    pub duration: Option<String>,
    /// The tool call's provider id, when there is one.
    pub tool_id: Option<String>,
}

impl LedgerRow {
    pub fn new(record: &TrajectoryRecord, starts_turn: bool) -> Self {
        Self {
            id: record.id,
            index: record.index,
            turn: record.turn,
            starts_turn,
            kind_label: record.kind.label(),
            text: record.text.to_string(),
            status: record.status_label(),
            failed: matches!(record.status, tact_desktop_core::RowStatus::Failed),
            duration: record.duration_ms.map(format_duration),
            tool_id: record.tool_id.as_ref().map(|id| id.to_string()),
        }
    }
}

/// The ledger projection: rows in record order, each marked when it opens a
/// turn.
///
/// `visible` holds 1-based record indexes (the `#N` the ledger prints), so a
/// search result list can drive the ledger without an off-by-one translation.
/// `starts_turn` is decided over the rows that survive the filter, so a
/// filtered ledger still groups its records by turn instead of losing every
/// header the query happened to skip.
pub fn ledger_rows(view: &TrajectoryView, visible: Option<&[usize]>) -> Vec<LedgerRow> {
    let mut previous_turn = None;
    view.records()
        .iter()
        .filter(|record| visible.is_none_or(|visible| visible.contains(&record.index)))
        .map(|record| {
            let starts_turn = previous_turn != Some(record.turn);
            previous_turn = Some(record.turn);
            LedgerRow::new(record, starts_turn)
        })
        .collect()
}

/// The footer stats line.
///
/// Every figure is optional: a producer that reported no usage, no cache
/// counters, or no context window leaves that figure out instead of showing a
/// zero the page cannot back with evidence.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatsLine {
    /// `N turns M steps`.
    pub turns: Option<String>,
    /// Throughput over the measured time, when both halves exist.
    pub rate: Option<String>,
    /// Total tokens the session moved.
    pub tokens: Option<String>,
    /// Share of prompt tokens served from cache.
    pub cache: Option<String>,
    /// How full the context window got.
    pub context: Option<String>,
}

impl StatsLine {
    /// Build the footer figures from a summary.
    ///
    /// `tok/s` needs both a token count and a measured duration; the cache
    /// share needs at least one cache counter; the context meter needs both a
    /// used count and a window size. Each is omitted rather than fabricated.
    pub fn new(summary: TrajectorySummary, labels: StatsLabels) -> Self {
        let steps: usize = summary.records;
        Self {
            turns: (summary.turns > 0 || steps > 0).then(|| {
                format!(
                    "{} {} {steps} {}",
                    summary.turns, labels.turns, labels.steps
                )
            }),
            rate: throughput(
                summary.input_tokens + summary.output_tokens,
                summary.measured_ms,
            ),
            tokens: (summary.input_tokens + summary.output_tokens > 0).then(|| {
                format!(
                    "{} {}",
                    compact_tokens(summary.input_tokens + summary.output_tokens),
                    labels.tokens
                )
            }),
            cache: cache_share(summary.cache_hit_tokens, summary.cache_miss_tokens)
                .map(|percent| format!("{} {percent}%", labels.cache_hit)),
            context: match summary.context_window {
                Some(window) if window > 0 && summary.context_used > 0 => Some(format!(
                    "{}%",
                    ((summary.context_used as u64 * 100) / window as u64).min(100)
                )),
                _ => None,
            },
        }
    }

    /// Whether the line has anything to show at all.
    pub fn is_empty(&self) -> bool {
        self.turns.is_none()
            && self.rate.is_none()
            && self.tokens.is_none()
            && self.cache.is_none()
            && self.context.is_none()
    }
}

/// The localized words the footer needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatsLabels {
    pub turns: &'static str,
    pub steps: &'static str,
    pub tokens: &'static str,
    pub cache_hit: &'static str,
}

/// Tokens per second over a measured window, or `None` without both halves.
pub fn throughput(tokens: u32, measured_ms: u64) -> Option<String> {
    if tokens == 0 || measured_ms == 0 {
        return None;
    }
    let per_second = tokens as f64 / (measured_ms as f64 / 1_000.);
    Some(format!("{per_second:.0} tok/s"))
}

/// The cache-hit share of the prompt tokens, or `None` when nothing was
/// reported on either side.
pub fn cache_share(hit: u32, miss: u32) -> Option<u64> {
    let total = hit as u64 + miss as u64;
    if total == 0 {
        return None;
    }
    Some((hit as u64 * 100) / total)
}

/// `338M tok`-style short form for a token total.
pub fn compact_tokens(tokens: u32) -> String {
    match tokens {
        0..=999 => tokens.to_string(),
        1_000..=999_999 => format!("{:.1}k", tokens as f64 / 1_000.),
        _ => format!("{:.0}M", tokens as f64 / 1_000_000.),
    }
}

/// Format a duration for the detail panel.
pub fn format_duration(milliseconds: u64) -> String {
    if milliseconds >= 60_000 {
        let minutes = milliseconds / 60_000;
        let rest = (milliseconds % 60_000) / 1_000;
        format!("{minutes}m {rest}s")
    } else if milliseconds >= 1_000 {
        format!("{:.2}s", milliseconds as f64 / 1_000.)
    } else {
        format!("{milliseconds}ms")
    }
}



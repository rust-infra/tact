use std::{
    collections::HashMap,
    fmt::Write,
    time::{Duration, Instant},
};

use comfy_table::{Cell, CellAlignment, ContentArrangement, Table, presets::UTF8_FULL};
use tact_protocol::TokenUsageInfo;

/// Tracks per-session statistics for the agent runtime.
#[derive(Debug)]
pub struct SessionStats {
    /// Number of LLM API calls (main-loop streaming + compaction).
    pub prompt_count: u64,
    /// Total characters across all serialized prompt JSON.
    pub total_prompt_chars: u64,
    /// Total characters across all serialized response content blocks.
    pub total_response_chars: u64,
    /// Number of `ContentBlock::Thinking` blocks returned by the LLM.
    pub thinking_blocks: u64,
    /// Total characters within thinking blocks.
    pub total_thinking_chars: u64,
    /// Number of context compaction operations performed.
    pub compactions: u64,
    /// Tool call counts keyed by tool name.
    pub tool_counts: HashMap<String, u64>,
    /// Successful tool call counts keyed by tool name.
    pub tool_success_counts: HashMap<String, u64>,
    /// Failed tool call counts keyed by tool name.
    pub tool_failure_counts: HashMap<String, u64>,
    /// Total wall-clock duration per tool in milliseconds.
    pub tool_total_durations_ms: HashMap<String, u64>,
    /// Number of timed executions per tool (for computing average).
    pub tool_timing_counts: HashMap<String, u64>,
    /// Wall-clock duration of each LLM API call.
    pub llm_call_durations: Vec<Duration>,
    /// Per-tool-execution durations in milliseconds.
    pub tool_durations_ms: Vec<u64>,
    /// Cumulative KV cache hit prompt tokens (DeepSeek).
    pub cache_hit_tokens: u64,
    /// Cumulative KV cache miss prompt tokens (DeepSeek).
    pub cache_miss_tokens: u64,
    /// Cumulative reasoning tokens.
    pub reasoning_tokens: u64,
    /// When the session started.
    pub start_time: Instant,
}

impl Default for SessionStats {
    fn default() -> Self {
        Self {
            prompt_count: 0,
            total_prompt_chars: 0,
            total_response_chars: 0,
            thinking_blocks: 0,
            total_thinking_chars: 0,
            compactions: 0,
            tool_counts: HashMap::new(),
            tool_success_counts: HashMap::new(),
            tool_failure_counts: HashMap::new(),
            tool_total_durations_ms: HashMap::new(),
            tool_timing_counts: HashMap::new(),
            llm_call_durations: Vec::new(),
            tool_durations_ms: Vec::new(),
            cache_hit_tokens: 0,
            cache_miss_tokens: 0,
            reasoning_tokens: 0,
            start_time: Instant::now(),
        }
    }
}

/// Format a Duration with the most appropriate unit: s, m:s, h:m, or d:h.
fn fmt_duration(d: Duration) -> String {
    let total_secs = d.as_secs_f64();
    if total_secs < 60.0 {
        format!("{:.1}s", total_secs)
    } else if total_secs < 3600.0 {
        let m = total_secs as u64 / 60;
        let s = (total_secs as u64) % 60;
        format!("{}m{}s", m, s)
    } else if total_secs < 86_400.0 {
        let h = total_secs as u64 / 3600;
        let m = ((total_secs as u64) % 3600) / 60;
        format!("{}h{}m", h, m)
    } else {
        let d = total_secs as u64 / 86_400;
        let h = ((total_secs as u64) % 86_400) / 3600;
        format!("{}d{}h", d, h)
    }
}

fn new_stats_table() -> Table {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Disabled)
        .force_no_tty();
    table
}

fn right_align_value_column(table: &mut Table) {
    if let Some(col) = table.column_mut(1) {
        col.set_cell_alignment(CellAlignment::Right);
    }
}

fn right_align_tool_numeric_columns(table: &mut Table) {
    for idx in 1..=3 {
        if let Some(col) = table.column_mut(idx) {
            col.set_cell_alignment(CellAlignment::Right);
        }
    }
}

fn add_metric_row(table: &mut Table, metric: &str, value: impl Into<String>) {
    table.add_row(vec![
        Cell::new(metric),
        Cell::new(value.into()).set_alignment(CellAlignment::Right),
    ]);
}

fn fmt_tool_wall_ms(total_ms: u64) -> String {
    if total_ms >= 1000 {
        format!("{:.1}s", total_ms as f64 / 1000.0)
    } else {
        format!("{total_ms}ms")
    }
}

fn fmt_count_sf(count: u64, success: u64, failure: u64) -> String {
    format!("{count} ({success}/{failure})")
}

impl SessionStats {
    /// Accumulate token usage info from an LLM call (streaming or compaction).
    pub fn record_token_usage(&mut self, usage: &TokenUsageInfo) {
        self.cache_hit_tokens += usage.prompt_cache_hit_tokens as u64;
        self.cache_miss_tokens += usage.prompt_cache_miss_tokens as u64;
        self.reasoning_tokens += usage.reasoning_tokens as u64;
    }

    /// Produce a human-readable summary of all recorded statistics.
    pub fn summary(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "── Session Stats ─────────────────────────────");

        let total_llm_ms: f64 = self
            .llm_call_durations
            .iter()
            .map(|d| d.as_secs_f64() * 1000.0)
            .sum();

        let mut head = new_stats_table();
        head.set_header(vec!["Metric", "Value"]);
        add_metric_row(
            &mut head,
            "Elapsed",
            fmt_duration(self.start_time.elapsed()),
        );
        add_metric_row(&mut head, "LLM API calls", self.prompt_count.to_string());
        add_metric_row(
            &mut head,
            "Total LLM time",
            fmt_duration(Duration::from_secs_f64(total_llm_ms / 1000.0)),
        );
        add_metric_row(
            &mut head,
            "Prompt chars sent",
            self.total_prompt_chars.to_string(),
        );
        add_metric_row(
            &mut head,
            "Response chars rcvd",
            self.total_response_chars.to_string(),
        );
        add_metric_row(
            &mut head,
            "Thinking blocks",
            self.thinking_blocks.to_string(),
        );
        add_metric_row(
            &mut head,
            "Thinking chars",
            self.total_thinking_chars.to_string(),
        );
        add_metric_row(&mut head, "Compactions", self.compactions.to_string());
        right_align_value_column(&mut head);
        let _ = writeln!(out, "{head}");

        if !self.tool_counts.is_empty() {
            let mut counts: Vec<_> = self.tool_counts.iter().collect();
            counts.sort_by_key(|(name, _)| *name);

            let total_tool: u64 = self.tool_counts.values().sum();
            let total_success: u64 = self.tool_success_counts.values().sum();
            let total_failure: u64 = self.tool_failure_counts.values().sum();

            let mut tools = new_stats_table();
            tools.set_header(vec!["Tool", "Count(s/f)", "Total", "Avg"]);
            tools.add_row(vec![
                Cell::new("Total"),
                Cell::new(fmt_count_sf(total_tool, total_success, total_failure))
                    .set_alignment(CellAlignment::Right),
                Cell::new(""),
                Cell::new(""),
            ]);

            for (name, count) in counts {
                let success = self
                    .tool_success_counts
                    .get(name.as_str())
                    .copied()
                    .unwrap_or(0);
                let failure = self
                    .tool_failure_counts
                    .get(name.as_str())
                    .copied()
                    .unwrap_or(0);
                let total_ms = self
                    .tool_total_durations_ms
                    .get(name.as_str())
                    .copied()
                    .unwrap_or(0);
                let timing_count = self
                    .tool_timing_counts
                    .get(name.as_str())
                    .copied()
                    .unwrap_or(0);
                let avg_ms = if timing_count > 0 {
                    total_ms as f64 / timing_count as f64
                } else {
                    0.0
                };
                tools.add_row(vec![
                    Cell::new(name.as_str()),
                    Cell::new(fmt_count_sf(*count, success, failure))
                        .set_alignment(CellAlignment::Right),
                    Cell::new(fmt_tool_wall_ms(total_ms)).set_alignment(CellAlignment::Right),
                    Cell::new(format!("{avg_ms:.0}ms")).set_alignment(CellAlignment::Right),
                ]);
            }
            right_align_tool_numeric_columns(&mut tools);

            let _ = writeln!(out);
            let _ = writeln!(out, "Tool calls");
            let _ = writeln!(out, "{tools}");
        }

        let has_tool_timings = !self.tool_durations_ms.is_empty();
        let has_cache = self.cache_hit_tokens > 0 || self.cache_miss_tokens > 0;
        let has_reasoning = self.reasoning_tokens > 0;

        if has_tool_timings || has_cache || has_reasoning {
            let mut trail = new_stats_table();
            trail.set_header(vec!["Metric", "Value"]);

            if has_tool_timings {
                let total_tool_ms: u64 = self.tool_durations_ms.iter().sum();
                let avg_ms = total_tool_ms as f64 / self.tool_durations_ms.len() as f64;
                add_metric_row(
                    &mut trail,
                    "Total tool time",
                    fmt_duration(Duration::from_millis(total_tool_ms)),
                );
                add_metric_row(&mut trail, "Avg tool time", format!("{avg_ms:.1}ms"));
            }

            if has_cache {
                let cache_total = self.cache_hit_tokens + self.cache_miss_tokens;
                let hit_rate = if cache_total > 0 {
                    (self.cache_hit_tokens as f64 / cache_total as f64) * 100.0
                } else {
                    0.0
                };
                add_metric_row(
                    &mut trail,
                    "Cache hit tokens",
                    self.cache_hit_tokens.to_string(),
                );
                add_metric_row(
                    &mut trail,
                    "Cache miss tokens",
                    self.cache_miss_tokens.to_string(),
                );
                add_metric_row(&mut trail, "Cache hit rate", format!("{hit_rate:.1}%"));
            }

            if has_reasoning {
                add_metric_row(
                    &mut trail,
                    "Reasoning tokens",
                    self.reasoning_tokens.to_string(),
                );
            }

            right_align_value_column(&mut trail);
            let _ = writeln!(out, "{trail}");
        }

        let _ = writeln!(out, "─────────────────────────────────────────────");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_accumulates_all_fields() {
        let mut s = SessionStats::default();
        s.record_token_usage(&TokenUsageInfo {
            prompt_cache_hit_tokens: 1000,
            prompt_cache_miss_tokens: 500,
            reasoning_tokens: 200,
            ..Default::default()
        });
        assert_eq!(s.cache_hit_tokens, 1000);
        assert_eq!(s.cache_miss_tokens, 500);
        assert_eq!(s.reasoning_tokens, 200);
        let _ = s.summary(); // smoke check
    }

    #[test]
    fn fmt_duration_picks_unit() {
        assert_eq!(fmt_duration(Duration::from_secs(0)), "0.0s");
        assert_eq!(fmt_duration(Duration::from_secs(12)), "12.0s");
        assert_eq!(fmt_duration(Duration::from_secs(59)), "59.0s");
        assert_eq!(fmt_duration(Duration::from_secs(60)), "1m0s");
        assert_eq!(fmt_duration(Duration::from_secs(125)), "2m5s");
        assert_eq!(fmt_duration(Duration::from_secs(3600)), "1h0m");
        assert_eq!(fmt_duration(Duration::from_secs(7384)), "2h3m");
        assert_eq!(fmt_duration(Duration::from_secs(86_400)), "1d0h");
        assert_eq!(fmt_duration(Duration::from_secs(100_000)), "1d3h");
    }

    #[test]
    fn summary_uses_metric_and_tool_tables() {
        let mut s = SessionStats::default();
        s.prompt_count = 1;
        s.tool_counts.insert("bash".into(), 2);
        s.tool_success_counts.insert("bash".into(), 2);
        s.tool_failure_counts.insert("bash".into(), 0);
        s.tool_total_durations_ms.insert("bash".into(), 1500);
        s.tool_timing_counts.insert("bash".into(), 2);
        s.tool_durations_ms.extend([1000, 500]);

        let text = s.summary();
        assert!(text.contains("Metric"), "missing metrics header:\n{text}");
        assert!(
            text.contains("Value"),
            "missing metrics Value header:\n{text}"
        );
        assert!(
            text.contains("Tool calls"),
            "missing Tool calls label:\n{text}"
        );
        assert!(
            text.contains("Count(s/f)"),
            "missing tools Count header:\n{text}"
        );
        assert!(text.contains("bash"), "missing tool row:\n{text}");
        assert!(
            text.contains("Total"),
            "missing Total row or Total column:\n{text}"
        );
    }
}

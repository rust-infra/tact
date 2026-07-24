use std::{
    collections::HashMap,
    fmt::Write,
    time::{Duration, Instant},
};

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

/// Escape cell text for GFM pipe tables (consumed by tui-markdown in the TUI).
fn md_cell(s: &str) -> String {
    s.replace('|', "\\|")
}

#[derive(Clone, Copy)]
enum ColAlign {
    Left,
    Right,
    Center,
}

fn parse_align(spec: &str) -> ColAlign {
    let s = spec.trim();
    match (s.starts_with(':'), s.ends_with(':')) {
        (true, true) => ColAlign::Center,
        (false, true) => ColAlign::Right,
        _ => ColAlign::Left,
    }
}

fn display_width(s: &str) -> usize {
    s.chars().count()
}

fn pad_cell(text: &str, width: usize, align: ColAlign) -> String {
    let w = display_width(text);
    if w >= width {
        return text.to_string();
    }
    let pad = width - w;
    match align {
        ColAlign::Left => format!("{text}{}", " ".repeat(pad)),
        ColAlign::Right => format!("{}{text}", " ".repeat(pad)),
        ColAlign::Center => {
            let left = pad / 2;
            let right = pad - left;
            format!("{}{text}{}", " ".repeat(left), " ".repeat(right))
        }
    }
}

fn separator_cell(width: usize, align: ColAlign) -> String {
    let width = width.max(3);
    match align {
        ColAlign::Left => "-".repeat(width),
        ColAlign::Right => format!("{}:", "-".repeat(width - 1)),
        ColAlign::Center => format!(":{}:", "-".repeat(width - 2)),
    }
}

/// Write a GFM pipe table with space-padded cells so plain-text (CLI / exit
/// `eprintln`) columns line up; tui-markdown still accepts the same source.
fn write_gfm_table(out: &mut String, headers: &[&str], aligns: &[&str], rows: &[Vec<String>]) {
    debug_assert_eq!(headers.len(), aligns.len());
    let cols = headers.len();
    let aligns: Vec<ColAlign> = aligns.iter().map(|a| parse_align(a)).collect();

    let headers: Vec<String> = headers.iter().map(|h| md_cell(h)).collect();
    let body: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            debug_assert_eq!(row.len(), cols);
            row.iter().map(|c| md_cell(c)).collect()
        })
        .collect();

    let mut widths: Vec<usize> = headers.iter().map(|h| display_width(h)).collect();
    for row in &body {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(display_width(cell));
        }
    }
    for w in &mut widths {
        *w = (*w).max(3);
    }

    let _ = writeln!(
        out,
        "| {} |",
        headers
            .iter()
            .enumerate()
            .map(|(i, h)| pad_cell(h, widths[i], ColAlign::Left))
            .collect::<Vec<_>>()
            .join(" | ")
    );
    let _ = writeln!(
        out,
        "| {} |",
        widths
            .iter()
            .enumerate()
            .map(|(i, &w)| separator_cell(w, aligns[i]))
            .collect::<Vec<_>>()
            .join(" | ")
    );
    for row in &body {
        let _ = writeln!(
            out,
            "| {} |",
            row.iter()
                .enumerate()
                .map(|(i, c)| pad_cell(c, widths[i], aligns[i]))
                .collect::<Vec<_>>()
                .join(" | ")
        );
    }
}

fn write_metric_table(out: &mut String, rows: &[(&str, String)]) {
    let body: Vec<Vec<String>> = rows
        .iter()
        .map(|(metric, value)| vec![(*metric).to_string(), value.clone()])
        .collect();
    write_gfm_table(out, &["Metric", "Value"], &["---", "---:"], &body);
}

impl SessionStats {
    /// Accumulate token usage info from an LLM call (streaming or compaction).
    pub fn record_token_usage(&mut self, usage: &TokenUsageInfo) {
        self.cache_hit_tokens += usage.prompt_cache_hit_tokens as u64;
        self.cache_miss_tokens += usage.prompt_cache_miss_tokens as u64;
        self.reasoning_tokens += usage.reasoning_tokens as u64;
    }

    /// Produce a human-readable summary of all recorded statistics.
    ///
    /// Tables are GFM pipe markdown so the TUI can render them via
    /// `tui-markdown` (Unicode box borders + alignment). Cells are
    /// space-padded so CLI / headless exit `eprintln` also lines up.
    pub fn summary(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "── Session Stats ─────────────────────────────");
        let _ = writeln!(out);

        let total_llm: Duration = self.llm_call_durations.iter().copied().sum();

        let head_rows: Vec<(&str, String)> = vec![
            ("Elapsed", fmt_duration(self.start_time.elapsed())),
            ("LLM API calls", self.prompt_count.to_string()),
            ("Total LLM time", fmt_duration(total_llm)),
            ("Prompt chars sent", self.total_prompt_chars.to_string()),
            ("Response chars rcvd", self.total_response_chars.to_string()),
            ("Thinking blocks", self.thinking_blocks.to_string()),
            ("Thinking chars", self.total_thinking_chars.to_string()),
            ("Compactions", self.compactions.to_string()),
        ];
        write_metric_table(&mut out, &head_rows);

        if !self.tool_counts.is_empty() {
            let mut counts: Vec<_> = self.tool_counts.iter().collect();
            counts.sort_by_key(|(name, _)| *name);

            let total_tool: u64 = self.tool_counts.values().sum();
            let total_success: u64 = self.tool_success_counts.values().sum();
            let total_failure: u64 = self.tool_failure_counts.values().sum();

            let mut tool_rows = vec![vec![
                "Total".to_string(),
                fmt_count_sf(total_tool, total_success, total_failure),
                String::new(),
                String::new(),
            ]];

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
                tool_rows.push(vec![
                    name.clone(),
                    fmt_count_sf(*count, success, failure),
                    fmt_tool_wall_ms(total_ms),
                    format!("{avg_ms:.0}ms"),
                ]);
            }

            let _ = writeln!(out);
            let _ = writeln!(out, "Tool calls");
            let _ = writeln!(out);
            write_gfm_table(
                &mut out,
                &["Tool", "Count(s/f)", "Total", "Avg"],
                &["---", "---:", "---:", "---:"],
                &tool_rows,
            );
        }

        let has_tool_timings = !self.tool_durations_ms.is_empty();
        let has_cache = self.cache_hit_tokens > 0 || self.cache_miss_tokens > 0;
        let has_reasoning = self.reasoning_tokens > 0;

        if has_tool_timings || has_cache || has_reasoning {
            let mut trail_rows: Vec<(&str, String)> = Vec::new();

            if has_tool_timings {
                let total_tool_ms: u64 = self.tool_durations_ms.iter().sum();
                let avg_ms = total_tool_ms as f64 / self.tool_durations_ms.len() as f64;
                trail_rows.push((
                    "Total tool time",
                    fmt_duration(Duration::from_millis(total_tool_ms)),
                ));
                trail_rows.push(("Avg tool time", format!("{avg_ms:.1}ms")));
            }

            if has_cache {
                let cache_total = self.cache_hit_tokens + self.cache_miss_tokens;
                let hit_rate = (self.cache_hit_tokens as f64 / cache_total as f64) * 100.0;
                trail_rows.push(("Cache hit tokens", self.cache_hit_tokens.to_string()));
                trail_rows.push(("Cache miss tokens", self.cache_miss_tokens.to_string()));
                trail_rows.push(("Cache hit rate", format!("{hit_rate:.1}%")));
            }

            if has_reasoning {
                trail_rows.push(("Reasoning tokens", self.reasoning_tokens.to_string()));
            }

            let _ = writeln!(out);
            write_metric_table(&mut out, &trail_rows);
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
    fn summary_uses_gfm_metric_and_tool_tables() {
        let mut s = SessionStats {
            prompt_count: 1,
            ..Default::default()
        };
        s.tool_counts.insert("bash".into(), 2);
        s.tool_success_counts.insert("bash".into(), 2);
        s.tool_failure_counts.insert("bash".into(), 0);
        s.tool_total_durations_ms.insert("bash".into(), 1500);
        s.tool_timing_counts.insert("bash".into(), 2);
        s.tool_durations_ms.extend([1000, 500]);

        let text = s.summary();
        assert!(
            text.contains("| Metric"),
            "missing metrics GFM header:\n{text}"
        );
        assert!(
            text.contains("| Value |"),
            "missing Value column header:\n{text}"
        );
        assert!(
            text.contains("| ---") || text.contains("|-----"),
            "missing metrics alignment row:\n{text}"
        );
        assert!(
            text.contains("Tool calls"),
            "missing Tool calls label:\n{text}"
        );
        assert!(text.contains("| Tool"), "missing tools GFM header:\n{text}");
        assert!(
            text.contains("Count(s/f)"),
            "missing tools Count header:\n{text}"
        );
        assert!(text.contains("bash"), "missing tool row:\n{text}");
        assert!(text.contains("| Total"), "missing Total row:\n{text}");
        assert!(
            text.contains("Total tool time"),
            "missing trailing metrics:\n{text}"
        );
    }

    #[test]
    fn gfm_table_pads_columns_for_plain_text_alignment() {
        let mut out = String::new();
        write_gfm_table(
            &mut out,
            &["Metric", "Value"],
            &["---", "---:"],
            &[
                vec!["Elapsed".into(), "1.2s".into()],
                vec!["Response chars rcvd".into(), "0".into()],
            ],
        );
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines.len() >= 4, "expected header/sep/2 rows:\n{out}");
        let pipe_cols: Vec<Vec<usize>> = lines
            .iter()
            .take(4)
            .map(|line| {
                line.char_indices()
                    .filter_map(|(i, c)| (c == '|').then_some(i))
                    .collect()
            })
            .collect();
        for cols in &pipe_cols[1..] {
            assert_eq!(
                cols, &pipe_cols[0],
                "pipe columns must align across rows:\n{out}"
            );
        }
        assert!(
            out.contains("| Response chars rcvd |"),
            "metric column should be left-padded to max width:\n{out}"
        );
        let value_row = lines
            .iter()
            .find(|l| l.contains("Response chars rcvd"))
            .expect("missing response row");
        assert!(
            value_row.ends_with("|     0 |") || value_row.contains("|     0 |"),
            "value column should be right-aligned in:\n{value_row}"
        );
    }
}

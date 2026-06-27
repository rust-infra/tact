use std::collections::HashMap;
use std::fmt::Write;
use std::time::{Duration, Instant};

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
        let _ = writeln!(
            out,
            "  Elapsed:               {:>8}",
            fmt_duration(self.start_time.elapsed())
        );

        let _ = writeln!(out, "  LLM API calls:         {:>8}", self.prompt_count);

        let total_llm_ms: f64 = self
            .llm_call_durations
            .iter()
            .map(|d| d.as_secs_f64() * 1000.0)
            .sum();
        let _ = writeln!(
            out,
            "  Total LLM time:        {:>8}",
            fmt_duration(Duration::from_secs_f64(total_llm_ms / 1000.0))
        );

        let _ = writeln!(out, "  Prompt chars sent:     {:>8}", self.total_prompt_chars);
        let _ = writeln!(
            out,
            "  Response chars rcvd:  {:>8}",
            self.total_response_chars
        );

        let _ = writeln!(out, "  Thinking blocks:       {:>8}", self.thinking_blocks);
        let _ = writeln!(
            out,
            "  Thinking chars:        {:>8}",
            self.total_thinking_chars
        );

        let _ = writeln!(out, "  Compactions:           {:>8}", self.compactions);

        let total_tool: u64 = self.tool_counts.values().sum();
        let _ = writeln!(out, "  Tool calls:            {:>8}", total_tool);

        if !self.tool_counts.is_empty() {
            let mut counts: Vec<_> = self.tool_counts.iter().collect();
            counts.sort_by_key(|(name, _)| *name);
            for (name, count) in counts {
                let _ = writeln!(out, "    {:<22} {:>4}", name, count);
            }
        }

        if !self.tool_durations_ms.is_empty() {
            let total_tool_ms: u64 = self.tool_durations_ms.iter().sum();
            let _ = writeln!(
                out,
                "  Total tool time:       {:>8}",
                fmt_duration(Duration::from_millis(total_tool_ms))
            );
        }

        if self.cache_hit_tokens > 0 || self.cache_miss_tokens > 0 {
            let cache_total = self.cache_hit_tokens + self.cache_miss_tokens;
            let hit_rate = if cache_total > 0 {
                (self.cache_hit_tokens as f64 / cache_total as f64) * 100.0
            } else {
                0.0
            };
            let _ = writeln!(
                out,
                "  Cache hit tokens:      {:>8}",
                self.cache_hit_tokens
            );
            let _ = writeln!(
                out,
                "  Cache miss tokens:     {:>8}",
                self.cache_miss_tokens
            );
            let _ = writeln!(out, "  Cache hit rate:        {:>8.1}%", hit_rate);
        }

        if self.reasoning_tokens > 0 {
            let _ = writeln!(
                out,
                "  Reasoning tokens:      {:>8}",
                self.reasoning_tokens
            );
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
}

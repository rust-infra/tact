use std::collections::HashMap;
use std::fmt::Write;
use std::time::{Duration, Instant};

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
            start_time: Instant::now(),
        }
    }
}

impl SessionStats {
    /// Produce a human-readable summary of all recorded statistics.
    pub fn summary(&self) -> String {
        let elapsed = self.start_time.elapsed();
        let mut out = String::new();

        let _ = writeln!(out, "── Session Stats ─────────────────────────────");
        let _ = writeln!(out, "  Elapsed:               {:>8.1}s", elapsed.as_secs_f64());

        let _ = writeln!(out, "  LLM API calls:         {:>8}", self.prompt_count);

        let total_llm_ms: f64 = self
            .llm_call_durations
            .iter()
            .map(|d| d.as_secs_f64() * 1000.0)
            .sum();
        let _ = writeln!(out, "  Total LLM time:        {:>8.1}s", total_llm_ms / 1000.0);

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
                "  Total tool time:       {:>8.1}s",
                total_tool_ms as f64 / 1000.0
            );
        }

        let _ = writeln!(out, "─────────────────────────────────────────────");
        out
    }
}

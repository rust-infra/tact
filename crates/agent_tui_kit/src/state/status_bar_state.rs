/// Bottom status bar data: model info, token stats, etc.
pub struct StatusBarState {
    pub git_branch: String,
    pub model_name: String,
    pub model_max_tokens: u32,
    pub model_thinking_budget: Option<u32>,
    pub model_reasoning_effort: Option<String>,
    pub token_prompt: u32,
    pub token_completion: u32,
    pub token_total: u32,
    pub token_cache_hit: u32,
    pub token_cache_miss: u32,
    pub token_reasoning: u32,
    /// User turns dispatched in this session (seeded from persisted history on
    /// resume). Incremented by the TUI at task dispatch.
    pub turn_user: u32,
    /// LLM turns (agent-loop iterations) completed in the current task.
    /// Written by [`crate::components::status_bar::StatusBarComponent`] on
    /// `AgentUpdate::TurnStats`, reset by the host at task dispatch.
    pub turn_llm: u32,
    /// Agent-loop turn cap (`Agent::max_turns`) reported by the last
    /// `TurnStats`; `None` = unbounded. Plumbed but not rendered today.
    pub turn_llm_cap: Option<u32>,
    /// Wall-clock seconds of the most recently finished turn.
    pub turn_last_secs: Option<u64>,
    /// Completed turns and their summed wall-clock seconds (for the average).
    pub turn_done: u32,
    pub turn_total_secs: u64,
    /// Active permission mode: "default" | "plan" | "auto".
    pub permission_mode: String,
}

impl StatusBarState {
    pub fn new(git_branch: String) -> Self {
        Self {
            git_branch,
            model_name: String::new(),
            model_max_tokens: 0,
            model_thinking_budget: None,
            model_reasoning_effort: None,
            token_prompt: 0,
            token_completion: 0,
            token_total: 0,
            token_cache_hit: 0,
            token_cache_miss: 0,
            token_reasoning: 0,
            turn_user: 0,
            turn_llm: 0,
            turn_llm_cap: None,
            turn_last_secs: None,
            turn_done: 0,
            turn_total_secs: 0,
            permission_mode: "auto".to_string(),
        }
    }
}

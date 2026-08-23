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
            permission_mode: "auto".to_string(),
        }
    }
}

/// Bottom status bar data: model info, token stats, etc.
pub(crate) struct StatusBarState {
    pub(crate) git_branch: String,
    pub(crate) model_name: String,
    pub(crate) model_max_tokens: u32,
    pub(crate) model_thinking_budget: Option<u32>,
    pub(crate) token_prompt: u32,
    pub(crate) token_completion: u32,
    pub(crate) token_total: u32,
    pub(crate) token_cache_hit: u32,
    pub(crate) token_cache_miss: u32,
}

impl StatusBarState {
    pub(crate) fn new(git_branch: String) -> Self {
        Self {
            git_branch,
            model_name: String::new(),
            model_max_tokens: 0,
            model_thinking_budget: None,
            token_prompt: 0,
            token_completion: 0,
            token_total: 0,
            token_cache_hit: 0,
            token_cache_miss: 0,
        }
    }
}

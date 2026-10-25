use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct PromptContext {
    pub workspace: PathBuf,
    pub session_id: Option<String>,
}

pub trait PromptSection: Send + Sync {
    fn id(&self) -> &'static str;
    fn priority(&self) -> i32;
    fn render(&self, ctx: &PromptContext) -> Option<String>;
}

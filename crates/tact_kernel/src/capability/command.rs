/// Where a command is available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandScope {
    Runtime,
    View(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandMeta {
    pub name: String,
    pub aliases: Vec<String>,
    pub summary: String,
    pub usage: String,
    pub scope: CommandScope,
    pub needs_args: bool,
}

pub trait Command: Send + Sync {
    fn meta(&self) -> &CommandMeta;
}

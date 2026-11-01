use tact_desktop_core::ReaderRef;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReaderSource {
    Transcript,
    Surface,
    Account,
    Static,
}

pub fn source(reader: &ReaderRef) -> ReaderSource {
    match reader {
        ReaderRef::ToolOutput(_) | ReaderRef::Diff(_) | ReaderRef::Thinking(_) => {
            ReaderSource::Transcript
        }
        ReaderRef::SystemPrompt | ReaderRef::Stats | ReaderRef::TasksDag | ReaderRef::History => {
            ReaderSource::Surface
        }
        ReaderRef::Balance => ReaderSource::Account,
        ReaderRef::Help | ReaderRef::Subagent(_) | ReaderRef::Background(_) => ReaderSource::Static,
    }
}

pub fn is_read_only(reader: &ReaderRef) -> bool {
    !matches!(reader, ReaderRef::Background(id) if id.is_empty())
}

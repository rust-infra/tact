use std::path::PathBuf;

use url::Url;

use crate::{FailureView, ReaderRequest};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopEffect {
    CopyText(String),
    OpenPath(PathBuf),
    RevealInFinder(PathBuf),
    OpenExternal(Url),
    OpenReader(ReaderRequest),
    SaveTranscript {
        markdown: String,
        suggested_name: String,
    },
    Notify {
        title: String,
        body: String,
    },
    WindowCloseRequested,
}

#[derive(Debug, Clone)]
pub enum DesktopEvent {
    Committed(crate::StateCommit),
    Effect(DesktopEffect),
    Fatal(FailureView),
    /// One step of the voice worker's lifecycle, forwarded off the actor's
    /// critical path so dictation feedback never waits for a snapshot.
    Voice(tact::voice::VoiceEvent),
}

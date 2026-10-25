//! The runtime service contracts a host talks to.
//!
//! These are the ports between a *view* and the *runtime*, not between the
//! runtime and its capabilities. A view asks "list the sessions in this
//! workspace" and "close the one you opened"; it never sees how a session is
//! stored, which is why the types here are opaque handles and neutral summaries.
//!
//! They live in the kernel because both a view and every runtime implementation
//! need the same shape. The extension crate implements them; a host drives them.

use std::path::Path;

use async_trait::async_trait;

/// An opaque session identity.
///
/// A newtype rather than a bare `String`: a session id and a run id and a tool
/// id are all strings, and passing the wrong one is a bug the compiler can
/// catch here.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(String);

impl SessionId {
    /// Wrap an id the runtime produced.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A session a host can list.
///
/// Deliberately shallow: enough to draw a picker row, and nothing that would
/// make a view depend on the transcript format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub id: SessionId,
    /// The workspace the session was opened in.
    pub root_dir: String,
    pub created_at: String,
    pub updated_at: String,
    pub message_count: i64,
}

/// How a session is being opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionOpen {
    /// Continue an existing session by id.
    Resume(SessionId),
    /// Continue the most recent session in the workspace, if there is one.
    ResumeLast,
    /// Start a new session with a caller-chosen id.
    ///
    /// The id is supplied rather than generated here so a host that needs to
    /// know it up front (to label a log line, to take a lock) does not have to
    /// ask twice.
    New(SessionId),
}

/// An open session.
///
/// Holds only what a host passes back to `close`; the runtime keeps the
/// transcript, the lock and the provider state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionHandle {
    pub id: SessionId,
}

/// What a host can do with sessions.
///
/// `open` resolves the id *and* takes the lock, so a host cannot end up with a
/// session it never locked; `close` releases only the matching lock.
#[async_trait]
pub trait SessionService: Send + Sync {
    /// Resolve and open a session in `workspace`.
    async fn open(&self, workspace: &Path, request: SessionOpen) -> anyhow::Result<SessionHandle>;

    /// The sessions recorded for `workspace`, newest first.
    async fn list(&self, workspace: &Path) -> anyhow::Result<Vec<SessionSummary>>;

    /// Release the session's lock.
    ///
    /// Idempotent: closing an already-closed handle is not an error, because a
    /// host that unwinds through an error path has no way to know.
    async fn close(&self, session: SessionHandle) -> anyhow::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session id is opaque but comparable and displayable.
    ///
    /// Display matters: the id is printed in the exit line and in `/sessions`,
    /// and a `Debug`-only type would leak `SessionId("…")` to a user.
    #[test]
    fn a_session_id_displays_its_value() {
        let id = SessionId::new("abc123");
        assert_eq!(id.as_str(), "abc123");
        assert_eq!(id.to_string(), "abc123");
        assert_eq!(id, SessionId::new("abc123"));
        assert_ne!(id, SessionId::new("abc124"));
    }

    /// A stub service can satisfy the trait with no storage.
    ///
    /// This is the property a view test depends on: it can hand the view a
    /// `SessionService` without a database.
    #[tokio::test]
    async fn the_trait_is_implementable_without_storage() {
        struct Memory {
            sessions: std::sync::Mutex<Vec<SessionSummary>>,
        }

        #[async_trait]
        impl SessionService for Memory {
            async fn open(
                &self,
                workspace: &Path,
                request: SessionOpen,
            ) -> anyhow::Result<SessionHandle> {
                let id = match request {
                    SessionOpen::Resume(id) | SessionOpen::New(id) => id,
                    SessionOpen::ResumeLast => SessionId::new("last"),
                };
                self.sessions.lock().expect("lock").push(SessionSummary {
                    id: id.clone(),
                    root_dir: workspace.display().to_string(),
                    created_at: "now".into(),
                    updated_at: "now".into(),
                    message_count: 0,
                });
                Ok(SessionHandle { id })
            }

            async fn list(&self, workspace: &Path) -> anyhow::Result<Vec<SessionSummary>> {
                let root = workspace.display().to_string();
                Ok(self
                    .sessions
                    .lock()
                    .expect("lock")
                    .iter()
                    .filter(|summary| summary.root_dir == root)
                    .cloned()
                    .collect())
            }

            async fn close(&self, _session: SessionHandle) -> anyhow::Result<()> {
                Ok(())
            }
        }

        let service = Memory {
            sessions: std::sync::Mutex::new(Vec::new()),
        };
        let workspace = Path::new("/repo");
        let handle = service
            .open(workspace, SessionOpen::Resume(SessionId::new("s1")))
            .await
            .expect("open");
        assert_eq!(handle.id.as_str(), "s1");
        let listed = service.list(workspace).await.expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, handle.id);
        service.close(handle).await.expect("close");
    }

    /// Listing is scoped to the workspace it was asked about.
    ///
    /// A view that shows another workspace's sessions is worse than one that
    /// shows none: the user would resume a session from the wrong project.
    #[tokio::test]
    async fn listing_is_scoped_to_the_workspace() {
        struct Fixed;
        #[async_trait]
        impl SessionService for Fixed {
            async fn open(
                &self,
                _workspace: &Path,
                _request: SessionOpen,
            ) -> anyhow::Result<SessionHandle> {
                unreachable!("not used")
            }

            async fn list(&self, workspace: &Path) -> anyhow::Result<Vec<SessionSummary>> {
                Ok(vec![SessionSummary {
                    id: SessionId::new("s"),
                    root_dir: workspace.display().to_string(),
                    created_at: String::new(),
                    updated_at: String::new(),
                    message_count: 0,
                }])
            }

            async fn close(&self, _session: SessionHandle) -> anyhow::Result<()> {
                Ok(())
            }
        }

        let listed = Fixed.list(Path::new("/project-a")).await.expect("list");
        assert_eq!(listed[0].root_dir, "/project-a");
    }
}

/// One unit of work a host asks the agent to perform.
///
/// Neutral by construction: it carries what the *host* knows (the text a user
/// typed, an optional goal binding) and nothing about how the model is called.
/// The conversation itself never crosses this boundary — the agent owns it, and a
/// request that carried the transcript would make every caller responsible for
/// assembling one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TurnRequest {
    /// The user's text for this turn.
    pub input: String,
    /// Images the user attached to this turn, if any.
    pub images: Vec<TurnImage>,
}

impl TurnRequest {
    /// A text-only request.
    pub fn text(input: impl Into<String>) -> Self {
        Self {
            input: input.into(),
            images: Vec::new(),
        }
    }

    /// Whether this request carries anything at all.
    ///
    /// An empty request is not a turn: submitting one would append an empty user
    /// message and spend a model call on it.
    pub fn is_empty(&self) -> bool {
        self.input.trim().is_empty() && self.images.is_empty()
    }
}

/// An image attached to a turn.
///
/// Bytes plus a media type, not a provider-specific part: which encoding a
/// provider wants is the provider adapter's problem, and a DTO that named one
/// would leak that choice into every caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnImage {
    /// The media type the bytes are in, e.g. `image/png`.
    pub media_type: String,
    /// Base64-encoded image data.
    pub data: String,
}

/// Per-turn limits and bindings.
///
/// These are the knobs a caller is allowed to set for one turn. Everything else
/// about how a turn runs belongs to the agent's own configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TurnOptions {
    /// Opaque run binding forwarded to tools; only Goal/Task extensions
    /// interpret it.
    pub active_goal_id: Option<String>,
    /// A cap on model iterations for this turn.
    pub max_turns: Option<u32>,
}

/// Why a turn stopped.
///
/// Kept as a closed enum rather than a string: a caller that has to branch on the
/// reason (a Goal loop decides whether to continue) needs the compiler to say
/// when a new reason appears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnStop {
    /// The model finished on its own.
    Completed,
    /// The turn handed control back deliberately (see [`YieldReason`]).
    Yielded(crate::tool::result::YieldReason),
    /// The iteration cap in [`TurnOptions`] was reached.
    MaxTurns,
    /// The model hit its token ceiling.
    MaxTokens,
    /// The caller cancelled.
    Cancelled,
    /// A hook blocked the turn.
    BlockedByHook,
}

/// The model loop, as a host sees it.
///
/// Owns streaming, tool scheduling, prompt assembly, recovery, cancellation and
/// the compaction calls a turn may trigger. The implementation lives in the
/// extensions crate; this trait is what lets a view drive it without linking
/// against the model client.
#[async_trait::async_trait]
pub trait AgentService: Send + Sync {
    /// Run one turn to a stop.
    ///
    /// # Errors
    ///
    /// Returns an error only for failures the caller cannot act on (the model
    /// endpoint refused, the store failed). A turn that was cancelled or blocked
    /// is a successful call with a non-`Completed` [`TurnOutcome::stop`], because
    /// those are outcomes the caller asked for, not faults.
    async fn run(&self, request: TurnRequest, options: TurnOptions) -> anyhow::Result<TurnOutcome>;

    /// Ask the running turn to stop at its next checkpoint.
    ///
    /// Cooperative: it returns immediately, and the turn reports
    /// [`TurnStop::Cancelled`].
    fn cancel(&self);

    /// Compact the conversation now.
    ///
    /// # Errors
    ///
    /// Returns an error when there is nothing to compact or the summary call
    /// fails.
    async fn compact(&self) -> anyhow::Result<()>;
}

/// One user turn, as a host sees it.
///
/// Sits above [`AgentService`]: it assembles the message, invokes the agent,
/// continues past a Stop hook and publishes the completion. It exists separately
/// because a Goal activation drives several agent turns and needs the turn
/// boundaries to be its own.
#[async_trait::async_trait]
pub trait ChatService: Send + Sync {
    /// Submit a user turn.
    ///
    /// # Errors
    ///
    /// Returns an error when the turn could not be started at all; the turn's own
    /// failures reach the caller through the event stream, which is where the
    /// user sees them.
    async fn submit(&self, message: TurnRequest) -> anyhow::Result<()>;

    /// Ask the current turn to stop.
    fn cancel(&self);

    /// Compact the conversation now.
    ///
    /// # Errors
    ///
    /// Returns an error when the compaction could not run.
    async fn compact(&self) -> anyhow::Result<()>;
}

/// What a finished turn produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnOutcome {
    /// Why it stopped.
    pub stop: TurnStop,
    /// The assistant's final text, when there was one.
    ///
    /// `None` rather than an empty string: "the model said nothing" and "the
    /// model said an empty thing" are different, and a caller that prints the
    /// difference needs to be able to see it.
    pub final_text: Option<String>,
}

impl TurnOutcome {
    /// Whether the turn ended without the caller cancelling or a hook blocking.
    pub fn is_finished(&self) -> bool {
        matches!(self.stop, TurnStop::Completed)
    }
}

#[cfg(test)]
mod turn_tests {
    use super::*;

    /// An empty request is not a turn.
    ///
    /// Whitespace alone would otherwise append an empty user message and spend a
    /// model call on it.
    #[test]
    fn an_empty_request_is_recognised() {
        assert!(TurnRequest::default().is_empty());
        assert!(TurnRequest::text("   \n ").is_empty());
        assert!(!TurnRequest::text("hello").is_empty());
    }

    /// An image makes a request non-empty even with no text.
    ///
    /// A user can paste a screenshot and press Enter; refusing that as "empty"
    /// would drop the one thing they attached.
    #[test]
    fn an_image_alone_is_a_request() {
        let request = TurnRequest {
            input: String::new(),
            images: vec![TurnImage {
                media_type: "image/png".to_string(),
                data: "AAAA".to_string(),
            }],
        };
        assert!(!request.is_empty());
    }

    /// Only a completed turn counts as finished.
    ///
    /// A Goal loop branches on this, so a yielded or cancelled turn must not
    /// read as success.
    #[test]
    fn only_completion_is_finished() {
        let outcome = |stop| TurnOutcome {
            stop,
            final_text: None,
        };
        assert!(outcome(TurnStop::Completed).is_finished());
        for stop in [
            TurnStop::Yielded(crate::tool::result::YieldReason::NeedsUser),
            TurnStop::MaxTurns,
            TurnStop::MaxTokens,
            TurnStop::Cancelled,
            TurnStop::BlockedByHook,
        ] {
            assert!(!outcome(stop).is_finished(), "{stop:?} is not finished");
        }
    }

    /// A cancelled turn is a successful call with a cancelled stop.
    ///
    /// This is the contract a Goal loop depends on: cancelling is something the
    /// caller asked for, so it must not arrive as an `Err` and force every
    /// caller to tell "cancelled" from "broken" by inspecting a message.
    #[tokio::test]
    async fn cancellation_is_an_outcome_not_an_error() {
        struct Cancelling;

        #[async_trait::async_trait]
        impl AgentService for Cancelling {
            async fn run(
                &self,
                _request: TurnRequest,
                _options: TurnOptions,
            ) -> anyhow::Result<TurnOutcome> {
                Ok(TurnOutcome {
                    stop: TurnStop::Cancelled,
                    final_text: None,
                })
            }

            fn cancel(&self) {}

            async fn compact(&self) -> anyhow::Result<()> {
                Ok(())
            }
        }

        let outcome = Cancelling
            .run(TurnRequest::text("hi"), TurnOptions::default())
            .await
            .expect("a cancelled turn is not an error");
        assert_eq!(outcome.stop, TurnStop::Cancelled);
        assert!(!outcome.is_finished());
    }

    /// `final_text` distinguishes "said nothing" from "said an empty thing".
    #[test]
    fn absent_text_is_not_empty_text() {
        let silent = TurnOutcome {
            stop: TurnStop::Completed,
            final_text: None,
        };
        let empty = TurnOutcome {
            stop: TurnStop::Completed,
            final_text: Some(String::new()),
        };
        assert_ne!(silent, empty);
    }
}

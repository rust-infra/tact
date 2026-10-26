//! The agent's session settings, as a service a command can reach.
//!
//! The three settings a session can change — model, thinking budget, reasoning
//! effort — are fields on the running agent, and the setters take `&mut self`. A
//! command has no agent, so the service records a *validated change* and the host
//! applies it, the same seam [`crate::goal::service::GoalActivator`] uses for Goal
//! activations.
//!
//! Validation lives here rather than in the command: an unparseable effort must be
//! a usage error the user can see, not a log line. The driver used to print one to
//! stderr and carry on.

use tact_protocol::ReasoningEffort;

/// One session setting to change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsChange {
    /// The model this session calls.
    Model(String),
    /// The thinking budget, in tokens.
    ThinkingBudget(usize),
    /// The reasoning effort, or `None` to clear it.
    ReasoningEffort(Option<ReasoningEffort>),
}

/// What a settings command needs from the runtime.
pub trait SettingsService: Send + Sync {
    /// Record a change for the host to apply.
    ///
    /// # Errors
    ///
    /// Returns an error when the host is gone, which means the session is shutting
    /// down — the change cannot land, and saying so is better than dropping it.
    fn apply(&self, change: SettingsChange) -> anyhow::Result<()>;
}

/// A service that records changes for the host to apply.
///
/// Applying needs `&mut Agent`, and a command runs while its caller holds one: a
/// service that reached for the agent here would deadlock on its own lock. The
/// change is queued instead, and the caller drains the queue once its borrow is
/// free — the same shape as the Goal activation queue.
pub struct QueueingSettings {
    queued: tokio::sync::mpsc::UnboundedSender<SettingsChange>,
}

impl QueueingSettings {
    /// A queue draining into `queued`.
    pub fn new(queued: tokio::sync::mpsc::UnboundedSender<SettingsChange>) -> Self {
        Self { queued }
    }
}

impl SettingsService for QueueingSettings {
    fn apply(&self, change: SettingsChange) -> anyhow::Result<()> {
        self.queued
            .send(change)
            .map_err(|_| anyhow::anyhow!("the settings queue is closed"))
    }
}

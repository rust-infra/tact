//! Run identity and lifecycle values.

/// Identifies one submitted task across model turns, tool waves and host updates.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RunId(pub u64);

/// Terminal outcome of a submitted run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunOutcome {
    Completed,
    Cancelled,
}

/// Small progress snapshot; conversation and provider state remain elsewhere.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RunProgress {
    pub id: RunId,
    pub model_turns: u32,
}

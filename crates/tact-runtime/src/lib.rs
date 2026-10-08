//! Run lifecycle primitives independent of the legacy `tact::Agent` facade.

use tact_contracts::run::{RunId, RunOutcome};

pub mod capability;
pub mod tool_batch;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunState {
    Running,
    Completed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunError {
    AlreadyTerminal,
}

pub struct RunMachine {
    id: RunId,
    state: RunState,
    model_turns: u32,
}

impl RunMachine {
    pub fn new(id: RunId) -> Self {
        Self {
            id,
            state: RunState::Running,
            model_turns: 0,
        }
    }

    pub fn id(&self) -> RunId {
        self.id
    }

    pub fn state(&self) -> RunState {
        self.state
    }

    pub fn model_turns(&self) -> u32 {
        self.model_turns
    }

    pub fn record_model_turn(&mut self) -> bool {
        if self.state != RunState::Running {
            return false;
        }
        self.model_turns = self.model_turns.saturating_add(1);
        true
    }

    pub fn complete(&mut self) -> Result<RunOutcome, RunError> {
        if self.state != RunState::Running {
            return Err(RunError::AlreadyTerminal);
        }
        self.state = RunState::Completed;
        Ok(RunOutcome::Completed)
    }

    pub fn cancel(&mut self) -> Result<RunOutcome, RunError> {
        if self.state != RunState::Running {
            return Err(RunError::AlreadyTerminal);
        }
        self.state = RunState::Cancelled;
        Ok(RunOutcome::Cancelled)
    }
}

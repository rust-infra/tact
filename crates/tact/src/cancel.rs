//! Host-owned cancellation lifecycle.
//!
//! The kernel only receives a [`CancelToken`]. This module owns the mutable
//! session state that decides when a cancelled token is retired and when a new
//! user turn starts with a fresh token.

use std::{
    fmt,
    sync::{Arc, Mutex},
};

use tact_agent_api::CancelToken;

#[derive(Debug)]
struct CancelState {
    generation: u64,
    session_token: CancelToken,
    active: usize,
}

/// Cancellation authority for one logical agent session.
///
/// `cancel()` invalidates the current session token, including every child
/// token handed to an activation or background task. A new user turn calls
/// [`Self::begin_turn`] while the agent is idle; only then is the cancelled
/// token retired.
#[derive(Clone)]
pub struct CancelController {
    state: Arc<Mutex<CancelState>>,
}

impl fmt::Debug for CancelController {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.lock();
        formatter
            .debug_struct("CancelController")
            .field("generation", &state.generation)
            .field("cancelled", &state.session_token.is_cancelled())
            .field("active", &state.active)
            .finish()
    }
}

impl Default for CancelController {
    fn default() -> Self {
        Self::new()
    }
}

impl CancelController {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(CancelState {
                generation: 0,
                session_token: CancelToken::new(),
                active: 0,
            })),
        }
    }

    /// Starts a user-committed turn.
    ///
    /// A cancelled session token is replaced only while no activation is
    /// running. This prevents a late `begin_turn` from reviving an activation
    /// that was already cancelled.
    pub fn begin_turn(&self) {
        let mut state = self.lock();
        if state.active == 0 && state.session_token.is_cancelled() {
            state.generation = state.generation.wrapping_add(1);
            state.session_token = CancelToken::new();
        }
    }

    /// Registers one bounded activation and returns its RAII guard.
    ///
    /// The guard's token is a child of the session token, so it observes both
    /// activation-local cancellation and [`Self::cancel`].
    pub fn begin_activation(&self) -> ActivationCancel {
        let mut state = self.lock();
        state.active += 1;
        ActivationCancel {
            controller: self.clone(),
            generation: state.generation,
            token: state.session_token.child_token(),
        }
    }

    /// Cancels the current session token and every token derived from it.
    pub fn cancel(&self) {
        self.lock().session_token.cancel();
    }

    /// Returns a token for work that must outlive one activation, such as a
    /// background process.
    pub fn session_token(&self) -> CancelToken {
        self.lock().session_token.clone()
    }

    pub fn is_cancelled(&self) -> bool {
        self.lock().session_token.is_cancelled()
    }

    #[cfg(test)]
    fn active_for_test(&self) -> usize {
        self.lock().active
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CancelState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }
}

/// RAII registration for one activation.
///
/// Dropping the guard makes the agent idle again. It deliberately does not
/// cancel the activation token: cancellation is an explicit session action,
/// not a side effect of a run finishing.
pub struct ActivationCancel {
    controller: CancelController,
    generation: u64,
    token: CancelToken,
}

impl fmt::Debug for ActivationCancel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActivationCancel")
            .field("generation", &self.generation)
            .field("cancelled", &self.token.is_cancelled())
            .finish()
    }
}

impl ActivationCancel {
    pub fn token(&self) -> CancelToken {
        self.token.clone()
    }

    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }
}

impl Drop for ActivationCancel {
    fn drop(&mut self) {
        let mut state = self.controller.lock();
        state.active = state.active.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;

    #[test]
    fn cancelled_turn_resets_only_while_idle() {
        let controller = CancelController::new();
        let first = controller.session_token();
        controller.cancel();
        assert!(first.is_cancelled());

        let activation = controller.begin_activation();
        controller.begin_turn();
        assert!(controller.is_cancelled(), "active work must stay cancelled");
        drop(activation);

        controller.begin_turn();
        let second = controller.session_token();
        assert!(!controller.is_cancelled());
        assert!(!second.is_cancelled());
        assert!(first.is_cancelled(), "old tokens must never be revived");
        assert_eq!(controller.active_for_test(), 0);
    }

    #[test]
    fn background_token_survives_activation_and_observes_session_cancel() {
        let controller = CancelController::new();
        let background = controller.session_token();
        let activation = controller.begin_activation();
        let activation_token = activation.token();
        drop(activation);

        assert!(!background.is_cancelled());
        controller.cancel();
        assert!(background.is_cancelled());
        assert!(activation_token.is_cancelled());
    }

    #[test]
    fn concurrent_cancel_and_begin_turn_stay_coherent() {
        let controller = Arc::new(CancelController::new());
        let barrier = Arc::new(Barrier::new(9));
        let mut workers = Vec::new();

        for index in 0..8 {
            let controller = controller.clone();
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                if index % 2 == 0 {
                    controller.cancel();
                } else {
                    controller.begin_turn();
                }
            }));
        }

        barrier.wait();
        for worker in workers {
            worker.join().expect("cancel worker panicked");
        }

        let current = controller.session_token();
        assert_eq!(controller.is_cancelled(), current.is_cancelled());
        controller.begin_turn();
        assert!(!controller.is_cancelled());
    }
}

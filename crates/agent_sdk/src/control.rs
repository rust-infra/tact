use std::sync::{Arc, Mutex, PoisonError};

use tact_agent_api::CancelToken;

struct ActiveActivation {
    generation: u64,
    token: CancelToken,
}

#[derive(Default)]
struct ControlState {
    generation: u64,
    active: Option<ActiveActivation>,
}

/// Host-facing cancellation handle for one session.
#[derive(Clone)]
pub struct SessionControl {
    shared: Arc<Mutex<ControlState>>,
}

impl SessionControl {
    pub(crate) fn new() -> Self {
        Self {
            shared: Arc::new(Mutex::new(ControlState::default())),
        }
    }

    pub(crate) fn begin_activation(&self) -> (CancelToken, ActivationGuard) {
        let mut state = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        state.generation = state.generation.saturating_add(1);
        let generation = state.generation;
        let token = CancelToken::new();
        state.active = Some(ActiveActivation {
            generation,
            token: token.clone(),
        });
        (
            token,
            ActivationGuard {
                shared: self.shared.clone(),
                generation,
            },
        )
    }

    pub fn cancel(&self) {
        let token = {
            let state = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
            state.active.as_ref().map(|active| active.token.clone())
        };
        if let Some(token) = token {
            token.cancel();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.shared
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .active
            .as_ref()
            .is_some_and(|active| active.token.is_cancelled())
    }
}

pub(crate) struct ActivationGuard {
    shared: Arc<Mutex<ControlState>>,
    generation: u64,
}

impl Drop for ActivationGuard {
    fn drop(&mut self) {
        let mut state = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if state.active.as_ref().map(|active| active.generation) == Some(self.generation) {
            state.active = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        panic::{AssertUnwindSafe, catch_unwind},
        sync::Arc,
        task::{Context, Poll, Wake, Waker},
    };

    use super::*;

    #[test]
    fn session_control_cancels_only_the_active_run() {
        let control = SessionControl::new();
        assert!(!control.is_cancelled());
        control.cancel();
        assert!(!control.is_cancelled());

        let (token, guard) = control.begin_activation();
        assert!(!token.is_cancelled());
        control.cancel();
        assert!(token.is_cancelled());
        assert!(control.is_cancelled());
        control.cancel();
        assert!(control.is_cancelled());
        drop(guard);
        assert!(!control.is_cancelled());
    }

    #[test]
    fn next_run_gets_a_fresh_cancel_token() {
        let control = SessionControl::new();
        let (first, first_guard) = control.begin_activation();
        control.cancel();
        assert!(first.is_cancelled());
        drop(first_guard);

        let (second, second_guard) = control.begin_activation();
        assert!(!second.is_cancelled());
        assert!(!control.is_cancelled());
        drop(second_guard);
    }

    #[test]
    fn idle_is_not_cancelled_and_repeated_cancel_is_idempotent() {
        let control = SessionControl::new();
        control.cancel();
        control.cancel();
        assert!(!control.is_cancelled());

        let (token, guard) = control.begin_activation();
        control.cancel();
        control.cancel();
        assert!(token.is_cancelled());
        assert!(control.is_cancelled());
        drop(guard);

        control.cancel();
        assert!(!control.is_cancelled());
    }

    #[test]
    fn activation_guard_preserves_newer_generation() {
        let control = SessionControl::new();
        let (_, old_guard) = control.begin_activation();
        let (new_token, new_guard) = control.begin_activation();
        drop(old_guard);
        assert!(!new_token.is_cancelled());
        control.cancel();
        assert!(new_token.is_cancelled());
        drop(new_guard);
    }

    #[test]
    fn panic_between_activations_does_not_poison_control() {
        let control = SessionControl::new();
        let (_, guard) = control.begin_activation();
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _guard = guard;
            panic!("test panic");
        }));
        assert!(result.is_err());
        assert!(!control.is_cancelled());

        let (token, guard) = control.begin_activation();
        control.cancel();
        assert!(token.is_cancelled());
        drop(guard);
    }

    #[test]
    fn cancel_wakes_without_holding_the_control_lock() {
        struct ReentrantWake(SessionControl);

        impl Wake for ReentrantWake {
            fn wake(self: Arc<Self>) {
                self.0.cancel();
            }

            fn wake_by_ref(self: &Arc<Self>) {
                self.0.cancel();
            }
        }

        let control = SessionControl::new();
        let (token, guard) = control.begin_activation();
        let waker = Waker::from(Arc::new(ReentrantWake(control.clone())));
        let mut context = Context::from_waker(&waker);
        let mut cancelled = Box::pin(token.cancelled());

        assert_eq!(cancelled.as_mut().poll(&mut context), Poll::Pending);
        control.cancel();
        assert_eq!(cancelled.as_mut().poll(&mut context), Poll::Ready(()));
        drop(guard);
    }
}

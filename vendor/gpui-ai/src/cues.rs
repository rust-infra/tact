//! Interaction cues: the moments an application may want to mark with a
//! sound, haptic, or notification.
//!
//! gpui-ai does not play audio. Components emit a [`Cue`] at the moments
//! that matter (a reply arriving, a response settling, text copied, a
//! prompt submitted, …) and an application can observe them in one place:
//!
//! ```no_run
//! # fn example(cx: &mut gpui::App, play: fn(&str)) {
//! use gpui_ai::cues::{self, Cue};
//!
//! let _cues = cues::observe(cx, move |cue, _cx| match cue {
//!     Cue::ResponseSettled { .. } => play("chime"),
//!     Cue::Copied => play("tick"),
//!     _ => {}
//! });
//! // Keep `_cues` alive for as long as the sounds should play.
//! # }
//! ```
//!
//! Cues are hints, never state: every cue corresponds to a typed component
//! event or snapshot transition that the application already receives.

use gpui::{App, Global, SharedString};
use std::{cell::Cell, rc::Rc};

/// A moment worth a cue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cue {
    /// A new message entered a conversation.
    MessageArrived {
        /// Stable message identifier.
        message_id: SharedString,
    },
    /// A streaming response finished (successfully or not).
    ResponseSettled {
        /// Stable message identifier.
        message_id: SharedString,
        /// Whether the response completed rather than failed.
        succeeded: bool,
    },
    /// Text was copied to the clipboard.
    Copied,
    /// The user submitted a prompt.
    Submitted,
    /// The user cancelled running work.
    Cancelled,
    /// The user chose a starter suggestion.
    SuggestionSelected,
    /// The user switched conversations.
    ThreadSelected,
    /// A tool call or approval gate was decided.
    Decided {
        /// Whether the decision was an approval.
        approved: bool,
    },
}

type Observer = Rc<dyn Fn(&Cue, &mut App)>;

struct ObserverRegistration {
    active: Rc<Cell<bool>>,
    observer: Observer,
}

#[derive(Default)]
struct CueHub {
    observers: Vec<ObserverRegistration>,
}

impl Global for CueHub {}

/// Keeps a cue observer registered; dropping it unregisters the observer.
#[must_use = "dropping the subscription stops cue delivery"]
pub struct CueSubscription {
    active: Option<Rc<Cell<bool>>>,
}

impl CueSubscription {
    /// Detaches the observer for the rest of the application's lifetime.
    pub fn detach(mut self) {
        self.active.take();
    }
}

impl Drop for CueSubscription {
    fn drop(&mut self) {
        // The registration owns another reference, so this signal remains
        // valid until that app next emits and prunes the observer.
        if let Some(active) = self.active.as_ref() {
            active.set(false);
        }
    }
}

/// Registers an observer for every cue emitted by gpui-ai components.
pub fn observe(cx: &mut App, observer: impl Fn(&Cue, &mut App) + 'static) -> CueSubscription {
    let active = Rc::new(Cell::new(true));
    cx.default_global::<CueHub>()
        .observers
        .push(ObserverRegistration {
            active: active.clone(),
            observer: Rc::new(observer),
        });
    CueSubscription {
        active: Some(active),
    }
}

/// Emits a cue to every live observer. Components call this at the moments
/// described on [`Cue`]; applications may also emit their own.
pub fn emit(cx: &mut App, cue: Cue) {
    if !cx.has_global::<CueHub>() {
        return;
    }
    let observers: Vec<Observer> = {
        let hub = cx.global_mut::<CueHub>();
        hub.observers
            .retain(|registration| registration.active.get());
        hub.observers
            .iter()
            .map(|registration| registration.observer.clone())
            .collect()
    };
    // Observers run outside the hub borrow: one of them may register another
    // observer or drop a subscription.
    for observer in observers {
        observer(&cue, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use std::{cell::RefCell, rc::Rc};

    #[gpui::test]
    fn observers_receive_cues_until_dropped(cx: &mut TestAppContext) {
        let received = Rc::new(RefCell::new(Vec::new()));
        let subscription = cx.update(|cx| {
            let received = received.clone();
            observe(cx, move |cue, _| received.borrow_mut().push(cue.clone()))
        });
        cx.update(|cx| emit(cx, Cue::Copied));
        assert_eq!(received.borrow().as_slice(), &[Cue::Copied]);

        drop(subscription);
        cx.update(|cx| emit(cx, Cue::Submitted));
        assert_eq!(received.borrow().as_slice(), &[Cue::Copied]);
    }

    #[gpui::test]
    fn emitting_without_observers_is_a_no_op(cx: &mut TestAppContext) {
        cx.update(|cx| emit(cx, Cue::Cancelled));
    }
}

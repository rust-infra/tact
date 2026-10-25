//! The kernel `InteractionBroker` port, backed by `UiResponder` + `UpdateSink`.
//!
//! No new UI protocol: a `Select` becomes the same `RequestSelect` the legacy
//! permission popup already emitted, so the TUI reconciliation, the headless
//! fallback and the "answer the oldest pending request" behaviour are
//! unchanged.

use async_trait::async_trait;
use tact_kernel::tool::context::{
    InteractionBroker, InteractionRequest, InteractionResponse, PendingInteraction,
};
use tact_protocol::UpdateSink;

use crate::ui_responder::{PendingUiRequestKind, UiResponder};

/// Asks the attached UI, or answers headlessly when nobody can.
pub struct UiInteractionBroker {
    responder: UiResponder,
    events: UpdateSink,
    from_subagent: bool,
}

impl UiInteractionBroker {
    pub fn new(responder: UiResponder, events: UpdateSink, from_subagent: bool) -> Self {
        Self {
            responder,
            events,
            from_subagent,
        }
    }

    /// Build one from a tool context.
    pub fn from_context(context: &crate::tool::ToolContext) -> Self {
        Self::new(
            context.ui_responder.clone(),
            context.events.clone(),
            context.is_subagent,
        )
    }
}

#[async_trait]
impl InteractionBroker for UiInteractionBroker {
    async fn request(&self, request: InteractionRequest) -> anyhow::Result<InteractionResponse> {
        match request {
            InteractionRequest::Select { prompt, options } => {
                let Some(tx) = self.events.ui() else {
                    // No UI: nothing was shown, so nothing was approved. The
                    // caller turns this into a denial; guessing on the user's
                    // behalf is what the legacy `ask_user` fallback did, and it
                    // only ever ran for risks the policy had already allowed.
                    return Ok(InteractionResponse::Cancelled);
                };
                let selection = match self
                    .responder
                    .request_select(tx, prompt, options.clone(), false, self.from_subagent)
                    .await
                {
                    Ok(selection) => selection,
                    // A closed channel means the prompt is gone — withdrawn, or
                    // the UI went away. That is a cancellation, not an error: the
                    // caller turns it into a denial, and surfacing it as `Err`
                    // would report a refusal as a tool failure.
                    Err(_) => return Ok(InteractionResponse::Cancelled),
                };
                Ok(match selection {
                    Some(index) => match options.get(index) {
                        Some(label) => InteractionResponse::Selected(vec![label.clone()]),
                        None => InteractionResponse::Cancelled,
                    },
                    None => InteractionResponse::Cancelled,
                })
            }
            // The permission flow uses `Select` exclusively, and `ask_user` — the
            // only tool that confirms or asks for text — talks to the responder
            // directly rather than through this port. So no renderer exists for
            // these two, and silently approving one would be worse than refusing
            // it. The alternative (answering `Approved`) would let a port nobody
            // is listening on grant consent.
            InteractionRequest::Confirm { .. } => Ok(InteractionResponse::Cancelled),
            InteractionRequest::Input { .. } => Ok(InteractionResponse::Cancelled),
        }
    }

    /// The prompts a view should be showing right now.
    ///
    /// Read through the responder rather than mirrored here: it already owns the
    /// authoritative pending set, and a second copy would be able to disagree
    /// with the waiters it describes.
    fn pending(&self) -> Vec<PendingInteraction> {
        self.responder
            .snapshot()
            .into_iter()
            .map(|pending| PendingInteraction {
                request_id: pending.request_id,
                request: match pending.kind {
                    PendingUiRequestKind::Select => InteractionRequest::Select {
                        prompt: pending.prompt,
                        options: pending.options,
                    },
                    // The kernel port has no multi-select request: the only
                    // multi-select the TUI shows is a permission prompt built as
                    // a `Select`. Reporting it as one keeps the popup renderable
                    // instead of dropping it from the snapshot, which would close
                    // a prompt that is still waiting.
                    PendingUiRequestKind::MultiSelect => InteractionRequest::Select {
                        prompt: pending.prompt,
                        options: pending.options,
                    },
                },
                from_subagent: pending.from_subagent,
            })
            .collect()
    }

    fn withdraw(&self, request_id: u64) -> bool {
        self.responder.withdraw(request_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tact_protocol::AgentUpdate;
    use tokio::sync::mpsc::unbounded_channel;

    struct Fixture {
        broker: UiInteractionBroker,
        responder: UiResponder,
        rx: tokio::sync::mpsc::UnboundedReceiver<AgentUpdate>,
    }

    fn fixture(ui: bool) -> Fixture {
        let responder = UiResponder::new();
        let (tx, rx) = unbounded_channel();
        let events = if ui {
            UpdateSink::for_ui(tx)
        } else {
            let _ = tx;
            UpdateSink::default()
        };
        Fixture {
            broker: UiInteractionBroker::new(responder.clone(), events, false),
            responder,
            rx,
        }
    }

    #[tokio::test]
    async fn select_is_answered_through_the_responder() {
        let mut fixture = fixture(true);
        let responder = fixture.responder.clone();
        let broker = fixture.broker;
        let ask = tokio::spawn(async move {
            broker
                .request(InteractionRequest::Select {
                    prompt: "Allow?".into(),
                    options: vec!["Allow once".into(), "Deny".into()],
                })
                .await
        });

        // The request is registered synchronously in the spawned task; wait for
        // the pending entry rather than sleeping a fixed time.
        let request_id = loop {
            if let Some(pending) = responder.snapshot().into_iter().next() {
                break pending.request_id;
            }
            tokio::task::yield_now().await;
        };
        assert!(responder.respond(tact_protocol::UiResponse::Select {
            request_id,
            choice: Some(0),
        }));
        let answer = ask.await.unwrap().unwrap();
        assert_eq!(
            answer,
            InteractionResponse::Selected(vec!["Allow once".to_string()])
        );
        let _ = &mut fixture.rx;
    }

    /// The broker reports the prompts a view should be showing.
    ///
    /// A view reconciles from this snapshot, so it has to describe the *same*
    /// waiters the responder holds: a prompt missing here would be closed on
    /// screen while its tool still waits, and a stale one would stay on screen
    /// after its waiter gave up.
    #[tokio::test]
    async fn pending_mirrors_the_responder() {
        let mut fixture = fixture(true);
        let responder = fixture.responder.clone();
        let broker = fixture.broker;
        assert!(
            InteractionBroker::pending(&broker).is_empty(),
            "nothing is waiting yet"
        );

        let ask = tokio::spawn(async move {
            broker
                .request(InteractionRequest::Select {
                    prompt: "Allow?".into(),
                    options: vec!["Allow once".into(), "Deny".into()],
                })
                .await
        });
        let request_id = loop {
            if let Some(pending) = responder.snapshot().into_iter().next() {
                break pending.request_id;
            }
            tokio::task::yield_now().await;
        };

        // Rebuild a broker over the same responder: the snapshot is shared
        // state, not per-call state.
        let observer = UiInteractionBroker::new(responder.clone(), UpdateSink::default(), false);
        let pending = InteractionBroker::pending(&observer);
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert_eq!(pending[0].request_id, request_id);
        assert_eq!(
            pending[0].request,
            InteractionRequest::Select {
                prompt: "Allow?".into(),
                options: vec!["Allow once".into(), "Deny".into()],
            }
        );

        // Withdrawing ends the wait as a cancellation and empties the snapshot.
        assert!(InteractionBroker::withdraw(&observer, request_id));
        assert!(InteractionBroker::pending(&observer).is_empty());
        assert_eq!(ask.await.unwrap().unwrap(), InteractionResponse::Cancelled);
        let _ = &mut fixture.rx;
    }

    #[tokio::test]
    async fn no_ui_cancels_instead_of_guessing() {
        let fixture = fixture(false);
        let answer = fixture
            .broker
            .request(InteractionRequest::Select {
                prompt: "Allow?".into(),
                options: vec!["Allow once".into(), "Deny".into()],
            })
            .await
            .unwrap();
        assert_eq!(answer, InteractionResponse::Cancelled);
    }

    #[tokio::test]
    async fn confirm_has_no_renderer_and_cancels() {
        let fixture = fixture(true);
        let answer = fixture
            .broker
            .request(InteractionRequest::Confirm {
                prompt: "sure?".into(),
            })
            .await
            .unwrap();
        assert_eq!(answer, InteractionResponse::Cancelled);
    }

    /// `Input` has no renderer either, for the same reason as `Confirm`: the one
    /// tool that asks for text (`ask_user`) talks to the responder directly.
    #[tokio::test]
    async fn input_has_no_renderer_and_cancels() {
        let fixture = fixture(true);
        let answer = fixture
            .broker
            .request(InteractionRequest::Input {
                prompt: "name?".into(),
            })
            .await
            .unwrap();
        assert_eq!(answer, InteractionResponse::Cancelled);
    }

    #[tokio::test]
    async fn out_of_range_selection_is_cancelled() {
        let fixture = fixture(true);
        let responder = fixture.responder.clone();
        let broker = fixture.broker;
        let ask = tokio::spawn(async move {
            broker
                .request(InteractionRequest::Select {
                    prompt: "Allow?".into(),
                    options: vec!["only".into()],
                })
                .await
        });
        let request_id = loop {
            if let Some(pending) = responder.snapshot().into_iter().next() {
                break pending.request_id;
            }
            tokio::task::yield_now().await;
        };
        assert!(responder.respond(tact_protocol::UiResponse::Select {
            request_id,
            choice: Some(9),
        }));
        assert_eq!(ask.await.unwrap().unwrap(), InteractionResponse::Cancelled);
        let _ = json!({});
    }
}

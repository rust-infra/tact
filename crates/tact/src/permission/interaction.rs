//! The kernel `InteractionBroker` port, backed by `UiResponder` + `UpdateSink`.
//!
//! No new UI protocol: a `Select` becomes the same `RequestSelect` the legacy
//! permission popup already emitted, so the TUI reconciliation, the headless
//! fallback and the "answer the oldest pending request" behaviour are
//! unchanged.

use async_trait::async_trait;
use tact_kernel::tool::context::{InteractionBroker, InteractionRequest, InteractionResponse};
use tact_protocol::UpdateSink;

use crate::ui_responder::UiResponder;

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
                let selection = self
                    .responder
                    .request_select(tx, prompt, options.clone(), false, self.from_subagent)
                    .await
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
                Ok(match selection {
                    Some(index) => match options.get(index) {
                        Some(label) => InteractionResponse::Selected(vec![label.clone()]),
                        None => InteractionResponse::Cancelled,
                    },
                    None => InteractionResponse::Cancelled,
                })
            }
            InteractionRequest::Confirm { .. } => {
                // The permission flow uses `Select` exclusively; a Confirm has no
                // renderer, and silently approving one would be worse than
                // refusing it.
                Ok(InteractionResponse::Cancelled)
            }
            InteractionRequest::Input { .. } => Ok(InteractionResponse::Cancelled),
        }
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

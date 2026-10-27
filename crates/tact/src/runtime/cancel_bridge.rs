//! Bridge Tact's public cancel flag into one kernel activation token.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use tact_agent_api::CancelToken;

/// Mirrors an `AtomicBool` cancel flag into a one-way `CancelToken`.
///
/// The watcher is scoped to the activation and aborted on drop. A flag that was
/// already set cancels synchronously, so no model or tool call can start first.
pub(crate) struct CancelBridge {
    token: CancelToken,
    watcher: tokio::task::JoinHandle<()>,
}

impl CancelBridge {
    pub(crate) fn new(flag: Arc<AtomicBool>) -> Self {
        let token = CancelToken::from_flag(flag.clone());
        if flag.load(Ordering::Relaxed) {
            token.cancel();
        }
        let watched_token = token.clone();
        let watcher = tokio::spawn(async move {
            if watched_token.is_cancelled() {
                return;
            }
            loop {
                if flag.load(Ordering::Relaxed) {
                    watched_token.cancel();
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        Self { token, watcher }
    }

    pub(crate) fn token(&self) -> CancelToken {
        self.token.clone()
    }
}

impl Drop for CancelBridge {
    fn drop(&mut self) {
        self.watcher.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use tact_agent_api::{
        AgentEvent, AgentMessage, AgentRole, AgentState, MessageOrigin, ModelEvent, RunInput,
        RunStop, SessionRef, StopReason,
    };
    use tact_agent_core::test_support::{
        RecordingEventPort, ScriptedContext, ScriptedGate, ScriptedModel, ScriptedTools,
        text_response,
    };
    use tact_agent_core::{AgentKernel, KernelDeps};

    use super::*;

    #[tokio::test]
    async fn an_already_cancelled_flag_never_loses_the_race() {
        let flag = Arc::new(AtomicBool::new(true));
        let bridge = CancelBridge::new(flag);
        assert!(bridge.token().is_cancelled());
    }

    #[tokio::test]
    async fn later_flag_writes_reach_the_activation_token() {
        let flag = Arc::new(AtomicBool::new(false));
        let bridge = CancelBridge::new(flag.clone());
        let token = bridge.token();

        flag.store(true, Ordering::Relaxed);
        tokio::time::timeout(Duration::from_secs(1), token.cancelled())
            .await
            .expect("cancel watcher must observe the flag");
    }

    #[tokio::test]
    async fn public_cancel_flag_keeps_one_terminal_event_and_committed_state() {
        let flag = Arc::new(AtomicBool::new(false));
        let bridge = CancelBridge::new(flag.clone());
        let cancel = bridge.token();
        let response = text_response("assistant-1", "committed", StopReason::EndTurn);
        let model = ScriptedModel::new(vec![
            tact_agent_core::test_support::ModelScript::WaitForCancel(vec![Ok(
                ModelEvent::Finished(response),
            )]),
        ]);
        let events = Arc::new(RecordingEventPort::new());
        let kernel = AgentKernel::new(KernelDeps {
            model,
            tools: ScriptedTools::new(Vec::new()),
            gate: ScriptedGate::new(),
            context: ScriptedContext::new(),
            transcript: Arc::new(tact_agent_api::test_support::InMemoryTranscriptStore::new()),
            usage: Arc::new(tact_agent_api::test_support::RecordingUsageSink::new()),
            events: events.clone(),
        });

        let task = tokio::spawn(async move {
            let mut state = AgentState {
                initialized: true,
                ..AgentState::default()
            };
            let outcome = kernel
                .run(
                    &mut state,
                    RunInput {
                        session: SessionRef::new("cancel-bridge"),
                        inputs: vec![AgentMessage::text(
                            AgentRole::User,
                            MessageOrigin::User,
                            "wait",
                        )],
                        max_total_turns: None,
                    },
                    cancel,
                )
                .await
                .expect("cancelled run still returns a typed outcome");
            (outcome, state)
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        flag.store(true, Ordering::Relaxed);
        let (outcome, state) = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("cancel must finish within the bounded test timeout")
            .expect("kernel task");

        assert_eq!(outcome.stop, RunStop::Cancelled);
        assert_eq!(state.transcript.len(), 2);
        assert_eq!(state.transcript[1].origin, MessageOrigin::Assistant);
        let terminal = events
            .events()
            .into_iter()
            .filter(|event| {
                matches!(
                    event,
                    AgentEvent::RunFinished { .. } | AgentEvent::RunFailed { .. }
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(terminal.len(), 1);
        assert!(matches!(
            &terminal[0],
            AgentEvent::RunFinished {
                outcome: tact_agent_api::RunOutcome {
                    stop: RunStop::Cancelled,
                    ..
                }
            }
        ));
    }
}

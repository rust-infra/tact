use std::sync::Arc;

use tact_agent_api::{
    AgentMessage, AgentRole, AgentState, CompactionOutcome, CompactionRequest, MessageOrigin,
    RunId, RunInput, RunOutcome, SessionRef,
};
use tact_agent_core::KernelError;

use crate::{AgentConfig, SessionControl};

/// One serialized agent session owning its caller-visible state.
pub struct AgentSession {
    pub(crate) kernel: Arc<tact_agent_core::AgentKernel>,
    session: SessionRef,
    state: AgentState,
    config: Arc<AgentConfig>,
    control: SessionControl,
}

impl std::fmt::Debug for AgentSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentSession")
            .field("session", &self.session)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl AgentSession {
    pub(crate) fn new(
        kernel: Arc<tact_agent_core::AgentKernel>,
        session_id: impl Into<String>,
        state: AgentState,
        config: Arc<AgentConfig>,
        control: SessionControl,
    ) -> Self {
        let session = SessionRef::new(session_id);
        let mut state = state;
        if state.session.is_none() {
            state.session = Some(session.clone());
        }
        Self {
            kernel,
            session,
            state,
            config,
            control,
        }
    }

    pub fn session_id(&self) -> &SessionRef {
        &self.session
    }

    pub fn state(&self) -> &AgentState {
        &self.state
    }

    pub fn transcript(&self) -> &[AgentMessage] {
        &self.state.transcript
    }

    pub fn into_state(self) -> AgentState {
        self.state
    }

    pub fn last_assistant_message(&self) -> Option<&AgentMessage> {
        self.state
            .transcript
            .iter()
            .rev()
            .find(|message| message.role == AgentRole::Assistant)
    }

    pub fn control(&self) -> SessionControl {
        self.control.clone()
    }

    pub async fn run_text(&mut self, text: impl Into<String>) -> Result<RunOutcome, KernelError> {
        self.run_message(AgentMessage::text(
            AgentRole::User,
            MessageOrigin::User,
            text,
        ))
        .await
    }

    pub async fn run_message(&mut self, message: AgentMessage) -> Result<RunOutcome, KernelError> {
        self.run_messages(vec![message]).await
    }

    pub async fn run_messages(
        &mut self,
        messages: Vec<AgentMessage>,
    ) -> Result<RunOutcome, KernelError> {
        let run_id = self.config.run_id_source.next_run_id();
        self.run_inner(run_id, messages, self.config.default_max_total_turns)
            .await
    }

    pub async fn run_with(
        &mut self,
        run_id: RunId,
        messages: Vec<AgentMessage>,
        max_total_turns: impl Into<Option<u32>>,
    ) -> Result<RunOutcome, KernelError> {
        self.run_inner(run_id, messages, max_total_turns.into())
            .await
    }

    pub async fn compact(
        &mut self,
        request: CompactionRequest,
    ) -> Result<CompactionOutcome, KernelError> {
        let run_id = self.config.run_id_source.next_run_id();
        let (cancel, guard) = self.control.begin_activation();
        let result = self
            .kernel
            .compact(&mut self.state, &self.session, run_id, request, cancel)
            .await;
        drop(guard);
        result
    }

    async fn run_inner(
        &mut self,
        run_id: RunId,
        messages: Vec<AgentMessage>,
        max_total_turns: Option<u32>,
    ) -> Result<RunOutcome, KernelError> {
        let (cancel, guard) = self.control.begin_activation();
        let input = RunInput {
            run_id: run_id.clone(),
            session: self.session.clone(),
            inputs: messages,
            max_total_turns,
        };

        #[cfg(feature = "telemetry")]
        let result = if self.config.telemetry.is_enabled() {
            let span = tracing::info_span!(
                "agent.run",
                session_id = %self.session.as_str(),
                run_id = %run_id.as_str(),
                duration_ms = tracing::field::Empty
            );
            crate::telemetry::timed(span, self.kernel.run(&mut self.state, input, cancel)).await
        } else {
            self.kernel.run(&mut self.state, input, cancel).await
        };

        #[cfg(not(feature = "telemetry"))]
        let result = self.kernel.run(&mut self.state, input, cancel).await;

        drop(guard);
        result
    }
}

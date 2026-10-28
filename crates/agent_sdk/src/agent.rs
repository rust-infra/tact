use std::sync::Arc;

use tact_agent_api::AgentState;

use crate::{AgentBuildError, AgentConfig, AgentSession, SessionControl};

/// Reusable composition root for independent sessions.
#[derive(Clone)]
pub struct Agent {
    pub(crate) kernel: Arc<tact_agent_core::AgentKernel>,
    pub(crate) config: Arc<AgentConfig>,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Agent").finish_non_exhaustive()
    }
}

impl Agent {
    pub fn session(&self, session_id: impl Into<String>) -> AgentSession {
        AgentSession::new(
            self.kernel.clone(),
            session_id.into(),
            AgentState::default(),
            self.config.clone(),
            SessionControl::new(),
        )
    }

    pub fn session_from_state(
        &self,
        session_id: impl Into<String>,
        state: AgentState,
    ) -> Result<AgentSession, AgentBuildError> {
        if !state.initialized {
            return Err(AgentBuildError::UninitializedState);
        }
        Ok(AgentSession::new(
            self.kernel.clone(),
            session_id.into(),
            state,
            self.config.clone(),
            SessionControl::new(),
        ))
    }
}

use tact_agent_api::{AgentEvent, EventPort};

/// Discard every runtime event.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopEventPort;

impl EventPort for NoopEventPort {
    fn emit(&self, _event: AgentEvent) {}
}

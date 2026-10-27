use crate::AgentEvent;

/// Ordered non-blocking kernel event sink.
pub trait EventPort: Send + Sync {
    fn emit(&self, event: AgentEvent);
}

/// Optional ordered observer invoked after the primary event port.
pub trait EventObserver: Send + Sync {
    fn observe(&self, event: &AgentEvent);
}

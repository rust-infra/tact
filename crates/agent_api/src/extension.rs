use crate::{AgentEvent, EventDelivery};

/// Ordered non-blocking kernel event sink.
///
/// Implementations must enqueue without waiting for a consumer. The delivery
/// policy is explicit so an adapter can bound or coalesce progress without
/// weakening terminal delivery.
pub trait EventPort: Send + Sync {
    fn emit(&self, event: AgentEvent);

    /// Enqueue one event with its delivery guarantee.
    ///
    /// The default preserves compatibility for simple ports. Adapters that
    /// maintain separate progress and terminal paths override this method.
    fn emit_with_delivery(&self, event: AgentEvent, _delivery: EventDelivery) {
        self.emit(event);
    }
}

/// Optional ordered observer invoked after the primary event port.
pub trait EventObserver: Send + Sync {
    fn observe(&self, event: &AgentEvent);

    /// Observe one event with its delivery guarantee.
    fn observe_with_delivery(&self, event: &AgentEvent, _delivery: EventDelivery) {
        self.observe(event);
    }
}

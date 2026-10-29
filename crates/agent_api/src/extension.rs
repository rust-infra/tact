use std::sync::Arc;

use crate::{AgentEvent, EventDelivery, RunContext};

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

    /// Enqueue one event with its activation identity.
    ///
    /// The default keeps existing ports source-compatible. Session-aware ports
    /// should override this method and route by `context.session`.
    fn emit_with_context(&self, _context: &RunContext, event: AgentEvent, delivery: EventDelivery) {
        self.emit_with_delivery(event, delivery);
    }

    /// Enqueue a shared event without forcing a deep clone.
    fn emit_shared(&self, event: Arc<AgentEvent>, delivery: EventDelivery) {
        self.emit_with_delivery((*event).clone(), delivery);
    }

    /// Enqueue a shared, context-tagged event without forcing a deep clone.
    fn emit_shared_with_context(
        &self,
        context: &RunContext,
        event: Arc<AgentEvent>,
        delivery: EventDelivery,
    ) {
        self.emit_with_context(context, (*event).clone(), delivery);
    }
}

/// Optional ordered observer invoked after the primary event port.
pub trait EventObserver: Send + Sync {
    fn observe(&self, event: &AgentEvent);

    /// Observe one event with its delivery guarantee.
    fn observe_with_delivery(&self, event: &AgentEvent, _delivery: EventDelivery) {
        self.observe(event);
    }

    fn observe_with_context(
        &self,
        _context: &RunContext,
        event: &AgentEvent,
        delivery: EventDelivery,
    ) {
        self.observe_with_delivery(event, delivery);
    }
}

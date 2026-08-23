//! Priority-ordered component registry (step 9 shell infrastructure).
//!
//! The host shell pushes components with a `priority()`; lower runs first
//! (the `LogCoordinator`-style shared surface is priority 0). Updates and
//! keys are dispatched in order until one claims them; rendering is a plain
//! sequential pass over `priority()` order.

use crossterm::event::KeyEvent;
use ratatui::{buffer::Buffer, layout::Rect};

use crate::{Component, Ctx};

/// Priority-ordered component registry.
///
/// `U` defaults to [`crate::protocol::AgentUpdate`]; hosts that emit a
/// different update enum parameterize the registry and map at the boundary.
pub struct ComponentRegistry<U: 'static = crate::protocol::AgentUpdate> {
    components: Vec<(u8, Box<dyn Component<U>>)>,
}

impl<U: 'static> Default for ComponentRegistry<U> {
    fn default() -> Self {
        Self::new()
    }
}

impl<U: 'static> ComponentRegistry<U> {
    pub fn new() -> Self {
        Self {
            components: Vec::new(),
        }
    }

    /// Push a component; kept sorted by `priority()` (stable: equal
    /// priorities keep insertion order).
    pub fn push(&mut self, component: impl Component<U> + 'static) {
        let priority = component.priority();
        let insert_at = self
            .components
            .iter()
            .position(|(p, _)| *p > priority)
            .unwrap_or(self.components.len());
        self.components
            .insert(insert_at, (priority, Box::new(component)));
    }

    /// Dispatch one update to every component; stops early when a component
    /// reports it consumed the update (returned `true`).
    pub fn dispatch_update(&mut self, update: &U, ctx: &mut Ctx<'_>) -> bool {
        for (_, component) in &mut self.components {
            if component.on_update(update, ctx) {
                return true;
            }
        }
        false
    }

    /// Dispatch a key event; stops early when a component consumed it.
    pub fn dispatch_key(&mut self, key: KeyEvent, ctx: &mut Ctx<'_>) -> bool {
        for (_, component) in &mut self.components {
            if component.on_key(key, ctx) {
                return true;
            }
        }
        false
    }

    /// Render every component into `buf` within `area` in priority order;
    /// returns the total visual height used.
    pub fn render_all(&self, area: Rect, buf: &mut Buffer, ctx: &Ctx<'_>) -> u16 {
        let mut used = 0u16;
        for (_, component) in &self.components {
            let remaining = Rect::new(
                area.x,
                area.y + used,
                area.width,
                area.height.saturating_sub(used),
            );
            if remaining.height == 0 {
                break;
            }
            used = used.saturating_add(component.render(remaining, buf, ctx));
        }
        used
    }

    /// Borrow a concrete component by type (downcast); the shell uses this
    /// to read component state for rendering.
    pub fn get<T: Component<U>>(&self) -> Option<&T> {
        self.components
            .iter()
            .find_map(|(_, c)| c.as_any().downcast_ref::<T>())
    }

    pub fn len(&self) -> usize {
        self.components.len()
    }

    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use crate::{
        InputMode, PendingQueue,
        protocol::{AgentUpdate, ThinkingChunk},
        state::LogCoordinator,
    };

    struct Counter {
        updates: Arc<AtomicUsize>,
        keys: Arc<AtomicUsize>,
        priority: u8,
        /// Claims thinking updates only when set.
        claim_thinking: bool,
    }

    impl Counter {
        fn named(priority: u8, claim_thinking: bool) -> (Self, Arc<AtomicUsize>, Arc<AtomicUsize>) {
            let updates = Arc::new(AtomicUsize::new(0));
            let keys = Arc::new(AtomicUsize::new(0));
            (
                Self {
                    updates: Arc::clone(&updates),
                    keys: Arc::clone(&keys),
                    priority,
                    claim_thinking,
                },
                updates,
                keys,
            )
        }
    }

    impl Component for Counter {
        fn on_update(&mut self, update: &AgentUpdate, _ctx: &mut Ctx<'_>) -> bool {
            self.updates.fetch_add(1, Ordering::Relaxed);
            matches!(update, AgentUpdate::ThinkingChunk(_)) && self.claim_thinking
        }

        fn on_key(&mut self, _key: KeyEvent, _ctx: &mut Ctx<'_>) -> bool {
            self.keys.fetch_add(1, Ordering::Relaxed);
            false
        }

        fn render(&self, _area: Rect, _buf: &mut Buffer, _ctx: &Ctx<'_>) -> u16 {
            0
        }

        fn priority(&self) -> u8 {
            self.priority
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
    }

    fn ctx<'a>(
        log: &'a mut LogCoordinator,
        pending: &'a mut PendingQueue,
        stream_events: &'a mut Vec<crate::state::StreamEvent>,
    ) -> Ctx<'a> {
        Ctx {
            log,
            input_mode: InputMode::Normal,
            pending,
            stream_events,
        }
    }

    #[test]
    fn registry_sorts_by_priority_and_stops_on_claim() {
        let mut reg = ComponentRegistry::new();
        let (low, low_updates, _) = Counter::named(50, false);
        let (thinking, thinking_updates, _) = Counter::named(10, true);
        let (high, high_updates, _) = Counter::named(100, false);
        reg.push(low);
        reg.push(thinking);
        reg.push(high);

        let mut log = LogCoordinator::default();
        let mut pending = PendingQueue::default();
        let mut events: Vec<crate::state::StreamEvent> = Vec::new();

        let claimed = reg.dispatch_update(
            &AgentUpdate::ThinkingChunk(ThinkingChunk::Started),
            &mut ctx(&mut log, &mut pending, &mut events),
        );
        assert!(claimed, "thinking counter claims thinking updates");
        // Priority order: 10 (thinking) → 50 (low) → 100 (high).
        assert_eq!(reg.components[0].0, 10);
        assert_eq!(reg.components[1].0, 50);
        assert_eq!(reg.components[2].0, 100);
        assert_eq!(thinking_updates.load(Ordering::Relaxed), 1, "claimed first");
        assert_eq!(
            low_updates.load(Ordering::Relaxed),
            0,
            "dispatch stopped at the claimer"
        );
        assert_eq!(high_updates.load(Ordering::Relaxed), 0);

        // Non-claiming updates reach everyone in order.
        let _ = reg.dispatch_update(
            &AgentUpdate::TaskComplete("done".into()),
            &mut ctx(&mut log, &mut pending, &mut events),
        );
        assert_eq!(thinking_updates.load(Ordering::Relaxed), 2);
        assert_eq!(low_updates.load(Ordering::Relaxed), 1);
        assert_eq!(high_updates.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn keys_bubble_through_all_components() {
        let mut reg = ComponentRegistry::new();
        let (a, _, a_keys) = Counter::named(10, false);
        let (b, _, b_keys) = Counter::named(20, false);
        reg.push(a);
        reg.push(b);
        let mut log = LogCoordinator::default();
        let mut pending = PendingQueue::default();
        let mut events: Vec<crate::state::StreamEvent> = Vec::new();
        let consumed = reg.dispatch_key(
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('x'),
                crossterm::event::KeyModifiers::NONE,
            ),
            &mut ctx(&mut log, &mut pending, &mut events),
        );
        assert!(!consumed);
        assert_eq!(a_keys.load(Ordering::Relaxed), 1);
        assert_eq!(b_keys.load(Ordering::Relaxed), 1);
    }
}

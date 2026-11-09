//! Streaming-text component — parse state machine + event outbox.
//!
//! Owns a [`StreamState`]; on `StreamChunk` it parses the text (fences,
//! tables, paragraphs, code blocks) and pushes [`StreamEvent`]s into
//! `Ctx::stream_events`. The shell applies those events to the log/UI after
//! the update dispatch (rendering, streaming indicators, mermaid splicing),
//! so the component stays free of host rendering concerns.

use crossterm::event::KeyEvent;
use ratatui::style::Color;

use crate::{Component, Ctx, i18n::Messages, protocol::AgentUpdate, state::StreamState};

/// Streams assistant output text into the log.
pub struct StreamComponent {
    state: StreamState,
    messages: Messages,
}

impl StreamComponent {
    pub fn new(messages: Messages) -> Self {
        Self {
            state: StreamState::default(),
            messages,
        }
    }

    /// Swap the locale. Components own a `Messages` because they write log
    /// text that outlives the frame that produced it, so a language change has
    /// to reach them too — see `App::toggle_language`.
    pub fn set_messages(&mut self, messages: Messages) {
        self.messages = messages;
    }

    /// Borrow the parse state (the shell reads `buffer` to render the
    /// in-flight line; event application updates the log).
    pub fn state(&self) -> &StreamState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut StreamState {
        &mut self.state
    }
}

/// Transparent field access: hosts keep `app.<field>…` working after the field
/// type becomes the component (no mechanical churn at call sites).
impl std::ops::Deref for StreamComponent {
    type Target = StreamState;
    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl std::ops::DerefMut for StreamComponent {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

impl Component for StreamComponent {
    fn on_update(&mut self, update: &AgentUpdate, ctx: &mut Ctx<'_>) -> bool {
        if let AgentUpdate::StreamChunk(text) = update {
            for event in self.state.push_chunk(text) {
                ctx.stream_events.push(event);
            }
            true
        } else {
            false
        }
    }

    fn on_key(&mut self, _key: KeyEvent, _ctx: &mut Ctx<'_>) -> bool {
        false
    }

    fn priority(&self) -> u8 {
        20
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// Silence the unused-Color import warning on platforms where the compiler
/// cannot prove the theme default; kept for parity with other components.
#[allow(dead_code)]
const _: Option<(Color, Messages)> = None;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::tool::ToolEvent;
    use crate::{
        InputMode, PendingQueue,
        i18n::Language,
        state::{LogCoordinator, StreamEvent},
    };
    fn setup() -> (
        StreamComponent,
        LogCoordinator,
        PendingQueue,
        Vec<StreamEvent>,
    ) {
        (
            StreamComponent::new(Messages::by_language(Language::English)),
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
        )
    }

    fn ctx<'a>(
        log: &'a mut LogCoordinator,
        pending: &'a mut PendingQueue,
        events: &'a mut Vec<StreamEvent>,
        tool_events: &'a mut Vec<ToolEvent>,
    ) -> Ctx<'a> {
        Ctx {
            log,
            input_mode: InputMode::Normal,
            pending,
            stream_events: events,
            tool_events,
        }
    }

    #[test]
    fn stream_chunk_parses_into_outbox() {
        let (mut comp, mut log, mut pending, mut events) = setup();
        let dirty = comp.on_update(
            &AgentUpdate::StreamChunk("hello\n\n".into()),
            &mut ctx(&mut log, &mut pending, &mut events, &mut Vec::new()),
        );
        assert!(dirty);
        assert_eq!(
            events,
            vec![
                StreamEvent::MarkdownParagraph {
                    text: "hello".into()
                },
                StreamEvent::Blank,
            ]
        );
        assert!(comp.state().buffer.is_empty());
    }

    /// An in-flight line lives in the component state, which is what the host
    /// paints from. The component draws nothing itself.
    #[test]
    fn unfinished_line_stays_in_state_for_the_host_to_paint() {
        let (mut comp, mut log, mut pending, mut events) = setup();
        comp.on_update(
            &AgentUpdate::StreamChunk("partial".into()),
            &mut ctx(&mut log, &mut pending, &mut events, &mut Vec::new()),
        );
        assert!(events.is_empty());
        assert_eq!(comp.state().buffer, "partial");
    }

    #[test]
    fn unrelated_updates_are_ignored() {
        let (mut comp, mut log, mut pending, mut events) = setup();
        let dirty = comp.on_update(
            &AgentUpdate::TaskComplete("done".into()),
            &mut ctx(&mut log, &mut pending, &mut events, &mut Vec::new()),
        );
        assert!(!dirty);
        assert!(events.is_empty());
    }

    #[test]
    fn registry_downcast_reads_stream_state() {
        use crate::components::ComponentRegistry;
        let mut reg = ComponentRegistry::new();
        let (comp, _, _, _) = setup();
        reg.push(comp);
        let borrowed = reg
            .get::<StreamComponent>()
            .expect("downcast to StreamComponent");
        assert!(borrowed.state().buffer.is_empty());
    }
}

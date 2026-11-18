use tact_protocol::{AgentUpdate, ToolOutputChunk};

/// Adjacent streaming values collapsed before they reach the reducer.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum CoalescedUpdate {
    Stream(String),
    ThinkingDelta(String),
    ToolProgress {
        run_id: Option<String>,
        tool_id: String,
        chunks: Vec<ToolOutputChunk>,
    },
    Other(AgentUpdate),
}

pub(crate) fn coalesce(updates: impl IntoIterator<Item = AgentUpdate>) -> Vec<CoalescedUpdate> {
    let mut output = Vec::new();
    for update in updates {
        match update {
            AgentUpdate::StreamChunk(text) => match output.last_mut() {
                Some(CoalescedUpdate::Stream(buffer)) => buffer.push_str(&text),
                _ => output.push(CoalescedUpdate::Stream(text)),
            },
            AgentUpdate::ThinkingChunk(tact_protocol::ThinkingChunk::Delta(text)) => {
                match output.last_mut() {
                    Some(CoalescedUpdate::ThinkingDelta(buffer)) => buffer.push_str(&text),
                    _ => output.push(CoalescedUpdate::ThinkingDelta(text)),
                }
            }
            AgentUpdate::ToolProgress {
                run_id,
                tool_id,
                chunks,
            } => match output.last_mut() {
                Some(CoalescedUpdate::ToolProgress {
                    run_id: previous_run,
                    tool_id: previous,
                    chunks: buffer,
                }) if previous == &tool_id && *previous_run == run_id => buffer.extend(chunks),
                _ => output.push(CoalescedUpdate::ToolProgress {
                    run_id,
                    tool_id,
                    chunks,
                }),
            },
            other => output.push(CoalescedUpdate::Other(other)),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guarantee the controller's coalescing test exists to protect, stated
    /// where it is actually implemented.
    ///
    /// `coalesce` runs inside one batch, so how many batches the actor thread
    /// manages to collect is a scheduling fact, not a property of the reducer.
    /// Asserting on commit counts across a thread boundary made that test fail
    /// on a busy CI machine while the coalescing itself was fine.
    #[test]
    fn a_run_of_stream_chunks_becomes_one_update() {
        let updates = std::iter::once(AgentUpdate::MessageStarted {
            message_id: "m".into(),
        })
        .chain((0..50).map(|_| AgentUpdate::StreamChunk("x".into())));

        let coalesced = coalesce(updates);

        assert_eq!(coalesced.len(), 2, "start plus one merged stream");
        assert!(matches!(
            &coalesced[1],
            CoalescedUpdate::Stream(text) if text == &"x".repeat(50)
        ));
    }

    /// A non-stream update between chunks ends the run: the two sides cannot be
    /// merged without reordering the transcript.
    #[test]
    fn a_message_boundary_ends_the_run() {
        let coalesced = coalesce([
            AgentUpdate::StreamChunk("a".into()),
            AgentUpdate::MessageStarted {
                message_id: "m".into(),
            },
            AgentUpdate::StreamChunk("b".into()),
        ]);

        assert_eq!(coalesced.len(), 3);
    }

    #[test]
    fn thinking_deltas_coalesce_separately_from_text() {
        use tact_protocol::ThinkingChunk;

        let coalesced = coalesce([
            AgentUpdate::ThinkingChunk(ThinkingChunk::Delta("a".into())),
            AgentUpdate::ThinkingChunk(ThinkingChunk::Delta("b".into())),
            AgentUpdate::StreamChunk("c".into()),
        ]);

        assert_eq!(coalesced.len(), 2);
        assert!(matches!(&coalesced[0], CoalescedUpdate::ThinkingDelta(t) if t == "ab"));
        assert!(matches!(&coalesced[1], CoalescedUpdate::Stream(t) if t == "c"));
    }
}

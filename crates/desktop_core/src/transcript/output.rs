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

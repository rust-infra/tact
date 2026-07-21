use tact_protocol::{AgentUpdate, ToolOutputChunk};

/// Sends an already-coalesced progress batch for one tool invocation.
#[derive(Clone, Default)]
pub struct ToolProgressReporter {
    tool_id: String,
    ui_tx: Option<tokio::sync::mpsc::UnboundedSender<AgentUpdate>>,
}

impl ToolProgressReporter {
    pub fn new(tool_id: impl Into<String>, ui_tx: Option<tokio::sync::mpsc::UnboundedSender<AgentUpdate>>) -> Self {
        Self { tool_id: tool_id.into(), ui_tx }
    }

    pub fn report(&self, chunks: Vec<ToolOutputChunk>) {
        if chunks.is_empty() {
            return;
        }
        if let Some(tx) = &self.ui_tx {
            let _ = tx.send(AgentUpdate::ToolProgress { tool_id: self.tool_id.clone(), chunks });
        }
    }
}

#[cfg(test)]
mod tests {
    use tact_protocol::AgentUpdate;

    use super::*;

    #[test]
    fn reporter_binds_progress_to_one_tool_id() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let reporter = ToolProgressReporter::new("bash-7", Some(tx));

        reporter.report(vec![ToolOutputChunk::stdout("hello\n")]);

        assert!(matches!(
            rx.try_recv().unwrap(),
            AgentUpdate::ToolProgress { tool_id, .. } if tool_id == "bash-7"
        ));
    }

    #[test]
    fn reporter_ignores_empty_batches_and_closed_receivers() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let reporter = ToolProgressReporter::new("bash-7", Some(tx));
        reporter.report(Vec::new());
        assert!(rx.try_recv().is_err());

        drop(rx);
        reporter.report(vec![ToolOutputChunk::stdout("ignored")]);
    }
}

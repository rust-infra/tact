//! Read-only projection supplied to lifecycle hooks.

use tact_agent_api::AgentMessage;
use tact_llm::{Message, Role};

use super::services::TactSharedServices;
use crate::hook::SessionStartSource;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionIdentity {
    pub id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelIdentity {
    pub name: String,
}

/// The complete hook-visible state for one activation.
///
/// It deliberately has no path to `Agent`, `AgentState`, a writable
/// transcript, `TranscriptStore`, `UsageSink`, or the model tool port.
#[derive(Clone)]
enum TranscriptView<'a> {
    Llm(&'a [Message]),
    Agent(&'a [AgentMessage]),
}

#[derive(Clone)]
pub struct TactActivationView<'a> {
    services: &'a TactSharedServices,
    transcript: Option<TranscriptView<'a>>,
    session: SessionIdentity,
    model: ModelIdentity,
    turns_taken: u32,
    remaining_turns: Option<u32>,
    session_start_source: SessionStartSource,
}

impl<'a> TactActivationView<'a> {
    pub(crate) fn new(
        services: &'a TactSharedServices,
        transcript: Option<&'a [Message]>,
        session: SessionIdentity,
        model: ModelIdentity,
        turns_taken: u32,
        remaining_turns: Option<u32>,
        session_start_source: SessionStartSource,
    ) -> Self {
        Self {
            services,
            transcript: transcript.map(TranscriptView::Llm),
            session,
            model,
            turns_taken,
            remaining_turns,
            session_start_source,
        }
    }

    pub(crate) fn new_with_agent_messages(
        services: &'a TactSharedServices,
        transcript: Option<&'a [AgentMessage]>,
        session: SessionIdentity,
        model: ModelIdentity,
        turns_taken: u32,
        remaining_turns: Option<u32>,
        session_start_source: SessionStartSource,
    ) -> Self {
        Self {
            services,
            transcript: transcript.map(TranscriptView::Agent),
            session,
            model,
            turns_taken,
            remaining_turns,
            session_start_source,
        }
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session.id.as_deref()
    }

    pub fn model(&self) -> &str {
        &self.model.name
    }

    pub fn turns_taken(&self) -> u32 {
        self.turns_taken
    }

    pub fn remaining_turns(&self) -> Option<u32> {
        self.remaining_turns
    }

    pub fn session_start_source(&self) -> SessionStartSource {
        self.session_start_source
    }

    pub fn permission_mode(&self) -> crate::permission::PermissionMode {
        self.services.permissions.mode()
    }

    pub fn next_hook_status_id(&self) -> u64 {
        self.services.next_hook_status_id()
    }

    pub fn record_rtk(&self, succeeded: bool, input_chars: u64, saved_chars: u64, elapsed_ms: u64) {
        self.services
            .stats
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .record_rtk(succeeded, input_chars, saved_chars, elapsed_ms);
    }

    pub fn emit_update(&self, update: tact_protocol::AgentUpdate) {
        self.services.emit_update(update);
    }

    pub fn queue_hook_context(&self, source: Option<&str>, text: &str) {
        self.services
            .pending_hook_context
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(crate::hook::HookContextChunk::new(source, text));
    }

    pub async fn call_hook_mcp(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> anyhow::Result<String> {
        self.services.hook_mcp().call(name, arguments).await
    }

    pub fn last_assistant_message(&self) -> Option<String> {
        match self.transcript.as_ref()? {
            TranscriptView::Llm(messages) => messages
                .iter()
                .rev()
                .find(|message| matches!(message.role, Role::Assistant))
                .map(|message| crate::extract_text(&message.content))
                .filter(|text| !text.is_empty()),
            TranscriptView::Agent(messages) => messages
                .iter()
                .rev()
                .find(|message| message.role == tact_agent_api::AgentRole::Assistant)
                .map(AgentMessage::text_content)
                .filter(|text| !text.is_empty()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    use tact_llm::{Message, Role};

    use super::*;
    use crate::{
        mcp::McpCatalogHandle,
        permission::{PermissionManager, PermissionMode},
        runtime::permission_authority::PermissionAuthority,
        stats::SessionStats,
    };

    fn services(mode: PermissionMode) -> TactSharedServices {
        TactSharedServices::new(
            tact_protocol::UpdateSink::default(),
            Arc::new(std::sync::RwLock::new(SessionStats::default())),
            PermissionAuthority::new(PermissionManager::try_new(mode).unwrap()),
            Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            Arc::new(std::sync::atomic::AtomicU64::new(1)),
            McpCatalogHandle::new(crate::mcp::MCPToolRouter::new()),
        )
    }

    #[test]
    fn view_exposes_metadata_and_read_only_transcript_queries() {
        let services = services(PermissionMode::Plan);
        let session = SessionIdentity {
            id: Some("session".to_string()),
        };
        let model = ModelIdentity {
            name: "model".to_string(),
        };
        let transcript = vec![
            Message::new_text(Role::User, "user"),
            Message::new_text(Role::Assistant, "assistant"),
        ];
        let view = TactActivationView::new(
            &services,
            Some(&transcript),
            session,
            model,
            3,
            Some(1),
            SessionStartSource::Resume,
        );

        assert_eq!(view.session_id(), Some("session"));
        assert_eq!(view.model(), "model");
        assert_eq!(view.permission_mode(), PermissionMode::Plan);
        assert_eq!(view.turns_taken(), 3);
        assert_eq!(view.remaining_turns(), Some(1));
        assert_eq!(view.last_assistant_message().as_deref(), Some("assistant"));
    }

    #[tokio::test]
    async fn hook_context_can_only_enter_the_pending_queue() {
        let services = services(PermissionMode::Default);
        let session = SessionIdentity { id: None };
        let model = ModelIdentity {
            name: "model".to_string(),
        };
        let view = TactActivationView::new(
            &services,
            None,
            session,
            model,
            0,
            None,
            SessionStartSource::Startup,
        );

        view.queue_hook_context(Some("test"), "context");

        let queue = services.pending_hook_context.lock().unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].text, "context");
    }

    #[tokio::test]
    async fn hook_mcp_handle_is_narrow_but_can_call_a_declared_tool() {
        let services = services(PermissionMode::Default);
        let session = SessionIdentity { id: None };
        let model = ModelIdentity {
            name: "model".to_string(),
        };
        let view = TactActivationView::new(
            &services,
            None,
            session,
            model,
            0,
            None,
            SessionStartSource::Startup,
        );

        assert!(
            view.call_hook_mcp("not-an-mcp-name", json!({}))
                .await
                .is_err()
        );
    }
}

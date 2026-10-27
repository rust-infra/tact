//! Product-owned services shared by one logical agent's adapters.

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};

use tact_protocol::{AgentUpdate, UpdateSink};

use super::{hook_adapter::HookMcpHandle, permission_authority::PermissionAuthority};
use crate::{
    hook::HookContextChunk, mcp::McpCatalogHandle, stats::SessionStats, subagent::SubagentResult,
};

/// Stable control-plane handles for one logical agent.
///
/// The bundle deliberately excludes transcript, cursor, usage, and kernel-event
/// authority. Those remain owned by the kernel or its state-transfer envelope.
#[derive(Clone)]
pub(crate) struct TactSharedServices {
    updates: UpdateSink,
    pub(crate) stats: Arc<RwLock<SessionStats>>,
    pub(crate) permissions: PermissionAuthority,
    pub(crate) pending_subagent_results: Arc<Mutex<VecDeque<SubagentResult>>>,
    pub(crate) pending_hook_context: Arc<Mutex<VecDeque<HookContextChunk>>>,
    hook_status_seq: Arc<AtomicU64>,
    hook_mcp: HookMcpHandle,
    mcp_catalog: McpCatalogHandle,
}

impl TactSharedServices {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        updates: UpdateSink,
        stats: Arc<RwLock<SessionStats>>,
        permissions: PermissionAuthority,
        pending_subagent_results: Arc<Mutex<VecDeque<SubagentResult>>>,
        pending_hook_context: Arc<Mutex<VecDeque<HookContextChunk>>>,
        hook_status_seq: Arc<AtomicU64>,
        mcp_catalog: McpCatalogHandle,
    ) -> Self {
        Self {
            updates,
            stats,
            permissions,
            pending_subagent_results,
            pending_hook_context,
            hook_status_seq,
            hook_mcp: HookMcpHandle::new(mcp_catalog.clone()),
            mcp_catalog,
        }
    }

    pub(crate) fn emit_update(&self, update: AgentUpdate) {
        match &update {
            AgentUpdate::TaskComplete(text) => {
                let summary = text.chars().take(200).collect::<String>();
                let _ = crate::notifications::notify_task_complete(&summary);
            }
            AgentUpdate::StepFailed { idx, error, .. } => {
                let _ = crate::notifications::notify_step_failed(*idx, error);
            }
            _ => {}
        }
        self.updates.send(update);
    }

    pub(crate) fn ui_sender(&self) -> Option<&tokio::sync::mpsc::UnboundedSender<AgentUpdate>> {
        self.updates.ui()
    }

    pub(crate) fn permissions(&self) -> &PermissionAuthority {
        &self.permissions
    }

    pub(crate) fn set_updates(&mut self, updates: UpdateSink) {
        self.updates = updates;
    }

    pub(crate) fn next_hook_status_id(&self) -> u64 {
        self.hook_status_seq.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn hook_mcp(&self) -> &HookMcpHandle {
        &self.hook_mcp
    }

    pub(crate) fn replace_mcp_catalog(&mut self, catalog: McpCatalogHandle) {
        self.hook_mcp = HookMcpHandle::new(catalog.clone());
        self.mcp_catalog = catalog;
    }
}

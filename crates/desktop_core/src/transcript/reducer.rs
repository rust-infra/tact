use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use tact_agent_api::{AgentContentBlock, AgentMessage, AgentRole, MessageOrigin};
use tact_protocol::{
    AgentUpdate, StepStatus, ThinkingChunk, ToolOutputChunk, ToolVisualKind, UiResponse,
};

use crate::{
    gates::PendingUiRequest,
    transcript::row::{
        AssistantRow, ErrorRow, GateCard, GateKind, GateOrigin, GateSelection, GateState,
        HookContextRow, HookStatusRow, LoopId, NoticeRow, RequestId, RowBody, RowId, RowStatus,
        SessionEpoch, ThinkingRow, ToolBody, ToolGroupItem, ToolGroupRow, ToolMetaView,
        ToolOutputView, ToolRow, TranscriptRow, TranscriptView, UsageView, UserRow,
    },
};

#[derive(Debug, Clone)]
pub(crate) struct ReduceContext {
    pub session_epoch: SessionEpoch,
}

#[derive(Debug, Default)]
pub(crate) struct RowDelta {
    pub dirty_rows: Vec<RowId>,
}

impl RowDelta {
    fn changed(&mut self, id: RowId) {
        if !self.dirty_rows.contains(&id) {
            self.dirty_rows.push(id);
        }
    }
}

pub(crate) struct TranscriptReducer {
    rows: im::Vector<Arc<TranscriptRow>>,
    next_row_id: u64,
    next_loop_id: u64,
    ordinal: u64,
    pending_run: Option<String>,
    active_run: Option<String>,
    active_loop: Option<LoopId>,
    active_group: Option<RowId>,
    active_message_id: Option<String>,
    active_assistant: Option<RowId>,
    active_thinking: Option<RowId>,
    assistant_messages: HashMap<String, RowId>,
    last_assistant: Option<RowId>,
    last_group: Option<RowId>,
    last_finished_rows: Vec<RowId>,
    tools: HashMap<(String, String), (RowId, RowId)>,
    gates: HashMap<RequestId, RowId>,
    hook_status: HashMap<u64, RowId>,
}

impl Default for TranscriptReducer {
    fn default() -> Self {
        Self {
            rows: im::Vector::new(),
            next_row_id: 0,
            next_loop_id: 0,
            ordinal: 0,
            pending_run: None,
            active_run: None,
            active_loop: None,
            active_group: None,
            active_message_id: None,
            active_assistant: None,
            active_thinking: None,
            assistant_messages: HashMap::new(),
            last_assistant: None,
            last_group: None,
            last_finished_rows: Vec::new(),
            tools: HashMap::new(),
            gates: HashMap::new(),
            hook_status: HashMap::new(),
        }
    }
}

impl TranscriptReducer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears session-scoped rows while preserving the monotonic identity
    /// allocator used by row-keyed shell entities across session switches.
    pub(crate) fn reset_for_session(&mut self) {
        self.rows.clear();
        self.pending_run = None;
        self.active_run = None;
        self.active_loop = None;
        self.active_group = None;
        self.active_message_id = None;
        self.active_assistant = None;
        self.active_thinking = None;
        self.assistant_messages.clear();
        self.last_assistant = None;
        self.last_group = None;
        self.last_finished_rows.clear();
        self.tools.clear();
        self.gates.clear();
        self.hook_status.clear();
    }

    pub fn view(&self) -> TranscriptView {
        TranscriptView {
            rows: self.rows.clone(),
        }
    }

    pub fn apply_batch(
        &mut self,
        updates: impl IntoIterator<Item = AgentUpdate>,
        context: &ReduceContext,
    ) -> RowDelta {
        let mut delta = RowDelta::default();
        for update in crate::transcript::coalesce(updates) {
            let update = match update {
                crate::transcript::CoalescedUpdate::Stream(text) => AgentUpdate::StreamChunk(text),
                crate::transcript::CoalescedUpdate::ThinkingDelta(text) => {
                    AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(text))
                }
                crate::transcript::CoalescedUpdate::ToolProgress {
                    run_id,
                    tool_id,
                    chunks,
                } => AgentUpdate::ToolProgress {
                    run_id,
                    tool_id,
                    chunks,
                },
                crate::transcript::CoalescedUpdate::Other(update) => update,
            };
            let next = self.apply_agent_update(update, context);
            for row in next.dirty_rows {
                delta.changed(row);
            }
        }
        delta
    }

    #[cfg(test)]
    pub fn total_rows(&self) -> usize {
        self.rows.len()
    }

    pub fn pending_gates(&self) -> Vec<GateCard> {
        let mut gates = Vec::new();
        for row in &self.rows {
            match &row.body {
                RowBody::Gate(gate)
                    if matches!(gate.status, GateState::Pending | GateState::Submitting) =>
                {
                    gates.push(gate.clone());
                }
                RowBody::ToolGroup(group) => {
                    gates.extend(group.items.iter().filter_map(|item| match item {
                        ToolGroupItem::Gate(gate)
                            if matches!(
                                gate.status,
                                GateState::Pending | GateState::Submitting
                            ) =>
                        {
                            Some(gate.clone())
                        }
                        _ => None,
                    }));
                }
                _ => {}
            }
        }
        gates.sort_by_key(|gate| gate.request_id.0);
        gates
    }

    pub fn push_user(
        &mut self,
        text: impl Into<Arc<str>>,
        attachments: Vec<std::path::PathBuf>,
    ) -> RowId {
        self.push_row(
            None,
            RowBody::User(UserRow {
                text: text.into(),
                attachments: attachments.into(),
            }),
        )
    }

    pub fn push_notice(&mut self, text: impl Into<Arc<str>>) -> RowId {
        self.push_row(None, RowBody::Notice(NoticeRow { text: text.into() }))
    }

    #[cfg(test)]
    pub fn push_diff(&mut self, path: Option<Arc<str>>, patch: impl Into<Arc<str>>) -> RowId {
        self.push_row(
            None,
            RowBody::Diff(crate::transcript::row::DiffRow {
                path,
                patch: patch.into(),
                status: RowStatus::Success,
            }),
        )
    }

    pub fn toggle_group(&mut self, id: RowId) -> Option<bool> {
        let mut collapsed = None;
        self.update_group(id, |group| {
            group.collapsed = !group.collapsed;
            collapsed = Some(group.collapsed);
        });
        collapsed
    }

    pub fn toggle_tool(&mut self, group: RowId, tool: RowId) -> Option<bool> {
        let mut expanded = None;
        self.update_tool_in_group(group, tool, |tool| {
            tool.expanded = !tool.expanded;
            expanded = Some(tool.expanded);
        });
        expanded
    }

    pub fn apply_agent_update(
        &mut self,
        update: AgentUpdate,
        _context: &ReduceContext,
    ) -> RowDelta {
        let mut delta = RowDelta::default();
        match update {
            AgentUpdate::RunStarted { run_id } => {
                self.pending_run = Some(run_id);
                self.active_run = None;
                self.active_group = None;
                self.active_assistant = None;
                self.active_thinking = None;
                self.assistant_messages.clear();
                self.last_assistant = None;
                self.last_group = None;
                self.last_finished_rows.clear();
            }
            AgentUpdate::RunFinished { run_id } => {
                let compact_only = self.pending_run.as_deref() == Some(run_id.as_str())
                    && self.active_run.is_none();
                if compact_only {
                    self.pending_run = None;
                    return delta;
                }
                if self.active_run.as_deref() == Some(run_id.as_str())
                    || self.pending_run.as_deref() == Some(run_id.as_str())
                {
                    if let Some(group) = self.active_group.take() {
                        self.settle_group(group, true);
                        self.last_group = Some(group);
                        self.last_finished_rows.push(group);
                        delta.changed(group);
                    }
                    if let Some(row) = self.active_assistant.take() {
                        self.update_assistant(row, |assistant| {
                            assistant.status = RowStatus::Success;
                        });
                        self.last_assistant = Some(row);
                        self.last_finished_rows.push(row);
                        delta.changed(row);
                    }
                    if let Some(row) = self.active_thinking.take() {
                        self.update_thinking(row, |thinking| {
                            thinking.status = RowStatus::Success;
                        });
                        self.last_finished_rows.push(row);
                        delta.changed(row);
                    }
                }
                if self.pending_run.as_deref() == Some(run_id.as_str()) {
                    self.pending_run = None;
                }
                if self.active_run.as_deref() == Some(run_id.as_str()) {
                    self.active_run = None;
                }
            }
            AgentUpdate::MessageStarted { message_id } => {
                self.begin_assistant_message(&message_id, &mut delta);
            }
            AgentUpdate::UiResponseRejected { request_id } => {
                let id = RequestId(request_id);
                if self.gates.contains_key(&id) {
                    self.update_gate(id, |gate| gate.status = GateState::Stale);
                    if let Some(row) = self.gates.get(&id).copied() {
                        delta.changed(self.top_level_group_for(row).unwrap_or(row));
                    }
                }
            }
            AgentUpdate::TurnStats { .. } => {
                let run_id = self
                    .pending_run
                    .take()
                    .or_else(|| self.active_run.clone())
                    .unwrap_or_else(|| "fallback-run".to_string());
                self.active_run = Some(run_id);
                if let Some(group) = self.active_group.take() {
                    self.update_group(group, |group| {
                        group.status = RowStatus::Success;
                        group.collapsed = true;
                    });
                    self.last_group = Some(group);
                    delta.changed(group);
                }
                let loop_id = LoopId(self.next_loop_id);
                self.next_loop_id += 1;
                self.active_loop = Some(loop_id);
            }
            AgentUpdate::StreamChunk(text) => {
                let row = self.ensure_assistant();
                self.append_assistant(row, &text);
                delta.changed(row);
            }
            AgentUpdate::ThinkingChunk(chunk) => match chunk {
                ThinkingChunk::Started => {
                    if let Some(row) = self.active_thinking.take() {
                        self.update_thinking(row, |thinking| {
                            thinking.status = RowStatus::Success;
                        });
                        delta.changed(row);
                    }
                    let row = self.push_row(
                        self.current_loop(),
                        RowBody::Thinking(ThinkingRow {
                            markdown: Arc::from(""),
                            status: RowStatus::Running,
                            duration_ms: None,
                        }),
                    );
                    self.active_thinking = Some(row);
                    self.move_empty_assistant_to_tail();
                    delta.changed(row);
                }
                ThinkingChunk::Delta(text) => {
                    let row = self.active_thinking.unwrap_or_else(|| {
                        self.push_row(
                            self.current_loop(),
                            RowBody::Thinking(ThinkingRow {
                                markdown: Arc::from(""),
                                status: RowStatus::Running,
                                duration_ms: None,
                            }),
                        )
                    });
                    self.active_thinking = Some(row);
                    self.move_empty_assistant_to_tail();
                    self.append_thinking(row, &text);
                    delta.changed(row);
                }
                ThinkingChunk::Finished => {
                    if let Some(row) = self.active_thinking.take() {
                        self.update_thinking(row, |thinking| {
                            thinking.status = RowStatus::Success;
                        });
                        delta.changed(row);
                    }
                }
            },
            AgentUpdate::StepStarted {
                tool_id,
                tool_name,
                arg_summary,
                arg_full,
                presentation,
                ..
            } => {
                let row = self.ensure_tool(
                    &tool_id,
                    &tool_name,
                    &arg_summary,
                    &arg_full,
                    presentation.visual_kind,
                );
                if let Some((group, _)) = self.current_tool(&tool_id).copied() {
                    delta.changed(group);
                } else {
                    delta.changed(row);
                }
            }
            AgentUpdate::StepFinished {
                run_id,
                tool_id,
                result,
                ..
            } => {
                if !self.run_matches_active(run_id.as_deref()) {
                    return delta;
                }
                if let Some((group, row)) = self.current_tool(&tool_id).copied() {
                    let keep_live = result.presentation.keep_live;
                    self.update_tool_by_row(row, |tool| {
                        tool.keep_live = keep_live;
                        tool.duration_ms = result.duration_us.map(|micros| micros / 1_000);
                        tool.status = match result.status {
                            StepStatus::Failed => RowStatus::Failed,
                            StepStatus::Success if keep_live => RowStatus::Running,
                            StepStatus::Success => RowStatus::Success,
                        };
                        let text = result
                            .detail
                            .clone()
                            .unwrap_or_else(|| result.message.clone());
                        if result.presentation.visual_kind == ToolVisualKind::FileEdit
                            && text.contains("@@")
                        {
                            tool.body = ToolBody::Diff {
                                path: Some(Arc::from(result.arg_summary)),
                                patch: Arc::from(text.clone()),
                            };
                        } else if !result.message.is_empty() {
                            tool.body = ToolBody::Text(Arc::from(result.message));
                        }
                        tool.output = output_view(&text);
                    });
                    self.settle_group(group, false);
                    delta.changed(group);
                } else {
                    let row = self
                        .push_notice(format!("Ignored StepFinished for unknown tool `{tool_id}`"));
                    delta.changed(row);
                }
            }
            AgentUpdate::StepFailed {
                run_id,
                tool_id,
                arg_summary,
                error,
                ..
            } => {
                if !self.run_matches_active(run_id.as_deref()) {
                    return delta;
                }
                if let Some((group, row)) = self.current_tool(&tool_id).copied() {
                    self.update_tool_by_row(row, |tool| {
                        tool.status = RowStatus::Failed;
                        tool.output = output_view(&error);
                        tool.expanded = true;
                    });
                    self.settle_group(group, false);
                    delta.changed(group);
                } else {
                    let row = self.push_notice(format!(
                        "Ignored StepFailed for unknown tool `{tool_id}` ({arg_summary})"
                    ));
                    delta.changed(row);
                }
            }
            AgentUpdate::ToolProgress {
                run_id,
                tool_id,
                chunks,
            } => {
                if !self.run_matches_active(run_id.as_deref()) {
                    return delta;
                }
                let text = chunks_text(&chunks);
                let target = if run_id.is_some() {
                    self.current_tool(&tool_id).copied()
                } else {
                    self.latest_tool(&tool_id).copied()
                };
                if let Some((group, row)) = target {
                    self.update_tool_by_row(row, |tool| append_output(tool, &text));
                    delta.changed(group);
                }
            }
            AgentUpdate::ToolMeta {
                tool_id,
                model,
                token_usage,
                context_window,
                task_id,
                subagent_id,
            } => {
                if let Some((group, row)) = self.latest_tool(&tool_id).copied() {
                    self.update_tool_by_row(row, |tool| {
                        if let Some(model) = model {
                            tool.meta.model = Some(Arc::from(model));
                        }
                        if let Some(usage) = token_usage {
                            tool.meta.token_usage = Some(UsageView {
                                prompt_tokens: usage.prompt,
                                completion_tokens: usage.completion,
                                total_tokens: usage.total,
                                cache_hit_tokens: usage.prompt_cache_hit_tokens,
                                reasoning_tokens: usage.reasoning_tokens,
                                context_window,
                            });
                        }
                        tool.meta.context_window = context_window;
                        tool.meta.task_id = task_id.map(Arc::from);
                        tool.meta.subagent_id = subagent_id.map(Arc::from);
                    });
                    delta.changed(group);
                }
            }
            AgentUpdate::BackgroundTaskFinished {
                tool_id,
                success,
                message,
                output,
            } => {
                if let Some((group, row)) = self.latest_tool(&tool_id).copied() {
                    self.update_tool_by_row(row, |tool| {
                        tool.status = if success {
                            RowStatus::Success
                        } else {
                            RowStatus::Failed
                        };
                        tool.output = output_view(&output);
                        tool.body = ToolBody::Text(Arc::from(message));
                        tool.keep_live = false;
                    });
                    self.settle_group(group, false);
                    delta.changed(group);
                }
            }
            AgentUpdate::SubagentFinished {
                tool_id,
                child_id,
                success,
                summary,
            } => {
                if let Some((group, row)) = self.latest_tool(&tool_id).copied() {
                    self.update_tool_by_row(row, |tool| {
                        tool.status = if success {
                            RowStatus::Success
                        } else {
                            RowStatus::Failed
                        };
                        tool.meta.subagent_id = Some(Arc::from(child_id));
                        tool.output = output_view(&summary);
                        tool.keep_live = false;
                    });
                    self.settle_group(group, false);
                    delta.changed(group);
                }
            }
            AgentUpdate::Info(text) | AgentUpdate::MdInfo(text) => {
                let row = self.push_notice(text);
                delta.changed(row);
            }
            AgentUpdate::HookContext { source, text } => {
                let row = self.push_row(
                    None,
                    RowBody::HookContext(HookContextRow {
                        source: source.map(Arc::from),
                        markdown: Arc::from(text),
                    }),
                );
                delta.changed(row);
            }
            AgentUpdate::HookStatus {
                id,
                source,
                message,
                elapsed_ms,
            } => {
                let row = if let Some(row) = self.hook_status.get(&id).copied() {
                    self.update_hook_status(row, |status| {
                        status.source = source.map(Arc::from);
                        status.message = Arc::from(message);
                        status.elapsed_ms = elapsed_ms;
                    });
                    row
                } else {
                    let row = self.push_row(
                        None,
                        RowBody::HookStatus(HookStatusRow {
                            hook_id: id,
                            source: source.map(Arc::from),
                            message: Arc::from(message),
                            elapsed_ms,
                        }),
                    );
                    self.hook_status.insert(id, row);
                    row
                };
                delta.changed(row);
            }
            AgentUpdate::Error(error) => {
                if let Some(row) = self.active_assistant.take().or(self.last_assistant) {
                    self.update_assistant(row, |assistant| {
                        assistant.status = RowStatus::Failed;
                    });
                    self.last_assistant = Some(row);
                    delta.changed(row);
                }
                if let Some(group) = self.active_group.take().or(self.last_group) {
                    self.update_group(group, |group| {
                        group.status = RowStatus::Failed;
                    });
                    self.last_group = Some(group);
                    delta.changed(group);
                }
                let row = self.push_row(
                    None,
                    RowBody::Error(ErrorRow {
                        message: Arc::from(error.to_string()),
                        retryable: true,
                    }),
                );
                delta.changed(row);
            }
            AgentUpdate::TaskComplete(_) => {
                self.last_finished_rows.clear();
                if let Some(group) = self.active_group.take() {
                    self.settle_group(group, true);
                    self.last_group = Some(group);
                    delta.changed(group);
                }
                if let Some(row) = self.active_assistant.take() {
                    self.update_assistant(row, |assistant| {
                        assistant.status = RowStatus::Success;
                    });
                    self.last_assistant = Some(row);
                    delta.changed(row);
                }
                if let Some(row) = self.active_thinking.take() {
                    self.update_thinking(row, |thinking| {
                        thinking.status = RowStatus::Success;
                    });
                    delta.changed(row);
                }
                self.pending_run = None;
                self.active_run = None;
            }
            AgentUpdate::TaskCancelled => {
                for row in std::mem::take(&mut self.last_finished_rows) {
                    match self
                        .rows
                        .iter()
                        .find(|candidate| candidate.id == row)
                        .map(|row| &row.body)
                    {
                        Some(RowBody::ToolGroup(_)) => self.cancel_group(row),
                        Some(RowBody::Assistant(_)) => self.update_assistant(row, |assistant| {
                            assistant.status = RowStatus::Cancelled;
                        }),
                        Some(RowBody::Thinking(_)) => self.update_thinking(row, |thinking| {
                            thinking.status = RowStatus::Cancelled;
                        }),
                        _ => {}
                    }
                    delta.changed(row);
                }
                if let Some(row) = self.active_assistant.take() {
                    self.update_assistant(row, |assistant| {
                        assistant.status = RowStatus::Cancelled;
                    });
                    self.last_assistant = Some(row);
                    delta.changed(row);
                }
                if let Some(group) = self.active_group.take() {
                    self.cancel_group(group);
                    self.last_group = Some(group);
                    delta.changed(group);
                }
                if let Some(row) = self.active_thinking.take() {
                    self.update_thinking(row, |thinking| {
                        thinking.status = RowStatus::Cancelled;
                    });
                    delta.changed(row);
                }
                self.pending_run = None;
                self.active_run = None;
            }
            AgentUpdate::TaskIdle => {
                if let Some(group) = self.active_group.take() {
                    self.settle_group(group, true);
                    self.last_group = Some(group);
                    delta.changed(group);
                }
                if let Some(row) = self.active_assistant.take() {
                    self.update_assistant(row, |assistant| {
                        assistant.status = RowStatus::Success;
                    });
                    self.last_assistant = Some(row);
                    delta.changed(row);
                }
                if let Some(row) = self.active_thinking.take() {
                    self.update_thinking(row, |thinking| {
                        thinking.status = RowStatus::Success;
                    });
                    delta.changed(row);
                }
                self.pending_run = None;
                self.active_run = None;
            }
            AgentUpdate::RequestSelect { .. } | AgentUpdate::RequestMultiSelect { .. } => {}
            AgentUpdate::ToolTranscript { tool_id, entry } => {
                if self.apply_subagent_entry(&tool_id, entry)
                    && let Some((group, _)) = self.latest_tool(&tool_id)
                {
                    delta.changed(*group);
                }
            }
            AgentUpdate::ToolTranscriptStream { tool_id, text } => {
                if self.replace_subagent_assistant(&tool_id, &text)
                    && let Some((group, _)) = self.latest_tool(&tool_id)
                {
                    delta.changed(*group);
                }
            }
            _ => {}
        }
        delta
    }

    pub fn reconcile_gates(
        &mut self,
        snapshot: &[PendingUiRequest],
        context: &ReduceContext,
    ) -> RowDelta {
        let mut delta = RowDelta::default();
        let pending: HashSet<RequestId> = snapshot
            .iter()
            .map(|request| RequestId(request.request_id))
            .collect();
        for request in snapshot {
            let id = RequestId(request.request_id);
            let row = if self.gates.contains_key(&id) {
                self.update_gate(id, |gate| {
                    gate.prompt = Arc::from(request.prompt.clone());
                    gate.options = request
                        .options
                        .iter()
                        .cloned()
                        .map(Arc::from)
                        .collect::<Vec<_>>()
                        .into();
                    gate.kind = match request.kind {
                        tact::ui_responder::PendingUiRequestKind::Select => GateKind::Select,
                        tact::ui_responder::PendingUiRequestKind::MultiSelect => {
                            GateKind::MultiSelect
                        }
                    };
                    gate.purpose = request.purpose;
                    gate.origin = GateOrigin::from(request.origin.clone());
                    if !matches!(
                        gate.status,
                        GateState::Submitting
                            | GateState::Resolved
                            | GateState::Cancelled
                            | GateState::Stale
                    ) {
                        gate.status = GateState::Pending;
                    }
                });
                self.gates
                    .get(&id)
                    .copied()
                    .map(|row| self.top_level_group_for(row).unwrap_or(row))
            } else {
                Some(self.insert_gate(
                    id,
                    context.session_epoch,
                    request.purpose,
                    match request.kind {
                        tact::ui_responder::PendingUiRequestKind::Select => GateKind::Select,
                        tact::ui_responder::PendingUiRequestKind::MultiSelect => {
                            GateKind::MultiSelect
                        }
                    },
                    request.prompt.clone(),
                    request.options.clone(),
                    request.origin.clone(),
                ))
            };
            if let Some(row) = row {
                delta.changed(row);
            }
        }
        let stale: Vec<RequestId> = self
            .gates
            .keys()
            .copied()
            .filter(|id| !pending.contains(id))
            .collect();
        for id in stale {
            let next = match self.gate_state(id) {
                Some(GateState::Pending) => Some(GateState::Withdrawn),
                Some(GateState::Submitting) => Some(GateState::Resolved),
                _ => None,
            };
            if let Some(next) = next {
                self.update_gate(id, |gate| gate.status = next);
                if let Some(row) = self.gates.get(&id).copied() {
                    delta.changed(self.top_level_group_for(row).unwrap_or(row));
                }
            }
        }
        delta
    }

    pub fn set_gate_draft(
        &mut self,
        session_epoch: SessionEpoch,
        request_id: RequestId,
        selected: Vec<usize>,
    ) -> Option<RowId> {
        let row_id = self.gates.get(&request_id).copied()?;
        if let Some(group_index) = self.rows.iter().position(|row| {
            matches!(
                &row.body,
                RowBody::ToolGroup(group)
                    if group.items.iter().any(|item| item.id() == row_id)
            )
        }) {
            let mut row = (*self.rows[group_index]).clone();
            let RowBody::ToolGroup(group) = &mut row.body else {
                return None;
            };
            let mut items = group.items.to_vec();
            let ToolGroupItem::Gate(gate) = items.iter_mut().find(|item| item.id() == row_id)?
            else {
                return None;
            };
            if gate.session_epoch != session_epoch || gate.status != GateState::Pending {
                return None;
            }
            let mut selected = selected;
            selected.sort_unstable();
            selected.dedup();
            gate.selection = GateSelection::Multiple(selected);
            group.items = Arc::from(items);
            let dirty = row.id;
            self.rows.set(group_index, Arc::new(row));
            return Some(dirty);
        }
        let index = self.rows.iter().position(|row| row.id == row_id)?;
        let mut row = (*self.rows[index]).clone();
        let RowBody::Gate(gate) = &mut row.body else {
            return None;
        };
        if gate.session_epoch != session_epoch || gate.status != GateState::Pending {
            return None;
        }
        let mut selected = selected;
        selected.sort_unstable();
        selected.dedup();
        gate.selection = GateSelection::Multiple(selected);
        self.rows.set(index, Arc::new(row));
        Some(row_id)
    }

    pub fn validate_gate_response(
        &mut self,
        session_epoch: SessionEpoch,
        response: &UiResponse,
    ) -> Result<(), GateResponseError> {
        let id = RequestId(response.request_id());
        let Some(gate) = self.gate(id) else {
            return Err(GateResponseError::Unknown);
        };
        if gate.session_epoch != session_epoch {
            return Err(GateResponseError::Stale);
        }
        let response_matches = matches!(
            (gate.kind, response),
            (GateKind::Select, UiResponse::Select { .. })
                | (GateKind::MultiSelect, UiResponse::MultiSelect { .. })
        );
        if !response_matches {
            return Err(GateResponseError::KindMismatch);
        }
        if gate.status != GateState::Pending {
            return Err(GateResponseError::AlreadyAnswered);
        }
        Ok(())
    }

    pub fn apply_gate_response(
        &mut self,
        session_epoch: SessionEpoch,
        response: &UiResponse,
    ) -> Result<RowId, GateResponseError> {
        self.validate_gate_response(session_epoch, response)?;
        let id = RequestId(response.request_id());
        let cancelled = matches!(
            response,
            UiResponse::Select { choice: None, .. } | UiResponse::MultiSelect { choices: None, .. }
        );
        self.update_gate(id, |gate| {
            gate.status = if cancelled {
                GateState::Cancelled
            } else {
                GateState::Submitting
            };
            gate.selection = match response {
                UiResponse::Select { choice, .. } => GateSelection::Single(*choice),
                UiResponse::MultiSelect { choices, .. } => {
                    GateSelection::Multiple(choices.clone().unwrap_or_default())
                }
            };
        });
        let row = self.gates[&id];
        Ok(self.top_level_group_for(row).unwrap_or(row))
    }

    pub fn seed_persisted(
        &mut self,
        messages: &[AgentMessage],
        _context: &ReduceContext,
    ) -> RowDelta {
        let mut delta = RowDelta::default();
        let mut tools = HashMap::<String, RowId>::new();
        for message in messages {
            match &message.origin {
                MessageOrigin::User => {
                    let text = message_text(message);
                    let row = self.push_row(
                        None,
                        RowBody::User(UserRow {
                            text: Arc::from(text),
                            attachments: Arc::from([]),
                        }),
                    );
                    delta.changed(row);
                }
                MessageOrigin::InjectedContext { source } => {
                    let row = self.push_row(
                        None,
                        RowBody::HookContext(HookContextRow {
                            source: Some(Arc::from(source.clone())),
                            markdown: Arc::from(message_text(message)),
                        }),
                    );
                    delta.changed(row);
                }
                MessageOrigin::ToolResult => {
                    for block in &message.content {
                        if let AgentContentBlock::ToolResult {
                            tool_use_id,
                            content,
                        } = block
                            && let Some(row) = tools.get(tool_use_id).copied()
                        {
                            self.update_tool_by_row(row, |tool| {
                                tool.output = output_view(content);
                                tool.status = RowStatus::Success;
                            });
                            delta.changed(row);
                        }
                    }
                }
                MessageOrigin::Assistant if message.role == AgentRole::Assistant => {
                    for block in &message.content {
                        if let AgentContentBlock::Thinking { thinking, .. } = block
                            && !thinking.trim().is_empty()
                        {
                            let row = self.push_row(
                                None,
                                RowBody::Thinking(ThinkingRow {
                                    markdown: Arc::from(thinking.as_str()),
                                    status: RowStatus::Success,
                                    duration_ms: None,
                                }),
                            );
                            delta.changed(row);
                        }
                    }
                    let text = message_text(message);
                    if !text.trim().is_empty() {
                        let row = self.push_row(
                            None,
                            RowBody::Assistant(AssistantRow {
                                markdown: Arc::from(text),
                                status: RowStatus::Success,
                                suggestions: Arc::from([]),
                            }),
                        );
                        delta.changed(row);
                    }
                    let tool_uses: Vec<(String, String, serde_json::Value)> = message
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            AgentContentBlock::ToolUse { id, name, input } => {
                                Some((id.clone(), name.clone(), input.clone()))
                            }
                            _ => None,
                        })
                        .collect();
                    if !tool_uses.is_empty() {
                        self.active_group = None;
                        let group = self.ensure_group();
                        for (id, name, input) in tool_uses {
                            let row = self.ensure_tool(
                                &id,
                                &name,
                                &input.to_string(),
                                &input.to_string(),
                                ToolVisualKind::Generic,
                            );
                            tools.insert(id, row);
                            delta.changed(row);
                        }
                        delta.changed(group);
                    }
                }
                MessageOrigin::GoalContext
                | MessageOrigin::SubagentResult
                | MessageOrigin::Continuation
                | MessageOrigin::CompactionSummary => {
                    let text = message_text(message);
                    if !text.trim().is_empty() {
                        let row = self.push_row(
                            None,
                            RowBody::Notice(NoticeRow {
                                text: Arc::from(text),
                            }),
                        );
                        delta.changed(row);
                    }
                }
                _ => {}
            }
        }
        let settled = self
            .rows
            .iter()
            .filter_map(|row| match &row.body {
                RowBody::ToolGroup(group)
                    if !group.items.is_empty()
                        && group.items.iter().all(|item| match item {
                            ToolGroupItem::Tool(tool) => {
                                !matches!(tool.status, RowStatus::Pending | RowStatus::Running)
                            }
                            ToolGroupItem::Gate(_) => true,
                        }) =>
                {
                    Some(row.id)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        for group in settled {
            self.settle_group(group, true);
            delta.changed(group);
        }
        delta
    }

    fn ensure_assistant(&mut self) -> RowId {
        if let Some(row) = self.active_assistant {
            return row;
        }
        if let Some(message_id) = self.active_message_id.as_deref()
            && let Some(row) = self.assistant_messages.get(message_id).copied()
        {
            self.active_assistant = Some(row);
            self.last_assistant = Some(row);
            return row;
        }
        let row = self.push_row(
            self.current_loop(),
            RowBody::Assistant(AssistantRow {
                markdown: Arc::from(""),
                status: RowStatus::Running,
                suggestions: Arc::from([]),
            }),
        );
        if let Some(message_id) = self.active_message_id.clone() {
            self.assistant_messages.insert(message_id, row);
        }
        self.active_assistant = Some(row);
        self.last_assistant = Some(row);
        row
    }

    fn begin_assistant_message(&mut self, message_id: &str, delta: &mut RowDelta) {
        if self.active_message_id.as_deref() == Some(message_id) {
            return;
        }
        if let Some(row) = self.active_thinking.take() {
            self.update_thinking(row, |thinking| {
                if thinking.status == RowStatus::Running {
                    thinking.status = RowStatus::Success;
                }
            });
            self.last_finished_rows.push(row);
            delta.changed(row);
        }
        if let Some(row) = self.assistant_messages.get(message_id).copied() {
            self.active_message_id = Some(message_id.to_string());
            self.active_assistant = Some(row);
            self.last_assistant = Some(row);
            return;
        }
        if let Some(row) = self.active_assistant.take() {
            self.update_assistant(row, |assistant| {
                if assistant.status == RowStatus::Running {
                    assistant.status = RowStatus::Success;
                }
            });
            self.last_assistant = Some(row);
            delta.changed(row);
        }
        self.active_message_id = Some(message_id.to_string());
        self.active_assistant = None;
    }

    fn ensure_group(&mut self) -> RowId {
        if let Some(group) = self.active_group {
            return group;
        }
        let loop_id = self.active_loop.take();
        let row = self.push_row(
            loop_id,
            RowBody::ToolGroup(ToolGroupRow {
                loop_id: loop_id.unwrap_or(LoopId(u64::MAX)),
                items: Arc::from([]),
                summary: Arc::from("0 tools"),
                status: RowStatus::Running,
                collapsed: false,
            }),
        );
        self.active_group = Some(row);
        self.last_group = Some(row);
        row
    }

    fn ensure_tool(
        &mut self,
        tool_id: &str,
        name: &str,
        arg_summary: &str,
        arg_full: &str,
        visual_kind: ToolVisualKind,
    ) -> RowId {
        let key = self.current_tool_key(tool_id);
        if let Some((_, row)) = self.tools.get(&key) {
            return *row;
        }
        let group = self.ensure_group();
        let row = self.alloc_id();
        let body = match visual_kind {
            ToolVisualKind::Command => ToolBody::Code {
                language: Some(Arc::from("shell")),
                text: Arc::from(arg_summary),
            },
            _ => ToolBody::None,
        };
        let tool = ToolGroupItem::Tool(ToolRow {
            id: row,
            tool_id: Arc::from(tool_id),
            name: Arc::from(name),
            arg_summary: Arc::from(arg_summary),
            arg_full: (!arg_full.is_empty()).then(|| Arc::from(arg_full)),
            status: RowStatus::Running,
            body,
            output: ToolOutputView::default(),
            meta: ToolMetaView::default(),
            subagent: None,
            expanded: false,
            keep_live: false,
            destructive: visual_kind == ToolVisualKind::FileEdit,
            duration_ms: None,
        });
        self.append_group_item(group, tool);
        self.tools.insert(key, (group, row));
        row
    }

    fn current_tool_key(&self, tool_id: &str) -> (String, String) {
        (
            self.active_run.clone().unwrap_or_default(),
            tool_id.to_string(),
        )
    }

    fn run_matches_active(&self, run_id: Option<&str>) -> bool {
        run_id.is_none_or(|run_id| self.active_run.as_deref() == Some(run_id))
    }

    fn current_tool(&self, tool_id: &str) -> Option<&(RowId, RowId)> {
        self.tools.get(&self.current_tool_key(tool_id))
    }

    fn latest_tool(&self, tool_id: &str) -> Option<&(RowId, RowId)> {
        self.tools
            .iter()
            .filter(|((_, candidate), _)| candidate == tool_id)
            .max_by_key(|(_, (_, row))| *row)
            .map(|(_, value)| value)
    }

    fn apply_subagent_entry(
        &mut self,
        tool_id: &str,
        entry: tact_protocol::subagent_transcript::SubagentTranscriptEntry,
    ) -> bool {
        use tact_protocol::subagent_transcript::{SubagentStepStatus, SubagentTranscriptEntry};
        let Some((_, row)) = self.latest_tool(tool_id).copied() else {
            return false;
        };
        let next_row_id = self.alloc_id();
        let next_ordinal = self.ordinal;
        self.ordinal += 1;
        let next_loop_id = LoopId(self.next_loop_id);
        self.next_loop_id += 1;
        let mut changed = true;
        self.update_tool_by_row(row, |tool| {
            let mut nested = tool.subagent.clone().unwrap_or(TranscriptView {
                rows: im::Vector::new(),
            });
            let body = match entry {
                SubagentTranscriptEntry::Assistant(text) => {
                    if let Some(last) = nested.rows.back_mut()
                        && let RowBody::Assistant(assistant) = &mut Arc::make_mut(last).body
                    {
                        assistant.markdown = Arc::from(text);
                        assistant.status = RowStatus::Running;
                        changed = true;
                        tool.subagent = Some(nested);
                        return;
                    }
                    RowBody::Assistant(AssistantRow {
                        markdown: Arc::from(text),
                        status: RowStatus::Running,
                        suggestions: Arc::from([]),
                    })
                }
                SubagentTranscriptEntry::Thinking { text, duration_us } => {
                    RowBody::Thinking(ThinkingRow {
                        markdown: Arc::from(text),
                        status: RowStatus::Success,
                        duration_ms: duration_us.map(|micros| micros / 1_000),
                    })
                }
                SubagentTranscriptEntry::Step(step) => {
                    let status = match step.status {
                        SubagentStepStatus::Running => RowStatus::Running,
                        SubagentStepStatus::Success => RowStatus::Success,
                        SubagentStepStatus::Failed => RowStatus::Failed,
                    };
                    let item = ToolGroupItem::Tool(ToolRow {
                        id: next_row_id,
                        tool_id: Arc::from(step.tool_id),
                        name: Arc::from(step.tool_name),
                        arg_summary: Arc::from(step.arg_summary),
                        arg_full: (!step.arg_full.is_empty()).then(|| Arc::from(step.arg_full)),
                        status,
                        body: ToolBody::Text(Arc::from(step.detail.unwrap_or(step.message))),
                        output: ToolOutputView::default(),
                        meta: ToolMetaView::default(),
                        subagent: None,
                        expanded: false,
                        keep_live: false,
                        destructive: false,
                        duration_ms: None,
                    });
                    RowBody::ToolGroup(ToolGroupRow {
                        loop_id: next_loop_id,
                        items: Arc::from([item]),
                        summary: Arc::from("1 tool"),
                        status,
                        collapsed: status != RowStatus::Running,
                    })
                }
                SubagentTranscriptEntry::Info(text) => RowBody::Notice(NoticeRow {
                    text: Arc::from(text),
                }),
                SubagentTranscriptEntry::Error(text) => RowBody::Error(ErrorRow {
                    message: Arc::from(text),
                    retryable: false,
                }),
            };
            nested.rows.push_back(Arc::new(TranscriptRow {
                id: next_row_id,
                ordinal: next_ordinal,
                loop_id: None,
                body,
            }));
            tool.subagent = Some(nested);
        });
        changed
    }

    fn replace_subagent_assistant(&mut self, tool_id: &str, text: &str) -> bool {
        let Some((_, row)) = self.latest_tool(tool_id).copied() else {
            return false;
        };
        let next_row_id = self.alloc_id();
        let next_ordinal = self.ordinal;
        self.ordinal += 1;
        self.update_tool_by_row(row, |tool| {
            let mut nested = tool.subagent.clone().unwrap_or(TranscriptView {
                rows: im::Vector::new(),
            });
            if let Some(last) = nested.rows.back_mut()
                && let RowBody::Assistant(assistant) = &mut Arc::make_mut(last).body
            {
                assistant.markdown = Arc::from(text);
                assistant.status = RowStatus::Running;
            } else {
                nested.rows.push_back(Arc::new(TranscriptRow {
                    id: next_row_id,
                    ordinal: next_ordinal,
                    loop_id: None,
                    body: RowBody::Assistant(AssistantRow {
                        markdown: Arc::from(text),
                        status: RowStatus::Running,
                        suggestions: Arc::from([]),
                    }),
                }));
            }
            tool.subagent = Some(nested);
        });
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_gate(
        &mut self,
        request_id: RequestId,
        session_epoch: SessionEpoch,
        purpose: tact_protocol::UiRequestPurpose,
        kind: GateKind,
        prompt: String,
        options: Vec<String>,
        origin: tact_protocol::UiRequestOrigin,
    ) -> RowId {
        if let Some(row) = self.gates.get(&request_id) {
            return *row;
        }
        let row = self.alloc_id();
        let gate = GateCard {
            id: row,
            request_id,
            session_epoch,
            purpose,
            kind,
            prompt: Arc::from(prompt),
            options: options
                .into_iter()
                .map(Arc::from)
                .collect::<Vec<_>>()
                .into(),
            selection: match kind {
                GateKind::Select => GateSelection::Single(None),
                GateKind::MultiSelect => GateSelection::Multiple(Vec::new()),
            },
            origin: GateOrigin::from(origin.clone()),
            status: GateState::Pending,
        };
        let group = match &gate.origin {
            GateOrigin::Tool { tool_id } => self.latest_tool(tool_id).map(|(group, _)| *group),
            GateOrigin::Main | GateOrigin::Subagent { .. } => None,
        };
        if let Some(group) = group {
            self.append_group_item(group, ToolGroupItem::Gate(gate));
            self.gates.insert(request_id, row);
            group
        } else {
            self.push_existing(row, None, RowBody::Gate(gate));
            self.gates.insert(request_id, row);
            row
        }
    }

    fn top_level_group_for(&self, nested: RowId) -> Option<RowId> {
        self.rows.iter().find_map(|row| match &row.body {
            RowBody::ToolGroup(group) if group.items.iter().any(|item| item.id() == nested) => {
                Some(row.id)
            }
            _ => None,
        })
    }

    fn update_gate(&mut self, id: RequestId, update: impl FnOnce(&mut GateCard)) {
        let Some(row_id) = self.gates.get(&id).copied() else {
            return;
        };
        if let Some(group_index) = self.rows.iter().position(|row| {
            matches!(
                &row.body,
                RowBody::ToolGroup(group) if group.items.iter().any(|item| item.id() == row_id)
            )
        }) {
            let mut group = match &self.rows[group_index].body {
                RowBody::ToolGroup(group) => group.clone(),
                _ => return,
            };
            let mut items = group.items.to_vec();
            if let Some(item) = items.iter_mut().find(|item| item.id() == row_id)
                && let ToolGroupItem::Gate(gate) = item
            {
                update(gate);
            }
            group.items = Arc::from(items);
            let mut row = (*self.rows[group_index]).clone();
            row.body = RowBody::ToolGroup(group);
            self.rows.set(group_index, Arc::new(row));
            return;
        }
        let Some(index) = self.rows.iter().position(|row| row.id == row_id) else {
            return;
        };
        let mut next = (*self.rows[index]).clone();
        if let RowBody::Gate(gate) = &mut next.body {
            update(gate);
            self.rows.set(index, Arc::new(next));
        }
    }

    fn gate(&self, id: RequestId) -> Option<&GateCard> {
        let row_id = self.gates.get(&id)?;
        for row in &self.rows {
            match &row.body {
                RowBody::Gate(gate) if gate.id == *row_id => return Some(gate),
                RowBody::ToolGroup(group) => {
                    if let Some(ToolGroupItem::Gate(gate)) =
                        group.items.iter().find(|item| item.id() == *row_id)
                    {
                        return Some(gate);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn gate_state(&self, id: RequestId) -> Option<GateState> {
        self.gate(id).map(|gate| gate.status)
    }

    fn update_group(&mut self, row: RowId, update: impl FnOnce(&mut ToolGroupRow)) {
        if let Some(index) = self.rows.iter().position(|candidate| candidate.id == row) {
            let mut next = (*self.rows[index]).clone();
            if let RowBody::ToolGroup(group) = &mut next.body {
                update(group);
                self.rows.set(index, Arc::new(next));
            }
        }
    }

    fn settle_group(&mut self, row: RowId, finish_run: bool) {
        self.update_group(row, |group| {
            let status = if group.items.iter().any(|item| {
                matches!(item, ToolGroupItem::Tool(tool) if tool.status == RowStatus::Failed)
            }) {
                RowStatus::Failed
            } else if group.items.iter().any(|item| {
                matches!(
                    item,
                    ToolGroupItem::Tool(tool)
                        if tool.status == RowStatus::Running
                            && (!finish_run || tool.keep_live)
                )
            }) {
                RowStatus::Running
            } else {
                RowStatus::Success
            };
            group.status = status;
            group.collapsed = status == RowStatus::Success;
        });
    }

    fn cancel_group(&mut self, row: RowId) {
        self.update_group(row, |group| {
            group.status = RowStatus::Cancelled;
            group.collapsed = false;
            let mut items = group.items.to_vec();
            for item in &mut items {
                match item {
                    ToolGroupItem::Tool(tool)
                        if matches!(tool.status, RowStatus::Pending | RowStatus::Running) =>
                    {
                        tool.status = RowStatus::Cancelled;
                    }
                    ToolGroupItem::Gate(gate)
                        if matches!(gate.status, GateState::Pending | GateState::Submitting) =>
                    {
                        gate.status = GateState::Withdrawn;
                    }
                    _ => {}
                }
            }
            group.items = Arc::from(items);
        });
    }

    fn append_group_item(&mut self, group_id: RowId, item: ToolGroupItem) {
        self.update_group(group_id, |group| {
            let mut items = group.items.to_vec();
            items.push(item);
            group.items = Arc::from(items);
            group.summary = Arc::from(format!("{} items", group.items.len()));
        });
    }

    fn update_tool_by_row(&mut self, row_id: RowId, update: impl FnOnce(&mut ToolRow)) {
        let Some((group_index, mut group)) =
            self.rows
                .iter()
                .enumerate()
                .find_map(|(index, row)| match &row.body {
                    RowBody::ToolGroup(group)
                        if group.items.iter().any(|item| item.id() == row_id) =>
                    {
                        Some((index, group.clone()))
                    }
                    _ => None,
                })
        else {
            return;
        };
        let mut items = group.items.to_vec();
        if let Some(item) = items.iter_mut().find(|item| item.id() == row_id)
            && let ToolGroupItem::Tool(tool) = item
        {
            update(tool);
        }
        group.items = Arc::from(items);
        let mut row = (*self.rows[group_index]).clone();
        row.body = RowBody::ToolGroup(group);
        self.rows.set(group_index, Arc::new(row));
    }

    fn update_tool_in_group(
        &mut self,
        group_id: RowId,
        tool_id: RowId,
        update: impl FnOnce(&mut ToolRow),
    ) {
        let Some(group_index) = self.rows.iter().position(|row| row.id == group_id) else {
            return;
        };
        let mut group = match &self.rows[group_index].body {
            RowBody::ToolGroup(group) => group.clone(),
            _ => return,
        };
        let mut items = group.items.to_vec();
        if let Some(ToolGroupItem::Tool(tool)) = items.iter_mut().find(|item| item.id() == tool_id)
        {
            update(tool);
        } else {
            return;
        }
        group.items = Arc::from(items);
        let mut row = (*self.rows[group_index]).clone();
        row.body = RowBody::ToolGroup(group);
        self.rows.set(group_index, Arc::new(row));
    }

    fn append_assistant(&mut self, row: RowId, text: &str) {
        self.update_assistant(row, |assistant| {
            assistant.markdown = Arc::from(format!("{}{}", assistant.markdown, text));
        });
    }

    fn update_assistant(&mut self, row: RowId, update: impl FnOnce(&mut AssistantRow)) {
        if let Some(index) = self.rows.iter().position(|candidate| candidate.id == row) {
            let mut next = (*self.rows[index]).clone();
            if let RowBody::Assistant(assistant) = &mut next.body {
                update(assistant);
                self.rows.set(index, Arc::new(next));
            }
        }
    }

    fn append_thinking(&mut self, row: RowId, text: &str) {
        self.update_thinking(row, |thinking| {
            thinking.markdown = Arc::from(format!("{}{}", thinking.markdown, text));
        });
    }

    fn update_thinking(&mut self, row: RowId, update: impl FnOnce(&mut ThinkingRow)) {
        if let Some(index) = self.rows.iter().position(|candidate| candidate.id == row) {
            let mut next = (*self.rows[index]).clone();
            if let RowBody::Thinking(thinking) = &mut next.body {
                update(thinking);
                self.rows.set(index, Arc::new(next));
            }
        }
    }

    fn move_empty_assistant_to_tail(&mut self) {
        let Some(assistant) = self.active_assistant else {
            return;
        };
        let empty = self.rows.iter().any(|candidate| {
            candidate.id == assistant
                && matches!(
                    &candidate.body,
                    RowBody::Assistant(row) if row.markdown.is_empty()
                )
        });
        if empty
            && let Some(index) = self
                .rows
                .iter()
                .position(|candidate| candidate.id == assistant)
        {
            let assistant_row = self.rows.remove(index);
            self.rows.push_back(assistant_row);
        }
    }

    fn update_hook_status(&mut self, row: RowId, update: impl FnOnce(&mut HookStatusRow)) {
        if let Some(index) = self.rows.iter().position(|candidate| candidate.id == row) {
            let mut next = (*self.rows[index]).clone();
            if let RowBody::HookStatus(status) = &mut next.body {
                update(status);
                self.rows.set(index, Arc::new(next));
            }
        }
    }

    fn current_loop(&self) -> Option<LoopId> {
        self.active_loop.or_else(|| {
            self.active_group.and_then(|group| {
                self.rows.iter().find_map(|row| match &row.body {
                    RowBody::ToolGroup(group_row) if row.id == group => Some(group_row.loop_id),
                    _ => None,
                })
            })
        })
    }

    fn alloc_id(&mut self) -> RowId {
        let id = RowId(self.next_row_id);
        self.next_row_id += 1;
        id
    }

    fn push_row(&mut self, loop_id: Option<LoopId>, body: RowBody) -> RowId {
        let id = self.alloc_id();
        self.push_existing(id, loop_id, body);
        id
    }

    fn push_existing(&mut self, id: RowId, loop_id: Option<LoopId>, body: RowBody) {
        let ordinal = self.ordinal;
        self.ordinal += 1;
        self.rows.push_back(Arc::new(TranscriptRow {
            id,
            ordinal,
            loop_id,
            body,
        }));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateResponseError {
    Unknown,
    Stale,
    KindMismatch,
    AlreadyAnswered,
}

fn message_text(message: &AgentMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            AgentContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn chunks_text(chunks: &[ToolOutputChunk]) -> String {
    chunks.iter().map(|chunk| chunk.text.as_str()).collect()
}

fn output_view(text: &str) -> ToolOutputView {
    const PREVIEW_CHARS: usize = 4_000;
    let chars = text.chars().count();
    ToolOutputView {
        preview: Arc::from(text.chars().take(PREVIEW_CHARS).collect::<String>()),
        detail: Some(Arc::from(text)),
        truncated: chars > PREVIEW_CHARS,
    }
}

fn append_output(tool: &mut ToolRow, text: &str) {
    let combined = format!("{}{}", tool.output.detail.as_deref().unwrap_or(""), text);
    tool.output = output_view(&combined);
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tact_agent_api::AgentContentBlock;
    use tact_protocol::{
        StepResult, ToolPresentationInfo, UiAgentRef, UiRequestOrigin, UiRequestPurpose, UiResponse,
    };

    use super::*;
    use crate::gates::{PendingUiRequest, PendingUiRequestKind};

    fn context(epoch: u64) -> ReduceContext {
        ReduceContext {
            session_epoch: SessionEpoch(epoch),
        }
    }

    fn run_started(reducer: &mut TranscriptReducer, id: &str) {
        reducer.apply_agent_update(
            AgentUpdate::RunStarted {
                run_id: id.to_string(),
            },
            &context(1),
        );
    }

    fn turn(reducer: &mut TranscriptReducer) {
        reducer.apply_agent_update(
            AgentUpdate::TurnStats {
                turns_taken: 1,
                max_turns: None,
            },
            &context(1),
        );
    }

    fn step_started(tool_id: &str) -> AgentUpdate {
        AgentUpdate::StepStarted {
            idx: 0,
            tool_id: tool_id.to_string(),
            tool_name: "read_file".into(),
            arg_summary: "README.md".into(),
            arg_full: "README.md".into(),
            presentation: ToolPresentationInfo::generic("read_file"),
        }
    }

    fn group_rows(reducer: &TranscriptReducer) -> Vec<Arc<TranscriptRow>> {
        reducer
            .rows
            .iter()
            .filter(|row| matches!(row.body, RowBody::ToolGroup(_)))
            .cloned()
            .collect()
    }

    #[test]
    fn run_started_and_message_started_keep_stable_rows() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(
            AgentUpdate::MessageStarted {
                message_id: "message-1".into(),
            },
            &context(1),
        );
        assert!(reducer.active_assistant.is_none());
        reducer.apply_agent_update(AgentUpdate::StreamChunk("hello".into()), &context(1));
        let first = reducer.active_assistant.expect("assistant row");
        reducer.apply_agent_update(
            AgentUpdate::MessageStarted {
                message_id: "message-1".into(),
            },
            &context(1),
        );
        assert_eq!(reducer.active_assistant, Some(first));
        assert_eq!(
            reducer
                .view()
                .iter()
                .filter(|row| matches!(row.body, RowBody::Assistant(_)))
                .count(),
            1
        );
    }

    #[test]
    fn run_finished_closes_the_run_cursor() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-1"), &context(1));
        reducer.apply_agent_update(
            AgentUpdate::RunFinished {
                run_id: "run-1".into(),
            },
            &context(1),
        );
        assert!(reducer.pending_run.is_none());
        assert!(reducer.active_run.is_none());
        assert!(reducer.active_group.is_none());
        let group = &group_rows(&reducer)[0];
        assert!(matches!(
            &group.body,
            RowBody::ToolGroup(group) if group.status == RowStatus::Success && group.collapsed
        ));
    }

    #[test]
    fn compaction_run_does_not_open_an_activation() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "compact-1");
        reducer.apply_agent_update(
            AgentUpdate::RunFinished {
                run_id: "compact-1".into(),
            },
            &context(1),
        );
        assert_eq!(reducer.total_rows(), 0);
    }

    #[test]
    fn turn_stats_allocates_one_tool_group() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        assert_eq!(group_rows(&reducer).len(), 0);
        reducer.apply_agent_update(step_started("tool-1"), &context(1));
        assert_eq!(group_rows(&reducer).len(), 1);
    }

    #[test]
    fn session_reset_keeps_row_ids_monotonic() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-1"), &context(1));
        let old = reducer.rows.iter().map(|row| row.id).max().unwrap();
        reducer.reset_for_session();
        let new = reducer.push_row(
            None,
            RowBody::Notice(NoticeRow {
                text: Arc::from("new session"),
            }),
        );
        assert!(new > old);
    }

    #[test]
    fn stream_chunks_append_to_one_assistant_row() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        reducer.apply_batch(
            [
                AgentUpdate::StreamChunk("Hello".into()),
                AgentUpdate::StreamChunk(" world".into()),
            ],
            &context(1),
        );
        let assistants: Vec<_> = reducer
            .rows
            .iter()
            .filter_map(|row| match &row.body {
                RowBody::Assistant(assistant) => Some(assistant.markdown.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(assistants, vec!["Hello world"]);
    }

    #[test]
    fn thinking_precedes_the_assistant_row_it_belongs_to() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(
            AgentUpdate::MessageStarted {
                message_id: "message-1".into(),
            },
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Started),
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Delta("reason".into())),
            &context(1),
        );
        reducer.apply_agent_update(AgentUpdate::StreamChunk("answer".into()), &context(1));
        let kinds = reducer
            .rows
            .iter()
            .map(|row| match row.body {
                RowBody::Thinking(_) => "thinking",
                RowBody::Assistant(_) => "assistant",
                _ => "other",
            })
            .collect::<Vec<_>>();
        assert_eq!(kinds, vec!["thinking", "assistant"]);
    }

    #[test]
    fn thinking_lifecycle_creates_one_row_and_finishes() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Started),
            &context(1),
        );
        let id = reducer.active_thinking.expect("thinking row");
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Delta("think".into())),
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Finished),
            &context(1),
        );
        let thinking: Vec<_> = reducer
            .rows
            .iter()
            .filter(|row| matches!(row.body, RowBody::Thinking(_)))
            .collect();
        assert_eq!(thinking.len(), 1);
        assert_eq!(thinking[0].id, id);
        assert!(matches!(
            &thinking[0].body,
            RowBody::Thinking(row) if row.markdown.as_ref() == "think"
                && row.status == RowStatus::Success
        ));
    }

    #[test]
    fn tool_progress_batches_do_not_duplicate_tool_rows() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-1"), &context(1));
        for text in ["one", "two", "three"] {
            reducer.apply_agent_update(
                AgentUpdate::ToolProgress {
                    run_id: Some("run-1".into()),
                    tool_id: "tool-1".into(),
                    chunks: vec![ToolOutputChunk::stdout(text)],
                },
                &context(1),
            );
        }
        let groups = group_rows(&reducer);
        let tools: Vec<_> = match &groups[0].body {
            RowBody::ToolGroup(group) => group
                .items
                .iter()
                .filter(|item| matches!(item, ToolGroupItem::Tool(_)))
                .collect(),
            _ => unreachable!(),
        };
        assert_eq!(tools.len(), 1);
        assert!(matches!(
            tools[0],
            ToolGroupItem::Tool(tool) if tool.output.detail.as_deref() == Some("onetwothree")
        ));
    }

    #[test]
    fn tool_group_includes_gate_and_subagent_rows() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-1"), &context(1));
        reconcile_select(
            &mut reducer,
            7,
            "Approve?",
            vec!["Allow".into()],
            UiRequestPurpose::Permission,
            UiRequestOrigin {
                agent: UiAgentRef::Main,
                tool_id: Some("tool-1".into()),
            },
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::SubagentFinished {
                tool_id: "tool-1".into(),
                child_id: "child-1".into(),
                success: true,
                summary: "done".into(),
            },
            &context(1),
        );
        let group = &group_rows(&reducer)[0];
        assert!(matches!(
            &group.body,
            RowBody::ToolGroup(row) if row.items.len() == 2
        ));
    }

    #[test]
    fn a_gate_nested_in_a_tool_group_tracks_its_response_state() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-1"), &context(1));
        reconcile_select(
            &mut reducer,
            1,
            "Pick",
            vec!["a".into(), "b".into()],
            UiRequestPurpose::Question,
            UiRequestOrigin {
                agent: UiAgentRef::Main,
                tool_id: Some("tool-1".into()),
            },
            &context(1),
        );
        reducer
            .apply_gate_response(
                SessionEpoch(1),
                &UiResponse::Select {
                    request_id: 1,
                    choice: None,
                },
            )
            .unwrap();
        let RowBody::ToolGroup(group) = &group_rows(&reducer)[0].body else {
            panic!("expected a tool group");
        };
        let gate = group.items.iter().find_map(|item| match item {
            ToolGroupItem::Gate(gate) => Some(gate),
            ToolGroupItem::Tool(_) => None,
        });
        assert!(matches!(
            gate,
            Some(gate) if gate.status == GateState::Cancelled
        ));
    }

    #[test]
    fn standalone_gate_and_diff_have_top_level_rows() {
        let mut reducer = TranscriptReducer::new();
        reducer.push_diff(Some(Arc::from("a.rs")), "@@ patch");
        reconcile_select(
            &mut reducer,
            1,
            "Plan?",
            vec!["Approve".into()],
            UiRequestPurpose::PlanApproval,
            UiRequestOrigin::main(),
            &context(1),
        );
        assert!(
            reducer
                .rows
                .iter()
                .any(|row| matches!(row.body, RowBody::Diff(_)))
        );
        assert!(
            reducer
                .rows
                .iter()
                .any(|row| matches!(row.body, RowBody::Gate(_)))
        );
    }

    fn pending(request_id: u64, prompt: &str) -> PendingUiRequest {
        PendingUiRequest {
            request_id,
            prompt: prompt.into(),
            options: vec!["Allow".into()],
            kind: PendingUiRequestKind::Select,
            log_confirm: false,
            from_subagent: false,
            purpose: UiRequestPurpose::Permission,
            origin: UiRequestOrigin::main(),
        }
    }

    fn reconcile_select(
        reducer: &mut TranscriptReducer,
        request_id: u64,
        prompt: &str,
        options: Vec<String>,
        purpose: UiRequestPurpose,
        origin: UiRequestOrigin,
        context: &ReduceContext,
    ) {
        reducer.reconcile_gates(
            &[PendingUiRequest {
                request_id,
                prompt: prompt.into(),
                options,
                kind: PendingUiRequestKind::Select,
                log_confirm: false,
                from_subagent: matches!(origin.agent, UiAgentRef::Subagent { .. }),
                purpose,
                origin,
            }],
            context,
        );
    }

    fn reconcile_multi(
        reducer: &mut TranscriptReducer,
        request_id: u64,
        prompt: &str,
        options: Vec<String>,
        purpose: UiRequestPurpose,
        origin: UiRequestOrigin,
        context: &ReduceContext,
    ) {
        reducer.reconcile_gates(
            &[PendingUiRequest {
                request_id,
                prompt: prompt.into(),
                options,
                kind: PendingUiRequestKind::MultiSelect,
                log_confirm: false,
                from_subagent: matches!(origin.agent, UiAgentRef::Subagent { .. }),
                purpose,
                origin,
            }],
            context,
        );
    }

    #[test]
    fn stale_gate_hint_is_repaired_from_snapshot() {
        let mut reducer = TranscriptReducer::new();
        reducer.apply_agent_update(
            AgentUpdate::RequestSelect {
                request_id: 1,
                prompt: "stale hint".into(),
                options: vec![],
                log_confirm: false,
                from_subagent: false,
                purpose: UiRequestPurpose::Other,
                origin: UiRequestOrigin::main(),
            },
            &context(1),
        );
        reducer.reconcile_gates(&[pending(1, "authoritative")], &context(1));
        assert_eq!(
            reducer.gate(RequestId(1)).unwrap().prompt.as_ref(),
            "authoritative"
        );
        assert_eq!(
            reducer.gate(RequestId(1)).unwrap().status,
            GateState::Pending
        );
    }

    #[test]
    fn resolved_gate_collapses_without_losing_history() {
        let mut reducer = TranscriptReducer::new();
        reconcile_select(
            &mut reducer,
            1,
            "Answer",
            vec!["Allow".into()],
            UiRequestPurpose::Permission,
            UiRequestOrigin::main(),
            &context(1),
        );
        reducer
            .apply_gate_response(
                SessionEpoch(1),
                &UiResponse::Select {
                    request_id: 1,
                    choice: Some(0),
                },
            )
            .unwrap();
        reducer.reconcile_gates(&[], &context(1));
        assert_eq!(
            reducer.gate(RequestId(1)).unwrap().status,
            GateState::Resolved
        );
        assert_eq!(reducer.total_rows(), 1);
    }

    #[test]
    fn session_epoch_rejects_stale_gate_response() {
        let mut reducer = TranscriptReducer::new();
        reconcile_select(
            &mut reducer,
            1,
            "Answer",
            vec!["Allow".into()],
            UiRequestPurpose::Permission,
            UiRequestOrigin::main(),
            &context(1),
        );
        assert_eq!(
            reducer.apply_gate_response(
                SessionEpoch(2),
                &UiResponse::Select {
                    request_id: 1,
                    choice: Some(0),
                },
            ),
            Err(GateResponseError::Stale)
        );
        assert_eq!(
            reducer.gate(RequestId(1)).unwrap().status,
            GateState::Pending
        );
    }

    #[test]
    fn host_rejected_response_marks_the_gate_stale() {
        let mut reducer = TranscriptReducer::new();
        reconcile_select(
            &mut reducer,
            1,
            "Answer",
            vec!["Allow".into()],
            UiRequestPurpose::Permission,
            UiRequestOrigin::main(),
            &context(1),
        );
        reducer
            .apply_gate_response(
                SessionEpoch(1),
                &UiResponse::Select {
                    request_id: 1,
                    choice: Some(0),
                },
            )
            .unwrap();
        reducer.apply_agent_update(
            AgentUpdate::UiResponseRejected { request_id: 1 },
            &context(1),
        );
        assert_eq!(reducer.gate(RequestId(1)).unwrap().status, GateState::Stale);
        reducer.reconcile_gates(&[], &context(1));
        assert_eq!(reducer.gate(RequestId(1)).unwrap().status, GateState::Stale);
    }

    #[test]
    fn gate_origin_is_not_inferred_from_prompt_text() {
        let mut reducer = TranscriptReducer::new();
        reconcile_select(
            &mut reducer,
            1,
            "from subagent: approve",
            vec!["Allow".into()],
            UiRequestPurpose::Question,
            UiRequestOrigin::main(),
            &context(1),
        );
        assert_eq!(reducer.gate(RequestId(1)).unwrap().origin, GateOrigin::Main);
    }

    #[test]
    fn subagent_gate_keeps_child_and_tool_attribution() {
        let mut reducer = TranscriptReducer::new();
        reconcile_select(
            &mut reducer,
            1,
            "Approve child edit",
            vec!["Allow".into()],
            UiRequestPurpose::Permission,
            UiRequestOrigin::subagent("child-1", Some("tool-9".into())),
            &context(1),
        );
        assert_eq!(
            reducer.gate(RequestId(1)).unwrap().origin,
            GateOrigin::Subagent {
                child_id: Arc::from("child-1"),
                tool_id: Some(Arc::from("tool-9")),
            }
        );
    }

    #[test]
    fn persisted_assistant_tool_uses_form_one_group() {
        let mut reducer = TranscriptReducer::new();
        let message = AgentMessage::new(
            AgentRole::Assistant,
            MessageOrigin::Assistant,
            vec![
                AgentContentBlock::ToolUse {
                    id: "tool-1".into(),
                    name: "read_file".into(),
                    input: json!({"path": "a.rs"}),
                },
                AgentContentBlock::ToolUse {
                    id: "tool-2".into(),
                    name: "read_file".into(),
                    input: json!({"path": "b.rs"}),
                },
            ],
        );
        reducer.seed_persisted(&[message], &context(1));
        let groups = group_rows(&reducer);
        assert_eq!(groups.len(), 1);
        assert!(matches!(
            &groups[0].body,
            RowBody::ToolGroup(row) if row.items.len() == 2
        ));
    }

    #[test]
    fn persisted_history_preserves_thinking_notices_and_message_groups() {
        let mut reducer = TranscriptReducer::new();
        let assistant = |tool_id: &str| {
            AgentMessage::new(
                AgentRole::Assistant,
                MessageOrigin::Assistant,
                vec![
                    AgentContentBlock::Thinking {
                        thinking: format!("thinking {tool_id}"),
                        signature: String::new(),
                    },
                    AgentContentBlock::Text {
                        text: format!("answer {tool_id}"),
                    },
                    AgentContentBlock::ToolUse {
                        id: tool_id.into(),
                        name: "read_file".into(),
                        input: json!({"path": "a.rs"}),
                    },
                ],
            )
        };
        reducer.seed_persisted(
            &[
                assistant("tool-1"),
                AgentMessage::text(AgentRole::User, MessageOrigin::GoalContext, "goal context"),
                assistant("tool-2"),
            ],
            &context(1),
        );
        let view = reducer.view();
        assert_eq!(
            view.iter()
                .filter(|row| matches!(row.body, RowBody::Thinking(_)))
                .count(),
            2
        );
        assert_eq!(
            view.iter()
                .filter(|row| matches!(row.body, RowBody::Notice(_)))
                .count(),
            1
        );
        assert_eq!(group_rows(&reducer).len(), 2);
    }

    #[test]
    fn persisted_tool_results_pair_by_tool_use_id() {
        let mut reducer = TranscriptReducer::new();
        let assistant = AgentMessage::new(
            AgentRole::Assistant,
            MessageOrigin::Assistant,
            vec![AgentContentBlock::ToolUse {
                id: "tool-1".into(),
                name: "read_file".into(),
                input: json!({"path": "a.rs"}),
            }],
        );
        let result = AgentMessage::new(
            AgentRole::User,
            MessageOrigin::ToolResult,
            vec![AgentContentBlock::ToolResult {
                tool_use_id: "tool-1".into(),
                content: "file body".into(),
            }],
        );
        reducer.seed_persisted(&[assistant, result], &context(1));
        let group = &group_rows(&reducer)[0];
        assert!(matches!(
            &group.body,
            RowBody::ToolGroup(row)
                if matches!(
                    &row.items[0],
                    ToolGroupItem::Tool(tool)
                        if tool.output.detail.as_deref() == Some("file body")
                )
        ));
    }

    #[test]
    fn persisted_injected_context_is_not_a_user_turn() {
        let mut reducer = TranscriptReducer::new();
        reducer.seed_persisted(
            &[AgentMessage::text(
                AgentRole::User,
                MessageOrigin::InjectedContext {
                    source: "hook".into(),
                },
                "briefing",
            )],
            &context(1),
        );
        assert!(
            !reducer
                .rows
                .iter()
                .any(|row| matches!(row.body, RowBody::User(_)))
        );
        assert!(
            reducer
                .rows
                .iter()
                .any(|row| matches!(row.body, RowBody::HookContext(_)))
        );
    }

    #[test]
    fn message_started_ids_keep_assistant_rows_distinct() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        for (message_id, text) in [("message-1", "first"), ("message-2", "second")] {
            reducer.apply_agent_update(
                AgentUpdate::MessageStarted {
                    message_id: message_id.into(),
                },
                &context(1),
            );
            reducer.apply_agent_update(AgentUpdate::StreamChunk(text.into()), &context(1));
        }
        let assistants = reducer
            .rows
            .iter()
            .filter_map(|row| match &row.body {
                RowBody::Assistant(assistant) => Some(assistant.markdown.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(assistants, ["first", "second"]);
    }

    #[test]
    fn a_new_thinking_started_finalizes_the_previous_block() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        for text in ["first", "second"] {
            reducer.apply_agent_update(
                AgentUpdate::ThinkingChunk(ThinkingChunk::Started),
                &context(1),
            );
            reducer.apply_agent_update(
                AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(text.into())),
                &context(1),
            );
        }
        let thinking = reducer
            .rows
            .iter()
            .filter_map(|row| match &row.body {
                RowBody::Thinking(thinking) => Some(thinking.markdown.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(thinking, ["first", "second"]);
    }

    #[test]
    fn a_new_message_finalizes_thinking_when_the_started_hint_was_dropped() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(
            AgentUpdate::MessageStarted {
                message_id: "message-1".into(),
            },
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Delta("a-body".into())),
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::MessageStarted {
                message_id: "message-2".into(),
            },
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Delta("b-body".into())),
            &context(1),
        );
        let thinking = reducer
            .rows
            .iter()
            .filter_map(|row| match &row.body {
                RowBody::Thinking(thinking) => Some(thinking.markdown.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(thinking, ["a-body", "b-body"]);
    }

    #[test]
    fn tool_identity_is_scoped_by_run() {
        let mut reducer = TranscriptReducer::new();
        for run in ["run-1", "run-2"] {
            run_started(&mut reducer, run);
            turn(&mut reducer);
            reducer.apply_agent_update(step_started("tool-1"), &context(1));
            reducer
                .apply_agent_update(AgentUpdate::RunFinished { run_id: run.into() }, &context(1));
        }
        let tools = group_rows(&reducer)
            .iter()
            .flat_map(|row| match &row.body {
                RowBody::ToolGroup(group) => group.items.iter().collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .filter(|item| matches!(item, ToolGroupItem::Tool(_)))
            .count();
        assert_eq!(tools, 2);
    }

    #[test]
    fn late_tool_progress_from_an_old_run_is_ignored() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-1"), &context(1));
        reducer.apply_agent_update(
            AgentUpdate::RunFinished {
                run_id: "run-1".into(),
            },
            &context(1),
        );
        run_started(&mut reducer, "run-2");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-1"), &context(1));
        reducer.apply_agent_update(
            AgentUpdate::ToolProgress {
                run_id: Some("run-1".into()),
                tool_id: "tool-1".into(),
                chunks: vec![ToolOutputChunk::stdout("late old output")],
            },
            &context(1),
        );
        let output = group_rows(&reducer)
            .iter()
            .rev()
            .find_map(|row| match &row.body {
                RowBody::ToolGroup(group) => group.items.iter().find_map(|item| match item {
                    ToolGroupItem::Tool(tool) => tool.output.detail.clone(),
                    ToolGroupItem::Gate(_) => None,
                }),
                _ => None,
            });
        assert_eq!(output, None);
    }

    #[test]
    fn late_step_finished_from_an_old_run_is_ignored() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-1"), &context(1));
        reducer.apply_agent_update(
            AgentUpdate::RunFinished {
                run_id: "run-1".into(),
            },
            &context(1),
        );
        run_started(&mut reducer, "run-2");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-1"), &context(1));
        reducer.apply_agent_update(
            AgentUpdate::StepFinished {
                idx: 0,
                run_id: Some("run-1".into()),
                tool_id: "tool-1".into(),
                result: StepResult {
                    tool: "read_file".into(),
                    arg_summary: "old".into(),
                    arg_full: None,
                    status: StepStatus::Failed,
                    message: "late old failure".into(),
                    detail: None,
                    duration_us: None,
                    permission_label: None,
                    presentation: ToolPresentationInfo::generic("read_file"),
                },
            },
            &context(1),
        );
        let groups = group_rows(&reducer);
        let tool = groups
            .iter()
            .rev()
            .find_map(|row| match &row.body {
                RowBody::ToolGroup(group) => group.items.iter().find_map(|item| match item {
                    ToolGroupItem::Tool(tool) => Some(tool),
                    ToolGroupItem::Gate(_) => None,
                }),
                _ => None,
            })
            .expect("new run tool");
        assert_eq!(tool.status, RowStatus::Running);
        assert!(tool.output.detail.is_none());
    }

    #[test]
    fn late_step_finished_for_unknown_tool_is_a_notice() {
        let mut reducer = TranscriptReducer::new();
        reducer.apply_agent_update(
            AgentUpdate::StepFinished {
                idx: 0,
                run_id: None,
                tool_id: "missing".into(),
                result: StepResult {
                    tool: "read_file".into(),
                    arg_summary: "README.md".into(),
                    arg_full: None,
                    status: StepStatus::Success,
                    message: "late".into(),
                    detail: None,
                    duration_us: None,
                    permission_label: None,
                    presentation: ToolPresentationInfo::generic("read_file"),
                },
            },
            &context(1),
        );
        assert!(
            reducer
                .rows
                .iter()
                .any(|row| matches!(row.body, RowBody::Notice(_)))
        );
        assert!(
            !reducer
                .rows
                .iter()
                .any(|row| matches!(row.body, RowBody::ToolGroup(_)))
        );
    }

    #[test]
    fn keep_live_step_finished_remains_running() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        let mut presentation = ToolPresentationInfo::generic("background_run");
        presentation.keep_live = true;
        reducer.apply_agent_update(
            AgentUpdate::StepStarted {
                idx: 0,
                tool_id: "tool-1".into(),
                tool_name: "background_run".into(),
                arg_summary: "sleep 10".into(),
                arg_full: "sleep 10".into(),
                presentation: presentation.clone(),
            },
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::StepFinished {
                idx: 0,
                run_id: None,
                tool_id: "tool-1".into(),
                result: StepResult {
                    tool: "background_run".into(),
                    arg_summary: "sleep 10".into(),
                    arg_full: None,
                    status: StepStatus::Success,
                    message: "started".into(),
                    detail: None,
                    duration_us: None,
                    permission_label: None,
                    presentation,
                },
            },
            &context(1),
        );
        let status = group_rows(&reducer)
            .iter()
            .find_map(|row| match &row.body {
                RowBody::ToolGroup(group) => group.items.iter().find_map(|item| match item {
                    ToolGroupItem::Tool(tool) => Some(tool.status),
                    _ => None,
                }),
                _ => None,
            })
            .unwrap();
        assert_eq!(status, RowStatus::Running);
    }

    #[test]
    fn gate_response_kind_must_match_and_stale_epoch_is_a_no_op() {
        let mut reducer = TranscriptReducer::new();
        reconcile_multi(
            &mut reducer,
            1,
            "Pick",
            vec!["a".into(), "b".into()],
            UiRequestPurpose::Question,
            UiRequestOrigin::main(),
            &context(2),
        );
        assert_eq!(
            reducer.apply_gate_response(
                SessionEpoch(2),
                &UiResponse::Select {
                    request_id: 1,
                    choice: Some(0),
                },
            ),
            Err(GateResponseError::KindMismatch)
        );
        assert_eq!(
            reducer.apply_gate_response(
                SessionEpoch(1),
                &UiResponse::MultiSelect {
                    request_id: 1,
                    choices: Some(vec![0]),
                },
            ),
            Err(GateResponseError::Stale)
        );
        assert_eq!(
            reducer.gate(RequestId(1)).unwrap().status,
            GateState::Pending
        );
    }

    #[test]
    fn an_answered_gate_is_not_reopened_by_a_stale_snapshot() {
        let mut reducer = TranscriptReducer::new();
        reconcile_select(
            &mut reducer,
            1,
            "Allow",
            vec!["yes".into()],
            UiRequestPurpose::Permission,
            UiRequestOrigin::main(),
            &context(1),
        );
        reducer
            .apply_gate_response(
                SessionEpoch(1),
                &UiResponse::Select {
                    request_id: 1,
                    choice: Some(0),
                },
            )
            .unwrap();
        reducer.reconcile_gates(&[pending(1, "Allow")], &context(1));
        assert_eq!(
            reducer.gate(RequestId(1)).unwrap().status,
            GateState::Submitting
        );
        reducer.reconcile_gates(&[], &context(1));
        assert_eq!(
            reducer.gate(RequestId(1)).unwrap().status,
            GateState::Resolved
        );
    }

    #[test]
    fn a_cancelled_gate_is_not_reopened_by_a_late_snapshot() {
        let mut reducer = TranscriptReducer::new();
        reconcile_select(
            &mut reducer,
            1,
            "Pick",
            vec!["a".into(), "b".into()],
            UiRequestPurpose::Question,
            UiRequestOrigin::main(),
            &context(1),
        );
        reducer
            .apply_gate_response(
                SessionEpoch(1),
                &UiResponse::Select {
                    request_id: 1,
                    choice: None,
                },
            )
            .unwrap();
        reducer.reconcile_gates(&[pending(1, "Pick")], &context(1));
        assert_eq!(
            reducer.gate(RequestId(1)).unwrap().status,
            GateState::Cancelled
        );
    }

    #[test]
    fn a_stale_gate_is_not_reopened_by_a_late_snapshot() {
        let mut reducer = TranscriptReducer::new();
        reconcile_select(
            &mut reducer,
            1,
            "Pick",
            vec!["a".into()],
            UiRequestPurpose::Question,
            UiRequestOrigin::main(),
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::UiResponseRejected { request_id: 1 },
            &context(1),
        );
        reducer.reconcile_gates(&[pending(1, "Pick")], &context(1));
        assert_eq!(reducer.gate(RequestId(1)).unwrap().status, GateState::Stale);
    }

    #[test]
    fn subagent_transcript_updates_reach_the_parent_tool_card() {
        use tact_protocol::subagent_transcript::SubagentTranscriptEntry;

        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-1"), &context(1));
        reducer.apply_agent_update(
            AgentUpdate::ToolTranscript {
                tool_id: "tool-1".into(),
                entry: SubagentTranscriptEntry::Info("child says hello".into()),
            },
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::ToolTranscriptStream {
                tool_id: "tool-1".into(),
                text: "streaming child text".into(),
            },
            &context(1),
        );
        let nested = group_rows(&reducer)
            .iter()
            .find_map(|row| match &row.body {
                RowBody::ToolGroup(group) => group.items.iter().find_map(|item| match item {
                    ToolGroupItem::Tool(tool) => tool.subagent.clone(),
                    _ => None,
                }),
                _ => None,
            })
            .expect("subagent transcript");
        assert_eq!(nested.len(), 2);
    }

    #[test]
    fn a_new_run_error_does_not_mutate_the_completed_previous_run() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-1"), &context(1));
        reducer.apply_agent_update(
            AgentUpdate::RunFinished {
                run_id: "run-1".into(),
            },
            &context(1),
        );
        run_started(&mut reducer, "run-2");
        reducer.apply_agent_update(
            AgentUpdate::Error(tact_protocol::AgentErrorKind::Other(
                "new run failed".into(),
            )),
            &context(1),
        );
        assert!(group_rows(&reducer).iter().any(|row| {
            matches!(
                &row.body,
                RowBody::ToolGroup(group) if group.status == RowStatus::Success
            )
        }));
    }
}

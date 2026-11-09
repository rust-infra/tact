use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use tact_agent_api::{AgentContentBlock, AgentMessage, AgentRole, MessageOrigin, MessagePhase};
use tact_protocol::{
    AgentUpdate, StepStatus, ThinkingChunk, ToolOutputChunk, ToolVisualKind, UiResponse,
};

use crate::{
    LatestToolView,
    gates::PendingUiRequest,
    transcript::row::{
        AssistantRow, ErrorRow, GateCard, GateKind, GateOrigin, GateSelection, GateState,
        HookContextRow, HookStatusRow, LoopId, NoticeRow, PlanRow, PlanStep as PlanRowStep,
        RequestId, RowBody, RowId, RowStatus, SessionEpoch, ThinkingRow, ToolBody, ToolGroupItem,
        ToolGroupRow, ToolMetaView, ToolOutputView, ToolRow, TranscriptRow, TranscriptView,
        UsageView, UserRow,
    },
};

#[derive(Debug, Clone)]
pub(crate) struct ReduceContext {
    pub session_epoch: SessionEpoch,
    /// Whether hook-status rows are drawn. A hidden row must not break the
    /// reasoning region it would otherwise appear to split.
    pub show_hook_output: bool,
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

/// The transcript row the Run inspector chip reads.
///
/// Only identity is stored. Status, duration and label come from the row on
/// every snapshot, so a mutation path that forgets to sync cannot make the
/// chip disagree with the card.
#[derive(Clone, Debug)]
struct LatestToolKey {
    tool_id: Arc<str>,
    group: RowId,
    row: RowId,
}

fn update_nested_tool(
    view: &mut TranscriptView,
    tool_id: &str,
    mut update: impl FnMut(&mut ToolRow),
) -> bool {
    for row in view.rows.iter_mut() {
        let RowBody::ToolGroup(group) = &mut Arc::make_mut(row).body else {
            continue;
        };
        let mut items = group.items.to_vec();
        let Some(ToolGroupItem::Tool(tool)) = items.iter_mut().find(
            |item| matches!(item, ToolGroupItem::Tool(tool) if tool.tool_id.as_ref() == tool_id),
        ) else {
            continue;
        };
        update(tool);
        group.items = Arc::from(items);
        if group.collapse_override.is_none() {
            group.collapsed = group.status() != RowStatus::Running;
        }
        return true;
    }
    false
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
    active_plan: Option<RowId>,
    assistant_messages: HashMap<String, RowId>,
    last_finished_rows: Vec<RowId>,
    tools: HashMap<(String, String), (RowId, RowId)>,
    gates: HashMap<RequestId, RowId>,
    hook_status: HashMap<u64, RowId>,
    /// The row the Run inspector chip reads, so its status and duration are
    /// always the transcript row's own values.
    latest_tool: Option<LatestToolKey>,
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
            active_plan: None,
            assistant_messages: HashMap::new(),
            last_finished_rows: Vec::new(),
            tools: HashMap::new(),
            gates: HashMap::new(),
            hook_status: HashMap::new(),
            latest_tool: None,
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
        self.last_finished_rows.clear();
        self.tools.clear();
        self.gates.clear();
        self.hook_status.clear();
        self.latest_tool = None;
    }

    pub fn view(&self) -> TranscriptView {
        TranscriptView {
            rows: self.rows.clone(),
        }
    }

    /// The latest tool the Run inspector shows, read from its transcript row.
    ///
    /// `None` before the run's first `StepQueued` or `StepStarted`, and for a
    /// session seeded from history. The row lookup keeps the chip and the card
    /// on one source of truth, so a replayed start or an interrupted run
    /// cannot leave the chip reporting a status the row no longer has.
    pub(crate) fn latest_tool_view(&self) -> Option<LatestToolView> {
        let latest = self.latest_tool.as_ref()?;
        let row = self
            .rows
            .iter()
            .find(|row| row.id == latest.group)
            .and_then(|row| match &row.body {
                RowBody::ToolGroup(group) => group.items.iter().find_map(|item| match item {
                    ToolGroupItem::Tool(tool) if tool.id == latest.row => Some(tool),
                    _ => None,
                }),
                _ => None,
            })?;
        Some(LatestToolView {
            tool_id: latest.tool_id.clone(),
            group: Some(latest.group),
            row: Some(latest.row),
            label: Arc::from(tool_label(&row.name, &row.arg_summary)),
            duration_ms: row.duration_ms,
            status: row.status,
        })
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
        // A new user turn closes the previous run's terminal-repaint window.
        // The chip belongs to the run that produced the tool, so it clears too.
        self.clear_closed_run_rows();
        self.latest_tool = None;
        self.push_row(
            None,
            RowBody::User(UserRow {
                text: text.into(),
                attachments: attachments.into(),
            }),
        )
    }

    /// Drops terminal-row bookkeeping before a later cancel can revisit it.
    pub(crate) fn clear_closed_run_rows(&mut self) {
        self.last_finished_rows.clear();
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
            group.collapse_override = Some(group.collapsed);
            collapsed = Some(group.collapsed);
        });
        if collapsed.is_some() {
            return collapsed;
        }
        let mut nested = None;
        self.update_nested_group(id, |group| {
            group.collapsed = !group.collapsed;
            group.collapse_override = Some(group.collapsed);
            nested = Some(group.collapsed);
        });
        nested
    }

    pub fn toggle_tool(&mut self, group: RowId, tool: RowId) -> Option<bool> {
        let mut expanded = None;
        self.update_tool_in_group(group, tool, |tool| {
            tool.expanded = !tool.expanded;
            expanded = Some(tool.expanded);
        });
        if expanded.is_some() {
            return expanded;
        }
        let mut nested = None;
        self.update_nested_group(group, |group| {
            let mut items = group.items.to_vec();
            if let Some(ToolGroupItem::Tool(child)) =
                items.iter_mut().find(|item| item.id() == tool)
            {
                child.expanded = !child.expanded;
                nested = Some(child.expanded);
                group.items = Arc::from(items);
            }
        });
        nested
    }

    /// Applies an update to a tool group inside a subagent transcript.
    ///
    /// Nested rows live under their parent tool's `subagent` view, not in the
    /// top-level row list, so a disclosure from the overlay has to find the
    /// parent, update the nested copy, and write it back.
    fn update_nested_group(
        &mut self,
        id: RowId,
        mut update: impl FnMut(&mut ToolGroupRow),
    ) -> bool {
        let Some((parent, mut nested)) = self.rows.iter().find_map(|row| match &row.body {
            RowBody::ToolGroup(group) => group.items.iter().find_map(|item| match item {
                ToolGroupItem::Tool(parent) => {
                    let nested = parent.subagent.as_ref()?;
                    nested
                        .iter()
                        .any(|nested_row| nested_row.id == id)
                        .then(|| (parent.id, nested.clone()))
                }
                ToolGroupItem::Gate(_) => None,
            }),
            _ => None,
        }) else {
            return false;
        };
        let mut changed = false;
        for nested_row in nested.rows.iter_mut() {
            if nested_row.id != id {
                continue;
            }
            let row = Arc::make_mut(nested_row);
            if let RowBody::ToolGroup(group) = &mut row.body {
                update(group);
                changed = true;
                break;
            }
        }
        if changed {
            self.update_tool_by_row(parent, |tool| tool.subagent = Some(nested));
        }
        changed
    }

    pub fn apply_agent_update(&mut self, update: AgentUpdate, context: &ReduceContext) -> RowDelta {
        let mut delta = RowDelta::default();
        // Same rule as the TUI: a reasoning region ends before this turn's
        // own output lands at the transcript tail. The next thinking delta
        // then opens a fresh card instead of growing the one above the new
        // content. An event that adds no visible row — a repeated hook
        // briefing, a hidden hook status — must not split the region: the
        // reader would see two adjacent thought cards with nothing between.
        let ends_region = ends_thinking_region(&update)
            && match &update {
                AgentUpdate::HookContext { source, text } => {
                    !self.hook_context_repeats(source.as_deref(), text)
                }
                AgentUpdate::HookStatus { .. } => context.show_hook_output,
                _ => true,
            };
        if ends_region {
            self.close_thinking(&mut delta);
        }
        match update {
            AgentUpdate::RunStarted { run_id } => {
                self.pending_run = Some(run_id);
            }
            AgentUpdate::RunFinished { run_id } => {
                if self.pending_run.as_deref() == Some(run_id.as_str())
                    && self.active_run.as_deref() != Some(run_id.as_str())
                {
                    self.pending_run = None;
                    return delta;
                }
                if self.active_run.as_deref() == Some(run_id.as_str())
                    || self.pending_run.as_deref() == Some(run_id.as_str())
                {
                    if let Some(group) = self.active_group.take() {
                        self.settle_group(group, true);
                        self.last_finished_rows.push(group);
                        delta.changed(group);
                    }
                    if let Some(row) = self.active_assistant.take() {
                        self.update_assistant(row, |assistant| {
                            assistant.status = RowStatus::Success;
                        });
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
                    self.settle_final_answer(&mut delta);
                }
                if self.pending_run.as_deref() == Some(run_id.as_str()) {
                    self.pending_run = None;
                }
                if self.active_run.as_deref() == Some(run_id.as_str()) {
                    self.active_run = None;
                    self.active_loop = None;
                }
            }
            AgentUpdate::RunFailed { run_id } => {
                if self.pending_run.as_deref() == Some(run_id.as_str())
                    && self.active_run.as_deref() != Some(run_id.as_str())
                {
                    self.pending_run = None;
                    return delta;
                }
                if self.active_run.as_deref() == Some(run_id.as_str())
                    || self.pending_run.as_deref() == Some(run_id.as_str())
                {
                    if let Some(group) = self.active_group.take() {
                        self.fail_group(group);
                        self.last_finished_rows.push(group);
                        delta.changed(group);
                    }
                    if let Some(row) = self.active_assistant.take() {
                        self.update_assistant(row, |assistant| {
                            assistant.status = RowStatus::Failed;
                        });
                        self.last_finished_rows.push(row);
                        delta.changed(row);
                    }
                    if let Some(row) = self.active_thinking.take() {
                        self.update_thinking(row, |thinking| {
                            thinking.status = RowStatus::Failed;
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
                    self.active_loop = None;
                }
            }
            AgentUpdate::MessageStarted { message_id } => {
                self.begin_assistant_message(&message_id, &mut delta);
            }
            AgentUpdate::MessageFinished { message_id, phase } => {
                if let Some(phase) = phase
                    && let Some(row) = self.assistant_messages.get(&message_id).copied()
                {
                    self.update_assistant(row, |assistant| assistant.phase = Some(phase));
                    delta.changed(row);
                }
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
            AgentUpdate::TurnStats { run_id, .. } => {
                let run_id = run_id
                    .or_else(|| self.pending_run.clone())
                    .or_else(|| self.active_run.clone())
                    .unwrap_or_else(|| "fallback-run".to_string());
                if self.active_run.as_deref() != Some(run_id.as_str()) {
                    self.start_visible_run(run_id);
                    self.pending_run = None;
                }
                self.active_plan = None;
            }
            AgentUpdate::StreamChunk(text) => {
                let row = self.ensure_assistant();
                self.append_assistant(row, &text);
                delta.changed(row);
            }
            AgentUpdate::ThinkingChunk(chunk) => match chunk {
                ThinkingChunk::Started => {
                    // A repeated Started is a hint, not a new region: the
                    // TUI keeps the open card and the deltas continue it.
                    if self.active_thinking.is_some() {
                        return delta;
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
            AgentUpdate::StepAdded(step) => {
                let row = self.ensure_plan();
                self.update_plan(row, |plan| {
                    let mut steps = plan.steps.to_vec();
                    steps.push(PlanRowStep {
                        tool_id: (!step.tool_id.is_empty())
                            .then(|| Arc::from(step.tool_id.as_str())),
                        label: Arc::from(step.description.clone()),
                        status: RowStatus::Pending,
                        detail: (!step.tool.is_empty()).then(|| Arc::from(step.tool.clone())),
                    });
                    plan.steps = steps.into();
                });
                delta.changed(row);
            }
            AgentUpdate::StepQueued {
                idx,
                tool_id,
                tool_name,
                arg_summary,
                arg_full,
                presentation,
                ..
            } => {
                self.mark_active_assistant_commentary(&mut delta);
                if self.set_plan_step(idx, RowStatus::Pending)
                    && let Some(plan) = self.active_plan
                {
                    delta.changed(plan);
                }
                let row = self.ensure_tool(
                    &tool_id,
                    &tool_name,
                    &arg_summary,
                    &arg_full,
                    presentation.visual_kind,
                    RowStatus::Pending,
                );
                if let Some((group, row)) = self.current_tool(&tool_id).copied() {
                    self.latest_tool = Some(LatestToolKey {
                        tool_id: Arc::from(tool_id.as_str()),
                        group,
                        row,
                    });
                    delta.changed(group);
                } else {
                    delta.changed(row);
                }
            }
            AgentUpdate::StepStarted {
                idx,
                tool_id,
                tool_name,
                arg_summary,
                arg_full,
                presentation,
                ..
            } => {
                self.mark_active_assistant_commentary(&mut delta);
                if self.set_plan_step(idx, RowStatus::Running)
                    && let Some(plan) = self.active_plan
                {
                    delta.changed(plan);
                }
                let row = self.ensure_tool(
                    &tool_id,
                    &tool_name,
                    &arg_summary,
                    &arg_full,
                    presentation.visual_kind,
                    RowStatus::Running,
                );
                self.reparent_queued_tool(&tool_id, row);
                self.update_tool_by_row(row, |tool| {
                    if tool.status == RowStatus::Pending {
                        tool.status = RowStatus::Running;
                    }
                    tool.name = Arc::from(tool_name);
                    tool.arg_summary = Arc::from(one_line_summary(&arg_summary));
                    tool.arg_full = (!arg_full.is_empty()).then(|| Arc::from(arg_full));
                });
                let location = self.current_tool(&tool_id).copied();
                if let Some((group, row)) = location {
                    self.latest_tool = Some(LatestToolKey {
                        tool_id: Arc::from(tool_id.as_str()),
                        group,
                        row,
                    });
                    delta.changed(group);
                } else {
                    delta.changed(row);
                }
            }
            AgentUpdate::StepFinished {
                idx,
                run_id,
                tool_id,
                result,
                ..
            } => {
                if !self.run_matches_active(run_id.as_deref()) {
                    return delta;
                }
                let plan_status = match result.status {
                    StepStatus::Failed => RowStatus::Failed,
                    StepStatus::Success => RowStatus::Success,
                    StepStatus::Cancelled => RowStatus::Cancelled,
                };
                if self.set_plan_step(idx, plan_status)
                    && let Some(plan) = self.active_plan
                {
                    delta.changed(plan);
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
                            StepStatus::Cancelled => RowStatus::Cancelled,
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
                        // Keep the streamed output: the error is appended to
                        // what the tool already produced, not a replacement
                        // for it.
                        let previous = tool.output.detail.as_deref().unwrap_or("");
                        let combined = if previous.is_empty() {
                            error.clone()
                        } else {
                            format!("{previous}\n\n{error}")
                        };
                        tool.output = output_view(&combined);
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
                        // A partial metadata update (token usage, model, …)
                        // must not erase an identity that the tool already
                        // published. The spawn tool stamps the child id
                        // before the child exists, and later child updates
                        // arrive without it.
                        if let Some(task_id) = task_id {
                            tool.meta.task_id = Some(Arc::from(task_id));
                        }
                        if let Some(subagent_id) = subagent_id {
                            tool.meta.subagent_id = Some(Arc::from(subagent_id));
                        }
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
                        if output.trim().is_empty() && !message.is_empty() {
                            tool.output = output_view(&message);
                        }
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
                if let Some(row) = self.push_hook_context(
                    source.map(Arc::from),
                    Arc::from(text),
                    Some(now_label()),
                ) {
                    delta.changed(row);
                }
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
                if let Some(row) = self.active_assistant.take() {
                    self.update_assistant(row, |assistant| {
                        assistant.status = RowStatus::Failed;
                    });
                    delta.changed(row);
                }
                if let Some(group) = self.active_group.take() {
                    self.update_group(group, |group| {
                        let mut items = group.items.to_vec();
                        for item in &mut items {
                            if let ToolGroupItem::Tool(tool) = item
                                && matches!(tool.status, RowStatus::Pending | RowStatus::Running)
                                && !tool.keep_live
                            {
                                tool.status = RowStatus::Failed;
                            }
                        }
                        group.items = Arc::from(items);
                    });
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
                    delta.changed(group);
                }
                if let Some(row) = self.active_assistant.take() {
                    self.update_assistant(row, |assistant| {
                        assistant.status = RowStatus::Success;
                    });
                    delta.changed(row);
                }
                if let Some(row) = self.active_thinking.take() {
                    self.update_thinking(row, |thinking| {
                        thinking.status = RowStatus::Success;
                    });
                    delta.changed(row);
                }
                self.settle_final_answer(&mut delta);
                self.pending_run = None;
                self.active_run = None;
                self.active_loop = None;
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
                    delta.changed(row);
                }
                if let Some(group) = self.active_group.take() {
                    self.cancel_group(group);
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
                self.active_loop = None;
            }
            AgentUpdate::TaskIdle => {
                if let Some(group) = self.active_group.take() {
                    self.settle_group(group, true);
                    delta.changed(group);
                }
                if let Some(row) = self.active_assistant.take() {
                    self.update_assistant(row, |assistant| {
                        assistant.status = RowStatus::Success;
                    });
                    delta.changed(row);
                }
                self.settle_final_answer(&mut delta);
                if let Some(row) = self.active_thinking.take() {
                    self.update_thinking(row, |thinking| {
                        thinking.status = RowStatus::Success;
                    });
                    delta.changed(row);
                }
                self.pending_run = None;
                self.active_run = None;
                self.active_loop = None;
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
        // Visible content ends a contiguous tool region. The next tool starts
        // its own card below that content, so transcript order stays causal.
        if let Some(group) = self.active_group
            && !self.rows.last().is_some_and(|row| row.id == group)
        {
            self.active_group = None;
            self.settle_group(group, false);
            delta.changed(group);
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

    /// Marks a gate as submitted for a response the host accepted even though
    /// the local row had drifted (withdrawn, stale, or already answered).
    ///
    /// [`Self::apply_gate_response`] rejects those rows on purpose; this path
    /// runs only after the host confirmed the request is still pending.
    pub fn confirm_gate_response(&mut self, response: &UiResponse) -> Option<RowId> {
        let id = RequestId(response.request_id());
        let row = *self.gates.get(&id)?;
        let cancelled = matches!(
            response,
            UiResponse::Select { choice: None, .. } | UiResponse::MultiSelect { choices: None, .. }
        );
        self.update_gate(id, |gate| {
            if matches!(gate.status, GateState::Resolved | GateState::Cancelled) {
                return;
            }
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
        Some(self.top_level_group_for(row).unwrap_or(row))
    }

    pub fn seed_persisted(
        &mut self,
        messages: &[AgentMessage],
        _context: &ReduceContext,
    ) -> RowDelta {
        let mut delta = RowDelta::default();
        let mut tools = HashMap::<String, RowId>::new();
        for message in messages {
            if message.origin.is_user_turn() {
                // A real user message starts the next run: its tools must not
                // append to the card the previous run left behind. Injected
                // context is not a user turn and keeps the group.
                self.active_group = None;
            }
            match &message.origin {
                MessageOrigin::User => {
                    let text = message_text(message);
                    let row = self.push_row_at(
                        None,
                        RowBody::User(UserRow {
                            text: Arc::from(text),
                            attachments: Arc::from([]),
                        }),
                        None,
                    );
                    delta.changed(row);
                }
                MessageOrigin::InjectedContext { source } => {
                    // The stored message keeps the `<hook-context>` frame that
                    // marks it as data rather than user speech; the card shows
                    // the body, and repeats of the same briefing collapse into
                    // the one already on screen.
                    let text = message_text(message);
                    let body = tact::hook::hook_context_body(&text);
                    if let Some(row) = self.push_hook_context(
                        Some(Arc::from(source.clone())),
                        Arc::from(body),
                        None,
                    ) {
                        self.active_group = None;
                        delta.changed(row);
                    }
                }
                MessageOrigin::ToolResult => {
                    for block in &message.content {
                        if let AgentContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            status,
                        } = block
                            && let Some(row) = tools.get(tool_use_id).copied()
                        {
                            self.update_tool_by_row(row, |tool| {
                                tool.output = output_view(content);
                                tool.status = match status {
                                    tact_agent_api::ToolResultStatus::Success => RowStatus::Success,
                                    tact_agent_api::ToolResultStatus::Failed => RowStatus::Failed,
                                    tact_agent_api::ToolResultStatus::Cancelled => {
                                        RowStatus::Cancelled
                                    }
                                    tact_agent_api::ToolResultStatus::Unknown => RowStatus::Unknown,
                                };
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
                            let row = self.push_row_at(
                                None,
                                RowBody::Thinking(ThinkingRow {
                                    markdown: Arc::from(thinking.as_str()),
                                    status: RowStatus::Success,
                                    duration_ms: None,
                                }),
                                None,
                            );
                            self.active_group = None;
                            delta.changed(row);
                        }
                    }
                    let text = message_text(message);
                    if !text.trim().is_empty() {
                        let row = self.push_row_at(
                            None,
                            RowBody::Assistant(AssistantRow {
                                markdown: Arc::from(text),
                                status: RowStatus::Success,
                                suggestions: Arc::from([]),
                                phase: message.phase,
                            }),
                            None,
                        );
                        self.active_group = None;
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
                        let group = self.ensure_group();
                        for (id, name, input) in tool_uses {
                            let row = self.ensure_tool(
                                &id,
                                &name,
                                &input.to_string(),
                                &input.to_string(),
                                ToolVisualKind::Generic,
                                RowStatus::Pending,
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
                        self.active_group = None;
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
        // Seeded history is a finished transcript. No group is still
        // collecting, and a tool with no persisted result never finished, so
        // it must not spin forever after a reload.
        self.active_group = None;
        let groups = self
            .rows
            .iter()
            .filter_map(|row| match &row.body {
                RowBody::ToolGroup(group) if !group.items.is_empty() => Some(row.id),
                _ => None,
            })
            .collect::<Vec<_>>();
        for group in groups {
            self.settle_group(group, true);
            delta.changed(group);
        }
        // Legacy history carries no provider phase; label it by turn position.
        self.infer_phases_from_position(&mut delta);
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
            return row;
        }
        let row = self.push_row(
            self.current_loop(),
            RowBody::Assistant(AssistantRow {
                markdown: Arc::from(""),
                status: RowStatus::Running,
                suggestions: Arc::from([]),
                phase: None,
            }),
        );
        if self.active_message_id.is_none() {
            tracing::warn!(
                run = self.active_run.as_deref().unwrap_or("unknown"),
                "assistant text arrived without MessageStarted; using a fallback row"
            );
        }
        if let Some(message_id) = self.active_message_id.clone() {
            self.assistant_messages.insert(message_id, row);
        }
        self.active_assistant = Some(row);
        row
    }

    fn begin_assistant_message(&mut self, message_id: &str, delta: &mut RowDelta) {
        if self.active_message_id.as_deref() == Some(message_id) {
            return;
        }
        if let Some(row) = self.assistant_messages.get(message_id).copied() {
            self.active_message_id = Some(message_id.to_string());
            self.active_assistant = Some(row);
            return;
        }
        if let Some(row) = self.active_assistant.take() {
            self.update_assistant(row, |assistant| {
                if assistant.status == RowStatus::Running {
                    assistant.status = RowStatus::Success;
                }
            });
            delta.changed(row);
        }
        self.active_message_id = Some(message_id.to_string());
        self.active_assistant = None;
    }

    fn ensure_group(&mut self) -> RowId {
        if let Some(group) = self.active_group {
            return group;
        }
        let loop_id = self.active_loop;
        let row = self.push_row(
            loop_id,
            RowBody::ToolGroup(ToolGroupRow {
                loop_id: loop_id.unwrap_or(LoopId(u64::MAX)),
                items: Arc::from([]),
                summary: Arc::from("0 tools"),
                collapsed: false,
                collapse_override: None,
            }),
        );
        self.active_group = Some(row);
        row
    }

    fn ensure_tool(
        &mut self,
        tool_id: &str,
        name: &str,
        arg_summary: &str,
        arg_full: &str,
        visual_kind: ToolVisualKind,
        initial_status: RowStatus,
    ) -> RowId {
        let key = self.current_tool_key(tool_id);
        if let Some((_, row)) = self.tools.get(&key) {
            return *row;
        }
        let group = self.ensure_group();
        let row = self.alloc_id();
        // The invocation already fills the Input panel. The body is the
        // output area, so a command must not pre-fill it with its own text or
        // the streamed output and the failure message have nowhere to show.
        let body = ToolBody::None;
        let tool = ToolGroupItem::Tool(ToolRow {
            id: row,
            tool_id: Arc::from(tool_id),
            name: Arc::from(name),
            arg_summary: Arc::from(one_line_summary(arg_summary)),
            arg_full: (!arg_full.is_empty()).then(|| Arc::from(arg_full)),
            status: initial_status,
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
                        assistant.status = RowStatus::Success;
                        changed = true;
                        tool.subagent = Some(nested);
                        return;
                    }
                    RowBody::Assistant(AssistantRow {
                        markdown: Arc::from(text),
                        status: RowStatus::Success,
                        suggestions: Arc::from([]),
                        phase: None,
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
                        SubagentStepStatus::Cancelled => RowStatus::Cancelled,
                    };
                    let output = step
                        .detail
                        .clone()
                        .filter(|detail| !detail.is_empty())
                        .unwrap_or_else(|| step.message.clone());
                    let folded = update_nested_tool(&mut nested, &step.tool_id, |child| {
                        child.status = status;
                        child.name = Arc::from(step.tool_name.clone());
                        child.arg_summary = Arc::from(one_line_summary(&step.arg_summary));
                        child.arg_full =
                            (!step.arg_full.is_empty()).then(|| Arc::from(step.arg_full.clone()));
                        child.output = output_view(&output);
                        child.duration_ms = step.duration_us.map(|micros| micros / 1_000);
                    });
                    if folded {
                        changed = true;
                        tool.subagent = Some(nested);
                        return;
                    }
                    let item = ToolGroupItem::Tool(ToolRow {
                        id: next_row_id,
                        tool_id: Arc::from(step.tool_id),
                        name: Arc::from(step.tool_name),
                        arg_summary: Arc::from(one_line_summary(&step.arg_summary)),
                        arg_full: (!step.arg_full.is_empty()).then(|| Arc::from(step.arg_full)),
                        status,
                        body: ToolBody::None,
                        output: output_view(&output),
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
                        collapsed: status != RowStatus::Running,
                        collapse_override: None,
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
                time: None,
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
                    time: None,
                    body: RowBody::Assistant(AssistantRow {
                        markdown: Arc::from(text),
                        status: RowStatus::Running,
                        suggestions: Arc::from([]),
                        phase: None,
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

    /// The top-level transcript row that owns a nested subagent row.
    ///
    /// A disclosure inside the subagent overlay changes a copy under the
    /// parent tool; the shell only remeasures top-level rows, so the intent
    /// handler has to mark that parent dirty as well as the nested id.
    pub(crate) fn top_level_row_for(&self, nested: RowId) -> Option<RowId> {
        self.rows.iter().find_map(|row| match &row.body {
            RowBody::ToolGroup(group) => group
                .items
                .iter()
                .any(|item| match item {
                    ToolGroupItem::Tool(tool) => {
                        tool.subagent.as_ref().is_some_and(|nested_view| {
                            nested_view.iter().any(|nested_row| nested_row.id == nested)
                        })
                    }
                    ToolGroupItem::Gate(gate) => gate.id == nested,
                })
                .then_some(row.id),
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
        // A group still collecting adjacent steps stays open. Visible rows
        // between tool regions end the group before the next StepQueued or
        // StepStarted creates another one.
        let collecting = self.active_group == Some(row);
        self.update_group(row, |group| {
            if finish_run {
                // A completed run ends every non-keep-live tool that never
                // reported a result. A keep-live tool still owns its card.
                let mut items = group.items.to_vec();
                let mut finalized = false;
                for item in &mut items {
                    if let ToolGroupItem::Tool(tool) = item
                        && matches!(tool.status, RowStatus::Pending | RowStatus::Running)
                        && !tool.keep_live
                    {
                        tool.status = RowStatus::Cancelled;
                        finalized = true;
                    }
                }
                if finalized {
                    group.items = Arc::from(items);
                }
            }
            let status = group.status();
            if group.collapse_override.is_none() {
                group.collapsed = status == RowStatus::Success && !collecting;
            }
        });
    }

    fn fail_group(&mut self, row: RowId) {
        self.update_group(row, |group| {
            let mut items = group.items.to_vec();
            for item in &mut items {
                if let ToolGroupItem::Tool(tool) = item
                    && matches!(tool.status, RowStatus::Pending | RowStatus::Running)
                    && !tool.keep_live
                {
                    tool.status = RowStatus::Failed;
                }
            }
            group.items = Arc::from(items);
            group.collapsed = false;
            group.collapse_override = Some(false);
        });
    }

    fn cancel_group(&mut self, row: RowId) {
        self.update_group(row, |group| {
            let mut items = group.items.to_vec();
            let mut cancelled = false;
            for item in &mut items {
                match item {
                    ToolGroupItem::Tool(tool)
                        if matches!(tool.status, RowStatus::Pending | RowStatus::Running) =>
                    {
                        tool.status = RowStatus::Cancelled;
                        cancelled = true;
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
            if cancelled {
                group.collapsed = false;
                group.collapse_override = Some(false);
            }
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

    fn reparent_queued_tool(&mut self, tool_id: &str, row_id: RowId) {
        let key = self.current_tool_key(tool_id);
        let Some((old_group, existing_row)) = self.tools.get(&key).copied() else {
            return;
        };
        if existing_row != row_id || self.active_group == Some(old_group) {
            return;
        }
        let queued = self.rows.iter().any(|row| {
            matches!(
                &row.body,
                RowBody::ToolGroup(group)
                    if group.items.iter().any(|item| {
                        matches!(
                            item,
                            ToolGroupItem::Tool(tool)
                                if tool.id == row_id && tool.status == RowStatus::Pending
                        )
                    })
            )
        });
        if !queued {
            return;
        }
        let Some(item) = self.remove_group_item(old_group, row_id) else {
            return;
        };
        let group = self.ensure_group();
        self.append_group_item(group, item);
        self.tools.insert(key, (group, row_id));
    }

    fn remove_group_item(&mut self, group_id: RowId, item_id: RowId) -> Option<ToolGroupItem> {
        let index = self.rows.iter().position(|row| row.id == group_id)?;
        let mut group = match &self.rows[index].body {
            RowBody::ToolGroup(group) => group.clone(),
            _ => return None,
        };
        let mut items = group.items.to_vec();
        let item_index = items.iter().position(|item| item.id() == item_id)?;
        let item = items.remove(item_index);
        if items.is_empty() {
            self.rows.remove(index);
        } else {
            group.items = Arc::from(items);
            group.summary = Arc::from(format!("{} items", group.items.len()));
            let mut row = (*self.rows[index]).clone();
            row.body = RowBody::ToolGroup(group);
            self.rows.set(index, Arc::new(row));
        }
        Some(item)
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
        let Some(ToolGroupItem::Tool(tool)) = items.iter_mut().find(|item| item.id() == tool_id)
        else {
            return;
        };
        update(tool);
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

    /// A tool call followed, so the open assistant text was narration.
    fn mark_active_assistant_commentary(&mut self, delta: &mut RowDelta) {
        let Some(row) = self.active_assistant else {
            return;
        };
        let unlabeled = self
            .rows
            .iter()
            .find(|candidate| candidate.id == row)
            .is_some_and(|candidate| match &candidate.body {
                RowBody::Assistant(assistant) => {
                    assistant.phase.is_none() && !assistant.markdown.trim().is_empty()
                }
                _ => false,
            });
        if unlabeled {
            self.update_assistant(row, |assistant| {
                assistant.phase = Some(MessagePhase::Commentary);
            });
            delta.changed(row);
        }
    }

    /// The run ended, so the last assistant text is this run's answer.
    ///
    /// Covers text with no provider phase and text the provider labeled
    /// [`MessagePhase::Commentary`] that nothing follows. `Commentary` means
    /// "more output or a tool call may follow", so a trailing `Commentary` row
    /// is a stale label: the response carried text beside a tool call (see
    /// `tact_llm`'s `inferred_phase`), the tool group drew above the text, and
    /// no later message re-labeled it. Leaving it would let a surface that
    /// folds narration — the desktop transcript — hide the answer inside its
    /// activity summary.
    fn settle_final_answer(&mut self, delta: &mut RowDelta) {
        // Walk back to the run's last text. A tool group or reasoning card
        // *after* it means the text really was narration for that work.
        let mut continued = false;
        let mut settled = None;
        for row in self.rows.iter().rev() {
            match &row.body {
                RowBody::ToolGroup(_) | RowBody::Thinking(_) => continued = true,
                RowBody::Assistant(assistant) if !assistant.markdown.trim().is_empty() => {
                    let unlabeled_or_trailing =
                        matches!(assistant.phase, None | Some(MessagePhase::Commentary));
                    if !continued && unlabeled_or_trailing {
                        settled = Some(row.id);
                    }
                    break;
                }
                RowBody::User(_) => break,
                _ => {}
            }
        }
        if let Some(row) = settled {
            self.update_assistant(row, |assistant| {
                assistant.phase = Some(MessagePhase::FinalAnswer);
            });
            delta.changed(row);
        }
    }

    /// Labels assistant text by turn position for history without a provider phase.
    ///
    /// Text followed by a tool group is narration; the last text before a user
    /// turn, or at the end, is that turn's answer.
    fn infer_phases_from_position(&mut self, delta: &mut RowDelta) {
        let mut updates: Vec<(RowId, MessagePhase)> = Vec::new();
        let mut pending: Option<RowId> = None;
        for row in self.rows.iter() {
            match &row.body {
                RowBody::Assistant(assistant) if !assistant.markdown.trim().is_empty() => {
                    if let Some(previous) = pending.take() {
                        updates.push((previous, MessagePhase::Commentary));
                    }
                    if assistant.phase.is_none() {
                        pending = Some(row.id);
                    }
                }
                RowBody::ToolGroup(_) => {
                    if let Some(row) = pending.take() {
                        updates.push((row, MessagePhase::Commentary));
                    }
                }
                RowBody::User(_) => {
                    if let Some(row) = pending.take() {
                        updates.push((row, MessagePhase::FinalAnswer));
                    }
                }
                _ => {}
            }
        }
        if let Some(row) = pending {
            updates.push((row, MessagePhase::FinalAnswer));
        }
        for (row, phase) in updates {
            self.update_assistant(row, |assistant| assistant.phase = Some(phase));
            delta.changed(row);
        }
    }

    /// Ends the open reasoning region, settling its card.
    fn close_thinking(&mut self, delta: &mut RowDelta) {
        if let Some(row) = self.active_thinking.take() {
            self.update_thinking(row, |thinking| {
                if thinking.status == RowStatus::Running {
                    thinking.status = RowStatus::Success;
                }
            });
            self.last_finished_rows.push(row);
            delta.changed(row);
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

    fn start_visible_run(&mut self, run_id: String) {
        self.active_run = Some(run_id);
        self.active_loop = Some(LoopId(self.next_loop_id));
        self.next_loop_id += 1;
        self.active_group = None;
        self.active_assistant = None;
        self.active_thinking = None;
        self.active_plan = None;
        self.assistant_messages.clear();
        self.last_finished_rows.clear();
        self.latest_tool = None;
    }

    fn alloc_id(&mut self) -> RowId {
        let id = RowId(self.next_row_id);
        self.next_row_id += 1;
        id
    }

    fn push_row(&mut self, loop_id: Option<LoopId>, body: RowBody) -> RowId {
        self.push_row_at(loop_id, body, Some(now_label()))
    }

    fn push_row_at(
        &mut self,
        loop_id: Option<LoopId>,
        body: RowBody,
        time: Option<Arc<str>>,
    ) -> RowId {
        let id = self.alloc_id();
        self.push_existing_at(id, loop_id, body, time);
        id
    }

    /// Appends a hook-context card unless the latest one already carries the
    /// same briefing.
    ///
    /// A resume re-runs the session-start hooks. A plugin that injects an
    /// unchanged briefing would otherwise add a card every time the session is
    /// opened, and the restored history would show one per past open. The card
    /// already on screen stays where the briefing first arrived; the model
    /// still receives every injection.
    fn hook_context_repeats(&self, source: Option<&str>, markdown: &str) -> bool {
        let mut latest = None;
        for row in self.rows.iter() {
            if let RowBody::HookContext(context) = &row.body {
                latest = Some(context);
            }
        }
        latest.is_some_and(|context| {
            context.source.as_deref() == source && context.markdown.as_ref() == markdown
        })
    }

    fn push_hook_context(
        &mut self,
        source: Option<Arc<str>>,
        markdown: Arc<str>,
        time: Option<Arc<str>>,
    ) -> Option<RowId> {
        if self.hook_context_repeats(source.as_deref(), &markdown) {
            return None;
        }
        Some(self.push_row_at(
            None,
            RowBody::HookContext(HookContextRow { source, markdown }),
            time,
        ))
    }

    fn ensure_plan(&mut self) -> RowId {
        if let Some(row) = self.active_plan {
            return row;
        }
        let row = self.push_row(
            self.current_loop(),
            RowBody::Plan(PlanRow {
                title: Arc::from("Execution plan"),
                steps: Arc::from([]),
            }),
        );
        self.active_plan = Some(row);
        row
    }

    fn update_plan(&mut self, row: RowId, update: impl FnOnce(&mut PlanRow)) {
        if let Some(existing) = self.rows.iter_mut().find(|candidate| candidate.id == row) {
            let mut next = (**existing).clone();
            if let RowBody::Plan(plan) = &mut next.body {
                update(plan);
            }
            *existing = Arc::new(next);
        }
    }

    fn set_plan_step(&mut self, index: usize, status: RowStatus) -> bool {
        let Some(row) = self.active_plan else {
            return false;
        };
        let mut updated = false;
        self.update_plan(row, |plan| {
            if let Some(step) = Arc::make_mut(&mut plan.steps).get_mut(index) {
                step.status = status;
                updated = true;
            }
        });
        updated
    }

    fn push_existing(&mut self, id: RowId, loop_id: Option<LoopId>, body: RowBody) {
        self.push_existing_at(id, loop_id, body, Some(now_label()));
    }

    fn push_existing_at(
        &mut self,
        id: RowId,
        loop_id: Option<LoopId>,
        body: RowBody,
        time: Option<Arc<str>>,
    ) {
        let ordinal = self.ordinal;
        self.ordinal += 1;
        self.rows.push_back(Arc::new(TranscriptRow {
            id,
            ordinal,
            loop_id,
            time,
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

/// The local `HH:MM` a live row carries into its label.
/// The one-line form of a tool's argument summary.
///
/// `bash` commands arrive with their own newlines and indentation, and a
/// summary that keeps them makes every header that shows it grow to the
/// command's shape. The full text still travels in `arg_full`; the summary is
/// a label, so its whitespace is flattened here, once, for every surface. The
/// length stays intact — callers truncate for display, and a search query
/// must remain parseable.
pub fn one_line_summary(text: &str) -> String {
    let mut collapsed = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        if !collapsed.is_empty() {
            collapsed.push(' ');
        }
        collapsed.push_str(word);
    }
    collapsed
}

/// The inspector chip's mono label: the tool name and, when it has one, the
/// same one-line argument summary the transcript group header shows.
fn tool_label(name: &str, arg_summary: &str) -> String {
    let summary = one_line_summary(arg_summary);
    if summary.is_empty() {
        name.to_string()
    } else {
        format!("{name} {summary}")
    }
}

fn now_label() -> Arc<str> {
    Arc::from(chrono::Local::now().format("%H:%M").to_string())
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

const TOOL_PREVIEW_CHARS: usize = 4_000;
const TOOL_DETAIL_CHARS: usize = 50_000;

fn output_view(text: &str) -> ToolOutputView {
    let chars = text.chars().count();
    let detail = tail_chars(text, TOOL_DETAIL_CHARS);
    ToolOutputView {
        preview: Arc::from(tail_chars(&detail, TOOL_PREVIEW_CHARS)),
        detail: (!detail.is_empty()).then(|| Arc::from(detail)),
        truncated: chars > TOOL_DETAIL_CHARS,
    }
}

fn append_output(tool: &mut ToolRow, text: &str) {
    let combined = format!("{}{}", tool.output.detail.as_deref().unwrap_or(""), text);
    let truncated = tool.output.truncated || combined.chars().count() > TOOL_DETAIL_CHARS;
    tool.output = output_view(&combined);
    tool.output.truncated |= truncated;
}

fn tail_chars(text: &str, max_chars: usize) -> String {
    let len = text.chars().count();
    text.chars().skip(len.saturating_sub(max_chars)).collect()
}

/// Updates that end an open reasoning region.
///
/// Mirrors the TUI's `ends_thinking_region`: these updates append this turn's
/// own output at the transcript tail or move the turn to its next stage, so
/// the open thinking card must settle before they land. Status-bar metadata,
/// overlays, and other agents' traffic leave the region open.
fn ends_thinking_region(update: &AgentUpdate) -> bool {
    matches!(
        update,
        AgentUpdate::StreamChunk(_)
            | AgentUpdate::StepAdded(_)
            | AgentUpdate::StepQueued { .. }
            | AgentUpdate::StepStarted { .. }
            | AgentUpdate::StepFinished { .. }
            | AgentUpdate::StepFailed { .. }
            | AgentUpdate::Info(_)
            | AgentUpdate::MdInfo(_)
            | AgentUpdate::HookContext { .. }
            | AgentUpdate::HookStatus { .. }
            | AgentUpdate::Error(_)
            | AgentUpdate::TaskComplete(_)
            | AgentUpdate::TaskIdle
    )
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
            show_hook_output: true,
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
        let run_id = reducer
            .pending_run
            .clone()
            .or_else(|| reducer.active_run.clone())
            .unwrap_or_else(|| "run-1".to_string());
        reducer.apply_agent_update(
            AgentUpdate::TurnStats {
                run_id: Some(run_id),
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

    fn assistant_phases(reducer: &TranscriptReducer) -> Vec<Option<MessagePhase>> {
        reducer
            .rows
            .iter()
            .filter_map(|row| match &row.body {
                RowBody::Assistant(assistant) => Some(assistant.phase),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn message_finished_labels_the_assistant_text() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(
            AgentUpdate::MessageStarted {
                message_id: "m1".into(),
            },
            &context(1),
        );
        reducer.apply_agent_update(AgentUpdate::StreamChunk("answer".into()), &context(1));
        reducer.apply_agent_update(
            AgentUpdate::MessageFinished {
                message_id: "m1".into(),
                phase: Some(MessagePhase::FinalAnswer),
            },
            &context(1),
        );
        assert_eq!(
            assistant_phases(&reducer),
            [Some(MessagePhase::FinalAnswer)],
            "the provider phase lands on the assistant row"
        );
    }

    #[test]
    fn text_before_a_tool_is_commentary_without_a_phase() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(
            AgentUpdate::MessageStarted {
                message_id: "m1".into(),
            },
            &context(1),
        );
        reducer.apply_agent_update(AgentUpdate::StreamChunk("checking".into()), &context(1));
        reducer.apply_agent_update(step_started("tool-1"), &context(1));
        assert_eq!(
            assistant_phases(&reducer),
            [Some(MessagePhase::Commentary)],
            "text followed by a tool call was narration"
        );
    }

    #[test]
    fn finishing_the_run_labels_the_last_text_as_the_answer() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(
            AgentUpdate::MessageStarted {
                message_id: "m1".into(),
            },
            &context(1),
        );
        reducer.apply_agent_update(AgentUpdate::StreamChunk("done".into()), &context(1));
        reducer.apply_agent_update(
            AgentUpdate::RunFinished {
                run_id: "run-1".into(),
            },
            &context(1),
        );
        assert_eq!(
            assistant_phases(&reducer),
            [Some(MessagePhase::FinalAnswer)],
            "the last text of the run is the answer"
        );
    }

    /// A provider labels a response that carries text beside a tool call as
    /// `Commentary`. When that response also ends the run, the label is stale:
    /// the desktop transcript would fold the answer into its activity summary.
    #[test]
    fn a_trailing_commentary_text_is_promoted_to_the_answer() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(
            AgentUpdate::MessageStarted {
                message_id: "m1".into(),
            },
            &context(1),
        );
        reducer.apply_agent_update(step_started("tool-1"), &context(1));
        reducer.apply_agent_update(step_finished("tool-1"), &context(1));
        // Text after the tool call, in the same response: the provider labels
        // the whole response `Commentary` even though nothing follows.
        reducer.apply_agent_update(AgentUpdate::StreamChunk("all done".into()), &context(1));
        reducer.apply_agent_update(
            AgentUpdate::MessageFinished {
                message_id: "m1".into(),
                phase: Some(MessagePhase::Commentary),
            },
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::RunFinished {
                run_id: "run-1".into(),
            },
            &context(1),
        );
        assert_eq!(
            assistant_phases(&reducer),
            [Some(MessagePhase::FinalAnswer)],
            "text that follows the tools ends the run, so it is the answer"
        );
    }

    /// The complement: when a tool group draws *after* the text, the text was
    /// narration and must stay folded.
    #[test]
    fn commentary_followed_by_a_tool_group_stays_narration() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(
            AgentUpdate::MessageStarted {
                message_id: "m1".into(),
            },
            &context(1),
        );
        reducer.apply_agent_update(AgentUpdate::StreamChunk("checking".into()), &context(1));
        reducer.apply_agent_update(step_started("tool-1"), &context(1));
        reducer.apply_agent_update(step_finished("tool-1"), &context(1));
        reducer.apply_agent_update(
            AgentUpdate::MessageFinished {
                message_id: "m1".into(),
                phase: Some(MessagePhase::Commentary),
            },
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::RunFinished {
                run_id: "run-1".into(),
            },
            &context(1),
        );
        assert_eq!(
            assistant_phases(&reducer),
            [Some(MessagePhase::Commentary)],
            "work followed this text, so it stays narration"
        );
    }

    #[test]
    fn persisted_history_without_phase_is_labelled_by_position() {
        let mut reducer = TranscriptReducer::new();
        reducer.seed_persisted(
            &[
                AgentMessage::new(
                    AgentRole::User,
                    MessageOrigin::User,
                    vec![AgentContentBlock::Text { text: "go".into() }],
                ),
                AgentMessage::new(
                    AgentRole::Assistant,
                    MessageOrigin::Assistant,
                    vec![
                        AgentContentBlock::Text {
                            text: "checking".into(),
                        },
                        AgentContentBlock::ToolUse {
                            id: "tool-1".into(),
                            name: "read_file".into(),
                            input: json!({"path": "README.md"}),
                        },
                    ],
                ),
                AgentMessage::new(
                    AgentRole::Assistant,
                    MessageOrigin::Assistant,
                    vec![AgentContentBlock::Text {
                        text: "done".into(),
                    }],
                ),
            ],
            &context(1),
        );
        assert_eq!(
            assistant_phases(&reducer),
            [
                Some(MessagePhase::Commentary),
                Some(MessagePhase::FinalAnswer)
            ]
        );
    }

    fn step_finished(tool_id: &str) -> AgentUpdate {
        AgentUpdate::StepFinished {
            idx: 0,
            run_id: None,
            tool_id: tool_id.to_string(),
            result: StepResult {
                tool: "read_file".into(),
                arg_summary: "README.md".into(),
                arg_full: None,
                status: StepStatus::Success,
                message: "ok".into(),
                detail: None,
                duration_us: None,
                permission_label: None,
                presentation: ToolPresentationInfo::generic("read_file"),
            },
        }
    }

    fn command_started(tool_id: &str, command: &str) -> AgentUpdate {
        let mut presentation = ToolPresentationInfo::generic("bash");
        presentation.visual_kind = ToolVisualKind::Command;
        AgentUpdate::StepStarted {
            idx: 0,
            tool_id: tool_id.to_string(),
            tool_name: "bash".into(),
            arg_summary: command.into(),
            arg_full: command.into(),
            presentation,
        }
    }

    #[test]
    fn step_added_builds_a_plan_row_and_tracks_status() {
        let mut reducer = TranscriptReducer::default();
        run_started(&mut reducer, "run-plan");
        reducer.apply_agent_update(
            AgentUpdate::StepAdded(tact_protocol::PlanStep::new(
                "Read the shell",
                "read_file",
                "tool-plan-1",
                Vec::<(&str, &str)>::new(),
            )),
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::StepAdded(tact_protocol::PlanStep::new(
                "Patch the shell",
                "edit_file",
                "tool-plan-2",
                Vec::<(&str, &str)>::new(),
            )),
            &context(1),
        );
        let plan = reducer
            .rows
            .iter()
            .find_map(|row| match &row.body {
                RowBody::Plan(plan) => Some(plan.clone()),
                _ => None,
            })
            .expect("plan row");
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].label.as_ref(), "Read the shell");
        assert_eq!(plan.steps[0].tool_id.as_deref(), Some("tool-plan-1"));
        assert_eq!(plan.steps[1].tool_id.as_deref(), Some("tool-plan-2"));

        reducer.apply_agent_update(
            AgentUpdate::StepStarted {
                idx: 1,
                tool_id: "tool-plan-2".into(),
                tool_name: "edit_file".into(),
                arg_summary: "src/lib.rs".into(),
                arg_full: "src/lib.rs".into(),
                presentation: ToolPresentationInfo::generic("edit_file"),
            },
            &context(1),
        );
        let plan = reducer
            .rows
            .iter()
            .find_map(|row| match &row.body {
                RowBody::Plan(plan) => Some(plan.clone()),
                _ => None,
            })
            .expect("plan row");
        assert_eq!(plan.steps[1].status, RowStatus::Running);
    }

    fn group_rows(reducer: &TranscriptReducer) -> Vec<Arc<TranscriptRow>> {
        reducer
            .rows
            .iter()
            .filter(|row| matches!(row.body, RowBody::ToolGroup(_)))
            .cloned()
            .collect()
    }

    fn first_group_status(reducer: &TranscriptReducer) -> RowStatus {
        match &group_rows(reducer)[0].body {
            RowBody::ToolGroup(group) => group.status(),
            _ => panic!("expected a tool group"),
        }
    }

    #[test]
    fn latest_tool_tracks_the_transcript_lifecycle() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-latest");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-latest"), &context(1));

        let latest = reducer.latest_tool_view().expect("latest tool");
        assert_eq!(latest.label.as_ref(), "read_file README.md");
        assert_eq!(latest.status, RowStatus::Running);
        let group = latest.group.expect("tool group");
        let row = latest.row.expect("tool row");
        assert!(reducer.rows.iter().any(|candidate| {
            candidate.id == group
                && matches!(
                    &candidate.body,
                    RowBody::ToolGroup(items)
                        if items.items.iter().any(|item| item.id() == row)
                )
        }));

        // A second step replaces the chip; the first row keeps its own life.
        reducer.apply_agent_update(step_started("tool-second"), &context(1));
        let second = reducer.latest_tool_view().expect("second latest tool");
        assert_eq!(second.tool_id.as_ref(), "tool-second");
        let second_group = second.group.expect("second tool group");
        let second_row = second.row.expect("second tool row");

        let mut finished = step_finished("tool-second");
        let AgentUpdate::StepFinished { result, .. } = &mut finished else {
            unreachable!("step_finished builds a StepFinished")
        };
        result.duration_us = Some(1_500_000);
        reducer.apply_agent_update(finished, &context(1));

        let latest = reducer.latest_tool_view().expect("latest tool");
        assert_eq!(latest.status, RowStatus::Success);
        assert_eq!(latest.duration_ms, Some(1_500));
        assert_eq!(latest.group, Some(second_group));
        assert_eq!(latest.row, Some(second_row));
    }

    fn first_tool(reducer: &TranscriptReducer) -> ToolRow {
        group_rows(reducer)
            .into_iter()
            .find_map(|row| match &row.body {
                RowBody::ToolGroup(group) => group.items.iter().find_map(|item| match item {
                    ToolGroupItem::Tool(tool) => Some(tool.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .expect("a tool row")
    }

    fn nested_tools(reducer: &TranscriptReducer) -> Vec<ToolRow> {
        let Some(nested) = first_tool(reducer).subagent else {
            return Vec::new();
        };
        nested
            .iter()
            .filter_map(|row| match &row.body {
                RowBody::ToolGroup(group) => group.items.iter().find_map(|item| match item {
                    ToolGroupItem::Tool(tool) => Some(tool.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .collect()
    }

    fn child_step(
        status: tact_protocol::subagent_transcript::SubagentStepStatus,
        message: &str,
        duration_us: Option<u64>,
    ) -> AgentUpdate {
        use tact_protocol::subagent_transcript::{SubagentStep, SubagentTranscriptEntry};
        AgentUpdate::ToolTranscript {
            tool_id: "spawn-1".into(),
            entry: SubagentTranscriptEntry::Step(SubagentStep {
                tool_id: "child-1".into(),
                tool_name: "bash".into(),
                arg_summary: "ls".into(),
                arg_full: "ls".into(),
                status,
                message: message.into(),
                detail: (!message.is_empty()).then(|| message.into()),
                duration_us,
                presentation: ToolPresentationInfo::generic("bash"),
            }),
        }
    }

    #[test]
    fn an_idle_error_keeps_the_finished_run_green() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-idle");
        turn(&mut reducer);
        reducer.apply_agent_update(AgentUpdate::StreamChunk("done".into()), &context(1));
        reducer.apply_agent_update(step_started("tool-idle"), &context(1));
        reducer.apply_agent_update(step_finished("tool-idle"), &context(1));
        reducer.apply_agent_update(
            AgentUpdate::RunFinished {
                run_id: "run-idle".into(),
            },
            &context(1),
        );
        reducer.push_user("next prompt", Vec::new());

        reducer.apply_agent_update(
            AgentUpdate::Error(tact_protocol::AgentErrorKind::Other("idle failure".into())),
            &context(1),
        );

        assert!(group_rows(&reducer).iter().all(|row| {
            matches!(
                &row.body,
                RowBody::ToolGroup(group) if group.status() == RowStatus::Success
            )
        }));
        let assistant = reducer.rows.iter().find_map(|row| match &row.body {
            RowBody::Assistant(assistant) => Some(assistant.status),
            _ => None,
        });
        assert_eq!(assistant, Some(RowStatus::Success));
        assert!(
            reducer.latest_tool_view().is_none(),
            "the next prompt starts a new run and clears the previous chip"
        );
    }

    #[test]
    fn late_error_does_not_rewrite_a_finished_tool() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-finished");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-finished"), &context(1));
        reducer.apply_agent_update(step_finished("tool-finished"), &context(1));
        reducer.apply_agent_update(
            AgentUpdate::RunFinished {
                run_id: "run-finished".into(),
            },
            &context(1),
        );

        reducer.apply_agent_update(
            AgentUpdate::Error(tact_protocol::AgentErrorKind::Other("late failure".into())),
            &context(1),
        );

        assert_eq!(first_tool(&reducer).status, RowStatus::Success);
        assert_eq!(first_group_status(&reducer), RowStatus::Success);
    }

    #[test]
    fn a_duplicate_step_started_does_not_regress_the_chip() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-duplicate");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-duplicate"), &context(1));
        reducer.apply_agent_update(step_finished("tool-duplicate"), &context(1));
        assert_eq!(
            reducer.latest_tool_view().map(|latest| latest.status),
            Some(RowStatus::Success)
        );

        reducer.apply_agent_update(step_started("tool-duplicate"), &context(1));

        assert_eq!(
            reducer.latest_tool_view().map(|latest| latest.status),
            Some(RowStatus::Success),
            "the chip must mirror the finished row, not the replayed start"
        );
    }

    #[test]
    fn a_late_duplicate_start_does_not_reparent_a_finished_tool() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-duplicate-closed");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-duplicate"), &context(1));
        let group = group_rows(&reducer)[0].id;
        reducer.apply_agent_update(step_finished("tool-duplicate"), &context(1));
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(tact_protocol::ThinkingChunk::Started),
            &context(1),
        );

        reducer.apply_agent_update(step_started("tool-duplicate"), &context(1));

        let groups = group_rows(&reducer);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].id, group);
        assert_eq!(first_tool(&reducer).status, RowStatus::Success);
    }

    #[test]
    fn a_run_error_finalizes_the_tool_it_interrupts() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-error");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-error"), &context(1));

        reducer.apply_agent_update(
            AgentUpdate::Error(tact_protocol::AgentErrorKind::Other(
                "provider failed".into(),
            )),
            &context(1),
        );

        assert_eq!(first_tool(&reducer).status, RowStatus::Failed);
        assert!(matches!(
            &group_rows(&reducer)[0].body,
            RowBody::ToolGroup(group) if group.status() == RowStatus::Failed
        ));
        assert_eq!(
            reducer.latest_tool_view().map(|latest| latest.status),
            Some(RowStatus::Failed)
        );
    }

    #[test]
    fn a_later_tool_start_reopens_the_group() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-1"), &context(1));
        reducer.apply_agent_update(step_finished("tool-1"), &context(1));
        assert_eq!(first_group_status(&reducer), RowStatus::Success);

        reducer.apply_agent_update(step_started("tool-2"), &context(1));

        assert_eq!(first_group_status(&reducer), RowStatus::Running);
    }

    #[test]
    fn run_failed_marks_an_interrupted_tool_failed() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-1"), &context(1));

        reducer.apply_agent_update(
            AgentUpdate::RunFailed {
                run_id: "run-1".into(),
            },
            &context(1),
        );

        assert_eq!(first_tool(&reducer).status, RowStatus::Failed);
        assert_eq!(first_group_status(&reducer), RowStatus::Failed);
        assert_eq!(
            reducer.latest_tool_view().map(|tool| tool.status),
            Some(RowStatus::Failed)
        );
    }

    #[test]
    fn a_cancelled_step_result_stays_cancelled() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-1"), &context(1));
        let result = StepResult {
            tool: "bash".into(),
            arg_summary: "sleep 30".into(),
            arg_full: None,
            status: StepStatus::Cancelled,
            message: "Cancelled by user".into(),
            detail: None,
            duration_us: Some(1_000),
            permission_label: None,
            presentation: ToolPresentationInfo::generic("bash"),
        };
        reducer.apply_agent_update(
            AgentUpdate::StepFinished {
                idx: 0,
                run_id: Some("run-1".into()),
                tool_id: "tool-1".into(),
                result,
            },
            &context(1),
        );

        assert_eq!(first_tool(&reducer).status, RowStatus::Cancelled);
        assert_eq!(first_group_status(&reducer), RowStatus::Cancelled);
        assert_eq!(
            reducer.latest_tool_view().map(|tool| tool.status),
            Some(RowStatus::Cancelled)
        );
    }

    #[test]
    fn queued_tools_are_visible_before_they_start() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(
            AgentUpdate::StepQueued {
                idx: 0,
                tool_id: "tool-queued".into(),
                tool_name: "bash".into(),
                arg_summary: "cargo test".into(),
                arg_full: "cargo test".into(),
                presentation: ToolPresentationInfo::generic("bash"),
            },
            &context(1),
        );

        assert_eq!(first_tool(&reducer).status, RowStatus::Pending);
        assert_eq!(
            reducer.latest_tool_view().map(|tool| tool.status),
            Some(RowStatus::Pending)
        );
        let row = first_tool(&reducer).id;

        reducer.apply_agent_update(
            AgentUpdate::StepStarted {
                idx: 0,
                tool_id: "tool-queued".into(),
                tool_name: "bash".into(),
                arg_summary: "cargo test".into(),
                arg_full: "cargo test".into(),
                presentation: ToolPresentationInfo::generic("bash"),
            },
            &context(1),
        );

        assert_eq!(first_tool(&reducer).id, row);
        assert_eq!(first_tool(&reducer).status, RowStatus::Running);
    }

    #[test]
    fn a_queued_tool_moves_below_a_visible_row_before_it_starts() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(
            AgentUpdate::StepQueued {
                idx: 0,
                tool_id: "tool-queued".into(),
                tool_name: "bash".into(),
                arg_summary: "cargo test".into(),
                arg_full: "cargo test".into(),
                presentation: ToolPresentationInfo::generic("bash"),
            },
            &context(1),
        );
        let old_group = group_rows(&reducer)[0].id;
        reducer.apply_agent_update(
            AgentUpdate::HookStatus {
                id: 1,
                source: Some("pre-tool".into()),
                message: "checking".into(),
                elapsed_ms: Some(1),
            },
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::StepStarted {
                idx: 0,
                tool_id: "tool-queued".into(),
                tool_name: "bash".into(),
                arg_summary: "cargo test".into(),
                arg_full: "cargo test".into(),
                presentation: ToolPresentationInfo::generic("bash"),
            },
            &context(1),
        );

        let kinds = reducer
            .rows
            .iter()
            .filter_map(|row| match row.body {
                RowBody::HookStatus(_) => Some("hook"),
                RowBody::ToolGroup(_) => Some("group"),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(kinds, ["hook", "group"]);
        assert_ne!(group_rows(&reducer)[0].id, old_group);
        assert_eq!(first_tool(&reducer).status, RowStatus::Running);
        assert_eq!(first_tool(&reducer).tool_id.as_ref(), "tool-queued");
    }

    #[test]
    fn keep_live_progress_remains_visible_after_step_finished() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-background");
        turn(&mut reducer);
        let mut presentation = ToolPresentationInfo::generic("background_run");
        presentation.keep_live = true;
        reducer.apply_agent_update(
            AgentUpdate::StepStarted {
                idx: 0,
                tool_id: "bg-1".into(),
                tool_name: "background_run".into(),
                arg_summary: "for i in 1 2 3".into(),
                arg_full: "for i in 1 2 3".into(),
                presentation: presentation.clone(),
            },
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::StepFinished {
                idx: 0,
                run_id: Some("run-background".into()),
                tool_id: "bg-1".into(),
                result: StepResult {
                    tool: "background_run".into(),
                    arg_summary: "for i in 1 2 3".into(),
                    arg_full: None,
                    status: StepStatus::Success,
                    message: "Background task bg-1 started".into(),
                    detail: None,
                    duration_us: None,
                    permission_label: None,
                    presentation,
                },
            },
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::ToolProgress {
                run_id: None,
                tool_id: "bg-1".into(),
                chunks: vec![ToolOutputChunk::stdout("tick 1\n")],
            },
            &context(1),
        );

        let tool = first_tool(&reducer);
        assert_eq!(tool.status, RowStatus::Running);
        assert!(matches!(tool.body, ToolBody::None));
        assert!(
            tool.output
                .detail
                .as_deref()
                .is_some_and(|output| output.contains("tick 1")),
            "live background output must remain in the rendered output"
        );
    }

    #[test]
    fn tool_progress_detail_is_bounded_to_its_tail() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-long");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-long"), &context(1));
        reducer.apply_agent_update(
            AgentUpdate::ToolProgress {
                run_id: Some("run-long".into()),
                tool_id: "tool-long".into(),
                chunks: vec![ToolOutputChunk::stdout(format!(
                    "{}TAIL",
                    "x".repeat(60_000)
                ))],
            },
            &context(1),
        );

        let tool = first_tool(&reducer);
        let detail = tool.output.detail.expect("bounded detail");
        assert_eq!(detail.chars().count(), 50_000);
        assert!(detail.ends_with("TAIL"));
        assert!(tool.output.truncated);
    }

    #[test]
    fn pending_out_of_band_run_does_not_close_the_active_tool() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-active");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-active"), &context(1));

        reducer.apply_agent_update(
            AgentUpdate::RunStarted {
                run_id: "run-compact".into(),
            },
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::RunFinished {
                run_id: "run-compact".into(),
            },
            &context(1),
        );

        assert_eq!(first_tool(&reducer).status, RowStatus::Running);
        assert_eq!(
            reducer.latest_tool_view().map(|tool| tool.tool_id),
            Some(Arc::from("tool-active"))
        );
    }

    #[test]
    fn a_command_card_shows_its_output_not_the_command() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-command");
        turn(&mut reducer);
        reducer.apply_agent_update(command_started("bash-1", "cargo test"), &context(1));

        assert!(matches!(first_tool(&reducer).body, ToolBody::None));
        reducer.apply_agent_update(
            AgentUpdate::ToolProgress {
                run_id: Some("run-command".into()),
                tool_id: "bash-1".into(),
                chunks: vec![ToolOutputChunk::stdout("partial output\n")],
            },
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::StepFailed {
                idx: 0,
                run_id: Some("run-command".into()),
                tool_id: "bash-1".into(),
                arg_summary: "cargo test".into(),
                error: "Error: exit 1".into(),
            },
            &context(1),
        );

        let tool = first_tool(&reducer);
        assert!(matches!(tool.body, ToolBody::None));
        let output = tool.output.detail.as_deref().unwrap_or("");
        assert!(output.contains("partial output"), "{output}");
        assert!(output.contains("Error: exit 1"), "{output}");
    }

    #[test]
    fn a_user_collapsed_card_stays_collapsed_when_a_tool_finishes() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-collapse");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-collapse"), &context(1));
        let group = group_rows(&reducer)[0].id;
        assert_eq!(reducer.toggle_group(group), Some(true));

        reducer.apply_agent_update(step_finished("tool-collapse"), &context(1));
        assert!(matches!(
            &group_rows(&reducer)[0].body,
            RowBody::ToolGroup(group) if group.collapsed
        ));

        reducer.apply_agent_update(
            AgentUpdate::RunFinished {
                run_id: "run-collapse".into(),
            },
            &context(1),
        );
        assert!(matches!(
            &group_rows(&reducer)[0].body,
            RowBody::ToolGroup(group) if group.collapsed
        ));
    }

    #[test]
    fn a_subagent_tool_is_one_row_after_both_halves() {
        use tact_protocol::subagent_transcript::SubagentStepStatus;

        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-child");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("spawn-1"), &context(1));

        reducer.apply_agent_update(
            child_step(SubagentStepStatus::Running, "", None),
            &context(1),
        );
        reducer.apply_agent_update(
            child_step(SubagentStepStatus::Success, "ok", Some(2_000_000)),
            &context(1),
        );

        let nested = nested_tools(&reducer);
        assert_eq!(nested.len(), 1, "both halves are one child card");
        assert_eq!(nested[0].status, RowStatus::Success);
        assert_eq!(nested[0].duration_ms, Some(2_000));
    }

    #[test]
    fn parallel_subagent_steps_pair_by_tool_id() {
        use tact_protocol::subagent_transcript::{
            SubagentStep, SubagentStepStatus, SubagentTranscriptEntry,
        };

        let child_step = |tool_id: &str, status: SubagentStepStatus| AgentUpdate::ToolTranscript {
            tool_id: "spawn-1".into(),
            entry: SubagentTranscriptEntry::Step(SubagentStep {
                tool_id: tool_id.into(),
                tool_name: "bash".into(),
                arg_summary: tool_id.into(),
                arg_full: tool_id.into(),
                status,
                message: format!("{tool_id} {status:?}"),
                detail: None,
                duration_us: None,
                presentation: ToolPresentationInfo::generic("bash"),
            }),
        };

        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-child");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("spawn-1"), &context(1));
        reducer.apply_agent_update(
            child_step("child-a", SubagentStepStatus::Running),
            &context(1),
        );
        reducer.apply_agent_update(
            child_step("child-b", SubagentStepStatus::Running),
            &context(1),
        );
        reducer.apply_agent_update(
            child_step("child-a", SubagentStepStatus::Success),
            &context(1),
        );
        reducer.apply_agent_update(
            child_step("child-b", SubagentStepStatus::Failed),
            &context(1),
        );

        let nested = nested_tools(&reducer);
        assert_eq!(nested.len(), 2, "completion must not create duplicate rows");
        assert!(matches!(
            nested.iter().find(|tool| tool.tool_id.as_ref() == "child-a"),
            Some(tool) if tool.status == RowStatus::Success
        ));
        assert!(matches!(
            nested.iter().find(|tool| tool.tool_id.as_ref() == "child-b"),
            Some(tool) if tool.status == RowStatus::Failed
        ));
    }

    #[test]
    fn final_subagent_assistant_text_is_not_left_running() {
        use tact_protocol::subagent_transcript::SubagentTranscriptEntry;

        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-child");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("spawn-1"), &context(1));
        reducer.apply_agent_update(
            AgentUpdate::ToolTranscript {
                tool_id: "spawn-1".into(),
                entry: SubagentTranscriptEntry::Assistant("finished".into()),
            },
            &context(1),
        );

        let nested = first_tool(&reducer).subagent.expect("nested transcript");
        let status = nested.iter().find_map(|row| match &row.body {
            RowBody::Assistant(assistant) => Some(assistant.status),
            _ => None,
        });
        assert_eq!(status, Some(RowStatus::Success));
    }

    #[test]
    fn a_nested_tool_disclosure_reaches_the_child_row() {
        use tact_protocol::subagent_transcript::SubagentStepStatus;

        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-nested");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("spawn-1"), &context(1));
        reducer.apply_agent_update(
            child_step(SubagentStepStatus::Running, "", None),
            &context(1),
        );
        reducer.apply_agent_update(
            child_step(SubagentStepStatus::Success, "ok", Some(2_000_000)),
            &context(1),
        );
        let parent = first_tool(&reducer);
        let nested = parent.subagent.expect("nested transcript");
        let (group_id, child_id) = nested
            .iter()
            .find_map(|row| match &row.body {
                RowBody::ToolGroup(group) => group.items.iter().find_map(|item| match item {
                    ToolGroupItem::Tool(tool) if tool.tool_id.as_ref() == "child-1" => {
                        Some((row.id, tool.id))
                    }
                    _ => None,
                }),
                _ => None,
            })
            .expect("nested child card");

        assert_eq!(reducer.toggle_group(group_id), Some(false));
        assert_eq!(reducer.toggle_tool(group_id, child_id), Some(true));
        assert_eq!(
            reducer.top_level_row_for(group_id),
            Some(group_rows(&reducer)[0].id)
        );

        let nested = first_tool(&reducer).subagent.expect("nested transcript");
        let group = nested
            .iter()
            .find(|row| row.id == group_id)
            .expect("nested group");
        assert!(matches!(
            &group.body,
            RowBody::ToolGroup(group) if !group.collapsed
        ));
        let child = nested_tools(&reducer)
            .into_iter()
            .find(|tool| tool.id == child_id)
            .expect("nested child");
        assert!(child.expanded);
    }

    #[test]
    fn an_unpaired_persisted_tool_is_cancelled() {
        let tool = AgentMessage::new(
            AgentRole::Assistant,
            MessageOrigin::Assistant,
            vec![AgentContentBlock::ToolUse {
                id: "tool-history".into(),
                name: "bash".into(),
                input: json!({"command": "cargo test"}),
            }],
        );
        let mut reducer = TranscriptReducer::new();
        reducer.seed_persisted(&[tool], &context(1));

        assert_eq!(first_tool(&reducer).status, RowStatus::Cancelled);
        assert!(matches!(
            &group_rows(&reducer)[0].body,
            RowBody::ToolGroup(group) if group.status() == RowStatus::Cancelled
        ));
    }

    #[test]
    fn a_settled_persisted_group_starts_collapsed() {
        let tool = AgentMessage::new(
            AgentRole::Assistant,
            MessageOrigin::Assistant,
            vec![AgentContentBlock::ToolUse {
                id: "tool-history".into(),
                name: "bash".into(),
                input: json!({"command": "cargo test"}),
            }],
        );
        let result = AgentMessage::new(
            AgentRole::User,
            MessageOrigin::ToolResult,
            vec![AgentContentBlock::ToolResult {
                tool_use_id: "tool-history".into(),
                content: "ok".into(),
                status: tact_agent_api::ToolResultStatus::Success,
            }],
        );
        let mut reducer = TranscriptReducer::new();
        reducer.seed_persisted(&[tool, result], &context(1));

        assert!(matches!(
            &group_rows(&reducer)[0].body,
            RowBody::ToolGroup(group)
                if group.status() == RowStatus::Success && group.collapsed
        ));
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
            RowBody::ToolGroup(group)
                if group.status() == RowStatus::Cancelled && !group.collapsed
        ));
        assert_eq!(first_tool(&reducer).status, RowStatus::Cancelled);
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
    fn one_run_allocates_one_loop_id() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        let loop_id = reducer.active_loop;
        assert!(loop_id.is_some());
        turn(&mut reducer);
        assert_eq!(reducer.active_loop, loop_id);
        reducer.apply_agent_update(
            AgentUpdate::RunFinished {
                run_id: "run-1".into(),
            },
            &context(1),
        );
        run_started(&mut reducer, "run-2");
        assert_eq!(reducer.active_loop, None);
    }

    #[test]
    fn consecutive_calls_share_one_card_across_turns() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-1"), &context(1));
        reducer.apply_agent_update(step_finished("tool-1"), &context(1));
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-2"), &context(1));

        let groups = group_rows(&reducer);
        assert_eq!(groups.len(), 1, "back-to-back calls stay in one card");
        assert_eq!(
            groups[0].loop_id,
            Some(LoopId(0)),
            "the card names the run's loop"
        );
        assert!(matches!(
            &groups[0].body,
            RowBody::ToolGroup(group) if group.items.len() == 2
        ));
    }

    #[test]
    fn reasoning_regions_split_tool_cards() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(tact_protocol::ThinkingChunk::Started),
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(tact_protocol::ThinkingChunk::Delta("first".into())),
            &context(1),
        );
        reducer.apply_agent_update(step_started("tool-1"), &context(1));
        reducer.apply_agent_update(step_finished("tool-1"), &context(1));

        turn(&mut reducer);
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(tact_protocol::ThinkingChunk::Started),
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(tact_protocol::ThinkingChunk::Delta("second".into())),
            &context(1),
        );
        reducer.apply_agent_update(step_started("tool-2"), &context(1));

        let kinds = reducer
            .rows
            .iter()
            .map(|row| match &row.body {
                RowBody::Thinking(_) => "thinking",
                RowBody::ToolGroup(_) => "group",
                _ => "other",
            })
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            ["thinking", "group", "thinking", "group"],
            "visible reasoning ends the previous tool region"
        );
        assert!(
            group_rows(&reducer).iter().all(
                |row| matches!(&row.body, RowBody::ToolGroup(group) if group.items.len() == 1)
            )
        );
    }

    #[test]
    fn visible_reasoning_closes_a_tool_group() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-1"), &context(1));
        reducer.apply_agent_update(step_finished("tool-1"), &context(1));
        assert!(
            matches!(
                &group_rows(&reducer)[0].body,
                RowBody::ToolGroup(group) if !group.collapsed
            ),
            "one step finishing must not hide the rest of the turn behind the group header"
        );

        reducer.apply_agent_update(step_started("tool-2"), &context(1));
        assert!(matches!(
            &group_rows(&reducer)[0].body,
            RowBody::ToolGroup(group) if !group.collapsed && group.items.len() == 2
        ));

        reducer.apply_agent_update(step_finished("tool-2"), &context(1));
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(tact_protocol::ThinkingChunk::Started),
            &context(1),
        );
        assert_eq!(group_rows(&reducer).len(), 1);
        assert!(matches!(
            &group_rows(&reducer)[0].body,
            RowBody::ToolGroup(group) if group.collapsed
        ));

        reducer.apply_agent_update(step_started("tool-3"), &context(1));
        let groups = group_rows(&reducer);
        assert_eq!(groups.len(), 2);
        assert!(matches!(
            &groups[1].body,
            RowBody::ToolGroup(group) if group.items.len() == 1
        ));
    }

    #[test]
    fn later_visible_rows_do_not_move_the_first_tool() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-card");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-1"), &context(1));
        let first = reducer.latest_tool_view().expect("first tool");
        let group = first.group.expect("group");
        let row = first.row.expect("row");

        // Reasoning and an answer close the first region. The next call gets
        // its own card below those rows.
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(tact_protocol::ThinkingChunk::Started),
            &context(1),
        );
        reducer.apply_agent_update(AgentUpdate::StreamChunk("answer".into()), &context(1));
        reducer.apply_agent_update(step_finished("tool-1"), &context(1));
        reducer.apply_agent_update(step_started("tool-2"), &context(1));

        let groups = group_rows(&reducer);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].id, group, "the card keeps its identity");
        let RowBody::ToolGroup(items) = &groups[0].body else {
            panic!("expected a tool group");
        };
        assert_eq!(items.items.len(), 1);
        assert_eq!(items.items[0].id(), row, "the first tool never moved");
        assert!(matches!(
            &items.items[0],
            ToolGroupItem::Tool(tool) if tool.status == RowStatus::Success
        ));
        assert!(matches!(
            &groups[1].body,
            RowBody::ToolGroup(items) if matches!(
                &items.items[0],
            ToolGroupItem::Tool(tool) if tool.id != row && tool.status == RowStatus::Running
            )
        ));
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
    fn failed_tool_keeps_its_streamed_output() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-1"), &context(1));
        reducer.apply_agent_update(
            AgentUpdate::ToolProgress {
                run_id: Some("run-1".into()),
                tool_id: "tool-1".into(),
                chunks: vec![ToolOutputChunk::stdout("partial output")],
            },
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::StepFailed {
                idx: 0,
                run_id: Some("run-1".into()),
                tool_id: "tool-1".into(),
                arg_summary: "README.md".into(),
                error: "exit status 1".into(),
            },
            &context(1),
        );
        let groups = group_rows(&reducer);
        let tools: Vec<_> = match &groups[0].body {
            RowBody::ToolGroup(group) => group
                .items
                .iter()
                .filter(|item| matches!(item, ToolGroupItem::Tool(_)))
                .collect(),
            _ => unreachable!(),
        };
        assert!(matches!(
            tools[0],
            ToolGroupItem::Tool(tool)
                if tool.status == RowStatus::Failed
                    && tool.expanded
                    && tool
                        .output
                        .detail
                        .as_deref()
                        .is_some_and(|output| output.contains("partial output")
                            && output.contains("exit status 1"))
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
    fn subagent_identity_survives_partial_tool_meta_updates() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(step_started("tool-1"), &context(1));
        reducer.apply_agent_update(
            AgentUpdate::ToolMeta {
                tool_id: "tool-1".into(),
                model: None,
                token_usage: None,
                context_window: None,
                task_id: None,
                subagent_id: Some("child-1".into()),
            },
            &context(1),
        );
        // Child token/model updates carry no child id; they must not erase it.
        reducer.apply_agent_update(
            AgentUpdate::ToolMeta {
                tool_id: "tool-1".into(),
                model: Some("mock".into()),
                token_usage: None,
                context_window: None,
                task_id: None,
                subagent_id: None,
            },
            &context(1),
        );
        let group = &group_rows(&reducer)[0];
        let RowBody::ToolGroup(row) = &group.body else {
            panic!("expected a tool group");
        };
        assert!(matches!(
            &row.items[0],
            ToolGroupItem::Tool(tool)
                if tool.meta.subagent_id.as_deref() == Some("child-1")
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
    fn persisted_consecutive_tool_messages_share_one_group() {
        let mut reducer = TranscriptReducer::new();
        let tool = |id: &str| {
            AgentMessage::new(
                AgentRole::Assistant,
                MessageOrigin::Assistant,
                vec![AgentContentBlock::ToolUse {
                    id: id.into(),
                    name: "bash".into(),
                    input: json!({"command": "true"}),
                }],
            )
        };
        let result = |tool_id: &str| {
            AgentMessage::new(
                AgentRole::User,
                MessageOrigin::ToolResult,
                vec![AgentContentBlock::ToolResult {
                    tool_use_id: tool_id.into(),
                    content: "ok".into(),
                    status: tact_agent_api::ToolResultStatus::Success,
                }],
            )
        };
        // Each model step is its own persisted message; a tool result between
        // them draws no row, so the two calls are still consecutive on screen.
        reducer.seed_persisted(
            &[
                tool("tool-1"),
                result("tool-1"),
                tool("tool-2"),
                result("tool-2"),
            ],
            &context(1),
        );
        let groups = group_rows(&reducer);
        assert_eq!(groups.len(), 1, "consecutive calls share one card");
        assert!(matches!(
            &groups[0].body,
            RowBody::ToolGroup(row) if row.items.len() == 2
        ));
    }

    #[test]
    fn persisted_user_messages_break_the_run_group() {
        let tool = |id: &str| {
            AgentMessage::new(
                AgentRole::Assistant,
                MessageOrigin::Assistant,
                vec![AgentContentBlock::ToolUse {
                    id: id.into(),
                    name: "bash".into(),
                    input: json!({"command": "true"}),
                }],
            )
        };

        // Injected context is not a user turn, but it is visible content, so
        // the next tool starts a new region.
        let mut reducer = TranscriptReducer::new();
        reducer.seed_persisted(
            &[
                tool("tool-1"),
                AgentMessage::text(
                    AgentRole::User,
                    MessageOrigin::InjectedContext {
                        source: "hook".into(),
                    },
                    "briefing",
                ),
                tool("tool-2"),
            ],
            &context(1),
        );
        assert_eq!(group_rows(&reducer).len(), 2);

        // A real user message starts the next run and its own card.
        let mut reducer = TranscriptReducer::new();
        reducer.seed_persisted(
            &[
                tool("tool-1"),
                AgentMessage::text(AgentRole::User, MessageOrigin::User, "next"),
                tool("tool-2"),
            ],
            &context(1),
        );
        let groups = group_rows(&reducer);
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().all(|row| matches!(
            &row.body,
            RowBody::ToolGroup(group) if group.items.len() == 1
        )));
    }

    #[test]
    fn persisted_history_keeps_tool_regions_chronological() {
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
        let groups = group_rows(&reducer);
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().all(|row| matches!(
            &row.body,
            RowBody::ToolGroup(row) if row.items.len() == 1
        )));
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
                status: tact_agent_api::ToolResultStatus::Success,
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
    fn persisted_tool_result_preserves_failed_and_unknown_status() {
        let tool = |id: &str| {
            AgentMessage::new(
                AgentRole::Assistant,
                MessageOrigin::Assistant,
                vec![AgentContentBlock::ToolUse {
                    id: id.into(),
                    name: "bash".into(),
                    input: json!({"command": "false"}),
                }],
            )
        };
        let result = |id: &str, status| {
            AgentMessage::new(
                AgentRole::User,
                MessageOrigin::ToolResult,
                vec![AgentContentBlock::ToolResult {
                    tool_use_id: id.into(),
                    content: "failed".into(),
                    status,
                }],
            )
        };

        let mut reducer = TranscriptReducer::new();
        reducer.seed_persisted(
            &[
                tool("failed"),
                result("failed", tact_agent_api::ToolResultStatus::Failed),
                tool("cancelled"),
                result("cancelled", tact_agent_api::ToolResultStatus::Cancelled),
                tool("legacy"),
                result("legacy", tact_agent_api::ToolResultStatus::Unknown),
            ],
            &context(1),
        );
        let statuses = group_rows(&reducer)
            .into_iter()
            .flat_map(|row| match &row.body {
                RowBody::ToolGroup(group) => group.items.iter().cloned().collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .filter_map(|item| match item {
                ToolGroupItem::Tool(tool) => Some((tool.tool_id.to_string(), tool.status)),
                ToolGroupItem::Gate(_) => None,
            })
            .collect::<HashMap<_, _>>();

        assert_eq!(statuses.get("failed"), Some(&RowStatus::Failed));
        assert_eq!(statuses.get("cancelled"), Some(&RowStatus::Cancelled));
        assert_eq!(statuses.get("legacy"), Some(&RowStatus::Unknown));
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
    fn a_repeated_thinking_started_keeps_the_open_card() {
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
        assert_eq!(thinking, ["firstsecond"]);
    }

    #[test]
    fn message_started_keeps_the_open_thinking_card() {
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
        assert_eq!(thinking, ["a-bodyb-body"]);
    }

    #[test]
    fn tool_output_ends_the_thinking_region() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Started),
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Delta("before".into())),
            &context(1),
        );
        reducer.apply_agent_update(step_started("tool-1"), &context(1));
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Started),
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Delta("after".into())),
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
        assert_eq!(thinking, ["before", "after"]);
    }

    #[test]
    fn stream_text_ends_the_thinking_region() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Started),
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Delta("before".into())),
            &context(1),
        );
        reducer.apply_agent_update(AgentUpdate::StreamChunk("answer".into()), &context(1));
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Started),
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Delta("after".into())),
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
        assert_eq!(thinking, ["before", "after"]);
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
        let status_before = match &group_rows(&reducer)[0].body {
            RowBody::ToolGroup(group) => group.status(),
            _ => panic!("expected a tool group"),
        };
        let tool_before = first_tool(&reducer).status;
        run_started(&mut reducer, "run-2");
        reducer.apply_agent_update(
            AgentUpdate::Error(tact_protocol::AgentErrorKind::Other(
                "new run failed".into(),
            )),
            &context(1),
        );
        let status_after = match &group_rows(&reducer)[0].body {
            RowBody::ToolGroup(group) => group.status(),
            _ => panic!("expected a tool group"),
        };
        assert_eq!(status_after, status_before);
        assert_eq!(first_tool(&reducer).status, tool_before);
    }

    #[test]
    fn tool_summaries_flatten_to_one_line() {
        assert_eq!(one_line_summary("echo  a\n\nb\tc"), "echo a b c");
        assert_eq!(one_line_summary(""), "");
        // The length is the display layer's business: a search query stays
        // intact so it can still be parsed.
        let long = "cargo ".repeat(40);
        let summary = one_line_summary(&long);
        assert_eq!(summary, "cargo ".repeat(40).trim_end());
        assert!(!summary.contains('\n'));
    }

    #[test]
    fn a_command_summary_stays_one_line_on_the_tool_row() {
        let mut reducer = TranscriptReducer::new();
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(
            AgentUpdate::StepStarted {
                idx: 0,
                tool_id: "tool-1".into(),
                tool_name: "bash".into(),
                arg_summary: "export A=1\nexport B=2\npython3 -c 'print(1)'".into(),
                arg_full: "export A=1\nexport B=2\npython3 -c 'print(1)'".into(),
                presentation: ToolPresentationInfo::generic("bash"),
            },
            &context(1),
        );
        let tool = group_rows(&reducer)
            .iter()
            .find_map(|row| match &row.body {
                RowBody::ToolGroup(group) => group.items.iter().find_map(|item| match item {
                    ToolGroupItem::Tool(tool) => Some(tool.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .expect("tool row");
        assert_eq!(
            tool.arg_summary.as_ref(),
            "export A=1 export B=2 python3 -c 'print(1)'"
        );
    }

    #[test]
    fn a_repeated_hook_briefing_stays_one_card() {
        let mut reducer = TranscriptReducer::new();
        let briefing = |text: &str| {
            AgentMessage::text(
                AgentRole::User,
                MessageOrigin::InjectedContext {
                    source: "plugin auto-memory".into(),
                },
                tact::hook::frame_hook_context(Some("plugin auto-memory"), text),
            )
        };
        let live = |text: &str| AgentUpdate::HookContext {
            source: Some("plugin auto-memory".into()),
            text: text.into(),
        };
        // Every open of the session stored another copy of the same briefing.
        reducer.seed_persisted(&[briefing("briefing"), briefing("briefing")], &context(1));
        reducer.apply_agent_update(live("briefing"), &context(1));
        let cards = |reducer: &TranscriptReducer| {
            reducer
                .rows
                .iter()
                .filter_map(|row| match &row.body {
                    RowBody::HookContext(card) => Some(card.markdown.to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(cards(&reducer), vec!["briefing".to_string()]);

        // A hook with fresh context still adds its card.
        reducer.apply_agent_update(live("new briefing"), &context(1));
        assert_eq!(
            cards(&reducer),
            vec!["briefing".to_string(), "new briefing".to_string()]
        );
    }

    #[test]
    fn an_invisible_hook_event_keeps_the_thinking_region_open() {
        let mut reducer = TranscriptReducer::new();
        reducer.push_hook_context(
            Some(Arc::from("plugin auto-memory")),
            Arc::from("briefing"),
            None,
        );
        run_started(&mut reducer, "run-1");
        turn(&mut reducer);
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Started),
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Delta("before ".into())),
            &context(1),
        );
        // The same briefing arrives mid-thought and adds no card.
        reducer.apply_agent_update(
            AgentUpdate::HookContext {
                source: Some("plugin auto-memory".into()),
                text: "briefing".into(),
            },
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Delta("after".into())),
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
        assert_eq!(thinking, ["before after"]);

        // A hidden hook status must not split the card either.
        let hidden = ReduceContext {
            session_epoch: SessionEpoch(1),
            show_hook_output: false,
        };
        reducer.apply_agent_update(
            AgentUpdate::HookStatus {
                id: 1,
                source: Some("plugin auto-memory".into()),
                message: "Loading Auto Memory context".into(),
                elapsed_ms: Some(21),
            },
            &hidden,
        );
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(" - still one".into())),
            &hidden,
        );
        let thinking = reducer
            .rows
            .iter()
            .filter_map(|row| match &row.body {
                RowBody::Thinking(thinking) => Some(thinking.markdown.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(thinking, ["before after - still one"]);

        // With hook output shown, the status row is real output and ends it.
        reducer.apply_agent_update(
            AgentUpdate::HookStatus {
                id: 2,
                source: Some("plugin auto-memory".into()),
                message: "Loading Auto Memory context".into(),
                elapsed_ms: Some(22),
            },
            &context(1),
        );
        reducer.apply_agent_update(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Delta("next".into())),
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
        assert_eq!(thinking, ["before after - still one", "next"]);
    }
}

use std::{collections::HashMap, sync::Arc};

use chrono::Utc;
use tact::store::ArchiveFilter;
use tact_protocol::{
    AccountUpdate, AgentUpdate, GoalSnapshot, SubagentRunSnapshot, TaskSnapshot, TokenUsageInfo,
};

use crate::{
    AccountView, Appearance, BackgroundTaskView, CatalogLocale, CommandCatalogView, ComposerView,
    DesktopConfig, DesktopSnapshot, ExtensionsView, FailureView, GateCard, GoalView, InspectorTab,
    InspectorView, ProjectRecord, ProjectView, ReducedMotion, Revision, RowId, RunPhase,
    RuntimeView, SectionMask, SessionEpoch, SessionListState, SessionPhase, SessionSummaryView,
    SessionView, StateCommit, SubagentView, TaskView, TranscriptReducer, UsageView,
};

pub struct DesktopState {
    pub project: ProjectView,
    pub projects: Vec<ProjectView>,
    pub sessions: SessionListState,
    pub session: SessionView,
    pub runtime: RuntimeView,
    pub(crate) reducer: TranscriptReducer,
    pub composer: ComposerView,
    pub inspector: InspectorView,
    pub catalog: Arc<CommandCatalogView>,
    pub extensions: ExtensionsView,
    pub account: Option<AccountView>,
    pub account_supported: bool,
    pub show_hook_output: bool,
    pub notifications: bool,
    pub failure: Option<FailureView>,
    pub appearance: Appearance,
    pub motion: ReducedMotion,
    pub text_scale: f32,
    pub language: tact_catalog::CatalogLocale,
    pub tail_following: bool,
    pub tasks: Vec<TaskView>,
    pub goals: Vec<GoalView>,
    pub subagents: Vec<SubagentView>,
    subagent_meta: HashMap<String, SubagentMetadata>,
    pub background: Vec<BackgroundTaskView>,
    pub surface: Option<tact_protocol::SurfaceSnapshot>,
    pub revision: Revision,
    next_epoch: u64,
    settings: crate::SettingsView,
    /// A kernel activation that has not produced a turn yet. Compaction and
    /// Goal activations emit `RunStarted`; only a following `TurnStats`
    /// promotes it into a visible run.
    pending_run: Option<String>,
    /// The activation that owns the current `RuntimeView` timer.
    active_run: Option<String>,
}

impl DesktopState {
    pub fn new(config: &DesktopConfig) -> Self {
        let catalog = tact_catalog::builtins()
            .search("", config.language)
            .into_iter()
            .collect::<Vec<_>>();
        let settings = tact::config::try_settings();
        let mut models: Vec<Arc<str>> = settings
            .as_ref()
            .map(|settings| {
                settings
                    .llm
                    .models
                    .iter()
                    .cloned()
                    .map(Arc::from)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(model) = config.host.model.as_deref()
            && !models.iter().any(|candidate| candidate.as_ref() == model)
        {
            models.insert(0, Arc::from(model));
        }
        let mut subagent_models: Vec<Arc<str>> = settings
            .as_ref()
            .map(|settings| {
                settings
                    .agent
                    .subagent
                    .as_ref()
                    .map(|subagent| {
                        subagent
                            .models
                            .iter()
                            .cloned()
                            .map(Arc::from)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        let subagent_model = settings
            .as_ref()
            .and_then(|settings| settings.agent.subagent.as_ref())
            .map(|subagent| subagent.provider.model.clone())
            .unwrap_or_default();
        if !subagent_model.is_empty()
            && !subagent_models
                .iter()
                .any(|candidate| candidate.as_ref() == subagent_model)
        {
            subagent_models.insert(0, Arc::from(subagent_model.clone()));
        }
        Self {
            project: ProjectView::default(),
            projects: Vec::new(),
            sessions: SessionListState::default(),
            session: SessionView::default(),
            runtime: RuntimeView::default(),
            reducer: TranscriptReducer::new(),
            composer: ComposerView {
                thinking_budget: settings
                    .as_ref()
                    .map(|settings| settings.agent.thinking_budget as u32),
                model: config.host.model.clone().map(Arc::from).unwrap_or_default(),
                models: models.into(),
                subagent_model: Arc::from(subagent_model),
                subagent_models: subagent_models.into(),
                permission_mode: config.host.permission_mode,
                ..ComposerView::default()
            },
            inspector: InspectorView::default(),
            catalog: Arc::new(CommandCatalogView {
                recent: Arc::from([]),
                items: catalog.into(),
            }),
            extensions: ExtensionsView::default(),
            account: None,
            account_supported: settings.is_some() && tact_host::account::is_supported(),
            show_hook_output: config.show_hook_output,
            notifications: config.notifications,
            failure: None,
            appearance: config.appearance,
            motion: config.reduced_motion,
            text_scale: config.text_scale,
            language: config.language,
            tail_following: true,
            tasks: Vec::new(),
            goals: Vec::new(),
            subagents: Vec::new(),
            subagent_meta: HashMap::new(),
            background: Vec::new(),
            surface: None,
            revision: 0,
            next_epoch: 0,
            settings: crate::snapshot::settings_from(config),
            pending_run: None,
            active_run: None,
        }
    }

    pub fn set_project(&mut self, project: Option<&ProjectRecord>) {
        self.project = project.map_or_else(ProjectView::default, |project| ProjectView {
            id: Some(project.id.clone()),
            root: Some(project.root.clone()),
            display_name: Arc::from(project.display_name.clone()),
            available: project.available,
        });
        self.sessions.project = self.project.id.clone();
    }

    pub fn set_projects(&mut self, projects: &[ProjectRecord]) {
        self.projects = projects
            .iter()
            .map(|project| ProjectView {
                id: Some(project.id.clone()),
                root: Some(project.root.clone()),
                display_name: Arc::from(project.display_name.clone()),
                available: project.available,
            })
            .collect();
    }

    pub fn begin_session_switch(&mut self) -> SessionEpoch {
        self.next_epoch = self.next_epoch.saturating_add(1);
        let next = SessionEpoch(self.next_epoch);
        self.session = SessionView {
            epoch: next,
            phase: SessionPhase::Switching,
            ..SessionView::default()
        };
        self.runtime = RuntimeView::default();
        self.pending_run = None;
        self.active_run = None;
        self.reducer.reset_for_session();
        self.sessions.items.clear();
        self.sessions.selected = None;
        self.sessions.error = None;
        self.sessions.loading = false;
        self.tasks.clear();
        self.goals.clear();
        self.subagents.clear();
        self.subagent_meta.clear();
        self.background.clear();
        self.surface = None;
        self.failure = None;
        next
    }

    pub fn refresh_settings(&mut self, config: &crate::DesktopConfig) {
        self.settings = crate::snapshot::settings_from(config);
    }

    pub fn set_sessions(&mut self, items: Vec<SessionSummaryView>) {
        self.sessions.items = items;
        self.sessions.loading = false;
        self.sessions.error = None;
    }

    pub fn apply_agent_update(&mut self, mut update: AgentUpdate) -> Vec<RowId> {
        self.localize_agent_update(&mut update);
        self.observe_agent_update(&update);
        let context = crate::ReduceContext {
            session_epoch: self.session.epoch,
        };
        let delta = self.reducer.apply_agent_update(update, &context);
        delta.dirty_rows
    }

    pub fn apply_agent_batch(&mut self, mut updates: Vec<AgentUpdate>) -> Vec<RowId> {
        for update in &mut updates {
            self.localize_agent_update(update);
        }
        for update in &updates {
            self.observe_agent_update(update);
        }
        let context = crate::ReduceContext {
            session_epoch: self.session.epoch,
        };
        self.reducer.apply_batch(updates, &context).dirty_rows
    }

    fn localize_agent_update(&self, update: &mut AgentUpdate) {
        let AgentUpdate::Info(message) = update else {
            return;
        };
        let translated = match (message.as_str(), self.language) {
            ("Cancelling...", CatalogLocale::Zh) => Some("正在取消…".to_string()),
            ("[compacting]", CatalogLocale::Zh) => Some("正在压缩…".to_string()),
            ("Compaction complete.", CatalogLocale::Zh) => Some("压缩完成。".to_string()),
            ("Cancelled by user", CatalogLocale::Zh) => Some("用户已取消".to_string()),
            ("(no goals)", CatalogLocale::Zh) => Some("暂无 Goal".to_string()),
            (message, CatalogLocale::Zh) if message.starts_with("Compaction complete (") => {
                let kind = message
                    .trim_start_matches("Compaction complete (")
                    .trim_end_matches(").");
                let kind = match kind {
                    "Auto" => "自动",
                    "Manual" => "手动",
                    other => other,
                };
                Some(format!("压缩完成（{kind}）。"))
            }
            (message, CatalogLocale::Zh) if message.starts_with("[max turns] stopped after ") => {
                Some(
                    message
                        .trim_start_matches("[max turns] stopped after ")
                        .replace(" turn(s)", " 轮后停止，达到最大轮次"),
                )
            }
            (message, CatalogLocale::Zh) if message.starts_with("Background check failed: ") => {
                Some(format!(
                    "后台检查失败：{}",
                    message.trim_start_matches("Background check failed: ")
                ))
            }
            _ => None,
        };
        if let Some(translated) = translated {
            *message = translated;
        }
    }

    fn observe_agent_update(&mut self, update: &AgentUpdate) {
        match update {
            AgentUpdate::RunStarted { run_id } => {
                // Pending boundary: compaction and Goal activations emit this
                // too. Do not start the visible run or its timer until a turn
                // follows.
                self.pending_run = Some(run_id.clone());
            }
            AgentUpdate::MessageStarted { .. } | AgentUpdate::StreamChunk(_) => {
                self.runtime.current_phase = RunPhase::Streaming;
            }
            AgentUpdate::ThinkingChunk(tact_protocol::ThinkingChunk::Started)
            | AgentUpdate::ThinkingChunk(tact_protocol::ThinkingChunk::Delta(_)) => {
                self.runtime.current_phase = RunPhase::Thinking;
            }
            AgentUpdate::TurnStats { turns_taken, .. } => {
                if self.runtime.run_started_at.is_none() {
                    self.runtime.run_started_at = Some(Utc::now());
                }
                if let Some(run_id) = self.pending_run.take() {
                    self.active_run = Some(run_id);
                }
                self.runtime.turns_taken = *turns_taken;
                self.runtime.current_phase = RunPhase::RunningTools;
            }
            AgentUpdate::StepStarted { tool_name, .. } => {
                self.runtime.latest_tool = Some(Arc::from(tool_name.clone()));
                self.runtime.current_phase = RunPhase::RunningTools;
            }
            AgentUpdate::TokenUsage(usage) => self.apply_token_usage(usage),
            AgentUpdate::ModelInfo(model) => {
                self.runtime.model = Some(Arc::from(model.model.clone()))
            }
            AgentUpdate::TaskComplete(_) => {
                self.runtime.current_phase = RunPhase::Complete;
            }
            AgentUpdate::TaskCancelled => {
                self.runtime.current_phase = RunPhase::Cancelling;
            }
            AgentUpdate::RunFinished { run_id } => {
                let finished_pending = self.pending_run.as_deref() == Some(run_id.as_str());
                let finished_active = self.active_run.as_deref() == Some(run_id.as_str());
                if finished_pending {
                    self.pending_run = None;
                }
                if finished_active {
                    self.active_run = None;
                    if self.runtime.current_phase != RunPhase::Cancelling {
                        self.runtime.current_phase = RunPhase::Complete;
                    }
                }
                // A compaction activation that never saw `TurnStats` leaves the
                // previous phase and timer untouched.
            }
            AgentUpdate::TasksChanged { tasks, .. } => {
                self.tasks = tasks.iter().map(task_view).collect();
            }
            AgentUpdate::GoalsChanged { goals } => {
                self.goals = goals
                    .iter()
                    .cloned()
                    .map(|snapshot| GoalView {
                        snapshot: Some(snapshot),
                    })
                    .collect();
            }
            AgentUpdate::SubagentsChanged { runs } => {
                self.subagents = runs
                    .iter()
                    .map(|run| subagent_view(run, self.subagent_meta.get(&run.child_id)))
                    .collect();
            }
            AgentUpdate::BackgroundTasksChanged { tasks } => {
                self.background = tasks.iter().map(background_view).collect();
            }
            AgentUpdate::SurfaceSnapshot { snapshot, .. } => {
                self.surface = Some(snapshot.clone());
            }
            AgentUpdate::ToolMeta {
                tool_id,
                model,
                token_usage,
                context_window,
                subagent_id,
                ..
            } => {
                let Some(child_id) = subagent_id else {
                    return;
                };
                let meta = self.subagent_meta.entry(child_id.clone()).or_default();
                if let Some(model) = model {
                    meta.model = Some(Arc::from(model.as_str()));
                }
                if let Some(usage) = token_usage {
                    meta.context = UsageView {
                        prompt_tokens: usage.prompt,
                        completion_tokens: usage.completion,
                        total_tokens: usage.total,
                        cache_hit_tokens: usage.prompt_cache_hit_tokens,
                        reasoning_tokens: usage.reasoning_tokens,
                        context_window: *context_window,
                    };
                }
                meta.last_activity = Some(Arc::from(tool_id.as_str()));
                if let Some(view) = self
                    .subagents
                    .iter_mut()
                    .find(|view| view.child_id.as_ref() == child_id)
                {
                    apply_subagent_meta(view, meta);
                }
            }
            AgentUpdate::RequestSelect { .. } | AgentUpdate::RequestMultiSelect { .. } => {
                self.runtime.current_phase = RunPhase::WaitingForGate;
            }
            AgentUpdate::Error(_) => {
                self.runtime.current_phase = RunPhase::Failed;
            }
            _ => {}
        }
    }

    pub fn apply_account_update(&mut self, update: AccountUpdate) {
        self.account = Some(match update {
            AccountUpdate::Balance(balance) => {
                let entry = balance.balance_infos.first();
                AccountView::Balance {
                    currency: Arc::from(entry.map(|entry| entry.currency.as_str()).unwrap_or("")),
                    total: entry
                        .map(|entry| entry.total_balance.to_string())
                        .unwrap_or_default(),
                }
            }
            AccountUpdate::UsageQuota(quota) => {
                let window = quota.windows.first();
                AccountView::UsageQuota {
                    plan: quota.membership_level.map(Arc::from),
                    used: window.and_then(|window| {
                        Some((window.limit? - window.remaining?).max(0.0) as u64)
                    }),
                    limit: window.and_then(|window| window.limit.map(|limit| limit as u64)),
                    resets_at: window
                        .and_then(|window| window.reset_time.clone())
                        .map(Arc::from),
                }
            }
            AccountUpdate::Error(error) => AccountView::Error(Arc::from(error.to_string())),
        });
    }

    pub fn pending_gates(&self) -> Vec<GateCard> {
        self.reducer.pending_gates()
    }

    pub fn snapshot(&self) -> Arc<DesktopSnapshot> {
        Arc::new(DesktopSnapshot {
            revision: self.revision,
            projects: self.projects.clone().into(),
            project: self.project.clone(),
            sessions: self.sessions.clone(),
            session: self.session.clone(),
            runtime: self.runtime.clone(),
            transcript: self.reducer.view(),
            composer: self.composer.clone(),
            inspector: InspectorView {
                goals: self.goals.clone().into(),
                tasks: self.tasks.clone().into(),
                subagents: self.subagents.clone().into(),
                background: self.background.clone().into(),
                ..self.inspector.clone()
            },
            pending_gates: self.pending_gates().into(),
            catalog: self.catalog.clone(),
            extensions: self.extensions.clone(),
            account: self.account.clone(),
            account_supported: self.account_supported,
            show_hook_output: self.show_hook_output,
            notifications: self.notifications,
            failure: self.failure.clone(),
            appearance: self.appearance,
            motion: self.motion,
            text_scale: self.text_scale,
            language: self.language,
            tail_following: self.tail_following,
            surface: self.surface.clone(),
            settings: self.settings.clone(),
        })
    }

    pub fn commit(&mut self, dirty_rows: Vec<RowId>, sections: SectionMask) -> StateCommit {
        self.revision = self.revision.saturating_add(1);
        StateCommit {
            revision: self.revision,
            snapshot: self.snapshot(),
            dirty_rows: dirty_rows.into(),
            sections,
        }
    }

    pub fn set_filter(&mut self, filter: ArchiveFilter) {
        self.sessions.filter = filter;
    }

    pub fn set_query(&mut self, query: String) {
        self.sessions.query = query;
    }

    pub fn set_inspector(&mut self, tab: InspectorTab) {
        self.inspector.tab = tab;
    }

    pub fn apply_token_usage(&mut self, usage: &TokenUsageInfo) {
        self.runtime.usage = UsageView {
            prompt_tokens: usage.prompt,
            completion_tokens: usage.completion,
            total_tokens: usage.total,
            cache_hit_tokens: usage.prompt_cache_hit_tokens,
            reasoning_tokens: usage.reasoning_tokens,
            context_window: self.inspector.context.context_window,
        };
        self.inspector.context = self.runtime.usage.clone();
    }
}

fn task_view(task: &TaskSnapshot) -> TaskView {
    TaskView {
        id: task.id,
        subject: Arc::from(task.subject.clone()),
        status: task.status,
        owner: Arc::from(task.owner.clone()),
        blocks: task.blocks.clone().into(),
        blocked_by: task.blocked_by.clone().into(),
    }
}

#[derive(Clone, Debug, Default)]
struct SubagentMetadata {
    model: Option<Arc<str>>,
    context: UsageView,
    last_activity: Option<Arc<str>>,
}

fn subagent_view(run: &SubagentRunSnapshot, metadata: Option<&SubagentMetadata>) -> SubagentView {
    let mut view = SubagentView {
        child_id: Arc::from(run.child_id.clone()),
        status: run.status,
        summary: Arc::from(run.summary_first.clone()),
        model: None,
        context: UsageView::default(),
        elapsed_ms: run
            .finished_at
            .zip(run.started_at)
            .map(|(finished, started)| (finished - started).max(0) as u64 * 1000)
            .unwrap_or(0),
        last_activity: Some(Arc::from(run.summary_first.as_str())),
    };
    if let Some(metadata) = metadata {
        apply_subagent_meta(&mut view, metadata);
    }
    view
}

fn apply_subagent_meta(view: &mut SubagentView, metadata: &SubagentMetadata) {
    view.model = metadata.model.clone();
    view.context = metadata.context.clone();
    view.last_activity = metadata
        .last_activity
        .clone()
        .or_else(|| view.last_activity.clone());
}

fn background_view(task: &tact_protocol::BackgroundTaskSnapshot) -> BackgroundTaskView {
    BackgroundTaskView {
        id: Arc::from(task.id.clone()),
        command: Arc::from(task.command.clone()),
        status: task.status,
        started_at: task.started_at,
        finished_at: task.finished_at,
        output_tail: Arc::from(task.output_tail.clone()),
        output_path: task.output_path.clone().map(Arc::from),
    }
}

impl From<GoalSnapshot> for GoalView {
    fn from(snapshot: GoalSnapshot) -> Self {
        Self {
            snapshot: Some(snapshot),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> (DesktopState, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let config = DesktopConfig::load(tmp.path().to_path_buf()).unwrap();
        (DesktopState::new(&config), tmp)
    }

    #[test]
    fn compaction_run_does_not_open_an_activation_in_the_runtime_view() {
        let (mut state, _tmp) = state();
        state.apply_agent_update(AgentUpdate::RunStarted {
            run_id: "compact-1".into(),
        });
        state.apply_agent_update(AgentUpdate::RunFinished {
            run_id: "compact-1".into(),
        });
        assert_eq!(state.runtime.current_phase, RunPhase::Idle);
        assert!(state.runtime.run_started_at.is_none());
        assert_eq!(state.runtime.turns_taken, 0);
    }

    #[test]
    fn turn_stats_opens_the_run_and_finish_closes_it() {
        let (mut state, _tmp) = state();
        state.apply_agent_update(AgentUpdate::RunStarted {
            run_id: "run-1".into(),
        });
        assert_eq!(state.runtime.current_phase, RunPhase::Idle);
        state.apply_agent_update(AgentUpdate::TurnStats {
            turns_taken: 2,
            max_turns: None,
        });
        assert!(state.runtime.run_started_at.is_some());
        assert_eq!(state.runtime.turns_taken, 2);
        state.apply_agent_update(AgentUpdate::RunFinished {
            run_id: "run-1".into(),
        });
        assert_eq!(state.runtime.current_phase, RunPhase::Complete);
    }
}

//! Work pane: Plan, Diff, Tasks, Subagents, and Files.
//!
//! The pane is a secondary surface, never a second application: every tab reads
//! state the session already produced (plan steps, recorded file changes, task
//! and subagent snapshots, the workspace tree). Nothing here talks to the agent
//! except through commands the shell already exposes.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use gpui_kit::base::{StyledExt as _, TestSupportExt as _};
use gpui_kit::component::{
    ActiveTheme as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    scroll::ScrollableElement as _,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Styled as _, div, rems,
};

use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use tact_protocol::{SubagentStatusSnapshot, TaskStatusSnapshot};

use crate::session::SessionState;
use crate::shell::TactApp;

/// Which surface the work pane shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorkPane {
    /// The agent's execution plan.
    #[default]
    Plan,
    /// File changes recorded from write and edit tools.
    Diff,
    /// Persistent tasks.
    Tasks,
    /// Subagent runs started by this session.
    Subagents,
    /// The workspace tree.
    Files,
}

impl WorkPane {
    /// Every pane in tab order.
    pub const ALL: [Self; 5] = [
        Self::Plan,
        Self::Diff,
        Self::Tasks,
        Self::Subagents,
        Self::Files,
    ];

    /// Tab label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Plan => "Plan",
            Self::Diff => "Diff",
            Self::Tasks => "Tasks",
            Self::Subagents => "Subagent",
            Self::Files => "Files",
        }
    }

    /// Position in [`Self::ALL`].
    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|pane| *pane == self)
            .unwrap_or_default()
    }

    /// The pane at `index`, falling back to [`WorkPane::Plan`].
    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }

    /// How many items the pane is showing, when that count is meaningful.
    ///
    /// The prototype badges Plan, Diff and Tasks but not Files, which is a
    /// tree rather than a counted list.
    fn count(self, state: &crate::session::SessionState) -> Option<usize> {
        match self {
            Self::Plan => Some(state.plan.len()),
            Self::Diff => Some(state.diff.len()),
            Self::Tasks => Some(state.tasks.len()),
            Self::Subagents => Some(state.subagents.len()),
            Self::Files => None,
        }
    }
}

/// Files pane state: which directories the user opened.
///
/// Kept as a path set rather than a node tree so re-reading the directory (the
/// workspace changes underneath the app) cannot invalidate node identities.
#[derive(Default)]
pub struct FilesPane {
    expanded: HashSet<PathBuf>,
}

impl FilesPane {
    /// Whether `path` is expanded.
    fn is_expanded(&self, path: &Path) -> bool {
        self.expanded.contains(path)
    }

    /// Toggle a directory open or closed.
    fn toggle(&mut self, path: &Path) {
        let path = path.to_path_buf();
        if !self.expanded.remove(&path) {
            self.expanded.insert(path);
        }
    }

    /// Flattened rows for the workspace rooted at `root`.
    ///
    /// Depth is capped so a deep or cyclic-looking tree cannot stall the frame;
    /// the pane is a navigation aid, not a file manager.
    pub(crate) fn rows(&self, root: &Path) -> Vec<FileRow> {
        let mut rows = Vec::new();
        collect(root, 0, self, &mut rows);
        rows
    }

    /// Handler for a click on a directory row.
    pub(crate) fn on_toggle(&mut self, path: PathBuf) {
        self.toggle(&path);
    }
}

/// One visible row of the files tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileRow {
    /// Path this row represents.
    pub(crate) path: PathBuf,
    /// File or directory name.
    pub(crate) name: String,
    /// Whether the row is a directory.
    pub(crate) is_dir: bool,
    /// Whether a directory row is open.
    pub(crate) expanded: bool,
    /// Indent depth from the workspace root.
    pub(crate) depth: usize,
}

/// Maximum depth the files pane walks.
const MAX_FILE_DEPTH: usize = 4;

/// Collect `path`'s children into `rows`, recursing into expanded directories.
fn collect(path: &Path, depth: usize, files: &FilesPane, rows: &mut Vec<FileRow>) {
    if depth > MAX_FILE_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };

    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| {
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        (!is_dir, entry.file_name())
    });

    for entry in entries {
        let child = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        // Hidden entries stay out of the tree: `.tact` and `.git` are noise in
        // a navigation surface whose job is "which file is the agent touching".
        if name.starts_with('.') {
            continue;
        }
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        let expanded = is_dir && files.is_expanded(&child);
        rows.push(FileRow {
            path: child.clone(),
            name,
            is_dir,
            expanded,
            depth,
        });
        if expanded {
            collect(&child, depth + 1, files, rows);
        }
    }
}

/// The whole pane: tab strip plus the selected body.
///
/// One entry point rather than separate `tabs`/`content` helpers: both need
/// `&mut Context`, and a single call keeps the two mutable borrows out of the
/// same expression.
pub(crate) fn view(
    selected: WorkPane,
    state: &SessionState,
    files: &FilesPane,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    // Each tab carries the prototype's count badge where the pane has a
    // meaningful cardinality; Files is a tree, so it stays unlabelled.
    let count_bg = cx.theme().background;
    let count_fg = cx.theme().muted_foreground;
    let tabs = TabBar::new("work-pane-tabs")
        .selected_index(selected.index())
        .children(WorkPane::ALL.map(|pane| {
            let tab = Tab::new().label(pane.label());
            match pane.count(state) {
                Some(count) => tab.suffix(
                    div()
                        .rounded(cx.theme().radius)
                        .bg(count_bg)
                        .px_1()
                        .text_xs()
                        .text_color(count_fg)
                        .child(SharedString::from(count.to_string())),
                ),
                None => tab,
            }
        }))
        .on_click(cx.listener(|this, index, _, cx| {
            this.set_work_pane(WorkPane::from_index(*index));
            cx.notify();
        }));

    let body = match selected {
        WorkPane::Plan => plan(state, cx).into_any_element(),
        WorkPane::Diff => diff(state, cx).into_any_element(),
        WorkPane::Tasks => tasks(state, cx).into_any_element(),
        WorkPane::Subagents => subagents(state, cx).into_any_element(),
        WorkPane::Files => files_tree(state, files, cx).into_any_element(),
    };
    let body_id = SharedString::from(format!(
        "work-pane-body-{}",
        selected.label().to_ascii_lowercase()
    ));

    v_flex()
        .w_full()
        .h_full()
        .min_h_0()
        .child(
            div()
                .flex_shrink_0()
                .id("work-pane-tabs-host")
                .test_support()
                .child(tabs),
        )
        .child(
            v_flex()
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .child(
                    div()
                        .w_full()
                        .p_3()
                        .id(body_id)
                        .test_support()
                        .child(body),
                ),
        )
        .child(work_footer(cx))
}

/// The pane's fixed footer action row.
fn work_footer(cx: &App) -> impl IntoElement {
    h_flex()
        .w_full()
        .flex_shrink_0()
        .items_center()
        .gap_1()
        .border_t_1()
        .border_color(cx.theme().border)
        .px_3()
        .py_2()
        .child(
            Button::new("work-pane-open-editor")
                .label("Open in editor")
                .icon(IconName::SquareTerminal)
                .ghost()
                .compact()
                .tooltip("Open the workspace in your editor")
                .accessibility_label("Open the workspace in your editor"),
        )
        .child(div().flex_1())
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from("Ctrl+\\ to close")),
        )
        .id("work-pane-footer")
        .test_support()
}

/// A pane heading: title, subtitle, and an optional trailing action.
///
/// Mirrors the prototype's `panelHead`, which every pane carries — without it
/// the pane reads as a loose list rather than a tool with a purpose.
fn panel_head(
    title: &str,
    subtitle: impl Into<SharedString>,
    action: Option<AnyElement>,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .items_start()
        .gap_3()
        .mb_3()
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child(SharedString::from(title.to_string())),
                )
                .child(
                    div()
                        .mt_1()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(subtitle.into()),
                ),
        )
        .when_some(action, |row, action| row.child(action))
}

/// A bordered surface card, the prototype's `card`.
fn card(cx: &App, children: Vec<AnyElement>) -> impl IntoElement {
    v_flex()
        .w_full()
        .rounded(cx.theme().radius_2xl())
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().popover)
        .overflow_hidden()
        .children(children)
}

/// A card that also carries a test/scroll anchor.
///
/// Views that need to be addressable by name use this rather than wrapping
/// [`card`], so the surface keeps one definition.
fn card_with_id(id: &'static str, cx: &App, children: Vec<AnyElement>) -> impl IntoElement {
    v_flex()
        .id(id)
        .test_support()
        .w_full()
        .rounded(cx.theme().radius_2xl())
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().popover)
        .overflow_hidden()
        .children(children)
}

/// The header strip inside a card: a label and a right-aligned note.
fn card_head(label: &str, note: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    h_flex()
        .w_full()
        .gap_2()
        .border_b_1()
        .border_color(cx.theme().border)
        .px_3()
        .py_2()
        .child(
            div()
                .text_xs()
                .font_semibold()
                .child(SharedString::from(label.to_string())),
        )
        .child(div().flex_1())
        .child(
            div()
                .font_family(cx.theme().mono_font_family.clone())
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(note.into()),
        )
}

/// Plan steps in arrival order.
fn plan(state: &SessionState, cx: &App) -> impl IntoElement {
    let total = state.plan.len();
    let done = state
        .plan
        .iter()
        .filter(|step| step.output.is_some())
        .count();
    let percent = (done * 100).checked_div(total).unwrap_or_default();
    // Exactly one row reads as the current step: the first that has not run.
    let current = state.plan.iter().position(|step| step.output.is_none());

    let head = panel_head(
        "Implementation plan",
        "Steps arrive as the agent plans, then report back as they run.",
        Some(
            Button::new("work-pane-plan-refresh")
                .icon(IconName::RefreshCw)
                .ghost()
                .compact()
                .tooltip("Refresh plan")
                .accessibility_label("Refresh plan")
                .into_any_element(),
        ),
        cx,
    );

    if total == 0 {
        return v_flex().w_full().child(head).child(empty("No plan yet.", cx));
    }

    let mut work = vec![
        card_head("Current work", format!("{percent}% complete"), cx).into_any_element(),
        v_flex()
            .w_full()
            .px_3()
            .pb_3()
            .child(
                div()
                    .h(rems(0.25))
                    .w_full()
                    .rounded_full()
                    .bg(cx.theme().border)
                    .child(
                        div()
                            .h_full()
                            .rounded_full()
                            .bg(cx.theme().primary)
                            .w(rems(plan_bar_width(percent))),
                    ),
            )
            .into_any_element(),
    ];
    for (index, step) in state.plan.iter().enumerate() {
        work.push(plan_step_row(index, current, step, cx).into_any_element());
    }

    v_flex().w_full().child(head).child(card(cx, work))
}


/// Width of the plan progress fill, in rems, for a 100%-wide track.
///
/// A zero-width child would collapse the rounded cap, so an executed-but-tiny
/// plan keeps a hairline of fill instead.
fn plan_bar_width(percent: usize) -> f32 {
    (percent as f32 / 100.).max(0.02)
}

/// One plan row: status glyph, description, detail, and a trailing state.
fn plan_step_row(
    index: usize,
    current: Option<usize>,
    step: &tact_protocol::PlanStep,
    cx: &App,
) -> impl IntoElement {
    let executed = step.output.is_some();
    let is_current = current == Some(index);
    let (icon, fg, bg, trailing) = if executed {
        (
            IconName::Check,
            cx.theme().success,
            cx.theme().success.opacity(0.12),
            "done".to_string(),
        )
    } else if is_current {
        (
            IconName::CircleDot,
            cx.theme().primary,
            cx.theme().primary.opacity(0.12),
            "now".to_string(),
        )
    } else {
        (
            IconName::Circle,
            cx.theme().muted_foreground,
            cx.theme().background,
            "next".to_string(),
        )
    };

    h_flex()
        .w_full()
        .items_center()
        .gap_2()
        .border_b_1()
        .border_color(cx.theme().border)
        .when(is_current, |row| row.bg(cx.theme().primary.opacity(0.06)))
        .px_3()
        .py_2()
        .child(
            div()
                .size(rems(1.125))
                .flex_shrink_0()
                .rounded_full()
                .bg(bg)
                .text_color(fg)
                .flex()
                .items_center()
                .justify_center()
                .child(icon),
        )
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .child(SharedString::from(step.description.clone())),
                )
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(step.tool.clone())),
                ),
        )
        .child(
            div()
                .flex_shrink_0()
                .font_family(cx.theme().mono_font_family.clone())
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(trailing)),
        )
}

/// File changes recorded from write and edit tool results.
fn diff(state: &SessionState, cx: &App) -> impl IntoElement {
    let total_added: u32 = state.diff.iter().filter_map(|entry| entry.added).sum();
    let total_removed: u32 = state.diff.iter().filter_map(|entry| entry.removed).sum();
    let files = state.diff.len();

    let subtitle = if total_added == 0 && total_removed == 0 {
        format!("{files} file{}", if files == 1 { "" } else { "s" })
    } else {
        format!(
            "{files} file{} · {total_added} additions · {total_removed} deletions",
            if files == 1 { "" } else { "s" }
        )
    };

    let head = panel_head(
        "Uncommitted changes",
        subtitle,
        Some(
            Button::new("work-pane-diff-comment")
                .label("Comment")
                .ghost()
                .compact()
                .into_any_element(),
        ),
        cx,
    );

    if state.diff.is_empty() {
        return v_flex()
            .w_full()
            .child(head)
            .child(empty("No file changes yet.", cx));
    }

    let mut body = v_flex().w_full().gap_3().child(head);
    for entry in &state.diff {
        body = body.child(diff_card(entry, cx));
    }

    body
}

/// One changed file: a monospace header with stats, then the diff body.
fn diff_card(entry: &DiffEntry, cx: &App) -> impl IntoElement {
    let stats = match (entry.added, entry.removed) {
        (Some(added), Some(removed)) => h_flex()
            .flex_shrink_0()
            .gap_1()
            .font_family(cx.theme().mono_font_family.clone())
            .text_xs()
            .child(
                div()
                    .text_color(cx.theme().success)
                    .child(SharedString::from(format!("+{added}"))),
            )
            .child(
                div()
                    .text_color(cx.theme().danger)
                    .child(SharedString::from(format!("\u{2212}{removed}"))),
            )
            .into_any_element(),
        _ => div().into_any_element(),
    };

    let header = h_flex()
        .w_full()
        .items_center()
        .gap_2()
        .border_b_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().background)
        .px_3()
        .py_2()
        .child(IconName::FileCode)
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .font_family(cx.theme().mono_font_family.clone())
                .text_xs()
                .child(SharedString::from(entry.path.clone())),
        )
        .child(stats);

    card(cx, vec![
        header.into_any_element(),
        div()
            .w_full()
            .font_family(cx.theme().mono_font_family.clone())
            .text_xs()
            .child(SharedString::from(entry.detail.clone()))
            .into_any_element(),
    ])
}

/// Persistent tasks, rendered as the prototype's Task / Status / Owner table.
fn tasks(state: &SessionState, cx: &App) -> impl IntoElement {
    let count = state.tasks.len();
    let blocked: Vec<_> = state
        .tasks
        .iter()
        .filter(|task| task.status == TaskStatusSnapshot::Pending && !task.blocked_by.is_empty())
        .collect();

    let head = panel_head(
        "Tasks",
        format!("{count} task{}", if count == 1 { "" } else { "s" }),
        Some(
            Button::new("work-pane-tasks-new")
                .label("New task")
                .ghost()
                .compact()
                .into_any_element(),
        ),
        cx,
    );

    if state.tasks.is_empty() {
        return v_flex()
            .w_full()
            .child(head)
            .child(empty("No tasks in this session.", cx));
    }

    let mut rows = vec![task_header_row(cx).into_any_element()];
    for task in &state.tasks {
        rows.push(task_row(task, cx).into_any_element());
    }

    let mut body = v_flex()
        .w_full()
        .gap_3()
        .child(head)
        .child(card_with_id("work-pane-task-table", cx, rows));

    if !blocked.is_empty() {
        let mut items = vec![card_head(
            "Blocked by",
            format!("{} item{}", blocked.len(), if blocked.len() == 1 { "" } else { "s" }),
            cx,
        )
        .into_any_element()];
        for task in blocked {
            items.push(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .child(
                        div()
                            .size(rems(1.125))
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(cx.theme().danger.opacity(0.12))
                            .text_color(cx.theme().danger)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(SharedString::from("!")),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_sm()
                            .child(SharedString::from(task.subject.clone())),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from("now")),
                    )
                    .into_any_element(),
            );
        }
        body = body.child(card_with_id("work-pane-blocked-by", cx, items));
    }

    body
}

/// The table's column headings.
fn task_header_row(cx: &App) -> impl IntoElement {
    h_flex()
        .w_full()
        .items_center()
        .gap_2()
        .border_b_1()
        .border_color(cx.theme().border)
        .px_3()
        .py_2()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(div().flex_1().child(SharedString::from("Task")))
        .child(div().w(rems(5.)).child(SharedString::from("Status")))
        .child(div().w(rems(3.)).child(SharedString::from("Owner")))
}

/// One task row with a status badge.
fn task_row(task: &tact_protocol::TaskSnapshot, cx: &App) -> impl IntoElement {
    use tact_protocol::TaskStatusSnapshot as Status;

    let blocked = task.status == Status::Pending && !task.blocked_by.is_empty();
    let (label, fg, bg) = if blocked {
        ("Blocked", cx.theme().danger, cx.theme().danger.opacity(0.12))
    } else {
        match task.status {
            Status::Completed => ("Done", cx.theme().success, cx.theme().success.opacity(0.12)),
            Status::InProgress => (
                "In progress",
                cx.theme().primary,
                cx.theme().primary.opacity(0.12),
            ),
            Status::Pending => (
                "Pending",
                cx.theme().muted_foreground,
                cx.theme().background,
            ),
        }
    };

    h_flex()
        .w_full()
        .items_center()
        .gap_2()
        .border_b_1()
        .border_color(cx.theme().border)
        .px_3()
        .py_2()
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_sm()
                .child(SharedString::from(task.subject.clone())),
        )
        .child(
            div().w(rems(5.)).flex_shrink_0().child(
                h_flex()
                    .items_center()
                    .rounded(cx.theme().radius)
                    .bg(bg)
                    .px_1()
                    .py_0p5()
                    .text_xs()
                    .text_color(fg)
                    .child(SharedString::from(label)),
            ),
        )
        .child(
            div()
                .w(rems(3.))
                .flex_shrink_0()
                .truncate()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(task.owner.clone())),
        )
}

/// Subagent runs with status and summary.
fn subagents(state: &SessionState, cx: &App) -> impl IntoElement {
    let count = state.subagents.len();
    let head = panel_head(
        "Subagent runs",
        format!("{count} run{}", if count == 1 { "" } else { "s" }),
        None,
        cx,
    );

    if state.subagents.is_empty() {
        return v_flex()
            .w_full()
            .child(head)
            .child(empty("No subagent runs yet.", cx));
    }

    let mut rows = vec![
        card_head("Runs", format!("{count} total"), cx).into_any_element(),
    ];
    for run in &state.subagents {
        let (label, fg, bg) = match run.status {
            SubagentStatusSnapshot::Running => (
                "Running",
                cx.theme().primary,
                cx.theme().primary.opacity(0.12),
            ),
            SubagentStatusSnapshot::Completed => (
                "Completed",
                cx.theme().success,
                cx.theme().success.opacity(0.12),
            ),
            SubagentStatusSnapshot::Failed => (
                "Failed",
                cx.theme().danger,
                cx.theme().danger.opacity(0.12),
            ),
            SubagentStatusSnapshot::Cancelled => (
                "Cancelled",
                cx.theme().muted_foreground,
                cx.theme().background,
            ),
        };
        rows.push(
            h_flex()
                .w_full()
                .items_center()
                .gap_2()
                .border_b_1()
                .border_color(cx.theme().border)
                .px_3()
                .py_2()
                .child(
                    v_flex()
                        .min_w_0()
                        .flex_1()
                        .child(
                            div()
                                .truncate()
                                .text_sm()
                                .child(SharedString::from(run.child_id.clone())),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(SharedString::from(run.summary_first.clone())),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .rounded(cx.theme().radius)
                        .bg(bg)
                        .px_1()
                        .py_0p5()
                        .text_xs()
                        .text_color(fg)
                        .child(SharedString::from(label)),
                )
                .into_any_element(),
        );
    }

    v_flex().w_full().child(head).child(card(cx, rows))
}

/// Workspace tree with expandable directories.
fn files_tree(
    state: &SessionState,
    files: &FilesPane,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    let changed = state.diff.len();
    let Some(root) = state.workdir.clone() else {
        return v_flex()
            .w_full()
            .child(empty("No workspace directory.", cx));
    };

    let rows = files.rows(&root);
    let head = panel_head(
        "Files",
        format!(
            "{changed} changed · {} open",
            rows.iter().filter(|row| row.is_dir && row.expanded).count()
        ),
        Some(
            Button::new("work-pane-files-add")
                .label("Add file")
                .ghost()
                .compact()
                .into_any_element(),
        ),
        cx,
    );

    if rows.is_empty() {
        return v_flex()
            .w_full()
            .child(head)
            .child(empty("The workspace is empty.", cx));
    }

    let hover_bg = cx.theme().primary.opacity(0.08);
    let mut tree = v_flex().w_full().p_1();
    for row in rows {
        let indent = rems_for_depth(row.depth);
        let path = row.path.clone();
        let icon = match (row.is_dir, row.expanded) {
            (true, true) => IconName::FolderOpen,
            (true, false) => IconName::Folder,
            (false, _) => IconName::File,
        };
        let id = SharedString::from(format!("file-row-{}", path.display()));
        let is_expanded = row.is_dir && row.expanded;
        let row_id = id.clone();

        let marker = if row.is_dir {
            Button::new(id)
                .icon(icon)
                .tooltip(if row.expanded { "Collapse" } else { "Expand" })
                .accessibility_label(if row.expanded {
                    "Collapse folder"
                } else {
                    "Expand folder"
                })
                .ghost()
                .compact()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_directory(path.clone());
                    cx.notify();
                }))
                .into_any_element()
        } else {
            div().px(rems(0.5)).child(icon).into_any_element()
        };

        tree = tree.child(
            h_flex()
                .id(row_id)
                .w_full()
                .min_w_0()
                .items_center()
                .gap_1()
                .rounded(cx.theme().radius)
                .pl(indent)
                .pr_2()
                .py_0p5()
                .when(is_expanded, move |row| row.bg(hover_bg))
                .child(marker)
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .child(SharedString::from(row.name)),
                ),
        );
    }

    v_flex().w_full().child(head).child(card(cx, vec![tree.into_any_element()]))
}

/// Indent step for one tree depth.
fn rems_for_depth(depth: usize) -> gpui_kit::Rems {
    gpui_kit::rems(depth as f32 * 0.75)
}

/// Muted placeholder used by every empty pane.
fn empty(text: &str, cx: &App) -> impl IntoElement {
    div()
        .px_3()
        .py_2()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(SharedString::from(text.to_string()))
}

/// One recorded file change, rendered as a card in the Diff pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiffEntry {
    /// Path the change applies to, as reported by the tool.
    pub(crate) path: String,
    /// Tool-reported change detail, or the unified-diff body.
    pub(crate) detail: String,
    /// Lines the change adds, when the tool reported a count.
    pub(crate) added: Option<u32>,
    /// Lines the change removes, when the tool reported a count.
    pub(crate) removed: Option<u32>,
}

impl DiffEntry {
    /// A change with no recorded line counts.
    pub(crate) fn new(path: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            detail: detail.into(),
            added: None,
            removed: None,
        }
    }

    /// A change whose counts are known, as when a `write`/`edit` tool reports them.
    pub(crate) fn with_stats(mut self, added: u32, removed: u32) -> Self {
        self.added = Some(added);
        self.removed = Some(removed);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_progress_bar_never_collapses_to_zero_width() {
        assert_eq!(plan_bar_width(100), 1.0);
        assert_eq!(plan_bar_width(50), 0.5);
        assert!(
            plan_bar_width(0) > 0.0,
            "an unstarted plan keeps a hairline rather than an invisible cap"
        );
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("tact-gui-pane-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/nested")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn lib() {}\n").unwrap();
        std::fs::write(root.join("README.md"), "# scratch\n").unwrap();
        std::fs::write(root.join(".hidden"), "not shown\n").unwrap();
        root
    }

    #[test]
    fn files_rows_sort_directories_first_and_hide_dotfiles() {
        let root = scratch_dir("sort");
        let files = FilesPane::default();

        let rows = files.rows(&root);
        let names: Vec<_> = rows.iter().map(|row| row.name.as_str()).collect();

        assert_eq!(names, ["src", "README.md"]);
        assert!(rows[0].is_dir);
        assert!(!rows[0].expanded);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn files_rows_recurse_only_into_expanded_directories() {
        let root = scratch_dir("expand");
        let mut files = FilesPane::default();
        files.on_toggle(root.join("src"));

        let rows = files.rows(&root);
        let names: Vec<_> = rows.iter().map(|row| row.name.as_str()).collect();

        assert_eq!(names, ["src", "nested", "lib.rs", "README.md"]);
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[2].depth, 1);

        files.on_toggle(root.join("src"));
        let collapsed = files.rows(&root);
        assert_eq!(
            collapsed.iter().map(|row| row.depth).collect::<Vec<_>>(),
            [0, 0]
        );

        let _ = std::fs::remove_dir_all(root);
    }
}

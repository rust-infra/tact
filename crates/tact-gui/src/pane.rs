//! Work pane: Plan, Diff, Tasks, Subagents, and Files.
//!
//! The pane is a secondary surface, never a second application: every tab reads
//! state the session already produced (plan steps, recorded file changes, task
//! and subagent snapshots, the workspace tree). Nothing here talks to the agent
//! except through commands the shell already exposes.

use std::{
    collections::{HashMap, HashSet},
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

/// Lazily loaded unified diffs for the Diff pane.
///
/// The protocol reports *that* a file changed and how many lines moved, but not
/// the diff body itself: `write_file`/`edit_file` carry the new content and
/// `apply_patch` a summary. The TUI solves this the same way, reading
/// `git diff` on demand; the pane caches per path so a frame never shells out
/// twice for the same file, and working-tree edits made outside the agent show
/// up on the next invalidation.
#[derive(Default)]
pub struct DiffPane {
    cache: HashMap<String, Option<String>>,
}

impl DiffPane {
    /// Create the cache; entries fill on first render of each path.
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop every cached diff, so the next frame re-reads the working tree.
    ///
    /// Called when a new change lands or the session switches, because the
    /// file on disk has moved on from what was cached.
    pub fn invalidate(&mut self) {
        self.cache.clear();
    }

    /// The unified diff for `path`, read once per invalidation.
    ///
    /// `None` means git had nothing to show — an untracked file, a clean path,
    /// or no repository — and the pane falls back to the recorded detail.
    fn diff(&mut self, workdir: Option<&Path>, path: &str) -> Option<&str> {
        if !self.cache.contains_key(path) {
            let loaded = workdir.and_then(|workdir| git_diff(workdir, path));
            self.cache.insert(path.to_string(), loaded);
        }
        self.cache.get(path).and_then(|entry| entry.as_deref())
    }
}

/// `git diff` for one path, run in the session's workspace.
fn git_diff(workdir: &Path, path: &str) -> Option<String> {
    // `--no-color` keeps the output parseable; the pane colors it itself.
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(workdir)
        .args(["diff", "--no-color", "--"])
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    (!text.trim().is_empty()).then_some(text)
}

/// One parsed line of a unified diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiffLineKind {
    /// Context, shown uncolored.
    Context,
    /// An added line.
    Added,
    /// A removed line.
    Removed,
    /// A file header or hunk marker, shown muted.
    Meta,
}

/// Split a unified diff into renderable lines, dropping the preamble.
///
/// Only the `@@` bodies and the added/removed lines matter visually; the
/// `diff --git` / `index` / `---` / `+++` preamble is what the card header
/// already states.
fn diff_lines(text: &str) -> Vec<(DiffLineKind, String)> {
    let mut lines = Vec::new();
    let mut in_hunk = false;
    for line in text.lines() {
        if line.starts_with("@@") {
            in_hunk = true;
            lines.push((DiffLineKind::Meta, line.to_string()));
        } else if !in_hunk {
            continue;
        } else if let Some(rest) = line.strip_prefix('+') {
            lines.push((DiffLineKind::Added, rest.to_string()));
        } else if let Some(rest) = line.strip_prefix('-') {
            lines.push((DiffLineKind::Removed, rest.to_string()));
        } else if line.starts_with('\\') {
            // "\ No newline at end of file" annotates the previous line; it is
            // not a line of the file and must not advance the gutter.
            lines.push((DiffLineKind::Meta, line.to_string()));
        } else {
            let rest = line.strip_prefix(' ').unwrap_or(line);
            lines.push((DiffLineKind::Context, rest.to_string()));
        }
    }
    lines
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
    diffs: &mut DiffPane,
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
        WorkPane::Diff => diff(state, diffs, cx).into_any_element(),
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
fn diff(state: &SessionState, diffs: &mut DiffPane, cx: &App) -> impl IntoElement {
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
    // The cache is keyed per path, so reading the working tree happens once
    // per file per invalidation rather than on every frame.
    let workdir = state.workdir.clone();
    for entry in &state.diff {
        let unified = diffs
            .diff(workdir.as_deref(), &entry.path)
            .map(str::to_string);
        body = body.child(diff_card(entry, unified.as_deref(), cx));
    }

    body
}

/// One changed file: a monospace header with stats, then the diff body.
fn diff_card(entry: &DiffEntry, unified: Option<&str>, cx: &App) -> impl IntoElement {
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

    // A real unified diff when git has one, so the pane shows the change
    // itself; the recorded tool detail is the fallback for files git cannot
    // diff (untracked, already committed, or outside a repository).
    let body = match unified {
        Some(unified) if !diff_lines(unified).is_empty() => {
            diff_body(unified, cx).into_any_element()
        }
        _ => div()
            .w_full()
            .px_3()
            .py_2()
            .font_family(cx.theme().mono_font_family.clone())
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(SharedString::from(entry.detail.clone()))
            .into_any_element(),
    };

    card(cx, vec![header.into_any_element(), body])
}

/// The prototype's `.diff` block: per-line gutter, marker, and code.
fn diff_body(unified: &str, cx: &App) -> impl IntoElement {
    let mut rows = v_flex()
        .w_full()
        .font_family(cx.theme().mono_font_family.clone())
        .text_xs();
    let mut number = DiffNumbering::default();

    for (kind, text) in diff_lines(unified) {
        // Hunk markers reset both sides, so removed lines use the old-file
        // number while added and context lines use the new-file number.
        if kind == DiffLineKind::Meta {
            number.reset_from(&text);
        }
        let (fg, bg, marker) = match kind {
            DiffLineKind::Added => (cx.theme().success, cx.theme().success.opacity(0.10), "+"),
            DiffLineKind::Removed => (cx.theme().danger, cx.theme().danger.opacity(0.10), "-"),
            DiffLineKind::Meta => (cx.theme().muted_foreground, cx.theme().background, ""),
            DiffLineKind::Context => (cx.theme().foreground, cx.theme().background, " "),
        };
        let gutter = number
            .gutter(kind)
            .map(|number| number.to_string())
            .unwrap_or_default();

        rows = rows.child(
            h_flex()
                .w_full()
                .bg(bg)
                .items_start()
                .child(
                    div()
                        .w(rems(2.5))
                        .flex_shrink_0()
                        .pr_1()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(gutter)),
                )
                .child(
                    div()
                        .w(rems(1.))
                        .flex_shrink_0()
                        .text_color(fg)
                        .child(SharedString::from(marker)),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .text_color(fg)
                        .child(SharedString::from(text)),
                ),
        );
    }

    rows
}

/// The old- and new-file line numbers a `@@ -a,b +c,d @@` marker starts at.
fn hunk_starts(marker: &str) -> Option<(usize, usize)> {
    let mut parts = marker.split_whitespace();
    if parts.next()? != "@@" {
        return None;
    }
    let old = parts.next()?;
    let new = parts.next()?;
    Some((side_start(old)?, side_start(new)?))
}

/// Parse the start line from one side of a hunk header (`-40,2` / `+128,9`).
fn side_start(spec: &str) -> Option<usize> {
    let digits = spec.strip_prefix(['-', '+'])?;
    digits.split(',').next()?.parse().ok()
}

/// Old/new line numbers while walking one hunk.
#[derive(Default)]
struct DiffNumbering {
    old: usize,
    new: usize,
}

impl DiffNumbering {
    /// Reset both sides at a hunk marker.
    fn reset_from(&mut self, marker: &str) {
        if let Some((old, new)) = hunk_starts(marker) {
            self.old = old;
            self.new = new;
        }
    }

    /// The gutter number for one line; removed lines keep the old-file side.
    fn gutter(&mut self, kind: DiffLineKind) -> Option<usize> {
        match kind {
            DiffLineKind::Meta => None,
            DiffLineKind::Removed => {
                let number = self.old;
                self.old += 1;
                Some(number)
            }
            DiffLineKind::Added => {
                let number = self.new;
                self.new += 1;
                Some(number)
            }
            DiffLineKind::Context => {
                let number = self.new;
                self.old += 1;
                self.new += 1;
                Some(number)
            }
        }
    }
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

    #[test]
    fn diff_lines_drop_the_preamble_and_start_at_the_first_hunk() {
        let unified = "\n".to_string()
            + "diff --git a/src/lib.rs b/src/lib.rs\n"
            + "index 1111111..2222222 100644\n"
            + "--- a/src/lib.rs\n"
            + "+++ b/src/lib.rs\n"
            + "@@ -1,2 +1,2 @@\n"
            + " context\n"
            + "-old\n"
            + "+new\n"
            + "\\ No newline at end of file\n";

        let lines = diff_lines(&unified);

        assert_eq!(
            lines,
            vec![
                (DiffLineKind::Meta, "@@ -1,2 +1,2 @@".to_string()),
                (DiffLineKind::Context, "context".to_string()),
                (DiffLineKind::Removed, "old".to_string()),
                (DiffLineKind::Added, "new".to_string()),
                (
                    DiffLineKind::Meta,
                    "\\ No newline at end of file".to_string()
                ),
            ]
        );
    }

    /// Each hunk restates the line numbers it resumes at, so the gutter has to
    /// realign rather than counting straight through the file.
    #[test]
    fn each_hunk_marker_restates_the_gutter_start() {
        assert_eq!(hunk_starts("@@ -1,2 +1,2 @@"), Some((1, 1)));
        assert_eq!(
            hunk_starts("@@ -40,6 +128,9 @@ fn main() {"),
            Some((40, 128))
        );
        assert_eq!(hunk_starts("@@ -10 +5 @@"), Some((10, 5)));
        assert_eq!(hunk_starts("@@ not a hunk @@"), None);
    }

    /// The pane reads the working tree once per file, not once per frame.
    #[test]
    fn diff_bodies_are_read_once_per_invalidation() {
        let mut diffs = DiffPane::new();
        let no_workdir: Option<&Path> = None;

        assert_eq!(diffs.diff(no_workdir, "src/lib.rs"), None);
        // Cached as "no diff", which is what makes the second call free.
        assert!(diffs.cache.contains_key("src/lib.rs"));

        diffs.invalidate();
        assert!(diffs.cache.is_empty());
    }

    /// Removed lines belong to the old file, so they must not advance the
    /// new-file gutter.
    #[test]
    fn diff_numbering_uses_the_old_side_for_removed_lines() {
        let mut number = DiffNumbering::default();
        number.reset_from("@@ -40,6 +128,9 @@ fn main() {");

        assert_eq!(number.gutter(DiffLineKind::Context), Some(128));
        assert_eq!(number.gutter(DiffLineKind::Removed), Some(41));
        assert_eq!(number.gutter(DiffLineKind::Added), Some(129));
        assert_eq!(number.gutter(DiffLineKind::Context), Some(130));
    }

    /// A diff body is a real `git diff`, so a scratch repository has to produce
    /// the added line the pane will color.
    #[test]
    fn git_diff_reads_the_working_tree_change_for_a_tracked_path() {
        let root = scratch_dir("gitdiff");
        let path = root.join("src/lib.rs");
        if std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["init", "--quiet"])
            .status()
            .map(|status| !status.success())
            .unwrap_or(true)
        {
            let _ = std::fs::remove_dir_all(root);
            return;
        }
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["add", "."])
            .status();
        std::fs::write(&path, "pub fn lib() {}\npub fn extra() {}\n").unwrap();

        let unified = git_diff(&root, "src/lib.rs").expect("a modified file has a diff");

        assert!(unified.contains("+pub fn extra() {}"), "{unified}");
        assert!(
            diff_lines(&unified)
                .iter()
                .any(|(kind, text)| *kind == DiffLineKind::Added && text.contains("extra")),
            "the added line reaches the renderer"
        );

        let _ = std::fs::remove_dir_all(root);
    }
}

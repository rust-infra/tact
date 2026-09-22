//! Work pane: Plan, Diff, Tasks, Subagents, and Files.
//!
//! The pane is a secondary surface, never a second application: every tab reads
//! state the session already produced (plan steps, recorded file changes, task
//! and subagent snapshots, the workspace tree). Nothing here talks to the agent
//! except through commands the shell already exposes.

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::Read as _,
    path::{Path, PathBuf},
    time::Duration,
};

use gpui_ai::loading::LoadingState;
use gpui_kit::base::animation::cubic_bezier;
use gpui_kit::base::{StyledExt as _, TestSupportExt as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Icon,
    button::{Button, ButtonVariants as _},
    h_flex,
    popover::Popover,
    scroll::ScrollableElement as _,
    v_flex,
};
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, Context, FocusHandle, Focusable as _,
    FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
    radians, relative, rems,
};

use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use tact_protocol::{SubagentStatusSnapshot, TaskStatusSnapshot};

use crate::layout::WorkPaneSide;
use crate::session::{SessionState, age_label, now_unix};
use crate::shell::{TactApp, focus_visible_ring, prototype_button, prototype_icon_button};
use crate::terminal::{TermColor, TerminalPane};

/// `.panel.active{animation:panel 180ms var(--ease)}` -- the pane's body fades
/// into place when a tab brings it in.
const PANEL_ENTRANCE: Duration = Duration::from_millis(180);

/// The prototype's `--ease:cubic-bezier(.23,1,.32,1)`, shared with the shell's
/// two sliding overlays.
fn panel_entrance() -> Animation {
    Animation::new(PANEL_ENTRANCE).with_easing(cubic_bezier(0.23, 1.0, 0.32, 1.0))
}

/// Which surface the work pane shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
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
    /// Charts over the session's recorded activity.
    Stats,
    /// A shell running in a real PTY.
    Terminal,
    /// URLs handed to the system browser.
    Browser,
}

impl WorkPane {
    /// Every pane in tab order.
    pub const ALL: [Self; 8] = [
        Self::Plan,
        Self::Diff,
        Self::Tasks,
        Self::Subagents,
        Self::Files,
        Self::Stats,
        Self::Terminal,
        Self::Browser,
    ];

    /// Tab label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Plan => "Plan",
            Self::Diff => "Diff",
            Self::Tasks => "Tasks",
            // "Agents" rather than "Subagent": six chips have to fit inside
            // the work pane's 420 px column, and the longer word pushed the
            // last chip past the strip's clipped edge.
            Self::Subagents => "Agents",
            Self::Files => "Files",
            Self::Stats => "Stats",
            Self::Terminal => "Term",
            Self::Browser => "Browser",
        }
    }

    /// Stable id slug, independent of the display label.
    ///
    /// The body's element id is what tests and the accessibility surface name;
    /// tying it to [`Self::label`] meant renaming "Subagent" to "Agents" would
    /// silently rename every id that pane owns.
    pub(crate) fn slug(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Diff => "diff",
            Self::Tasks => "tasks",
            Self::Subagents => "subagent",
            Self::Files => "files",
            Self::Stats => "stats",
            Self::Terminal => "terminal",
            Self::Browser => "browser",
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
            Self::Stats => None,
            Self::Terminal => None,
            Self::Browser => None,
        }
    }
}

/// Files pane state: which directories the user opened, plus the flattened
/// listing those choices produced.
///
/// The expansion set is kept as a path set rather than a node tree so re-reading
/// the directory (the workspace changes underneath the app) cannot invalidate
/// node identities. The walk itself is cached across frames because it is
/// `read_dir` plus one `stat` per entry on the UI thread: rebuilding it every
/// render turns expanding one large directory (`target/`, `node_modules/`) into
/// a frame-long stall.
#[derive(Default)]
pub struct FilesPane {
    expanded: HashSet<PathBuf>,
    /// The row whose preview is open, if any.
    selected: Option<PathBuf>,
    /// The selected file's content, refreshed whenever the walk invalidates.
    preview: Option<FilePreview>,
    /// Advances whenever the cached walk stops describing `expanded`.
    revision: u64,
    cache: Option<FilesCache>,
}

/// One memoized [`collect`] walk.
struct FilesCache {
    root: PathBuf,
    revision: u64,
    rows: Vec<FileRow>,
}

/// Maximum bytes read for one Files preview.
const FILE_PREVIEW_MAX_BYTES: usize = 64 * 1024;
/// Maximum lines rendered for one Files preview.
const FILE_PREVIEW_MAX_LINES: usize = 160;

/// The selected file's preview content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FilePreview {
    pub(crate) path: PathBuf,
    pub(crate) lines: Vec<String>,
    pub(crate) truncated: bool,
    pub(crate) binary: bool,
    pub(crate) error: Option<String>,
    /// Number of entries, when the selection is a directory rather than a file.
    ///
    /// `File::open` succeeds on a directory on Linux and the read then fails,
    /// so a selected folder used to render as a read error. A directory is not
    /// an unreadable file; it is a different kind of thing, and the preview
    /// says which.
    pub(crate) directory: Option<usize>,
}

impl FilePreview {
    /// Read at most [`FILE_PREVIEW_MAX_BYTES`] so a large generated file does
    /// not turn one tree click into an unbounded allocation.
    fn load(path: &Path) -> Self {
        if path.is_dir() {
            let entries = std::fs::read_dir(path)
                .map(|entries| entries.flatten().count())
                .unwrap_or(0);
            return Self {
                path: path.to_path_buf(),
                lines: Vec::new(),
                truncated: false,
                binary: false,
                error: None,
                directory: Some(entries),
            };
        }
        let mut bytes = Vec::new();
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(error) => {
                return Self {
                    path: path.to_path_buf(),
                    lines: Vec::new(),
                    truncated: false,
                    binary: false,
                    error: Some(format!("Could not read {}: {error}", path.display())),
                    directory: None,
                };
            }
        };
        let mut truncated = false;
        if let Err(error) = file
            .by_ref()
            .take((FILE_PREVIEW_MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
        {
            return Self {
                path: path.to_path_buf(),
                lines: Vec::new(),
                truncated: false,
                binary: false,
                error: Some(format!("Could not read {}: {error}", path.display())),
                directory: None,
            };
        }
        if bytes.len() > FILE_PREVIEW_MAX_BYTES {
            bytes.truncate(FILE_PREVIEW_MAX_BYTES);
            truncated = true;
        }

        let binary = bytes.contains(&0);
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        if lines.len() > FILE_PREVIEW_MAX_LINES {
            lines.truncate(FILE_PREVIEW_MAX_LINES);
            truncated = true;
        }

        Self {
            path: path.to_path_buf(),
            lines,
            truncated,
            binary,
            error: None,
            directory: None,
        }
    }
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
        self.invalidate();
    }

    /// Drop the cached walk so the next [`FilesPane::rows`] re-reads disk.
    ///
    /// Expansion clicks invalidate implicitly; this covers the other reason a
    /// listing goes stale — the agent writing files while the pane is closed.
    pub(crate) fn invalidate(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        if let Some(path) = self.selected.clone() {
            self.preview = Some(FilePreview::load(&path));
        }
    }

    /// Clear per-workspace navigation and preview state.
    pub(crate) fn reset_workspace(&mut self) {
        self.expanded.clear();
        self.selected = None;
        self.preview = None;
        self.invalidate();
    }

    /// Flattened rows for the workspace rooted at `root`.
    ///
    /// Depth is capped so a deep or cyclic-looking tree cannot stall the frame;
    /// the pane is a navigation aid, not a file manager. The result is cached
    /// until `root` or the revision changes, and callers therefore get a slice
    /// borrowed from the pane rather than a freshly allocated `Vec` per frame.
    pub(crate) fn rows(&mut self, root: &Path) -> &[FileRow] {
        let cached = matches!(
            &self.cache,
            Some(cache) if cache.root == root && cache.revision == self.revision
        );
        if !cached {
            let mut rows = Vec::new();
            collect(root, 0, self, &mut rows);
            self.cache = Some(FilesCache {
                root: root.to_path_buf(),
                revision: self.revision,
                rows,
            });
        }
        &self.cache.as_ref().expect("walk just cached").rows
    }

    /// Select a row and load its preview.
    ///
    /// Directories are selectable too: the footer's Reveal and Mention act on
    /// the selection, and both mean something for a folder.
    pub(crate) fn select(&mut self, path: &Path) {
        let path = path.to_path_buf();
        self.preview = Some(FilePreview::load(&path));
        self.selected = Some(path);
    }

    /// Handler for a press on a directory row or its chevron.
    ///
    /// Selecting first matters: `toggle` invalidates the cached walk and, with
    /// it, reloads the preview — so the selection has to be in place before the
    /// invalidation runs.
    pub(crate) fn on_toggle(&mut self, path: PathBuf) {
        self.select(&path);
        self.toggle(&path);
    }

    /// Open a file row in the pane's preview.
    pub(crate) fn select_file(&mut self, path: &Path) {
        self.select(path);
    }

    /// The file row whose preview is open.
    pub(crate) fn selected_path(&self) -> Option<&Path> {
        self.selected.as_deref()
    }

    /// The preview shown beneath the tree.
    pub(crate) fn selected_preview(&self) -> Option<&FilePreview> {
        self.preview.as_ref()
    }
}

/// Local display preferences for the Tasks pane.
#[derive(Default)]
pub struct TasksPane {
    filter: TaskFilter,
    sort: TaskSort,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum TaskFilter {
    #[default]
    All,
    Open,
    Done,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum TaskSort {
    #[default]
    Status,
    Owner,
    Newest,
}

impl TasksPane {
    pub(crate) fn cycle_filter(&mut self) {
        self.filter = match self.filter {
            TaskFilter::All => TaskFilter::Open,
            TaskFilter::Open => TaskFilter::Done,
            TaskFilter::Done => TaskFilter::All,
        };
    }

    pub(crate) fn cycle_sort(&mut self) {
        self.sort = match self.sort {
            TaskSort::Status => TaskSort::Owner,
            TaskSort::Owner => TaskSort::Newest,
            TaskSort::Newest => TaskSort::Status,
        };
    }

    fn filter_label(&self) -> &'static str {
        match self.filter {
            TaskFilter::All => "All",
            TaskFilter::Open => "Open",
            TaskFilter::Done => "Done",
        }
    }

    fn sort_label(&self) -> &'static str {
        match self.sort {
            TaskSort::Status => "By status",
            TaskSort::Owner => "By owner",
            TaskSort::Newest => "Newest",
        }
    }

    fn visible<'a>(
        &self,
        tasks: &'a [tact_protocol::TaskSnapshot],
    ) -> Vec<&'a tact_protocol::TaskSnapshot> {
        let mut visible: Vec<_> = tasks
            .iter()
            .filter(|task| match self.filter {
                TaskFilter::All => true,
                TaskFilter::Open => task.status != TaskStatusSnapshot::Completed,
                TaskFilter::Done => task.status == TaskStatusSnapshot::Completed,
            })
            .collect();
        visible.sort_by(|left, right| match self.sort {
            TaskSort::Status => task_status_rank(left.status)
                .cmp(&task_status_rank(right.status))
                .then_with(|| left.id.cmp(&right.id)),
            TaskSort::Owner => left
                .owner
                .to_lowercase()
                .cmp(&right.owner.to_lowercase())
                .then_with(|| left.id.cmp(&right.id)),
            TaskSort::Newest => task_latest_stamp(right)
                .cmp(&task_latest_stamp(left))
                .then_with(|| right.id.cmp(&left.id)),
        });
        visible
    }
}

fn task_status_rank(status: TaskStatusSnapshot) -> u8 {
    match status {
        TaskStatusSnapshot::Pending => 0,
        TaskStatusSnapshot::InProgress => 1,
        TaskStatusSnapshot::Completed => 2,
    }
}

fn next_task_status(status: TaskStatusSnapshot) -> TaskStatusSnapshot {
    match status {
        TaskStatusSnapshot::Pending => TaskStatusSnapshot::InProgress,
        TaskStatusSnapshot::InProgress => TaskStatusSnapshot::Completed,
        TaskStatusSnapshot::Completed => TaskStatusSnapshot::Pending,
    }
}

fn task_status_action(status: TaskStatusSnapshot) -> &'static str {
    match status {
        TaskStatusSnapshot::Pending => "Start",
        TaskStatusSnapshot::InProgress => "Complete",
        TaskStatusSnapshot::Completed => "Reopen",
    }
}

fn task_latest_stamp(task: &tact_protocol::TaskSnapshot) -> Option<i64> {
    [task.completed_at, task.started_at, task.created_at]
        .into_iter()
        .flatten()
        .max()
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
///
/// The recorded path arrives in whichever form the tool call used: usually
/// relative to the session workspace, but sometimes written from the
/// repository root while the workspace sits in a subdirectory. A relative
/// path that exists under the workspace is resolved to an absolute one;
/// anything else is anchored at the repository top, so both forms reach the
/// same file. An absolute path is passed through untouched, because a path
/// git is asked about need not exist on disk.
fn git_diff(workdir: &Path, path: &str) -> Option<String> {
    let recorded = Path::new(path);
    let pathspec = if recorded.is_absolute() {
        recorded.to_path_buf()
    } else {
        let resolved = workdir.join(recorded);
        if resolved.exists() {
            resolved
        } else {
            PathBuf::from(format!(":(top){path}"))
        }
    };
    // `--no-color` keeps the output parseable; the pane colors it itself. Once
    // the repository has a HEAD, compare against HEAD so a staged change stays
    // visible after the user presses Stage. Before the first commit there is no
    // HEAD to diff against, so fall back to the working-tree form the pane used
    // originally.
    let has_head = std::process::Command::new("git")
        .arg("-C")
        .arg(workdir)
        .args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    let mut command = std::process::Command::new("git");
    command.arg("-C").arg(workdir).args(["diff", "--no-color"]);
    if has_head {
        command.arg("HEAD");
    }
    let output = command.arg("--").arg(pathspec).output().ok()?;
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

/// Directory names the files pane never descends into.
///
/// These are build output and vendored dependencies: they are gitignored, they
/// are thousands of entries deep, and the pane exists to answer "which file is
/// the agent touching", not to browse an artifact cache.
pub(crate) const FILE_TREE_SKIPPED_DIRS: [&str; 2] = ["target", "node_modules"];

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
        if is_dir && FILE_TREE_SKIPPED_DIRS.contains(&name.as_str()) {
            continue;
        }
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
pub(crate) struct PaneState<'a> {
    pub(crate) files: &'a mut FilesPane,
    pub(crate) diffs: &'a mut DiffPane,
    pub(crate) tasks: &'a mut TasksPane,
    /// The embedded shell, once the user has started one.
    pub(crate) terminal: &'a mut Option<TerminalPane>,
    /// Focus target for the terminal grid; typing goes to the PTY only while
    /// this holds focus.
    pub(crate) terminal_focus: &'a FocusHandle,
    /// Grid the terminal should measure itself to, in cells.
    pub(crate) terminal_size: (u16, u16),
    /// Edge the pane is currently docked to, so the header can say so.
    pub(crate) side: WorkPaneSide,
    /// Browser pane address field.
    pub(crate) browser_url: &'a gpui_kit::Entity<InputState>,
    /// URLs the Browser pane has opened, newest first.
    pub(crate) browser_history: &'a [String],
}

pub(crate) fn view(
    selected: WorkPane,
    state: &SessionState,
    panes: PaneState<'_>,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    let PaneState {
        files,
        diffs,
        tasks: tasks_pane,
        terminal,
        terminal_focus,
        terminal_size,
        side,
        browser_url,
        browser_history,
    } = panes;
    let body = match selected {
        WorkPane::Plan => plan(state, cx).into_any_element(),
        WorkPane::Diff => diff(state, diffs, cx).into_any_element(),
        WorkPane::Tasks => tasks(state, tasks_pane, cx).into_any_element(),
        WorkPane::Subagents => subagents(state, cx).into_any_element(),
        WorkPane::Files => files_tree(state, files, cx).into_any_element(),
        WorkPane::Stats => stats(state, cx).into_any_element(),
        WorkPane::Terminal => {
            terminal_pane(terminal, terminal_focus, terminal_size, cx).into_any_element()
        }
        WorkPane::Browser => browser_pane(browser_url, browser_history, cx).into_any_element(),
    };
    let body_id = SharedString::from(format!("work-pane-body-{}", selected.slug()));
    let footer = work_footer(side, cx).into_any_element();
    let tabs = work_tabs(selected, state, cx);

    v_flex()
        .w_full()
        .h_full()
        .min_h_0()
        .child(
            // The prototype's `.workTop` is a 38px row with 8px side padding
            // and a trailing close button.
            h_flex()
                .flex_shrink_0()
                .h(rems(2.375))
                .items_center()
                .gap(rems(0.125))
                .px_2()
                .id("work-pane-tabs-host")
                .test_support()
                .child(tabs)
                .child(
                    prototype_icon_button("work-pane-close", IconName::X, cx)
                        .flex_shrink_0()
                        .tooltip("Close work pane (Ctrl+\\)")
                        .accessibility_label("Close work pane")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_work_pane();
                            cx.notify();
                        })),
                ),
        )
        .child(
            v_flex()
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .id("work-pane-body-scroll")
                .child(
                    div()
                        .w_full()
                        .p_3()
                        // Room for the overlay scrollbar, which gpui-component
                        // paints *over* the scroller's right edge rather than
                        // reserving a gutter for it: without this inset the last
                        // characters of a full-width line sit under the thumb.
                        .pr(rems(1.125))
                        .id(body_id.clone())
                        .test_support()
                        .child(body)
                        // `.panel` carries `animation:panel 180ms`, and
                        // `@keyframes panel` is `opacity:0 -> 1` plus a 3px
                        // lift. GPUI has no paint-level transform on a `Div`,
                        // but a relative `top` is a non-layout inset, so it
                        // gives the lift without jogging the scroll container.
                        // The animation key is the body's own per-pane id, so
                        // switching tabs replays it.
                        .with_animation(
                            SharedString::from(format!("{body_id}-enter")),
                            panel_entrance(),
                            |this, progress| this.opacity(progress).top(px(3.0 * (1.0 - progress))),
                        ),
                ),
        )
        .child(footer)
}

/// The prototype's `.wtabs`: 28px chips with an optional count badge.
///
/// `.wtabs{min-width:0;flex:1;display:flex;gap:2px;overflow:hidden}` is a flex
/// sibling of the close button, so the strip shrinks and clips inside its own
/// column instead of pushing the close button off the row (or letting the last
/// chip hide underneath it).
fn work_tabs(selected: WorkPane, state: &SessionState, cx: &mut Context<TactApp>) -> AnyElement {
    let ink = cx.theme().foreground;
    let ink3 = crate::theme::ink3(cx);
    let surface = cx.theme().popover;
    let line = cx.theme().border;
    // `.wtab:hover` uses the prototype's `--hover`, which the theme exposes as
    // `accent`; `muted` is the inset well color the cards use.
    let hover = cx.theme().accent;
    // `.count` is the inset well (`--surface2`); only the hovered tab uses `--hover`.
    let well = cx.theme().muted;
    let tint = crate::theme::accent_tint(cx);
    let accent = cx.theme().accent_foreground;

    let mut chips: Vec<AnyElement> = Vec::with_capacity(WorkPane::ALL.len());
    for (index, pane) in WorkPane::ALL.iter().copied().enumerate() {
        let active = pane == selected;
        let count = pane.count(state);
        chips.push(
            h_flex()
                .id(index)
                // Shrinkable, not fixed: the strip clips rather than scrolls,
                // so a chip that refuses to narrow is a pane the user cannot
                // reach once the column is at its narrowest.
                .min_w(rems(1.75))
                .flex_shrink(1.0)
                .overflow_hidden()
                .h(rems(1.75))
                .items_center()
                .gap(rems(0.1875))
                // Eight tabs have to fit the 420 px column, and the strip
                // clips rather than scrolls, so the prototype's chip is
                // tightened where the prototype never went: 4 px of side
                // padding and 10 px type instead of 8 px and 11 px.
                .px(rems(0.25))
                .rounded(rems(0.375))
                .text_size(rems(0.625))
                .whitespace_nowrap()
                .cursor_pointer()
                // `.wtab` is a `<button>` in the prototype, so each pane chip is
                // a tab stop. Without this the whole tab row was mouse-only --
                // including the only route to the Files pane, which made that
                // pane unreachable from the keyboard entirely.
                .tab_index(0)
                .focus_visible({
                    let ring = focus_visible_ring(cx);
                    move |style| style.shadow(ring.clone())
                })
                .when(active, |this| {
                    this.bg(surface)
                        .text_color(ink)
                        .border_1()
                        .border_color(line)
                })
                .when(!active, |this| {
                    this.text_color(ink3)
                        .hover(move |style| style.bg(hover).text_color(ink))
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select_work_pane(pane, window);
                    cx.notify();
                }))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .child(SharedString::from(pane.label())),
                )
                .when_some(if active { count } else { None }, |this, count| {
                    this.child(
                        // `.count{min-width:16px;height:16px;padding:0 4px;
                        // border-radius:5px;background:var(--surface2)}`, and
                        // the active tab recolours it with the accent tint.
                        // Only the active chip keeps its count: eight labels
                        // plus every badge do not fit the pane's narrow tab
                        // strip, and truncated tab names are worse than a
                        // count the body can explain once the pane is open.
                        h_flex()
                            .min_w(rems(1.))
                            .h(rems(1.))
                            .items_center()
                            .justify_center()
                            .px_1()
                            .rounded(rems(0.3125))
                            .text_size(rems(0.5625))
                            .font_family(cx.theme().mono_font_family.clone())
                            .bg(if active { tint } else { well })
                            .text_color(if active { accent } else { ink3 })
                            .child(SharedString::from(count.to_string())),
                    )
                })
                .test_support()
                .into_any_element(),
        );
    }

    h_flex()
        .id("work-pane-tabs")
        .test_support()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .children(chips)
        .into_any_element()
}

/// The pane's still-unbacked prototype-only actions.
///
/// `Open in editor` is live for a selected Files row; the remaining controls
/// keep the prototype's place and weight, but a press has to land somewhere:
/// each answers with the reason it cannot act yet, the shape the session menu
/// already uses for rename/duplicate/archive/reveal.
const REFRESH_PLAN_UNAVAILABLE: &str = "Refreshing the plan is not available yet: the pane already follows every step the agent reports.";
const NEW_TASK_UNAVAILABLE: &str =
    "Creating a task is not available yet: tasks arrive from the agent's own task tool.";
const ADD_FILE_UNAVAILABLE: &str =
    "Adding a file is not available yet: the pane lists the files the session itself changed.";

/// The pane's fixed footer action row.
fn work_footer(side: WorkPaneSide, cx: &mut Context<TactApp>) -> impl IntoElement {
    h_flex()
        .w_full()
        .flex_shrink_0()
        .items_center()
        .gap(rems(0.375))
        .border_t_1()
        .border_color(cx.theme().border)
        .px(rems(0.625))
        .py(rems(0.5))
        // The dock control lives in the footer rather than beside the tabs:
        // seven tab chips already fill the pane's width, and a control in that
        // row clipped the last one.
        .child(
            prototype_icon_button("work-pane-dock", IconName::PanelBottom, cx)
                .tooltip(SharedString::from(format!(
                    "Move work pane (docked {})",
                    side.label()
                )))
                .accessibility_label(SharedString::from(format!(
                    "Move work pane, currently docked {}",
                    side.label()
                )))
                .on_click(cx.listener(|this, _, _, cx| this.cycle_work_pane_side(cx))),
        )
        .child(
            prototype_button("work-pane-open-editor", false, cx)
                .label("Open in editor")
                .icon(IconName::Book)
                .tooltip("Open the selected file with the default application")
                .accessibility_label("Open the selected file")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.open_selected_file(cx);
                })),
        )
        .child(div().flex_1())
        .child(
            div()
                .text_xs()
                .text_color(crate::theme::ink3(cx))
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
        .mb(rems(0.625))
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .child(
                    div()
                        .text_size(rems(0.8125))
                        .font_semibold()
                        .child(SharedString::from(title.to_string())),
                )
                .child(
                    div()
                        .mt(rems(0.1875))
                        .text_size(rems(0.6875))
                        .text_color(crate::theme::ink3(cx))
                        .child(subtitle.into()),
                ),
        )
        .when_some(action, |row, action| row.child(action))
}

/// A bordered surface card, the prototype's `card`.
///
/// `.card{border-radius:var(--r10)}` is the prototype's 10 px step, which the
/// theme's radius scale cannot express: its base is 6 px, so `radius_2xl()`
/// measures 15 px and every card reads rounder than the design. The shell
/// draws the step explicitly, as it does for the other `rem` boxes.
fn card(cx: &App, children: Vec<AnyElement>) -> impl IntoElement {
    v_flex()
        .w_full()
        .rounded(rems(0.625))
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
        .rounded(rems(0.625))
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
        .px(rems(0.6875))
        .py(rems(0.625))
        .child(
            div()
                .text_size(rems(0.71875))
                .font_semibold()
                .child(SharedString::from(label.to_string())),
        )
        .child(div().flex_1())
        .child(
            div()
                .font_family(cx.theme().mono_font_family.clone())
                .text_size(rems(0.625))
                .text_color(crate::theme::ink3(cx))
                .child(note.into()),
        )
}

/// Plan steps in arrival order.
fn plan(state: &SessionState, cx: &mut Context<TactApp>) -> impl IntoElement {
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
            prototype_icon_button("work-pane-plan-refresh", IconName::RefreshCw, cx)
                .tooltip("Refresh plan")
                .accessibility_label("Refresh plan")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.push_system_row(REFRESH_PLAN_UNAVAILABLE.to_string(), cx);
                }))
                .into_any_element(),
        ),
        cx,
    )
    .into_any_element();

    if total == 0 {
        return v_flex().w_full().child(head).child(empty(
            "work-pane-empty-plan",
            "No plan yet.",
            cx,
        ));
    }

    let mut work = vec![
        card_head("Current work", format!("{percent}% complete"), cx).into_any_element(),
        v_flex()
            .w_full()
            .px(rems(0.625))
            .pb(rems(0.6875))
            .child(
                div()
                    .id("work-pane-plan-progress")
                    .test_support()
                    .h(rems(0.25))
                    .w_full()
                    .rounded_full()
                    .bg(cx.theme().border)
                    .child(
                        div()
                            .id("work-pane-plan-progress-fill")
                            .test_support()
                            .h_full()
                            .rounded_full()
                            .bg(cx.theme().primary)
                            .w(relative(plan_bar_width(percent))),
                    ),
            )
            .into_any_element(),
    ];
    let plan_len = state.plan.len();
    for (index, step) in state.plan.iter().enumerate() {
        let done_at = state.plan_done_at.get(&index).copied();
        let expanded = state.plan_expanded.contains(&index);
        let failed = state.plan_failed.contains(&index);
        work.push(
            plan_step_row(
                index,
                step,
                PlanStepRowState {
                    len: plan_len,
                    current,
                    done_at,
                    expanded,
                    failed,
                },
                cx,
            )
            .into_any_element(),
        );
    }

    let mut body = v_flex()
        .w_full()
        .gap(rems(0.625))
        .child(head)
        .child(card(cx, work));

    // `.card + .card`: the prototype follows the plan with one suggestion the
    // protocol already knows about. Tact's protocol has no suggestion update,
    // so the row is the first pending, unblocked task — the same list the Tasks
    // pane reads — and the card is omitted rather than invented when there is
    // none.
    if let Some(next) = state
        .tasks
        .iter()
        .find(|task| task.status == TaskStatusSnapshot::Pending && task.blocked_by.is_empty())
    {
        body = body.child(card(
            cx,
            vec![
                card_head("Suggested next", "from protocol", cx).into_any_element(),
                suggested_next_row(next, cx).into_any_element(),
            ],
        ));
    }

    body
}

/// One `.step` row for the plan pane's suggestion, which has no step clock.
fn suggested_next_row(task: &tact_protocol::TaskSnapshot, cx: &App) -> impl IntoElement {
    h_flex()
        .w_full()
        .min_h(rems(2.625))
        .items_center()
        .gap_2()
        .px(rems(0.625))
        .py(rems(0.4375))
        .child(
            div()
                .size(rems(1.125))
                .flex_shrink_0()
                .rounded_full()
                .bg(cx.theme().muted)
                .text_color(crate::theme::ink3(cx))
                .flex()
                .items_center()
                .justify_center()
                .child(IconName::Plus),
        )
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .child(
                    div()
                        .truncate()
                        .text_size(rems(0.71875))
                        .font_semibold()
                        .child(SharedString::from(task.subject.clone())),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(rems(0.65625))
                        .text_color(crate::theme::ink3(cx))
                        .child(SharedString::from(task.owner.clone())),
                ),
        )
}

/// The prototype's step clock: how long ago the step finished.
///
/// A step that was already finished when this window opened has no stamp, so
/// it reports its state instead of inventing a time.
fn step_age(done_at: Option<i64>) -> String {
    match done_at {
        Some(stamp) => age_label((now_unix() - stamp).max(0)),
        None => "done".to_string(),
    }
}

/// Fraction of the plan progress track to fill.
///
/// A zero-width child would collapse the rounded cap, so an executed-but-tiny
/// plan keeps a hairline of fill instead.
fn plan_bar_width(percent: usize) -> f32 {
    (percent as f32 / 100.).max(0.02)
}

/// One plan row: status glyph, description, detail, and a trailing state.
#[derive(Clone, Copy)]
struct PlanStepRowState {
    len: usize,
    current: Option<usize>,
    done_at: Option<i64>,
    expanded: bool,
    failed: bool,
}

fn plan_step_row(
    index: usize,
    step: &tact_protocol::PlanStep,
    state: PlanStepRowState,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    let PlanStepRowState {
        len,
        current,
        done_at,
        expanded,
        failed,
    } = state;
    let executed = step.output.is_some();
    let is_current = current == Some(index);
    let is_last = index + 1 == len;
    let line = cx.theme().border;
    let hover_bg = cx.theme().muted;
    let (icon, fg, bg, trailing) = if failed {
        (
            IconName::X,
            cx.theme().danger,
            cx.theme().danger.opacity(crate::theme::tint_alpha(cx)),
            "failed".to_string(),
        )
    } else if executed {
        (
            IconName::Check,
            cx.theme().success,
            cx.theme().success.opacity(crate::theme::tint_alpha(cx)),
            step_age(done_at),
        )
    } else if is_current {
        (
            IconName::CircleDot,
            cx.theme().accent_foreground,
            crate::theme::accent_tint(cx),
            "now".to_string(),
        )
    } else {
        (
            IconName::Circle,
            crate::theme::ink3(cx),
            cx.theme().muted,
            // The prototype distinguishes the step queued behind the current
            // one from the ones after it.
            if current.is_some_and(|current| index == current + 1) {
                "next".to_string()
            } else {
                "later".to_string()
            },
        )
    };

    let row =
        h_flex()
            .id(SharedString::from(format!("plan-step-{index}")))
            .test_support()
            .w_full()
            .min_h(rems(2.625))
            .items_center()
            .gap_2()
            .cursor_pointer()
            .when(!is_last || expanded, |row| {
                row.border_b_1().border_color(line)
            })
            .when(is_current, |row| row.bg(crate::theme::accent_tint(cx)))
            .hover(move |row| row.bg(hover_bg))
            .px(rems(0.625))
            .py(rems(0.4375))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_plan_step(index, cx)))
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
                            .text_size(rems(0.71875))
                            .font_semibold()
                            .child(SharedString::from(step.description.clone())),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(rems(0.65625))
                            .text_color(crate::theme::ink3(cx))
                            .child(SharedString::from(step.tool.clone())),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(rems(0.59375))
                    .text_color(if failed {
                        cx.theme().danger
                    } else {
                        crate::theme::ink3(cx)
                    })
                    .child(SharedString::from(trailing)),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(cx.theme().muted_foreground)
                    .child(div().size(rems(0.875)).child(
                        Icon::new(IconName::ChevronRight).rotate(radians(if expanded {
                            std::f32::consts::FRAC_PI_2
                        } else {
                            0.0
                        })),
                    )),
            );

    let mut detail = v_flex()
        .w_full()
        .gap(rems(0.5))
        .border_t_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().popover)
        .px(rems(2.375))
        .py(rems(0.625));

    if !step.args.is_empty() {
        let args = step
            .args
            .iter()
            .map(|(key, value)| format!("{key}: {value}"))
            .collect::<Vec<_>>()
            .join("\n");
        detail = detail.child(
            v_flex()
                .w_full()
                .gap(rems(0.1875))
                .child(
                    div()
                        .text_size(rems(0.59375))
                        .font_semibold()
                        .text_color(crate::theme::ink3(cx))
                        .child(SharedString::from("INPUT")),
                )
                .child(
                    div()
                        .w_full()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(rems(0.640625))
                        .line_height(relative(1.5))
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(args)),
                ),
        );
    }

    detail = detail.child(
        v_flex()
            .w_full()
            .gap(rems(0.1875))
            .child(
                div()
                    .text_size(rems(0.59375))
                    .font_semibold()
                    .text_color(crate::theme::ink3(cx))
                    .child(SharedString::from(if failed { "ERROR" } else { "RESULT" })),
            )
            .child(
                div()
                    .w_full()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(rems(0.640625))
                    .line_height(relative(1.5))
                    .text_color(if failed {
                        cx.theme().danger
                    } else {
                        cx.theme().muted_foreground
                    })
                    .child(SharedString::from(
                        step.output
                            .clone()
                            .unwrap_or_else(|| "Waiting to run.".to_string()),
                    )),
            ),
    );

    let mut actions = h_flex().items_center().gap(rems(0.375));
    if !step.tool_id.is_empty() {
        let tool_id = step.tool_id.clone();
        actions = actions.child(
            prototype_button(
                SharedString::from(format!("plan-step-open-{index}")),
                false,
                cx,
            )
            .label("Open transcript")
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_plan_step_transcript(&tool_id, cx);
            })),
        );
    }
    if failed {
        actions = actions.child(
            prototype_button(
                SharedString::from(format!("plan-step-retry-{index}")),
                false,
                cx,
            )
            .label("Retry")
            .on_click(cx.listener(move |this, _, _, cx| this.retry_plan_step(index, cx))),
        );
    }
    if !step.tool_id.is_empty() || failed {
        detail = detail.child(actions);
    }

    v_flex()
        .w_full()
        .child(row)
        .when(expanded, |container| container.child(detail))
}

/// File changes recorded from write and edit tool results.
fn diff(state: &SessionState, diffs: &mut DiffPane, cx: &mut Context<TactApp>) -> impl IntoElement {
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
            prototype_button("work-pane-diff-comment", false, cx)
                .label("Comment")
                .tooltip("Draft one batch review in the composer")
                .accessibility_label("Draft a batch review")
                .on_click(cx.listener(|this, _, window, cx| {
                    this.draft_diff_review(window, cx);
                }))
                .into_any_element(),
        ),
        cx,
    );

    if state.diff.is_empty() {
        return v_flex().w_full().child(head).child(empty(
            "work-pane-empty-diff",
            "No file changes yet.",
            cx,
        ));
    }

    let mut body = v_flex().w_full().gap(rems(0.625)).child(head);
    // The cache is keyed per path, so reading the working tree happens once
    // per file per invalidation rather than on every frame.
    let workdir = state.workdir.clone();
    for (index, entry) in state.diff.iter().enumerate() {
        let unified = diffs
            .diff(workdir.as_deref(), &entry.path)
            .map(str::to_string);
        body = body.child(diff_card(entry, unified.as_deref(), index, cx));
    }

    body
}

/// One changed file: a monospace header with stats, then the diff body.
fn diff_card(
    entry: &DiffEntry,
    unified: Option<&str>,
    index: usize,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    let stats = match (entry.added, entry.removed) {
        (Some(added), Some(removed)) => h_flex()
            .flex_shrink_0()
            .gap(rems(0.3125))
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(rems(0.59375))
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
        .bg(cx.theme().muted)
        .px(rems(0.625))
        .py(rems(0.5))
        .child(IconName::FileCode)
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .font_family(cx.theme().mono_font_family.clone())
                .text_size(rems(0.65625))
                .child(SharedString::from(entry.path.clone())),
        )
        .child({
            let path = entry.path.clone();
            prototype_button(SharedString::from(format!("diff-stage-{index}")), false, cx)
                .label("Stage")
                .tooltip("Stage this changed path with git add")
                .accessibility_label("Stage this changed path")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.stage_diff_path(path.clone(), cx);
                }))
        })
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
        .text_size(rems(0.640625))
        .line_height(relative(1.6));
    let mut number = DiffNumbering::default();

    for (kind, text) in diff_lines(unified) {
        // Hunk markers reset both sides, so removed lines use the old-file
        // number while added and context lines use the new-file number.
        if kind == DiffLineKind::Meta {
            number.reset_from(&text);
        }
        let (fg, bg, marker) = match kind {
            DiffLineKind::Added => (
                cx.theme().success,
                cx.theme()
                    .success
                    .opacity(crate::theme::tint_alpha(cx) * 0.7),
                "+",
            ),
            DiffLineKind::Removed => (
                cx.theme().danger,
                cx.theme()
                    .danger
                    .opacity(crate::theme::tint_alpha(cx) * 0.7),
                "-",
            ),
            DiffLineKind::Meta => (cx.theme().muted_foreground, cx.theme().popover, ""),
            DiffLineKind::Context => (cx.theme().foreground, cx.theme().popover, " "),
        };
        let gutter = number
            .gutter(kind)
            .map(|number| number.to_string())
            .unwrap_or_default();

        rows = rows.child(
            h_flex()
                .w_full()
                .min_h(rems(1.1875))
                .bg(bg)
                .items_start()
                .px(rems(0.5))
                .child(
                    div()
                        .w(rems(2.125))
                        .flex_shrink_0()
                        .border_r_1()
                        .border_color(cx.theme().border)
                        .pr(rems(0.4375))
                        .text_color(crate::theme::ink3(cx))
                        .text_align(gpui_kit::TextAlign::Right)
                        .child(SharedString::from(gutter)),
                )
                .child(
                    div()
                        .w(rems(0.875))
                        .flex_shrink_0()
                        .pl(rems(0.1875))
                        .text_color(fg)
                        .child(SharedString::from(marker)),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .pl(rems(0.4375))
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

/// When the task list last reported progress.
///
/// The protocol records a creation, start, and completion stamp per task; the
/// newest of those is the list's age. A snapshot that carries none of them has
/// nothing to report, so the caller leaves the subtitle bare.
fn tasks_updated_at(tasks: &[tact_protocol::TaskSnapshot]) -> Option<i64> {
    tasks
        .iter()
        .flat_map(|task| [task.completed_at, task.started_at, task.created_at])
        .flatten()
        .max()
}

/// The prototype's `.panelHead p` clock.
///
/// Its tasks subtitle writes sub-minute ages in seconds ("4 tasks · updated
/// 12s ago"), so this keeps second precision where the session list's coarse
/// `age_label` deliberately rounds to "just now".
fn updated_age(seconds: i64) -> String {
    if seconds < 60 {
        format!("{seconds}s ago")
    } else {
        age_label(seconds)
    }
}

/// Persistent tasks, rendered as the prototype's Task / Status / Owner table.
fn tasks(
    state: &SessionState,
    tasks_pane: &mut TasksPane,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    let visible = tasks_pane.visible(&state.tasks);
    let count = visible.len();
    let blocked: Vec<_> = visible
        .iter()
        .copied()
        .filter(|task| task.status == TaskStatusSnapshot::Pending && !task.blocked_by.is_empty())
        .collect();

    // `.panelHead p` in the prototype carries the list's age. The protocol
    // stamps creation, start, and completion, so the newest stamp any task
    // carries is real evidence; a snapshot with none stays bare rather than
    // inventing an age.
    let updated = tasks_updated_at(&state.tasks)
        .map(|stamp| format!(" · updated {}", updated_age((now_unix() - stamp).max(0))))
        .unwrap_or_default();

    let head = panel_head(
        "Tasks",
        format!("{count} task{}{updated}", if count == 1 { "" } else { "s" }),
        Some(
            h_flex()
                .items_center()
                .gap(rems(0.375))
                .child(
                    prototype_button("work-pane-tasks-filter", false, cx)
                        .label(format!("Filter: {}", tasks_pane.filter_label()))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.cycle_task_filter(cx);
                        })),
                )
                .child(
                    prototype_button("work-pane-tasks-sort", false, cx)
                        .label(format!("Sort: {}", tasks_pane.sort_label()))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.cycle_task_sort(cx);
                        })),
                )
                .child(
                    prototype_button("work-pane-tasks-new", false, cx)
                        .label("New task")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.push_system_row(NEW_TASK_UNAVAILABLE.to_string(), cx);
                        })),
                )
                .into_any_element(),
        ),
        cx,
    )
    .into_any_element();

    if state.tasks.is_empty() {
        return v_flex().w_full().child(head).child(empty(
            "work-pane-empty-tasks",
            "No tasks in this session.",
            cx,
        ));
    }

    if visible.is_empty() {
        return v_flex().w_full().child(head).child(empty(
            "work-pane-empty-task-filter",
            "No tasks match this filter.",
            cx,
        ));
    }

    let tasks_len = visible.len();
    let mut rows = vec![task_header_row(cx).into_any_element()];
    for (index, task) in visible.into_iter().enumerate() {
        rows.push(task_row(task, index + 1 == tasks_len, cx).into_any_element());
    }

    let mut body = v_flex()
        .w_full()
        .gap(rems(0.625))
        .child(head)
        .child(card_with_id("work-pane-task-table", cx, rows));

    if !blocked.is_empty() {
        let mut items = vec![
            card_head(
                "Blocked by",
                format!(
                    "{} item{}",
                    blocked.len(),
                    if blocked.len() == 1 { "" } else { "s" }
                ),
                cx,
            )
            .into_any_element(),
        ];
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
                            .bg(cx.theme().danger.opacity(crate::theme::tint_alpha(cx)))
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
        .px(rems(0.5))
        .py(rems(0.4375))
        .text_size(rems(0.59375))
        .text_color(crate::theme::ink3(cx))
        // `.tasks th { text-transform: uppercase }`. GPUI has no text
        // transform, so the literals carry the case the CSS asked for.
        .child(div().flex_1().child(SharedString::from("TASK")))
        .child(div().w(rems(5.)).child(SharedString::from("STATUS")))
        .child(div().w(rems(3.)).child(SharedString::from("OWNER")))
}

/// One task row with a status badge.
fn task_row(
    task: &tact_protocol::TaskSnapshot,
    is_last: bool,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    use tact_protocol::TaskStatusSnapshot as Status;

    let blocked = task.status == Status::Pending && !task.blocked_by.is_empty();
    let line = cx.theme().border;
    let hover_bg = cx.theme().muted;
    let (label, fg, bg) = if blocked {
        (
            "Blocked",
            cx.theme().danger,
            cx.theme().danger.opacity(crate::theme::tint_alpha(cx)),
        )
    } else {
        match task.status {
            Status::Completed => (
                "Done",
                cx.theme().success,
                cx.theme().success.opacity(crate::theme::tint_alpha(cx)),
            ),
            Status::InProgress => (
                "In progress",
                cx.theme().accent_foreground,
                crate::theme::accent_tint(cx),
            ),
            Status::Pending => ("Pending", crate::theme::ink3(cx), cx.theme().muted),
        }
    };

    let task_id = task.id;
    let next_status = next_task_status(task.status);
    let status_action = task_status_action(task.status);
    let session_id = task.session_id.clone();
    let has_session = !session_id.is_empty();

    h_flex()
        .id(SharedString::from(format!("task-row-{task_id}")))
        .w_full()
        .items_center()
        .gap_2()
        .when(!is_last, |row| row.border_b_1().border_color(line))
        .hover(move |row| row.bg(hover_bg))
        .px(rems(0.5))
        .py(rems(0.5))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_size(rems(0.6875))
                .font_semibold()
                .child(SharedString::from(task.subject.clone())),
        )
        .child(
            div().w(rems(5.)).flex_shrink_0().child(
                h_flex()
                    .id(SharedString::from(format!("task-update-{task_id}")))
                    .test_support()
                    .h(rems(1.1875))
                    .items_center()
                    .rounded(rems(0.3125))
                    .bg(bg)
                    .px(rems(0.375))
                    .text_size(rems(0.59375))
                    .text_color(fg)
                    .child(SharedString::from(label))
                    .aria_label(SharedString::from(format!(
                        "{status_action} task {task_id}"
                    )))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.update_task_status(task_id, next_status, cx);
                    })),
            ),
        )
        .child(if has_session {
            div()
                .id(SharedString::from(format!("task-open-session-{task_id}")))
                .test_support()
                .w(rems(3.))
                .flex_shrink_0()
                .truncate()
                .text_size(rems(0.6875))
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(task.owner.clone()))
                .aria_label(SharedString::from(format!(
                    "Open session for task {task_id}"
                )))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open_task_session(session_id.clone(), cx);
                }))
                .into_any_element()
        } else {
            div()
                .w(rems(3.))
                .flex_shrink_0()
                .truncate()
                .text_size(rems(0.6875))
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(task.owner.clone()))
                .into_any_element()
        })
}

/// Subagent runs with status and summary.
fn subagents(state: &SessionState, cx: &mut Context<TactApp>) -> impl IntoElement {
    let count = state.subagents.len();
    let head = panel_head(
        "Subagent runs",
        format!("{count} run{}", if count == 1 { "" } else { "s" }),
        None,
        cx,
    );

    if state.subagents.is_empty() {
        return v_flex().w_full().child(head).child(empty(
            "work-pane-empty-subagent",
            "No subagent runs yet.",
            cx,
        ));
    }

    let mut rows = vec![card_head("Runs", format!("{count} total"), cx).into_any_element()];
    for (index, run) in state.subagents.iter().enumerate() {
        let (label, fg, bg) = match run.status {
            SubagentStatusSnapshot::Running => (
                "Running",
                cx.theme().accent_foreground,
                crate::theme::accent_tint(cx),
            ),
            SubagentStatusSnapshot::Completed => (
                "Completed",
                cx.theme().success,
                cx.theme().success.opacity(crate::theme::tint_alpha(cx)),
            ),
            SubagentStatusSnapshot::Failed => (
                "Failed",
                cx.theme().danger,
                cx.theme().danger.opacity(crate::theme::tint_alpha(cx)),
            ),
            SubagentStatusSnapshot::Cancelled => {
                ("Cancelled", crate::theme::ink3(cx), cx.theme().muted)
            }
        };
        let child_id = run.child_id.clone();
        let inspect = if state
            .subagent_transcript
            .as_ref()
            .is_some_and(|transcript| transcript.child_id == run.child_id)
        {
            "Hide transcript"
        } else {
            "Inspect transcript"
        };
        let mut actions = h_flex().items_center().gap(rems(0.375));
        if run.status == SubagentStatusSnapshot::Running {
            let child_id = child_id.clone();
            actions = actions.child(
                prototype_button(
                    SharedString::from(format!("subagent-cancel-{index}")),
                    false,
                    cx,
                )
                .label("Cancel")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.cancel_subagent(&child_id, cx);
                })),
            );
        }
        let child_id = child_id.clone();
        actions = actions.child(
            prototype_button(
                SharedString::from(format!("subagent-inspect-{index}")),
                false,
                cx,
            )
            .label(inspect)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_subagent_transcript(&child_id, cx);
            })),
        );

        rows.push(
            h_flex()
                .w_full()
                .items_center()
                .gap_2()
                .border_b_1()
                .border_color(cx.theme().border)
                .px(rems(0.625))
                .py(rems(0.5))
                .child(
                    v_flex()
                        .min_w_0()
                        .flex_1()
                        .child(
                            div()
                                .truncate()
                                .text_size(rems(0.71875))
                                .font_semibold()
                                .child(SharedString::from(run.child_id.clone())),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(rems(0.65625))
                                .text_color(cx.theme().muted_foreground)
                                .child(SharedString::from(run.summary_first.clone())),
                        ),
                )
                .child(actions.into_any_element())
                .child(
                    div()
                        .flex_shrink_0()
                        .h(rems(1.1875))
                        .rounded(rems(0.3125))
                        .bg(bg)
                        .px(rems(0.375))
                        .text_size(rems(0.59375))
                        .text_color(fg)
                        .child(SharedString::from(label)),
                )
                .into_any_element(),
        );
    }

    let mut body = v_flex().w_full().child(head).child(card(cx, rows));
    if let Some(transcript) = &state.subagent_transcript {
        body = body.child(subagent_transcript_card(transcript, cx));
    }
    body
}

/// The stored transcript for one inspected subagent.
fn subagent_transcript_card(
    transcript: &crate::session::SubagentTranscriptState,
    cx: &App,
) -> impl IntoElement {
    use tact_session::{HistoryBlock, HistoryRole};

    let note = if transcript.loading {
        "loading…".to_string()
    } else if transcript.error.is_some() {
        "error".to_string()
    } else {
        format!("{} messages", transcript.messages.len())
    };
    let mut body = v_flex().w_full().gap(rems(0.5)).p(rems(0.625));

    if let Some(error) = &transcript.error {
        body = body.child(
            div()
                .text_size(rems(0.6875))
                .text_color(cx.theme().danger)
                .child(SharedString::from(format!(
                    "Could not load transcript: {error}"
                ))),
        );
    } else if transcript.loading {
        body = body.child(div().child(LoadingState::new().label("Loading the stored transcript…")));
    } else if transcript.messages.is_empty() {
        body = body.child(
            div()
                .text_size(rems(0.6875))
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from("No stored transcript for this run.")),
        );
    } else {
        for message in &transcript.messages {
            let role = match message.role {
                HistoryRole::User => "You",
                HistoryRole::Assistant => "Subagent",
            };
            body = body.child(
                v_flex()
                    .w_full()
                    .gap(rems(0.25))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .pb(rems(0.5))
                    .child(
                        div()
                            .text_size(rems(0.59375))
                            .font_semibold()
                            .text_color(crate::theme::ink3(cx))
                            .child(SharedString::from(role)),
                    ),
            );
            for block in &message.blocks {
                let (label, text, mono) = match block {
                    HistoryBlock::Text(text) => (None, text.clone(), false),
                    HistoryBlock::Thinking(text) => (Some("Thinking"), text.clone(), false),
                    HistoryBlock::ToolUse { name, detail, .. } => (
                        Some("Tool"),
                        if detail.is_empty() {
                            name.clone()
                        } else {
                            format!("{name} · {detail}")
                        },
                        true,
                    ),
                    HistoryBlock::ToolResult { output, .. } => {
                        (Some("Result"), output.clone(), true)
                    }
                };
                let mut row = v_flex().w_full().gap(rems(0.125));
                if let Some(label) = label {
                    row = row.child(
                        div()
                            .text_size(rems(0.5625))
                            .font_semibold()
                            .text_color(crate::theme::ink3(cx))
                            .child(SharedString::from(label)),
                    );
                }
                row = row.child(
                    div()
                        .w_full()
                        .when(mono, |text| {
                            text.font_family(cx.theme().mono_font_family.clone())
                        })
                        .text_size(rems(0.65625))
                        .line_height(relative(1.5))
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(text)),
                );
                body = body.child(row);
            }
        }
    }

    card_with_id(
        "work-pane-subagent-transcript",
        cx,
        vec![
            card_head(
                "Transcript",
                format!("{} · {note}", transcript.child_id),
                cx,
            )
            .into_any_element(),
            body.into_any_element(),
        ],
    )
}

/// Workspace tree with expandable directories and a file preview.
fn files_tree(
    state: &SessionState,
    files: &mut FilesPane,
    cx: &mut Context<TactApp>,
) -> impl IntoElement {
    let changed = state.diff.len();
    let Some(root) = state.workdir.clone() else {
        return v_flex().w_full().child(empty(
            "work-pane-empty-workdir",
            "No workspace directory.",
            cx,
        ));
    };

    let selected = files.selected_path().map(Path::to_path_buf);
    let rows = files.rows(&root);
    let head = panel_head(
        "Files",
        format!(
            "{changed} changed · {} open",
            rows.iter().filter(|row| row.is_dir && row.expanded).count()
        ),
        Some(
            prototype_button("work-pane-files-add", false, cx)
                .label("Add file")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.push_system_row(ADD_FILE_UNAVAILABLE.to_string(), cx);
                }))
                .into_any_element(),
        ),
        cx,
    );

    if rows.is_empty() {
        return v_flex().w_full().child(head).child(empty(
            "work-pane-empty-files",
            "The workspace is empty.",
            cx,
        ));
    }

    let hover_bg = crate::theme::accent_tint(cx);
    let hover_ink = cx.theme().accent_foreground;
    // `.tree .row2 { color: var(--ink2) }`; the hovered and the expanded row
    // take `--accentInk` below.
    let ink2 = cx.theme().muted_foreground;
    let mut tree = v_flex().w_full().px(rems(0.4375)).py(rems(0.5));
    // `rows` is the pane's cached slice, so each row is borrowed rather than
    // owned; only the label has to be copied out per frame.
    for row in rows {
        let indent = rems_for_depth(row.depth);
        let path = row.path.clone();
        let icon = match (row.is_dir, row.expanded) {
            (true, true) => IconName::FolderOpen,
            (true, false) => IconName::Folder,
            (false, _) => IconName::File,
        };
        // The row is the observation anchor for tests (`file-row-<path>`), so
        // its expand toggle takes a separate id: two observed elements sharing
        // one id make every query for it ambiguous.
        let row_id = SharedString::from(format!("file-row-{}", path.display()));
        let toggle_id = SharedString::from(format!("file-toggle-{}", path.display()));
        let is_expanded = row.is_dir && row.expanded;
        let is_dir = row.is_dir;
        let is_selected = selected.as_deref() == Some(path.as_path());
        let toggle_path = path.clone();

        let marker = if row.is_dir {
            Button::new(toggle_id)
                .icon(icon)
                .tooltip(if row.expanded { "Collapse" } else { "Expand" })
                .accessibility_label(if row.expanded {
                    "Collapse folder"
                } else {
                    "Expand folder"
                })
                .ghost()
                // The prototype's tree rows are `.tree .row2{color:var(--ink2)}`
                // and the chevron inherits that; the ghost variant would paint
                // `secondary_foreground` (`--ink`) and out-shout its own label.
                .text_color(cx.theme().muted_foreground)
                .compact()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_directory(toggle_path.clone());
                    // The stock button does not stop a press it *has* a
                    // handler for, so without this the press would also reach
                    // the row below and toggle the directory straight back.
                    cx.stop_propagation();
                    cx.notify();
                }))
                .into_any_element()
        } else {
            div().px(rems(0.5)).child(icon).into_any_element()
        };

        tree = tree.child(
            h_flex()
                .id(row_id)
                .test_support()
                .w_full()
                .min_w_0()
                .h(rems(1.75))
                .items_center()
                .gap(rems(0.375))
                .rounded(rems(0.375))
                .pl(indent)
                .pr(rems(0.4375))
                .text_color(ink2)
                .when(is_expanded, move |row| {
                    row.bg(hover_bg).text_color(hover_ink)
                })
                .when(is_selected, move |row| {
                    row.bg(hover_bg).text_color(hover_ink)
                })
                .hover(move |row| row.bg(hover_bg).text_color(hover_ink))
                .aria_selected(is_selected)
                // Both kinds of row are live, and the chevron above stops its
                // own press from reaching this handler, so a folder toggles
                // exactly once whether the press lands on the icon or the name.
                .cursor_pointer()
                .tab_index(0)
                .focus_visible({
                    let ring = focus_visible_ring(cx);
                    move |style| style.shadow(ring.clone())
                })
                .on_click(cx.listener({
                    let path = path.clone();
                    move |this, _, _, cx| {
                        if is_dir {
                            this.toggle_directory(path.clone());
                        } else {
                            this.select_file(path.clone(), cx);
                        }
                        cx.notify();
                    }
                }))
                // The row is a tab stop, so it has to answer the keys a
                // focused row is expected to answer. Without this the row was
                // reachable by keyboard and inert once it got there.
                .on_key_down(cx.listener({
                    let path = path.clone();
                    move |this, event: &KeyDownEvent, _, cx| {
                        if !matches!(event.keystroke.key.as_str(), "enter" | "return" | "space") {
                            return;
                        }
                        if is_dir {
                            this.toggle_directory(path.clone());
                        } else {
                            this.select_file(path.clone(), cx);
                        }
                        cx.notify();
                    }
                }))
                .child(marker)
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(rems(0.6875))
                        .child(SharedString::from(row.name.clone())),
                ),
        );
    }

    let selected_preview = files.selected_preview().is_some();
    let mut body = v_flex().w_full().child(head);
    if selected_preview {
        body = body.child(file_preview_card(files, cx));
    }
    body.child(card(cx, vec![tree.into_any_element()]))
}

/// The content below the tree for the file selected in it.
fn file_preview_card(files: &FilesPane, cx: &mut Context<TactApp>) -> AnyElement {
    let mut children: Vec<AnyElement> = Vec::new();
    let owner = cx.entity().downgrade();
    let Some(preview) = files.selected_preview() else {
        return card_with_id(
            "work-pane-file-preview",
            cx,
            vec![
                card_head("Preview", "select a file", cx).into_any_element(),
                empty(
                    "work-pane-empty-file-preview",
                    "Select a file in the tree to preview it.",
                    cx,
                )
                .into_any_element(),
            ],
        )
        .into_any_element();
    };

    let name = preview
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| preview.path.display().to_string());
    let note = if let Some(entries) = preview.directory {
        format!("{entries} entr{}", if entries == 1 { "y" } else { "ies" })
    } else if preview.error.is_some() {
        "unreadable".to_string()
    } else if preview.binary {
        "binary".to_string()
    } else {
        format!(
            "{} line{}{}",
            preview.lines.len(),
            if preview.lines.len() == 1 { "" } else { "s" },
            if preview.truncated {
                " · truncated"
            } else {
                ""
            }
        )
    };
    children.push(card_head("Preview", format!("{name} · {note}"), cx).into_any_element());
    children.push(
        h_flex()
            .w_full()
            .items_center()
            .gap(rems(0.375))
            .border_b_1()
            .border_color(cx.theme().border)
            .px(rems(0.6875))
            .py(rems(0.5))
            .child(
                prototype_button("work-pane-file-reveal", false, cx)
                    .label("Reveal")
                    .tooltip("Reveal the selected file in the file manager")
                    .accessibility_label("Reveal the selected file")
                    .on_click(cx.listener(|this, _, _, cx| this.reveal_selected_file(cx))),
            )
            .child(
                prototype_button("work-pane-file-mention", false, cx)
                    .label("Mention")
                    .tooltip("Insert the selected file into the composer")
                    .accessibility_label("Mention the selected file in the composer")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.mention_selected_file(window, cx)),
                    ),
            )
            .child(
                prototype_button("work-pane-file-neovim", false, cx)
                    .label("Neovim")
                    .tooltip("Open the selected file in embedded Neovim")
                    .accessibility_label("Open the selected file in Neovim")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_selected_file_in_neovim(window, cx)
                    })),
            )
            .child({
                let selection_owner = owner.clone();
                Popover::new("work-pane-file-neovim-selection-popover")
                    .anchor(gpui_kit::Anchor::TopLeft)
                    .trigger(
                        prototype_button("work-pane-file-neovim-selection", false, cx)
                            .label("Nvim selection")
                            .tooltip("Reference Neovim's last visual selection")
                            .accessibility_label("Reference Neovim selection"),
                    )
                    .content(move |_state, _window, cx| {
                        let owner = selection_owner.clone();
                        v_flex()
                            .id("work-pane-file-neovim-selection-panel")
                            .test_support()
                            .gap_2()
                            .min_w(rems(13.))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(
                                        "Insert Neovim's last visual selection into the composer.",
                                    )),
                            )
                            .child(
                                prototype_button(
                                    "work-pane-file-neovim-selection-confirm",
                                    true,
                                    cx,
                                )
                                .label("Reference selection")
                                .on_click(move |_, window, cx| {
                                    let _ = owner.update(cx, |app, cx| {
                                        app.reference_neovim_selection(window, cx)
                                    });
                                }),
                            )
                    })
            })
            .into_any_element(),
    );

    if let Some(entries) = preview.directory {
        children.push(
            empty(
                "work-pane-directory-preview",
                &format!(
                    "Directory with {entries} entr{}. Reveal opens it in the file manager; Mention inserts its path.",
                    if entries == 1 { "y" } else { "ies" }
                ),
                cx,
            )
            .into_any_element(),
        );
    } else if let Some(error) = preview.error.as_deref() {
        children.push(empty("work-pane-file-error", error, cx).into_any_element());
    } else if preview.binary {
        children.push(
            empty(
                "work-pane-file-binary",
                "Binary files are not rendered in the preview.",
                cx,
            )
            .into_any_element(),
        );
    } else if preview.lines.is_empty() {
        children.push(
            empty("work-pane-empty-file-content", "This file is empty.", cx).into_any_element(),
        );
    } else {
        let mut body = v_flex()
            .id("work-pane-file-content")
            .test_support()
            .w_full()
            .px(rems(0.6875))
            .py(rems(0.5))
            .gap(rems(0.0625));
        for (index, line) in preview.lines.iter().enumerate() {
            body = body.child(
                div()
                    .w_full()
                    .min_w_0()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(rems(0.625))
                    .line_height(relative(1.45))
                    .whitespace_nowrap()
                    .truncate()
                    .child(SharedString::from(format!("{:>4}  {line}", index + 1))),
            );
        }
        if preview.truncated {
            body = body.child(
                div()
                    .pt(rems(0.25))
                    .text_size(rems(0.625))
                    .text_color(crate::theme::ink3(cx))
                    .child(SharedString::from("Preview truncated.")),
            );
        }
        children.push(body.into_any_element());
    }

    card_with_id("work-pane-file-preview", cx, children).into_any_element()
}

/// Indent step for one tree depth.
fn rems_for_depth(depth: usize) -> gpui_kit::Rems {
    // `.row2 { padding: 0 7px }` plus the prototype's 12px depth step.
    gpui_kit::rems(depth as f32 * 0.75 + 0.4375)
}

/// The Browser pane: an address bar that hands URLs to the system browser.
///
/// This is deliberately *not* an embedded web view, and the pane says so. GPUI
/// renders its own GPU surface and has no way to host a `webkit2gtk` or `wry`
/// view inside it, so an "embedded browser" here would be a screenshot at
/// worst and a lie at best. What it does instead is what the tab can honestly
/// do: normalize an address, hand it to the desktop's default browser, and keep
/// the last few so a link is one press away.
fn browser_pane(
    input: &gpui_kit::Entity<InputState>,
    history: &[String],
    cx: &mut Context<TactApp>,
) -> AnyElement {
    let field_focus = input.read(cx).focus_handle(cx);
    let mut body = v_flex()
        .gap_3()
        .child(panel_head(
            "Browser",
            "Links open in your system browser; Tact has no embedded web view",
            None,
            cx,
        ))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap_2()
                .child(
                    // The field is wrapped so the whole box focuses it, the
                    // way the sidebar's search field does: GPUI does not focus
                    // an input from a press that lands on its surroundings.
                    div()
                        .id("browser-url")
                        .test_support()
                        .flex_1()
                        .min_w_0()
                        .track_focus(&field_focus)
                        .on_mouse_down(MouseButton::Left, {
                            let input = input.clone();
                            move |_, window, cx| {
                                input.update(cx, |input, cx| {
                                    input.focus_handle(cx).focus(window, cx);
                                });
                            }
                        })
                        .child(Input::new(input).appearance(false)),
                )
                .child(
                    prototype_button("browser-open", false, cx)
                        .label("Open")
                        .icon(IconName::ExternalLink)
                        .tooltip("Open the address in your default browser")
                        .accessibility_label("Open address in browser")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.open_browser_url(window, cx)),
                        ),
                ),
        );

    if history.is_empty() {
        return body
            .child(empty(
                "work-pane-empty-browser",
                "No addresses opened yet. Type one above and press Open.",
                cx,
            ))
            .into_any_element();
    }

    let mut list = v_flex().w_full().gap(rems(0.25));
    for (index, url) in history.iter().enumerate() {
        let url = url.clone();
        list = list.child(
            h_flex()
                .id(SharedString::from(format!("browser-history-{index}")))
                .test_support()
                .aria_label(SharedString::from(format!("Open {url}")))
                .w_full()
                .items_center()
                .gap_2()
                .px_2()
                .py(rems(0.375))
                .rounded(rems(0.375))
                .text_size(rems(0.6875))
                .text_color(cx.theme().foreground)
                .tab_index(0)
                .focus_visible({
                    let ring = focus_visible_ring(cx);
                    move |style| style.shadow(ring.clone())
                })
                .hover(|style| style.bg(cx.theme().accent))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open_browser_history(index, cx);
                }))
                .child(IconName::Link)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(SharedString::from(url)),
                ),
        );
    }

    body = body
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .child(
                    div()
                        .text_size(rems(0.625))
                        .text_color(crate::theme::ink3(cx))
                        .child(SharedString::from("Recent addresses")),
                )
                .child(
                    prototype_button("browser-clear", false, cx)
                        .label("Clear")
                        .icon(IconName::X)
                        .tooltip("Forget every remembered address")
                        .accessibility_label("Clear browser history")
                        .on_click(cx.listener(|this, _, _, cx| this.clear_browser_history(cx))),
                ),
        )
        .child(list);

    body.into_any_element()
}

/// The Terminal pane: a shell in a real PTY, drawn as a character grid.
///
/// Nothing is spawned until the user presses Start. A pane that launched a
/// shell merely by being opened would also launch one in every test that walks
/// the work-pane tabs, and a shell is a process with side effects — the user
/// should be the one who asks for it.
fn terminal_pane(
    terminal: &mut Option<TerminalPane>,
    focus: &FocusHandle,
    size: (u16, u16),
    cx: &mut Context<TactApp>,
) -> AnyElement {
    let Some(pane) = terminal.as_mut() else {
        return v_flex()
            .gap_3()
            .child(panel_head(
                "Terminal",
                "Run a shell in this workspace",
                None,
                cx,
            ))
            .child(empty(
                "work-pane-empty-terminal",
                "No terminal is running. Starting one opens your shell in the workspace directory.",
                cx,
            ))
            .child(
                prototype_button("terminal-start", false, cx)
                    .label("Start terminal")
                    .icon(IconName::Terminal)
                    .tooltip("Open a shell in the workspace directory")
                    .accessibility_label("Start terminal")
                    .on_click(cx.listener(|this, _, _, cx| this.start_terminal(cx))),
            )
            .into_any_element();
    };

    // Drain anything the reader thread produced before drawing. The pump task
    // also does this, but polling here keeps a frame self-sufficient: a render
    // triggered by anything at all picks up the shell's output, so the grid is
    // never a frame behind a wake-up that did not arrive.
    pane.poll();
    // The PTY and the parser are resized together, to the grid the pane will
    // actually draw rather than to whatever size it opened with.
    pane.resize(size.0, size.1);

    let exited = pane.exited();
    let program = pane.program().to_string();
    let (cursor_row, cursor_col) = pane.cursor();
    let show_cursor = exited.is_none();
    let cols = pane.cols();
    let mut grid = v_flex().w_full().gap_0();
    for row in 0..pane.rows() {
        let mut line = h_flex()
            .w_full()
            .h(rems(0.9375))
            .items_center()
            .flex_shrink_0();
        // The cursor row is drawn in three pieces so the block lands in the
        // cell the shell put the cursor in, not appended to the line.
        if show_cursor && row == cursor_row {
            for run in pane.row_runs_between(row, 0, cursor_col) {
                line = line.child(terminal_run(run, cx));
            }
            line = line.child(terminal_cursor_cell(
                pane.cell_text(row, cursor_col),
                row,
                cx,
            ));
            for run in pane.row_runs_between(row, cursor_col + 1, cols) {
                line = line.child(terminal_run(run, cx));
            }
        } else {
            for run in pane.row_runs(row) {
                line = line.child(terminal_run(run, cx));
            }
        }
        grid = grid.child(
            line.id(SharedString::from(format!("terminal-row-{row}")))
                .test_support(),
        );
    }

    let mut body = v_flex().gap_2().child(
        h_flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_family(cx.theme().mono_font_family.clone())
                    .child(SharedString::from(program)),
            )
            .child(
                div()
                    .text_size(rems(0.625))
                    .text_color(crate::theme::ink3(cx))
                    .child(SharedString::from(format!(
                        "{}×{}",
                        pane.cols(),
                        pane.rows()
                    ))),
            )
            .child(div().flex_1())
            .child(
                prototype_button("terminal-restart", false, cx)
                    .label("Restart")
                    .icon(IconName::RotateCw)
                    .tooltip("Kill the shell and start a new one")
                    .accessibility_label("Restart terminal")
                    .on_click(cx.listener(|this, _, _, cx| this.restart_terminal(cx))),
            ),
    );

    if let Some(code) = exited {
        body = body.child(
            div()
                .id("terminal-exited")
                .test_support()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(SharedString::from(format!(
                    "The shell exited with status {code}. Restart to open a new one."
                ))),
        );
    }

    body.child(
        // The grid is the keyboard target: focus it and every keystroke becomes
        // PTY input. `.tab_index(0)` makes it reachable without the mouse.
        div()
            .id("terminal-grid")
            .test_support()
            // The grid is a canvas: without a name it is an unlabelled box in
            // the accessibility tree. The visible text is the honest label.
            .aria_label(SharedString::from(
                pane.contents()
                    .trim_end()
                    .chars()
                    .take(500)
                    .collect::<String>(),
            ))
            .track_focus(focus)
            .tab_index(0)
            .focus_visible({
                let ring = focus_visible_ring(cx);
                move |style| style.shadow(ring.clone())
            })
            .on_click({
                let focus = focus.clone();
                move |_, window, cx| focus.focus(window, cx)
            })
            .w_full()
            .p_2()
            .rounded(rems(0.375))
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(rems(0.75))
            .text_color(cx.theme().foreground)
            .child(grid),
    )
    .into_any_element()
}

/// The block cursor: the cell's own character over the accent fill.
fn terminal_cursor_cell(under: String, row: u16, cx: &App) -> AnyElement {
    div()
        .bg(cx.theme().accent)
        .text_color(cx.theme().accent_foreground)
        .whitespace_nowrap()
        .id(SharedString::from(format!("terminal-cursor-{row}")))
        .test_support()
        .child(SharedString::from(if under.is_empty() {
            " ".to_string()
        } else {
            under
        }))
        .into_any_element()
}

/// One styled run of terminal cells.
fn terminal_run(run: crate::terminal::TermRun, cx: &App) -> AnyElement {
    let (fg, bg) = if run.inverse {
        (
            terminal_color(run.bg, crate::theme::ink3(cx), cx),
            terminal_color(run.fg, cx.theme().foreground, cx),
        )
    } else {
        (
            terminal_color(run.fg, cx.theme().foreground, cx),
            terminal_color(run.bg, cx.theme().popover, cx),
        )
    };
    div()
        .bg(bg)
        .text_color(fg)
        .when(run.bold, |this| this.font_weight(FontWeight::BOLD))
        .when(run.underline, |this| this.underline())
        .whitespace_nowrap()
        .child(SharedString::from(run.text))
        .into_any_element()
}

/// Map a terminal colour onto the theme.
///
/// The 16 ANSI slots are the terminal's own palette, not the app's, so they are
/// fixed values chosen to read against both themes; `Default` is the one slot
/// that belongs to the application and takes the theme's own ink.
fn terminal_color(color: TermColor, fallback: gpui_kit::Hsla, cx: &App) -> gpui_kit::Hsla {
    const ANSI: [(u8, u8, u8); 16] = [
        (0x1c, 0x1c, 0x1c),
        (0xcc, 0x33, 0x33),
        (0x33, 0x99, 0x33),
        (0xcc, 0x99, 0x33),
        (0x33, 0x66, 0xcc),
        (0x99, 0x33, 0x99),
        (0x33, 0x99, 0x99),
        (0xcc, 0xcc, 0xcc),
        (0x66, 0x66, 0x66),
        (0xff, 0x66, 0x66),
        (0x66, 0xff, 0x66),
        (0xff, 0xff, 0x66),
        (0x66, 0x99, 0xff),
        (0xff, 0x66, 0xff),
        (0x66, 0xff, 0xff),
        (0xff, 0xff, 0xff),
    ];
    let _ = cx;
    match color {
        TermColor::Default => fallback,
        TermColor::Rgb(r, g, b) => {
            gpui_kit::rgba(((r as u32) << 24) | ((g as u32) << 16) | ((b as u32) << 8) | 0xff)
                .into()
        }
        TermColor::Indexed(index) => {
            let (r, g, b) = match index {
                0..=15 => ANSI[index as usize],
                16..=231 => {
                    // The xterm 6×6×6 cube: the index carries three base-6
                    // digits, each mapped onto the 0/95/135/175/215/255 ramp.
                    let cube = index - 16;
                    let ramp = |value: u8| if value == 0 { 0 } else { 55 + value * 40 };
                    (ramp(cube / 36), ramp((cube % 36) / 6), ramp(cube % 6))
                }
                _ => {
                    // 232..=255 are the 24-step grey ramp.
                    let level = 8 + (index - 232) * 10;
                    (level, level, level)
                }
            };
            gpui_kit::rgba(((r as u32) << 24) | ((g as u32) << 16) | ((b as u32) << 8) | 0xff)
                .into()
        }
    }
}

/// The Stats pane: the session's own numbers, drawn rather than listed.
///
/// The prototype parks chart-heavy dashboards past v1, and this is the honest
/// version of one: everything it draws already exists in [`SessionState`], so
/// the pane adds a reading of the session rather than a second source of
/// truth. Three charts answer the three questions a work pane can actually
/// answer mid-session -- how much context has been spent, how far the tasks
/// have moved, and which files the change is concentrated in.
fn stats(state: &SessionState, cx: &mut Context<TactApp>) -> impl IntoElement {
    let usage = state.usage.as_ref();
    let prompt = usage.map(|u| u.prompt).unwrap_or(0);
    let completion = usage.map(|u| u.completion).unwrap_or(0);
    let cache_hit = usage.map(|u| u.prompt_cache_hit_tokens).unwrap_or(0);
    let cache_miss = usage.map(|u| u.prompt_cache_miss_tokens).unwrap_or(0);
    let reasoning = usage.map(|u| u.reasoning_tokens).unwrap_or(0);
    let total = usage
        .map(|u| u.total)
        .unwrap_or_else(|| prompt.saturating_add(completion));

    let tasks_total = state.tasks.len();
    let task_buckets = [
        (
            "Pending",
            state
                .tasks
                .iter()
                .filter(|task| task.status == TaskStatusSnapshot::Pending)
                .count(),
        ),
        (
            "In progress",
            state
                .tasks
                .iter()
                .filter(|task| task.status == TaskStatusSnapshot::InProgress)
                .count(),
        ),
        (
            "Completed",
            state
                .tasks
                .iter()
                .filter(|task| task.status == TaskStatusSnapshot::Completed)
                .count(),
        ),
    ];

    let plan_total = state.plan.len();
    let plan_done = state.plan_done_at.len().min(plan_total);
    let plan_failed = state.plan_failed.len();

    let added: u32 = state.diff.iter().filter_map(|entry| entry.added).sum();
    let removed: u32 = state.diff.iter().filter_map(|entry| entry.removed).sum();

    let running_subagents = state
        .subagents
        .iter()
        .filter(|run| run.status == SubagentStatusSnapshot::Running)
        .count();

    let context_percent = if total > 0 {
        ((prompt as u64 * 100) / total as u64).min(100) as u32
    } else {
        0
    };

    let mut body = v_flex().gap_3().child(panel_head(
        "Session statistics",
        "Tokens, tasks, and recorded changes for this session",
        None,
        cx,
    ));

    if usage.is_none() && tasks_total == 0 && plan_total == 0 && state.diff.is_empty() {
        return body
            .child(empty(
                "work-pane-empty-stats",
                "No session activity yet. Statistics appear after the first turn.",
                cx,
            ))
            .into_any_element();
    }

    body = body.child(
        h_flex()
            .w_full()
            .flex_wrap()
            .gap_2()
            .child(stat_tile(
                "stats-tile-tokens",
                "Tokens",
                format_thousands(total),
                format!("{context_percent}% context"),
                cx,
            ))
            .child(stat_tile(
                "stats-tile-tasks",
                "Tasks",
                format!(
                    "{} / {}",
                    tasks_total.saturating_sub(task_buckets[0].1 + task_buckets[1].1),
                    tasks_total
                ),
                format!(
                    "{} pending · {} running",
                    task_buckets[0].1, task_buckets[1].1
                ),
                cx,
            ))
            .child(stat_tile(
                "stats-tile-plan",
                "Plan",
                format!("{plan_done} / {plan_total}"),
                if plan_failed > 0 {
                    format!("{plan_failed} failed")
                } else {
                    "no failures".to_string()
                },
                cx,
            ))
            .child(stat_tile(
                "stats-tile-diff",
                "Diff",
                format!("+{added} \u{2212}{removed}"),
                format!("{} files", state.diff.len()),
                cx,
            )),
    );

    // Token split: one stacked bar plus a legend, so the pane answers "where
    // did the context go" without a table of six numbers.
    let token_split = prompt.saturating_add(completion).max(1);
    let prompt_share = prompt as f32 / token_split as f32;
    body = body.child(chart_card(
        "stats-chart-tokens",
        "Token usage",
        "Prompt is the context sent; completion is what the model wrote back.",
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .w_full()
                    .h(rems(0.75))
                    .rounded(rems(0.25))
                    .overflow_hidden()
                    .bg(cx.theme().muted)
                    .child(
                        div()
                            .h_full()
                            .w(relative(prompt_share))
                            .bg(cx.theme().accent),
                    ),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .text_size(rems(0.6875))
                    .text_color(crate::theme::ink3(cx))
                    .child(legend_swatch("Prompt", prompt, cx.theme().accent, cx))
                    .child(legend_swatch("Completion", completion, cx.theme().info, cx))
                    .child(legend_swatch(
                        "Cache hit",
                        cache_hit,
                        cx.theme().success,
                        cx,
                    ))
                    .child(legend_swatch(
                        "Cache miss",
                        cache_miss,
                        cx.theme().danger,
                        cx,
                    ))
                    .child(legend_swatch(
                        "Reasoning",
                        reasoning,
                        cx.theme().muted_foreground,
                        cx,
                    )),
            )
            .into_any_element(),
        cx,
    ));

    // Tasks by status: three vertical bars, scaled to the tallest bucket so a
    // session with one task still reads as a chart rather than a sliver.
    let tallest = task_buckets
        .iter()
        .map(|(_, count)| *count)
        .max()
        .unwrap_or(0)
        .max(1);
    body = body.child(chart_card(
        "stats-chart-tasks",
        "Tasks by status",
        "The same snapshot the Tasks pane lists, counted.",
        h_flex()
            .w_full()
            .items_end()
            .gap_3()
            .h(rems(6.))
            .children(task_buckets.into_iter().map(|(label, count)| {
                let share = count as f32 / tallest as f32;
                v_flex()
                    .flex_1()
                    .items_center()
                    .gap_1()
                    .child(SharedString::from(count.to_string()))
                    .child(
                        div()
                            .w_full()
                            .h(rems(4.0 * share))
                            .min_h(px(2.))
                            .rounded(rems(0.25))
                            .bg(cx.theme().accent),
                    )
                    .child(
                        div()
                            .text_size(rems(0.625))
                            .text_color(crate::theme::ink3(cx))
                            .child(SharedString::from(label)),
                    )
            }))
            .into_any_element(),
        cx,
    ));

    // Changes by file: the top five changed paths by line count, additions over
    // removals, so a wide change is visible without opening Diff.
    let mut by_file: Vec<(String, u32, u32)> = state
        .diff
        .iter()
        .map(|entry| {
            (
                entry.path.clone(),
                entry.added.unwrap_or(0),
                entry.removed.unwrap_or(0),
            )
        })
        .filter(|(_, added, removed)| added + removed > 0)
        .collect();
    by_file.sort_by_key(|(_, added, removed)| std::cmp::Reverse(added + removed));
    by_file.truncate(5);

    if !by_file.is_empty() {
        let widest = by_file
            .iter()
            .map(|(_, added, removed)| added + removed)
            .max()
            .unwrap_or(1)
            .max(1);
        body = body.child(chart_card(
            "stats-chart-diff",
            "Changes by file",
            "The five largest recorded changes, additions over removals.",
            v_flex()
                .w_full()
                .gap_2()
                .children(by_file.into_iter().map(|(path, added, removed)| {
                    let share = (added + removed) as f32 / widest as f32;
                    v_flex()
                        .w_full()
                        .gap(rems(0.25))
                        .child(
                            h_flex()
                                .justify_between()
                                .text_size(rems(0.6875))
                                .text_color(crate::theme::ink3(cx))
                                .child(div().truncate().child(SharedString::from(path)))
                                .child(SharedString::from(format!("+{added} \u{2212}{removed}"))),
                        )
                        .child(
                            h_flex()
                                .w_full()
                                .h(rems(0.375))
                                .rounded(rems(0.1875))
                                .overflow_hidden()
                                .bg(cx.theme().muted)
                                .child(
                                    div()
                                        .h_full()
                                        .w(relative(
                                            share * added as f32 / (added + removed).max(1) as f32,
                                        ))
                                        .bg(cx.theme().success),
                                )
                                .child(div().h_full().flex_1().bg(cx.theme().danger.opacity(0.55))),
                        )
                }))
                .into_any_element(),
            cx,
        ));
    }

    if running_subagents > 0 {
        body = body.child(
            div()
                .id("stats-running-subagents")
                .test_support()
                .text_sm()
                .text_color(crate::theme::ink3(cx))
                .child(SharedString::from(format!(
                    "{running_subagents} subagent{} running",
                    if running_subagents == 1 { "" } else { "s" }
                ))),
        );
    }

    body.into_any_element()
}

/// One number with its label and a subordinate hint.
fn stat_tile(
    id: &'static str,
    label: &'static str,
    value: String,
    hint: String,
    cx: &App,
) -> impl IntoElement {
    v_flex()
        .id(id)
        .test_support()
        .aria_label(SharedString::from(format!("{label}: {value}")))
        .min_w(rems(7.5))
        .flex_1()
        .gap(rems(0.25))
        .rounded(rems(0.5))
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().muted)
        .px_2()
        .py(rems(0.5))
        .child(
            div()
                .text_size(rems(0.625))
                .text_color(crate::theme::ink3(cx))
                .child(SharedString::from(label)),
        )
        .child(
            div()
                .text_color(cx.theme().foreground)
                .child(SharedString::from(value)),
        )
        .child(
            div()
                .text_size(rems(0.625))
                .text_color(crate::theme::ink3(cx))
                .child(SharedString::from(hint)),
        )
}

/// A bordered card with a heading, a subtitle, and one chart body.
fn chart_card(
    id: &'static str,
    title: &'static str,
    subtitle: &'static str,
    body: AnyElement,
    cx: &App,
) -> impl IntoElement {
    v_flex()
        .id(id)
        .test_support()
        .w_full()
        .gap_2()
        .rounded(rems(0.5))
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().popover)
        .p_2()
        .child(
            v_flex()
                .gap(rems(0.125))
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().foreground)
                        .child(SharedString::from(title)),
                )
                .child(
                    div()
                        .text_size(rems(0.625))
                        .text_color(crate::theme::ink3(cx))
                        .child(SharedString::from(subtitle)),
                ),
        )
        .child(body)
}

/// A colored square, a name, and a count in one legend entry.
fn legend_swatch(
    label: &'static str,
    count: u32,
    color: gpui_kit::Hsla,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .items_center()
        .gap(rems(0.25))
        .child(div().size(rems(0.5)).rounded(rems(0.125)).bg(color))
        .child(SharedString::from(label))
        .child(
            div()
                .font_family(cx.theme().mono_font_family.clone())
                .text_color(cx.theme().foreground)
                .child(SharedString::from(format_thousands(count))),
        )
}

/// Group thousands so a six-figure token count stays readable.
fn format_thousands(value: u32) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Muted placeholder used by every empty pane.
///
/// Each empty branch carries its own `work-pane-empty-*` id: a pane that
/// silently renders nothing looks exactly like a pane that broke, so the tests
/// read the line back by name.
fn empty(id: &'static str, text: &str, cx: &App) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .aria_label(SharedString::from(text.to_string()))
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

    #[test]
    fn task_filter_and_sort_keep_the_expected_rows() {
        let tasks = vec![
            tact_protocol::TaskSnapshot {
                id: 1,
                subject: "pending".into(),
                status: TaskStatusSnapshot::Pending,
                owner: "zoe".into(),
                created_at: Some(40),
                ..Default::default()
            },
            tact_protocol::TaskSnapshot {
                id: 2,
                subject: "active".into(),
                status: TaskStatusSnapshot::InProgress,
                owner: "amy".into(),
                created_at: Some(30),
                ..Default::default()
            },
            tact_protocol::TaskSnapshot {
                id: 3,
                subject: "done".into(),
                status: TaskStatusSnapshot::Completed,
                owner: "bob".into(),
                created_at: Some(50),
                ..Default::default()
            },
        ];

        let mut pane = TasksPane::default();
        let ids = |pane: &TasksPane| {
            pane.visible(&tasks)
                .into_iter()
                .map(|task| task.id)
                .collect::<Vec<_>>()
        };

        assert_eq!(ids(&pane), vec![1, 2, 3], "status order is the default");
        pane.cycle_filter();
        assert_eq!(ids(&pane), vec![1, 2], "Open hides completed tasks");
        pane.cycle_filter();
        assert_eq!(ids(&pane), vec![3], "Done keeps only completed tasks");
        pane.cycle_filter();
        pane.cycle_sort();
        assert_eq!(ids(&pane), vec![2, 3, 1], "owner order is case-normalized");
        pane.cycle_sort();
        assert_eq!(ids(&pane), vec![3, 1, 2], "newest uses the latest stamp");
    }

    #[test]
    fn the_task_list_reports_its_newest_stamp() {
        let tasks = vec![
            tact_protocol::TaskSnapshot {
                created_at: Some(100),
                ..Default::default()
            },
            tact_protocol::TaskSnapshot {
                created_at: Some(90),
                started_at: Some(140),
                ..Default::default()
            },
            tact_protocol::TaskSnapshot {
                created_at: Some(80),
                completed_at: Some(120),
                ..Default::default()
            },
        ];
        assert_eq!(
            tasks_updated_at(&tasks),
            Some(140),
            "the newest stamp wins regardless of which task carries it"
        );
        assert_eq!(
            tasks_updated_at(&[tact_protocol::TaskSnapshot::default()]),
            None,
            "a snapshot with no stamps reports no age instead of an invented one"
        );
    }

    #[test]
    fn the_tasks_subtitle_keeps_second_precision() {
        // `.panelHead p` in the prototype reads "4 tasks · updated 12s ago",
        // so anything under a minute stays in seconds instead of collapsing
        // to the session list's "just now".
        assert_eq!(updated_age(0), "0s ago");
        assert_eq!(updated_age(12), "12s ago");
        assert_eq!(updated_age(59), "59s ago");
        assert_eq!(updated_age(60), "1m ago");
        assert_eq!(updated_age(7_200), "2h ago");
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
        let mut files = FilesPane::default();

        let rows = files.rows(&root);
        let names: Vec<_> = rows.iter().map(|row| row.name.as_str()).collect();

        assert_eq!(names, ["src", "README.md"]);
        assert!(rows[0].is_dir);
        assert!(!rows[0].expanded);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn files_rows_are_cached_until_the_pane_is_invalidated() {
        // The walk is `read_dir` plus a `stat` per entry, so it must not run
        // once per frame; only an explicit invalidation may notice new files.
        let root = scratch_dir("cache");
        let mut files = FilesPane::default();
        assert_eq!(files.rows(&root).len(), 2);

        std::fs::write(root.join("CHANGELOG.md"), "new\n").unwrap();
        assert_eq!(files.rows(&root).len(), 2, "cached walk re-read the disk");

        files.invalidate();
        assert_eq!(files.rows(&root).len(), 3, "invalidate did not re-walk");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn files_rows_skip_build_output_directories() {
        // `target/` and `node_modules/` are gitignored artifact trees with
        // thousands of entries; expanding one must not enqueue a row each.
        let root = scratch_dir("skips");
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        let mut files = FilesPane::default();
        files.on_toggle(root.join("target"));
        files.on_toggle(root.join("node_modules"));

        let rows = files.rows(&root);
        let names: Vec<_> = rows.iter().map(|row| row.name.as_str()).collect();

        assert_eq!(names, ["src", "README.md"]);

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
    fn files_preview_reads_the_selected_file_and_refreshes_on_invalidate() {
        let root = scratch_dir("preview");
        let path = root.join("src/lib.rs");
        let mut files = FilesPane::default();

        files.select_file(&path);
        {
            let preview = files
                .selected_preview()
                .expect("a selected file has a preview");
            assert_eq!(preview.path, path);
            assert!(preview.error.is_none());
            assert_eq!(preview.lines, vec!["pub fn lib() {}".to_string()]);
            assert!(!preview.truncated);
            assert!(!preview.binary);
        }

        std::fs::write(&path, "pub fn lib() { /* changed */ }\n").unwrap();
        files.invalidate();
        assert_eq!(
            files.selected_preview().expect("preview").lines,
            vec!["pub fn lib() { /* changed */ }".to_string()],
            "invalidation refreshes the selected file"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn files_preview_marks_binary_and_truncated_content() {
        let root = scratch_dir("preview-edges");
        let mut files = FilesPane::default();
        let binary = root.join("binary.bin");
        std::fs::write(&binary, [0, 1, 2, 3]).unwrap();
        files.select_file(&binary);
        assert!(files.selected_preview().expect("preview").binary);

        let large = root.join("large.txt");
        std::fs::write(&large, "x".repeat(FILE_PREVIEW_MAX_BYTES + 1)).unwrap();
        files.select_file(&large);
        let preview = files.selected_preview().expect("preview");
        assert!(preview.truncated);
        assert_eq!(preview.lines.len(), 1);

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

    #[test]
    fn git_diff_keeps_a_staged_change_visible() {
        let root = scratch_dir("gitdiffstaged");
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
        let committed = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args([
                "-c",
                "user.name=Tact Test",
                "-c",
                "user.email=tact@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "initial",
            ])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        if !committed {
            let _ = std::fs::remove_dir_all(root);
            return;
        }

        std::fs::write(&path, "pub fn lib() {}\npub fn staged() {}\n").unwrap();
        let staged = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["add", "--", "src/lib.rs"])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        assert!(staged);

        let unified = git_diff(&root, "src/lib.rs").expect("a staged file has a diff");

        assert!(unified.contains("+pub fn staged() {}"), "{unified}");
        let _ = std::fs::remove_dir_all(root);
    }

    /// The pane records the path the tool call was given, which is sometimes
    /// written from the repository root while the session workspace sits in a
    /// subdirectory. The pathspec then has to anchor at the repository rather
    /// than at the workspace, or the pane silently falls back to the detail.
    #[test]
    fn git_diff_resolves_a_repository_root_path_from_a_subdirectory() {
        let root = scratch_dir("gitdiffsub");
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
        std::fs::write(
            root.join("src/lib.rs"),
            "pub fn lib() {}\npub fn extra() {}\n",
        )
        .unwrap();

        let workdir = root.join("src/nested");
        let unified = git_diff(&workdir, "src/lib.rs").expect("a repository-root path resolves");

        assert!(unified.contains("+pub fn extra() {}"), "{unified}");
        let _ = std::fs::remove_dir_all(root);
    }
}

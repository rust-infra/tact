//! Code diff viewer: a unified patch as reviewable hunks.
//!
//! Agents propose edits; people review them. [`CodeDiff`] shows one file's
//! patch with old/new line gutters, change tints, a copyable source, and,
//! when reviewable, an Accept / Reject pair per hunk that reports
//! [`CodeDiffEvent`]s by file path and hunk index. [`DiffFile::from_unified`]
//! parses standard unified diffs so the application can hand over what a
//! tool already produced.
//!
//! The code itself is rendered through the upstream syntax-highlighted,
//! selectable text view; the gutter and change tints are laid out beside it
//! on the same rem-based line height so numbers and lines stay aligned.

use crate::control::ControlMetricsExt as _;
use crate::decoration::{DecoratedExt as _, Decoration};
use crate::motion::{acknowledged_state, disclosure_progress};
use crate::surface::CardFrameExt as _;
use crate::{
    handlers::SharedHandler,
    status::{StatusBadge, StatusTone},
    surface::{icon_button_turned, meta},
    theme::SemanticStyledExt as _,
};
use gpui::{
    App, ClickEvent, ElementId, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Rems, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    clipboard::Clipboard,
    h_flex,
    text::{TextView, TextViewStyle},
    v_flex,
};
use std::rc::Rc;

/// Line height shared by the gutter and the code so rows stay aligned.
const LINE_HEIGHT: Rems = Rems(1.5);

/// Width of one line-number gutter column: four digits of the mono
/// extra-small size, right-aligned, in the type's own unit so the gutter
/// scales with the code beside it.
const NUMBER_GUTTER_WIDTH: Rems = Rems(2.25);

/// Width of the +/- sign column between the numbers and the code.
const SIGN_GUTTER_WIDTH: Rems = Rems(0.75);

/// Whether a line was kept, added, or removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiffLineKind {
    /// Present on both sides.
    Context,
    /// Present only after the change.
    Added,
    /// Present only before the change.
    Removed,
}

impl DiffLineKind {
    fn sign(self) -> &'static str {
        match self {
            Self::Context => " ",
            Self::Added => "+",
            Self::Removed => "-",
        }
    }
}

/// One line of a hunk with its position on either side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    kind: DiffLineKind,
    old_number: Option<u32>,
    new_number: Option<u32>,
    text: SharedString,
}

impl DiffLine {
    /// Creates a line without positions.
    pub fn new(kind: DiffLineKind, text: impl Into<SharedString>) -> Self {
        Self {
            kind,
            old_number: None,
            new_number: None,
            text: text.into(),
        }
    }

    /// Sets the line number before the change.
    pub fn old_number(mut self, number: u32) -> Self {
        self.old_number = Some(number);
        self
    }

    /// Sets the line number after the change.
    pub fn new_number(mut self, number: u32) -> Self {
        self.new_number = Some(number);
        self
    }

    /// Returns the line kind.
    pub fn kind(&self) -> DiffLineKind {
        self.kind
    }

    /// Returns the line text without its leading sign.
    pub fn text(&self) -> &SharedString {
        &self.text
    }

    /// Returns the line number before the change.
    pub fn old_line(&self) -> Option<u32> {
        self.old_number
    }

    /// Returns the line number after the change.
    pub fn new_line(&self) -> Option<u32> {
        self.new_number
    }
}

/// The review decision recorded for a hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum HunkReview {
    /// Not yet decided.
    #[default]
    Pending,
    /// The change was accepted.
    Accepted,
    /// The change was rejected.
    Rejected,
}

/// A contiguous group of changed lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    header: SharedString,
    lines: Vec<DiffLine>,
    review: HunkReview,
}

impl DiffHunk {
    /// Creates a hunk with its `@@ … @@` header (or any short caption).
    pub fn new(header: impl Into<SharedString>) -> Self {
        Self {
            header: header.into(),
            lines: Vec::new(),
            review: HunkReview::Pending,
        }
    }

    /// Sets the lines, in order.
    pub fn lines(mut self, lines: impl IntoIterator<Item = DiffLine>) -> Self {
        self.lines = lines.into_iter().collect();
        self
    }

    /// Records the review decision.
    pub fn review(mut self, review: HunkReview) -> Self {
        self.review = review;
        self
    }

    /// Returns the hunk header.
    pub fn header(&self) -> &SharedString {
        &self.header
    }

    /// Returns the lines.
    pub fn line_refs(&self) -> &[DiffLine] {
        &self.lines
    }

    /// Returns the review decision.
    pub fn review_state(&self) -> HunkReview {
        self.review
    }

    /// Counts added and removed lines.
    pub fn stats(&self) -> DiffStats {
        let mut stats = DiffStats::default();
        for line in &self.lines {
            match line.kind {
                DiffLineKind::Added => stats.added += 1,
                DiffLineKind::Removed => stats.removed += 1,
                DiffLineKind::Context => {}
            }
        }
        stats
    }

    fn code(&self) -> String {
        let mut code = String::new();
        for line in &self.lines {
            code.push_str(&line.text);
            code.push('\n');
        }
        code
    }

    fn patch_text(&self) -> String {
        let mut patch = String::new();
        patch.push_str(&self.header);
        patch.push('\n');
        for line in &self.lines {
            patch.push_str(line.kind.sign());
            patch.push_str(&line.text);
            patch.push('\n');
        }
        patch
    }
}

/// Added and removed line counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DiffStats {
    /// Lines added.
    pub added: usize,
    /// Lines removed.
    pub removed: usize,
}

impl DiffStats {
    /// The conventional `+a −r` summary.
    pub fn label(self) -> String {
        format!("+{} \u{2212}{}", self.added, self.removed)
    }
}

/// One file's patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFile {
    path: SharedString,
    old_path: Option<SharedString>,
    language: Option<SharedString>,
    hunks: Vec<DiffHunk>,
}

impl DiffFile {
    /// Creates a patch for `path`; the language is inferred from its extension.
    pub fn new(path: impl Into<SharedString>) -> Self {
        let path = path.into();
        Self {
            language: language_for(&path).map(SharedString::from),
            path,
            old_path: None,
            hunks: Vec::new(),
        }
    }

    /// Records the path before a rename.
    pub fn renamed_from(mut self, old_path: impl Into<SharedString>) -> Self {
        self.old_path = Some(old_path.into());
        self
    }

    /// Overrides the highlighting language.
    pub fn language(mut self, language: impl Into<SharedString>) -> Self {
        self.language = Some(language.into());
        self
    }

    /// Sets the hunks, in order.
    pub fn hunks(mut self, hunks: impl IntoIterator<Item = DiffHunk>) -> Self {
        self.hunks = hunks.into_iter().collect();
        self
    }

    /// Returns the file path after the change.
    pub fn path(&self) -> &SharedString {
        &self.path
    }

    /// Returns the path before a rename, if any.
    pub fn old_path(&self) -> Option<&SharedString> {
        self.old_path.as_ref()
    }

    /// Returns the highlighting language, if known.
    pub fn language_name(&self) -> Option<&SharedString> {
        self.language.as_ref()
    }

    /// Returns the hunks.
    pub fn hunk_refs(&self) -> &[DiffHunk] {
        &self.hunks
    }

    /// Sums the hunk statistics.
    pub fn stats(&self) -> DiffStats {
        self.hunks.iter().fold(DiffStats::default(), |acc, hunk| {
            let stats = hunk.stats();
            DiffStats {
                added: acc.added + stats.added,
                removed: acc.removed + stats.removed,
            }
        })
    }

    /// The accessible name: path and statistics.
    pub fn accessibility_label(&self) -> String {
        format!("Diff of {}, {}", self.path, self.stats().label())
    }

    /// Rebuilds a unified patch for this file (what the copy button exports).
    pub fn to_unified(&self) -> String {
        let old = self.old_path.as_ref().unwrap_or(&self.path);
        let mut patch = format!("--- a/{old}\n+++ b/{}\n", self.path);
        for hunk in &self.hunks {
            patch.push_str(&hunk.patch_text());
        }
        patch
    }

    /// Parses a unified diff (as produced by `git diff` or `diff -u`) into
    /// one file per `---`/`+++` pair. Unknown lines are ignored so partial
    /// tool output still renders.
    pub fn from_unified(patch: &str) -> Vec<DiffFile> {
        let mut files: Vec<DiffFile> = Vec::new();
        let mut pending_old: Option<String> = None;
        let mut old_line = 0u32;
        let mut new_line = 0u32;

        for raw in patch.lines() {
            if let Some(rest) = raw.strip_prefix("--- ") {
                pending_old = Some(strip_prefix_path(rest));
                continue;
            }
            if let Some(rest) = raw.strip_prefix("+++ ") {
                let path = strip_prefix_path(rest);
                let mut file = DiffFile::new(path.clone());
                if let Some(old) = pending_old.take()
                    && old != path
                    && old != "/dev/null"
                {
                    file = file.renamed_from(old);
                }
                files.push(file);
                continue;
            }
            if let Some(rest) = raw.strip_prefix("@@") {
                let Some(file) = files.last_mut() else {
                    continue;
                };
                let (old_start, new_start) = parse_hunk_ranges(rest);
                old_line = old_start;
                new_line = new_start;
                file.hunks.push(DiffHunk::new(raw.to_owned()));
                continue;
            }
            let Some(hunk) = files.last_mut().and_then(|file| file.hunks.last_mut()) else {
                continue;
            };
            let (kind, text) = match raw.chars().next() {
                Some('+') => (DiffLineKind::Added, &raw[1..]),
                Some('-') => (DiffLineKind::Removed, &raw[1..]),
                Some(' ') => (DiffLineKind::Context, &raw[1..]),
                Some('\\') => (DiffLineKind::Context, raw),
                None => (DiffLineKind::Context, ""),
                _ => continue,
            };
            let mut line = DiffLine::new(kind, text.to_owned());
            match kind {
                DiffLineKind::Context => {
                    line = line.old_number(old_line).new_number(new_line);
                    old_line += 1;
                    new_line += 1;
                }
                DiffLineKind::Added => {
                    line = line.new_number(new_line);
                    new_line += 1;
                }
                DiffLineKind::Removed => {
                    line = line.old_number(old_line);
                    old_line += 1;
                }
            }
            hunk.lines.push(line);
        }
        files
    }
}

fn strip_prefix_path(rest: &str) -> String {
    let path = rest.split('\t').next().unwrap_or(rest).trim();
    path.strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path)
        .to_owned()
}

fn parse_hunk_ranges(rest: &str) -> (u32, u32) {
    let mut old_start = 1;
    let mut new_start = 1;
    for token in rest.split_whitespace().take(2) {
        let (sign, range) = token.split_at(1);
        let start = range
            .split(',')
            .next()
            .and_then(|start| start.parse::<u32>().ok())
            .unwrap_or(1);
        match sign {
            "-" => old_start = start,
            "+" => new_start = start,
            _ => {}
        }
    }
    (old_start, new_start)
}

fn language_for(path: &str) -> Option<&'static str> {
    let extension = path.rsplit_once('.').map(|(_, ext)| ext)?;
    Some(match extension {
        "rs" => "rust",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" | "mjs" => "javascript",
        "py" => "python",
        "go" => "go",
        "toml" => "toml",
        "json" => "json",
        "md" => "markdown",
        "sh" | "zsh" | "bash" => "bash",
        "css" => "css",
        "html" => "html",
        "yaml" | "yml" => "yaml",
        "sql" => "sql",
        "c" | "h" => "c",
        "cpp" | "hpp" | "cc" => "cpp",
        "java" => "java",
        "kt" => "kotlin",
        "swift" => "swift",
        "rb" => "ruby",
        _ => return None,
    })
}

/// An interaction emitted by [`CodeDiff`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeDiffEvent {
    /// The file header was toggled open or closed.
    Toggled {
        /// File path after the change.
        path: SharedString,
    },
    /// A hunk was accepted.
    HunkAccepted {
        /// File path after the change.
        path: SharedString,
        /// Zero-based hunk index.
        hunk: usize,
    },
    /// A hunk was rejected.
    HunkRejected {
        /// File path after the change.
        path: SharedString,
        /// Zero-based hunk index.
        hunk: usize,
    },
}

/// One file's patch as a card: header with path and statistics, then each
/// hunk with gutters, tints, highlighted selectable code, and optional
/// review controls.
///
/// # Example
///
/// ```no_run
/// # use gpui_ai::prelude::*;
/// # fn example(patch: &str) {
/// let file = DiffFile::from_unified(patch).remove(0);
/// CodeDiff::new("patch", &file)
///     .reviewable(true)
///     .on_event(|event, _, _| { /* CodeDiffEvent::HunkAccepted { path, hunk } */ });
/// # }
/// ```
#[derive(IntoElement)]
pub struct CodeDiff {
    id: ElementId,
    style: StyleRefinement,
    decoration: Decoration,
    file: DiffFile,
    open: bool,
    reviewable: bool,
    on_event: Option<SharedHandler<CodeDiffEvent>>,
}

impl CodeDiff {
    /// Layers painted into this diff's frame: one under the content, one
    /// over it, both clipped to its own shape and neither affecting layout.
    ///
    /// This crate ships no effects of its own — what goes in a decoration is
    /// the application's expression.
    pub fn decoration(mut self, decoration: Decoration) -> Self {
        self.decoration = decoration;
        self
    }

    /// Creates a viewer for one file, open by default.
    pub fn new(id: impl Into<ElementId>, file: &DiffFile) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            decoration: Decoration::default(),
            file: file.clone(),
            open: true,
            reviewable: false,
            on_event: None,
        }
    }

    /// Shows or hides the hunks; the header toggle reports
    /// [`CodeDiffEvent::Toggled`] so the application can flip this.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Shows Accept / Reject for every pending hunk.
    pub fn reviewable(mut self, reviewable: bool) -> Self {
        self.reviewable = reviewable;
        self
    }

    /// Handles typed interactions. Without a handler the viewer is static.
    pub fn on_event(
        mut self,
        handler: impl Fn(&CodeDiffEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }
}

impl Styled for CodeDiff {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for CodeDiff {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        // Taken once: each layer is placed at most once, and a
        // component with no decoration adds no elements at all.
        let mut decoration = std::mem::take(&mut self.decoration);
        let file = self.file;
        let handler = self.on_event;
        let path = file.path.clone();
        let path_debug = path.clone();
        let stats = file.stats();
        let label: SharedString = file.accessibility_label().into();
        let language: SharedString = file
            .language
            .clone()
            .unwrap_or_else(|| SharedString::from("text"));
        let root_id = self.id.clone();
        let disclosure =
            disclosure_progress((root_id.clone(), "disclosure"), self.open, window, cx);
        let disclosure_opacity =
            crate::motion::disclosure_fade((root_id.clone(), "disclosure"), self.open, window, cx);
        let showing = self.open;

        let toggle = handler.clone().map(|handler| {
            let toggle_path = path.clone();
            let toggle_debug = path.clone();
            // The same rotating chevron every other disclosure in the
            // library uses, turned by the same sample the body opens on.
            // Swapping between two icons could only ever be fully open or
            // fully closed, and snapped while the panel was still moving.
            icon_button_turned(
                (root_id.clone(), "toggle"),
                IconName::ChevronRight,
                0.25 * disclosure,
                if self.open {
                    "Collapse diff"
                } else {
                    "Expand diff"
                },
                window,
                cx,
            )
            .debug_selector(move || format!("code-diff-toggle-{toggle_debug}"))
            .on_click(move |_: &ClickEvent, window, cx| {
                handler(
                    &CodeDiffEvent::Toggled {
                        path: toggle_path.clone(),
                    },
                    window,
                    cx,
                )
            })
        });

        let header = h_flex()
            .items_center()
            .gap(tokens.spacing.sm)
            .px(tokens.spacing.md)
            .py(tokens.spacing.xs)
            .bg(cx.theme().muted.opacity(0.35))
            .when(showing, |this| {
                this.border_b_1().border_color(cx.theme().border)
            })
            .children(toggle)
            .child(
                Icon::new(IconName::File)
                    .xsmall()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_token(tokens.typography.sm)
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_color(cx.theme().foreground)
                    .child(path.clone()),
            )
            .when_some(file.old_path.clone(), |this, old_path| {
                this.child(meta(format!("renamed from {old_path}"), cx))
            })
            .child(div().flex_1())
            .child(
                h_flex()
                    .flex_none()
                    .gap(tokens.spacing.xxs)
                    .text_token(tokens.typography.xs)
                    .font_family(cx.theme().mono_font_family.clone())
                    .child(
                        div()
                            .text_color(cx.theme().success)
                            .child(format!("+{}", stats.added)),
                    )
                    .child(
                        div()
                            .text_color(cx.theme().danger)
                            .child(format!("\u{2212}{}", stats.removed)),
                    ),
            )
            .child(Clipboard::new((root_id.clone(), "copy")).value(file.to_unified()));

        let mut hunks = Vec::with_capacity(file.hunks.len());
        if showing {
            for (index, hunk) in file.hunks.iter().enumerate() {
                hunks.push(render_hunk(
                    &root_id,
                    &path,
                    index,
                    hunk,
                    &language,
                    self.reviewable,
                    handler.clone(),
                    window,
                    cx,
                ));
            }
        }

        v_flex()
            .id(self.id)
            .role(Role::Group)
            .aria_label(label)
            .debug_selector(move || format!("code-diff-{path_debug}"))
            .w_full()
            .min_w_0()
            .card_frame(cx)
            .decoration_under(&mut decoration)
            .overflow_hidden()
            .child(header)
            .when(showing, |this| {
                // The body is selectable only while the disclosure is open.
                this.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .opacity(disclosure_opacity)
                        .top(tokens.spacing.xxs * (1.0 - disclosure) * crate::motion::travel(cx))
                        .children(hunks),
                )
            })
            .decoration_over(&mut decoration)
            .refine_style(&self.style)
    }
}

#[allow(clippy::too_many_arguments)]
fn render_hunk(
    root_id: &ElementId,
    path: &SharedString,
    index: usize,
    hunk: &DiffHunk,
    language: &SharedString,
    reviewable: bool,
    handler: Option<SharedHandler<CodeDiffEvent>>,
    window: &mut Window,
    cx: &mut App,
) -> gpui::AnyElement {
    let tokens = cx.theme().semantic_tokens();
    let hunk_id = ElementId::from((root_id.clone(), format!("hunk-{index}")));
    let hunk_debug = format!("{path}-{index}");
    let gutter_debug = hunk_debug.clone();
    let text_debug = hunk_debug.clone();
    let stats = hunk.stats();
    let hunk_label: SharedString =
        format!("Hunk {} of {}: {}", index + 1, hunk.header, stats.label()).into();

    // Resolution settles in once, after the controlled review changes; a
    // diff that mounts already reviewed shows its badges at rest, and the
    // badge's own fixed-slot swap covers a later accepted-to-rejected
    // change.
    let resolved = acknowledged_state(
        ElementId::from((hunk_id.clone(), "resolved")),
        hunk.review as u64,
        window,
        cx,
    );
    let review = match (hunk.review, reviewable, handler) {
        (HunkReview::Accepted, _, _) => Some(
            div()
                .opacity(resolved)
                .child(
                    StatusBadge::new((hunk_id.clone(), "review"), "Accepted")
                        .tone(StatusTone::Success),
                )
                .into_any_element(),
        ),
        (HunkReview::Rejected, _, _) => Some(
            div()
                .opacity(resolved)
                .child(
                    StatusBadge::new((hunk_id.clone(), "review"), "Rejected")
                        .tone(StatusTone::Neutral),
                )
                .into_any_element(),
        ),
        (HunkReview::Pending, true, Some(handler)) => {
            let accept_path = path.clone();
            let reject_path = path.clone();
            let accept_handler = handler.clone();
            let accept_debug = hunk_debug.clone();
            let reject_debug = hunk_debug.clone();
            Some(
                h_flex()
                    .flex_none()
                    .items_center()
                    .gap(tokens.spacing.xs)
                    .child(
                        div()
                            .debug_selector(move || format!("code-diff-reject-{reject_debug}"))
                            .child(
                                Button::new((hunk_id.clone(), "reject"))
                                    .outline()
                                    .xsmall()
                                    .control_metrics(cx)
                                    .accessibility_id(format!("{path}-reject-{index}"))
                                    .label("Reject")
                                    .on_click(move |_: &ClickEvent, window, cx| {
                                        handler(
                                            &CodeDiffEvent::HunkRejected {
                                                path: reject_path.clone(),
                                                hunk: index,
                                            },
                                            window,
                                            cx,
                                        )
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .debug_selector(move || format!("code-diff-accept-{accept_debug}"))
                            .child(
                                Button::new((hunk_id.clone(), "accept"))
                                    .primary()
                                    .xsmall()
                                    .control_metrics(cx)
                                    .accessibility_id(format!("{path}-accept-{index}"))
                                    .label("Accept")
                                    .on_click(move |_: &ClickEvent, window, cx| {
                                        accept_handler(
                                            &CodeDiffEvent::HunkAccepted {
                                                path: accept_path.clone(),
                                                hunk: index,
                                            },
                                            window,
                                            cx,
                                        )
                                    }),
                            ),
                    )
                    .into_any_element(),
            )
        }
        _ => None,
    };

    let header = h_flex()
        .items_center()
        .gap(tokens.spacing.sm)
        .px(tokens.spacing.md)
        .py(tokens.spacing.xxs)
        .bg(cx.theme().accent.opacity(0.35))
        .border_t_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_token(tokens.typography.xs)
                .font_family(cx.theme().mono_font_family.clone())
                .text_color(cx.theme().muted_foreground)
                .child(hunk.header.clone()),
        )
        .child(div().flex_1())
        .children(review);

    // Tint rows and number gutters share the code's line height; the text
    // view's own code-block chrome is stripped so its first line starts at
    // the same y as the first gutter row.
    let tints = v_flex()
        .absolute()
        .inset_0()
        .children(hunk.lines.iter().map(|line| {
            div().w_full().h(LINE_HEIGHT).when_some(
                match line.kind {
                    DiffLineKind::Added => Some(cx.theme().success.opacity(0.12)),
                    DiffLineKind::Removed => Some(cx.theme().danger.opacity(0.12)),
                    DiffLineKind::Context => None,
                },
                |this, color| this.bg(color),
            )
        }));
    let gutter = v_flex()
        .flex_none()
        .debug_selector(move || format!("code-diff-gutter-{gutter_debug}"))
        .px(tokens.spacing.sm)
        .text_token(tokens.typography.xs)
        .font_family(cx.theme().mono_font_family.clone())
        .text_color(cx.theme().muted_foreground)
        .children(hunk.lines.iter().map(|line| {
            let sign_color = match line.kind {
                DiffLineKind::Added => cx.theme().success,
                DiffLineKind::Removed => cx.theme().danger,
                DiffLineKind::Context => cx.theme().muted_foreground,
            };
            h_flex()
                .h(LINE_HEIGHT)
                .items_center()
                .gap(tokens.spacing.xs)
                .child(
                    div()
                        .w(NUMBER_GUTTER_WIDTH)
                        .text_right()
                        .child(line.old_number.map_or(String::new(), |n| n.to_string())),
                )
                .child(
                    div()
                        .w(NUMBER_GUTTER_WIDTH)
                        .text_right()
                        .child(line.new_number.map_or(String::new(), |n| n.to_string())),
                )
                .child(
                    div()
                        .w(SIGN_GUTTER_WIDTH)
                        .text_center()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(sign_color)
                        .child(line.kind.sign().trim().to_owned()),
                )
        }));
    let fence = "`".repeat(
        (hunk
            .lines
            .iter()
            .flat_map(|line| line.text.split(|c| c != '`'))
            .map(str::len)
            .max()
            .unwrap_or(0)
            + 1)
        .max(3),
    );
    let source = format!("{fence}{language}\n{}{fence}", hunk.code());
    let code_style = StyleRefinement::default()
        .p_0()
        .m_0()
        .rounded(gpui::Pixels::ZERO)
        .bg(cx.theme().transparent)
        .line_height(LINE_HEIGHT);
    let text = div()
        .id((hunk_id.clone(), "scroll"))
        .debug_selector(move || format!("code-diff-text-{text_debug}"))
        .flex_1()
        .min_w_0()
        .overflow_x_scroll()
        .whitespace_nowrap()
        .line_height(LINE_HEIGHT)
        .pr(tokens.spacing.md)
        .child(
            TextView::markdown((hunk_id.clone(), "code"), source)
                .style(TextViewStyle::default().code_block(code_style))
                .selectable(true),
        );

    v_flex()
        .id(hunk_id)
        .role(Role::Group)
        .aria_label(hunk_label)
        .debug_selector(move || format!("code-diff-hunk-{hunk_debug}"))
        .w_full()
        .min_w_0()
        .child(header)
        .child(
            h_flex()
                .relative()
                .w_full()
                .min_w_0()
                .items_start()
                .line_height(LINE_HEIGHT)
                .child(tints)
                .child(gutter)
                .child(text),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "diff --git a/src/pricing.rs b/src/pricing.rs\n--- a/src/pricing.rs\n+++ b/src/pricing.rs\n@@ -1,4 +1,5 @@ fn unit_price\n fn unit_price(order: &Order) -> Money {\n-    order.total / order.units\n+    let units = order.units.max(1);\n+    order.total / units\n }\n \n@@ -10,3 +11,3 @@\n fn discount() -> f32 {\n-    0.05\n+    0.07\n }\n";

    #[test]
    fn unified_patches_parse_into_numbered_hunks() {
        let files = DiffFile::from_unified(PATCH);
        assert_eq!(files.len(), 1);
        let file = &files[0];
        assert_eq!(file.path().as_ref(), "src/pricing.rs");
        assert_eq!(file.language_name().map(|l| l.as_ref()), Some("rust"));
        assert_eq!(file.hunk_refs().len(), 2);
        assert_eq!(
            file.stats(),
            DiffStats {
                added: 3,
                removed: 2
            }
        );
        assert_eq!(
            file.accessibility_label(),
            "Diff of src/pricing.rs, +3 \u{2212}2"
        );

        let first = &file.hunk_refs()[0];
        assert_eq!(first.header().as_ref(), "@@ -1,4 +1,5 @@ fn unit_price");
        let lines = first.line_refs();
        assert_eq!(lines[0].kind(), DiffLineKind::Context);
        assert_eq!(
            (lines[0].old_line(), lines[0].new_line()),
            (Some(1), Some(1))
        );
        assert_eq!(lines[1].kind(), DiffLineKind::Removed);
        assert_eq!((lines[1].old_line(), lines[1].new_line()), (Some(2), None));
        assert_eq!(lines[2].kind(), DiffLineKind::Added);
        assert_eq!((lines[2].old_line(), lines[2].new_line()), (None, Some(2)));
        assert_eq!(lines[3].text().as_ref(), "    order.total / units");
        assert_eq!(
            (lines[4].old_line(), lines[4].new_line()),
            (Some(3), Some(4))
        );

        let second = &file.hunk_refs()[1];
        assert_eq!(second.line_refs()[0].old_line(), Some(10));
        assert_eq!(second.line_refs()[0].new_line(), Some(11));
    }

    #[test]
    fn round_trips_to_a_unified_patch() {
        let file = DiffFile::from_unified(PATCH).remove(0);
        let unified = file.to_unified();
        assert!(unified.starts_with(
            "--- a/src/pricing.rs\n+++ b/src/pricing.rs\n@@ -1,4 +1,5 @@ fn unit_price\n"
        ));
        assert!(
            unified
                .contains("-    order.total / order.units\n+    let units = order.units.max(1);\n")
        );
        let again = DiffFile::from_unified(&unified).remove(0);
        assert_eq!(again, file);
    }

    #[test]
    fn renames_and_unknown_lines_are_tolerated() {
        let files = DiffFile::from_unified(
            "--- a/old.md\n+++ b/new.md\nindex 1..2\n@@ -1 +1 @@\n-old\n+new\n",
        );
        assert_eq!(files[0].old_path().map(|p| p.as_ref()), Some("old.md"));
        assert_eq!(files[0].path().as_ref(), "new.md");
        assert_eq!(files[0].hunk_refs()[0].line_refs().len(), 2);
        assert!(DiffFile::from_unified("not a diff").is_empty());
    }
}

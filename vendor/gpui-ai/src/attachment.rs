//! Attachment previews: the files a person adds to a prompt and the files a
//! message carries.
//!
//! One [`Attachment`] value describes a file by stable ID, name, kind, size,
//! optional thumbnail, and upload lifecycle. [`AttachmentPreview`] renders it
//! as a tile (thumbnail or kind glyph, name, metadata, state) and
//! [`AttachmentStrip`] lays tiles out in a wrapping row that ripples in.
//! The same strip serves both sides of a conversation: removable and compact
//! inside the composer, read-only and openable inside a message.
//!
//! Applications own the bytes, the upload, and what "open" means; the
//! components only report [`AttachmentEvent`]s keyed by attachment ID.

use crate::{
    control::{PressReleaseExt as _, composed_button},
    handlers::SharedHandler,
    motion::{ArrivalRoster, MotionTokens, Shimmer},
    stream::ProgressState,
    surface::icon_button,
    theme::SemanticStyledExt as _,
};
use gpui::{
    App, ClickEvent, ElementId, FontWeight, Image, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled, StyledImage as _, Window, div, img, prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, spinner::Spinner,
};
use std::{fmt, rc::Rc, sync::Arc};

/// The family a file belongs to; drives the fallback glyph and the kind label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AttachmentKind {
    /// Raster or vector images.
    Image,
    /// Documents and plain text.
    Document,
    /// Source code and scripts.
    Code,
    /// Audio recordings.
    Audio,
    /// Video clips.
    Video,
    /// Compressed bundles.
    Archive,
    /// Tabular or structured data.
    Data,
    /// Anything else.
    #[default]
    Other,
}

impl AttachmentKind {
    /// Infers the kind from a file name's extension.
    pub fn from_name(name: &str) -> Self {
        let extension = name
            .rsplit_once('.')
            .map(|(_, extension)| extension.to_ascii_lowercase())
            .unwrap_or_default();
        match extension.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "heic" | "avif" => {
                Self::Image
            }
            "md" | "txt" | "pdf" | "doc" | "docx" | "rtf" | "odt" | "pages" => Self::Document,
            "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "rb" | "java" | "kt" | "swift"
            | "c" | "h" | "cpp" | "hpp" | "cs" | "sh" | "zsh" | "toml" | "yaml" | "yml"
            | "html" | "css" | "sql" => Self::Code,
            "mp3" | "wav" | "m4a" | "ogg" | "flac" | "aac" => Self::Audio,
            "mp4" | "mov" | "webm" | "mkv" | "avi" => Self::Video,
            "zip" | "tar" | "gz" | "tgz" | "7z" | "rar" | "bz2" | "xz" => Self::Archive,
            "csv" | "tsv" | "json" | "xlsx" | "xls" | "parquet" | "ndjson" => Self::Data,
            _ => Self::Other,
        }
    }

    /// The short human label for this family.
    pub fn label(self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::Document => "Document",
            Self::Code => "Code",
            Self::Audio => "Audio",
            Self::Video => "Video",
            Self::Archive => "Archive",
            Self::Data => "Data",
            Self::Other => "File",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::Image => IconName::GalleryVerticalEnd,
            Self::Document => IconName::File,
            Self::Code => IconName::SquareTerminal,
            Self::Audio => IconName::Play,
            Self::Video => IconName::Frame,
            Self::Archive => IconName::Folder,
            Self::Data => IconName::ChartPie,
            Self::Other => IconName::File,
        }
    }
}

/// One file with stable identity, presentation metadata, and upload state.
///
/// Equality compares every field; thumbnails compare by identity so a
/// snapshot that re-attaches the same image does not invalidate rows.
#[derive(Clone)]
pub struct Attachment {
    id: SharedString,
    name: SharedString,
    kind: AttachmentKind,
    size_bytes: Option<u64>,
    detail: Option<SharedString>,
    thumbnail: Option<Arc<Image>>,
    state: ProgressState,
    progress: Option<f32>,
}

impl Attachment {
    /// Creates a ready attachment; the kind is inferred from the name.
    pub fn new(id: impl Into<SharedString>, name: impl Into<SharedString>) -> Self {
        let name = name.into();
        Self {
            id: id.into(),
            kind: AttachmentKind::from_name(&name),
            name,
            size_bytes: None,
            detail: None,
            thumbnail: None,
            state: ProgressState::Complete,
            progress: None,
        }
    }

    /// Overrides the inferred kind.
    pub fn kind(mut self, kind: AttachmentKind) -> Self {
        self.kind = kind;
        self
    }

    /// Sets the size shown in the metadata line.
    pub fn size_bytes(mut self, size_bytes: u64) -> Self {
        self.size_bytes = Some(size_bytes);
        self
    }

    /// Adds a short detail such as `12 pages` or `1280×720`.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Shows an image thumbnail instead of the kind glyph.
    pub fn thumbnail(mut self, image: Arc<Image>) -> Self {
        self.thumbnail = Some(image);
        self
    }

    /// Sets the upload lifecycle: pending or running files are not yet
    /// usable, failed ones carry a reason.
    pub fn state(mut self, state: ProgressState) -> Self {
        self.state = state;
        self
    }

    /// Sets the upload fraction (`0.0..=1.0`) shown while running.
    pub fn progress(mut self, fraction: f32) -> Self {
        self.progress = Some(fraction.clamp(0.0, 1.0));
        self
    }

    /// Returns the stable attachment identifier.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// Returns the visible file name.
    pub fn name(&self) -> &SharedString {
        &self.name
    }

    /// Returns the kind used for the glyph and label.
    pub fn attachment_kind(&self) -> AttachmentKind {
        self.kind
    }

    /// Returns the size, if known.
    pub fn size(&self) -> Option<u64> {
        self.size_bytes
    }

    /// Returns the optional detail text.
    pub fn detail_text(&self) -> Option<&SharedString> {
        self.detail.as_ref()
    }

    /// Returns the thumbnail, if any.
    pub fn thumbnail_image(&self) -> Option<&Arc<Image>> {
        self.thumbnail.as_ref()
    }

    /// Returns the upload lifecycle.
    pub fn upload_state(&self) -> &ProgressState {
        &self.state
    }

    /// Returns whether the file is usable (uploaded, not failed).
    pub fn is_ready(&self) -> bool {
        self.state == ProgressState::Complete
    }

    /// The one-line metadata shown under the name and read to assistive
    /// technology: kind, size, detail, or the lifecycle when not ready.
    pub fn summary(&self) -> String {
        match &self.state {
            ProgressState::Pending => "Queued".to_owned(),
            ProgressState::Running => match self.progress {
                Some(fraction) => format!("Uploading {}%", (fraction * 100.0).round() as u32),
                None => "Uploading".to_owned(),
            },
            ProgressState::Failed(reason) => format!("Failed: {reason}"),
            ProgressState::Complete => {
                let mut parts = vec![self.kind.label().to_owned()];
                if let Some(size) = self.size_bytes {
                    parts.push(format_bytes(size));
                }
                if let Some(detail) = &self.detail {
                    parts.push(detail.to_string());
                }
                parts.join(" · ")
            }
        }
    }

    /// The accessible name: file name plus summary.
    pub fn accessibility_label(&self) -> String {
        format!("{}, {}", self.name, self.summary())
    }
}

impl PartialEq for Attachment {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.name == other.name
            && self.kind == other.kind
            && self.size_bytes == other.size_bytes
            && self.detail == other.detail
            && self.state == other.state
            && self.progress == other.progress
            && match (&self.thumbnail, &other.thumbnail) {
                (Some(mine), Some(theirs)) => Arc::ptr_eq(mine, theirs),
                (None, None) => true,
                _ => false,
            }
    }
}

impl Eq for Attachment {}

impl fmt::Debug for Attachment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Attachment")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("size_bytes", &self.size_bytes)
            .field("detail", &self.detail)
            .field("thumbnail", &self.thumbnail.as_ref().map(|_| "<image>"))
            .field("state", &self.state)
            .field("progress", &self.progress)
            .finish()
    }
}

/// Formats a byte count the way file managers do (`820 B`, `12 KB`, `1.4 MB`).
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if value >= 10.0 {
        format!("{} {}", value.round() as u64, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// An interaction emitted by attachment previews.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentEvent {
    /// The user activated a read-only attachment (open, preview, download).
    Opened {
        /// Stable attachment identifier.
        id: SharedString,
    },
    /// The user removed an attachment from a composer.
    Removed {
        /// Stable attachment identifier.
        id: SharedString,
    },
}

/// One attachment tile.
///
/// Read-only tiles become buttons that report [`AttachmentEvent::Opened`]
/// when a handler is present; removable tiles expose a named remove button
/// that reports [`AttachmentEvent::Removed`].
///
/// # Example
///
/// ```no_run
/// # use gpui_ai::prelude::*;
/// # fn example(attachment: Attachment) {
/// AttachmentPreview::new("pricing", &attachment)
///     .removable(true)
///     .on_event(|event, _, _| { /* AttachmentEvent::Removed { id } */ });
/// # }
/// ```
#[derive(IntoElement)]
pub struct AttachmentPreview {
    id: ElementId,
    style: StyleRefinement,
    attachment: Attachment,
    removable: bool,
    compact: bool,
    on_event: Option<SharedHandler<AttachmentEvent>>,
}

impl AttachmentPreview {
    /// Creates a tile for one attachment.
    pub fn new(id: impl Into<ElementId>, attachment: &Attachment) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            attachment: attachment.clone(),
            removable: false,
            compact: false,
            on_event: None,
        }
    }

    /// Shows a remove control instead of making the tile openable.
    pub fn removable(mut self, removable: bool) -> Self {
        self.removable = removable;
        self
    }

    /// Uses the single-line chip layout that suits a composer.
    pub fn compact(mut self, compact: bool) -> Self {
        self.compact = compact;
        self
    }

    /// Handles typed interactions. Without a handler the tile is static.
    pub fn on_event(
        mut self,
        handler: impl Fn(&AttachmentEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }

    fn on_shared_event(mut self, handler: SharedHandler<AttachmentEvent>) -> Self {
        self.on_event = Some(handler);
        self
    }
}

impl Styled for AttachmentPreview {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for AttachmentPreview {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        let attachment = self.attachment;
        let debug_id = attachment.id.to_string();
        let label = attachment.accessibility_label();
        let summary = attachment.summary();
        let failed = matches!(attachment.state, ProgressState::Failed(_));
        let running = attachment.state == ProgressState::Running;
        // The compact tile seats its glyph in the medium slot; the full
        // tile doubles it for a thumbnail. Derived from the size policy so
        // a resized slot scale moves the tiles with every other glyph box.
        let slot = crate::sizing::slot_md(cx);
        let leading_size = if self.compact { slot } else { slot * 2. };

        // Thumbnail when we have one; otherwise the kind glyph on a quiet tint.
        let leading = match (&attachment.thumbnail, running) {
            (Some(image), _) => img(image.clone())
                .flex_none()
                .size(leading_size)
                .rounded(tokens.radius.sm)
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            (None, true) if self.compact => Spinner::new().xsmall().into_any_element(),
            (None, _) => div()
                .flex_none()
                .size(leading_size)
                .flex()
                .items_center()
                .justify_center()
                .rounded(tokens.radius.sm)
                .bg(if failed {
                    cx.theme().danger.opacity(0.12)
                } else {
                    cx.theme().primary.opacity(0.1)
                })
                .text_color(if failed {
                    cx.theme().danger
                } else {
                    cx.theme().primary
                })
                .child(if running {
                    Spinner::new().xsmall().into_any_element()
                } else {
                    Icon::new(attachment.kind.icon())
                        .xsmall()
                        .into_any_element()
                })
                .into_any_element(),
        };

        let name = div()
            .min_w_0()
            .truncate()
            .text_token(tokens.typography.sm)
            .font_weight(FontWeight::MEDIUM)
            .text_color(cx.theme().foreground)
            .child(attachment.name.clone());
        let meta_text = if running {
            Shimmer::new((self.id.clone(), "summary"), summary.clone()).into_any_element()
        } else {
            div().truncate().child(summary.clone()).into_any_element()
        };
        let meta = div()
            .min_w_0()
            .text_token(tokens.typography.xs)
            .text_color(if failed {
                cx.theme().danger
            } else {
                cx.theme().muted_foreground
            })
            .child(meta_text);

        let body = if self.compact {
            h_flex()
                .min_w_0()
                .items_baseline()
                .gap(tokens.spacing.xs)
                .child(name)
                .when(!attachment.is_ready(), |this| this.child(meta))
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .gap(tokens.spacing.xxs)
                .child(name)
                .child(meta)
                .into_any_element()
        };

        let compact = self.compact;
        let style = self.style;
        match (self.removable, self.on_event.clone()) {
            (true, handler) => {
                let remove_id = attachment.id.clone();
                let remove_debug_id = debug_id.clone();
                tile_frame(div().id(self.id.clone()), compact, failed, cx)
                    .role(Role::Group)
                    .aria_label(label.clone())
                    .debug_selector(move || format!("attachment-{debug_id}"))
                    .child(leading)
                    .child(body)
                    .when_some(handler, |this, handler| {
                        this.child(
                            icon_button(
                                (self.id.clone(), "remove"),
                                IconName::Close,
                                format!("Remove {}", attachment.name),
                                window,
                                cx,
                            )
                            .debug_selector(move || format!("attachment-remove-{remove_debug_id}"))
                            .on_click(
                                move |_: &ClickEvent, window, cx| {
                                    handler(
                                        &AttachmentEvent::Removed {
                                            id: remove_id.clone(),
                                        },
                                        window,
                                        cx,
                                    )
                                },
                            ),
                        )
                    })
                    .refine_style(&style)
                    .into_any_element()
            }
            (false, Some(handler)) => {
                let open_id = attachment.id.clone();
                tile_frame(
                    composed_button(self.id.clone(), label.clone()),
                    compact,
                    failed,
                    cx,
                )
                .debug_selector(move || format!("attachment-{debug_id}"))
                // Hover fills; the ring stays a keyboard-focus signal.
                .hover(|style| style.bg(cx.theme().accent.opacity(0.6)))
                .active(|style| style.bg(cx.theme().accent))
                .focus_visible(|style| style.border_color(cx.theme().ring))
                .press_release(
                    ElementId::from((self.id.clone(), "press")),
                    tokens.radius.md,
                    window,
                    cx,
                )
                .child(leading)
                .child(body)
                .on_click(move |_: &ClickEvent, window, cx| {
                    handler(
                        &AttachmentEvent::Opened {
                            id: open_id.clone(),
                        },
                        window,
                        cx,
                    )
                })
                .refine_style(&style)
                .into_any_element()
            }
            (false, None) => tile_frame(div().id(self.id.clone()), compact, failed, cx)
                .role(Role::Group)
                .aria_label(label.clone())
                .debug_selector(move || format!("attachment-{debug_id}"))
                .child(leading)
                .child(body)
                .refine_style(&style)
                .into_any_element(),
        }
    }
}

/// The shared tile chrome: a surface pill when compact, a surface card
/// otherwise; failures get a danger hairline.
fn tile_frame<E: Styled>(element: E, compact: bool, failed: bool, cx: &App) -> E {
    let tokens = cx.theme().semantic_tokens();
    element
        .flex()
        .items_center()
        .max_w_full()
        .min_w_0()
        .gap(tokens.spacing.sm)
        .px(if compact {
            tokens.spacing.sm
        } else {
            tokens.spacing.md
        })
        .py(if compact {
            tokens.spacing.xxs
        } else {
            tokens.spacing.sm
        })
        .border_1()
        .border_color(if failed {
            cx.theme().danger.opacity(0.5)
        } else {
            cx.theme().border
        })
        .rounded(if compact {
            tokens.radius.full
        } else {
            tokens.radius.md
        })
        .bg(tokens.colors.surface)
}

/// A wrapping row of attachment tiles that ripple into place.
///
/// # Example
///
/// ```no_run
/// # use gpui_ai::prelude::*;
/// # fn example(message_attachments: Vec<Attachment>) {
/// AttachmentStrip::new("message-files")
///     .items(message_attachments.iter().cloned())
///     .on_event(|event, _, _| { /* AttachmentEvent::Opened { id } */ });
/// # }
/// ```
#[derive(IntoElement)]
pub struct AttachmentStrip {
    id: ElementId,
    style: StyleRefinement,
    label: SharedString,
    items: Vec<Attachment>,
    removable: bool,
    compact: bool,
    on_event: Option<SharedHandler<AttachmentEvent>>,
}

impl AttachmentStrip {
    /// Creates an empty strip.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            label: "Attachments".into(),
            items: Vec::new(),
            removable: false,
            compact: false,
            on_event: None,
        }
    }

    /// Sets the accessible name of the group.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }

    /// Sets the attachments, in display order.
    pub fn items(mut self, items: impl IntoIterator<Item = Attachment>) -> Self {
        self.items = items.into_iter().collect();
        self
    }

    /// Makes every tile removable instead of openable.
    pub fn removable(mut self, removable: bool) -> Self {
        self.removable = removable;
        self
    }

    /// Uses the single-line chip layout for every tile.
    pub fn compact(mut self, compact: bool) -> Self {
        self.compact = compact;
        self
    }

    /// Handles typed interactions for every tile.
    pub fn on_event(
        mut self,
        handler: impl Fn(&AttachmentEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }
}

impl Styled for AttachmentStrip {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for AttachmentStrip {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        let root_id = self.id.clone();

        // Tiles the strip has already shown stay at rest; a file attached to
        // a mounted composer settles in on the capped cascade, and a strip
        // restored with its attachments joins at rest.
        let motion = MotionTokens::read(cx).clone();
        let roster = window.use_keyed_state((root_id.clone(), "arrivals"), cx, |_, _| {
            ArrivalRoster::new()
        });
        roster.update(cx, |roster, cx| {
            roster.note(
                self.items.iter().map(|attachment| {
                    ElementId::Name(SharedString::from(format!("tile-arrive-{}", attachment.id)))
                }),
                true,
                &motion,
                cx.background_executor().now(),
            );
        });

        let mut tiles = Vec::with_capacity(self.items.len());
        for attachment in self.items.iter() {
            let tile_id = ElementId::from((root_id.clone(), format!("tile-{}", attachment.id)));
            let mut preview = AttachmentPreview::new(tile_id, attachment)
                .removable(self.removable)
                .compact(self.compact);
            if let Some(handler) = self.on_event.clone() {
                preview = preview.on_shared_event(handler);
            }
            let arrival = roster.update(cx, |roster, cx| {
                roster.progress(
                    &ElementId::Name(SharedString::from(format!("tile-arrive-{}", attachment.id))),
                    window,
                    cx,
                )
            });
            let preview = match arrival {
                Some(progress) => preview
                    .opacity(progress)
                    .top(tokens.spacing.xxs * (1.0 - progress) * crate::motion::travel(cx)),
                None => preview,
            };
            tiles.push(preview);
        }
        h_flex()
            .id(self.id)
            .role(Role::Group)
            .aria_label(self.label)
            .flex_wrap()
            .w_full()
            .min_w_0()
            .gap(tokens.spacing.xs)
            .children(tiles)
            .refine_style(&self.style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext, VisualTestContext, div, px};

    struct StripProbe {
        count: usize,
    }

    impl Render for StripProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(360.)).h(px(320.)).child(
                AttachmentStrip::new("probe-strip").items(
                    (0..self.count).map(|ix| Attachment::new(format!("a-{ix}"), "notes.md")),
                ),
            )
        }
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    #[gpui::test]
    fn a_restored_strip_rests_and_a_newly_attached_file_settles_in(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(|_, _| StripProbe { count: 3 });
        let cx: &mut VisualTestContext = cx;
        draw(cx);
        crate::motion::take_reveal_frame_requests();
        draw(cx);
        assert_eq!(
            crate::motion::take_reveal_frame_requests(),
            0,
            "a restored strip joins at rest"
        );

        probe.update(cx, |probe, cx| {
            probe.count = 4;
            cx.notify();
        });
        draw(cx);
        assert!(
            crate::motion::take_reveal_frame_requests() > 0,
            "a file attached to a mounted strip must settle in"
        );
    }

    #[test]
    fn kinds_follow_extensions_case_insensitively() {
        assert_eq!(AttachmentKind::from_name("Hero.PNG"), AttachmentKind::Image);
        assert_eq!(
            AttachmentKind::from_name("notes.md"),
            AttachmentKind::Document
        );
        assert_eq!(AttachmentKind::from_name("main.rs"), AttachmentKind::Code);
        assert_eq!(AttachmentKind::from_name("call.m4a"), AttachmentKind::Audio);
        assert_eq!(AttachmentKind::from_name("demo.mov"), AttachmentKind::Video);
        assert_eq!(
            AttachmentKind::from_name("bundle.tar.gz"),
            AttachmentKind::Archive
        );
        assert_eq!(AttachmentKind::from_name("sales.csv"), AttachmentKind::Data);
        assert_eq!(AttachmentKind::from_name("README"), AttachmentKind::Other);
    }

    #[test]
    fn byte_counts_read_like_a_file_manager() {
        assert_eq!(format_bytes(820), "820 B");
        assert_eq!(format_bytes(12_300), "12 KB");
        assert_eq!(format_bytes(1_400_000), "1.4 MB");
        assert_eq!(format_bytes(3_500_000_000), "3.5 GB");
    }

    #[test]
    fn summaries_describe_lifecycle_before_metadata() {
        let ready = Attachment::new("a", "pricing.md")
            .size_bytes(12_300)
            .detail("12 pages");
        assert_eq!(ready.summary(), "Document · 12 KB · 12 pages");
        assert_eq!(
            ready.accessibility_label(),
            "pricing.md, Document · 12 KB · 12 pages"
        );

        let uploading = Attachment::new("b", "hero.png")
            .state(ProgressState::Running)
            .progress(0.4);
        assert_eq!(uploading.summary(), "Uploading 40%");
        assert!(!uploading.is_ready());

        let failed =
            Attachment::new("c", "big.zip").state(ProgressState::Failed("Too large".into()));
        assert_eq!(failed.summary(), "Failed: Too large");
    }

    #[test]
    fn equality_compares_thumbnails_by_identity() {
        let image = Arc::new(Image::from_bytes(gpui::ImageFormat::Png, Vec::new()));
        let first = Attachment::new("a", "hero.png").thumbnail(image.clone());
        let same = Attachment::new("a", "hero.png").thumbnail(image);
        let other = Attachment::new("a", "hero.png").thumbnail(Arc::new(Image::from_bytes(
            gpui::ImageFormat::Png,
            Vec::new(),
        )));
        assert_eq!(first, same);
        assert_ne!(first, other);
    }
}

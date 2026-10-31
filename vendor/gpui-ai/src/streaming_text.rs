//! Streamed markdown answers with sources and follow-up suggestions.

use crate::stream::{ProgressState, StreamedContent};
use crate::theme::SemanticStyledExt as _;
use crate::{
    control::{composed_button, outlined_control},
    handlers::SharedHandler,
    surface::initial_badge,
};
use gpui::{
    AnyElement, App, Axis, ClickEvent, ElementId, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, Role, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, StyleRefinement, Styled, Window, div,
    prelude::FluentBuilder as _,
};
use gpui_base::Button;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, hover_card::HoverCard,
    scroll::ScrollableMask, text::TextView, v_flex,
};
use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

/// The glyph that marks the tail of text that is still arriving.
const STREAMING_CURSOR: &str = "▌";

#[cfg(test)]
thread_local! {
    /// Counts `transform_citations` calls on the calling thread. Cache tests
    /// assert on work performed rather than elapsed time; each test owns its
    /// thread, so counts never cross tests.
    static TRANSFORM_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };

    /// The Markdown source handed to the text view by the most recent render.
    static RENDERED_SOURCE: std::cell::RefCell<Option<SharedString>> =
        const { std::cell::RefCell::new(None) };
}

/// A source backing a streamed answer, shown as a chip under the text.
///
/// A source with a URL becomes an activatable chip that reports
/// [`StreamingTextEvent::SourceActivated`]; its domain initial doubles as a
/// favicon-style badge so a row of sources scans at a glance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRef {
    id: SharedString,
    title: SharedString,
    url: Option<SharedString>,
}

/// A stable follow-up suggestion shown after an answer settles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowUp {
    id: SharedString,
    label: SharedString,
}

/// A stable inline citation supplied by the application.
///
/// The destination is opaque application data. [`StreamingText`] never opens
/// it directly; activation returns it through
/// [`StreamingTextEvent::CitationActivated`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitationRef {
    id: SharedString,
    label: SharedString,
    title: SharedString,
    destination: SharedString,
}

impl CitationRef {
    /// Creates a citation with stable identity and an application-owned target.
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        title: impl Into<SharedString>,
        destination: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            title: title.into(),
            destination: destination.into(),
        }
    }

    /// Returns the stable application-level citation identifier.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// Returns the visible citation label.
    pub fn label(&self) -> &SharedString {
        &self.label
    }

    /// Returns the accessible citation title.
    pub fn title(&self) -> &SharedString {
        &self.title
    }

    /// Returns the opaque destination passed back on activation.
    pub fn destination(&self) -> &SharedString {
        &self.destination
    }

    fn internal_url(&self) -> String {
        format!("gpui-ai-citation://{}", percent_encode_id(&self.id))
    }
}

impl FollowUp {
    /// Creates a follow-up with a stable application-level identifier.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }

    fn selected_event(&self) -> StreamingTextEvent {
        StreamingTextEvent::FollowUpSelected {
            id: self.id.clone(),
        }
    }
}

/// An interaction emitted by [`StreamingText`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamingTextEvent {
    /// A follow-up suggestion was selected.
    FollowUpSelected {
        /// Stable follow-up identifier.
        id: SharedString,
    },
    /// An inline citation was activated.
    CitationActivated {
        /// Stable application-level citation identifier.
        id: SharedString,
        /// Opaque application-owned navigation destination.
        destination: SharedString,
    },
    /// A source chip with a location was activated.
    SourceActivated {
        /// Stable source identifier.
        id: SharedString,
        /// The source location.
        url: SharedString,
    },
}

fn percent_encode_id(id: &str) -> String {
    let mut encoded = String::with_capacity(id.len());
    for byte in id.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn escape_markdown_label(label: &str) -> String {
    let mut escaped = String::with_capacity(label.len());
    for character in label.chars() {
        if matches!(character, '\\' | '[' | ']') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn escape_markdown_title(title: &str) -> String {
    let mut escaped = String::with_capacity(title.len());
    for character in title.chars() {
        match character {
            '\\' | '"' => {
                escaped.push('\\');
                escaped.push(character);
            }
            '\r' | '\n' => escaped.push(' '),
            _ => escaped.push(character),
        }
    }
    escaped
}

#[derive(Clone, Copy)]
struct FenceCandidate<'a> {
    marker: u8,
    length: usize,
    trailing: &'a str,
}

#[derive(Clone, Copy)]
struct OpenFence {
    marker: u8,
    length: usize,
    quote_depth: usize,
}

fn block_quote_content(mut line: &str) -> (usize, &str) {
    let mut depth = 0;
    loop {
        let bytes = line.as_bytes();
        let indentation = bytes.iter().take_while(|byte| **byte == b' ').count();
        if indentation > 3 || bytes.get(indentation) != Some(&b'>') {
            return (depth, line);
        }

        depth += 1;
        line = &line[indentation + 1..];
        if line.starts_with([' ', '\t']) {
            line = &line[1..];
        }
    }
}

fn fence_candidate(line: &str) -> Option<FenceCandidate<'_>> {
    let bytes = line.as_bytes();
    let indentation = bytes.iter().take_while(|byte| **byte == b' ').count();
    if indentation > 3 {
        return None;
    }
    let marker = *bytes.get(indentation)?;
    if !matches!(marker, b'`' | b'~') {
        return None;
    }
    let length = bytes[indentation..]
        .iter()
        .take_while(|byte| **byte == marker)
        .count();
    (length >= 3).then_some(FenceCandidate {
        marker,
        length,
        trailing: &line[indentation + length..],
    })
}

fn opens_fence(candidate: FenceCandidate<'_>) -> bool {
    candidate.marker != b'`' || !candidate.trailing.contains('`')
}

fn closes_fence(candidate: FenceCandidate<'_>, open: OpenFence) -> bool {
    candidate.marker == open.marker
        && candidate.length >= open.length
        && candidate
            .trailing
            .trim_end_matches(['\r', '\n'])
            .bytes()
            .all(|byte| matches!(byte, b' ' | b'\t'))
}

#[cfg(test)]
fn transform_citation_markers(
    source: &str,
    citations: &[CitationRef],
    interactive: bool,
) -> String {
    transform_citations(source, citations, interactive).markdown
}

struct CitationTransform {
    markdown: String,
    referenced: Vec<CitationRef>,
}

/// Marker id to citation. `or_insert` keeps the first declaration of a repeated
/// id, which is what a linear search over `citations` resolved to.
fn citation_index(citations: &[CitationRef]) -> HashMap<&str, &CitationRef> {
    let mut index = HashMap::with_capacity(citations.len());
    for citation in citations {
        index.entry(citation.id.as_str()).or_insert(citation);
    }
    index
}

fn transform_citations(
    source: &str,
    citations: &[CitationRef],
    interactive: bool,
) -> CitationTransform {
    #[cfg(test)]
    TRANSFORM_CALLS.with(|calls| calls.set(calls.get() + 1));

    // One index for the whole pass keeps marker resolution O(1), so the
    // transform stays linear in source length plus citation count however many
    // markers the answer carries. It is built on the first marker that needs
    // resolving, so an answer that declares citations it has not reached yet —
    // the ordinary early-stream shape — pays nothing for them.
    let mut by_id: Option<HashMap<&str, &CitationRef>> = None;

    let mut transformed = String::with_capacity(source.len());
    let mut referenced = Vec::new();
    let mut referenced_ids: HashSet<&str> = HashSet::new();
    let mut fence: Option<OpenFence> = None;
    let mut inline_ticks: Option<usize> = None;

    for line in source.split_inclusive('\n') {
        let (quote_depth, quote_content) = block_quote_content(line);
        let candidate = fence_candidate(quote_content);

        if let Some(open) = fence {
            if quote_depth >= open.quote_depth {
                transformed.push_str(line);
                if quote_depth == open.quote_depth
                    && candidate.is_some_and(|candidate| closes_fence(candidate, open))
                {
                    fence = None;
                }
                continue;
            }
            fence = None;
        }

        if inline_ticks.is_none()
            && let Some(candidate) = candidate
            && opens_fence(candidate)
        {
            fence = Some(OpenFence {
                marker: candidate.marker,
                length: candidate.length,
                quote_depth,
            });
            transformed.push_str(line);
            continue;
        }

        let mut offset = 0;
        while offset < line.len() {
            let remainder = &line[offset..];
            if remainder.starts_with('`') {
                let run = remainder.bytes().take_while(|byte| *byte == b'`').count();
                transformed.push_str(&remainder[..run]);
                match inline_ticks {
                    Some(open) if open == run => inline_ticks = None,
                    None => inline_ticks = Some(run),
                    _ => {}
                }
                offset += run;
                continue;
            }

            if inline_ticks.is_none()
                && let Some(marker) = remainder.strip_prefix("[[cite:")
                && let Some(end) = marker.find("]]")
            {
                let id = &marker[..end];
                let index = by_id.get_or_insert_with(|| citation_index(citations));
                if let Some(citation) = index.get(id).copied() {
                    if referenced_ids.insert(citation.id.as_str()) {
                        referenced.push(citation.clone());
                    }
                    let label = escape_markdown_label(&citation.label);
                    if interactive {
                        transformed.push('[');
                        transformed.push_str(&label);
                        transformed.push_str("](");
                        transformed.push_str(&citation.internal_url());
                        transformed.push_str(" \"");
                        transformed.push_str(&escape_markdown_title(&citation.title));
                        transformed.push_str("\")");
                    } else {
                        transformed.push_str("\\[");
                        transformed.push_str(&label);
                        transformed.push_str("\\]");
                    }
                    offset += "[[cite:".len() + end + "]]".len();
                    continue;
                }
            }

            if let Some(character) = remainder.chars().next() {
                transformed.push(character);
                offset += character.len_utf8();
            } else {
                break;
            }
        }
    }

    CitationTransform {
        markdown: transformed,
        referenced,
    }
}

fn decorated_source(markdown: &SharedString, streaming: bool) -> SharedString {
    if !streaming {
        return markdown.clone();
    }
    let mut source = String::with_capacity(markdown.len() + STREAMING_CURSOR.len());
    source.push_str(markdown);
    source.push_str(STREAMING_CURSOR);
    SharedString::from(source)
}

/// Everything a cached citation transform depends on, plus the revision of the
/// [`StreamedContent`] it came from.
///
/// Revision is compared first because it is the only field that costs nothing
/// to compare, but it cannot decide reuse alone: two separately constructed
/// [`Progressive`](crate::stream::Progressive) values both report revision `0`,
/// so equal revisions can still carry different text. The text, citation set,
/// interactivity, and streaming flag are the authority.
#[derive(PartialEq, Eq)]
struct CitationKey {
    revision: u64,
    interactive: bool,
    streaming: bool,
    citations: Vec<CitationRef>,
    text: SharedString,
}

impl CitationKey {
    /// Whether the transform's own inputs are unchanged. Revision and the
    /// streaming flag are not transform inputs, so a change confined to them
    /// invalidates the cursor-decorated source and nothing else.
    fn transforms_alike(&self, other: &Self) -> bool {
        self.interactive == other.interactive
            && self.text == other.text
            && self.citations == other.citations
    }
}

/// Citation transform output retained across frames for one component id.
///
/// [`StreamingText`] is `RenderOnce` and is rebuilt every frame, so the
/// retained copy lives in keyed window state. Refreshing memoises a pure
/// function and never notifies, so it cannot schedule a frame from `render`.
struct CitationCache {
    key: Option<CitationKey>,
    markdown: SharedString,
    source: SharedString,
    referenced: Vec<CitationRef>,
}

impl CitationCache {
    fn new() -> Self {
        Self {
            key: None,
            markdown: SharedString::default(),
            source: SharedString::default(),
            referenced: Vec::new(),
        }
    }

    fn refresh(&mut self, key: CitationKey) {
        if self.key.as_ref().is_some_and(|cached| *cached == key) {
            return;
        }
        let transform_stale = self
            .key
            .as_ref()
            .is_none_or(|cached| !cached.transforms_alike(&key));
        if transform_stale {
            let transformed = transform_citations(&key.text, &key.citations, key.interactive);
            self.markdown = SharedString::from(transformed.markdown);
            self.referenced = transformed.referenced;
        }
        let cursor_stale = self
            .key
            .as_ref()
            .is_none_or(|cached| cached.streaming != key.streaming);
        if transform_stale || cursor_stale {
            self.source = decorated_source(&self.markdown, key.streaming);
        }
        self.key = Some(key);
    }
}

fn citation_event_for_url(url: &str, citations: &[CitationRef]) -> Option<StreamingTextEvent> {
    citations
        .iter()
        .find(|citation| citation.internal_url() == url)
        .map(|citation| StreamingTextEvent::CitationActivated {
            id: citation.id.clone(),
            destination: citation.destination.clone(),
        })
}

fn citation_companion_link(
    root_id: ElementId,
    citation: CitationRef,
    handler: SharedHandler<StreamingTextEvent>,
    cx: &mut App,
) -> gpui_base::Link {
    let tokens = cx.theme().semantic_tokens();
    let event = StreamingTextEvent::CitationActivated {
        id: citation.id.clone(),
        destination: citation.destination.clone(),
    };
    let local_id = SharedString::from(format!("citation-{}", citation.id));
    let debug_id = citation.id.clone();
    let visible_label = SharedString::from(format!("Citation: {}", citation.label));

    gpui_base::Link::new((root_id, local_id))
        .debug_selector(move || format!("streaming-citation-{debug_id}"))
        .accessibility_label(citation.title)
        .flex()
        .flex_none()
        .items_center()
        .px(tokens.spacing.sm)
        .py(tokens.spacing.xxs)
        .border_1()
        .border_color(tokens.colors.border)
        .rounded(tokens.radius.sm)
        .bg(tokens.colors.surface)
        .text_token(tokens.typography.xs)
        .text_color(tokens.colors.surface_foreground)
        .hover(|style| style.bg(cx.theme().button_hover))
        .active(|style| style.bg(cx.theme().button_active))
        .focus_visible(|style| style.border_color(tokens.colors.ring))
        .on_activate(move |_, window, cx| handler(&event, window, cx))
        .child(div().child(visible_label))
}

fn follow_up_button(
    follow_up: FollowUp,
    handler: Option<SharedHandler<StreamingTextEvent>>,
    window: &mut Window,
    cx: &mut App,
) -> Button {
    let debug_id = follow_up.id.clone();
    let event = follow_up.selected_event();
    outlined_control(follow_up.id.clone(), follow_up.label, window, cx)
        .debug_selector(move || format!("streaming-follow-up-{debug_id}"))
        .accessibility_id(format!("follow-up-{}", follow_up.id))
        .when_some(handler, |this, handler| {
            this.on_click(move |_: &ClickEvent, window, cx| handler(&event, window, cx))
        })
}

fn citation_preview(title: SharedString, destination: SharedString, cx: &mut App) -> AnyElement {
    let tokens = cx.theme().semantic_tokens();
    v_flex()
        .gap(tokens.spacing.xxs)
        .max_w(tokens.spacing.xxl * 9.0)
        .child(crate::surface::subtitle(title, cx))
        .child(
            div()
                .text_token(tokens.typography.xs)
                .font_family(cx.theme().mono_font_family.clone())
                .text_color(cx.theme().muted_foreground)
                .truncate()
                .child(destination),
        )
        .into_any_element()
}

/// A source chip: favicon-style initial, title, and — with a URL and a
/// handler — an activatable link glyph.
fn source_chip(
    root_id: ElementId,
    index: usize,
    source: SourceRef,
    handler: Option<SharedHandler<StreamingTextEvent>>,
    cx: &mut App,
) -> AnyElement {
    let tokens = cx.theme().semantic_tokens();
    let accessibility_label = source.accessibility_label();
    let badge = initial_badge(source.initial(), cx);
    let content = h_flex()
        .items_center()
        .gap(tokens.spacing.xs)
        .text_token(tokens.typography.xs)
        .text_color(cx.theme().foreground)
        .child(badge)
        .child(source.title.clone())
        .when(source.url.is_some(), |this| {
            this.child(
                Icon::new(IconName::ExternalLink)
                    .xsmall()
                    .text_color(cx.theme().muted_foreground),
            )
        });
    match (source.url.clone(), handler) {
        (Some(url), Some(handler)) => {
            let event = StreamingTextEvent::SourceActivated {
                id: source.id.clone(),
                url,
            };
            let debug_id = source.id.clone();
            composed_button((root_id, format!("source-{index}")), accessibility_label)
                .debug_selector(move || format!("streaming-source-{debug_id}"))
                .px(tokens.spacing.sm)
                .py(tokens.spacing.xxs)
                .bg(cx.theme().secondary)
                .border_1()
                .border_color(cx.theme().border)
                .rounded(tokens.radius.full)
                .hover(|style| style.bg(cx.theme().accent.opacity(0.6)))
                .active(|style| style.bg(cx.theme().accent))
                .focus_visible(|style| style.border_color(cx.theme().ring))
                .child(content)
                .on_click(move |_: &ClickEvent, window, cx| handler(&event, window, cx))
                .into_any_element()
        }
        _ => h_flex()
            .id((root_id, index.to_string()))
            .role(Role::ListItem)
            .aria_label(accessibility_label)
            .px(tokens.spacing.sm)
            .py(tokens.spacing.xxs)
            .bg(cx.theme().secondary)
            .border_1()
            .border_color(cx.theme().border)
            .rounded(tokens.radius.full)
            .child(content)
            .into_any_element(),
    }
}

struct CitationDispatcher {
    citations: Vec<CitationRef>,
    on_event: SharedHandler<StreamingTextEvent>,
}

impl CitationDispatcher {
    fn activate(&self, url: &str, event: &ClickEvent, window: &mut Window, cx: &mut App) -> bool {
        if event.is_right_click() {
            return true;
        }
        let Some(event) = citation_event_for_url(url, &self.citations) else {
            return false;
        };
        (self.on_event)(&event, window, cx);
        true
    }
}

impl SourceRef {
    /// Creates a source chip with a display title; the title doubles as its
    /// stable identifier.
    pub fn new(title: impl Into<SharedString>) -> Self {
        let title = title.into();
        Self {
            id: title.clone(),
            title,
            url: None,
        }
    }

    /// Creates a source with an explicit stable identifier.
    pub fn with_id(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            url: None,
        }
    }

    /// Adds the source's location; the chip becomes activatable and shows
    /// the domain's initial as its badge.
    pub fn url(mut self, url: impl Into<SharedString>) -> Self {
        self.url = Some(url.into());
        self
    }

    /// Returns the stable source identifier.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// Returns the display title.
    pub fn title(&self) -> &SharedString {
        &self.title
    }

    /// Returns the location, when set.
    pub fn url_text(&self) -> Option<&SharedString> {
        self.url.as_ref()
    }

    /// The host of the URL without a `www.` prefix, when a URL is set.
    pub fn domain(&self) -> Option<String> {
        let url = self.url.as_ref()?;
        let rest = url
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(url.as_ref());
        let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
        let host = host.strip_prefix("www.").unwrap_or(host);
        (!host.is_empty()).then(|| host.to_owned())
    }

    /// One uppercase character standing in for a favicon.
    pub fn initial(&self) -> String {
        crate::surface::initial_of(&self.domain().unwrap_or_else(|| self.title.to_string()))
    }

    fn accessibility_label(&self) -> SharedString {
        match self.domain() {
            Some(domain) => format!("{}, {domain}", self.title).into(),
            None => self.title.clone(),
        }
    }
}

/// A streamed markdown answer: live text, then sources and follow-ups.
///
/// The component renders a [`StreamedContent`] snapshot — the application
/// owns the content and re-renders as chunks arrive. While streaming, a
/// cursor glyph marks the end of the text; on failure the reason is shown
/// under the answer.
///
/// # Example
///
/// ```no_run
/// # use gpui_ai::prelude::*;
/// # fn example(answer: StreamedContent) {
/// StreamingText::new("answer", &answer)
///     .citations([
///         CitationRef::new("pricing", "Pricing report", "Open pricing report", "app://pricing"),
///     ])
///     .sources(["pricing.md", "suppliers.csv"])
///     .follow_ups([
///         FollowUp::new("delivery", "Compare delivery times"),
///         FollowUp::new("history", "Show price history"),
///     ])
///     .on_event(|event, _, _| {
///         // Route citation destinations or start follow-up work in the application.
///     });
/// # }
/// ```
#[derive(IntoElement)]
pub struct StreamingText {
    id: ElementId,
    style: StyleRefinement,
    text: SharedString,
    state: ProgressState,
    /// The snapshot's revision, carried privately so the citation transform can
    /// be reused across frames. Not part of any public signature.
    revision: u64,
    sources: Vec<SourceRef>,
    citations: Vec<CitationRef>,
    follow_ups: Vec<FollowUp>,
    on_event: Option<SharedHandler<StreamingTextEvent>>,
    label: SharedString,
}

impl StreamingText {
    /// Creates the component from a snapshot of streamed content.
    pub fn new(id: impl Into<ElementId>, content: &StreamedContent) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            text: SharedString::from(content.text().to_string()),
            state: content.state().clone(),
            revision: content.revision(),
            sources: Vec::new(),
            citations: Vec::new(),
            follow_ups: Vec::new(),
            on_event: None,
            label: "Answer".into(),
        }
    }

    /// Overrides the accessible name (default "Answer").
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }

    /// Adds source chips shown once streaming has finished.
    pub fn sources(mut self, sources: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.sources = sources.into_iter().map(SourceRef::new).collect();
        self
    }

    /// Adds pre-built [`SourceRef`]s (equivalent to [`Self::sources`] today;
    /// kept separate so richer source metadata can grow here).
    pub fn source_refs(mut self, sources: impl IntoIterator<Item = SourceRef>) -> Self {
        self.sources = sources.into_iter().collect();
        self
    }

    /// Adds inline citation metadata resolved from `[[cite:<stable-id>]]`
    /// markers in the streamed Markdown source.
    ///
    /// At the current upstream pin, the inline Markdown glyph is pointer-only.
    /// When an event handler is present, the component therefore renders a
    /// named companion Link as the keyboard and AccessKit authority; both
    /// representations emit the same [`StreamingTextEvent::CitationActivated`].
    pub fn citations(mut self, citations: impl IntoIterator<Item = CitationRef>) -> Self {
        self.citations = citations.into_iter().collect();
        self
    }

    /// Adds follow-up suggestions shown once streaming has finished.
    pub fn follow_ups(mut self, follow_ups: impl IntoIterator<Item = FollowUp>) -> Self {
        self.follow_ups = follow_ups.into_iter().collect();
        self
    }

    /// Handles typed answer interactions.
    pub fn on_event(
        mut self,
        handler: impl Fn(&StreamingTextEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }
}

impl Styled for StreamingText {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for StreamingText {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        let streaming = self.state == ProgressState::Running;
        let settled = self.state == ProgressState::Complete;
        let interactive_citations = self.on_event.is_some();
        let root_id = self.id.clone();
        let sources_settled = crate::motion::acknowledged_state(
            ElementId::from((root_id.clone(), "sources-settled")),
            (settled && !self.sources.is_empty()) as u64,
            window,
            cx,
        );
        let cache = window.use_keyed_state((root_id.clone(), "citation-cache"), cx, |_, _| {
            CitationCache::new()
        });
        let (source, referenced_citations) = cache.update(cx, |cache, _| {
            cache.refresh(CitationKey {
                revision: self.revision,
                interactive: interactive_citations,
                streaming,
                citations: self.citations.clone(),
                text: self.text.clone(),
            });
            (cache.source.clone(), cache.referenced.clone())
        });
        #[cfg(test)]
        RENDERED_SOURCE.with(|rendered| *rendered.borrow_mut() = Some(source.clone()));
        let on_event = self.on_event.clone();
        let failure = match &self.state {
            ProgressState::Failed(reason) => Some(reason.clone()),
            _ => None,
        };
        let dispatcher = on_event.as_ref().map(|handler| {
            let initial_citations = referenced_citations.clone();
            let initial_handler = handler.clone();
            let dispatcher =
                window.use_keyed_state((root_id.clone(), "citation-dispatch"), cx, move |_, _| {
                    CitationDispatcher {
                        citations: initial_citations,
                        on_event: initial_handler,
                    }
                });
            dispatcher.update(cx, |dispatcher, _| {
                dispatcher.citations = referenced_citations.clone();
                dispatcher.on_event = handler.clone();
            });
            dispatcher
        });
        let mut answer = TextView::markdown((root_id.clone(), "answer"), source).selectable(true);
        if let Some(dispatcher) = dispatcher {
            let dispatcher = dispatcher.downgrade();
            answer = answer.on_link_click(move |url, event, window, cx| {
                let handled = dispatcher
                    .update(cx, |dispatcher, cx| {
                        dispatcher.activate(url, event, window, cx)
                    })
                    .is_ok_and(|handled| handled);
                if !handled && !event.is_right_click() {
                    cx.open_url(url);
                }
            });
        }
        let citation_scroll_handle = window
            .use_keyed_state((root_id.clone(), "citation-scroll-handle"), cx, |_, _| {
                ScrollHandle::new()
            })
            .read(cx)
            .clone();

        v_flex()
            .id(self.id)
            .w_full()
            .min_w_0()
            .role(Role::Article)
            .aria_label(self.label)
            .when_some(failure.clone(), |this, reason| {
                this.aria_description(reason)
            })
            .gap(tokens.spacing.md)
            .child(
                div()
                    .debug_selector(|| "streaming-text-body".to_owned())
                    .text_token(tokens.typography.sm)
                    .text_color(cx.theme().foreground)
                    .child(answer),
            )
            .when_some(failure, |this, reason| {
                // The glyph rides a first-line slot: the row stays
                // items_start, so however far the reason wraps, the mark
                // holds centered on the first line.
                this.child(
                    h_flex()
                        .items_start()
                        .gap(tokens.spacing.sm)
                        .text_token(tokens.typography.xs)
                        .text_color(cx.theme().danger)
                        .child(crate::surface::leading_glyph_slot(
                            crate::sizing::slot_sm(cx),
                            Icon::new(IconName::CircleX).xsmall(),
                        ))
                        .child(div().min_w_0().child(reason)),
                )
            })
            .when_some(on_event.clone(), |this, handler| {
                this.when(!referenced_citations.is_empty(), |this| {
                    this.child(
                        v_flex()
                            .id((root_id.clone(), "inline-citations"))
                            .role(Role::Group)
                            .aria_label("Inline citation links")
                            .tab_group()
                            .gap(tokens.spacing.xxs)
                            .child(
                                div()
                                    .relative()
                                    .w_full()
                                    .child(
                                        h_flex()
                                            .id((root_id.clone(), "inline-citation-scroll"))
                                            .debug_selector(|| {
                                                "streaming-citation-scroll".to_owned()
                                            })
                                            .w_full()
                                            .max_w_full()
                                            .min_w_0()
                                            .overflow_x_scroll()
                                            .track_scroll(&citation_scroll_handle)
                                            .gap(tokens.spacing.xs)
                                            .children(referenced_citations.into_iter().map(
                                                |citation| {
                                                    let preview_title = citation.title.clone();
                                                    let preview_destination =
                                                        citation.destination.clone();
                                                    let card_id = ElementId::from((
                                                        root_id.clone(),
                                                        format!("citation-card-{}", citation.id),
                                                    ));
                                                    HoverCard::new(card_id)
                                                        .trigger(citation_companion_link(
                                                            root_id.clone(),
                                                            citation,
                                                            handler.clone(),
                                                            cx,
                                                        ))
                                                        .content(move |_, _, cx| {
                                                            citation_preview(
                                                                preview_title.clone(),
                                                                preview_destination.clone(),
                                                                cx,
                                                            )
                                                        })
                                                },
                                            )),
                                    )
                                    .child(
                                        ScrollableMask::new(
                                            Axis::Horizontal,
                                            &citation_scroll_handle,
                                        )
                                        .id((root_id.clone(), "inline-citation-scroll-mask")),
                                    ),
                            ),
                    )
                })
            })
            .when(settled && !self.sources.is_empty(), |this| {
                // The sources row is a discrete addition: it appears whole
                // when the answer settles, acknowledged once — an answer
                // that mounts already settled shows it at rest, and
                // repeated snapshots replay nothing. Streamed prose above
                // is never animated.
                let settled_in = sources_settled;
                this.child(
                    h_flex()
                        .id((root_id.clone(), "sources"))
                        .opacity(settled_in)
                        .top(tokens.spacing.xxs * (1.0 - settled_in) * crate::motion::travel(cx))
                        .role(Role::List)
                        .aria_label("Sources")
                        .flex_wrap()
                        .gap(tokens.spacing.xs)
                        .children(self.sources.into_iter().enumerate().map(|(ix, source)| {
                            source_chip(root_id.clone(), ix, source, on_event.clone(), cx)
                        })),
                )
            })
            .when(settled && !self.follow_ups.is_empty(), |this| {
                let handler = on_event.clone();
                this.child(
                    h_flex()
                        .id((root_id, "follow-ups"))
                        .role(Role::Group)
                        .aria_label("Follow-up suggestions")
                        .flex_wrap()
                        .gap(tokens.spacing.xs)
                        .children(self.follow_ups.into_iter().map(|follow_up| {
                            follow_up_button(follow_up, handler.clone(), window, cx)
                        })),
                )
            })
            .refine_style(&self.style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::Progressive;
    use gpui::{Element as _, Entity, TestAppContext, VisualTestContext, accesskit, canvas};
    use std::{
        sync::{Arc, Mutex},
        time::Instant,
    };

    /// The pre-cache transform, kept verbatim so the indexed rewrite can be
    /// proven byte-identical rather than merely plausible.
    fn transform_citations_by_linear_scan(
        source: &str,
        citations: &[CitationRef],
        interactive: bool,
    ) -> CitationTransform {
        let mut transformed = String::with_capacity(source.len());
        let mut referenced = Vec::new();
        let mut fence: Option<OpenFence> = None;
        let mut inline_ticks: Option<usize> = None;

        for line in source.split_inclusive('\n') {
            let (quote_depth, quote_content) = block_quote_content(line);
            let candidate = fence_candidate(quote_content);

            if let Some(open) = fence {
                if quote_depth >= open.quote_depth {
                    transformed.push_str(line);
                    if quote_depth == open.quote_depth
                        && candidate.is_some_and(|candidate| closes_fence(candidate, open))
                    {
                        fence = None;
                    }
                    continue;
                }
                fence = None;
            }

            if inline_ticks.is_none()
                && let Some(candidate) = candidate
                && opens_fence(candidate)
            {
                fence = Some(OpenFence {
                    marker: candidate.marker,
                    length: candidate.length,
                    quote_depth,
                });
                transformed.push_str(line);
                continue;
            }

            let mut offset = 0;
            while offset < line.len() {
                let remainder = &line[offset..];
                if remainder.starts_with('`') {
                    let run = remainder.bytes().take_while(|byte| *byte == b'`').count();
                    transformed.push_str(&remainder[..run]);
                    match inline_ticks {
                        Some(open) if open == run => inline_ticks = None,
                        None => inline_ticks = Some(run),
                        _ => {}
                    }
                    offset += run;
                    continue;
                }

                if inline_ticks.is_none()
                    && let Some(marker) = remainder.strip_prefix("[[cite:")
                    && let Some(end) = marker.find("]]")
                {
                    let id = &marker[..end];
                    if let Some(citation) = citations.iter().find(|citation| citation.id == id) {
                        if !referenced
                            .iter()
                            .any(|referenced: &CitationRef| referenced.id == citation.id)
                        {
                            referenced.push(citation.clone());
                        }
                        let label = escape_markdown_label(&citation.label);
                        if interactive {
                            transformed.push('[');
                            transformed.push_str(&label);
                            transformed.push_str("](");
                            transformed.push_str(&citation.internal_url());
                            transformed.push_str(" \"");
                            transformed.push_str(&escape_markdown_title(&citation.title));
                            transformed.push_str("\")");
                        } else {
                            transformed.push_str("\\[");
                            transformed.push_str(&label);
                            transformed.push_str("\\]");
                        }
                        offset += "[[cite:".len() + end + "]]".len();
                        continue;
                    }
                }

                if let Some(character) = remainder.chars().next() {
                    transformed.push(character);
                    offset += character.len_utf8();
                } else {
                    break;
                }
            }
        }

        CitationTransform {
            markdown: transformed,
            referenced,
        }
    }

    fn citations() -> [CitationRef; 2] {
        [
            CitationRef::new("pricing", "Pricing", "Open pricing source", "app://pricing"),
            CitationRef::new(
                "supplier notes",
                "Supplier [notes]",
                "Open \"supplier\" notes",
                "app://suppliers?section=notes",
            ),
        ]
    }

    #[test]
    fn maps_all_shared_lifecycle_states() {
        let cases = [
            Progressive::pending("answer".to_owned()),
            Progressive::running("answer".to_owned()),
            Progressive::complete("answer".to_owned()),
            Progressive::failed("answer".to_owned(), "offline"),
        ];
        for content in cases {
            assert_eq!(
                StreamingText::new("answer", &content).state,
                *content.state()
            );
        }
    }

    #[test]
    fn sources_derive_domains_and_initials() {
        let web = SourceRef::new("Wholesale pricing").url("https://www.alpenrose.com/pricing?q=1");
        assert_eq!(web.domain().as_deref(), Some("alpenrose.com"));
        assert_eq!(web.initial(), "A");
        assert_eq!(
            web.accessibility_label(),
            "Wholesale pricing, alpenrose.com"
        );
        let file = SourceRef::new("pricing.md");
        assert_eq!(file.domain(), None);
        assert_eq!(file.initial(), "P");
        assert_eq!(file.id(), "pricing.md");
        let custom = SourceRef::with_id("src-1", "(notes)");
        assert_eq!(custom.initial(), "N");
        assert_eq!(SourceRef::new("…").initial(), "•");
    }

    #[test]
    fn duplicate_follow_up_labels_emit_distinct_stable_ids() {
        let first = FollowUp::new("first", "Try again");
        let second = FollowUp::new("second", "Try again");
        assert_eq!(
            first.selected_event(),
            StreamingTextEvent::FollowUpSelected { id: "first".into() }
        );
        assert_eq!(
            second.selected_event(),
            StreamingTextEvent::FollowUpSelected {
                id: "second".into()
            }
        );
    }

    #[test]
    fn complete_known_markers_become_internal_markdown_links() {
        assert_eq!(
            transform_citation_markers("See [[cite:pricing]].", &citations(), true),
            "See [Pricing](gpui-ai-citation://pricing \"Open pricing source\")."
        );
        assert_eq!(
            transform_citation_markers("See [[cite:supplier notes]].", &citations(), true),
            "See [Supplier \\[notes\\]](gpui-ai-citation://supplier%20notes \"Open \\\"supplier\\\" notes\")."
        );
    }

    #[test]
    fn unknown_and_incomplete_markers_remain_readable_literals() {
        assert_eq!(
            transform_citation_markers("Unknown [[cite:missing]].", &citations(), true),
            "Unknown [[cite:missing]]."
        );
        assert_eq!(
            transform_citation_markers("Partial [[cite:pricing", &citations(), true),
            "Partial [[cite:pricing"
        );
    }

    #[test]
    fn noninteractive_citations_render_visible_non_link_labels() {
        assert_eq!(
            transform_citation_markers("See [[cite:pricing]].", &citations(), false),
            "See \\[Pricing\\]."
        );
    }

    #[test]
    fn markers_inside_inline_and_fenced_code_are_not_rewritten() {
        let source = concat!(
            "Outside [[cite:pricing]] and `inline [[cite:pricing]]`.\n\n",
            "```text\n[[cite:pricing]]\n```\n",
            "~~~\n[[cite:pricing]]\n~~~\n"
        );
        let expected = concat!(
            "Outside [Pricing](gpui-ai-citation://pricing \"Open pricing source\") and `inline [[cite:pricing]]`.\n\n",
            "```text\n[[cite:pricing]]\n```\n",
            "~~~\n[[cite:pricing]]\n~~~\n"
        );
        assert_eq!(
            transform_citation_markers(source, &citations(), true),
            expected
        );
    }

    #[test]
    fn trailing_text_cannot_close_a_fenced_code_block() {
        let source = concat!(
            "```rust\n",
            "[[cite:pricing]]\n",
            "```not-a-close\n",
            "[[cite:pricing]]\n",
            "```\n",
            "Outside [[cite:pricing]]."
        );
        let expected = concat!(
            "```rust\n",
            "[[cite:pricing]]\n",
            "```not-a-close\n",
            "[[cite:pricing]]\n",
            "```\n",
            "Outside [Pricing](gpui-ai-citation://pricing \"Open pricing source\")."
        );

        assert_eq!(
            transform_citation_markers(source, &citations(), true),
            expected
        );
    }

    #[test]
    fn block_quote_fences_protect_markers_and_accept_longer_closers() {
        let source = concat!(
            "> ```rust\n",
            "> [[cite:pricing]]\n",
            "> ````\n",
            "Outside [[cite:pricing]]."
        );
        let expected = concat!(
            "> ```rust\n",
            "> [[cite:pricing]]\n",
            "> ````\n",
            "Outside [Pricing](gpui-ai-citation://pricing \"Open pricing source\")."
        );

        assert_eq!(
            transform_citation_markers(source, &citations(), true),
            expected
        );
    }

    #[test]
    fn duplicate_labels_route_by_stable_id_and_preserve_destination() {
        let refs = [
            CitationRef::new("first", "Report", "Open first", "app://first"),
            CitationRef::new("second", "Report", "Open second", "app://second"),
        ];

        assert_eq!(
            citation_event_for_url("gpui-ai-citation://second", &refs),
            Some(StreamingTextEvent::CitationActivated {
                id: "second".into(),
                destination: "app://second".into(),
            })
        );
    }

    #[test]
    fn only_complete_referenced_citations_receive_companion_controls() {
        let transformed = transform_citations(
            "[[cite:pricing]] again [[cite:pricing]] then [[cite:supplier notes",
            &citations(),
            true,
        );

        assert_eq!(transformed.referenced.len(), 1);
        assert_eq!(transformed.referenced[0].id(), "pricing");
    }

    fn numbered_citations(count: usize) -> Vec<CitationRef> {
        (0..count)
            .map(|index| {
                CitationRef::new(
                    format!("source-{index}"),
                    format!("Source [{index}]"),
                    format!("Open \"source {index}\""),
                    format!("app://sources/{index}?section=notes"),
                )
            })
            .collect()
    }

    /// Prose of at least `bytes` length carrying one marker per sentence,
    /// cycling through `citations` so a long answer references many of them.
    fn answer_of(bytes: usize, citations: &[CitationRef]) -> String {
        const SENTENCE: &str = "Wholesale prices moved again this quarter, and the report explains the shift in detail. ";
        let mut source = String::with_capacity(bytes + SENTENCE.len() + 64);
        let mut sentence = 0usize;
        while source.len() < bytes {
            source.push_str(SENTENCE);
            if let Some(citation) = citations.get(sentence % citations.len().max(1)) {
                source.push_str("[[cite:");
                source.push_str(citation.id());
                source.push_str("]] ");
            }
            sentence += 1;
            if sentence.is_multiple_of(4) {
                source.push_str("\n\n");
            }
        }
        source
    }

    #[test]
    fn the_indexed_transform_matches_the_linear_scan_byte_for_byte() {
        let dense = numbered_citations(100);
        let duplicate_ids = [
            CitationRef::new("pricing", "First", "Open first", "app://first"),
            CitationRef::new("pricing", "Second", "Open second", "app://second"),
        ];
        let edge_cases = concat!(
            "Outside [[cite:pricing]] and `inline [[cite:pricing]]`.\n\n",
            "```text\n[[cite:pricing]]\n```\n",
            "~~~\n[[cite:supplier notes]]\n~~~\n",
            "> ```rust\n> [[cite:pricing]]\n> ````\n",
            "Unknown [[cite:missing]], partial [[cite:pricing, repeat [[cite:pricing]].\n",
            "Escaped [[cite:supplier notes]] twice [[cite:supplier notes]].\n"
        );
        let cases: [(&str, &[CitationRef]); 5] = [
            (edge_cases, &citations()),
            (edge_cases, &duplicate_ids),
            (&answer_of(4 * 1024, &dense), &dense),
            (&answer_of(4 * 1024, &citations()), &citations()),
            (&answer_of(1024, &[]), &[]),
        ];

        for (source, refs) in cases {
            for interactive in [true, false] {
                let indexed = transform_citations(source, refs, interactive);
                let scanned = transform_citations_by_linear_scan(source, refs, interactive);
                assert_eq!(indexed.markdown, scanned.markdown);
                assert_eq!(indexed.referenced, scanned.referenced);
            }
        }
    }

    /// Fastest of `rounds` timed runs, after one untimed warm-up, in
    /// microseconds per pass. The minimum is the least noisy estimator here:
    /// this measures a pure function against an unloaded allocator.
    fn microseconds_per_pass(rounds: usize, passes: usize, mut pass: impl FnMut() -> usize) -> f64 {
        let mut observed = 0usize;
        let mut best = f64::MAX;
        for round in 0..=rounds {
            let started = Instant::now();
            for _ in 0..passes {
                observed = observed.wrapping_add(pass());
            }
            let elapsed = started.elapsed().as_secs_f64() * 1e6 / passes as f64;
            if round > 0 {
                best = best.min(elapsed);
            }
        }
        assert!(observed > 0, "the timed transform must produce output");
        best
    }

    #[test]
    fn citation_transform_timings_are_informational() {
        println!("citation transform, microseconds per pass (best of 3)");
        println!(
            "{:>10} {:>10} {:>10} {:>12} {:>14}",
            "bytes", "citations", "markers", "indexed", "linear scan"
        );
        for bytes in [1024, 32 * 1024, 256 * 1024] {
            for count in [0, 10, 100] {
                let refs = numbered_citations(count);
                // An answer that declares citations it has not reached yet is
                // the ordinary early-stream shape, so it gets its own row.
                for cited in [true, false] {
                    if count == 0 && !cited {
                        continue;
                    }
                    let source = answer_of(bytes, if cited { refs.as_slice() } else { &[] });
                    let markers = source.matches("[[cite:").count();
                    let passes = (1 << 20usize) / bytes;
                    let indexed = microseconds_per_pass(3, passes, || {
                        transform_citations(&source, &refs, true).markdown.len()
                    });
                    let scanned = microseconds_per_pass(3, passes, || {
                        transform_citations_by_linear_scan(&source, &refs, true)
                            .markdown
                            .len()
                    });
                    println!(
                        "{bytes:>10} {count:>10} {markers:>10} {indexed:>12.1} {scanned:>14.1}"
                    );
                }
            }
        }
    }

    struct CacheProbe {
        content: StreamedContent,
        citations: Vec<CitationRef>,
        interactive: bool,
    }

    impl gpui::Render for CacheProbe {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            StreamingText::new("cache-probe", &self.content)
                .citations(self.citations.clone())
                .when(self.interactive, |this| this.on_event(|_, _, _| {}))
        }
    }

    fn transform_calls() -> usize {
        TRANSFORM_CALLS.with(std::cell::Cell::get)
    }

    fn rendered_source() -> SharedString {
        RENDERED_SOURCE.with(|rendered| {
            rendered
                .borrow()
                .clone()
                .expect("a frame should have rendered the answer")
        })
    }

    fn cache_probe(
        cx: &mut TestAppContext,
        content: StreamedContent,
    ) -> (Entity<CacheProbe>, &mut VisualTestContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(move |_, _| CacheProbe {
            content,
            citations: citations().to_vec(),
            interactive: true,
        });
        let cx: &mut VisualTestContext = cx;
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (probe, cx)
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn cited_answer() -> String {
        "Prices rose; see [[cite:pricing]] and [[cite:supplier notes]].".to_owned()
    }

    #[gpui::test]
    fn an_unrelated_re_render_reuses_the_cached_citation_transform(cx: &mut TestAppContext) {
        let (probe, cx) = cache_probe(cx, Progressive::running(cited_answer()));
        let transformed_once = transform_calls();
        assert!(transformed_once > 0, "the first frame must transform");
        let first_source = rendered_source();

        // Nothing about the answer changed; only the parent asked for a frame.
        probe.update(cx, |_, cx| cx.notify());
        draw(cx);
        draw(cx);

        assert_eq!(transform_calls(), transformed_once);
        assert_eq!(rendered_source(), first_source);
    }

    #[gpui::test]
    fn appending_streamed_text_invalidates_the_cached_transform(cx: &mut TestAppContext) {
        let (probe, cx) = cache_probe(cx, Progressive::running(cited_answer()));
        let before = transform_calls();

        probe.update(cx, |probe, cx| {
            probe.content.append(" Also [[cite:pricing]] again.");
            cx.notify();
        });
        draw(cx);

        assert_eq!(transform_calls(), before + 1);
        assert!(rendered_source().contains("again"));
    }

    #[gpui::test]
    fn replacing_text_at_the_same_revision_invalidates_the_cache(cx: &mut TestAppContext) {
        let (probe, cx) = cache_probe(
            cx,
            Progressive::complete("First [[cite:pricing]] answer.".to_owned()),
        );
        let before = transform_calls();
        assert!(rendered_source().contains("First"));

        // A freshly constructed snapshot reports revision zero exactly like the
        // one it replaces, so revision alone would report a false cache hit.
        probe.update(cx, |probe, cx| {
            probe.content = Progressive::complete("Second [[cite:pricing]] answer.".to_owned());
            assert_eq!(probe.content.revision(), 0);
            cx.notify();
        });
        draw(cx);

        assert_eq!(transform_calls(), before + 1);
        assert!(rendered_source().contains("Second"));
    }

    #[gpui::test]
    fn changing_the_citation_set_invalidates_the_cache(cx: &mut TestAppContext) {
        let (probe, cx) = cache_probe(cx, Progressive::complete(cited_answer()));
        let before = transform_calls();
        assert!(rendered_source().contains("Supplier"));

        probe.update(cx, |probe, cx| {
            probe.citations = vec![citations()[0].clone()];
            cx.notify();
        });
        draw(cx);

        assert_eq!(transform_calls(), before + 1);
        let source = rendered_source();
        assert!(source.contains("Pricing"));
        assert!(source.contains("[[cite:supplier notes]]"));
    }

    #[gpui::test]
    fn changing_interactivity_invalidates_the_cache(cx: &mut TestAppContext) {
        let (probe, cx) = cache_probe(cx, Progressive::complete(cited_answer()));
        let before = transform_calls();
        assert!(rendered_source().contains("gpui-ai-citation://pricing"));

        probe.update(cx, |probe, cx| {
            probe.interactive = false;
            cx.notify();
        });
        draw(cx);

        assert_eq!(transform_calls(), before + 1);
        let source = rendered_source();
        assert!(!source.contains("gpui-ai-citation://"));
        assert!(source.contains("\\[Pricing\\]"));
    }

    #[gpui::test]
    fn a_lifecycle_move_rebuilds_only_the_cursor(cx: &mut TestAppContext) {
        let (probe, cx) = cache_probe(cx, Progressive::running(cited_answer()));
        let before = transform_calls();
        let streaming_source = rendered_source();
        assert!(streaming_source.ends_with(STREAMING_CURSOR));

        probe.update(cx, |probe, cx| {
            probe.content.finish();
            cx.notify();
        });
        draw(cx);
        assert!(
            probe.read_with(cx, |probe, _| probe.content.revision()) > 0,
            "finishing must advance the revision"
        );

        // The cursor is decoration over the transform, not an input to it.
        assert_eq!(transform_calls(), before);
        let settled_source = rendered_source();
        assert_ne!(settled_source, streaming_source);
        assert_eq!(
            Some(settled_source.as_str()),
            streaming_source.strip_suffix(STREAMING_CURSOR)
        );

        // A second lifecycle move that leaves the cursor off keeps both parts.
        probe.update(cx, |probe, cx| {
            probe.content.fail("offline");
            cx.notify();
        });
        draw(cx);
        assert_eq!(transform_calls(), before);
        assert_eq!(rendered_source(), settled_source);
    }

    struct CapturedCitationA11y {
        role: Option<Role>,
        node: accesskit::Node,
    }

    struct CitationA11yProbe {
        captured: Arc<Mutex<Option<CapturedCitationA11y>>>,
    }

    impl gpui::Render for CitationA11yProbe {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            let captured = self.captured.clone();
            canvas(
                move |_, window, cx| {
                    let element = citation_companion_link(
                        ElementId::from("answer"),
                        CitationRef::new(
                            "pricing",
                            "Pricing",
                            "Open pricing source",
                            "app://pricing",
                        ),
                        Rc::new(|_, _, _| {}),
                        cx,
                    )
                    .render(window, cx)
                    .into_element();
                    let role = element.a11y_role();
                    let mut node = accesskit::Node::new(role.unwrap_or(Role::Unknown));
                    element.write_a11y_info(&mut node);
                    *captured.lock().expect("capture mutex should be available") =
                        Some(CapturedCitationA11y { role, node });
                },
                |_, _, _, _| {},
            )
        }
    }

    #[gpui::test]
    fn companion_link_exposes_role_name_and_click_action(cx: &mut gpui::TestAppContext) {
        cx.update(crate::init);
        let captured = Arc::new(Mutex::new(None));
        let result = captured.clone();
        let (_, cx) = cx.add_window_view(move |_, _| CitationA11yProbe { captured });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let captured = result
            .lock()
            .expect("capture mutex should be available")
            .take()
            .expect("citation link should be captured");
        assert_eq!(captured.role, Some(Role::Link));
        assert_eq!(captured.node.label(), Some("Open pricing source"));
        assert!(captured.node.supports_action(accesskit::Action::Click));
    }

    struct FollowUpA11yProbe {
        captured: Arc<Mutex<Option<CapturedCitationA11y>>>,
    }

    impl gpui::Render for FollowUpA11yProbe {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            let captured = self.captured.clone();
            canvas(
                move |_, window, cx| {
                    let element = follow_up_button(
                        FollowUp::new("compare", "Compare suppliers"),
                        Some(Rc::new(|_, _, _| {})),
                        window,
                        cx,
                    )
                    .render(window, cx)
                    .into_element();
                    let role = element.a11y_role();
                    let mut node = accesskit::Node::new(Role::Unknown);
                    element.write_a11y_info(&mut node);
                    *captured.lock().expect("capture mutex should be available") =
                        Some(CapturedCitationA11y { role, node });
                },
                |_, _, _, _| {},
            )
        }
    }

    #[gpui::test]
    fn follow_up_exposes_production_role_name_and_click_action(cx: &mut gpui::TestAppContext) {
        cx.update(crate::init);
        let captured = Arc::new(Mutex::new(None));
        let result = captured.clone();
        let (_, cx) = cx.add_window_view(move |_, _| FollowUpA11yProbe { captured });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let captured = result
            .lock()
            .expect("capture mutex should be available")
            .take()
            .expect("follow-up button should be captured");
        assert_eq!(captured.role, Some(Role::Button));
        assert_eq!(captured.node.label(), Some("Compare suppliers"));
        assert!(captured.node.supports_action(accesskit::Action::Click));
    }
}

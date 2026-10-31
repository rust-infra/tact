//! Shared surface and typography helpers.
//!
//! These are the crate's visual grammar in code: one card frame, one title
//! role, one description role, one eyebrow, one quiet icon button. Every
//! component builds from them so radii, spacing, weights, and hover treatment
//! stay identical across the library — and change in one place.

use crate::control::{PressReleaseExt as _, composed_button};
use crate::motion::VisibleAnimationExt as _;
use crate::theme::SemanticStyledExt as _;
use gpui::{
    App, Div, ElementId, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, RenderOnce, SharedString, Stateful,
    StatefulInteractiveElement as _, StyleRefinement, Styled, Window, div,
    prelude::FluentBuilder as _,
};
use gpui_base::{Button, StyledExt as _};
use gpui_component::{ActiveTheme as _, Icon, IconNamed, Sizable as _, tooltip::Tooltip, v_flex};

/// The card's own surface: background, hairline border, and the card
/// radius — the three properties that make a thing look like a card,
/// without the layout a particular card wants.
///
/// Most of the library's cards are not [`card`]: they carry their own
/// padding and gaps, or they are buttons, or they are scroll containers.
/// They stated these three lines each instead, which is how a radius
/// change becomes a ten-file change.
pub(crate) trait CardFrameExt: Sized {
    /// Paints this element as a card: surface, hairline border, card radius.
    fn card_frame(self, cx: &App) -> Self;
}

impl<E: gpui::Styled + Sized> CardFrameExt for E {
    fn card_frame(self, cx: &App) -> Self {
        let tokens = cx.theme().semantic_tokens();
        self.bg(tokens.colors.surface)
            .border_1()
            .border_color(cx.theme().border)
            .rounded(tokens.radius.lg)
    }
}

/// The standard card: [`card_frame`] plus panel padding and a medium
/// content gap, for a card whose layout is a plain vertical stack.
pub(crate) fn card(id: impl Into<ElementId>, cx: &App) -> Stateful<Div> {
    let tokens = cx.theme().semantic_tokens();
    v_flex()
        .id(id)
        .w_full()
        .min_w_0()
        .gap(tokens.spacing.md)
        .p(tokens.spacing.lg)
        .card_frame(cx)
}

/// Loading rows that mirror the layout they stand in for: quiet muted
/// blocks in the coming shape, breathing on one shared clock so a whole
/// skeleton costs a single scheduled animation. Column widths vary
/// deterministically so the placeholder reads as content, not stripes.
/// A reduced motion preference holds the pulse at its middle; the caller
/// keeps the progress semantics (role and label) on its own frame.
pub(crate) fn skeleton_rows(
    id: impl Into<ElementId>,
    rows: usize,
    columns: usize,
    cx: &App,
) -> impl IntoElement {
    let tokens = cx.theme().semantic_tokens();
    let block = cx.theme().muted;
    let gap = tokens.spacing.lg;
    let row_gap = tokens.spacing.sm;
    let radius = tokens.radius.sm;
    let full = crate::motion::motion_is_full(cx);
    let spec = crate::motion::MotionTokens::read(cx).breathing();
    v_flex().w_full().gap(row_gap).with_visible_animation(
        id,
        spec.looping_synced(),
        move |body, delta| {
            let delta = if full { delta } else { 0.5 };
            // A triangle wave keeps the pulse symmetric; the band is
            // narrow so the skeleton stays quiet.
            let wave = (delta * 2.0 - 1.0).abs();
            let pulse = 0.55 + 0.3 * wave;
            body.opacity(pulse).children((0..rows).map(|row| {
                gpui::div()
                    .flex()
                    .w_full()
                    .items_center()
                    .gap(gap)
                    .children((0..columns).map(move |column| {
                        let fraction = 0.5 + 0.4 * (((row * 7 + column * 3) % 5) as f32 / 4.0);
                        gpui::div().flex_1().child(
                            gpui::div()
                                .h(gpui::rems(0.75))
                                .w(gpui::relative(fraction))
                                .rounded(radius)
                                .bg(block),
                        )
                    }))
            }))
        },
    )
}

/// The hairline a structural divider is drawn in: the border color at
/// reduced alpha, so a rule inside a bordered container separates
/// without competing with the frame, on any theme.
pub(crate) fn hairline(cx: &App) -> gpui::Hsla {
    cx.theme().border.opacity(0.6)
}

/// The empty-state anatomy every surface shares: a quiet icon, one line
/// that says why it is empty, and an optional hint — centered, padded,
/// never a bare string in a corner. Callers keep their own identity,
/// role, and status semantics on the wrapper they mount this into.
pub(crate) fn empty_state(
    icon: impl IconNamed,
    title: impl Into<SharedString>,
    note: Option<SharedString>,
    cx: &App,
) -> Div {
    let tokens = cx.theme().semantic_tokens();
    v_flex()
        .w_full()
        .items_center()
        .gap(tokens.spacing.xs)
        .p(tokens.spacing.lg)
        .child(
            Icon::new(icon)
                .small()
                .text_color(cx.theme().muted_foreground),
        )
        .child(div().text_token(tokens.typography.sm).child(title.into()))
        .when_some(note, |this, note| this.child(hint(note, cx)))
}

/// The nesting rule for a rounded surface inside a rounded container:
/// inner radius = container radius − inset, floored so a deep inset
/// cannot square the corner entirely.
pub(crate) fn nested_radius(container: Pixels, inset: Pixels, floor: Pixels) -> Pixels {
    if container > inset {
        (container - inset).max(floor)
    } else {
        floor
    }
}

/// The one selected-surface grammar for rows in lists and pickers.
///
/// An inset, rounded fill spanning the whole row — trailing controls
/// included — whose radius follows the nesting rule against the row's
/// container. Selection paints the theme's list-active token and keeps a
/// visible hover delta; unselected rows hover on the list-hover token.
/// Callers own semantics (aria_selected and friends) and content; this
/// owns only the surface, so every list that selects looks like one
/// family.
/// Where a row's hover comes from.
///
/// A list either paints hover on each row or runs one highlight that
/// glides between them; a row cannot do both without painting the hover
/// twice. Passing the list's glide state — or `None` — is how a row says
/// which list it is in.
pub(crate) type RowGlide<'a> =
    Option<(&'a SharedString, &'a gpui::Entity<crate::glide::GlideHover>)>;

pub(crate) fn selection_surface<E>(
    row: E,
    selected: bool,
    container_radius: Pixels,
    inset: Pixels,
    glide: RowGlide<'_>,
    cx: &App,
) -> E
where
    E: gpui::StatefulInteractiveElement + gpui::ParentElement + gpui::Styled,
{
    let tokens = cx.theme().semantic_tokens();
    let radius = nested_radius(container_radius, inset, tokens.radius.sm);
    let row = row.rounded(radius);
    match glide {
        // The highlight is the hover; a fill here would paint it twice.
        Some((key, state)) => {
            let row = if selected {
                row.bg(cx.theme().list_active)
            } else {
                row
            };
            crate::glide::glide_row(row, key.clone(), state)
        }
        None if selected => row
            .bg(cx.theme().list_active)
            .hover(|style| style.bg(cx.theme().list_active.opacity(0.85))),
        None => row.hover(|style| style.bg(cx.theme().list_hover)),
    }
}

/// Seats a glyph beside wrappable text, centered on the text's first line.
///
/// A square box whose side equals the first line's line-height (the size
/// policy's slot tokens name the ones the crate uses), so the row itself stays
/// `items_start` and the glyph holds to the first line however far the text
/// wraps. Centering a bare glyph against a whole text block is the
/// misalignment class the 0.4.0 audit found across eight components; a row
/// that can wrap composes this instead.
pub(crate) fn leading_glyph_slot(slot: Pixels, glyph: impl IntoElement) -> Div {
    leading_control_slot(slot, slot, glyph)
}

/// The same seat for a control that is not square.
///
/// Two dimensions because a switch is not a glyph: its track is wider than it
/// is tall, and a square seat crushed it to the width of a checkbox - which is
/// what turned the library's only switch into a dot with its thumb outside the
/// track. `line` is still the first line's line-height, because that is what
/// holds the control against the text; `width` is the widest control the
/// column can hold, so a mixed column of switches and boxes keeps one text
/// inset and one left edge.
///
/// Left-aligned rather than centered, for that left edge: a checkbox centered
/// in a switch's width would sit five pixels in from every switch above it.
pub(crate) fn leading_control_slot(line: Pixels, width: Pixels, control: impl IntoElement) -> Div {
    div()
        .flex_none()
        .w(width)
        .h(line)
        .flex()
        .items_center()
        .justify_start()
        .child(control)
}

/// A compact inset panel placed *inside* a card (payloads, code, previews).
/// Uses the muted surface and the medium radius so it nests without a
/// second full-card frame.
pub(crate) fn inset(cx: &App) -> Div {
    let tokens = cx.theme().semantic_tokens();
    div()
        .w_full()
        .min_w_0()
        .p(tokens.spacing.md)
        .bg(cx.theme().muted.opacity(0.45))
        .rounded(tokens.radius.md)
}

/// Card or section title.
pub(crate) fn title(text: impl Into<SharedString>, cx: &App) -> Div {
    let tokens = cx.theme().semantic_tokens();
    // No ink of its own. The root sets the theme's foreground and every
    // surface inherits it, so pinning it here changed nothing except that a
    // caller could no longer say otherwise.
    div()
        .text_token(tokens.typography.md)
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

/// Supporting prose under a title.
pub(crate) fn description(text: impl Into<SharedString>, cx: &App) -> Quiet {
    Quiet::new(text, cx.theme().semantic_tokens().typography.sm)
}

/// Supporting text, in whatever ink the surface around it is written in.
///
/// The library's quiet roles — descriptions, hints, eyebrows, metadata — all
/// want the same thing: to read as softer than the words above them. That was
/// the theme's `muted_foreground`, painted straight onto the element, and it
/// made text colour the one style a caller could not influence: a component's
/// leaf always won, however the component itself was styled.
///
/// A `Div` cannot do better, because it fixes its colour when it is built and
/// the surface has not said what ink is in force yet. So this is an element
/// instead, and it reads the ink during layout, inside the parent's text
/// scope — the same window a button's label uses to find its font. It is still
/// exactly one `div` in the tree, and it is still `Styled`, so every caller
/// that chains `truncate` or `flex_none` onto a hint keeps working.
///
/// The rule it applies: while the ink is the theme's own, use the theme's own
/// muted ink, which is authored per theme and is not merely the foreground at
/// lower alpha. Once someone has changed the ink — a card over a photograph,
/// say — follow them, softened, because their ink is the one that has to stay
/// readable.
#[derive(IntoElement)]
pub(crate) struct Quiet {
    text: SharedString,
    token: gpui_base::theme_tokens::TextStyleToken,
    family: Option<SharedString>,
    semibold: bool,
    style: StyleRefinement,
}

impl Quiet {
    fn new(text: impl Into<SharedString>, token: gpui_base::theme_tokens::TextStyleToken) -> Self {
        Self {
            text: text.into(),
            token,
            family: None,
            semibold: false,
            style: StyleRefinement::default(),
        }
    }

    fn family(mut self, family: SharedString) -> Self {
        self.family = Some(family);
        self
    }

    fn semibold(mut self) -> Self {
        self.semibold = true;
        self
    }
}

impl Styled for Quiet {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Quiet {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .text_token(self.token)
            .when_some(self.family, |text, family| text.font_family(family))
            .when(self.semibold, |text| text.font_weight(FontWeight::SEMIBOLD))
            .text_color(quiet_ink(window.text_style().color, cx))
            // After the ink, so a caller who names a colour on the role
            // itself still outranks what the surface implies.
            .refine_style(&self.style)
            .child(self.text)
    }
}

/// The supporting ink to use while `ink` is the ink in force.
///
/// While the ink is the theme's own, the theme's own muted ink - authored per
/// theme, and not the foreground at lower alpha in any of the fifty-five.
///
/// Once someone has changed it, theirs, unsoftened. Softening it was tried and
/// is wrong: an override is a deliberate act for a background the theme could
/// not know about - white over a photograph - and dimming it to seventy-two
/// per cent spends the legibility it was chosen for. The hierarchy does not
/// need the colour anyway: supporting text is already a size and a weight
/// below the words above it, which is what still tells them apart here.
fn quiet_ink(ink: Hsla, cx: &App) -> Hsla {
    if ink == cx.theme().foreground {
        cx.theme().muted_foreground
    } else {
        ink
    }
}

/// A short, quiet section label above a group of content.
pub(crate) fn eyebrow(text: impl Into<SharedString>, cx: &App) -> Quiet {
    Quiet::new(text, cx.theme().semantic_tokens().typography.xs).semibold()
}

/// Small monospace metadata (durations, counts, identifiers).
pub(crate) fn meta(text: impl Into<SharedString>, cx: &App) -> Quiet {
    Quiet::new(text, cx.theme().semantic_tokens().typography.xs)
        .family(cx.theme().mono_font_family.clone())
}

/// The library's one disclosure affordance: a chevron that rotates open.
///
/// Takes the same disclosure sample the body's height and fade are drawn
/// from, so the glyph and the panel can never disagree about how open the
/// thing is — a swapped pair of icons can only ever be fully one or the
/// other, and would snap while the panel was still moving.
pub(crate) fn disclosure_chevron(disclosure: f32) -> gpui_component::Icon {
    gpui_component::Sizable::xsmall(gpui_component::Icon::new(
        gpui_component::IconName::ChevronRight,
    ))
    .rotate(gpui::percentage(0.25 * disclosure))
}

/// Quiet supporting text: extra-small, muted.
///
/// The most-used text role in the library, and the one that had no name —
/// a domain beside a title, a count beside a label, a hint under a field.
/// [`meta`] is its monospace sibling for values a reader may compare.
pub(crate) fn hint(text: impl Into<SharedString>, cx: &App) -> Quiet {
    Quiet::new(text, cx.theme().semantic_tokens().typography.xs)
}

/// A heading inside a card: small, semibold, in the foreground ink.
///
/// Distinct from [`title`], which is a card's own name at the medium size;
/// this is the heading of a section within one.
pub(crate) fn subtitle(text: impl Into<SharedString>, cx: &App) -> Div {
    let tokens = cx.theme().semantic_tokens();
    div()
        .text_token(tokens.typography.sm)
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

/// The trailing unit on a numeric field — "px", "%", "ms".
///
/// A bare string handed to an input's suffix inherits the field's own ink
/// and size, so the unit reads as part of the number. This gives every
/// unit in the library one quiet voice instead.
pub(crate) fn field_unit(unit: impl Into<SharedString>, cx: &App) -> Quiet {
    hint(unit, cx).flex_none()
}

/// A favicon-style badge: one uppercase initial on a primary tint. Sources,
/// search results, and attachments share it so provenance scans at a glance.
pub(crate) fn initial_badge(initial: impl Into<SharedString>, cx: &App) -> Div {
    let tokens = cx.theme().semantic_tokens();
    div()
        .flex_none()
        // The box is the type's own line box, from the size policy's slot
        // scale — a fixed pixel square held rem-scaled glyphs, so a larger
        // type scale pushed the letter out of its own circle.
        .size(crate::sizing::slot_sm(cx))
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .rounded(tokens.radius.sm)
        .bg(cx.theme().primary.opacity(0.14))
        .text_token(tokens.typography.xs)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().primary)
        .child(initial.into())
}

/// First alphanumeric character of `text`, uppercased, for [`initial_badge`].
pub(crate) fn initial_of(text: &str) -> String {
    text.chars()
        .find(|character| character.is_alphanumeric())
        .map(|character| character.to_uppercase().collect())
        .unwrap_or_else(|| "•".to_owned())
}

/// A quiet, square icon-only button with an accessible name.
///
/// Rests muted, lifts to the foreground on hover, shows the theme ring on
/// keyboard focus, and names itself on hover. Used for message actions and
/// card toolbars.
pub(crate) fn icon_button(
    id: impl Into<ElementId>,
    icon: impl IconNamed,
    accessibility_label: impl Into<SharedString>,
    window: &mut gpui::Window,
    cx: &mut App,
) -> Button {
    icon_button_turned(id, icon, 0.0, accessibility_label, window, cx)
}

/// [`icon_button`] whose glyph is rotated by `turns` of a full circle.
///
/// A disclosure control is an icon button that also has to point somewhere,
/// and its rotation composes with the press compression rather than
/// replacing it — one transform on one glyph, so a control being pressed
/// mid-open keeps both.
pub(crate) fn icon_button_turned(
    id: impl Into<ElementId>,
    icon: impl IconNamed,
    turns: f32,
    accessibility_label: impl Into<SharedString>,
    window: &mut gpui::Window,
    cx: &mut App,
) -> Button {
    let tokens = cx.theme().semantic_tokens();
    let id = id.into();
    // One read of the press ramp, shared by the tint and the glyph.
    let (pressed, fade) = crate::control::press_release_state(&id, window, cx);
    let name = accessibility_label.into();
    // The name it already carries, made visible. A control with no label is
    // only guessable from its glyph, and the guess is wrong often enough that
    // every desktop application shows the name on hover - so the screen reader
    // and the pointer are told the same thing, from one argument, and no
    // caller has to remember to say it twice.
    let tooltip = name.clone();
    composed_button(id.clone(), name)
        .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(crate::sizing::control_sm(cx))
        .rounded(tokens.radius.sm)
        .border_1()
        .border_color(cx.theme().transparent)
        .text_color(cx.theme().muted_foreground)
        .hover(|style| {
            style
                .bg(cx.theme().accent.opacity(0.6))
                .text_color(cx.theme().accent_foreground)
        })
        .active(|style| style.bg(cx.theme().accent))
        .focus_visible(|style| style.border_color(cx.theme().ring))
        // A toggle that reports selected also shows it: the accent fill
        // stays while selected, so state stops being invisible.
        .styles(|styles| {
            styles.selected(|style| {
                style
                    .bg(cx.theme().accent)
                    .text_color(cx.theme().accent_foreground)
            })
        })
        .press_release(id.clone(), tokens.radius.sm, window, cx)
        .child(Icon::new(icon).xsmall().transform({
            // The glyph compresses while pressed and eases back on the
            // same release clock as the tint — SVG transforms are free,
            // so the compression costs no extra frames.
            let intensity = if pressed { 1.0 } else { fade };
            let scale = 1.0 - 0.03 * intensity;
            gpui::Transformation::rotate(gpui::percentage(turns))
                .with_scaling(gpui::size(scale, scale))
        }))
}

#[cfg(test)]
mod tests {
    use super::nested_radius;
    use gpui::px;

    #[test]
    fn the_nesting_rule_subtracts_the_inset_and_floors() {
        assert_eq!(nested_radius(px(8.), px(4.), px(3.)), px(4.));
        assert_eq!(nested_radius(px(8.), px(6.), px(3.)), px(3.), "floored");
        assert_eq!(
            nested_radius(px(6.), px(8.), px(3.)),
            px(3.),
            "inset past the corner"
        );
        assert_eq!(nested_radius(px(10.), px(2.), px(3.)), px(8.));
    }
}

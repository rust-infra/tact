//! Chip and pill geometry: the optical rules the 0.4.0 feel review found
//! broken by eye and no automated gate could see.
//!
//! Rasters are how a reader judges a pill, but bounds are how a test can.
//! These measure what the reader is reacting to: whether a pill sizes to
//! its own label, whether the space left of the text matches the space
//! right of it, and whether the lifecycle dot sits on the label's centre
//! line rather than near it.

use gpui::{
    Bounds, Context, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render,
    Styled as _, TestAppContext, VisualTestContext, Window, div, px,
};
use gpui_ai::{
    prelude::{Suggestion, Suggestions},
    status::{StatusBadge, StatusTone},
    stream::ProgressState,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

struct ChipProbe;

impl Render for ChipProbe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        v_flex()
            .w(px(560.))
            .p(px(16.))
            .gap(tokens.spacing.md)
            // Lifecycle badges: the shortest and the longest label the
            // lifecycle can produce, side by side.
            .child(
                h_flex()
                    .gap(tokens.spacing.sm)
                    .child(div().debug_selector(|| "probe-badge-failed".into()).child(
                        StatusBadge::for_progress(
                            "probe-failed",
                            &ProgressState::Failed("offline".into()),
                        ),
                    ))
                    .child(
                        div()
                            .debug_selector(|| "probe-badge-complete".into())
                            .child(StatusBadge::for_progress(
                                "probe-complete",
                                &ProgressState::Complete,
                            )),
                    )
                    .child(div().debug_selector(|| "probe-badge-plain".into()).child(
                        StatusBadge::new("probe-plain", "Needs review").tone(StatusTone::Warning),
                    )),
            )
            .child(
                div().child(
                    Suggestions::new("probe-suggestions")
                        .items([
                            Suggestion::new("short", "Go"),
                            Suggestion::new("long", "Compare supplier prices"),
                        ])
                        .on_event(|_, _, _| {}),
                ),
            )
    }
}

fn probe(cx: &mut TestAppContext) -> &mut VisualTestContext {
    cx.update(gpui_ai::init);
    let (_, cx) = cx.add_window_view(|_, _| ChipProbe);
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx
}

fn bounds(cx: &mut VisualTestContext, selector: &str) -> Bounds<Pixels> {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
        .unwrap_or_else(|| panic!("{selector} should render"))
}

/// A pill is as wide as what it says. Reserving the widest lifecycle
/// label's width for every badge is what left "Failed" adrift in a
/// "Completed"-sized pill, with the longest label crowding its own right
/// edge — the feel review's first finding.
#[gpui::test]
fn lifecycle_badges_size_to_their_own_label(cx: &mut TestAppContext) {
    let cx = probe(cx);
    let failed = bounds(cx, "probe-badge-failed");
    let complete = bounds(cx, "probe-badge-complete");
    assert!(
        failed.size.width < complete.size.width,
        "a short lifecycle label must not be padded out to the longest one: \
         Failed {:?} vs Completed {:?}",
        failed.size.width,
        complete.size.width
    );
}

/// The space before a pill's text and the space after it are the same
/// space. Any pill whose label sits in a slot wider than itself breaks
/// this, and it reads as a missing right padding.
#[gpui::test]
fn pill_text_is_evenly_inset_from_both_ends(cx: &mut TestAppContext) {
    let cx = probe(cx);
    for (chip, label) in [
        ("probe-badge-complete", "status-badge-label-Completed"),
        ("probe-badge-failed", "status-badge-label-Failed"),
        ("probe-badge-plain", "status-badge-label-Needs review"),
    ] {
        let chip = bounds(cx, chip);
        let label = bounds(cx, label);
        let trailing = chip.right() - label.right();
        assert!(
            trailing >= px(6.) && trailing <= px(10.),
            "{label:?}: a pill's trailing inset must match its leading one, \
             found {trailing:?}"
        );
    }
}

/// The lifecycle dot sits on the label's centre line. A slot sized to
/// anything but the label's own line box centres the dot against a box
/// the reader cannot see, and the dot reads as high or low.
#[gpui::test]
fn the_lifecycle_dot_shares_the_label_centre_line(cx: &mut TestAppContext) {
    let cx = probe(cx);
    let dot = bounds(cx, "status-badge-indicator-Completed");
    let label = bounds(cx, "status-badge-label-Completed");
    let drift = dot.center().y - label.center().y;
    assert!(
        drift.abs() <= px(0.5),
        "the dot must ride the label's centre line, drifted {drift:?}"
    );
}

/// The space before a badge's dot matches the space after its label.
///
/// The dot rides a fixed slot sized for the spinner that replaces it, so
/// the slot is wider than the dot — and the chip's own leading padding
/// sat outside that slack, leaving visibly more room before the dot than
/// after the word.
#[gpui::test]
fn a_badge_insets_its_dot_and_its_label_alike(cx: &mut TestAppContext) {
    let cx = probe(cx);
    let chip = bounds(cx, "probe-badge-complete");
    let dot = bounds(cx, "status-badge-dot-Completed");
    let label = bounds(cx, "status-badge-label-Completed");
    let leading = dot.left() - chip.left();
    let trailing = chip.right() - label.right();
    assert!(
        (leading - trailing).abs() <= px(1.),
        "a badge's dot and its label stand equally far from their own ends:          {leading:?} before, {trailing:?} after"
    );
}

/// A pill leaves as much room above its text as below it.
#[gpui::test]
fn suggestion_pills_inset_their_text_evenly_top_and_bottom(cx: &mut TestAppContext) {
    let cx = probe(cx);
    let pill = bounds(cx, "suggestion-long");
    let label = bounds(cx, "suggestion-label-long");
    let above = label.top() - pill.top();
    let below = pill.bottom() - label.bottom();
    assert!(
        (above - below).abs() <= px(0.5),
        "a pill's text must sit between equal space: {above:?} above, {below:?} below"
    );
}

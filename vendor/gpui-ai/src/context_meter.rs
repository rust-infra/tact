//! Context-window usage: how much of the model's window a conversation has
//! consumed, as a ring, a bar, or plain text, with a hover breakdown.
//!
//! The meter changes tone as the window fills — calm below 65%, warning to
//! 85%, danger above — and exposes the same numbers semantically so the
//! level never depends on color alone.

use crate::motion::MotionTokens;
use crate::theme::SemanticStyledExt as _;
use gpui::{
    App, ElementId, FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, Role, SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled,
    Window, div, prelude::FluentBuilder as _, relative,
};
use gpui_base::motion::{Transition, transition};
use gpui_component::{
    ActiveTheme as _, Sizable as _, StyledExt as _, h_flex, hover_card::HoverCard,
    progress::ProgressCircle, v_flex,
};

/// Fraction of the window at which the meter turns to the warning tone.
const ELEVATED_AT: f32 = 0.65;
/// Fraction of the window at which the meter turns to the danger tone.
const CRITICAL_AT: f32 = 0.85;

/// How full the context window is, in the three bands people act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageLevel {
    /// Plenty of room.
    Comfortable,
    /// Worth keeping an eye on.
    Elevated,
    /// Close to the limit; the application should offer a remedy.
    Critical,
}

/// Application-measured token usage for one context window.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContextUsage {
    used: u64,
    limit: u64,
    input: Option<u64>,
    output: Option<u64>,
    reasoning: Option<u64>,
    cached: Option<u64>,
    cost: Option<SharedString>,
}

impl ContextUsage {
    /// Creates a usage snapshot from consumed tokens and the window size.
    pub fn new(used: u64, limit: u64) -> Self {
        Self {
            used,
            limit,
            ..Self::default()
        }
    }

    /// Records input (prompt) tokens for the breakdown.
    pub fn input(mut self, tokens: u64) -> Self {
        self.input = Some(tokens);
        self
    }

    /// Records output (completion) tokens for the breakdown.
    pub fn output(mut self, tokens: u64) -> Self {
        self.output = Some(tokens);
        self
    }

    /// Records reasoning tokens for the breakdown.
    pub fn reasoning(mut self, tokens: u64) -> Self {
        self.reasoning = Some(tokens);
        self
    }

    /// Records cached (prompt-cache hit) tokens for the breakdown.
    pub fn cached(mut self, tokens: u64) -> Self {
        self.cached = Some(tokens);
        self
    }

    /// Records an application-formatted cost line (for example `$0.42`).
    pub fn cost(mut self, cost: impl Into<SharedString>) -> Self {
        self.cost = Some(cost.into());
        self
    }

    /// Used tokens as a fraction of the limit, clamped to `0.0..=1.0`.
    pub fn fraction(&self) -> f32 {
        if self.limit == 0 {
            return 0.0;
        }
        (self.used as f64 / self.limit as f64).clamp(0.0, 1.0) as f32
    }

    /// Used tokens as a whole percentage.
    pub fn percent(&self) -> u8 {
        (self.fraction() * 100.0).round() as u8
    }

    /// The band the current usage falls in.
    pub fn level(&self) -> UsageLevel {
        let fraction = self.fraction();
        if fraction >= CRITICAL_AT {
            UsageLevel::Critical
        } else if fraction >= ELEVATED_AT {
            UsageLevel::Elevated
        } else {
            UsageLevel::Comfortable
        }
    }

    /// Returns the consumed token count.
    pub fn used(&self) -> u64 {
        self.used
    }

    /// Returns the window size in tokens.
    pub fn limit(&self) -> u64 {
        self.limit
    }

    fn has_breakdown(&self) -> bool {
        self.input.is_some()
            || self.output.is_some()
            || self.reasoning.is_some()
            || self.cached.is_some()
            || self.cost.is_some()
    }

    fn summary(&self) -> String {
        format!(
            "{} of {} tokens used, {}%",
            format_tokens(self.used),
            format_tokens(self.limit),
            self.percent()
        )
    }
}

/// Formats a token count compactly: `912`, `84.3K`, `1.2M`.
pub fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        let millions = tokens as f64 / 1_000_000.0;
        if millions >= 10.0 {
            format!("{millions:.0}M")
        } else {
            format!("{millions:.1}M")
        }
    } else if tokens >= 1_000 {
        let thousands = tokens as f64 / 1_000.0;
        if thousands >= 100.0 {
            format!("{thousands:.0}K")
        } else {
            format!("{thousands:.1}K")
        }
    } else {
        tokens.to_string()
    }
}

/// How the meter draws its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContextMeterVariant {
    /// A small ring next to the percentage.
    #[default]
    Ring,
    /// A horizontal bar with the percentage.
    Bar,
    /// Text only: `42% · 84.3K / 200K`.
    Text,
}

/// A context-window usage readout.
///
/// # Example
///
/// ```
/// # use gpui_ai::prelude::*;
/// ContextMeter::new(
///     "context",
///     &ContextUsage::new(84_300, 200_000)
///         .input(61_000)
///         .output(19_800)
///         .cached(3_500)
///         .cost("$0.42"),
/// )
/// .variant(ContextMeterVariant::Ring);
/// ```
#[derive(IntoElement)]
pub struct ContextMeter {
    id: ElementId,
    style: StyleRefinement,
    usage: ContextUsage,
    variant: ContextMeterVariant,
    label: SharedString,
}

impl ContextMeter {
    /// Creates a meter from a usage snapshot.
    pub fn new(id: impl Into<ElementId>, usage: &ContextUsage) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            usage: usage.clone(),
            variant: ContextMeterVariant::default(),
            label: "Context usage".into(),
        }
    }

    /// Sets how the value is drawn (default: ring).
    pub fn variant(mut self, variant: ContextMeterVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Overrides the accessible name (default "Context usage").
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }

    fn tone(&self, cx: &App) -> Hsla {
        match self.usage.level() {
            UsageLevel::Comfortable => cx.theme().primary,
            UsageLevel::Elevated => cx.theme().warning,
            UsageLevel::Critical => cx.theme().danger,
        }
    }
}

impl Styled for ContextMeter {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

fn breakdown_row(label: &'static str, value: String, cx: &App) -> impl IntoElement {
    let tokens = cx.theme().semantic_tokens();
    h_flex()
        .w_full()
        .justify_between()
        .gap(tokens.spacing.lg)
        .text_token(tokens.typography.xs)
        // The label yields and the value holds. A row that states something
        // and its number has one part that can be shortened without losing
        // the point and one that cannot: a truncated heading still leaves a
        // readable row, while a dropped number leaves a row that says
        // nothing. Neither could shrink before, so a long label pushed the
        // number out of the row, where it was clipped.
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(
            div()
                .flex_none()
                .font_family(cx.theme().mono_font_family.clone())
                .text_color(cx.theme().foreground)
                .child(value),
        )
}

impl RenderOnce for ContextMeter {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        let color = self.tone(cx);
        let percent = self.usage.percent();
        // The drawn geometry retargets from the previous controlled usage at
        // the standard tempo; the first render starts at the supplied value,
        // and the numeric readout stays controlled — the number never
        // counts. Reduced motion snaps through the transition contract.
        let fraction = transition(
            (self.id.clone(), "context-fill"),
            self.usage.fraction(),
            Transition::new(crate::motion::retarget_duration(
                cx,
                MotionTokens::read(cx).standard(),
            ))
            .ease(crate::motion::ease_in_out_quart),
            window,
            cx,
        );
        let summary: SharedString = self.usage.summary().into();
        let percent_text = format!("{percent}%");
        let ratio_text = format!(
            "{} / {}",
            format_tokens(self.usage.used),
            format_tokens(self.usage.limit)
        );
        let root_id = self.id.clone();

        let readout = match self.variant {
            ContextMeterVariant::Ring => h_flex()
                .items_center()
                .gap(tokens.spacing.xs)
                .child(
                    ProgressCircle::new((root_id.clone(), "ring"))
                        .value(fraction * 100.0)
                        .color(color)
                        .large(),
                )
                .child(
                    div()
                        .text_token(tokens.typography.xs)
                        .font_family(cx.theme().mono_font_family.clone())
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(cx.theme().foreground)
                        .child(percent_text),
                )
                .into_any_element(),
            ContextMeterVariant::Bar => h_flex()
                .items_center()
                .gap(tokens.spacing.sm)
                .child(
                    div()
                        .w(tokens.spacing.xxl * 3.0)
                        .h(tokens.spacing.xs)
                        .rounded(tokens.radius.full)
                        .bg(cx.theme().muted)
                        .child(
                            div()
                                .h_full()
                                .w(relative(fraction))
                                .rounded(tokens.radius.full)
                                .bg(color),
                        ),
                )
                .child(
                    div()
                        .text_token(tokens.typography.xs)
                        .font_family(cx.theme().mono_font_family.clone())
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(cx.theme().foreground)
                        .child(percent_text),
                )
                .into_any_element(),
            ContextMeterVariant::Text => h_flex()
                .items_center()
                .gap(tokens.spacing.xs)
                .text_token(tokens.typography.xs)
                .font_family(cx.theme().mono_font_family.clone())
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(color)
                        .child(percent_text),
                )
                .child(div().text_color(cx.theme().muted_foreground).child("·"))
                .child(
                    div()
                        .text_color(cx.theme().muted_foreground)
                        .child(ratio_text.clone()),
                )
                .into_any_element(),
        };

        let visual = h_flex()
            .id((root_id.clone(), "meter"))
            .flex_none()
            .items_center()
            .px(tokens.spacing.xs)
            .py(tokens.spacing.xxs)
            .rounded(tokens.radius.sm)
            .hover(|style| style.bg(cx.theme().accent.opacity(0.6)))
            .child(readout);

        let content = if self.usage.has_breakdown() {
            let usage = self.usage.clone();
            let label = self.label.clone();
            // The breakdown is data the application already holds, so it
            // reveals rather than loads: the crate's popup policy sets the
            // wait, where upstream's own default is 600ms.
            let popup = crate::popup::PopupTokens::read(cx);
            HoverCard::new((root_id.clone(), "breakdown"))
                .open_delay(popup.hover_open_delay())
                .close_delay(popup.hover_close_delay())
                .trigger(visual)
                .content(move |_, _, cx| {
                    let tokens = cx.theme().semantic_tokens();
                    v_flex()
                        .gap(tokens.spacing.xs)
                        .min_w(tokens.spacing.xxl * 5.0)
                        .child(crate::surface::subtitle(label.clone(), cx))
                        .child(
                            div()
                                .h(tokens.spacing.xs)
                                .w_full()
                                .rounded(tokens.radius.full)
                                .bg(cx.theme().muted)
                                .child(
                                    div()
                                        .h_full()
                                        .w(relative(usage.fraction()))
                                        .rounded(tokens.radius.full)
                                        .bg(match usage.level() {
                                            UsageLevel::Comfortable => cx.theme().primary,
                                            UsageLevel::Elevated => cx.theme().warning,
                                            UsageLevel::Critical => cx.theme().danger,
                                        }),
                                ),
                        )
                        .child(breakdown_row(
                            "Used",
                            format!(
                                "{} / {}",
                                format_tokens(usage.used),
                                format_tokens(usage.limit)
                            ),
                            cx,
                        ))
                        .when_some(usage.input, |this, input| {
                            this.child(breakdown_row("Input", format_tokens(input), cx))
                        })
                        .when_some(usage.output, |this, output| {
                            this.child(breakdown_row("Output", format_tokens(output), cx))
                        })
                        .when_some(usage.reasoning, |this, reasoning| {
                            this.child(breakdown_row("Reasoning", format_tokens(reasoning), cx))
                        })
                        .when_some(usage.cached, |this, cached| {
                            this.child(breakdown_row("Cached", format_tokens(cached), cx))
                        })
                        .when_some(usage.cost.clone(), |this, cost| {
                            this.child(breakdown_row("Cost", cost.to_string(), cx))
                        })
                })
                .into_any_element()
        } else {
            visual.into_any_element()
        };

        // The semantic root stays one named progress indicator regardless of
        // whether a hover breakdown wraps the visual.
        div()
            .id(root_id)
            .role(Role::ProgressIndicator)
            .aria_label(self.label.clone())
            .aria_description(summary)
            .aria_min_numeric_value(0.)
            .aria_max_numeric_value(100.)
            .aria_numeric_value(f64::from(percent))
            .flex_none()
            .child(content)
            .refine_style(&self.style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_counts_format_compactly() {
        assert_eq!(format_tokens(912), "912");
        assert_eq!(format_tokens(1_000), "1.0K");
        assert_eq!(format_tokens(84_300), "84.3K");
        assert_eq!(format_tokens(200_000), "200K");
        assert_eq!(format_tokens(1_200_000), "1.2M");
        assert_eq!(format_tokens(12_000_000), "12M");
    }

    #[test]
    fn fraction_is_clamped_and_levels_follow_the_thresholds() {
        assert_eq!(ContextUsage::new(0, 0).fraction(), 0.0);
        assert_eq!(ContextUsage::new(500, 100).fraction(), 1.0);
        assert_eq!(ContextUsage::new(50, 100).level(), UsageLevel::Comfortable);
        assert_eq!(ContextUsage::new(65, 100).level(), UsageLevel::Elevated);
        assert_eq!(ContextUsage::new(84, 100).level(), UsageLevel::Elevated);
        assert_eq!(ContextUsage::new(85, 100).level(), UsageLevel::Critical);
        assert_eq!(ContextUsage::new(84_300, 200_000).percent(), 42);
    }
}

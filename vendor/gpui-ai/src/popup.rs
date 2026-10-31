//! Where transient surfaces open, how soon, and what edge they draw.
//!
//! Dropdowns, menus, hover cards, and toolbars all float above the page,
//! and a reader reads them as one family or as none. This module owns the
//! three things that make them a family: the side they prefer to open on,
//! how long a hover waits before one appears, and the edge that separates
//! a floating surface from the content behind it.
//!
//! Placement is a *preference*, not a command. Every surface is positioned
//! through the shared positioner, which flips to the opposite side when the
//! preferred one does not fit and clamps to the window when neither does —
//! so a composer at the bottom of a window opens its menu upward without
//! the caller arranging it. [`PopupSide::Auto`] simply states that
//! preference explicitly.
//!
//! # Example
//!
//! ```no_run
//! # fn example(cx: &mut gpui::App) {
//! gpui_ai::init(cx);
//! gpui_ai::popup::PopupTokens::default()
//!     .with_side(gpui_ai::popup::PopupSide::Above)
//!     .set(cx);
//! # }
//! ```

use std::time::Duration;

use gpui::{App, Global};
use gpui_base::Placement;
use gpui_component::ActiveTheme as _;

/// The side a floating surface prefers to open on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PopupSide {
    /// Below the trigger, flipping above when it does not fit.
    #[default]
    Below,
    /// Above the trigger, flipping below when it does not fit.
    Above,
    /// Whichever side has more room, decided per opening.
    Auto,
}

impl PopupSide {
    /// The placement to hand the positioner for a trigger at `anchor_top`
    /// in a window `viewport_height` tall.
    ///
    /// [`Auto`](Self::Auto) resolves by which side of the trigger has more
    /// room; the two fixed sides state a preference the positioner is
    /// still free to flip when it does not fit.
    pub fn placement(self, anchor_top: gpui::Pixels, viewport_height: gpui::Pixels) -> Placement {
        match self {
            Self::Below => Placement::Bottom,
            Self::Above => Placement::Top,
            Self::Auto => {
                if anchor_top > viewport_height * 0.5 {
                    Placement::Top
                } else {
                    Placement::Bottom
                }
            }
        }
    }
}

/// The crate's policy for floating surfaces.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PopupTokens {
    side: PopupSide,
    hover_open_delay: Duration,
    hover_close_delay: Duration,
}

impl Global for PopupTokens {}

impl Default for PopupTokens {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl PopupTokens {
    /// The crate's default: open below with a smart flip, and show a
    /// hover surface almost at once.
    ///
    /// Upstream waits 600ms before a hover card appears, which reads as
    /// the data being fetched rather than revealed. A hover surface over
    /// data the application already holds should feel like a disclosure,
    /// so the default is the quick tempo's own delay.
    pub const DEFAULT: Self = Self {
        side: PopupSide::Below,
        hover_open_delay: Duration::from_millis(100),
        hover_close_delay: Duration::from_millis(200),
    };

    /// The policy the application installed, or the crate's default.
    pub fn read(cx: &App) -> Self {
        cx.try_global::<Self>().copied().unwrap_or(Self::DEFAULT)
    }

    /// Makes this policy the application's, for every floating surface.
    pub fn set(self, cx: &mut App) {
        cx.set_global(self);
    }

    /// The side floating surfaces prefer.
    pub const fn side(&self) -> PopupSide {
        self.side
    }

    /// How long a pointer rests before a hover surface opens.
    pub const fn hover_open_delay(&self) -> Duration {
        self.hover_open_delay
    }

    /// How long a hover surface waits before closing.
    pub const fn hover_close_delay(&self) -> Duration {
        self.hover_close_delay
    }

    /// Replaces the [`side`](Self::side).
    pub const fn with_side(mut self, side: PopupSide) -> Self {
        self.side = side;
        self
    }

    /// Replaces the [`hover_open_delay`](Self::hover_open_delay).
    pub const fn with_hover_open_delay(mut self, delay: Duration) -> Self {
        self.hover_open_delay = delay;
        self
    }

    /// Replaces the [`hover_close_delay`](Self::hover_close_delay).
    pub const fn with_hover_close_delay(mut self, delay: Duration) -> Self {
        self.hover_close_delay = delay;
        self
    }
}

/// Installs the crate's default popup policy unless the application chose
/// one first.
pub(crate) fn install(cx: &mut App) {
    if !cx.has_global::<PopupTokens>() {
        PopupTokens::DEFAULT.set(cx);
    }
}

/// The surface every floating panel draws.
///
/// Upstream's popover style carries a shadow and the shadow's own ring,
/// which reads clearly on a light page and nearly disappears on a dark
/// one — the 0.4.0 feel review asked for a visible edge. This keeps the
/// shadow and states the border, so a floating panel is separable from
/// whatever it covers on any theme.
pub(crate) fn popover_surface<E>(surface: E, cx: &App) -> E
where
    E: gpui_component::ThemeStyled,
{
    surface
        .popover_style(cx)
        .border_1()
        .border_color(cx.theme().border)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, px};

    /// A surface near the bottom of the window opens upward without the
    /// caller arranging it; one near the top opens downward.
    #[test]
    fn auto_opens_away_from_the_nearer_edge() {
        let viewport = px(800.);
        assert_eq!(
            PopupSide::Auto.placement(px(700.), viewport),
            Placement::Top
        );
        assert_eq!(
            PopupSide::Auto.placement(px(100.), viewport),
            Placement::Bottom
        );
    }

    /// Hover surfaces open at the crate's tempo, not upstream's 600ms.
    #[gpui::test]
    fn hover_surfaces_open_promptly(cx: &mut TestAppContext) {
        cx.update(crate::init);
        cx.update(|cx| {
            assert!(PopupTokens::read(cx).hover_open_delay() <= Duration::from_millis(150));
        });
    }

    /// The policy is a policy: an application that chooses first keeps it.
    #[gpui::test]
    fn install_respects_a_policy_the_application_chose_first(cx: &mut TestAppContext) {
        cx.update(|cx| {
            PopupTokens::default().with_side(PopupSide::Above).set(cx);
            crate::init(cx);
        });
        cx.update(|cx| assert_eq!(PopupTokens::read(cx).side(), PopupSide::Above));
    }
}

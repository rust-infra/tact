//! Responsive three-column layout decisions.

pub const RAIL_BREAKPOINT: f32 = 860.0;
pub const INSPECTOR_BREAKPOINT: f32 = 1180.0;
pub const THREE_COLUMN_BREAKPOINT: f32 = INSPECTOR_BREAKPOINT;
pub const RAIL_WIDTH: f32 = 264.0;
pub const INSPECTOR_WIDTH: f32 = 300.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShellLayout {
    pub width: f32,
    pub text_scale: f32,
    pub three_column: bool,
    pub rail_inline: bool,
    pub inspector_inline: bool,
    pub rail_overlay: bool,
    pub inspector_overlay: bool,
    pub content_width: f32,
}

pub fn layout(width: f32, text_scale: f32, rail_open: bool, inspector_open: bool) -> ShellLayout {
    let scale = text_scale.clamp(1.0, 1.5);
    let rail_inline = rail_open && width >= RAIL_BREAKPOINT * scale;
    let inspector_inline = inspector_open && width >= INSPECTOR_BREAKPOINT * scale;
    let three_column = rail_inline && inspector_inline;
    let mut content_width = width.max(0.0);
    if rail_inline {
        content_width = (content_width - RAIL_WIDTH).max(0.0);
    }
    if inspector_inline {
        content_width = (content_width - INSPECTOR_WIDTH).max(0.0);
    }
    ShellLayout {
        width,
        text_scale: scale,
        three_column,
        rail_inline,
        inspector_inline,
        rail_overlay: rail_open && !rail_inline,
        inspector_overlay: inspector_open && !inspector_inline,
        content_width,
    }
}

pub fn usable_text_scale(text_scale: f32) -> bool {
    (1.0..=1.5).contains(&text_scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_window_moves_panels_to_scrimmed_overlays() {
        let narrow = layout(820.0, 1.0, true, true);
        assert!(!narrow.three_column);
        assert!(!narrow.rail_inline);
        assert!(!narrow.inspector_inline);
        assert!(narrow.rail_overlay);
        assert!(narrow.inspector_overlay);

        let middle = layout(1100.0, 1.0, true, true);
        assert!(!middle.three_column);
        assert!(middle.rail_inline);
        assert!(!middle.inspector_inline);
        assert!(!middle.rail_overlay);
        assert!(middle.inspector_overlay);

        let roomy = layout(1280.0, 1.0, true, true);
        assert!(roomy.rail_inline);
        assert!(roomy.inspector_inline);
        assert!(!roomy.rail_overlay);
        assert!(!roomy.inspector_overlay);
    }

    #[test]
    fn text_scale_125_and_150_remain_usable() {
        for scale in [1.25, 1.5] {
            assert!(usable_text_scale(scale));
            let breakpoint = THREE_COLUMN_BREAKPOINT * scale;
            let just_narrow = layout(breakpoint - 1.0, scale, true, true);
            assert_eq!(just_narrow.text_scale, scale);
            assert!(!just_narrow.three_column);
            assert!(just_narrow.rail_inline);
            assert!(just_narrow.inspector_overlay);
            assert_eq!(just_narrow.content_width, breakpoint - 1.0 - RAIL_WIDTH);

            let just_roomy = layout(breakpoint, scale, true, true);
            assert!(just_roomy.three_column);
            assert!(just_roomy.rail_inline);
            assert!(just_roomy.inspector_inline);
            assert!(!just_roomy.rail_overlay);
            assert!(!just_roomy.inspector_overlay);
            assert_eq!(
                just_roomy.content_width,
                breakpoint - RAIL_WIDTH - INSPECTOR_WIDTH
            );
        }
    }
}

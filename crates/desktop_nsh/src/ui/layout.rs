//! Responsive three-column layout decisions.

/// Window width at which the session rail can stay inline at 100% text.
///
/// Breakpoints are window widths, not text widths: a larger text scale must
/// not push the rail into an overlay on a window that still has room for it.
pub const RAIL_BREAKPOINT: f32 = 860.0;
pub const INSPECTOR_BREAKPOINT: f32 = 1180.0;
pub const THREE_COLUMN_BREAKPOINT: f32 = INSPECTOR_BREAKPOINT;
pub const RAIL_WIDTH: f32 = 250.0;
pub const INSPECTOR_WIDTH: f32 = 318.0;

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

impl ShellLayout {
    /// The rail's rendered width at this text scale.
    pub fn rail_width(&self) -> f32 {
        RAIL_WIDTH * self.text_scale
    }

    /// The inspector's rendered width at this text scale.
    pub fn inspector_width(&self) -> f32 {
        INSPECTOR_WIDTH * self.text_scale
    }
}

pub fn layout(width: f32, text_scale: f32, rail_open: bool, inspector_open: bool) -> ShellLayout {
    let scale = text_scale.clamp(1.0, 1.5);
    let rail_inline = rail_open && width >= RAIL_BREAKPOINT;
    // The inspector is the first panel to become an overlay: at 125% and 150%
    // it would leave the transcript too narrow, so it needs proportionally more
    // window before it may stay inline.
    let inspector_inline = inspector_open && width >= INSPECTOR_BREAKPOINT * scale;
    let three_column = rail_inline && inspector_inline;
    let mut content_width = width.max(0.0);
    if rail_inline {
        content_width = (content_width - RAIL_WIDTH * scale).max(0.0);
    }
    if inspector_inline {
        content_width = (content_width - INSPECTOR_WIDTH * scale).max(0.0);
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



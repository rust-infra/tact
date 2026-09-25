use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};
use unicode_width::UnicodeWidthStr;

/// Semantic visual variant for a compact, text-based TUI button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonVariant {
    /// Primary action, emphasized with the theme accent.
    Primary,
    /// Low-emphasis action, suitable for inline affordances such as Copy.
    #[default]
    Secondary,
    /// Caution: reversible but consequential (dropping queued work, say).
    Warning,
    /// Destructive action, emphasized with the theme error color.
    Danger,
}

/// Frame drawn around the label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonChrome {
    /// ` label ` — spacing only.
    #[default]
    Plain,
    /// `[label]` — the classic TUI affordance. Horizontal padding does not
    /// apply; the brackets are the padding.
    Brackets,
}

/// Interaction state is independent of a button's semantic variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonState {
    /// Available but not focused.
    #[default]
    Normal,
    /// Keyboard focus is on this button.
    Focused,
    /// Pointer/key press feedback.
    Pressed,
    /// The action just succeeded; the host shows this briefly, then reverts.
    Success,
    /// Visible but not actionable.
    Disabled,
}

/// Theme colors needed to render a [`Button`].
#[derive(Debug, Clone, Copy)]
pub struct ButtonTheme {
    pub fg: ratatui::style::Color,
    pub bg: ratatui::style::Color,
    pub accent: ratatui::style::Color,
    pub muted: ratatui::style::Color,
    pub warning: ratatui::style::Color,
    pub error: ratatui::style::Color,
    pub success: ratatui::style::Color,
}

impl ButtonTheme {
    /// Adapt the kit's [`Theme`] to a button palette.
    ///
    /// [`Theme`]: crate::theme::Theme
    pub fn from_theme(theme: &crate::theme::Theme) -> Self {
        Self {
            fg: theme.fg,
            bg: theme.bg,
            accent: theme.accent,
            muted: theme.muted,
            warning: theme.warning,
            error: theme.error,
            success: theme.success,
        }
    }

    /// Repaint the button on another surface (input box, tool card, popup):
    /// the button fills its whole rect with `bg`.
    pub fn with_bg(mut self, bg: ratatui::style::Color) -> Self {
        self.bg = bg;
        self
    }
}

/// Compact reusable text button. It only renders; the host owns focus,
/// activation, mouse routing, and the action itself.
#[derive(Debug, Clone, Copy)]
pub struct Button<'a> {
    label: &'a str,
    variant: ButtonVariant,
    state: ButtonState,
    theme: ButtonTheme,
    horizontal_padding: u16,
    modifier: Modifier,
    chrome: ButtonChrome,
}

impl<'a> Button<'a> {
    pub fn new(label: &'a str, theme: ButtonTheme) -> Self {
        Self {
            label,
            variant: ButtonVariant::Secondary,
            state: ButtonState::Normal,
            theme,
            horizontal_padding: 1,
            modifier: Modifier::empty(),
            chrome: ButtonChrome::Plain,
        }
    }

    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn state(mut self, state: ButtonState) -> Self {
        self.state = state;
        self
    }

    /// Set spaces on each side of the label. Defaults to one cell.
    pub fn horizontal_padding(mut self, padding: u16) -> Self {
        self.horizontal_padding = padding;
        self
    }

    /// Choose the frame drawn around the label. Defaults to
    /// [`ButtonChrome::Plain`].
    pub fn chrome(mut self, chrome: ButtonChrome) -> Self {
        self.chrome = chrome;
        self
    }

    /// Extra text modifiers the host needs on top of the state's own
    /// (an underlined inline affordance, say).
    pub fn modifier(mut self, modifier: Modifier) -> Self {
        self.modifier = modifier;
        self
    }

    /// The label this button draws, without padding or chrome.
    pub fn label(&self) -> &'a str {
        self.label
    }

    /// The button as one styled line, chrome included.
    ///
    /// Hosts that render inside a text row (whose hit-testing and selection are
    /// byte-based) compose this instead of calling [`Widget::render`], so the
    /// affordance has exactly one definition.
    pub fn line(&self) -> Line<'a> {
        let style = self.style().add_modifier(self.modifier);
        match self.chrome {
            ButtonChrome::Plain => Line::from(vec![
                Span::styled(" ".repeat(self.horizontal_padding as usize), style),
                Span::styled(self.label, style),
                Span::styled(" ".repeat(self.horizontal_padding as usize), style),
            ]),
            ButtonChrome::Brackets => Line::from(vec![
                Span::styled("[", style),
                Span::styled(self.label, style),
                Span::styled("]", style),
            ]),
        }
    }

    /// Natural one-row size, chrome included.
    pub fn preferred_size(&self) -> (u16, u16) {
        let padding = match self.chrome {
            ButtonChrome::Plain => self.horizontal_padding as usize * 2,
            ButtonChrome::Brackets => 2,
        };
        (
            UnicodeWidthStr::width(self.label)
                .saturating_add(padding)
                .min(u16::MAX as usize) as u16,
            1,
        )
    }

    /// Whether a screen coordinate lies in the supplied rendered button area.
    pub fn hit_test(area: Rect, x: u16, y: u16) -> bool {
        area.width > 0 && area.height > 0 && area.contains(ratatui::layout::Position { x, y })
    }

    fn style(&self) -> Style {
        let base_fg = match self.variant {
            ButtonVariant::Primary => self.theme.accent,
            ButtonVariant::Secondary => self.theme.muted,
            ButtonVariant::Warning => self.theme.warning,
            ButtonVariant::Danger => self.theme.error,
        };
        match self.state {
            ButtonState::Normal => Style::default().fg(base_fg).bg(self.theme.bg),
            ButtonState::Focused => Style::default()
                .fg(self.theme.bg)
                .bg(base_fg)
                .add_modifier(Modifier::BOLD),
            ButtonState::Pressed => Style::default()
                .fg(self.theme.bg)
                .bg(base_fg)
                .add_modifier(Modifier::BOLD),
            ButtonState::Success => Style::default()
                .fg(self.theme.success)
                .bg(self.theme.bg)
                .add_modifier(Modifier::BOLD),
            ButtonState::Disabled => Style::default().fg(self.theme.muted).bg(self.theme.bg),
        }
    }
}

impl Widget for Button<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let style = self.style().add_modifier(self.modifier);
        // Paint the full rect so unused columns/rows cannot retain stale styles.
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_symbol(" ").set_style(style);
                }
            }
        }
        if area.width == 0 {
            return;
        }
        let row = Rect::new(area.x, area.y, area.width, 1);
        self.line().render(row, buf);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{buffer::Buffer, layout::Rect, style::Color, widgets::Widget};

    use super::{Button, ButtonChrome, ButtonState, ButtonTheme, ButtonVariant};

    fn theme() -> ButtonTheme {
        ButtonTheme {
            fg: Color::White,
            bg: Color::Black,
            accent: Color::Cyan,
            muted: Color::Gray,
            warning: Color::Yellow,
            error: Color::Red,
            success: Color::Green,
        }
    }

    #[test]
    fn preferred_size_accounts_for_unicode_label_and_padding() {
        let button = Button::new("复制", theme());
        assert_eq!(button.preferred_size(), (6, 1));
    }

    #[test]
    fn focused_primary_button_paints_its_full_area() {
        let area = Rect::new(2, 1, 10, 2);
        let mut buf = Buffer::empty(Rect::new(0, 0, 14, 4));
        Button::new("Copy", theme())
            .variant(ButtonVariant::Primary)
            .state(ButtonState::Focused)
            .render(area, &mut buf);

        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                let cell = &buf[(x, y)];
                assert_eq!(cell.bg, Color::Cyan);
            }
        }
        assert_eq!(buf[(0, 0)].bg, Color::Reset);
    }

    #[test]
    fn line_is_the_padded_label_and_render_uses_it() {
        let button = Button::new("Copy", theme()).horizontal_padding(2);
        assert_eq!(button.label(), "Copy");

        let drawn: String = button
            .line()
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(drawn, "  Copy  ");
    }

    #[test]
    fn hit_test_covers_the_whole_supplied_rect() {
        let area = Rect::new(4, 2, 8, 1);
        assert!(Button::hit_test(area, 4, 2));
        assert!(Button::hit_test(area, 11, 2));
        assert!(!Button::hit_test(area, 12, 2));
        assert!(!Button::hit_test(area, 4, 3));
    }

    #[test]
    fn brackets_chrome_frames_the_label_and_sizes_to_it() {
        let button = Button::new("Cancel", theme())
            .chrome(ButtonChrome::Brackets)
            .horizontal_padding(3); // brackets replace padding

        assert_eq!(button.preferred_size(), (8, 1));
        let drawn: String = button
            .line()
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(drawn, "[Cancel]");
    }

    #[test]
    fn success_state_draws_the_confirmation_in_the_success_color() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 12, 1));
        Button::new("Copied", theme())
            .state(ButtonState::Success)
            .horizontal_padding(0)
            .render(Rect::new(0, 0, 12, 1), &mut buf);
        assert_eq!(buf[(0, 0)].fg, Color::Green);
    }

    #[test]
    fn warning_variant_uses_the_warning_color() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 12, 1));
        Button::new("[Cancel]", theme())
            .variant(ButtonVariant::Warning)
            .render(Rect::new(0, 0, 12, 1), &mut buf);
        assert_eq!(buf[(0, 0)].fg, Color::Yellow);
    }
}

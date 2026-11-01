//! ANSI SGR to GPUI spans.
//!
//! Terminal tools (test runners, package managers, linters) emit colour
//! escapes in their output. The transcript reader shows that output verbatim;
//! without this adapter the escapes would appear as literal `\x1b[32m` noise.
//! The parser keeps the text and maps SGR colours and attributes onto the
//! design's monospace scale. Sequences the terminal would not paint (cursor
//! movement, OSC titles) are dropped, and the text around them is kept.

use gpui::{
    AnyElement, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _, div, px,
};
use gpui_component::{h_flex, v_flex};

/// One ANSI colour, resolved to RGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    fn hex(self) -> u32 {
        (u32::from(self.r) << 16) | (u32::from(self.g) << 8) | u32::from(self.b)
    }
}

/// The attributes active for one run of terminal text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnsiStyle {
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
}

impl AnsiStyle {
    /// Resolves a standard or bright palette index.
    fn palette(index: u8) -> Option<Rgb> {
        Some(match index {
            0 => Rgb::new(0x1c, 0x1c, 0x1e),
            1 => Rgb::new(0xc7, 0x3e, 0x3e),
            2 => Rgb::new(0x2f, 0x8f, 0x4f),
            3 => Rgb::new(0xb8, 0x86, 0x0b),
            4 => Rgb::new(0x2b, 0x6c, 0xd9),
            5 => Rgb::new(0x9a, 0x4b, 0xc4),
            6 => Rgb::new(0x1d, 0x8f, 0x9a),
            7 => Rgb::new(0xd8, 0xd8, 0xdc),
            8 => Rgb::new(0x6e, 0x6e, 0x73),
            9 => Rgb::new(0xff, 0x6b, 0x6b),
            10 => Rgb::new(0x4f, 0xd1, 0x7a),
            11 => Rgb::new(0xf2, 0xc0, 0x4c),
            12 => Rgb::new(0x6c, 0xa8, 0xff),
            13 => Rgb::new(0xd2, 0x8b, 0xff),
            14 => Rgb::new(0x4f, 0xd6, 0xe0),
            15 => Rgb::new(0xff, 0xff, 0xff),
            16..=231 => {
                let index = index - 16;
                let steps = [0u8, 0x5f, 0x87, 0xaf, 0xd7, 0xff];
                Rgb::new(
                    steps[usize::from(index / 36)],
                    steps[usize::from((index % 36) / 6)],
                    steps[usize::from(index % 6)],
                )
            }
            232..=255 => {
                let level = 8 + (index - 232) * 10;
                Rgb::new(level, level, level)
            }
        })
    }

    fn apply(&mut self, params: &[u16]) {
        if params.is_empty() {
            *self = Self::default();
            return;
        }
        let mut index = 0;
        while index < params.len() {
            match params[index] {
                0 => *self = Self::default(),
                1 => self.bold = true,
                2 => self.dim = true,
                3 => self.italic = true,
                4 => self.underline = true,
                22 => {
                    self.bold = false;
                    self.dim = false;
                }
                23 => self.italic = false,
                24 => self.underline = false,
                30..=37 => self.fg = Self::palette((params[index] - 30) as u8),
                39 => self.fg = None,
                40..=47 => self.bg = Self::palette((params[index] - 40) as u8),
                49 => self.bg = None,
                90..=97 => self.fg = Self::palette((params[index] - 90 + 8) as u8),
                100..=107 => self.bg = Self::palette((params[index] - 100 + 8) as u8),
                38 | 48 => {
                    let foreground = params[index] == 38;
                    if let Some((colour, consumed)) = extended_colour(&params[index + 1..]) {
                        if foreground {
                            self.fg = Some(colour);
                        } else {
                            self.bg = Some(colour);
                        }
                        index += consumed;
                    }
                }
                _ => {}
            }
            index += 1;
        }
    }
}

/// Reads `5;n` or `2;r;g;b` after a 38/48 parameter.
fn extended_colour(params: &[u16]) -> Option<(Rgb, usize)> {
    match params.first().copied()? {
        5 => {
            let index = u8::try_from(*params.get(1)?).ok()?;
            Some((AnsiStyle::palette(index)?, 2))
        }
        2 => {
            let r = u8::try_from(*params.get(1)?).ok()?;
            let g = u8::try_from(*params.get(2)?).ok()?;
            let b = u8::try_from(*params.get(3)?).ok()?;
            Some((Rgb::new(r, g, b), 4))
        }
        _ => None,
    }
}

/// A run of text with one style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnsiSpan {
    pub text: String,
    pub style: AnsiStyle,
}

/// One terminal line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnsiLine {
    pub spans: Vec<AnsiSpan>,
}

impl AnsiLine {
    fn push(&mut self, text: &mut String, style: AnsiStyle) {
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.spans.last_mut()
            && last.style == style
        {
            last.text.push_str(text);
        } else {
            self.spans.push(AnsiSpan {
                text: std::mem::take(text),
                style,
            });
        }
        text.clear();
    }

    fn is_empty(&self) -> bool {
        self.spans.iter().all(|span| span.text.trim().is_empty())
    }
}

/// Whether `text` carries an SGR escape the reader should paint.
pub fn contains_ansi(text: &str) -> bool {
    text.as_bytes().windows(2).any(|window| window == b"\x1b[")
}

/// Splits terminal output into styled lines.
pub fn parse(input: &str) -> Vec<AnsiLine> {
    let mut lines = Vec::new();
    let mut line = AnsiLine::default();
    let mut style = AnsiStyle::default();
    let mut text = String::new();
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\x1b' => {
                line.push(&mut text, style);
                if chars.peek() == Some(&'[') {
                    chars.next();
                    let mut params = String::new();
                    let mut final_byte = None;
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            final_byte = Some(next);
                            break;
                        }
                        params.push(next);
                    }
                    if final_byte == Some('m') {
                        let params: Vec<u16> = params
                            .split(';')
                            .map(|part| part.trim().parse().unwrap_or(0))
                            .collect();
                        style.apply(&params);
                    }
                } else if chars.peek() == Some(&']') {
                    // OSC: consume until BEL or ST.
                    chars.next();
                    while let Some(next) = chars.next() {
                        if next == '\x07' {
                            break;
                        }
                        if next == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                } else {
                    chars.next();
                }
            }
            '\n' => {
                line.push(&mut text, style);
                lines.push(std::mem::take(&mut line));
            }
            '\r' => {
                // A carriage return redraws the line; terminal output that
                // arrives this way is the final frame.
                text.clear();
                line = AnsiLine::default();
            }
            '\t' => {
                text.push_str("    ");
            }
            other => text.push(other),
        }
    }
    line.push(&mut text, style);
    if !line.spans.is_empty() || !lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// Renders parsed terminal output on the reader's monospace scale.
pub fn element(id: &'static str, lines: &[AnsiLine], dark: bool) -> AnyElement {
    let default_ink = if dark { 0xd8d8dc } else { 0x1c1c1e };
    let mut body = v_flex()
        .id(id)
        .debug_selector(move || id.to_string())
        .w_full()
        .min_w_0()
        .rounded_md()
        .bg(gpui::rgb(if dark { 0x141416 } else { 0xf7f7f8 }))
        .border_1()
        .border_color(gpui::rgb(if dark { 0x2a2a2e } else { 0xe4e4e7 }))
        .p(px(10.))
        .gap(px(1.));
    for line in lines {
        if line.is_empty() {
            body = body.child(div().h(px(8.)));
            continue;
        }
        let mut row = h_flex().min_h(px(16.)).flex_wrap();
        for span in &line.spans {
            let mut text = span.text.replace(' ', "\u{00a0}");
            if text.is_empty() {
                continue;
            }
            let mut run = div().font_family("Lilex").text_size(px(11.5));
            let ink = span.style.fg.map(Rgb::hex).unwrap_or(default_ink);
            run = run.text_color(gpui::rgb(ink));
            if let Some(background) = span.style.bg {
                run = run.bg(gpui::rgb(background.hex()));
            }
            if span.style.bold {
                run = run.font_weight(gpui::FontWeight::SEMIBOLD);
            }
            if span.style.dim {
                run = run.opacity(0.65);
            }
            if span.style.italic {
                run = run.italic();
            }
            if span.style.underline {
                run = run.underline();
            }
            row = row.child(run.child(std::mem::take(&mut text)));
        }
        body = body.child(row);
    }
    body.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{AnsiStyle, Rgb, contains_ansi, parse};

    #[test]
    fn plain_text_stays_plain() {
        let lines = parse("hello\nworld");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].spans[0].text, "hello");
        assert_eq!(lines[0].spans[0].style, AnsiStyle::default());
        assert!(!contains_ansi("hello"));
    }

    #[test]
    fn sgr_colours_and_resets_apply_to_the_following_text() {
        let lines = parse("\x1b[31mred\x1b[0m plain");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].spans.len(), 2);
        assert_eq!(lines[0].spans[0].text, "red");
        assert_eq!(lines[0].spans[0].style.fg, Some(Rgb::new(0xc7, 0x3e, 0x3e)));
        assert_eq!(lines[0].spans[1].text, " plain");
        assert_eq!(lines[0].spans[1].style, AnsiStyle::default());
    }

    #[test]
    fn bold_and_dim_are_cleared_by_22() {
        let lines = parse("\x1b[1;2mhi\x1b[22mthere");
        assert!(lines[0].spans[0].style.bold);
        assert!(lines[0].spans[0].style.dim);
        assert!(!lines[0].spans[1].style.bold);
        assert!(!lines[0].spans[1].style.dim);
    }

    #[test]
    fn truecolor_and_256_colours_are_read() {
        let truecolor = parse("\x1b[38;2;12;34;56mx");
        assert_eq!(truecolor[0].spans[0].style.fg, Some(Rgb::new(12, 34, 56)));
        let indexed = parse("\x1b[48;5;196mx");
        assert_eq!(indexed[0].spans[0].style.bg, Some(Rgb::new(0xff, 0, 0)));
    }

    #[test]
    fn non_sgr_sequences_are_dropped_and_text_survives() {
        let lines = parse("\x1b[2K\x1b[1Gkept\x1b]0;title\x07!");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].spans[0].text, "kept!");
    }

    #[test]
    fn carriage_return_keeps_the_final_frame() {
        let lines = parse("10%\r100%");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].spans[0].text, "100%");
    }

    #[test]
    fn adjacent_runs_with_the_same_style_merge() {
        let lines = parse("\x1b[32ma\x1b[32mb");
        assert_eq!(lines[0].spans.len(), 1);
        assert_eq!(lines[0].spans[0].text, "ab");
    }
}

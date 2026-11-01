//! Raw-pixel discipline audit for library layout code.
//!
//! The upstream design guides (gpui-component, 2026-08) require application
//! layout to resolve through semantic spacing tokens or GPUI's rem scale
//! helpers instead of raw `px(...)` literals. Pixels stay correct only at the
//! physical boundaries enumerated by [`Category`].
//!
//! Exceptions are per call site, never per file. A file-wide allowlist lets
//! the first justified literal hide every later violation in the same file,
//! so each allowed use carries its own [`Exception`] entry and the audit
//! reconciles table against source one-for-one in both directions: an entry
//! with no matching use is stale, a use with no entry is a violation, and an
//! entry matching more than one use is too vague to review.
//!
//! Entries carry no line numbers, which rot on every edit above the call
//! site. A use is identified by its normalized `px(...)` expression plus, when
//! the expression repeats within the file, a distinguishing substring of the
//! statement containing it. Both sides pass through [`normalize`], so
//! rustfmt rewrapping a call does not invalidate the table.
//!
//! Comments, string literals, and `#[cfg(test)]`-gated items are excluded by
//! parsing rather than by line prefix or by truncating at the first
//! `#[cfg(test)]`: that marker also appears on inner items mid-file, and
//! truncating there blinds the audit to every production line below it.
//!
//! A file that opens with the inner form — `#![cfg(test)]` before any item —
//! is test code in its entirety. That is how a split-out `tests.rs` under a
//! module directory gates itself, and the gate must be legible from the file
//! alone because the audit never sees the parent's `mod` declaration.

mod source_audit;

use source_audit::{balanced_end, blanked, normalize, rust_sources, test_gated_spans};
use std::path::Path;

/// Physical boundaries where a raw pixel value is the honest unit. Anything
/// outside this list belongs on `SemanticThemeTokens::spacing` or a rem
/// helper, so the value follows the user's type scale.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    /// Intrinsic image or canvas dimensions, which are pixels by definition.
    RasterDimension,
    /// Arithmetic over measured layout results: bounds, extents, anchors.
    MeasuredGeometry,
    /// Distance from an animation's rest position, including rest itself.
    AnimationDisplacement,
    /// Wheel, autoscroll, and scroll-offset motion, whose unit is the pixel.
    ScrollDelta,
    /// How far past the viewport a virtual list keeps rows rendered.
    VirtualOverdraw,
    /// A theme's own token definitions, which pixels feed rather than bypass.
    ThemeDefinition,
    /// A device-pixel hairline that must not grow with the type scale.
    PhysicalHairline,
}

impl Category {
    /// Every sanctioned category, listed in the failure message so an author
    /// adding an exception has to name one of them.
    const ALL: &'static [Self] = &[
        Self::RasterDimension,
        Self::MeasuredGeometry,
        Self::AnimationDisplacement,
        Self::ScrollDelta,
        Self::VirtualOverdraw,
        Self::ThemeDefinition,
        Self::PhysicalHairline,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::RasterDimension => "Category::RasterDimension",
            Self::MeasuredGeometry => "Category::MeasuredGeometry",
            Self::AnimationDisplacement => "Category::AnimationDisplacement",
            Self::ScrollDelta => "Category::ScrollDelta",
            Self::VirtualOverdraw => "Category::VirtualOverdraw",
            Self::ThemeDefinition => "Category::ThemeDefinition",
            Self::PhysicalHairline => "Category::PhysicalHairline",
        }
    }

    fn meaning(self) -> &'static str {
        match self {
            Self::RasterDimension => "intrinsic image or canvas size, not UI spacing",
            Self::MeasuredGeometry => "arithmetic over measured bounds, extents, or anchors",
            Self::AnimationDisplacement => "distance from an animation's rest position",
            Self::ScrollDelta => "wheel, autoscroll, or scroll-offset motion",
            Self::VirtualOverdraw => "rows a virtual list keeps rendered past the viewport",
            Self::ThemeDefinition => "a theme's own token definitions",
            Self::PhysicalHairline => "a device-pixel hairline that must not scale",
        }
    }
}

/// One justified raw-pixel call site.
struct Exception {
    /// Path relative to the crate's `src`, forward slashes.
    file: &'static str,
    /// The whole `px(...)` call, qualifier included, as [`normalize`] renders
    /// it. Compared for equality, so `gpui::px(0.)` and `px(0.)` are distinct.
    expression: &'static str,
    /// A substring of the containing statement, as [`normalize`] renders it.
    /// Empty when the expression is the file's only occurrence — the
    /// exactly-one-match rule turns a later duplicate into a failure that
    /// forces a context in rather than letting it pass unreviewed.
    context: &'static str,
    /// Which physical boundary this is.
    category: Category,
    /// One line: why this value cannot come from the token scale.
    rationale: &'static str,
}

/// Every raw `px(...)` the library is allowed to construct outside tests.
const EXCEPTIONS: &[Exception] = &[
    Exception {
        file: "sizing.rs",
        expression: "px(12.)",
        context: "STATUS_INDICATOR_SLOT",
        category: Category::ThemeDefinition,
        rationale: "the status indicator's own box; pixels feed the token",
    },
    Exception {
        file: "sizing.rs",
        expression: "px(12.)",
        context: "control_padding_sm",
        category: Category::ThemeDefinition,
        rationale: "the size policy's own default control paddings; pixels feed the token",
    },
    Exception {
        file: "sizing.rs",
        expression: "px(14.)",
        context: "control_padding_md",
        category: Category::ThemeDefinition,
        rationale: "the size policy's own default control paddings; pixels feed the token",
    },
    Exception {
        file: "status.rs",
        expression: "gpui::px(6.)",
        context: "INDICATOR_DOT",
        category: Category::MeasuredGeometry,
        rationale: "names the dot's own size so the badge's leading inset can absorb its slot's slack",
    },
    Exception {
        file: "glide.rs",
        expression: "px(x)",
        context: ".left(px(x))",
        category: Category::MeasuredGeometry,
        rationale: "the glide highlight repositions to transition samples of captured row bounds",
    },
    Exception {
        file: "glide.rs",
        expression: "px(y)",
        context: ".top(px(y))",
        category: Category::MeasuredGeometry,
        rationale: "the glide highlight repositions to transition samples of captured row bounds",
    },
    Exception {
        file: "glide.rs",
        expression: "px(w)",
        context: ".w(px(w))",
        category: Category::MeasuredGeometry,
        rationale: "the glide highlight sizes to transition samples of captured row bounds",
    },
    Exception {
        file: "glide.rs",
        expression: "px(h)",
        context: ".h(px(h))",
        category: Category::MeasuredGeometry,
        rationale: "the glide highlight sizes to transition samples of captured row bounds",
    },
    Exception {
        file: "sizing.rs",
        expression: "px(24.)",
        context: "control_sm",
        category: Category::ThemeDefinition,
        rationale: "the size policy's own default control heights; pixels feed the token",
    },
    Exception {
        file: "sizing.rs",
        expression: "px(28.)",
        context: "control_md",
        category: Category::ThemeDefinition,
        rationale: "the size policy's own default control heights; pixels feed the token",
    },
    Exception {
        file: "sizing.rs",
        expression: "px(32.)",
        context: "control_lg",
        category: Category::ThemeDefinition,
        rationale: "the size policy's own default control heights; pixels feed the token",
    },
    Exception {
        file: "sizing.rs",
        expression: "px(16.)",
        context: "slot_sm",
        category: Category::ThemeDefinition,
        rationale: "the glyph slots default to the text line-heights they seat glyphs beside",
    },
    Exception {
        file: "sizing.rs",
        expression: "px(20.)",
        context: "slot_md",
        category: Category::ThemeDefinition,
        rationale: "the glyph slots default to the text line-heights they seat glyphs beside",
    },
    Exception {
        file: "chat.rs",
        expression: "px(256.)",
        context: "",
        category: Category::VirtualOverdraw,
        rationale: "list overdraw is a render-budget distance, independent of the type scale",
    },
    Exception {
        file: "chat.rs",
        expression: "gpui::px(0.)",
        context: "viewport <= gpui::px(0.)",
        category: Category::MeasuredGeometry,
        rationale: "an unlaid-out viewport has no height for the jump drive to travel through",
    },
    Exception {
        file: "chat.rs",
        expression: "gpui::px(1.)",
        context: "if max + current <=",
        category: Category::ScrollDelta,
        rationale: "already-at-the-tail threshold over scroll offsets, whose unit is the pixel",
    },
    Exception {
        file: "image_generation.rs",
        expression: "px(240.)",
        context: "",
        category: Category::RasterDimension,
        rationale: "default frame width mirrors an image's intrinsic pixels; callers override it",
    },
    Exception {
        file: "image_generation.rs",
        expression: "px(160.)",
        context: "",
        category: Category::RasterDimension,
        rationale: "default frame height mirrors an image's intrinsic pixels; callers override it",
    },
    Exception {
        file: "motion.rs",
        expression: "px(REVEAL_RISE * (1.0 - progress) * travel)",
        context: "",
        category: Category::AnimationDisplacement,
        rationale: "reveal rise is travel from rest, interpolated to zero at the end of the curve",
    },
    Exception {
        file: "orbs.rs",
        expression: "px(40.)",
        context: "",
        category: Category::RasterDimension,
        rationale: "default cluster diameter is a decorative canvas extent, exposed via diameter()",
    },
    Exception {
        file: "orbs.rs",
        expression: "px(4.2 * scale)",
        context: "",
        category: Category::MeasuredGeometry,
        rationale: "dot size derives from the caller's canvas extent, not from spacing",
    },
    Exception {
        file: "orbs.rs",
        expression: "px(6.5 * scale)",
        context: "",
        category: Category::MeasuredGeometry,
        rationale: "lattice pitch derives from the caller's canvas extent, not from spacing",
    },
    Exception {
        file: "orbs.rs",
        expression: "px((N - 1) as f32 * pitch.as_f32())",
        context: "",
        category: Category::MeasuredGeometry,
        rationale: "lattice span centers the grid inside the measured canvas extent",
    },
    Exception {
        file: "orbs.rs",
        expression: "px(dx)",
        context: "",
        category: Category::AnimationDisplacement,
        rationale: "horizontal swirl offset from a dot's rest position",
    },
    Exception {
        file: "orbs.rs",
        expression: "px(dy)",
        context: "",
        category: Category::AnimationDisplacement,
        rationale: "vertical swirl offset from a dot's rest position",
    },
    Exception {
        file: "records_table/reorder.rs",
        expression: "gpui::px(0.)",
        context: "let _ = spring(",
        category: Category::AnimationDisplacement,
        rationale: "seeds a reorder spring at its physical origin so the retarget paints travel",
    },
    Exception {
        file: "records_table/reorder.rs",
        expression: "gpui::px(0.)",
        context: "let prior_offset =",
        category: Category::AnimationDisplacement,
        rationale: "a row with no retained offset sits at rest, which is zero displacement",
    },
    Exception {
        file: "records_table/reorder.rs",
        expression: "gpui::px(0.)",
        context: "target:",
        category: Category::AnimationDisplacement,
        rationale: "a never-sampled channel targets rest minus the full displacement",
    },
    Exception {
        file: "records_table/reorder.rs",
        expression: "gpui::px(0.)",
        context: "- motion.target",
        category: Category::AnimationDisplacement,
        rationale: "projects the unpainted offset back from rest for a channel awaiting adoption",
    },
    Exception {
        file: "records_table/reorder.rs",
        expression: "gpui::px(0.)",
        context: "if projected_offset ==",
        category: Category::AnimationDisplacement,
        rationale: "rest comparison prunes rows that owe no motion",
    },
    Exception {
        file: "scrolling.rs",
        expression: "gpui::px(boost * LINE_HEIGHT_PX)",
        context: "",
        category: Category::ScrollDelta,
        rationale: "wheel acceleration converts notches to scroll distance, whose unit is pixels",
    },
    Exception {
        file: "scrolling.rs",
        expression: "gpui::px(1.)",
        context: "",
        category: Category::ScrollDelta,
        rationale: "autoscroll dead zone: one pixel of pointer travel from the anchor",
    },
    Exception {
        file: "scrolling.rs",
        expression: "gpui::px(0.)",
        context: "",
        category: Category::ScrollDelta,
        rationale: "no scroll distance this frame while the pointer sits on the anchor",
    },
    Exception {
        file: "scrolling.rs",
        expression: "gpui::px(AUTOSCROLL_FULL_SPEED_DISTANCE_PX)",
        context: "",
        category: Category::ScrollDelta,
        rationale: "full-speed pointer distance, a tuning parameter measured in pixels",
    },
    Exception {
        file: "scrolling.rs",
        expression: "gpui::px(direction * speed * delta_seconds)",
        context: "",
        category: Category::ScrollDelta,
        rationale: "per-frame autoscroll distance from speed and elapsed time",
    },
    Exception {
        file: "scrolling.rs",
        expression: "px(0.)",
        context: "",
        category: Category::ScrollDelta,
        rationale: "clamps a reported maximum scroll offset to its physical floor",
    },
    Exception {
        file: "scrolling.rs",
        expression: "px(0.5)",
        context: "-self.max_offset +",
        category: Category::ScrollDelta,
        rationale: "half-pixel tolerance deciding whether the bottom edge is already pinned",
    },
    Exception {
        file: "scrolling.rs",
        expression: "px(0.5)",
        context: "self.offset < -px(",
        category: Category::ScrollDelta,
        rationale: "half-pixel tolerance deciding whether the top edge is already pinned",
    },
];

#[test]
fn library_layout_resolves_through_tokens_not_raw_pixels() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let src = Path::new(manifest).join("src");

    let mut uses = Vec::new();
    for (relative, path) in rust_sources(&src) {
        let source = std::fs::read_to_string(&path).expect("source should be readable");
        uses.extend(raw_pixel_uses(&relative, &source));
    }

    let mut unmatched_uses = Vec::new();
    let mut ambiguous_uses = Vec::new();
    let mut matches_per_entry = vec![0usize; EXCEPTIONS.len()];

    for one_use in &uses {
        let matching: Vec<usize> = EXCEPTIONS
            .iter()
            .enumerate()
            .filter(|(_, exception)| exception.matches(one_use))
            .map(|(index, _)| index)
            .collect();
        for index in &matching {
            matches_per_entry[*index] += 1;
        }
        match matching.len() {
            0 => unmatched_uses.push(one_use),
            1 => {}
            _ => ambiguous_uses.push((one_use, matching.len())),
        }
    }

    let mut report = String::new();

    if !unmatched_uses.is_empty() {
        report.push_str(&format!(
            "\n{} raw px() literal(s) with no exception entry:\n",
            unmatched_uses.len()
        ));
        for one_use in &unmatched_uses {
            report.push_str(&one_use.describe());
        }
    }

    let stale: Vec<&Exception> = EXCEPTIONS
        .iter()
        .zip(&matches_per_entry)
        .filter(|(_, count)| **count == 0)
        .map(|(exception, _)| exception)
        .collect();
    if !stale.is_empty() {
        report.push_str(&format!(
            "\n{} stale exception(s) — the code moved on; delete or update the entry:\n",
            stale.len()
        ));
        for exception in stale {
            report.push_str(&exception.describe(0));
        }
    }

    let vague: Vec<(&Exception, usize)> = EXCEPTIONS
        .iter()
        .zip(&matches_per_entry)
        .filter(|(_, count)| **count > 1)
        .map(|(exception, count)| (exception, *count))
        .collect();
    if !vague.is_empty() {
        report.push_str(&format!(
            "\n{} exception(s) matching several uses — split them and give each a `context`:\n",
            vague.len()
        ));
        for (exception, count) in vague {
            report.push_str(&exception.describe(count));
        }
    }

    if !ambiguous_uses.is_empty() {
        report.push_str(&format!(
            "\n{} use(s) claimed by several entries — narrow their `context`:\n",
            ambiguous_uses.len()
        ));
        for (one_use, count) in &ambiguous_uses {
            report.push_str(&format!("  claimed by {count} entries\n"));
            report.push_str(&one_use.describe());
        }
    }

    assert!(report.is_empty(), "{}{}", report, HOW_TO_ADD_AN_EXCEPTION);
}

/// Guard for the guard: a `#[cfg(test)]` on an inner item must not hide the
/// production code that follows it, which is how the previous file-truncating
/// audit went blind.
#[test]
fn a_test_gate_on_an_inner_item_does_not_hide_later_production_code() {
    let source = "\
impl Widget {
    #[cfg(test)]
    fn probe(&self) -> usize {
        self.count
    }

    fn render(&self) -> Div {
        div().w(px(12.))
    }
}
";
    let found = raw_pixel_uses("fixture.rs", source);
    assert_eq!(
        found.len(),
        1,
        "production code after the gate must be read"
    );
    assert_eq!(found[0].expression, "px(12.)");
    assert_eq!(found[0].line, 8);
}

/// A split-out `tests.rs` gates itself with a leading `#![cfg(test)]`; the
/// audit must read that as "this whole file is test code" without seeing the
/// parent's `mod` declaration.
#[test]
fn a_file_opening_with_an_inner_test_gate_is_test_code_in_its_entirety() {
    let source = "\
//! Tests split out of their component module.

#![cfg(test)]

fn fixture() -> Pixels {
    px(7.)
}
";
    assert!(
        raw_pixel_uses("widget/tests.rs", source).is_empty(),
        "a leading inner gate must cover the whole file"
    );
}

/// The inner form deeper in the file belongs to some block, not the file, so
/// it must not blank the production code around it.
#[test]
fn an_inner_test_gate_after_an_item_does_not_hide_the_file() {
    let source = "\
fn real() -> Pixels {
    px(8.)
}

mod grouped {
    #![cfg(test)]
}
";
    let expressions: Vec<String> = raw_pixel_uses("fixture.rs", source)
        .into_iter()
        .map(|found| found.expression)
        .collect();
    assert_eq!(expressions, ["px(8.)"]);
}

#[test]
fn test_gated_items_comments_and_literals_are_excluded() {
    let source = "\
fn real() -> Pixels {
    // px(1.)
    /* px(2.) */
    let label = \"px(3.)\";
    let raw = r#\"px(4.)\"#;
    let tick = '\\'';
    let marker: &'static str = label;
    px(5.)
}

#[cfg(test)]
mod tests {
    fn fixture() -> Pixels {
        px(6.)
    }
}
";
    let expressions: Vec<String> = raw_pixel_uses("fixture.rs", source)
        .into_iter()
        .map(|found| found.expression)
        .collect();
    assert_eq!(expressions, ["px(5.)"]);
}

#[test]
fn the_padding_helper_and_px_suffixed_identifiers_are_not_raw_literals() {
    let source = "\
fn styled() -> Div {
    div().px(tokens.spacing.sm).w(rem_px(4.))
}
";
    assert!(raw_pixel_uses("fixture.rs", source).is_empty());
}

#[test]
fn normalization_absorbs_rustfmt_rewrapping() {
    let rewrapped = "px(\n            4.2 * scale,\n        )";
    assert_eq!(normalize(rewrapped), normalize("px(4.2 * scale)"));
}

/// A raw `px(...)` construction in production code.
struct RawPixelUse {
    /// Path relative to the crate's `src`, forward slashes.
    file: String,
    /// One-based line of the expression's first byte, for the message only.
    line: usize,
    /// The normalized call text, qualifier included.
    expression: String,
    /// The normalized statement containing the call, comments removed.
    statement: String,
}

impl RawPixelUse {
    fn describe(&self) -> String {
        format!(
            "  crates/gpui-ai/src/{}:{}\n    expression: {}\n    statement : {}\n",
            self.file, self.line, self.expression, self.statement
        )
    }
}

impl Exception {
    fn matches(&self, one_use: &RawPixelUse) -> bool {
        self.file == one_use.file
            && normalize(self.expression) == one_use.expression
            && one_use.statement.contains(&normalize(self.context))
    }

    fn describe(&self, matched: usize) -> String {
        let count = if matched == 0 {
            String::new()
        } else {
            format!(" — matched {matched} uses")
        };
        format!(
            "  {} {} context {:?} [{}]{}\n    claimed: {}\n",
            self.file,
            self.expression,
            self.context,
            self.category.name(),
            count,
            self.rationale
        )
    }
}

/// Trailer appended to every failure, so the fix is mechanical.
const HOW_TO_ADD_AN_EXCEPTION: HowTo = HowTo;

/// Renders the instructions lazily; `Display` keeps the category list in one
/// place instead of duplicating it into a string constant.
struct HowTo;

impl std::fmt::Display for HowTo {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            "\nLayout must resolve through semantic spacing tokens \
             (SemanticThemeTokens::spacing)\nor GPUI's rem helpers, so it follows the user's \
             type scale.\n\nIf a value really is a physical boundary, justify that one call \
             site by adding\nan entry to EXCEPTIONS in crates/gpui-ai/tests/pixel_discipline.rs \
             — copy the\n`expression` printed above verbatim:\n\n    \
             Exception {\n        file: \"<path under crates/gpui-ai/src>\",\n        \
             expression: \"<the expression line above>\",\n        // Substring of the \
             statement line above; leave empty when that\n        // expression is the file's \
             only one.\n        context: \"\",\n        category: <one of the categories \
             below>,\n        rationale: \"<one line: why pixels, not tokens>\",\n    },\n\n\
             Sanctioned categories:\n",
        )?;
        for category in Category::ALL {
            writeln!(
                formatter,
                "    {:<32} {}",
                category.name(),
                category.meaning()
            )?;
        }
        formatter.write_str(
            "\nRemoving a call site means removing its entry: an entry that matches \
             nothing\nfails too, so the table cannot rot into a file-wide allowlist.\n",
        )
    }
}

/// Finds every raw `px(...)` construction in the production part of `source`.
fn raw_pixel_uses(file: &str, source: &str) -> Vec<RawPixelUse> {
    let code = blanked(source, true);
    let readable = blanked(source, false);
    let bytes = code.as_bytes();
    let gated = test_gated_spans(source, &code);

    let mut uses = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = code[cursor..].find("px(") {
        let at = cursor + offset;
        cursor = at + 3;

        // `.px(` is the Styled padding helper, which takes a token; an
        // identifier ending in `px` is its own function.
        if at > 0 {
            let previous = bytes[at - 1];
            if previous == b'.' || previous == b'_' || previous.is_ascii_alphanumeric() {
                continue;
            }
        }
        if gated.iter().any(|(start, end)| at >= *start && at < *end) {
            continue;
        }
        let Some(call_end) = balanced_end(bytes, at + 2, b'(', b')') else {
            continue;
        };

        // Take the path qualifier with the call, so `gpui::px(0.)` and a bare
        // `px(0.)` stay distinguishable entries.
        let mut start = at;
        while start > 0 {
            let previous = bytes[start - 1];
            if previous == b':' || previous == b'_' || previous.is_ascii_alphanumeric() {
                start -= 1;
            } else {
                break;
            }
        }

        // The statement window is what an entry may quote as `context`, and it
        // moves with the code instead of pinning a line number. Backwards it
        // reaches the statement head — the part that names what is being
        // computed — so it does not stop at an argument comma; forwards it
        // stops at the end of the operand.
        let mut head = start;
        while head > 0 && !matches!(bytes[head - 1], b';' | b'{' | b'}') {
            head -= 1;
        }
        let mut tail = call_end;
        while tail < bytes.len() && !matches!(bytes[tail], b';' | b'{' | b'}' | b',') {
            tail += 1;
        }

        uses.push(RawPixelUse {
            file: file.to_owned(),
            line: source[..start]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
                + 1,
            expression: normalize(&readable[start..call_end]),
            statement: normalize(&readable[head..tail]),
        });
    }
    uses
}

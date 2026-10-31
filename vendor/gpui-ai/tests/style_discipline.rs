//! What a caller may do to a component's frame, and on which components.
//!
//! Two promises this library makes about every component, both of which it
//! kept unevenly until a gate said so. Styles are the first.
//!
//! The rule is one sentence: if you can put it on screen, you can style it.
//! `.bg()`, `.border_color()`, `.w_full()`, `.text_color()` — the same words
//! that work on a `div`, on the component's own frame.
//!
//! It is a gate because the library drifted out of it without anyone noticing.
//! Every one of the thirty fluent builders implemented [`gpui::Styled`], and
//! none of the eleven entity components did — so half the library styled and
//! half did not compile, and which half you were holding was not something the
//! API said anywhere. A wrapper `div` is not the way out either: layout can be
//! wrapped, but a background, a border, a radius, or an ink set on a wrapper
//! paints *around* the component instead of on it.
//!
//! Two halves to the rule, because a `Styled` impl on its own is the worse
//! failure of the two: it compiles at every call site, reads as supported in
//! the docs, and does nothing at all. So the store has to be applied as well
//! as offered.
//!
//! Where it is applied is a convention this cannot check — `refine_style` has
//! to come after the component's own defaults or they win — and the renders
//! that build their root in pieces put it on the piece rather than on the last
//! line. Applying it is what is enforced; applying it last is what review is
//! for.

mod source_audit;

use source_audit::{blanked, contains_word, rust_sources};
use std::path::Path;

/// Components whose frame is not theirs to hand out.
///
/// Empty, and the aim is to keep it that way: an entry here is a component a
/// caller cannot restyle, which needs a better reason than the work involved.
const EXCEPTIONS: &[&str] = &[];

#[test]
fn every_renderable_component_takes_a_caller_s_styles() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut missing = Vec::new();

    for (relative, path) in rust_sources(&root) {
        let source = std::fs::read_to_string(&path).expect("library source should be readable");
        let code = blanked(&source, true);

        for name in rendered_types(&code) {
            if EXCEPTIONS.contains(&name.as_str()) {
                continue;
            }
            let styled = contains_word(&code, "Styled")
                && (code.contains(&format!("impl Styled for {name}"))
                    || code.contains(&format!("impl gpui::Styled for {name}")));
            if !styled {
                missing.push(format!(
                    "{relative}: {name} renders but implements no Styled"
                ));
                continue;
            }
            if !applies_its_style(&code, &name) {
                missing.push(format!(
                    "{relative}: {name} implements Styled but its render never applies it"
                ));
            }
        }
    }

    missing.sort();
    assert!(
        missing.is_empty(),
        "{} component(s) a caller cannot style:\n  {}\n\n\
         Store a `gpui::StyleRefinement`, implement `Styled` over it, and end the render's \
         root element with `.refine_style(&self.style)` so the caller's words land last.",
        missing.len(),
        missing.join("\n  ")
    );
}

/// Every type in this file that a caller can put on screen.
fn rendered_types(code: &str) -> Vec<String> {
    let mut found = Vec::new();
    for marker in ["impl Render for ", "impl RenderOnce for "] {
        let mut from = 0;
        while let Some(at) = code[from..].find(marker) {
            let start = from + at + marker.len();
            let name: String = code[start..]
                .chars()
                .take_while(|character| character.is_alphanumeric() || *character == '_')
                .collect();
            // A private helper element is the component's own business.
            if !name.is_empty()
                && (code.contains(&format!("pub struct {name}"))
                    || code.contains(&format!("pub struct {name}<")))
            {
                found.push(name);
            }
            from = start;
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Whether `name`'s render hands the caller's store to anything.
///
/// Scoped to the render impl, so a `Styled` impl paired with a `refine_style`
/// somewhere else in the file does not read as satisfied — and then followed
/// one call deep, because a render whose whole body is `toggle_button(self,
/// ..)` has delegated the frame rather than abandoned it.
fn applies_its_style(code: &str, name: &str) -> bool {
    let Some(body) = render_body(code, name) else {
        return false;
    };
    if body.contains("refine_style(") {
        return true;
    }
    called_in(&body).into_iter().any(|callee| {
        function_body(code, &callee).is_some_and(|body| body.contains("refine_style("))
    })
}

/// The text of `name`'s render impl.
fn render_body(code: &str, name: &str) -> Option<String> {
    let header = ["impl Render for ", "impl RenderOnce for "]
        .into_iter()
        .find_map(|marker| code.find(&format!("{marker}{name}")))?;
    let tail = &code[header..];
    let end = tail.find("\n}\n").map_or(tail.len(), |at| at + 1);
    Some(tail[..end].to_owned())
}

/// The text of a free function declared in this file.
fn function_body(code: &str, name: &str) -> Option<String> {
    let at = code.find(&format!("fn {name}("))?;
    let tail = &code[at..];
    let end = tail.find("\n}\n").map_or(tail.len(), |at| at + 1);
    Some(tail[..end].to_owned())
}

/// Names called as plain functions in `body`.
fn called_in(body: &str) -> Vec<String> {
    let mut found = Vec::new();
    let bytes = body.as_bytes();
    let mut name = String::new();
    for (index, byte) in bytes.iter().enumerate() {
        if byte.is_ascii_alphanumeric() || *byte == b'_' {
            name.push(char::from(*byte));
            continue;
        }
        // A method call belongs to its receiver, not to this file.
        if *byte == b'(' && !name.is_empty() && index > name.len() {
            let before = bytes[index - name.len() - 1];
            if before != b'.' && before != b':' {
                found.push(std::mem::take(&mut name));
                continue;
            }
        }
        name.clear();
    }
    found
}

/// Components that build a card frame but hand out no decoration slot.
///
/// The slot is the library's answer to "this component is ours, the surface
/// under and over it is yours" — a photograph behind an approval gate, a veil
/// over a plan. It is offered by every component whose root is the shared card
/// frame, and by no component that is not a card, which is the line this draws.
///
/// The runtime companion to this test renders each one and checks that both
/// layers land on the frame; what it cannot do is notice a *new* card that
/// nobody added to its list. That is this test's job, and it is the failure
/// this file already exists to prevent, one promise over.
#[test]
fn every_card_hands_out_a_decoration_slot() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut missing = Vec::new();

    for (relative, path) in rust_sources(&root) {
        // The card helpers themselves, not a component built from them.
        if relative == "surface.rs" {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("library source should be readable");
        let code = blanked(&source, true);

        for name in rendered_types(&code) {
            let Some(body) = render_body(&code, &name) else {
                continue;
            };
            let framed = body.contains("card(") || body.contains("card_frame(");
            if !framed {
                continue;
            }
            if !code.contains("pub fn decoration(") {
                missing.push(format!(
                    "{relative}: {name} is a card with no decoration builder"
                ));
                continue;
            }
            for half in ["decoration_under", "decoration_over"] {
                if !body.contains(half) {
                    missing.push(format!("{relative}: {name} never places its {half} layer"));
                }
            }
        }
    }

    missing.sort();
    assert!(
        missing.is_empty(),
        "{} card(s) that do not take a decoration:\n  {}\n\n\
         Store a `Decoration`, offer `pub fn decoration(self, decoration: Decoration) -> Self`, \
         and place both layers: `decoration_under` before any content and `decoration_over` \
         after all of it, because paint order is child order.",
        missing.len(),
        missing.join("\n  ")
    );
}

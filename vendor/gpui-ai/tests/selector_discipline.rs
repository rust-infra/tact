//! Debug-selector discipline audit for library render code.
//!
//! Outside `cfg(test)`, GPUI's `debug_selector` takes its closure and drops it
//! without ever calling it:
//!
//! ```ignore
//! fn debug_selector(self, _: impl FnOnce() -> String) -> Self { self }
//! ```
//!
//! So a name built *inside* the closure costs nothing in a release build, and
//! a name built *for* one costs a heap allocation on every frame that draws
//! the element, to describe something no one will read. The difference is one
//! line, and it is invisible: both forms compile, both pass every test, and
//! the wasteful one is the one that reads more naturally.
//!
//! That is why this is a gate rather than a convention. The same mistake was
//! found and fixed by hand in the table's sort glyph, then made three more
//! times in the next release, then found by an audit to be sitting at
//! twenty-nine call sites — so remembering it had failed at every scale it was
//! tried at.
//!
//! The rule is narrow on purpose: a `String` allocated in a binding whose
//! every reader is a dropped closure. A binding that also feeds a real
//! `ElementId` has earned its allocation and is not reported, which is what
//! makes the shared form — one `SharedString`, cloned into each selector — the
//! obvious way out rather than an exception to file. There is no exceptions
//! table, because no call site needs one: if a closure wants a name, it can
//! build it.

mod source_audit;

use source_audit::{balanced_end, blanked, rust_sources, test_gated_spans};
use std::path::Path;

#[test]
fn no_string_is_allocated_for_a_selector_release_builds_never_call() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut wasted = Vec::new();
    for (file, path) in rust_sources(&source_root) {
        let source = std::fs::read_to_string(&path).expect("library source should be readable");
        wasted.extend(wasted_allocations(&file, &source));
    }
    assert!(wasted.is_empty(), "{}", Report(&wasted));
}

/// Guards for the guard. Each fixture is the shape of a mistake this has to
/// keep telling apart from the shape of correct code beside it.
mod guarding_the_guard {
    use super::wasted_allocations;

    fn caught(source: &str) -> Vec<String> {
        wasted_allocations("fixture.rs", source)
            .into_iter()
            .map(|wasted| wasted.binding)
            .collect()
    }

    #[test]
    fn a_name_built_for_a_dropped_closure_is_caught() {
        let found = caught(
            r#"
fn row(id: &SharedString) -> Div {
    let debug_id = format!("row-{id}");
    div().debug_selector(move || debug_id.clone())
}
"#,
        );
        assert_eq!(
            found,
            ["debug_id"],
            "built whole, then handed over to be dropped"
        );
    }

    #[test]
    fn a_name_interpolated_inside_the_format_string_still_counts_as_read() {
        // Written after getting this wrong here: scanning source with string
        // literals blanked finds no reader for `debug_id` at all, and a
        // binding with no readers is not reported — so the audit passed
        // clean over thirty-five live call sites.
        let found = caught(
            r#"
fn row(id: &SharedString) -> Div {
    let debug_id = id.to_string();
    div().debug_selector(move || format!("row-{debug_id}"))
}
"#,
        );
        assert!(!found.is_empty(), "an inline format argument is a reader");
    }

    #[test]
    fn a_name_that_also_reaches_a_real_id_has_earned_its_allocation() {
        let found = caught(
            r#"
fn row(id: &SharedString) -> Div {
    let debug_id = id.to_string();
    div().id(debug_id.clone()).debug_selector(move || format!("row-{debug_id}"))
}
"#,
        );
        assert!(
            found.is_empty(),
            "one allocation, two readers, one of them real"
        );
    }

    #[test]
    fn a_name_built_inside_the_closure_costs_nothing() {
        let found = caught(
            r#"
fn row(id: &SharedString) -> Div {
    let debug_id = id.clone();
    div().debug_selector(move || format!("row-{debug_id}"))
}
"#,
        );
        assert!(
            found.is_empty(),
            "a SharedString clone is not an allocation"
        );
    }

    #[test]
    fn a_test_build_reads_the_closure_so_nothing_there_is_wasted() {
        let found = caught(
            r#"
#[cfg(test)]
fn row(id: &SharedString) -> Div {
    let debug_id = id.to_string();
    div().debug_selector(move || format!("row-{debug_id}"))
}
"#,
        );
        assert!(found.is_empty(), "under cfg(test) the closure is called");
    }

    #[test]
    fn an_honest_binding_next_door_does_not_excuse_a_wasteful_one() {
        // Both are named `debug_id`; only the second has a real reader. Reading
        // the file rather than the block would see one honest use and let the
        // other through, which is how a hand audit missed most of them.
        let found = caught(
            r#"
fn rows(id: &SharedString) -> Div {
    {
        let debug_id = id.to_string();
        div().debug_selector(move || format!("row-{debug_id}"))
    }
    {
        let debug_id = id.to_string();
        div().id(debug_id.clone()).debug_selector(move || format!("row-{debug_id}"))
    }
}
"#,
        );
        assert_eq!(found, ["debug_id"], "exactly the one with no real reader");
    }
}

/// A `String` built for a closure that release builds throw away unread.
struct Wasted {
    file: String,
    line: usize,
    binding: String,
    statement: String,
}

/// Every wasteful binding in the production part of one file.
fn wasted_allocations(file: &str, source: &str) -> Vec<Wasted> {
    let code = blanked(source, true);
    let readable = blanked(source, false);
    let gated = test_gated_spans(source, &code);
    // A test build does call the closure, so a name built for one there is
    // read rather than wasted.
    let production = |at: usize| !gated.iter().any(|(from, to)| (*from..*to).contains(&at));
    let selectors = selector_arguments(&code);
    let reader_is_a_selector =
        |at: &usize| selectors.iter().any(|(from, to)| (*from..*to).contains(at));

    string_bindings(&code)
        .into_iter()
        .filter(|binding| production(binding.at))
        .filter(|binding| {
            let scope = enclosing_block(code.as_bytes(), binding.at);
            // Uses are counted in text that still has its string literals: a
            // captured inline format argument — `format!("row-{debug_id}")` —
            // is a real read, and the commonest shape of this mistake. Reading
            // the blanked code here would find no reader at all and call every
            // one of them fine.
            let readers = word_uses(&readable, &binding.name, binding.initializer_end, scope);
            !readers.is_empty() && readers.iter().all(reader_is_a_selector)
        })
        .map(|binding| Wasted {
            file: file.to_owned(),
            line: line_of(&code, binding.at),
            binding: binding.name,
            statement: line_at(&readable, binding.at),
        })
        .collect()
}

/// A `let` binding whose initializer allocates a fresh `String`.
struct StringBinding {
    name: String,
    at: usize,
    initializer_end: usize,
}

/// Every binding in `code` that allocates a `String` outright.
///
/// Deliberately literal about the forms it recognizes. A cheap handle — a
/// `SharedString` clone, an `into()` on a `&'static str` — is not an
/// allocation to account for, and guessing at the cost of an arbitrary
/// expression would make this either noisy or dishonest.
fn string_bindings(code: &str) -> Vec<StringBinding> {
    let bytes = code.as_bytes();
    let mut found = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = code[cursor..].find("let ") {
        let at = cursor + offset;
        cursor = at + "let ".len();
        if at > 0 && is_word_byte(bytes[at - 1]) {
            continue;
        }
        let Some(end) = statement_end(bytes, cursor) else {
            continue;
        };
        let statement = &code[cursor..end];
        let Some(equals) = assignment(statement) else {
            continue;
        };
        let (bound, initializer) = statement.split_at(equals);
        let name = bound
            .trim()
            .trim_start_matches("mut ")
            .split([':', ' '])
            .next()
            .unwrap_or_default()
            .trim();
        if name.is_empty() || !name.bytes().all(is_word_byte) {
            continue;
        }
        if allocates_a_string(initializer[1..].trim()) {
            found.push(StringBinding {
                name: name.to_owned(),
                at,
                initializer_end: end,
            });
        }
    }
    found
}

/// Whether an initializer expression produces a freshly allocated `String`.
fn allocates_a_string(initializer: &str) -> bool {
    let expression = initializer.trim_end_matches(';').trim_end();
    expression.starts_with("format!")
        || expression.starts_with("String::from")
        || expression.ends_with(".to_string()")
        || expression.ends_with(".to_owned()")
}

/// Argument spans of every `debug_selector(...)` call.
fn selector_arguments(code: &str) -> Vec<(usize, usize)> {
    let bytes = code.as_bytes();
    let mut found = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = code[cursor..].find("debug_selector") {
        let at = cursor + offset;
        cursor = at + "debug_selector".len();
        // The name is a struct field elsewhere; only a call takes a closure.
        let open = cursor + (code[cursor..].len() - code[cursor..].trim_start().len());
        if bytes.get(open) != Some(&b'(') {
            continue;
        }
        if let Some(close) = balanced_end(bytes, open, b'(', b')') {
            found.push((open, close));
            cursor = close;
        }
    }
    found
}

/// End of the innermost block containing `index`.
///
/// A binding is visible only to the end of its own block, so a same-named
/// binding in a sibling block cannot vouch for this one. Reading the whole
/// file instead would let a single honest `debug_id` anywhere in it excuse
/// every wasteful `debug_id` beside it — which is how twenty-nine of these
/// stayed invisible to a reviewer reading file by file.
fn enclosing_block(bytes: &[u8], index: usize) -> usize {
    let mut depth = 0usize;
    let mut at = index;
    while at > 0 {
        at -= 1;
        match bytes[at] {
            b'}' => depth += 1,
            b'{' if depth == 0 => {
                return balanced_end(bytes, at, b'{', b'}').unwrap_or(bytes.len());
            }
            b'{' => depth -= 1,
            _ => {}
        }
    }
    bytes.len()
}

/// Offsets at which `name` is used as a whole word within `from..to`.
fn word_uses(code: &str, name: &str, from: usize, to: usize) -> Vec<usize> {
    let bytes = code.as_bytes();
    let mut found = Vec::new();
    let mut cursor = from;
    while cursor < to {
        let Some(offset) = code[cursor..to].find(name) else {
            break;
        };
        let at = cursor + offset;
        cursor = at + name.len();
        let follows = bytes.get(cursor).copied().is_some_and(is_word_byte);
        if at
            .checked_sub(1)
            .map(|ix| bytes[ix])
            .is_some_and(is_word_byte)
            || follows
        {
            continue;
        }
        found.push(at);
    }
    found
}

/// Index of the `;` ending the statement starting at `from`, if it has one.
fn statement_end(bytes: &[u8], from: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, byte) in bytes[from..].iter().enumerate() {
        match byte {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.checked_sub(1)?,
            b';' if depth == 0 => return Some(from + offset),
            _ => {}
        }
    }
    None
}

/// Offset of the `=` that opens the initializer, skipping `==` and generics.
fn assignment(statement: &str) -> Option<usize> {
    let bytes = statement.as_bytes();
    let mut depth = 0usize;
    for (offset, byte) in bytes.iter().enumerate() {
        match byte {
            b'(' | b'[' | b'<' => depth += 1,
            b')' | b']' | b'>' => depth = depth.saturating_sub(1),
            b'=' if depth == 0 && bytes.get(offset + 1) != Some(&b'=') => return Some(offset),
            _ => {}
        }
    }
    None
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn line_of(code: &str, index: usize) -> usize {
    code[..index].bytes().filter(|byte| *byte == b'\n').count() + 1
}

/// The whole line containing `index`, trimmed, for the failure message.
fn line_at(readable: &str, index: usize) -> String {
    let start = readable[..index].rfind('\n').map_or(0, |at| at + 1);
    let end = readable[index..]
        .find('\n')
        .map_or(readable.len(), |at| index + at);
    readable[start..end].trim().to_owned()
}

struct Report<'a>(&'a [Wasted]);

impl std::fmt::Display for Report<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            formatter,
            "{} string(s) allocated for a selector that release builds never call:\n",
            self.0.len()
        )?;
        for wasted in self.0 {
            writeln!(
                formatter,
                "  src/{}:{}  `{}`\n    {}",
                wasted.file, wasted.line, wasted.binding, wasted.statement
            )?;
        }
        write!(
            formatter,
            "\nEach is a heap allocation per frame, for a name dropped unread \
             outside cfg(test).\nBuild the name inside the closure instead, \
             capturing a cheap handle:\n\n  \
             - let debug_id = format!(\"row-{{id}}\");\n  \
             - .debug_selector(move || debug_id.clone())\n  \
             + let debug_id = id.clone();   // a SharedString clone is a refcount bump\n  \
             + .debug_selector(move || format!(\"row-{{debug_id}}\"))\n\n\
             A binding that also feeds a real ElementId has earned its \
             allocation and is not reported."
        )
    }
}

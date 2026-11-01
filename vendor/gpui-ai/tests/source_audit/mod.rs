//! Reading Rust source the way the compiler would, for audits that read it.
//!
//! Two gates now scan the library's own source: the raw-pixel audit and the
//! debug-selector audit. Both have to see past the same three things — a
//! `px(` inside a doc comment, a brace inside a string literal, and an item
//! compiled only for tests — and getting any of them wrong makes an audit
//! either blind or wrong. Scanning lives here so there is one implementation
//! to trust rather than two to keep in step.
//!
//! Each integration test compiles as its own crate and takes only what it
//! needs, so unused helpers here are expected rather than dead.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// Every `.rs` file under `src`, recursively, paired with its `src`-relative
/// path. Recursion matters: a module moved into a subdirectory must not fall
/// out of the audit silently.
pub(crate) fn rust_sources(root: &Path) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory).expect("library src should be readable");
        for entry in entries {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let relative = path
                    .strip_prefix(root)
                    .expect("walked paths live under src")
                    .to_string_lossy()
                    .replace('\\', "/");
                found.push((relative, path));
            }
        }
    }
    found.sort();
    found
}

/// Byte spans of `#[cfg(test)]`-gated items, attribute included.
///
/// The gate sits on inner functions as often as on the trailing `mod tests`,
/// so each span is bounded by the item it annotates rather than running to
/// end of file.
pub(crate) fn test_gated_spans(source: &str, code: &str) -> Vec<(usize, usize)> {
    let bytes = code.as_bytes();
    // A leading inner gate compiles the whole file for tests only. Leading
    // means before any item: an inner attribute deeper in the file belongs to
    // some block, and guessing which one would over-blank production code.
    if let Some(start) = code.find("#![")
        && code[..start].trim().is_empty()
        && balanced_end(bytes, start + 2, b'[', b']')
            .is_some_and(|end| is_test_gate(&source[start..end]))
    {
        return vec![(0, code.len())];
    }
    let mut spans = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = code[cursor..].find("#[") {
        let start = cursor + offset;
        let Some(attribute_end) = balanced_end(bytes, start + 1, b'[', b']') else {
            break;
        };
        if is_test_gate(&source[start..attribute_end]) {
            let end = gated_item_end(bytes, attribute_end);
            spans.push((start, end));
            cursor = end;
        } else {
            cursor = attribute_end;
        }
    }
    spans
}

/// Whether an attribute compiles its item for tests only. `not(...)` is
/// treated as no gate at all: guessing wrong there would silently drop
/// production code from the audit.
pub(crate) fn is_test_gate(attribute: &str) -> bool {
    let normalized = normalize(attribute);
    (normalized.starts_with("#[cfg(") || normalized.starts_with("#![cfg("))
        && !normalized.contains("not(")
        && contains_word(&normalized, "test")
}

pub(crate) fn contains_word(haystack: &str, word: &str) -> bool {
    haystack.match_indices(word).any(|(index, _)| {
        let before = haystack[..index].chars().next_back();
        let after = haystack[index + word.len()..].chars().next();
        !before.is_some_and(|character| character.is_alphanumeric() || character == '_')
            && !after.is_some_and(|character| character.is_alphanumeric() || character == '_')
    })
}

/// End of the item an attribute annotates: the close of its block, or the
/// semicolon of a block-less item such as a gated `use`.
pub(crate) fn gated_item_end(bytes: &[u8], from: usize) -> usize {
    let mut braces = 0usize;
    let mut nesting = 0usize;
    let mut body_started = false;
    for (index, &byte) in bytes.iter().enumerate().skip(from) {
        match byte {
            b'{' => {
                braces += 1;
                body_started = true;
            }
            b'}' => {
                braces = braces.saturating_sub(1);
                if body_started && braces == 0 {
                    return index + 1;
                }
            }
            b'(' | b'[' => nesting += 1,
            b')' | b']' => nesting = nesting.saturating_sub(1),
            b';' if braces == 0 && nesting == 0 => return index + 1,
            _ => {}
        }
    }
    bytes.len()
}

/// Index just past the delimiter that closes the one at `open`.
pub(crate) fn balanced_end(bytes: &[u8], open: usize, opener: u8, closer: u8) -> Option<usize> {
    let mut depth = 0usize;
    for (index, &byte) in bytes.iter().enumerate().skip(open) {
        if byte == opener {
            depth += 1;
        } else if byte == closer {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index + 1);
            }
        }
    }
    None
}

/// Overwrites comments — and optionally string, byte-string, and character
/// literals — with spaces, preserving byte offsets and line breaks.
///
/// Scanning the blanked text is what keeps a `px(` inside a doc comment or a
/// brace inside a string from steering the audit. Expression and statement
/// text is read from the comment-only form, so quoted text stays readable in
/// the failure message while comments never leak into a `context`.
pub(crate) fn blanked(source: &str, blank_literals: bool) -> String {
    let bytes = source.as_bytes();
    let mut mask = bytes.to_vec();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'/') {
            let end = source[index..]
                .find('\n')
                .map_or(bytes.len(), |at| index + at);
            blank(&mut mask, index, end);
            index = end;
            continue;
        }
        if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
            let end = block_comment_end(bytes, index + 2);
            blank(&mut mask, index, end);
            index = end;
            continue;
        }
        if let Some(end) = literal_end(bytes, index) {
            if blank_literals {
                blank(&mut mask, index, end);
            }
            index = end;
            continue;
        }
        index += 1;
    }
    String::from_utf8(mask).expect("blanking replaces whole ASCII-delimited regions")
}

pub(crate) fn blank(mask: &mut [u8], from: usize, to: usize) {
    let to = to.min(mask.len());
    for byte in &mut mask[from..to] {
        if *byte != b'\n' {
            *byte = b' ';
        }
    }
}

/// End of a (possibly nested) block comment opened before `from`.
pub(crate) fn block_comment_end(bytes: &[u8], from: usize) -> usize {
    let mut depth = 1usize;
    let mut index = from;
    while index < bytes.len() {
        if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
            depth += 1;
            index += 2;
        } else if bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/') {
            depth -= 1;
            index += 2;
            if depth == 0 {
                return index;
            }
        } else {
            index += 1;
        }
    }
    bytes.len()
}

/// End of the string, raw string, byte string, or character literal starting
/// at `index`, or `None` when nothing starts there. A `'` that is not a
/// complete character literal is a lifetime and stays code.
pub(crate) fn literal_end(bytes: &[u8], index: usize) -> Option<usize> {
    // A `b` or `r` inside an identifier is not a literal prefix.
    let after_identifier = index
        .checked_sub(1)
        .is_some_and(|before| bytes[before].is_ascii_alphanumeric() || bytes[before] == b'_');
    let mut cursor = index;
    if !after_identifier {
        if bytes.get(cursor) == Some(&b'b') {
            cursor += 1;
        }
        if bytes.get(cursor) == Some(&b'r') {
            cursor += 1;
            let hashes = hash_run(bytes, cursor);
            cursor += hashes;
            if bytes.get(cursor) != Some(&b'"') {
                return None;
            }
            return Some(raw_string_end(bytes, cursor + 1, hashes));
        }
    }
    match bytes.get(cursor) {
        Some(&b'"') => Some(string_end(bytes, cursor + 1)),
        Some(&b'\'') => character_end(bytes, cursor),
        _ => None,
    }
}

pub(crate) fn string_end(bytes: &[u8], from: usize) -> usize {
    let mut index = from;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'"' => return index + 1,
            _ => index += 1,
        }
    }
    bytes.len()
}

pub(crate) fn raw_string_end(bytes: &[u8], from: usize, hashes: usize) -> usize {
    let mut index = from;
    while index < bytes.len() {
        if bytes[index] == b'"' && hash_run(bytes, index + 1) >= hashes {
            return (index + 1 + hashes).min(bytes.len());
        }
        index += 1;
    }
    bytes.len()
}

/// Length of the run of `#` starting at `from`, which delimits a raw string.
pub(crate) fn hash_run(bytes: &[u8], from: usize) -> usize {
    bytes[from..]
        .iter()
        .take_while(|byte| **byte == b'#')
        .count()
}

pub(crate) fn character_end(bytes: &[u8], quote: usize) -> Option<usize> {
    let mut index = quote + 1;
    if bytes.get(index) == Some(&b'\\') {
        index += 2;
        while index < bytes.len() && bytes[index] != b'\'' {
            index += 1;
        }
        return (index < bytes.len()).then_some(index + 1);
    }
    let leading = *bytes.get(index)?;
    index += match leading {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    };
    (bytes.get(index) == Some(&b'\'')).then_some(index + 1)
}

/// Collapses whitespace and the punctuation spacing rustfmt rearranges when
/// it wraps a call, so an entry survives reformatting: runs of whitespace
/// become one space, spaces hugging brackets and commas go, and a trailing
/// comma before a closing bracket goes with them.
pub(crate) fn normalize(text: &str) -> String {
    let mut spaced = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_whitespace() {
            if !spaced.ends_with(' ') {
                spaced.push(' ');
            }
        } else {
            spaced.push(character);
        }
    }

    let mut tightened = String::with_capacity(spaced.len());
    for character in spaced.chars() {
        match character {
            ' ' if tightened.ends_with(['(', '[']) || tightened.is_empty() => {}
            ')' | ']' | ',' | ';' => {
                if tightened.ends_with(' ') {
                    tightened.pop();
                }
                if matches!(character, ')' | ']') && tightened.ends_with(',') {
                    tightened.pop();
                }
                tightened.push(character);
            }
            _ => tightened.push(character),
        }
    }
    tightened.trim_end().to_owned()
}

//! Line diff for a tool card's detail body.
//!
//! A file-changing tool reports *what it changed*, not only the text it ended
//! up with: `edit_file`'s `old_text`/`new_text` pair and `write_file`'s
//! `content` are rendered here as a fragment diff. The output follows
//! `git diff`'s line grammar — `-` removed, `+` added, a leading space for
//! unchanged context — but carries no `@@` header, because a fragment of an
//! input field has no file position to name.
//!
//! Both front ends read the result: the TUI expands a card onto this text, and
//! the desktop client counts `+`/`-` lines for the write badge and shows the
//! body in its Diff pane.

/// Upper bound on the LCS table, in cells.
///
/// A pathological field pair (a multi-thousand-line replacement) would
/// otherwise allocate a table proportional to the product of both line counts.
/// Past this the diff degrades to "remove everything, add everything", which is
/// still a correct — if noisier — rendering of the change. The bound is cheap
/// to keep: the largest pair that still fits here (1024 lines against 1024)
/// measures ~3.6 ms, and one line past it drops to ~60 µs.
const MAX_TABLE_CELLS: usize = 1 << 20;

/// Render `old` → `new` as a line diff.
///
/// An empty `old` is a file being created: every line of `new` is an addition.
/// An empty `new` (an edit that deletes text) is the mirror image. Two empty
/// sides have nothing to report, so the result is empty and the caller drops
/// the detail entirely.
pub(crate) fn fragment(old: &str, new: &str) -> String {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    if old_lines.is_empty() && new_lines.is_empty() {
        return String::new();
    }
    if old_lines.is_empty() {
        return prefixed(&new_lines, '+');
    }
    if new_lines.is_empty() {
        return prefixed(&old_lines, '-');
    }
    if old_lines.len().saturating_mul(new_lines.len()) > MAX_TABLE_CELLS {
        return format!(
            "{}\n{}",
            prefixed(&old_lines, '-'),
            prefixed(&new_lines, '+')
        );
    }
    lcs(&old_lines, &new_lines)
}

/// Every line with `mark` in front of it.
///
/// Built in place rather than mapped and joined: the output is written once to
/// the database and once to the card, so a `write_file` detail of tens of
/// thousands of lines must not allocate a `String` per line on the way.
fn prefixed(lines: &[&str], mark: char) -> String {
    let mut out = String::new();
    for line in lines {
        push_line(&mut out, mark, line);
    }
    out
}

/// Append one rendered line, separating it from the previous one.
///
/// The `mark` guarantees a non-empty buffer after the first line, so "is this
/// the first line" is just "is the buffer empty".
fn push_line(out: &mut String, mark: char, line: &str) {
    if !out.is_empty() {
        out.push('\n');
    }
    out.push(mark);
    out.push_str(line);
}

/// The longest-common-subsequence walk over two line slices.
///
/// The table is built backwards so the forward walk can emit the result in
/// order without a second reversal. Ties prefer removal, which keeps a
/// replaced block reading "old, then new" rather than interleaving the two.
fn lcs(old: &[&str], new: &[&str]) -> String {
    let (n, m) = (old.len(), new.len());
    let width = m + 1;
    let mut table = vec![0u32; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i * width + j] = if old[i] == new[j] {
                table[(i + 1) * width + j + 1] + 1
            } else {
                table[(i + 1) * width + j].max(table[i * width + j + 1])
            };
        }
    }

    let mut out = String::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if old[i] == new[j] {
            push_line(&mut out, ' ', old[i]);
            i += 1;
            j += 1;
        } else if table[(i + 1) * width + j] >= table[i * width + j + 1] {
            push_line(&mut out, '-', old[i]);
            i += 1;
        } else {
            push_line(&mut out, '+', new[j]);
            j += 1;
        }
    }
    while i < n {
        push_line(&mut out, '-', old[i]);
        i += 1;
    }
    while j < m {
        push_line(&mut out, '+', new[j]);
        j += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::fragment;

    #[test]
    fn a_new_file_is_all_additions() {
        assert_eq!(fragment("", "a\nb\n"), "+a\n+b");
    }

    #[test]
    fn a_deleted_fragment_is_all_removals() {
        assert_eq!(fragment("a\nb\n", ""), "-a\n-b");
    }

    #[test]
    fn unchanged_lines_are_context() {
        let out = fragment("keep\nold\ntail\n", "keep\nnew\ntail\n");
        assert_eq!(out, " keep\n-old\n+new\n tail");
    }

    #[test]
    fn nothing_to_report_renders_empty() {
        assert_eq!(fragment("", ""), "");
    }

    #[test]
    fn a_replaced_block_reads_old_then_new() {
        let out = fragment("fn run() {\n    a();\n}", "fn run() {\n    b();\n}");
        assert_eq!(out, " fn run() {\n-    a();\n+    b();\n }");
    }
}

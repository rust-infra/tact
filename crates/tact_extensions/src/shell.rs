//! Shared shell command validation.

use anyhow::Result;

const BLOCKED_SUBSTRINGS: &[&str] = &[
    "sudo",
    "shutdown",
    "reboot",
    "> /dev/",
    ">> /dev/",
    ">/dev/",
    ">>/dev/",
    "rm -rf /",
    "rm -fr /",
    "rm -rf /*",
    "rm -fr /*",
    "rm -rf ~",
    "rm -fr ~",
    "rm -rf ~/",
    "rm -fr ~/",
    "rm -rf $home",
    "rm -fr $home",
];

/// Returns true for commands that must always be blocked from execution.
pub fn validate_shell_command(command: &str) -> Result<()> {
    if is_high_risk_shell_command(command) {
        anyhow::bail!("Error: Dangerous command blocked");
    }
    Ok(())
}

/// Returns true for shell commands that require explicit user approval.
pub fn is_high_risk_shell_command(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    // Strip /dev/null to avoid false-positives on common stderr/stdout suppression.
    let lower = lower.replace("/dev/null", "");
    BLOCKED_SUBSTRINGS
        .iter()
        .any(|pattern| lower.contains(pattern))
}

/// Split a shell command into the independent commands `sh -c` would run.
///
/// Rules match the *whole* command string, and a glob `*` matches everything —
/// including `;`. So a rule meant to cover one command (`bash(command:cargo
/// test *)`) would also cover `cargo test; rm -rf ~`, and worse, it would
/// pre-empt the `deny` rule naming the second half: deny and allow both match
/// the whole string, and a string starting with `cargo test` does not start
/// with `rm`, so only the allow matches. Segmenting is what makes a prefix rule
/// expressible — every segment has to be covered before the rule applies.
///
/// Returns `None` when the command cannot be enumerated as a flat list of
/// segments: command substitution (`$(…)`, backticks), process substitution
/// (`<(…)`, `>(…)`), or an unbalanced quote. Those run code that no prefix of
/// the command string names, so a caller must read `None` as "cannot be covered
/// by a prefix rule", never as "nothing to check".
///
/// Quotes and escapes are honoured, because the split must not disagree with
/// what the shell actually runs: an operator inside `'…'` or `"…"` is literal
/// text, `\` outside quotes escapes the next character (a `\`-newline is a line
/// continuation), and inside double quotes only `"`, `\`, `$` and a backtick
/// are escapable — which is the shell's own rule.
#[must_use]
pub fn command_segments(command: &str) -> Option<Vec<String>> {
    let mut segments: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            // Single quotes are literal throughout: no escapes, no operators,
            // and no substitutions.
            '\'' => {
                current.push('\'');
                loop {
                    match chars.next() {
                        Some('\'') => {
                            current.push('\'');
                            break;
                        }
                        Some(inner) => current.push(inner),
                        None => return None,
                    }
                }
            }
            '"' => {
                current.push('"');
                loop {
                    match chars.next() {
                        Some('"') => {
                            current.push('"');
                            break;
                        }
                        Some('\\') => match chars.peek().copied() {
                            Some(next @ ('"' | '\\' | '$' | '`' | '\n')) => {
                                current.push('\\');
                                current.push(next);
                                chars.next();
                            }
                            // Not an escape here, just a literal backslash.
                            _ => current.push('\\'),
                        },
                        // Substitutions run inside double quotes too.
                        Some('$') if chars.peek() == Some(&'(') => return None,
                        Some('`') => return None,
                        Some(inner) => current.push(inner),
                        None => return None,
                    }
                }
            }
            '\\' => match chars.next() {
                // A backslash-newline joins the lines; a trailing backslash
                // has nothing to escape.
                Some('\n') | None => {}
                Some(next) => current.push(next),
            },
            '$' if chars.peek() == Some(&'(') => return None,
            '<' | '>' if chars.peek() == Some(&'(') => return None,
            '`' => return None,
            ';' | '\n' => push_segment(&mut segments, &mut current),
            '&' => {
                // `2>&1`, `>&2`, `<&3` and `&>file` are redirections, not a
                // background separator. Only a bare `&` (or the first half of
                // `&&`) ends the segment.
                let preceded_by_redirect = current
                    .chars()
                    .last()
                    .is_some_and(|prev| prev.is_ascii_digit() || prev == '>' || prev == '<');
                if preceded_by_redirect || chars.peek() == Some(&'>') {
                    current.push('&');
                } else {
                    if chars.peek() == Some(&'&') {
                        chars.next();
                    }
                    push_segment(&mut segments, &mut current);
                }
            }
            '|' => {
                // `||` and `|&` are one operator each.
                if matches!(chars.peek(), Some('|') | Some('&')) {
                    chars.next();
                }
                push_segment(&mut segments, &mut current);
            }
            _ => current.push(c),
        }
    }
    push_segment(&mut segments, &mut current);
    Some(segments)
}

/// Close the segment built so far, dropping one that is only whitespace.
fn push_segment(segments: &mut Vec<String>, current: &mut String) {
    let segment = current.trim();
    if !segment.is_empty() {
        segments.push(segment.to_string());
    }
    current.clear();
}

#[cfg(test)]
mod segment_tests {
    use super::command_segments;

    /// The segments of a command that is enumerable, or a panic naming the one
    /// that was expected to be.
    fn segments(command: &str) -> Vec<String> {
        command_segments(command).unwrap_or_else(|| panic!("`{command}` should be enumerable"))
    }

    #[test]
    fn a_plain_command_is_one_segment() {
        assert_eq!(segments("cargo test --lib"), vec!["cargo test --lib"]);
    }

    #[test]
    fn control_operators_split_segments() {
        assert_eq!(
            segments("cargo test && rm -rf /"),
            vec!["cargo test", "rm -rf /"]
        );
        assert_eq!(
            segments("cargo test; rm -rf /"),
            vec!["cargo test", "rm -rf /"]
        );
        assert_eq!(
            segments("cargo test || rm -rf /"),
            vec!["cargo test", "rm -rf /"]
        );
        assert_eq!(
            segments("cargo test & rm -rf /"),
            vec!["cargo test", "rm -rf /"]
        );
        assert_eq!(
            segments("cargo test\nrm -rf /"),
            vec!["cargo test", "rm -rf /"]
        );
        assert_eq!(segments("cargo test | sh"), vec!["cargo test", "sh"]);
    }

    #[test]
    fn a_quoted_operator_is_text_not_a_separator() {
        assert_eq!(segments("echo 'a; b'"), vec!["echo 'a; b'"]);
        assert_eq!(segments("echo \"a; b\""), vec!["echo \"a; b\""]);
        // An escaped `;` is literal too.
        assert_eq!(segments("echo a\\; b"), vec!["echo a; b"]);
    }

    #[test]
    fn a_redirection_ampersand_is_not_a_separator() {
        assert_eq!(segments("cargo test 2>&1"), vec!["cargo test 2>&1"]);
        assert_eq!(segments("cargo test &> log"), vec!["cargo test &> log"]);
        assert_eq!(segments("cargo test >& log"), vec!["cargo test >& log"]);
        // ...but a genuine `&` after one still splits.
        assert_eq!(
            segments("cargo test 2>&1 & other"),
            vec!["cargo test 2>&1", "other"]
        );
    }

    #[test]
    fn substitution_is_unenumerable_rather_than_a_segment() {
        for command in [
            "cargo test $(rm -rf /)",
            "cargo test `rm -rf /`",
            "cat <(rm -rf /)",
            "echo \"$(rm -rf /)\"",
            "echo \"`rm -rf /`\"",
        ] {
            assert_eq!(command_segments(command), None, "`{command}`");
        }
        // Inside single quotes it is literal text, not a substitution.
        assert_eq!(segments("echo 'a$(rm -rf /)'"), vec!["echo 'a$(rm -rf /)'"]);
    }

    #[test]
    fn an_unbalanced_quote_is_unenumerable() {
        assert_eq!(command_segments("echo \"unterminated"), None);
        assert_eq!(command_segments("echo 'unterminated"), None);
    }

    #[test]
    fn whitespace_only_segments_are_dropped() {
        assert_eq!(segments("cargo test;"), vec!["cargo test"]);
        assert_eq!(segments("  cargo test  "), vec!["cargo test"]);
        assert!(segments("").is_empty());
    }

    #[test]
    fn a_line_continuation_joins_the_lines() {
        assert_eq!(segments("cargo \\\ntest"), vec!["cargo test"]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_rm_rf_root_variants() {
        for cmd in ["rm -rf /", "rm -rf /*", "rm -rf ~", "rm -fr $HOME"] {
            assert!(
                validate_shell_command(cmd).is_err(),
                "expected block: {cmd}"
            );
        }
    }

    #[test]
    fn blocks_dangerous_device_redirects() {
        for cmd in ["> /dev/sda", "> /dev/mem", ">/dev/nvme0n1", ">>/dev/kmem"] {
            assert!(
                validate_shell_command(cmd).is_err(),
                "expected block: {cmd}"
            );
        }
    }

    #[test]
    fn allows_dev_null_redirect() {
        for cmd in [
            "> /dev/null",
            ">/dev/null",
            ">>/dev/null",
            "2>/dev/null",
            "2>&1 >/dev/null",
        ] {
            assert!(validate_shell_command(cmd).is_ok(), "expected allow: {cmd}");
        }
    }

    #[test]
    fn allows_benign_commands() {
        assert!(validate_shell_command("ls -la").is_ok());
        assert!(validate_shell_command("rm -rf ./build").is_ok());
    }
}

//! Shared shell command validation.

use anyhow::Result;

/// Programs that must always be blocked: a privilege escalation or a host power
/// transition is never something the agent should run, approved or not.
const BLOCKED_PROGRAMS: &[&str] = &["reboot", "shutdown", "sudo"];

/// Wrappers that do not change which program runs, so `nohup sudo …`,
/// `nice sudo …` and `env FOO=1 sudo …` are still blocked. A path-qualified
/// program is normalised to its basename before the comparison, so
/// `/usr/bin/sudo` is caught too.
const TRANSPARENT_PREFIXES: &[&str] = &[
    "busybox", "command", "env", "exec", "ionice", "nice", "nohup", "setsid", "stdbuf", "time",
    "xargs",
];

/// `rm` flags that make a delete recursive or forced. Either one is enough: the
/// destructive *target* is what keeps `rm -rf ./build` allowed, so demanding
/// both flags would only lose coverage of `rm -r /`.
const RECURSIVE_OR_FORCE_FLAGS: &[&str] =
    &["-f", "-r", "-R", "-rf", "-fr", "--force", "--recursive"];

/// Targets of a destructive `rm`, as whole words: the filesystem root and the
/// home directory. `${HOME}` is folded to `$home` and any `~user` form to `~`
/// before the comparison, so `rm -rf ~root` is caught as well.
const DESTRUCTIVE_RM_TARGETS: &[&str] = &["/", "/*", "~", "~/", "~/*", "$home", "$home/"];

/// The substring list that used to be the whole guard, kept for the one case
/// [`command_segments`] cannot enumerate (command substitution, backticks, an
/// unbalanced quote). There, there is no boundary to match words against, so we
/// stay deliberately conservative instead of silently widening.
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
///
/// Matching happens per `sh -c` segment and on whole words, never as a bare
/// substring of the joined command text. A substring test over the whole
/// string is wrong in both directions at once: it blocks `grep shutdown`,
/// `rg sudo`, `echo sudoku`, yet lets the same text through when it is spelled
/// `"shut""down"` or split by an operator. Segments and words are the boundary
/// the shell itself respects, which is what this guard matches on.
///
/// A command that cannot be enumerated falls back to the legacy substring scan:
/// substitution hides the very text the guard inspects, so widening there would
/// be guessing. This is a guard, not a sandbox — the cost of a false positive
/// is one approval, the cost of a false negative is a wiped machine.
pub fn is_high_risk_shell_command(command: &str) -> bool {
    match command_segments(command) {
        Some(segments) => segments.iter().any(|segment| segment_is_high_risk(segment)),
        None => {
            // Not word-split, so `/dev/null` cannot be removed at token
            // boundaries here; the cost is a false positive on a
            // `$(…) 2>/dev/null` line, which is the safe direction.
            let lower = command.to_ascii_lowercase();
            BLOCKED_SUBSTRINGS
                .iter()
                .any(|pattern| lower.contains(pattern))
        }
    }
}

/// True for one `sh -c` segment: a device write, a blocked program, or a
/// destructive `rm`.
fn segment_is_high_risk(segment: &str) -> bool {
    // `$HOME` was lowercased already, so normalise `${home}` → `$home` for the
    // whole-word comparisons below.
    let lower = segment.to_ascii_lowercase().replace("${home}", "$home");
    words_are_high_risk(&shell_words(&lower), 0)
}

/// The maximum `sh -c '…'` nesting inspected; deeper input is reported as
/// high-risk rather than recursed into without bound.
const MAX_NESTING: usize = 4;

/// True when this command line runs a device write, a blocked program, or a
/// destructive `rm` — recursing into a nested `sh -c '<script>'` operand, so
/// `bash -c 'rm -rf /'` is caught at the `rm` that actually runs.
fn words_are_high_risk(words: &[String], depth: usize) -> bool {
    if depth > MAX_NESTING {
        return true;
    }
    if writes_to_a_device(words) {
        return true;
    }
    // Program and `rm` resolve on the words the shell would actually execute —
    // redirections removed — so `> /dev/null sudo id` is `sudo`.
    let command_words = strip_redirections(words);
    if resolves_to_blocked_program(&command_words) {
        return true;
    }
    if is_destructive_rm(&command_words) {
        return true;
    }
    if let Some(idx) = effective_program_index(&command_words)
        && is_shell(program_basename(&command_words[idx]))
        && let Some(nested) = nested_shell_words(&command_words, idx)
    {
        return words_are_high_risk(&nested, depth + 1);
    }
    false
}

/// The basename of a program word: `/usr/bin/sudo` and `.\sudo` both name
/// `sudo`, and `programs` is compared against bare names only.
fn program_basename(word: &str) -> &str {
    word.rsplit(['/', '\\']).next().unwrap_or(word)
}

/// True when the word is one a *shell* would run its next argument as code.
fn is_shell(word: &str) -> bool {
    matches!(
        program_basename(word),
        "sh" | "bash" | "dash" | "zsh" | "ksh" | "eval"
    )
}

/// Wrapper flags whose *value* is a separate word (`env -u NAME`, `nice -n 10`,
/// `xargs -I {}`), so the word after them is not the program either. Encoding
/// the few that matter beats guessing every wrapper's flag arity — guessing
/// wrong skips the program itself (`nice sudo id` → `id`).
const FLAGS_TAKING_A_VALUE: &[&str] = &[
    "-I",
    "-L",
    "-P",
    "-a",
    "-d",
    "-n",
    "-s",
    "-u",
    "--adjustment",
    "--max-args",
    "--max-procs",
];

/// Words that are never the program: a flag, a `KEY=VALUE` assignment, or the
/// bare numeric argument a flag consumes (`nice -n 10 sudo`). Skipping these
/// uniformly avoids encoding every wrapper's flag arity — and the failure mode
/// of guessing wrong is skipping the program itself (`nice sudo id` → `id`).
fn is_not_the_program(word: &str) -> bool {
    let word = program_basename(word);
    word.starts_with('-')
        || is_env_assignment(word)
        || word.chars().all(|c| c.is_ascii_digit())
        || word.is_empty()
}

/// Split a shell word into the sub-words a *nested* command line holds, so a
/// quoted script (`sh -c 'sudo id'`) is inspected instead of treated as opaque.
/// `None` means the operand is not a plain command line (substitution or an
/// unbalanced quote), which callers must read as "cannot be verified".
fn nested_words(operand: &str) -> Option<Vec<String>> {
    command_segments(operand).map(|segments| {
        segments
            .iter()
            .flat_map(|segment| shell_words(segment))
            .collect()
    })
}

/// Walk a word list to the index of the program that actually runs, unwrapping
/// wrappers, their flags, `KEY=VALUE` assignments and `{}` placeholders.
///
/// The traversal is bounded: every iteration consumes at least one word.
fn effective_program_index(words: &[String]) -> Option<usize> {
    let mut idx = 0;
    while idx < words.len() {
        let word = &words[idx];
        if is_not_the_program(word) {
            idx += 1;
            continue;
        }
        let basename = program_basename(word);
        if TRANSPARENT_PREFIXES.contains(&basename) {
            idx += 1;
            // Flags and `KEY=VALUE` never name the program (`env -i sudo …`); a
            // flag known to carry a value also consumes the word after it
            // (`nice -n 10 sudo`, `xargs -I {} rm …`).
            while let Some(w) = words.get(idx) {
                if !is_not_the_program(w) {
                    break;
                }
                let takes_value = FLAGS_TAKING_A_VALUE.contains(&program_basename(w));
                idx += if takes_value { 2 } else { 1 };
            }
            continue;
        }
        return Some(idx);
    }
    None
}

/// The basename of the program this word list runs, or `None` when it names
/// none that can be resolved.
fn resolve_program(words: &[String]) -> Option<String> {
    effective_program_index(words).map(|idx| program_basename(&words[idx]).to_string())
}

/// The words of a nested `sh -c '<script>'` operand, or `None` when the word
/// list runs no shell with that form.
fn nested_shell_words(words: &[String], idx: usize) -> Option<Vec<String>> {
    let mut j = idx + 1;
    while let Some(word) = words.get(j) {
        if program_basename(word) == "-c" {
            return words.get(j + 1).and_then(|script| nested_words(script));
        }
        if is_not_the_program(word) {
            j += 1;
            continue;
        }
        return None;
    }
    None
}

/// True when the segment finally runs a blocked program.
fn resolves_to_blocked_program(words: &[String]) -> bool {
    resolve_program(words).is_some_and(|program| BLOCKED_PROGRAMS.contains(&program.as_str()))
}

/// True when the segment writes to a character/block device (or any path under
/// `/dev`). Matched on the words rather than the raw text, so a tab or a run of
/// spaces between `>` and the target cannot slip past, and `/dev/null` (the
/// usual stderr suppression) is not a device write at all.
fn writes_to_a_device(words: &[String]) -> bool {
    for (i, word) in words.iter().enumerate() {
        // `>`, `>>`, `1>`, `2>`, `&>` and the attached `>/dev/x` form all carry
        // the operator; anything after the last `>` is the target, or the next
        // word when the operator stands alone.
        let Some(operator) = word.rfind('>') else {
            continue;
        };
        let attached = &word[operator + 1..];
        let target = if attached.is_empty() {
            words.get(i + 1).map(String::as_str).unwrap_or("")
        } else {
            attached
        };
        if target.starts_with("/dev/") && target != "/dev/null" {
            return true;
        }
    }
    false
}

/// Drop redirection operators and their targets, so `> /dev/null sudo id`
/// resolves to `sudo` — which is what the shell actually runs.
fn strip_redirections(words: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(word) = words.get(i) {
        if word.contains('>') || word.contains('<') {
            // An attached target (`>/dev/null`) lives in the same word; a bare
            // operator leaves it as the next one.
            let attached = word.rsplit(['>', '<']).next().unwrap_or("");
            i += if attached.is_empty() { 2 } else { 1 };
            continue;
        }
        out.push(word.clone());
        i += 1;
    }
    out
}

/// True for an `rm` that force-deletes the filesystem root or the home
/// directory, in any flag order and including the split `-r -f` form.
/// `rm -rf ./build` and `rm -rf /tmp/x` stay allowed.
fn is_destructive_rm(words: &[String]) -> bool {
    if resolve_program(words).as_deref() != Some("rm")
        || !words
            .iter()
            .any(|word| RECURSIVE_OR_FORCE_FLAGS.contains(&word.as_str()))
    {
        return false;
    }
    words.iter().skip(1).any(|word| {
        if word.starts_with('-') {
            return false;
        }
        // `$HOME` was lowercased and `${home}` folded to `$home` already;
        // `~root` names a home directory too, so fold any `~user` form to `~`.
        let target = if word.starts_with('~') && !word.starts_with("~/") {
            "~"
        } else {
            word.as_str()
        };
        DESTRUCTIVE_RM_TARGETS.contains(&target) || resolves_to_root(target)
    })
}

/// True when an absolute path lexically reduces to `/`, so the `/tmp/../`
/// spelling of the root is blocked too. `..` cannot climb above `/`, which is
/// how the shell resolves it as well.
fn resolves_to_root(path: &str) -> bool {
    if !path.starts_with('/') {
        return false;
    }
    let mut depth: i64 = 0;
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => depth = (depth - 1).max(0),
            _ => depth += 1,
        }
    }
    depth == 0
}

/// `KEY=VALUE`, the form `env` accepts before the program it runs.
fn is_env_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Split a segment into words the way `sh` does, honouring quotes and
/// backslash escapes, so a token written inside quotes is still recognised as
/// the whole word it evaluates to.
fn shell_words(segment: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut has_word = false;
    let mut chars = segment.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            // Single quotes are literal throughout.
            '\'' => {
                loop {
                    match chars.next() {
                        Some('\'') | None => break,
                        Some(inner) => current.push(inner),
                    }
                }
                has_word = true;
            }
            '"' => {
                loop {
                    match chars.next() {
                        Some('"') | None => break,
                        // Inside double quotes only `"`, `\`, `$` and a
                        // backtick are escapable; the shell's own rule.
                        Some('\\') => match chars.peek().copied() {
                            Some(next @ ('"' | '\\' | '$' | '`')) => {
                                current.push(next);
                                chars.next();
                            }
                            Some('\n') => {
                                chars.next();
                            }
                            _ => current.push('\\'),
                        },
                        Some(inner) => current.push(inner),
                    }
                }
                has_word = true;
            }
            '\\' => match chars.next() {
                Some('\n') | None => {}
                Some(next) => current.push(next),
            },
            c if c.is_whitespace() => {
                if has_word {
                    words.push(std::mem::take(&mut current));
                    has_word = false;
                }
            }
            _ => {
                current.push(c);
                has_word = true;
            }
        }
    }
    if has_word {
        words.push(current);
    }
    words
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
    fn blocks_privilege_and_power_programs() {
        for cmd in [
            "sudo rm -rf /",
            "sudo -i",
            "shutdown -h now",
            "reboot",
            "nohup sudo echo hi",
            "env FOO=1 sudo echo hi",
        ] {
            assert!(
                validate_shell_command(cmd).is_err(),
                "expected block: {cmd}"
            );
        }
    }

    #[test]
    fn blocks_destructive_rm_variants() {
        for cmd in [
            "rm -rf /",
            "rm -rf /*",
            "rm -rf ~",
            "rm -fr ${HOME}",
            "rm -r /",
            "rm -f /",
            "rm -r -f /tmp/../",
        ] {
            assert!(
                validate_shell_command(cmd).is_err(),
                "expected block: {cmd}"
            );
        }
    }

    #[test]
    fn allows_benign_commands() {
        assert!(validate_shell_command("ls -la").is_ok());
        assert!(validate_shell_command("rm -rf ./build").is_ok());
    }

    /// The whole point of matching per word: ordinary text that merely contains
    /// a blocked name must run.
    #[test]
    fn allows_blocked_names_as_arguments_or_substrings() {
        for cmd in [
            "echo shutdown",
            "echo sudoku",
            "grep -rn shutdown .",
            "rg sudo crates/",
            "git log --grep reboot",
            "echo \"shut\"\"down\"",
        ] {
            assert!(validate_shell_command(cmd).is_ok(), "expected allow: {cmd}");
        }
    }

    /// Segments are the boundary the shell respects, so a blocked program is
    /// caught wherever it really runs.
    #[test]
    fn blocks_a_blocked_program_in_any_segment() {
        for cmd in [
            "ls; sudo echo hi",
            "true && shutdown -h now",
            "cat file | sudo tee out",
        ] {
            assert!(
                validate_shell_command(cmd).is_err(),
                "expected block: {cmd}"
            );
        }
    }

    /// Every bypass this rewrite's first cut introduced (found by review, each
    /// one passing under the OLD substring guard, so none may regress).
    #[test]
    fn blocks_the_spellings_a_bare_word_match_would_miss() {
        for cmd in [
            "/bin/rm -rf /",
            "/usr/bin/rm -rf /",
            "xargs rm -rf /",
            "busybox rm -rf /",
            "nice rm -rf /",
            "nice sudo id",
            "setsid sudo id",
            "stdbuf -o0 sudo id",
            "env -i sudo id",
            "env -u FOO rm -rf ~",
            "nice -n 10 sudo id",
            "time -p sudo id",
            "sh -c 'sudo id'",
            "bash -c 'rm -rf /'",
            "nohup sh -c 'sudo id'",
            "/usr/bin/sudo id",
            "/bin/shutdown -h now",
            "rm -rf ~root",
            "rm -rf ~/*",
            ">  /dev/sda",
            ">\t/dev/sda",
            "2>/dev/sda",
            "> /dev/null sudo id",
        ] {
            assert!(
                validate_shell_command(cmd).is_err(),
                "expected block: {cmd}"
            );
        }
    }

    /// The whole reason `/dev/null` is special-cased: the usual stderr
    /// suppression must not count as a device write, and the literal quoted
    /// form is not a redirection.
    #[test]
    fn device_and_dev_null_boundaries_stay_correct() {
        for cmd in ["> /dev/null", ">/dev/null", ">>/dev/null", "2>/dev/null"] {
            assert!(validate_shell_command(cmd).is_ok(), "expected allow: {cmd}");
        }
        assert!(validate_shell_command("echo '> /dev/sda'").is_ok());
    }

    /// Names that merely *contain* a blocked word stay allowed — the false
    /// positives the substring guard produced.
    #[test]
    fn allows_names_that_contain_a_blocked_word() {
        for cmd in [
            "echo sudoers",
            "systemctl status shutdown.service",
            "rebootctl",
        ] {
            assert!(validate_shell_command(cmd).is_ok(), "expected allow: {cmd}");
        }
    }

    /// Substitution hides the text being inspected, so the conservative
    /// substring scan still applies.
    #[test]
    fn blocks_substitutions_conservatively() {
        for cmd in ["echo $(sudo id)", "echo `shutdown -h now`", "ls; rm -rf /"] {
            assert!(
                validate_shell_command(cmd).is_err(),
                "expected block: {cmd}"
            );
        }
    }
}

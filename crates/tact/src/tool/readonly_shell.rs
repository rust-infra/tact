//! Conservative read-only classification for shell command strings.
//!
//! Plan mode denies every `Write`-classified tool, so `bash(command: "ls")`
//! is blocked there. To let `ls` / `grep`-style inspection through,
//! [`PermissionPolicy::ShellCommand`](super::metadata::PermissionPolicy)
//! classifies a command string as `Read` when — and only when — it provably
//! cannot modify anything.
//!
//! The classifier is deliberately conservative: false negatives (a safe
//! command classified as `Write`) only cost the user an approval prompt;
//! false positives (a mutating command classified as `Read`) would run
//! silently under plan mode, so anything remotely ambiguous is rejected.
//!
//! Two stages:
//!
//! 1. [`split_plain_command`] accepts only *plain commands*: whitespace-
//!    separated words where every word is bare text or fully quoted, and no
//!    shell metacharacter appears anywhere (`; & | > < $ backtick \`,
//!    globs, braces, parentheses, `!`; a leading `~` is harmless — tilde
//!    expansion only substitutes the home directory, and every safelisted
//!    program stays read-only on the expanded result). Redirections, pipes,
//!    command substitution, glob expansion and escapes are all rejected —
//!    the split must not be able to disagree with what `sh -c` would
//!    actually run.
//! 2. [`is_known_safe_command`] matches the first word against a safelist of
//!    programs whose *options alone* cannot write, plus option-level checks
//!    for programs with dangerous flags (`find`, `rg`, `git`, `sed`,
//!    `base64`).
//!
//! The safelist and its option rules mirror OpenAI Codex's
//! `is_known_safe_command`
//! (`codex-rs/shell-command/src/command_safety/is_safe_command.rs`), which is
//! built for exactly this purpose: auto-approving read-only commands.

const UNSAFE_FIND_OPTIONS: &[&str] = &[
    // Options that can execute arbitrary commands.
    "-exec", "-execdir", "-ok", "-okdir",  // Option that deletes matching files.
    "-delete", // Options that write pathnames to a file.
    "-fls", "-fprint", "-fprint0", "-fprintf",
];

const UNSAFE_RG_OPTIONS_WITH_ARGS: &[&str] = &[
    // Takes an arbitrary command that is executed for each match.
    "--pre",
    // Takes a command that can be used to obtain the local hostname.
    "--hostname-bin",
];

const UNSAFE_RG_OPTIONS_WITHOUT_ARGS: &[&str] = &[
    // Calls out to other decompression tools, so do not auto-approve out of
    // an abundance of caution.
    "--search-zip",
    "-z",
];

/// Returns `true` when `command` is provably read-only and thus safe to run
/// without a write approval.
pub fn is_read_only_shell_command(command: &str) -> bool {
    let Some(words) = split_plain_command(command) else {
        return false;
    };
    is_known_safe_command(&words)
}

/// Split a command string into words, returning `None` whenever the string
/// uses any shell feature whose effect on the word list is not trivially
/// visible to this parser.
fn split_plain_command(command: &str) -> Option<Vec<String>> {
    let mut words: Vec<String> = Vec::new();
    let mut chars = command.chars().peekable();

    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let Some(&first) = chars.peek() else {
            break;
        };

        let word = match first {
            // Double quotes: only plain characters are safe; expansions and
            // escapes change the result in ways we do not model.
            '"' => {
                chars.next();
                let mut s = String::new();
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('$') | Some('`') | Some('\\') => return None,
                        Some(other) => s.push(other),
                        None => return None,
                    }
                }
                s
            }
            // Bare word: non-whitespace characters, with embedded
            // single-quoted segments (strictly literal, safe to
            // concatenate). Any other shell metacharacter is rejected.
            _ => {
                let mut s = String::new();
                loop {
                    match chars.peek() {
                        None => break,
                        Some(next) if next.is_whitespace() => break,
                        Some('\'') => {
                            chars.next();
                            loop {
                                match chars.next() {
                                    Some('\'') => break,
                                    Some(other) => s.push(other),
                                    None => return None, // unterminated quote
                                }
                            }
                        }
                        Some(next) if is_shell_metacharacter(*next) => return None,
                        Some(next) => {
                            s.push(*next);
                            chars.next();
                        }
                    }
                }
                s
            }
        };
        words.push(word);
    }

    if words.is_empty() { None } else { Some(words) }
}

fn is_shell_metacharacter(c: char) -> bool {
    matches!(
        c,
        '\'' | '"'
            | '\\'
            | ';'
            | '&'
            | '|'
            | '>'
            | '<'
            | '$'
            | '`'
            | '*'
            | '?'
            | '['
            | ']'
            | '{'
            | '}'
            | '('
            | ')'
            | '!'
    )
}

/// Classify an already-split plain command by its first word.
fn is_known_safe_command(words: &[String]) -> bool {
    let Some(cmd0) = words.first().map(String::as_str) else {
        return false;
    };

    match cmd0 {
        // Programs with no option that writes anything.
        "cat" | "cd" | "cut" | "echo" | "expr" | "false" | "grep" | "head" | "id" | "ls" | "nl"
        | "paste" | "pwd" | "rev" | "seq" | "stat" | "tail" | "tr" | "true" | "uname" | "uniq"
        | "wc" | "which" | "whoami" => true,

        "base64" => {
            // `-o` / `--output` write decoded data to a file.
            !words.iter().skip(1).any(|arg| {
                arg == "-o"
                    || arg == "--output"
                    || arg.starts_with("--output=")
                    || (arg.starts_with("-o") && arg != "-o")
            })
        }

        "find" => !words
            .iter()
            .skip(1)
            .any(|arg| UNSAFE_FIND_OPTIONS.contains(&arg.as_str())),

        "rg" => !words.iter().skip(1).any(|arg| {
            UNSAFE_RG_OPTIONS_WITH_ARGS.contains(&arg.as_str())
                || arg.starts_with("--pre=")
                || arg.starts_with("--hostname-bin=")
                || UNSAFE_RG_OPTIONS_WITHOUT_ARGS.contains(&arg.as_str())
        }),

        "git" => is_safe_git_command(words),

        // Special-case `sed -n {N|M,N}p` (print-only line ranges).
        "sed"
            if words.len() <= 4
                && words.get(1).map(String::as_str) == Some("-n")
                && is_valid_sed_n_arg(words.get(2).map(String::as_str)) =>
        {
            true
        }

        _ => false,
    }
}

// ---------------------------------------------------------------------------
// git
// ---------------------------------------------------------------------------

const GIT_READ_ONLY_SUBCOMMANDS: &[&str] = &["status", "log", "diff", "show", "branch"];

fn is_safe_git_command(words: &[String]) -> bool {
    let Some((subcommand_idx, subcommand)) = find_git_subcommand(words, GIT_READ_ONLY_SUBCOMMANDS)
    else {
        return false;
    };

    let global_args = &words[1..subcommand_idx];
    if git_has_unsafe_global_option(global_args) {
        return false;
    }

    let subcommand_args = &words[subcommand_idx + 1..];
    match subcommand {
        "status" | "log" | "diff" | "show" => git_subcommand_args_are_read_only(subcommand_args),
        "branch" => {
            git_subcommand_args_are_read_only(subcommand_args)
                && git_branch_is_read_only(subcommand_args)
        }
        _ => false,
    }
}

/// Find the git subcommand, skipping known global options that may appear
/// before it (e.g. `-C`, `-c`, `--git-dir`). The first non-option token must
/// be the subcommand; otherwise later positional arguments (branch names,
/// paths) could be misclassified.
fn find_git_subcommand<'a>(words: &'a [String], subcommands: &[&str]) -> Option<(usize, &'a str)> {
    let mut skip_next = false;
    for (idx, arg) in words.iter().enumerate().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }
        let arg = arg.as_str();

        if is_git_global_option_with_inline_value(arg) {
            continue;
        }
        if is_git_global_option_with_value(arg) {
            skip_next = true;
            continue;
        }
        // Any other flag: skip it here; if it sits in the global section it
        // is later rejected by `git_has_unsafe_global_option`.
        if arg == "--" || arg.starts_with('-') {
            continue;
        }
        if subcommands.contains(&arg) {
            return Some((idx, arg));
        }
        return None;
    }
    None
}

fn is_git_global_option_with_value(arg: &str) -> bool {
    matches!(
        arg,
        "-C" | "-c"
            | "--config-env"
            | "--exec-path"
            | "--git-dir"
            | "--namespace"
            | "--super-prefix"
            | "--work-tree"
    )
}

fn is_git_global_option_with_inline_value(arg: &str) -> bool {
    arg.starts_with("--config-env=")
        || arg.starts_with("--exec-path=")
        || arg.starts_with("--git-dir=")
        || arg.starts_with("--namespace=")
        || arg.starts_with("--super-prefix=")
        || arg.starts_with("--work-tree=")
        || ((arg.starts_with("-C") || arg.starts_with("-c")) && arg.len() > 2)
}

#[derive(Clone, Copy)]
enum GitOptionPattern {
    Exact(&'static str),
    ShortWithInlineValue(&'static str),
    Prefix(&'static str),
}

/// Global options that can change which repository is operated on or how
/// output is produced; any of them before the subcommand disqualifies the
/// command.
const UNSAFE_GIT_GLOBAL_OPTIONS: &[GitOptionPattern] = &[
    GitOptionPattern::Exact("-C"),
    GitOptionPattern::ShortWithInlineValue("-C"),
    GitOptionPattern::Exact("-c"),
    GitOptionPattern::ShortWithInlineValue("-c"),
    GitOptionPattern::Exact("-p"),
    GitOptionPattern::Exact("--config-env"),
    GitOptionPattern::Prefix("--config-env="),
    GitOptionPattern::Exact("--exec-path"),
    GitOptionPattern::Prefix("--exec-path="),
    GitOptionPattern::Exact("--git-dir"),
    GitOptionPattern::Prefix("--git-dir="),
    GitOptionPattern::Exact("--namespace"),
    GitOptionPattern::Prefix("--namespace="),
    GitOptionPattern::Exact("--paginate"),
    GitOptionPattern::Exact("--super-prefix"),
    GitOptionPattern::Prefix("--super-prefix="),
    GitOptionPattern::Exact("--work-tree"),
    GitOptionPattern::Prefix("--work-tree="),
];

/// Subcommand options that write to files or execute external programs.
const UNSAFE_GIT_SUBCOMMAND_OPTIONS: &[GitOptionPattern] = &[
    GitOptionPattern::Exact("--output"),
    GitOptionPattern::Prefix("--output="),
    GitOptionPattern::Exact("--ext-diff"),
    GitOptionPattern::Exact("--textconv"),
    GitOptionPattern::Exact("--exec"),
    GitOptionPattern::Prefix("--exec="),
];

impl GitOptionPattern {
    fn matches(self, arg: &str) -> bool {
        match self {
            GitOptionPattern::Exact(option) => arg == option,
            GitOptionPattern::ShortWithInlineValue(option) => {
                arg.starts_with(option) && arg.len() > option.len()
            }
            GitOptionPattern::Prefix(prefix) => arg.starts_with(prefix),
        }
    }
}

fn git_matches_option_pattern(arg: &str, patterns: &[GitOptionPattern]) -> bool {
    patterns.iter().any(|pattern| pattern.matches(arg))
}

fn git_has_unsafe_global_option(global_args: &[String]) -> bool {
    global_args
        .iter()
        .map(String::as_str)
        .any(|arg| git_matches_option_pattern(arg, UNSAFE_GIT_GLOBAL_OPTIONS))
}

fn git_subcommand_args_are_read_only(args: &[String]) -> bool {
    !args
        .iter()
        .map(String::as_str)
        .any(|arg| git_matches_option_pattern(arg, UNSAFE_GIT_SUBCOMMAND_OPTIONS))
}

/// Treat `git branch` as read-only only when the arguments clearly indicate
/// a query, not a branch mutation (create/rename/delete).
fn git_branch_is_read_only(branch_args: &[String]) -> bool {
    if branch_args.is_empty() {
        // `git branch` with no additional args lists branches.
        return true;
    }

    let mut saw_read_only_flag = false;
    for arg in branch_args.iter().map(String::as_str) {
        match arg {
            "--list" | "-l" | "--show-current" | "-a" | "--all" | "-r" | "--remotes" | "-v"
            | "-vv" | "--verbose" => {
                saw_read_only_flag = true;
            }
            _ if arg.starts_with("--format=") => {
                saw_read_only_flag = true;
            }
            _ => {
                // Any other flag or positional argument may create, rename,
                // or delete branches.
                return false;
            }
        }
    }

    saw_read_only_flag
}

// ---------------------------------------------------------------------------
// sed
// ---------------------------------------------------------------------------

/// Returns true if `arg` matches /^(\d+,)?\d+p$/ (e.g. `10p` or `1,5p`).
fn is_valid_sed_n_arg(arg: Option<&str>) -> bool {
    let Some(s) = arg else {
        return false;
    };
    let Some(core) = s.strip_suffix('p') else {
        return false;
    };
    let parts: Vec<&str> = core.split(',').collect();
    match parts.as_slice() {
        [num] => !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()),
        [a, b] => {
            !a.is_empty()
                && !b.is_empty()
                && a.chars().all(|c| c.is_ascii_digit())
                && b.chars().all(|c| c.is_ascii_digit())
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_ro(command: &str) -> bool {
        is_read_only_shell_command(command)
    }

    #[test]
    fn plain_read_only_commands_are_classified_read_only() {
        for cmd in [
            "ls",
            "ls -la",
            "ls /tmp /etc",
            "grep -rn needle .",
            "grep -rn 'needle hay' .",
            "grep -rn \"needle hay\" .",
            "cat Cargo.toml",
            "head -20 f",
            "tail -n 5 f",
            "wc -l a b",
            "pwd",
            "which python3",
            "echo hello",
            "true",
            "false",
            "uname -a",
            "stat f",
            "whoami",
            "base64 f",
            "sed -n 5p f",
            "sed -n '1,5p' f",
            "sed -n 5p",
        ] {
            assert!(is_ro(cmd), "expected read-only: {cmd}");
        }
    }

    #[test]
    fn shell_metacharacters_are_never_read_only() {
        for cmd in [
            "ls; rm -rf /",
            "ls | wc -l",
            "echo hi > /tmp/f",
            "echo hi >> /tmp/f",
            "cat < /tmp/f",
            "ls *.rs",
            "ls dir?/f",
            "ls dir[ab]/f",
            "echo $(id)",
            "echo `id`",
            "echo $HOME",
            "echo a\\nb",
            "echo hi &",
            "ls && ls",
            "echo {a,b}",
            "echo (id)",
            "ls 'unterminated",
            "ls \"unterminated",
            "echo \"$HOME\"",
        ] {
            assert!(!is_ro(cmd), "expected NOT read-only: {cmd}");
        }
    }

    #[test]
    fn unknown_or_mutating_programs_are_not_read_only() {
        for cmd in [
            "",
            "   ",
            "rm -rf /",
            "mkdir foo",
            "touch foo",
            "mv a b",
            "cp a b",
            "chmod +x f",
            "cargo test",
            "cargo build",
            "curl -o f https://x",
            "git push",
            "git commit -m x",
            "git checkout -b x",
            "git merge main",
            "sudo ls",
            "su root",
        ] {
            assert!(!is_ro(cmd), "expected NOT read-only: {cmd}");
        }
    }

    #[test]
    fn find_dangerous_options_are_not_read_only() {
        assert!(is_ro("find . -name '*.rs'"));
        assert!(is_ro("find . -maxdepth 2 -type f"));
        for cmd in [
            "find . -delete",
            "find . -exec rm {} \\;",
            "find . -execdir sh -c 'x'",
            "find . -ok rm {} \\;",
            "find . -okdir true \\;",
            "find . -fls out",
            "find . -fprint out",
            "find . -fprint0 out",
            "find . -fprintf out '%p'",
        ] {
            assert!(!is_ro(cmd), "expected NOT read-only: {cmd}");
        }
    }

    #[test]
    fn rg_dangerous_options_are_not_read_only() {
        assert!(is_ro("rg -n init ."));
        assert!(is_ro("rg --files --max-depth 2 ."));
        for cmd in [
            "rg --pre cmd init .",
            "rg --pre=cmd init .",
            "rg --hostname-bin cmd init .",
            "rg --hostname-bin=cmd init .",
            "rg --search-zip init .",
            "rg -z init .",
        ] {
            assert!(!is_ro(cmd), "expected NOT read-only: {cmd}");
        }
    }

    #[test]
    fn git_read_only_subcommands_are_read_only() {
        for cmd in [
            "git status",
            "git status -s",
            "git log",
            "git log --oneline -5",
            "git diff",
            "git diff HEAD~1",
            "git show",
            "git show HEAD:src/lib.rs",
            "git branch",
            "git branch --show-current",
            "git branch -a",
            "git branch --format='%(refname)'",
        ] {
            assert!(is_ro(cmd), "expected read-only: {cmd}");
        }
    }

    #[test]
    fn git_mutating_commands_are_not_read_only() {
        for cmd in [
            "git fetch",
            "git pull",
            "git push",
            "git status --output=f",
            "git status --output f",
            "git diff --ext-diff",
            "git diff --textconv",
            "git show --exec=cmd",
            "git -C . status",
            "git -c x=y status",
            "git --paginate status",
            "git branch -d foo",
            "git branch -D foo",
            "git branch -m a b",
            "git branch foo",
            "git branch --delete foo",
        ] {
            assert!(!is_ro(cmd), "expected NOT read-only: {cmd}");
        }
    }

    #[test]
    fn sed_only_print_line_ranges_are_read_only() {
        assert!(is_ro("sed -n 10p f"));
        assert!(is_ro("sed -n 1,5p f"));
        for cmd in [
            "sed -n xp f",
            "sed -n '1,5' f",
            "sed -i s/a/b/ f",
            "sed s/a/b/ f",
            "sed -n 1,5p a b",
        ] {
            assert!(!is_ro(cmd), "expected NOT read-only: {cmd}");
        }
    }

    #[test]
    fn base64_output_options_are_not_read_only() {
        assert!(is_ro("base64 f"));
        for cmd in [
            "base64 -o out f",
            "base64 --output out f",
            "base64 --output=out f",
            "base64 -oout f",
        ] {
            assert!(!is_ro(cmd), "expected NOT read-only: {cmd}");
        }
    }
}

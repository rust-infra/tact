//! Paths and shell commands that are secret by their nature.
//!
//! `tool::safe_path` keeps the file tools inside the workspace, but nothing
//! stops `bash "cat ~/.ssh/id_ed25519"`: the command is classified `Read` by
//! `readonly_shell`, and `PermissionManager::check_with_auto` returns `Allow`
//! for `Read` **before** plan mode and before any settings rule. This module is
//! the registry that closes that hole, plus the classifier the permission layer
//! and the redactor both consult.
//!
//! # Two tiers
//!
//! "Sensitive" spans two very different things, and collapsing them gives
//! either a useless warning or a wall:
//!
//! - [`Tier::Credential`] — the file *is* the secret (private keys, token
//!   stores). There is no legitimate agent read, so the decision is a refusal,
//!   and the escape hatch is a hand-written `permissions.sensitive_paths.allow`
//!   entry rather than a one-click "Always allow" (a refusal whose escape is one
//!   click away is not a refusal).
//! - [`Tier::Secret`] — reading it *may* expose a secret, and the user may well
//!   want it (`.env` during a config task, shell history during a debugging
//!   task). The decision is the ordinary `High` prompt, so `deny`/`ask` rules,
//!   plan mode and "Always allow" all keep working.
//!
//! # What this is not
//!
//! [`classify_command`] is a heuristic. It does not resolve command
//! substitution, variables or `python -c`. A determined path gets past it; the
//! backstop for that is `crate::security::redact`, and the real boundary is
//! `crate::sandbox` (Linux-only, opt-in today).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use globset::{Glob, GlobMatcher};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// How badly a hit should hurt. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Refused outright, before modes, hooks and rules.
    Credential,
    /// Escalated to `High`: asks in Default mode, denied in Plan and headless.
    Secret,
}

/// What kind of secret this is — carried into the refusal text and the
/// redaction marker so both stay legible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensitiveKind {
    PrivateKey,
    CredentialStore,
    AgentConfig,
    EnvFile,
    ShellHistory,
    KeyMaterial,
}

impl SensitiveKind {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::PrivateKey => "private-key",
            Self::CredentialStore => "credential-store",
            Self::AgentConfig => "agent-config",
            Self::EnvFile => "env-file",
            Self::ShellHistory => "shell-history",
            Self::KeyMaterial => "key-material",
        }
    }
}

/// A classification result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub tier: Tier,
    pub kind: SensitiveKind,
    /// The path or token that matched, for the reason text.
    pub matched: String,
    /// The registry glob responsible, for the reason text.
    pub pattern: String,
}

// ---------------------------------------------------------------------------
// The registry
// ---------------------------------------------------------------------------

/// Where a rule's glob applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Matched against the path relative to `$HOME`.
    Home,
    /// Matched against the final path component, anywhere on the filesystem.
    Name,
}

struct Rule {
    scope: Scope,
    glob: &'static str,
    tier: Tier,
    kind: SensitiveKind,
}

const fn rule(scope: Scope, glob: &'static str, tier: Tier, kind: SensitiveKind) -> Rule {
    Rule {
        scope,
        glob,
        tier,
        kind,
    }
}

use Scope::{Home, Name};
use SensitiveKind as K;
use Tier::{Credential, Secret};

/// The registry, **in priority order**: the first match wins, so a narrow
/// exception must precede the directory glob it carves out of (which is why
/// `~/.ssh/config` is listed above `~/.ssh/**`).
///
/// Names matching [`EXEMPT_NAMES`] are checked before any of these.
const RULES: &[Rule] = &[
    // --- `~/.ssh`: everything is key material except the three public files ---
    rule(Home, ".ssh/config", Secret, K::CredentialStore),
    rule(Home, ".ssh/known_hosts", Secret, K::CredentialStore),
    rule(Home, ".ssh/known_hosts.*", Secret, K::CredentialStore),
    rule(Home, ".ssh/**", Credential, K::PrivateKey),
    rule(Home, ".ssh", Credential, K::PrivateKey),
    // --- single-file credential stores in `$HOME` ---
    rule(Home, ".netrc", Credential, K::CredentialStore),
    rule(Home, "_netrc", Credential, K::CredentialStore),
    rule(Home, ".authinfo", Credential, K::CredentialStore),
    rule(Home, ".authinfo.gpg", Credential, K::CredentialStore),
    rule(Home, ".msmtprc", Credential, K::CredentialStore),
    rule(Home, ".fetchmailrc", Credential, K::CredentialStore),
    rule(Home, ".git-credentials", Credential, K::CredentialStore),
    rule(Home, ".npmrc", Credential, K::CredentialStore),
    rule(Home, ".pypirc", Credential, K::CredentialStore),
    rule(Home, ".pgpass", Credential, K::CredentialStore),
    rule(Home, ".my.cnf", Credential, K::CredentialStore),
    rule(Home, ".s3cfg", Credential, K::CredentialStore),
    rule(Home, ".boto", Credential, K::CredentialStore),
    rule(Home, ".vault-token", Credential, K::CredentialStore),
    rule(Home, ".gem/credentials", Credential, K::CredentialStore),
    rule(
        Home,
        ".cargo/credentials.toml",
        Credential,
        K::CredentialStore,
    ),
    rule(Home, ".cargo/credentials", Credential, K::CredentialStore),
    rule(Home, ".huggingface/token", Credential, K::CredentialStore),
    // --- credential directories in `$HOME` ---
    rule(Home, ".aws/**", Credential, K::CredentialStore),
    rule(Home, ".aws", Credential, K::CredentialStore),
    rule(Home, ".gnupg/**", Credential, K::PrivateKey),
    rule(Home, ".gnupg", Credential, K::PrivateKey),
    rule(Home, ".kube/**", Credential, K::CredentialStore),
    rule(Home, ".kube", Credential, K::CredentialStore),
    rule(Home, ".docker/**", Credential, K::CredentialStore),
    rule(Home, ".docker", Credential, K::CredentialStore),
    rule(Home, ".terraform.d/**", Credential, K::CredentialStore),
    rule(Home, ".config/gh/**", Credential, K::CredentialStore),
    rule(Home, ".config/gcloud/**", Credential, K::CredentialStore),
    rule(Home, ".config/heroku/**", Credential, K::CredentialStore),
    rule(Home, ".heroku/**", Credential, K::CredentialStore),
    rule(Home, ".config/op/**", Credential, K::CredentialStore),
    rule(Home, ".azure/**", Credential, K::CredentialStore),
    // --- agent-host config that carries `env` tokens (the file that leaked) ---
    rule(Home, ".claude/settings.json", Credential, K::AgentConfig),
    rule(
        Home,
        ".claude/.credentials.json",
        Credential,
        K::AgentConfig,
    ),
    rule(Home, ".claude.json", Credential, K::AgentConfig),
    rule(Home, ".codex/auth.json", Credential, K::AgentConfig),
    rule(Home, ".tact/settings.json", Credential, K::AgentConfig),
    rule(Home, ".agents/**", Credential, K::AgentConfig),
    // --- key material and secret stores, by filename, anywhere ---
    rule(Name, "id_rsa", Credential, K::PrivateKey),
    rule(Name, "id_dsa", Credential, K::PrivateKey),
    rule(Name, "id_ecdsa", Credential, K::PrivateKey),
    rule(Name, "id_ed25519", Credential, K::PrivateKey),
    rule(Name, "*_rsa", Credential, K::PrivateKey),
    rule(Name, "*_dsa", Credential, K::PrivateKey),
    rule(Name, "*_ecdsa", Credential, K::PrivateKey),
    rule(Name, "*_ed25519", Credential, K::PrivateKey),
    rule(Name, "*.pem", Credential, K::PrivateKey),
    rule(Name, "*.key", Credential, K::PrivateKey),
    rule(Name, "*.p12", Credential, K::PrivateKey),
    rule(Name, "*.pfx", Credential, K::PrivateKey),
    rule(Name, "*.jks", Credential, K::PrivateKey),
    rule(Name, "*.keystore", Credential, K::PrivateKey),
    rule(Name, "*.ppk", Credential, K::PrivateKey),
    rule(Name, "*.kdbx", Credential, K::PrivateKey),
    rule(Name, "*.tfstate", Credential, K::KeyMaterial),
    rule(Name, "*.tfstate.backup", Credential, K::KeyMaterial),
    rule(Name, "*.tfvars", Credential, K::KeyMaterial),
    rule(Name, "credentials.json", Credential, K::CredentialStore),
    rule(Name, "credential.json", Credential, K::CredentialStore),
    rule(
        Name,
        "service-account*.json",
        Credential,
        K::CredentialStore,
    ),
    rule(
        Name,
        "service_account*.json",
        Credential,
        K::CredentialStore,
    ),
    rule(Name, "secrets.json", Credential, K::KeyMaterial),
    rule(Name, "secrets.yml", Credential, K::KeyMaterial),
    rule(Name, "secrets.yaml", Credential, K::KeyMaterial),
    rule(Name, "secrets.toml", Credential, K::KeyMaterial),
    rule(Name, "secret.json", Credential, K::KeyMaterial),
    rule(Name, "secret.yml", Credential, K::KeyMaterial),
    // In-repo copies of the single-file stores above: a checked-in `.npmrc`
    // carrying `_authToken` is a real leak, and the ask is cheap.
    rule(Name, ".npmrc", Credential, K::CredentialStore),
    rule(Name, ".pypirc", Credential, K::CredentialStore),
    rule(Name, ".pgpass", Credential, K::CredentialStore),
    rule(Name, ".netrc", Credential, K::CredentialStore),
    rule(Name, ".git-credentials", Credential, K::CredentialStore),
    rule(Name, ".htpasswd", Credential, K::CredentialStore),
    rule(Name, ".my.cnf", Credential, K::CredentialStore),
    rule(Name, ".s3cfg", Credential, K::CredentialStore),
    rule(Name, ".boto", Credential, K::CredentialStore),
    // --- `Secret` tier: ask, because the user may legitimately want it ---
    rule(Name, ".env", Secret, K::EnvFile),
    rule(Name, ".env.*", Secret, K::EnvFile),
    rule(Name, "*.env", Secret, K::EnvFile),
    rule(Name, ".envrc", Secret, K::EnvFile),
    rule(Home, ".direnv/**", Secret, K::EnvFile),
    rule(Home, ".bash_history", Secret, K::ShellHistory),
    rule(Home, ".zsh_history", Secret, K::ShellHistory),
    rule(Home, ".sh_history", Secret, K::ShellHistory),
    rule(Home, ".python_history", Secret, K::ShellHistory),
    rule(Home, ".psql_history", Secret, K::ShellHistory),
    rule(Home, ".mysql_history", Secret, K::ShellHistory),
    rule(Home, ".sqlite_history", Secret, K::ShellHistory),
    rule(Home, ".node_repl_history", Secret, K::ShellHistory),
    rule(Home, ".irb_history", Secret, K::ShellHistory),
    rule(Home, ".wget-hsts", Secret, K::ShellHistory),
];

/// Filenames that are public or are committed placeholders, checked before
/// [`RULES`].
///
/// `*.crt` / `*.cer` / `*.der` are certificates — public by definition. `*.pub`
/// is a public key. The `*.example`/`*.sample`/`*.template`/`*.dist` forms are
/// placeholders (`config.example.toml`, `.env.example`), which would otherwise
/// make every repository holding one prompt on every read.
///
/// `*.pem` is deliberately **not** here: it is far more often a private key
/// than a certificate chain, and a needless prompt costs one click, while
/// missing one costs the secret. The `allow` list covers the certbot case.
const EXEMPT_NAMES: &[&str] = &[
    "*.pub",
    "*.crt",
    "*.cer",
    "*.der",
    "*.example",
    "*.sample",
    "*.template",
    "*.dist",
];

/// Bare words (no `.`, no `/`) that a shell command can use to name a secret
/// directly, e.g. `cat id_rsa`.
///
/// Command tokenisation splits on `=`, so matching *every* bare word against
/// the filename globs would refuse `rg "foo_rsa" crates/` — a false positive
/// that costs a refusal, not a prompt. Only these exact names are honoured
/// without a `.` or `/`.
const BARE_SECRET_NAMES: &[&str] = &[
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "authorized_keys",
    "known_hosts",
    "credentials",
    "netrc",
];

// ---------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------

fn matcher(pattern: &str) -> Option<GlobMatcher> {
    Glob::new(pattern).ok().map(|g| g.compile_matcher())
}

/// Where a user-supplied pattern (`extra`, `allow`, `basic_only_paths`)
/// applies.
///
/// A leading `~/` makes it home-relative and the tilde is **stripped** before
/// compiling — a glob containing a literal `~` would never match the relative
/// path it is compared against. A leading `/` makes it absolute; a pattern that
/// merely *contains* a `/` is a path glob (`**/fixtures/**`) and is matched
/// against the whole path, because a pattern with a separator in it can never
/// match a bare filename. Anything else matches the final component.
enum UserScope<'a> {
    Home(&'a str),
    Absolute(&'a str),
    Path(&'a str),
    Name(&'a str),
}

fn user_scope(pattern: &str) -> Option<UserScope<'_>> {
    let pattern = pattern.trim();
    if pattern.is_empty() || pattern == "~" {
        return None;
    }
    if let Some(rest) = pattern.strip_prefix("~/") {
        return Some(UserScope::Home(rest));
    }
    if pattern.starts_with('/') {
        return Some(UserScope::Absolute(pattern));
    }
    if pattern.contains('/') {
        return Some(UserScope::Path(pattern));
    }
    Some(UserScope::Name(pattern))
}

/// Compiled `(rule index, matcher)` pairs, in priority order.
fn compiled_rules() -> &'static [(usize, GlobMatcher)] {
    static COMPILED: OnceLock<Vec<(usize, GlobMatcher)>> = OnceLock::new();
    COMPILED.get_or_init(|| {
        RULES
            .iter()
            .enumerate()
            .filter_map(|(i, r)| matcher(r.glob).map(|m| (i, m)))
            .collect()
    })
}

fn compiled_exempt() -> &'static [GlobMatcher] {
    static COMPILED: OnceLock<Vec<GlobMatcher>> = OnceLock::new();
    COMPILED.get_or_init(|| EXEMPT_NAMES.iter().filter_map(|p| matcher(p)).collect())
}

/// `$HOME`, resolved once. `None` leaves the home-relative rules unmatched —
/// the filename rules still apply.
#[must_use]
pub fn home_dir() -> Option<&'static Path> {
    static HOME: OnceLock<Option<PathBuf>> = OnceLock::new();
    HOME.get_or_init(|| {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
    })
    .as_deref()
}

/// Expand a leading `~`, `$HOME` or `${HOME}` into `home`.
fn expand_home(raw: &str, home: Option<&Path>) -> String {
    let Some(home) = home else {
        return raw.to_string();
    };
    let home = home.to_string_lossy();
    let home = home.trim_end_matches('/');
    for prefix in ["${HOME}/", "$HOME/", "~/"] {
        if let Some(rest) = raw.strip_prefix(prefix) {
            return format!("{home}/{rest}");
        }
    }
    if raw == "~" || raw == "$HOME" || raw == "${HOME}" {
        return home.to_string();
    }
    raw.to_string()
}

/// The last path component, as a string.
fn file_name_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn exempt(name: &str) -> bool {
    compiled_exempt().iter().any(|m| m.is_match(name))
}

/// The path relative to `home`, when it is under `home`.
fn home_relative(expanded: &str, home: Option<&Path>) -> Option<String> {
    let home = home?;
    let home = home.to_string_lossy();
    expanded
        .strip_prefix(home.trim_end_matches('/'))
        .map(|rest| rest.trim_start_matches('/').to_string())
}

/// The general form: built-in registry plus caller-supplied extra patterns.
///
/// `extra` patterns are matched the way the registry is — home-relative when
/// they start with `~` or `/`, otherwise by filename — and always resolve to
/// [`Tier::Secret`]: a user-added pattern should ask, not hard-refuse, because
/// this module cannot know what they meant.
#[must_use]
pub fn classify_with(raw: &str, home: Option<&Path>, extra: &[String]) -> Option<Hit> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let name = file_name_of(trimmed);
    if exempt(name) {
        return None;
    }

    let expanded = expand_home(trimmed, home);
    let relative = home_relative(&expanded, home);

    for (idx, m) in compiled_rules() {
        let rule = &RULES[*idx];
        let subject = match rule.scope {
            Scope::Home => match &relative {
                Some(rel) => rel.as_str(),
                None => continue,
            },
            Scope::Name => name,
        };
        if m.is_match(subject) {
            return Some(Hit {
                tier: rule.tier,
                kind: rule.kind,
                matched: trimmed.to_string(),
                pattern: rule.glob.to_string(),
            });
        }
    }

    for pattern in extra {
        let Some(scope) = user_scope(pattern) else {
            continue;
        };
        let (subject, glob) = match scope {
            UserScope::Home(g) => match relative.as_deref() {
                Some(rel) => (rel, g),
                None => continue,
            },
            UserScope::Absolute(g) | UserScope::Path(g) => (expanded.as_str(), g),
            UserScope::Name(g) => (name, g),
        };
        if matcher(glob).is_some_and(|m| m.is_match(subject)) {
            return Some(Hit {
                tier: Tier::Secret,
                kind: K::KeyMaterial,
                matched: trimmed.to_string(),
                pattern: pattern.clone(),
            });
        }
    }

    None
}

/// Classify a path against the built-in registry only.
#[must_use]
pub fn classify_path_with_home(raw: &str, home: Option<&Path>) -> Option<Hit> {
    classify_with(raw, home, &[])
}

/// Classify a path against the built-in registry, resolving `$HOME`.
#[must_use]
pub fn classify_path(raw: &str) -> Option<Hit> {
    classify_path_with_home(raw, home_dir())
}

/// Classify a shell command string, built-in registry only.
#[must_use]
pub fn classify_command(command: &str) -> Option<Hit> {
    classify_command_with(command, home_dir(), &[])
}

/// Classify a shell command string: split it into candidate tokens and classify
/// each one.
///
/// Quotes are **removed** rather than treated as separators, so a spliced path
/// (`~/.ss"h"/id_rsa`) reconstructs into the token a shell would actually pass.
/// Metacharacters end a token, which is what picks up redirection targets
/// (`> .env`).
///
/// Documented limits — this does **not** follow command substitution, variable
/// indirection, or code inside `python -c` / `node -e`.
#[must_use]
pub fn classify_command_with(command: &str, home: Option<&Path>, extra: &[String]) -> Option<Hit> {
    let unquoted: String = command
        .chars()
        .filter(|c| *c != '\'' && *c != '"')
        .collect();
    for token in unquoted.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                ';' | '&' | '|' | '>' | '<' | '(' | ')' | '`' | '=' | ',' | '\n'
            )
    }) {
        // Leading `-` so `--file=~/.netrc`-style flags still classify.
        let token = token.trim_start_matches('-');
        if token.is_empty() {
            continue;
        }
        // A bare word can only name a secret through the exact list; anything
        // else without a `.` or `/` is prose, a flag value, or a grep pattern.
        if !token.contains('.') && !token.contains('/') && !BARE_SECRET_NAMES.contains(&token) {
            continue;
        }
        if let Some(hit) = classify_with(token, home, extra) {
            return Some(hit);
        }
    }
    None
}

/// The refusal text for a [`Tier::Credential`] hit.
///
/// Read by two audiences: the model (which must not retry, and must tell the
/// user rather than work around it) and the human (who needs the exact escape
/// hatch).
#[must_use]
pub fn refusal_text(hit: &Hit) -> String {
    format!(
        "Refused: {} is credential material ({}). Reading it is not something this agent does. \
         If the user asked for this, they can permit the path in .tact/settings.json under \
         permissions.sensitive_paths.allow (or $HOME/.tact/settings.json for a global allowance).",
        hit.matched,
        hit.kind.label(),
    )
}

/// Whether any pattern in `patterns` matches `raw`, using the same rules the
/// scanner's `allow` list uses: `~/` is home-relative, `/` is absolute, and
/// anything else matches the final path component.
///
/// Shared rather than duplicated because `redaction.basic_only_paths` and
/// `sensitive_paths.allow` are the same matching problem with different
/// consequences, and two implementations would drift.
#[must_use]
pub fn matches_any_with(patterns: &[String], raw: &str, home: Option<&Path>) -> bool {
    if patterns.is_empty() {
        return false;
    }
    let raw = raw.trim();
    let expanded = expand_home(raw, home);
    let name = file_name_of(raw);
    let relative = home_relative(&expanded, home);
    patterns.iter().any(|pattern| {
        let Some(scope) = user_scope(pattern) else {
            return false;
        };
        let (subject, glob) = match scope {
            UserScope::Home(g) => match relative.as_deref() {
                Some(rel) => (rel, g),
                None => return false,
            },
            UserScope::Absolute(g) | UserScope::Path(g) => (expanded.as_str(), g),
            UserScope::Name(g) => (name, g),
        };
        matcher(glob).is_some_and(|m| m.is_match(subject))
    })
}

/// [`matches_any_with`] against the process `$HOME`.
#[must_use]
pub fn matches_any(patterns: &[String], raw: &str) -> bool {
    matches_any_with(patterns, raw, home_dir())
}

// ---------------------------------------------------------------------------
// Scanner
// ---------------------------------------------------------------------------

/// The guard, with the user's configuration applied.
///
/// `allow` is checked first and exempts a path from the guard entirely —
/// including the [`Tier::Credential`] refusal, which is the point of requiring
/// a file edit instead of offering a button.
#[derive(Debug, Clone, Default)]
pub struct Scanner {
    enabled: bool,
    extra: Vec<String>,
    allow: Vec<String>,
    home: Option<PathBuf>,
}

impl Scanner {
    /// A scanner with the guard on, no user overrides, and `$HOME` resolved —
    /// the built-in behaviour.
    #[must_use]
    pub fn builtin() -> Self {
        Self {
            enabled: true,
            extra: Vec::new(),
            allow: Vec::new(),
            home: home_dir().map(Path::to_path_buf),
        }
    }

    /// A scanner with the guard off. For callers that have not loaded settings
    /// yet; the guard is on by default everywhere else.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::builtin()
        }
    }

    /// Build from the parsed `permissions.sensitive_paths` object.
    #[must_use]
    pub fn from_parts(enabled: bool, extra: Vec<String>, allow: Vec<String>) -> Self {
        Self {
            enabled,
            extra,
            allow,
            home: home_dir().map(Path::to_path_buf),
        }
    }

    /// Build from explicit parts with a pinned home — for tests.
    #[must_use]
    pub fn with_parts(home: Option<PathBuf>, extra: Vec<String>, allow: Vec<String>) -> Self {
        Self {
            enabled: true,
            extra,
            allow,
            home,
        }
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub fn extra(&self) -> &[String] {
        &self.extra
    }

    #[must_use]
    pub fn allow(&self) -> &[String] {
        &self.allow
    }

    #[must_use]
    pub fn home(&self) -> Option<&Path> {
        self.home.as_deref()
    }

    /// Whether an `allow` entry exempts this raw path or command token.
    fn is_allowed(&self, raw: &str) -> bool {
        matches_any_with(&self.allow, raw, self.home.as_deref())
    }

    /// Classify a path, honouring `enabled` and `allow`.
    #[must_use]
    pub fn classify(&self, raw: &str) -> Option<Hit> {
        if !self.enabled || self.is_allowed(raw) {
            return None;
        }
        classify_with(raw, self.home.as_deref(), &self.extra)
    }

    /// Classify a shell command, honouring `enabled` and `allow`.
    #[must_use]
    pub fn classify_command(&self, command: &str) -> Option<Hit> {
        if !self.enabled {
            return None;
        }
        classify_command_with(command, self.home.as_deref(), &self.extra)
            .filter(|hit| !self.is_allowed(&hit.matched))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> &'static Path {
        Path::new("/home/tester")
    }

    /// Turn a glob into a path that must match it.
    fn sample(glob: &str) -> String {
        glob.replace("**", "x").replace('*', "x")
    }

    /// A sample path for a rule, in the form the classifier will see.
    fn rule_sample(r: &Rule) -> String {
        let s = sample(r.glob);
        match r.scope {
            Scope::Home => format!("/home/tester/{s}"),
            Scope::Name => s,
        }
    }

    /// Every registry row must classify as its own tier and kind. Driven by the
    /// table itself, so adding a row without a case is impossible.
    #[test]
    fn registry_table() {
        for r in RULES {
            let path = rule_sample(r);
            let hit = classify_path_with_home(&path, Some(home()))
                .unwrap_or_else(|| panic!("no hit for {path} (from {})", r.glob));
            assert_eq!(hit.pattern, r.glob, "{path} matched the wrong rule");
            assert_eq!(hit.tier, r.tier, "{path}");
            assert_eq!(hit.kind, r.kind, "{path}");
        }
    }

    #[test]
    fn exclusions_stay_excluded() {
        for path in [
            "/home/tester/.ssh/id_rsa.pub",
            "/home/tester/.ssh/id_ed25519.pub",
            "certs/fullchain.crt",
            "certs/fullchain.cer",
            "key.der",
            ".env.example",
            ".env.sample",
            "config.template",
            "thing.dist",
        ] {
            assert!(
                classify_path_with_home(path, Some(home())).is_none(),
                "{path} should be exempt"
            );
        }
    }

    #[test]
    fn home_relative_only_at_home() {
        assert!(classify_path_with_home("/home/tester/.netrc", Some(home())).is_some());
        // `.netrc` is a credential store *anywhere* — the home rule is not what
        // makes it sensitive, the filename is.
        let elsewhere = classify_path_with_home("/home/other/.netrc", Some(home())).unwrap();
        assert_eq!(elsewhere.tier, Tier::Credential);
        assert_eq!(
            elsewhere.pattern, ".netrc",
            "should match the filename rule"
        );
        // A directory that merely shares the name is not.
        assert!(classify_path_with_home("/tmp/netrc", Some(home())).is_none());
        // Home-only rules really are home-only.
        assert!(classify_path_with_home("/home/other/.ssh/id_rsa", Some(home())).is_some());
        assert!(classify_path_with_home("/home/other/.aws/credentials", Some(home())).is_none());
    }

    #[test]
    fn tilde_expands_before_matching() {
        let hit = classify_path_with_home("~/.ssh/id_ed25519", Some(home())).unwrap();
        assert_eq!(hit.tier, Tier::Credential);
        assert_eq!(hit.kind, SensitiveKind::PrivateKey);

        let hit = classify_path_with_home("$HOME/.netrc", Some(home())).unwrap();
        assert_eq!(hit.kind, SensitiveKind::CredentialStore);

        let hit = classify_path_with_home("${HOME}/.aws/credentials", Some(home())).unwrap();
        assert_eq!(hit.tier, Tier::Credential);
    }

    #[test]
    fn ssh_config_is_secret_but_the_key_is_credential() {
        let hit = classify_path_with_home("/home/tester/.ssh/config", Some(home())).unwrap();
        assert_eq!(hit.tier, Tier::Secret);
        assert_eq!(hit.kind, SensitiveKind::CredentialStore);

        let hit = classify_path_with_home("/home/tester/.ssh/id_ed25519", Some(home())).unwrap();
        assert_eq!(hit.tier, Tier::Credential);
        // The narrow exception must not swallow the directory rule.
        assert_eq!(hit.pattern, ".ssh/**");
    }

    #[test]
    fn claude_settings_is_credential_but_memory_is_not() {
        assert_eq!(
            classify_path_with_home("/home/tester/.claude/settings.json", Some(home()))
                .unwrap()
                .tier,
            Tier::Credential
        );
        assert!(
            classify_path_with_home("/home/tester/.claude/CLAUDE.md", Some(home())).is_none(),
            "agent memories must stay readable"
        );
    }

    #[test]
    fn in_repo_env_file_is_secret() {
        let hit = classify_path_with_home(".env", Some(home())).unwrap();
        assert_eq!(hit.tier, Tier::Secret);
        assert_eq!(hit.kind, SensitiveKind::EnvFile);

        assert!(classify_path_with_home("tests/fixtures/.env.local", Some(home())).is_some());
        assert!(classify_path_with_home(".env.example", Some(home())).is_none());
    }

    #[test]
    fn in_repo_credential_stores_are_credential() {
        for name in [
            ".npmrc",
            ".pypirc",
            "secrets.json",
            "service-account-prod.json",
        ] {
            let hit = classify_path_with_home(name, Some(home())).unwrap();
            assert_eq!(hit.tier, Tier::Credential, "{name}");
        }
    }

    #[test]
    fn ordinary_source_is_not_sensitive() {
        for path in [
            "src/main.rs",
            "crates/tact/src/security/sensitive.rs",
            "book/10_chapter_permission_zh.md",
            "keyboard.rs",
            "notes.txt",
        ] {
            assert!(
                classify_path_with_home(path, Some(home())).is_none(),
                "{path} must not be gated"
            );
        }
    }

    #[test]
    fn classify_command_catches_plain_reads() {
        for command in [
            "cat ~/.ssh/id_ed25519",
            "cat $HOME/.netrc",
            "cat /home/tester/.netrc",
            "head -5 ~/.aws/credentials",
            "grep -r token ~/.aws",
            "echo x > .env",
            "cat '/home/tester/.netrc'",
            "base64 ~/.ssh/id_rsa",
            "cp ~/.netrc /tmp/loot",
            "cat id_rsa",
            "cat secrets.json",
        ] {
            assert!(
                classify_command_with(command, Some(home()), &[]).is_some(),
                "should be caught: {command}"
            );
        }
    }

    #[test]
    fn classify_command_catches_quote_spliced_paths() {
        let hit = classify_command_with("cat ~/.ss\"h\"/id_rsa", Some(home()), &[]).unwrap();
        assert_eq!(hit.tier, Tier::Credential);
    }

    #[test]
    fn classify_command_allows_benign_commands() {
        for command in [
            "ls -la",
            "cargo test -p tact --lib",
            "cat src/main.rs",
            "grep -rn token crates/",
            "git status",
            "cat README.md",
            // A grep pattern that merely looks like a key filename must not
            // cost a refusal.
            "rg \"foo_rsa\" crates/",
            "cargo build --release",
        ] {
            assert!(
                classify_command_with(command, Some(home()), &[]).is_none(),
                "false positive on: {command}"
            );
        }
    }

    #[test]
    fn scanner_honours_allow() {
        let scanner = Scanner::with_parts(
            Some(home().to_path_buf()),
            vec![],
            vec!["~/.ssh/config".to_string()],
        );
        assert!(scanner.classify("/home/tester/.ssh/config").is_none());
        // Everything else stays gated.
        assert!(scanner.classify("/home/tester/.ssh/id_ed25519").is_some());
    }

    #[test]
    fn scanner_allow_lifts_a_credential_hit() {
        let scanner = Scanner::with_parts(
            Some(home().to_path_buf()),
            vec![],
            vec!["~/.netrc".to_string()],
        );
        assert!(
            scanner.classify("/home/tester/.netrc").is_none(),
            "the allow list is the documented escape from a Credential refusal"
        );
        assert!(
            scanner
                .classify_command("cat /home/tester/.netrc")
                .is_none()
        );
    }

    #[test]
    fn scanner_honours_extra_patterns() {
        let scanner = Scanner::with_parts(
            Some(home().to_path_buf()),
            vec!["*.vault".to_string()],
            vec![],
        );
        let hit = scanner.classify("config.vault").unwrap();
        assert_eq!(hit.tier, Tier::Secret);
        assert_eq!(hit.pattern, "*.vault");
    }

    /// A separator in a pattern means it is a path glob: it can never match a
    /// bare filename, so matching it against one silently does nothing.
    #[test]
    fn a_slash_in_a_pattern_makes_it_a_path_glob() {
        let scanner = Scanner::with_parts(
            Some(home().to_path_buf()),
            vec![],
            vec!["**/fixtures/**".to_string()],
        );
        assert!(
            scanner.classify("tests/fixtures/.env").is_none(),
            "a nested path must be matched, not reduced to its filename"
        );
        assert!(
            scanner.classify(".env").is_some(),
            "outside fixtures it holds"
        );
    }

    #[test]
    fn scanner_disabled_gates_nothing() {
        let scanner = Scanner::disabled();
        assert!(scanner.classify("/home/tester/.ssh/id_ed25519").is_none());
        assert!(scanner.classify_command("cat ~/.netrc").is_none());
    }

    #[test]
    fn refusal_text_names_the_path_and_the_escape() {
        let hit = classify_path_with_home("~/.ssh/id_ed25519", Some(home())).unwrap();
        let text = refusal_text(&hit);
        assert!(text.contains("~/.ssh/id_ed25519"), "{text}");
        assert!(text.contains("private-key"), "{text}");
        assert!(text.contains("sensitive_paths.allow"), "{text}");
    }
}

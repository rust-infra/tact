//! Pattern matching shared by the sensitive-path guard and the redactor.
//!
//! Two Kernel/extension rules compare a user-supplied pattern against a path:
//! the sensitive-path registry decides whether a read is refused, and the
//! redaction policy decides whether a path keeps the structural rules. They
//! must agree on what a pattern means, so the meaning lives here once.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use globset::{Glob, GlobMatcher};

pub fn matcher(pattern: &str) -> Option<GlobMatcher> {
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
pub enum UserScope<'a> {
    Home(&'a str),
    Absolute(&'a str),
    Path(&'a str),
    Name(&'a str),
}

pub fn user_scope(pattern: &str) -> Option<UserScope<'_>> {
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
pub fn expand_home(raw: &str, home: Option<&Path>) -> String {
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
pub fn file_name_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The path relative to `home`, when it is under `home`.
pub fn home_relative(expanded: &str, home: Option<&Path>) -> Option<String> {
    let home = home?;
    let home = home.to_string_lossy();
    expanded
        .strip_prefix(home.trim_end_matches('/'))
        .map(|rest| rest.trim_start_matches('/').to_string())
}

/// Does any pattern match `raw`, resolving against an explicit `home`?
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

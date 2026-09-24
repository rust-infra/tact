//! Composer-only product helpers.
//!
//! GPUI owns the text editing mechanics, while this module owns the small
//! pieces of policy the composer needs: attachment references, `@file` /
//! `/skill` trigger parsing, file suggestions, and the skill catalogue. Keeping
//! these pure helpers out of the window shell makes them cheap to test and
//! prevents render code from walking the workspace synchronously more than the
//! bounded suggestion limit allows.

use std::path::{Path, PathBuf};

/// One attachment chip shown above the composer input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Attachment {
    pub(crate) label: String,
    pub(crate) path: PathBuf,
}

/// The kind of completion a composer trigger opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SuggestionKind {
    File,
    Skill,
}

/// One selectable completion candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Suggestion {
    pub(crate) kind: SuggestionKind,
    pub(crate) label: String,
    pub(crate) insertion: String,
}

/// The active `@` or `/` token in the draft, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Trigger {
    pub(crate) kind: SuggestionKind,
    pub(crate) query: String,
    pub(crate) start: usize,
}

/// Parse the trailing trigger token from a composer draft.
///
/// A trigger only starts at the beginning of the input or after whitespace, so
/// an email address or a path with an embedded slash does not unexpectedly open
/// completion. The returned byte range is safe to splice with
/// [`apply_suggestion`] because the token itself is UTF-8.
pub(crate) fn parse_trigger(draft: &str) -> Option<Trigger> {
    let token_start = draft
        .char_indices()
        .rev()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(index + ch.len_utf8()))
        .unwrap_or(0);
    let token = &draft[token_start..];
    let (kind, query) = match token.as_bytes().first().copied() {
        Some(b'@') => (SuggestionKind::File, &token[1..]),
        Some(b'/') => (SuggestionKind::Skill, &token[1..]),
        _ => return None,
    };
    if query.contains('/') {
        return None;
    }
    Some(Trigger {
        kind,
        query: query.to_string(),
        start: token_start,
    })
}

/// Replace the active trigger token with a selected suggestion.
pub(crate) fn apply_suggestion(draft: &str, trigger: &Trigger, insertion: &str) -> String {
    let mut next = String::with_capacity(draft.len() + insertion.len());
    next.push_str(&draft[..trigger.start]);
    next.push_str(insertion);
    next.push(' ');
    next
}

/// Suggestions for the active trigger, bounded for a responsive popover.
pub(crate) fn suggestions(draft: &str, files: &[PathBuf]) -> Vec<Suggestion> {
    let Some(trigger) = parse_trigger(draft) else {
        return Vec::new();
    };
    match trigger.kind {
        SuggestionKind::File => rank_files(files, &trigger.query),
        SuggestionKind::Skill => skill_suggestions(&trigger.query),
    }
}

/// How many rows the completion popover shows.
const SUGGESTION_LIMIT: usize = 8;

/// Every file below `root` a mention can name, relative to `root`.
///
/// The shell builds this once per workspace and keeps it on the session. A walk
/// is cheap as a one-off and ruinous per frame — the composer asks for
/// suggestions every time it draws — and the ceiling this used to carry
/// (512 files, first depth-first) silently dropped the file the reader was
/// after. This repository holds 517 files outside the trees the walk skips, so
/// the list was both incomplete and arbitrary.
pub(crate) fn file_index(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    collect_files(root, root, 0, &mut paths);
    paths.sort();
    paths
}

/// Up to [`SUGGESTION_LIMIT`] files matching `query`, best match first.
///
/// Ranked by where the query lands in the path, then by path. A flat `contains`
/// treated `src/alpha.rs` and `src/notes/alpha.rs` as equals, which is how a
/// list of eight fills with the wrong eight.
pub(crate) fn rank_files(index: &[PathBuf], query: &str) -> Vec<Suggestion> {
    let query = query.to_ascii_lowercase();
    let mut ranked: Vec<(u8, &PathBuf)> = index
        .iter()
        .filter_map(|path| rank(path, &query).map(|rank| (rank, path)))
        .collect();
    ranked.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(right.1)));
    ranked
        .into_iter()
        .take(SUGGESTION_LIMIT)
        .map(|(_, path)| Suggestion {
            kind: SuggestionKind::File,
            label: path.to_string_lossy().to_string(),
            insertion: format!("@{}", path.to_string_lossy()),
        })
        .collect()
}

/// How well `path` matches `query`; lower is better, `None` is no match.
///
/// 0 — the file name starts with the query (what a reader typing a name means).
/// 1 — the file name contains it.
/// 2 — only the directory part does.
fn rank(path: &Path, query: &str) -> Option<u8> {
    if query.is_empty() {
        return Some(3);
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if name.starts_with(query) {
        return Some(0);
    }
    if name.contains(query) {
        return Some(1);
    }
    path.to_string_lossy()
        .to_ascii_lowercase()
        .contains(query)
        .then_some(2)
}

fn collect_files(root: &Path, path: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    /// Deep enough for a workspace nested a few crates down, shallow enough
    /// that a symlink loop cannot run away.
    const MAX_DEPTH: usize = 8;
    /// A guard against a pathological tree, not a display budget: the popover
    /// shows eight, and everything else is what the ranking picks from.
    const MAX_INDEX_FILES: usize = 20_000;
    if depth > MAX_DEPTH || out.len() > MAX_INDEX_FILES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let child = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if let Some(dir) = kind.is_dir().then_some(name.as_str())
            && crate::pane::FILE_TREE_SKIPPED_DIRS.contains(&dir)
        {
            // Build output and vendored dependencies are gitignored artifact
            // trees: they are thousands of directories deep, and a mention is
            // asked for a file the agent works on, not a cached object.
            continue;
        }
        if kind.is_dir() {
            collect_files(root, &child, depth + 1, out);
        } else if kind.is_file()
            && let Ok(relative) = child.strip_prefix(root)
        {
            out.push(relative.to_path_buf());
        }
    }
}

/// Skill names from the installed Codex/agents skill roots.
///
/// The fallback keeps completion useful in a freshly installed app where the
/// process has not yet been given a user skill directory. It is deliberately
/// small and only contains names that are safe to show as slash-command hints;
/// the real catalogue is discovered from disk when available.
pub(crate) fn skill_suggestions(query: &str) -> Vec<Suggestion> {
    let query = query.to_ascii_lowercase();
    skill_names()
        .into_iter()
        .filter(|name| query.is_empty() || name.to_ascii_lowercase().contains(&query))
        .take(8)
        .map(|name| Suggestion {
            kind: SuggestionKind::Skill,
            label: format!("/{name}"),
            insertion: format!("/{name}"),
        })
        .collect()
}

fn skill_names() -> Vec<String> {
    let mut roots = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        roots.push(PathBuf::from(&home).join(".agents/skills"));
        roots.push(PathBuf::from(&home).join(".codex/skills"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd.join(".agents/skills"));
        roots.push(cwd.join(".codex/skills"));
    }

    let mut names: Vec<String> = roots
        .iter()
        .flat_map(|root| skill_names_from_root(root))
        .collect();
    if names.is_empty() {
        names.extend(
            [
                "frontend-design",
                "ui-ux-pro-max",
                "gpui-kit",
                "gpui-kit-design-guides",
                "emil-design-eng",
            ]
            .into_iter()
            .map(str::to_string),
        );
    }
    names.sort();
    names.dedup();
    names
}

fn skill_names_from_root(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.path().join("SKILL.md").is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect()
}

/// Compose a message body that carries selected attachment paths in a stable,
/// human-readable form. The protocol currently submits a string, so this keeps
/// attachments explicit rather than silently dropping them at the UI boundary.
pub(crate) fn with_attachments(prompt: &str, attachments: &[Attachment]) -> String {
    if attachments.is_empty() {
        return prompt.to_string();
    }
    let mut body = String::from(prompt);
    body.push_str("\n\nAttached context:\n");
    for attachment in attachments {
        body.push_str("- ");
        body.push_str(&attachment.path.display().to_string());
        body.push('\n');
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_requires_a_token_boundary() {
        assert!(parse_trigger("hello @src").is_some());
        assert!(parse_trigger("@src").is_some());
        assert!(parse_trigger("mail foo@example.com").is_none());
        assert!(parse_trigger("path src/lib").is_none());
    }

    #[test]
    fn suggestion_replaces_only_the_active_token() {
        let draft = "read @sr";
        let trigger = parse_trigger(draft).expect("trigger");
        assert_eq!(
            apply_suggestion(draft, &trigger, "@src/lib.rs"),
            "read @src/lib.rs "
        );
    }

    #[test]
    fn attachments_are_explicit_in_the_submitted_body() {
        let attachments = vec![Attachment {
            label: "src/lib.rs".into(),
            path: PathBuf::from("src/lib.rs"),
        }];
        assert_eq!(
            with_attachments("review this", &attachments),
            "review this\n\nAttached context:\n- src/lib.rs\n"
        );
    }

    #[test]
    fn skill_names_are_discovered_from_a_skill_root() {
        let root = std::env::temp_dir().join(format!(
            "tact-gui-skills-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("my-skill")).unwrap();
        std::fs::write(root.join("my-skill/SKILL.md"), "# my skill\n").unwrap();
        assert_eq!(skill_names_from_root(&root), ["my-skill"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_name_that_starts_with_the_query_outranks_one_that_merely_contains_it() {
        let index = vec![
            PathBuf::from("src/notes/alpha.rs"),
            PathBuf::from("src/admission.rs"),
            PathBuf::from("src/alpha.rs"),
        ];

        let labels: Vec<_> = rank_files(&index, "alph")
            .into_iter()
            .map(|suggestion| suggestion.label)
            .collect();

        assert_eq!(
            labels,
            ["src/alpha.rs", "src/notes/alpha.rs"],
            "the file named for the query comes first; `admission.rs` is not a match"
        );
    }

    #[test]
    fn the_index_is_not_capped_below_a_working_repository() {
        // The old ceiling was 512 files, which this repository already exceeds
        // outside the skipped trees — the file a reader wanted could simply be
        // missing from every list.
        let root = std::env::temp_dir().join(format!("tact-gui-index-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for index in 0..600 {
            std::fs::write(root.join(format!("file-{index:03}.rs")), "").unwrap();
        }

        let index = file_index(&root);
        assert_eq!(index.len(), 600);
        assert!(
            rank_files(&index, "file-599")
                .iter()
                .any(|suggestion| suggestion.label.ends_with("file-599.rs")),
            "the last file is reachable"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn file_suggestions_skip_build_output_directories() {
        // The mention walk is a `read_dir` over the workdir on every keystroke
        // that carries a trigger. `target/` alone holds thousands of entries in
        // this repository (19ms of the walk against the workspace root, against
        // 0.7ms without it), and a mention is not how a build artifact is
        // reached.
        let root = std::env::temp_dir().join(format!("tact-gui-mentions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn lib() {}\n").unwrap();
        std::fs::write(root.join("target/debug/out"), "artifact\n").unwrap();
        std::fs::write(root.join("node_modules/pkg/index.js"), "module\n").unwrap();

        let labels: Vec<_> = rank_files(&file_index(&root), "")
            .into_iter()
            .map(|suggestion| suggestion.label)
            .collect();

        assert_eq!(labels, ["src/lib.rs"]);

        let _ = std::fs::remove_dir_all(root);
    }
}

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
    /// A directory to step into rather than a file to name. Taking it extends
    /// the query instead of ending it, which is how the tree is walked.
    pub(crate) is_dir: bool,
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

/// A workspace's files and directories, relative to its root.
///
/// Built once per workspace and kept on the session. The composer reads it on
/// every frame it draws, so it cannot be assembled from the filesystem there —
/// and a flat file list cannot answer "what is in `src/`", which is how a
/// reader walks a tree they do not have memorised.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct FileIndex {
    /// Every file below the root, sorted.
    pub(crate) files: Vec<PathBuf>,
    /// Every directory below the root, sorted. The root itself is not listed.
    pub(crate) dirs: Vec<PathBuf>,
}

impl FileIndex {
    /// Walk `root` once and keep everything a mention can name.
    pub(crate) fn build(root: &Path) -> Self {
        let mut index = Self::default();
        collect_files(root, root, 0, &mut index);
        index.files.sort();
        index.dirs.sort();
        index
    }
}

/// Suggestions for the active trigger, bounded for a responsive popover.
pub(crate) fn suggestions(draft: &str, index: &FileIndex) -> Vec<Suggestion> {
    let Some(trigger) = parse_trigger(draft) else {
        return Vec::new();
    };
    match trigger.kind {
        SuggestionKind::File => rank_entries(index, &trigger.query),
        SuggestionKind::Skill => skill_suggestions(&trigger.query),
    }
}

/// How many rows the completion popover shows.
const SUGGESTION_LIMIT: usize = 8;

/// Up to [`SUGGESTION_LIMIT`] entries below the directory the query names.
///
/// The query is a path, not a search string: `@src/ta` lists what `src/` holds
/// that matches `ta`, and taking a directory writes it back as `@src/` — so a
/// reader walks into the tree the way the terminal's picker lets them, without
/// the composer needing a mode of its own. Directories are listed before files,
/// since a directory is what the reader is usually aiming at mid-path.
pub(crate) fn rank_entries(index: &FileIndex, query: &str) -> Vec<Suggestion> {
    let (dir, prefix) = split_query(query);
    let prefix = prefix.to_ascii_lowercase();

    let mut ranked: Vec<(u8, u8, &PathBuf, bool)> = Vec::new();
    for (candidates, is_dir) in [(&index.dirs, true), (&index.files, false)] {
        for candidate in candidates {
            if candidate.parent().unwrap_or(Path::new("")) != dir {
                continue;
            }
            let Some(name) = candidate.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(rank) = rank(name, &prefix) else {
                continue;
            };
            ranked.push((rank, u8::from(!is_dir), candidate, is_dir));
        }
    }
    ranked.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.cmp(right.2))
    });

    ranked
        .into_iter()
        .take(SUGGESTION_LIMIT)
        .map(|(_, _, path, is_dir)| Suggestion {
            kind: SuggestionKind::File,
            label: path.to_string_lossy().to_string(),
            insertion: if is_dir {
                format!("@{}/", path.display())
            } else {
                mention(path)
            },
            is_dir,
        })
        .collect()
}

/// Split a mention query into the directory it names and the prefix inside it.
///
/// `"src/ta"` → `("src", "ta")`, `"src/"` → `("src", "")`, `"ta"` → `("", "ta")`.
fn split_query(query: &str) -> (&Path, &str) {
    match query.rfind('/') {
        Some(index) => (Path::new(&query[..index]), &query[index + 1..]),
        None => (Path::new(""), query),
    }
}

/// How well one name matches; lower is better, `None` is no match.
///
/// 0 — the name starts with the query (what a reader typing a name means).
/// 1 — it contains the query somewhere.
fn rank(name: &str, query: &str) -> Option<u8> {
    if query.is_empty() {
        return Some(1);
    }
    let name = name.to_ascii_lowercase();
    if name.starts_with(query) {
        Some(0)
    } else if name.contains(query) {
        Some(1)
    } else {
        None
    }
}

/// What a chosen file inserts: `@src/lib.rs`, or `@"my notes.md"` when the
/// path holds a space.
///
/// The terminal client's rule, kept identical so a draft from either front end
/// reads the same way.
fn mention(path: &Path) -> String {
    let text = path.to_string_lossy();
    if text.chars().any(char::is_whitespace) {
        format!("@\"{text}\"")
    } else {
        format!("@{text}")
    }
}

fn collect_files(root: &Path, path: &Path, depth: usize, index: &mut FileIndex) {
    /// Deep enough for a workspace nested a few crates down, shallow enough
    /// that a symlink loop cannot run away.
    const MAX_DEPTH: usize = 8;
    /// A guard against a pathological tree, not a display budget: the popover
    /// shows eight, and everything else is what the ranking picks from.
    const MAX_INDEX_FILES: usize = 20_000;
    if depth > MAX_DEPTH || index.files.len() > MAX_INDEX_FILES {
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
            if let Ok(relative) = child.strip_prefix(root) {
                index.dirs.push(relative.to_path_buf());
            }
            collect_files(root, &child, depth + 1, index);
        } else if kind.is_file()
            && let Ok(relative) = child.strip_prefix(root)
        {
            index.files.push(relative.to_path_buf());
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
            is_dir: false,
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

    /// Two files and a directory in the workspace root.
    fn root_index() -> FileIndex {
        FileIndex {
            files: vec![PathBuf::from("my notes.md"), PathBuf::from("lib.rs")],
            dirs: vec![PathBuf::from("docs")],
        }
    }

    #[test]
    fn a_path_with_a_space_is_quoted_the_way_the_terminal_quotes_it() {
        let insertions: Vec<_> = rank_entries(&root_index(), "")
            .into_iter()
            .map(|suggestion| suggestion.insertion)
            .collect();

        // The directory leads, then the files by name.
        assert_eq!(insertions, ["@docs/", "@lib.rs", "@\"my notes.md\""]);
    }

    #[test]
    fn a_name_that_starts_with_the_query_outranks_one_that_merely_contains_it() {
        let index = FileIndex {
            files: vec![PathBuf::from("alpha.rs"), PathBuf::from("admission.rs")],
            dirs: Vec::new(),
        };

        let labels: Vec<_> = rank_entries(&index, "alph")
            .into_iter()
            .map(|suggestion| suggestion.label)
            .collect();

        assert_eq!(
            labels,
            ["alpha.rs"],
            "a prefix beats a hit in the middle, and `admission.rs` is neither"
        );

        let labels: Vec<_> = rank_entries(&index, "mission")
            .into_iter()
            .map(|suggestion| suggestion.label)
            .collect();
        assert_eq!(labels, ["admission.rs"]);
    }

    /// Walking a tree is a longer query, not a mode: the root lists its own
    /// children, and taking a directory writes `@src/` back into the draft.
    #[test]
    fn a_directory_is_stepped_into_by_extending_the_query() {
        let root = std::env::temp_dir().join(format!("tact-gui-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/gui")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "").unwrap();
        std::fs::write(root.join("src/gui/main.rs"), "").unwrap();
        std::fs::write(root.join("README.md"), "").unwrap();

        let index = FileIndex::build(&root);
        let entries = |query: &str| -> Vec<(String, bool)> {
            rank_entries(&index, query)
                .into_iter()
                .map(|suggestion| (suggestion.label, suggestion.is_dir))
                .collect()
        };

        assert_eq!(
            entries(""),
            [("src".to_string(), true), ("README.md".to_string(), false)],
            "the root lists what it holds, directories first"
        );
        assert_eq!(
            entries("src/"),
            [
                ("src/gui".to_string(), true),
                ("src/lib.rs".to_string(), false)
            ],
            "stepping in is just a longer query"
        );
        assert_eq!(
            entries("src/gui/"),
            [("src/gui/main.rs".to_string(), false)],
            "only that directory's own children are listed"
        );

        let directory = rank_entries(&index, "")
            .into_iter()
            .next()
            .expect("a directory");
        assert_eq!(
            directory.insertion, "@src/",
            "taking it writes the query that opens it"
        );

        let _ = std::fs::remove_dir_all(root);
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

        let index = FileIndex::build(&root);
        assert_eq!(index.files.len(), 600);
        assert!(
            rank_entries(&index, "file-599")
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

        let labels: Vec<_> = rank_entries(&FileIndex::build(&root), "src/")
            .into_iter()
            .map(|suggestion| suggestion.label)
            .collect();

        assert_eq!(labels, ["src/lib.rs"]);

        let _ = std::fs::remove_dir_all(root);
    }
}

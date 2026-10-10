//! Persistent memory (user preferences, project facts, feedback).
//!
//! Memories are Markdown files with YAML frontmatter declaring `name`,
//! `description`, `type` and `modified`, stored one directory **per git
//! repository** under `~/.tact/projects/<slug>/memory/` — the analogue of
//! Claude Code's `~/.claude/projects/<project>/memory/`.
//!
//! ## One directory per repository
//!
//! The slug comes from the repository's *common* git dir, not from the working
//! directory ([`memory_root`]). `git rev-parse --git-common-dir` returns `.git`
//! in the main checkout and the same absolute path from a linked worktree, so a
//! worktree and its main checkout share one memory directory. A project fact
//! therefore stays inside the repository that produced it instead of being
//! injected into every unrelated project.
//!
//! ## Index in the prompt, topics on demand
//!
//! Only [`MEMORY_INDEX_FILE`] is injected, capped at [`MAX_INDEX_LINES`] lines
//! or [`MAX_INDEX_BYTES`] bytes — whichever comes first, the same two limits
//! Claude Code uses. Topic bodies stay on disk and the model fetches the one it
//! needs with the `load_memory` tool. This is why the file tools' workspace
//! confinement ([`crate::tool::safe_path`]) does not leave memories
//! unreachable: the tool reads `$HOME` on the model's behalf.
//!
//! ## Memory types
//!
//! - `user`: user preferences ("I like tabs").
//! - `feedback`: corrections from the user.
//! - `project`: hard-won project facts (compliance, legacy constraints).
//! - `reference`: external resource URLs.
//!
//! ## [`MemoryManager`]
//!
//! Loads, saves, and queries memories.  The `load_memory_index_prompt` method
//! produces the injectable index block for the system prompt, and
//! [`MemoryManager::load_topic`] returns one memory's full text.
//!
//! ## Guidance
//!
//! [`MEMORY_GUIDANCE`] is a static string injected into the system prompt
//! to teach the LLM when memory should be saved vs. ignored.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::Deserialize;
use strum::VariantArray;
use strum_macros::{AsRefStr, Display, EnumString};
use walkdir::WalkDir;

pub const MEMORY_INDEX_FILE: &str = "MEMORY.md";
pub const MAX_INDEX_LINES: usize = 200;
/// Byte cap on the injected index, mirroring Claude Code's 25 KB limit.
///
/// The line cap alone does not bound the prompt: one index line carries a
/// description, and descriptions are free-form.
pub const MAX_INDEX_BYTES: usize = 25 * 1024;
pub const MEMORY_GUIDANCE: &str = r#"
When to save memories:
- User states a preference ("I like tabs", "always use pytest") -> type: user
- User corrects you ("don't do X", "that was wrong because...") -> type: feedback
- You learn a project fact that is not easy to infer from current code alone
  (for example: a rule exists because of compliance, or a legacy module must
  stay untouched for business reasons) -> type: project
- You learn where an external resource lives (ticket board, dashboard, docs URL)
  -> type: reference

When NOT to save:
- Anything easily derivable from code (function signatures, file structure, directory layout)
- Temporary task state (current branch, open PR numbers, current TODOs)
- Secrets or credentials (API keys, passwords)
"#;

/// Resolve the memory directory for `workdir`.
///
/// `$HOME/.tact/projects/<slug>/memory` when `workdir` is inside a git
/// repository, otherwise `<workdir>/.tact/memory`. `$HOME` unset falls back to
/// the project-local path, which is what keeps this usable in tests and in
/// containers without a home directory.
///
/// `[agent].auto_memory_directory`, when set, overrides the derived path —
/// the analogue of Claude Code's `autoMemoryDirectory`. It is read from the
/// installed process config, so a session-scoped override does not require
/// threading a parameter through every caller.
///
/// Pure otherwise: only `$HOME`, the parent walk, and the `.git`
/// file/directory are read. No process is spawned, so this is safe on the
/// session-start path.
#[must_use]
pub fn memory_root(workdir: &Path) -> PathBuf {
    memory_root_with(
        workdir,
        home_dir().as_deref(),
        configured_memory_dir().as_deref(),
    )
}

/// [`memory_root`] with the home directory and the configured override passed
/// in, so the whole decision is unit-testable without touching `$HOME` or the
/// process-global config.
#[must_use]
pub fn memory_root_with(
    workdir: &Path,
    home: Option<&Path>,
    override_dir: Option<&str>,
) -> PathBuf {
    if let Some(raw) = override_dir {
        return expand_memory_dir(raw, workdir);
    }

    match home {
        Some(home) => memory_root_for(workdir, home),
        None => workdir.join(".tact").join("memory"),
    }
}

/// The raw `[agent].auto_memory_directory` value, when configured.
fn configured_memory_dir() -> Option<String> {
    override_from_settings(crate::config::try_settings().as_ref())
}

/// Pull the override out of a resolved config.
///
/// Takes the config by reference rather than reading the process-global one, so
/// the extraction is testable without mutating settings that other tests in the
/// same process are concurrently reading.
fn override_from_settings(config: Option<&crate::config::ResolvedConfig>) -> Option<String> {
    config?.agent.auto_memory_directory.clone()
}

/// Expand a configured memory path: `~`/`~/…` against `$HOME`, a relative path
/// against `workdir`, an absolute path as-is.
///
/// The same three rules `[agent].skill_dirs` follows, so the two path-valued
/// settings cannot disagree about what `~/x` means.
#[must_use]
pub fn expand_memory_dir(raw: &str, workdir: &Path) -> PathBuf {
    let raw = raw.trim();

    if raw == "~" || raw.starts_with("~/") {
        let Some(home) = home_dir() else {
            // Without a home directory the tilde cannot be resolved, and
            // inventing one would silently write somewhere unexpected.
            return PathBuf::from(raw);
        };
        return if raw == "~" {
            home
        } else {
            home.join(&raw[2..])
        };
    }

    let path = PathBuf::from(raw);
    if path.is_absolute() {
        path
    } else {
        workdir.join(path)
    }
}

/// [`memory_root`] against an explicit home directory.
///
/// Split out so the derivation is unit-testable without touching `$HOME`, the
/// same seam `TactPath::home_memory_dir_for` uses.
#[must_use]
pub fn memory_root_for(workdir: &Path, home: &Path) -> PathBuf {
    match repo_slug(workdir) {
        Some(slug) => crate::consts::TactPath::home_projects_dir_for(home)
            .join(slug)
            .join("memory"),
        None => workdir.join(".tact").join("memory"),
    }
}

/// The repository's common git dir, or `None` when `workdir` is not in a repo.
///
/// Reads `.git` directly rather than shelling out to `git rev-parse`:
///
/// - a **directory** in a main checkout → that is the common dir already;
/// - a **file** in a linked worktree or submodule → `gitdir: <path>` names that
///   checkout's own git dir, and the repository's common dir is recorded
///   *inside* it in a `commondir` file. A worktree's `.git` points at
///   `<main>/.git/worktrees/<name>`, whose `commondir` holds `../..`.
///
/// Skipping the `commondir` step is not a cosmetic error: it slugs the
/// worktree as `<main>/.git/worktrees` and gives it a private memory directory,
/// which is exactly the split this module exists to prevent.
fn git_common_dir(workdir: &Path) -> Option<PathBuf> {
    let normalized = lexical_normalize(workdir);
    let mut dir = Some(normalized.as_path());
    while let Some(current) = dir {
        let dot_git = current.join(".git");
        if dot_git.is_dir() {
            return Some(lexical_normalize(&dot_git));
        }
        if dot_git.is_file()
            && let Ok(contents) = std::fs::read_to_string(&dot_git)
            && let Some(rest) = contents.trim().strip_prefix("gitdir:")
        {
            let checkout_git_dir = lexical_normalize(&current.join(rest.trim()));
            return Some(common_dir_of(&checkout_git_dir));
        }
        dir = current.parent();
    }
    None
}

/// Follow a git dir's `commondir` file to the repository's shared git dir.
///
/// The file is written by git for worktrees and submodules; its content is
/// relative to the git dir holding it. Absent the file, the git dir *is* the
/// common dir (an ordinary clone).
fn common_dir_of(checkout_git_dir: &Path) -> PathBuf {
    std::fs::read_to_string(checkout_git_dir.join("commondir"))
        .ok()
        .map(|relative| lexical_normalize(&checkout_git_dir.join(relative.trim())))
        .unwrap_or_else(|| checkout_git_dir.to_path_buf())
}

/// Resolve `.` and `..` lexically, without touching the filesystem.
///
/// `Path::canonicalize` would need every component to exist and would follow
/// symlinks, which is both stricter and looser than wanted here: the memory
/// path only has to be *stable*, so a purely textual result is the right
/// answer. Without this, a workdir spelled `/repo/crates/x/../..` produces a
/// second directory for the same repository.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            // `..` above the root is meaningless; popping past it would make
            // the result relative.
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    normalized
}

/// Slug for the repository containing `workdir`.
///
/// Derived from the **common git dir's parent** — the checkout's root — so all
/// worktrees of one repository land on the same directory. Every character
/// outside `[A-Za-z0-9._-]` becomes `-` and the result keeps its leading dash,
/// which is exactly how Claude Code spells the same thing
/// (`-Users-me-Projects-tact`); the two layouts stay comparable.
fn repo_slug(workdir: &Path) -> Option<String> {
    let root = git_common_dir(workdir)?.parent()?.to_path_buf();

    let slug = root
        .to_string_lossy()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();

    // A root of `/` slugs to `-`, which names nothing. Require a real character.
    slug.chars()
        .any(|ch| ch.is_ascii_alphanumeric())
        .then_some(slug)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Copy memories from the retired global directory into `memory_dir`.
///
/// The old layout kept one `~/.tact/memory/` for every project on the machine.
/// Scoping would strand those files, so the first session in a repository
/// copies them over. Nothing is deleted: a second checkout migrates from the
/// same source, and a user who prefers the global layout still has it.
///
/// Only runs when `memory_dir` is empty or absent — once a repository has its
/// own memories, a stale global directory must not overwrite them.
///
/// Returns the number of files copied.
pub fn migrate_legacy_memory(legacy_dir: &Path, memory_dir: &Path) -> Result<usize> {
    if legacy_dir == memory_dir || !legacy_dir.is_dir() {
        return Ok(0);
    }

    let already_scoped = std::fs::read_dir(memory_dir)
        .map(|entries| {
            entries
                .filter_map(std::result::Result::ok)
                .any(|entry| entry.path().extension().is_some_and(|ext| ext == "md"))
        })
        .unwrap_or(false);
    if already_scoped {
        return Ok(0);
    }

    let mut copied = 0;
    for entry in std::fs::read_dir(legacy_dir)?.filter_map(std::result::Result::ok) {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "md")
            || path
                .file_name()
                .is_some_and(|name| name == MEMORY_INDEX_FILE)
        {
            continue;
        }

        std::fs::create_dir_all(memory_dir)
            .with_context(|| format!("can't create memory dir: {}", memory_dir.display()))?;
        let destination = memory_dir.join(path.file_name().unwrap_or_default());
        std::fs::copy(&path, &destination).with_context(|| {
            format!(
                "can't migrate memory {} to {}",
                path.display(),
                destination.display()
            )
        })?;
        copied += 1;
    }

    Ok(copied)
}

/// The four categories of persistent memory.
///
/// Used as a YAML `type` field in memory frontmatter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Display, EnumString, AsRefStr, VariantArray)]
#[strum(serialize_all = "snake_case")]
pub enum MemoryType {
    User,
    Feedback,
    Project,
    Reference,
}

/// A single in-memory memory entry.
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub name: String,
    /// File stem on disk — the handle `load_memory` takes, and what the index
    /// prints so the model has something concrete to fetch.
    pub file_stem: String,
    pub description: String,
    pub memory_type: MemoryType,
    pub content: String,
    /// Frontmatter `modified` stamp, when the file carries one.
    pub modified: Option<String>,
}

impl std::fmt::Display for MemoryEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "### {}: {}", self.name, self.description)?;
        if !self.content.trim().is_empty() {
            writeln!(f, "{}", self.content.trim())?;
        }
        Ok(())
    }
}

/// Manages the persistent memory store.
///
/// Backed by Markdown files under `memory_dir`.  Each file begins with
/// YAML frontmatter (`name`, `description`, `type`, `modified`).  The index
/// file (`MEMORY.md`) is regenerated on every save and is what gets injected
/// into the system prompt; topic bodies are fetched through
/// [`MemoryManager::load_topic`].
pub struct MemoryManager {
    memory_dir: PathBuf,
    memories: HashMap<String, MemoryEntry>,
}

pub fn memory_manager(memory_dir: PathBuf) -> Result<MemoryManager> {
    let mut manager = MemoryManager::new(memory_dir);
    manager.load_all()?;
    Ok(manager)
}

impl MemoryManager {
    pub fn new(memory_dir: PathBuf) -> Self {
        Self {
            memory_dir,
            memories: HashMap::new(),
        }
    }

    /// The directory this manager reads and writes.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.memory_dir
    }

    pub fn load_all(&mut self) -> Result<()> {
        self.memories.clear();

        if !self.memory_dir.exists() {
            return Ok(());
        }

        for entry in WalkDir::new(&self.memory_dir)
            .max_depth(1)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file())
            .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("md"))
            .filter(|entry| entry.file_name().to_str() != Some(MEMORY_INDEX_FILE))
        {
            let path = entry.path();
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("can't read memory file: {}", path.display()))?;
            let Some(parsed) = parse_frontmatter(&content)? else {
                continue;
            };

            let file_stem = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let name = parsed
                .name
                .clone()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| file_stem.clone());
            let description = parsed.description.unwrap_or_default();
            let memory_type = parsed
                .memory_type
                .as_deref()
                .unwrap_or("project")
                .parse::<MemoryType>()?;

            self.memories.insert(
                name.clone(),
                MemoryEntry {
                    name,
                    file_stem,
                    description,
                    memory_type,
                    content: parsed.content,
                    modified: parsed.modified,
                },
            );
        }

        // The index is now the artifact the prompt injects, so it has to be
        // correct in a directory that was hand-written, edited outside the
        // session, or left behind by an older layout — `save_memory` alone is
        // not enough. Rewrite only when the content actually differs, so a
        // clean start does not churn the file's mtime.
        let expected = self.render_index();
        let current = std::fs::read_to_string(self.memory_dir.join(MEMORY_INDEX_FILE)).ok();
        if current.as_deref() != Some(expected.as_str()) {
            let _ = self.write_index(&expected);
        }

        Ok(())
    }

    /// The system-prompt block: the memory **index** plus how to read a topic.
    ///
    /// Only `MEMORY.md` is injected — the topic bodies stay on disk and one
    /// `load_memory` call away, which is what bounds the per-session cost to
    /// [`MAX_INDEX_LINES`] lines / [`MAX_INDEX_BYTES`] bytes however many
    /// memories accumulate.
    ///
    /// Returns an empty string when there is nothing to say, so a project with
    /// no memories pays nothing.
    pub fn load_memory_index_prompt(&self) -> String {
        if self.memories.is_empty() {
            return String::new();
        }

        let index = self.read_index().unwrap_or_default();
        let index = truncate_index(&index);
        if index.trim().is_empty() {
            return String::new();
        }

        format!(
            "The index below lists memories from previous sessions in this \
             repository. Read one in full with the `load_memory` tool (pass its \
             file name without the `.md` suffix) before relying on it.\n\n{index}"
        )
    }

    /// Read `MEMORY.md` as written, falling back to building it in memory when
    /// the file is unreadable.
    fn read_index(&self) -> Option<String> {
        std::fs::read_to_string(self.memory_dir.join(MEMORY_INDEX_FILE)).ok()
    }

    /// One memory's full file text (frontmatter included), by file stem.
    ///
    /// The stem is the handle the index prints. A `name` that differs from its
    /// stem is also accepted, so a model that repeats the display name still
    /// gets an answer.
    pub fn load_topic(&self, name: &str) -> Result<String> {
        if self.memories.is_empty() {
            return Err(anyhow::anyhow!(
                "Error: no memories saved for this repository yet."
            ));
        }

        let wanted = sanitize_name(name);
        let entry = self
            .memories
            .values()
            .find(|entry| entry.file_stem == wanted || sanitize_name(&entry.name) == wanted)
            .ok_or_else(|| {
                let mut known = self
                    .memories
                    .values()
                    .map(|entry| entry.file_stem.clone())
                    .collect::<Vec<_>>();
                known.sort();
                anyhow::anyhow!(
                    "Error: Unknown memory '{name}'. Available: {}",
                    known.join(", ")
                )
            })?;

        std::fs::read_to_string(self.memory_dir.join(format!("{}.md", entry.file_stem)))
            .with_context(|| format!("can't read memory '{}'", entry.name))
    }

    pub fn save_memory(
        &mut self,
        name: &str,
        description: &str,
        memory_type: MemoryType,
        content: &str,
    ) -> Result<String> {
        let safe_name = sanitize_name(name);
        if safe_name.is_empty() {
            return Err(anyhow::anyhow!("invalid memory name"));
        }

        std::fs::create_dir_all(&self.memory_dir)
            .with_context(|| format!("can't create memory dir: {}", self.memory_dir.display()))?;

        let file_name = format!("{safe_name}.md");
        let file_path = self.memory_dir.join(&file_name);
        let modified = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let frontmatter = format!(
            "---\nname: {name}\ndescription: {description}\ntype: {memory_type}\n\
             modified: {modified}\n---\n{content}\n"
        );
        std::fs::write(&file_path, frontmatter)
            .with_context(|| format!("can't write memory file: {}", file_path.display()))?;

        self.memories.insert(
            name.to_string(),
            MemoryEntry {
                name: name.to_string(),
                file_stem: safe_name,
                description: description.to_string(),
                memory_type,
                content: content.to_string(),
                modified: Some(modified),
            },
        );

        self.rebuild_index()?;

        Ok(format!(
            "Saved memory '{}' [{}] to {}",
            name,
            memory_type,
            display_save_path(&file_path)?
        ))
    }

    pub fn memories(&self) -> &HashMap<String, MemoryEntry> {
        &self.memories
    }

    pub fn describe_memories(&self) -> String {
        if self.memories.is_empty() {
            return "  (no memories)".to_string();
        }

        self.sorted_memories()
            .into_iter()
            .map(|entry| {
                format!(
                    "  [{}] {}: {}",
                    entry.memory_type, entry.name, entry.description
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Render `MEMORY.md` from the loaded entries.
    fn render_index(&self) -> String {
        let mut lines = vec!["# Memory Index".to_string(), String::new()];
        for entry in self.sorted_memories() {
            // The file name is part of the line on purpose: it is the handle
            // `load_memory` takes, and the index is the only place the model
            // learns what it can fetch.
            lines.push(format!(
                "- {} ({}.md): {} [{}]",
                entry.name, entry.file_stem, entry.description, entry.memory_type
            ));

            if lines.len() >= MAX_INDEX_LINES {
                lines.push(format!("... (truncated at {} lines)", MAX_INDEX_LINES));
                break;
            }
        }

        format!("{}\n", lines.join("\n"))
    }

    fn write_index(&self, contents: &str) -> Result<()> {
        let index_path = self.memory_dir.join(MEMORY_INDEX_FILE);
        std::fs::write(&index_path, contents)
            .with_context(|| format!("can't write memory index: {}", index_path.display()))
    }

    fn rebuild_index(&self) -> Result<()> {
        self.write_index(&self.render_index())
    }

    fn sorted_memories(&self) -> Vec<&MemoryEntry> {
        let mut memories = self.memories.values().collect::<Vec<_>>();
        memories.sort_by(|a, b| a.name.cmp(&b.name));
        memories
    }
}

#[derive(Debug, Default, Deserialize)]
struct MemoryFrontmatter {
    name: Option<String>,
    description: Option<String>,
    #[serde(rename = "type")]
    memory_type: Option<String>,
    modified: Option<String>,
}

struct ParsedMemory {
    name: Option<String>,
    description: Option<String>,
    memory_type: Option<String>,
    modified: Option<String>,
    content: String,
}

fn parse_frontmatter(text: &str) -> Result<Option<ParsedMemory>> {
    let text = text.replace("\r\n", "\n");
    let Some(rest) = text.strip_prefix("---\n") else {
        return Ok(None);
    };

    let Some((frontmatter, body)) = rest.split_once("\n---\n") else {
        return Ok(None);
    };

    let meta = serde_yaml::from_str::<MemoryFrontmatter>(frontmatter).unwrap_or_default();

    Ok(Some(ParsedMemory {
        name: meta.name,
        description: meta.description,
        memory_type: meta.memory_type,
        modified: meta.modified,
        content: body.trim().to_string(),
    }))
}

/// Apply the byte cap to the index, cutting on a line boundary.
///
/// The line cap alone does not bound the prompt — one line carries a
/// free-form description — so both limits apply, whichever trips first, the
/// same pair Claude Code uses.
fn truncate_index(index: &str) -> String {
    let mut out = String::new();

    for (count, line) in index.lines().enumerate() {
        if count >= MAX_INDEX_LINES {
            out.push_str(&format!("\n... (truncated at {} lines)", MAX_INDEX_LINES));
            break;
        }
        if out.len() + line.len() + 1 > MAX_INDEX_BYTES {
            out.push_str(&format!("\n... (truncated at {} bytes)", MAX_INDEX_BYTES));
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
    }

    out
}

fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

/// Render the path shown in the `save_memory` confirmation: `~/…` when the
/// memory lives under `$HOME` (the normal `~/.tact/...` case), otherwise
/// relative to the current directory when possible, else absolute.
fn display_save_path(path: &std::path::Path) -> anyhow::Result<String> {
    if let Some(home) = std::env::var_os("HOME")
        && let Ok(relative) = path.strip_prefix(std::path::PathBuf::from(home))
    {
        return Ok(format!("~/{}", relative.display()));
    }

    Ok(path
        .strip_prefix(std::env::current_dir()?)
        .unwrap_or(path)
        .display()
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_INDEX_LINES, MemoryManager, MemoryType, migrate_legacy_memory, parse_frontmatter,
        truncate_index,
    };

    /// A unique scratch directory under the system temp dir.
    ///
    /// The suite runs in parallel, so the name has to be per-call unique or two
    /// tests share one directory and corrupt each other's expectations.
    fn scratch(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tact_memory_{label}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_frontmatter_with_yaml_header() {
        let parsed =
            parse_frontmatter("---\nname: tabs\ndescription: prefers tabs\ntype: user\n---\nbody")
                .unwrap()
                .unwrap();

        assert_eq!(parsed.name.as_deref(), Some("tabs"));
        assert_eq!(parsed.description.as_deref(), Some("prefers tabs"));
        assert_eq!(parsed.memory_type.as_deref(), Some("user"));
        assert_eq!(parsed.content, "body");
    }

    #[test]
    fn saves_memory_and_rebuilds_index() {
        let dir = scratch("save");
        let mut manager = MemoryManager::new(dir.clone());

        manager
            .save_memory(
                "Prefer Tabs",
                "Indent with tabs",
                MemoryType::User,
                "Use tabs by default.",
            )
            .unwrap();

        let saved_file = dir.join("prefer_tabs.md");
        let index_file = dir.join("MEMORY.md");

        assert!(saved_file.exists());
        assert!(index_file.exists());

        // The index names the file, because that is the handle `load_memory`
        // takes and the index is the only place the model learns it.
        let index = std::fs::read_to_string(index_file).unwrap();
        assert!(
            index.contains("- Prefer Tabs (prefer_tabs.md): Indent with tabs [user]"),
            "{index}"
        );
        assert!(index.lines().count() <= MAX_INDEX_LINES + 1);

        // The write stamps an age, so a stale memory is recognisable.
        let saved = std::fs::read_to_string(saved_file).unwrap();
        assert!(saved.contains("modified: "), "{saved}");

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn prompt_carries_the_index_and_not_the_bodies() {
        let dir = scratch("prompt");
        let mut manager = MemoryManager::new(dir.clone());
        manager
            .save_memory(
                "Prefer Tabs",
                "Indent with tabs",
                MemoryType::User,
                "BODY_SENTINEL use tabs always.",
            )
            .unwrap();

        let prompt = manager.load_memory_index_prompt();
        assert!(prompt.contains("prefer_tabs.md"), "{prompt}");
        assert!(prompt.contains("load_memory"), "{prompt}");
        // The whole point of the alignment: the body is NOT injected.
        assert!(!prompt.contains("BODY_SENTINEL"), "{prompt}");

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_empty_store_injects_nothing() {
        let dir = scratch("empty");
        let manager = MemoryManager::new(dir.clone());
        assert_eq!(manager.load_memory_index_prompt(), "");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A directory that never went through `save_memory` still gets an index:
    /// the index is now the injected artifact, so it cannot be save-time only.
    #[test]
    fn a_hand_written_directory_gets_an_index_on_load() {
        let dir = scratch("handwritten");
        std::fs::write(
            dir.join("manual.md"),
            "---\nname: manual\ndescription: by hand\ntype: project\n---\nbody\n",
        )
        .unwrap();

        let mut manager = MemoryManager::new(dir.clone());
        manager.load_all().unwrap();

        assert!(dir.join("MEMORY.md").exists());
        assert!(manager.load_memory_index_prompt().contains("manual.md"));

        // A file dropped in *after* the first load must also appear, or the
        // model keeps seeing a stale index while the memory sits on disk.
        std::fs::write(
            dir.join("later.md"),
            "---\nname: later\ndescription: added outside\ntype: project\n---\nbody\n",
        )
        .unwrap();
        manager.load_all().unwrap();
        assert!(manager.load_memory_index_prompt().contains("later.md"));

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn load_topic_returns_the_full_file() {
        let dir = scratch("topic");
        let mut manager = MemoryManager::new(dir.clone());
        manager
            .save_memory("Tabs", "indent", MemoryType::User, "Use tabs.")
            .unwrap();

        let body = manager.load_topic("tabs").unwrap();
        assert!(body.contains("Use tabs."), "{body}");
        // Frontmatter comes back too — the model gets the metadata it may need.
        assert!(body.contains("type: user"), "{body}");

        // The display name is accepted as well, so a model repeating the index
        // title rather than the file stem is not punished for it.
        assert!(manager.load_topic("Tabs").unwrap().contains("Use tabs."));

        let error = manager.load_topic("missing").unwrap_err().to_string();
        assert!(error.contains("Unknown memory"), "{error}");
        assert!(error.contains("tabs"), "{error}");

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn byte_cap_truncates_a_long_index() {
        let index = (0..MAX_INDEX_LINES + 500)
            .map(|i| format!("- line {i} {}", "x".repeat(200)))
            .collect::<Vec<_>>()
            .join("\n");

        let capped = truncate_index(&index);
        assert!(
            capped.len() <= super::MAX_INDEX_BYTES + 64,
            "{}",
            capped.len()
        );
        assert!(capped.contains("truncated"), "{capped}");
    }

    #[test]
    fn migration_copies_memories_once_and_never_overwrites() {
        let legacy = scratch("legacy");
        let dest = scratch("scoped");
        std::fs::write(
            legacy.join("one.md"),
            "---\nname: one\ndescription: d\ntype: project\n---\nA\n",
        )
        .unwrap();
        // The index is rebuilt from scratch at the destination, never copied.
        std::fs::write(legacy.join("MEMORY.md"), "# Memory Index\n").unwrap();

        assert_eq!(migrate_legacy_memory(&legacy, &dest).unwrap(), 1);
        assert!(dest.join("one.md").exists());
        assert!(!dest.join("MEMORY.md").exists());

        // Second run: the destination is populated, so nothing is copied —
        // a stale global directory must not overwrite scoped memories.
        std::fs::write(legacy.join("two.md"), "---\nname: two\n---\nB\n").unwrap();
        assert_eq!(migrate_legacy_memory(&legacy, &dest).unwrap(), 0);
        assert!(!dest.join("two.md").exists());

        std::fs::remove_dir_all(legacy).unwrap();
        std::fs::remove_dir_all(dest).unwrap();
    }

    /// The whole point of deriving the slug from the *common* git dir: a linked
    /// worktree and its main checkout must agree, or the same repository gets
    /// two memory directories and project facts split in half.
    ///
    /// The fixture reproduces git's real on-disk shape, which is what made this
    /// test worth writing: a worktree's `.git` points at
    /// `<main>/.git/worktrees/<name>`, **not** at `<main>/.git`, and only the
    /// `commondir` file inside it leads back to the shared dir. The first
    /// version of this test stubbed `.git` pointing straight at `<main>/.git`,
    /// so it passed while real worktrees were getting private directories.
    #[test]
    fn a_worktree_and_its_checkout_resolve_to_one_directory() {
        let home = scratch("wt_home");
        let repo = scratch("wt_repo");

        // Main checkout: `.git` is a directory.
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let main = super::memory_root_for(&repo, &home);

        // Linked worktree, spelled the way git spells it.
        let worktree_git_dir = repo.join(".git").join("worktrees").join("feature");
        std::fs::create_dir_all(&worktree_git_dir).unwrap();
        std::fs::write(worktree_git_dir.join("commondir"), "../..\n").unwrap();
        let worktree = repo.join(".worktrees").join("feature");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", worktree_git_dir.display()),
        )
        .unwrap();
        let linked = super::memory_root_for(&worktree, &home);

        assert_eq!(
            main, linked,
            "worktree must share the checkout's memory dir"
        );
        assert!(main.starts_with(home.join(".tact").join("projects")));

        // And a subdirectory of either resolves to the same place.
        let nested = repo.join("crates").join("tact");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(super::memory_root_for(&nested, &home), main);

        std::fs::remove_dir_all(home).unwrap();
        std::fs::remove_dir_all(repo).unwrap();
    }

    /// Against the *real* worktrees in this repository, not a fixture.
    ///
    /// The fixture above encodes what git is believed to write; this asserts
    /// the actual `.git` files on disk resolve into the same directory as the
    /// main checkout. Skipped where the layout is absent (a release tarball, a
    /// fresh clone's CI checkout), because the fixture already covers the logic.
    #[test]
    fn the_real_worktrees_in_this_repository_share_the_main_directory() {
        let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
            return;
        };
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .map(std::path::Path::to_path_buf)
            .unwrap();
        let worktrees = repo_root.join(".worktrees");
        if !worktrees.is_dir() {
            return;
        }

        // The main checkout is authoritative; every worktree must match it.
        let expected = super::memory_root_for(&repo_root, &home);

        let mut checked = 0;
        let mut stack = vec![worktrees.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.join(".git").is_file() {
                    assert_eq!(
                        super::memory_root_for(&path, &home),
                        expected,
                        "worktree {} must share the main memory dir",
                        path.display()
                    );
                    checked += 1;
                } else if path.is_dir() {
                    stack.push(path);
                }
            }
        }

        assert!(
            checked > 0,
            "no worktrees found under {}",
            worktrees.display()
        );
    }

    /// The config key is threaded all the way to the decision: a resolved
    /// config carrying `auto_memory_directory` produces a different directory
    /// than the derived one.
    #[test]
    fn the_config_value_is_what_the_resolver_reads() {
        assert_eq!(super::override_from_settings(None), None);

        // The extraction is a plain read of the resolved field; build one the
        // same way `resolve` does rather than mutating process-global settings.
        let mut config = crate::config::test_support::resolved_config();
        config.agent.auto_memory_directory = Some("~/from-config".to_string());
        assert_eq!(
            super::override_from_settings(Some(&config)).as_deref(),
            Some("~/from-config")
        );

        config.agent.auto_memory_directory = None;
        assert_eq!(super::override_from_settings(Some(&config)), None);
    }

    /// `[agent].auto_memory_directory` overrides the derived path, with the
    /// same three expansion rules `[agent].skill_dirs` uses.
    #[test]
    fn a_configured_directory_overrides_the_derived_one() {
        let home = scratch("override_home");
        let repo = scratch("override_repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();

        // Without an override the repository decides.
        let derived = super::memory_root_with(&repo, Some(&home), None);
        assert_eq!(derived, super::memory_root_for(&repo, &home));
        assert!(derived.starts_with(home.join(".tact").join("projects")));

        // With one, the override wins outright — including out of the
        // per-repository layout that would otherwise apply.
        assert_eq!(
            super::memory_root_with(&repo, Some(&home), Some("/var/lib/tact-memory")),
            std::path::PathBuf::from("/var/lib/tact-memory")
        );
        assert_eq!(
            super::memory_root_with(&repo, Some(&home), Some("./memories")),
            repo.join("memories"),
            "a relative override resolves against the workdir"
        );
        assert_eq!(
            super::memory_root_with(&repo, Some(&home), Some("~/my-memories")),
            std::path::PathBuf::from(std::env::var("HOME").unwrap()).join("my-memories"),
            "`~` expands against the real home, not the one passed in"
        );

        // The override is consulted before the home fallback, so it still
        // applies when `$HOME` is unset.
        assert_eq!(
            super::memory_root_with(&repo, None, Some("/var/lib/tact-memory")),
            std::path::PathBuf::from("/var/lib/tact-memory")
        );

        std::fs::remove_dir_all(home).unwrap();
        std::fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn expand_memory_dir_follows_the_skill_dirs_rules() {
        let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
        let workdir = std::path::PathBuf::from("/tmp/whatever");

        assert_eq!(super::expand_memory_dir("~", &workdir), home);
        assert_eq!(
            super::expand_memory_dir("~/my-memories", &workdir),
            home.join("my-memories")
        );
        assert_eq!(
            super::expand_memory_dir("./memories", &workdir),
            workdir.join("memories")
        );
        assert_eq!(
            super::expand_memory_dir("/var/lib/tact-memory", &workdir),
            std::path::PathBuf::from("/var/lib/tact-memory")
        );
        // Surrounding whitespace is a config-file typo, not part of the path.
        assert_eq!(
            super::expand_memory_dir("  ~/trimmed  ", &workdir),
            home.join("trimmed")
        );
    }

    #[test]
    fn outside_a_repository_the_workdir_owns_the_directory() {
        let home = scratch("norepo_home");
        let plain = scratch("norepo_dir");

        let root = super::memory_root_for(&plain, &home);
        assert_eq!(root, plain.join(".tact").join("memory"));

        std::fs::remove_dir_all(home).unwrap();
        std::fs::remove_dir_all(plain).unwrap();
    }

    /// A workdir spelled with `..` must not produce a second directory for the
    /// same repository. Caught in practice: the first version slugged
    /// `/repo/crates/tact/../..` and gave tact two memory stores.
    #[test]
    fn a_dotted_workdir_resolves_like_its_normal_form() {
        let home = scratch("dotted_home");
        let repo = scratch("dotted_repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let nested = repo.join("crates").join("tact");
        std::fs::create_dir_all(&nested).unwrap();

        let dotted = nested.join("..").join("..");
        assert_eq!(
            super::memory_root_for(&dotted, &home),
            super::memory_root_for(&repo, &home),
            "`..` must be resolved before slugging"
        );

        std::fs::remove_dir_all(home).unwrap();
        std::fs::remove_dir_all(repo).unwrap();
    }

    /// Two different repositories must not share a directory — that is the
    /// leak the per-repository layout exists to close.
    #[test]
    fn different_repositories_get_different_directories() {
        let home = scratch("two_home");
        let first = scratch("two_first");
        let second = scratch("two_second");
        std::fs::create_dir_all(first.join(".git")).unwrap();
        std::fs::create_dir_all(second.join(".git")).unwrap();

        assert_ne!(
            super::memory_root_for(&first, &home),
            super::memory_root_for(&second, &home)
        );

        std::fs::remove_dir_all(home).unwrap();
        std::fs::remove_dir_all(first).unwrap();
        std::fs::remove_dir_all(second).unwrap();
    }
}

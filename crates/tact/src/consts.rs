use std::path::{Path, PathBuf};

/// Directories under the workdir.  Kept private; accessed via [`TactPath`].
const TACT_DIR: &str = ".tact";
const CLAUDE_DIR: &str = ".claude";
const MEMORY_DIR: &str = "memory";
const SKILL_DIR: &str = "skills";

/// Sub-directories under `.claude/`.  Available through [`TactPath`] methods.
const TRANSCRIPT_SUBDIR: &str = "transcripts";
const TOOL_RESULTS_SUBDIR: &str = "tool-results";
const CRON_SUBDIR: &str = "cron";

/// Centralised path abstraction for all tact directories.
///
/// Construct with [`TactPath::new`] (any workdir) or [`TactPath::from_cwd`].
/// Paths are computed lazily — field accessors are equivalent to a
/// `PathBuf::join`.
#[derive(Clone, Debug)]
pub struct TactPath {
    workdir: PathBuf,
}

impl TactPath {
    // ----------------------------------------------------------------
    // Constructors
    // ----------------------------------------------------------------

    pub fn new(workdir: impl Into<PathBuf>) -> Self {
        Self {
            workdir: workdir.into(),
        }
    }

    pub fn from_cwd() -> std::io::Result<Self> {
        Ok(Self::new(std::env::current_dir()?))
    }

    // ----------------------------------------------------------------
    // Workdir & top-level dirs
    // ----------------------------------------------------------------

    /// The root working directory passed to the constructor.
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// `<workdir>/.tact`
    pub fn tact_dir(&self) -> PathBuf {
        self.workdir.join(TACT_DIR)
    }

    /// `<workdir>/.tact/tact.db` — SQLite session store.
    pub fn session_db_path(&self) -> PathBuf {
        self.tact_dir().join("tact.db")
    }

    /// `<workdir>/.claude`
    pub fn claude_dir(&self) -> PathBuf {
        self.workdir.join(CLAUDE_DIR)
    }

    /// `<workdir>/.claude/skills` — Claude Code–compatible project skills.
    pub fn skills_dir(&self) -> PathBuf {
        self.claude_dir().join(SKILL_DIR)
    }

    /// Legacy `<workdir>/skills` (still scanned for backward compatibility).
    pub fn legacy_skills_dir(&self) -> PathBuf {
        self.workdir.join(SKILL_DIR)
    }

    /// Skill roots in load order. Later entries win on name clash:
    /// legacy `<workdir>/skills` → `~/.claude/skills` → `<workdir>/.claude/skills`.
    pub fn skill_search_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = vec![self.legacy_skills_dir()];
        if let Some(home) = Self::home_claude_dir() {
            dirs.push(home.join(SKILL_DIR));
        }
        dirs.push(self.skills_dir());
        dirs
    }

    // ----------------------------------------------------------------
    // Subdirectories under `.claude/`
    // ----------------------------------------------------------------

    /// `<workdir>/.claude/memory`
    pub fn memory_dir(&self) -> PathBuf {
        self.claude_dir().join(MEMORY_DIR)
    }

    /// `<workdir>/.claude/transcripts`
    pub fn transcript_dir(&self) -> PathBuf {
        self.claude_dir().join(TRANSCRIPT_SUBDIR)
    }

    /// `<workdir>/.claude/tool-results`
    pub fn tool_results_dir(&self) -> PathBuf {
        self.claude_dir().join(TOOL_RESULTS_SUBDIR)
    }

    /// `<workdir>/.claude/cron`
    pub fn cron_dir(&self) -> PathBuf {
        self.claude_dir().join(CRON_SUBDIR)
    }

    // ----------------------------------------------------------------
    // Home-directory paths (global config)
    // ----------------------------------------------------------------

    /// `$HOME/.tact` — global tact config directory.
    pub fn home_tact_dir() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(TACT_DIR))
    }

    /// `$HOME/.claude` — global claude config directory.
    pub fn home_claude_dir() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(CLAUDE_DIR))
    }
}

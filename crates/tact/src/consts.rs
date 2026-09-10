use std::path::{Path, PathBuf};

/// Hard cap on user input / skill payload length in characters.
///
/// Independent of the model context window (token budget). Do not use
/// `model_context_window` as a character submit limit.
pub const MAX_INPUT_CHARS: usize = 500_000;

/// Home-directory paths used by the plugin marketplace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginHome {
    /// `$HOME` — the root every discovered compatibility path resolves against.
    ///
    /// Stored explicitly so path derivation never depends on the depth of
    /// [`Self::root`].
    pub home: PathBuf,
    /// `$HOME/.tact/plugins` — plugin root, holding derived directories only.
    pub root: PathBuf,
    /// `$HOME/.tact/plugins/state` — `installed.json`, `marketplaces.json`.
    ///
    /// Kept apart from [`Self::cache`]: state is a few KB and worth backing up,
    /// while the cache is hundreds of MB and disposable.
    pub state: PathBuf,
    pub marketplaces: PathBuf,
    pub cache: PathBuf,
}

impl PluginHome {
    /// Resolves the plugin marketplace directories under the current user's home.
    #[must_use]
    pub fn from_environment() -> Option<Self> {
        std::env::var_os("HOME").map(|home| Self::from_home(Path::new(&home)))
    }

    /// Resolves the plugin marketplace directories under `home`.
    #[must_use]
    pub fn from_home(home: &Path) -> Self {
        let root = home.join(".tact").join("plugins");
        Self {
            home: home.to_path_buf(),
            state: root.join("state"),
            marketplaces: root.join("marketplaces"),
            cache: root.join("cache"),
            root,
        }
    }
}

/// Returns true when `char_count` exceeds [`MAX_INPUT_CHARS`].
#[inline]
pub fn exceeds_input_char_limit(char_count: usize) -> bool {
    char_count > MAX_INPUT_CHARS
}

#[cfg(test)]
mod tact_path_tests {
    use std::path::{Path, PathBuf};

    use super::TactPath;

    #[test]
    fn home_memory_dir_for_joins_home_tact_memory() {
        assert_eq!(
            TactPath::home_memory_dir_for(Path::new("/home/alice")),
            Path::new("/home/alice/.tact/memory")
        );
    }

    #[test]
    fn tact_skill_root_outranks_the_agents_compatibility_root() {
        // Asserts on relative positions rather than mutating `HOME`, so this
        // stays safe alongside tests that run in parallel.
        let workdir = PathBuf::from("/proj");
        let dirs = TactPath::new(&workdir).skill_search_dirs();
        let home = std::env::var_os("HOME").expect("HOME is set for tests");

        let agents = PathBuf::from(&home).join(".agents/skills");
        let tact = PathBuf::from(&home).join(".tact/skills");
        let project = workdir.join(".tact/skills");

        let position = |needle: &Path| {
            dirs
                .iter()
                .position(|dir| dir == needle)
                .unwrap_or_else(|| panic!("{} missing from {dirs:?}", needle.display()))
        };

        // Later wins, so these must be strictly increasing.
        assert!(
            position(&agents) < position(&tact),
            "Tact's own root must beat the Codex compatibility root: {dirs:?}"
        );
        assert!(
            position(&tact) < position(&project),
            "the project root must beat every user root: {dirs:?}"
        );
        assert_eq!(dirs.len(), 3);
    }
}

#[cfg(test)]
mod input_limit_tests {
    use super::{MAX_INPUT_CHARS, exceeds_input_char_limit};

    #[test]
    fn exceeds_input_char_limit_at_boundaries() {
        assert!(!exceeds_input_char_limit(0));
        assert!(!exceeds_input_char_limit(MAX_INPUT_CHARS));
        assert!(exceeds_input_char_limit(MAX_INPUT_CHARS + 1));
    }
}

/// Directories under the workdir.  Kept private; accessed via [`TactPath`].
const TACT_DIR: &str = ".tact";
const AGENTS_DIR: &str = ".agents";
const MEMORY_DIR: &str = "memory";
const SKILL_DIR: &str = "skills";

/// Sub-directory names used under `.tact/`.  Available through [`TactPath`] methods.
const TRANSCRIPT_SUBDIR: &str = "transcripts";
const TOOL_RESULTS_SUBDIR: &str = "tool-results";

/// Tact's native MCP server declaration file, read from `.tact/` at both
/// project and user scope.
const MCP_CONFIG_FILE: &str = "mcp.json";

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

    /// `<workdir>/.tact/skills` — project-local tact skills.
    pub fn tact_skills_dir(&self) -> PathBuf {
        self.tact_dir().join(SKILL_DIR)
    }

    /// Skill roots in load order (later entries win on name clash):
    /// `~/.agents/skills` → `~/.tact/skills` → `<workdir>/.tact/skills`.
    ///
    /// The Codex compatibility root comes first so it loses to Tact's own root
    /// and to the project root — a project skill always wins, and Tact's
    /// canonical root always beats the foreign one.
    ///
    /// Callers may append config `[agent].skill_dirs` after these.
    pub fn skill_search_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Some(home) = Self::home_agents_dir() {
            dirs.push(home.join(SKILL_DIR));
        }
        if let Some(home) = Self::home_tact_dir() {
            dirs.push(home.join(SKILL_DIR));
        }
        dirs.push(self.tact_skills_dir());
        dirs
    }

    // ----------------------------------------------------------------
    // Subdirectories under `.tact/`
    // ----------------------------------------------------------------

    /// `<workdir>/.tact/memory` — legacy project-local memory directory.
    ///
    /// The agent runtime keeps memory user-global under
    /// [`home_memory_dir`](Self::home_memory_dir); this project-local path is
    /// only used as a fallback when `$HOME` is unset.
    pub fn memory_dir(&self) -> PathBuf {
        self.tact_dir().join(MEMORY_DIR)
    }

    /// `<workdir>/.tact/transcripts`
    pub fn transcript_dir(&self) -> PathBuf {
        self.tact_dir().join(TRANSCRIPT_SUBDIR)
    }

    /// `<workdir>/.tact/mcp.json` — project-scoped MCP server declarations.
    ///
    /// Preferred over every compatibility source (`.mcp.json`,
    /// `.codex-plugin/plugin.json`), which are read only as read-only inputs.
    pub fn mcp_config_path(&self) -> PathBuf {
        self.tact_dir().join(MCP_CONFIG_FILE)
    }

    /// `<workdir>/.tact/tool-results`
    pub fn tool_results_dir(&self) -> PathBuf {
        self.tact_dir().join(TOOL_RESULTS_SUBDIR)
    }

    // ----------------------------------------------------------------
    // Home-directory paths (global config)
    // ----------------------------------------------------------------

    /// `<workdir>/.tact/settings.json` — project-scoped permission settings.
    pub fn settings_path(&self) -> PathBuf {
        self.tact_dir().join("settings.json")
    }

    /// `$HOME/.tact/settings.json` — global permission settings.
    pub fn home_settings_path() -> Option<PathBuf> {
        Self::home_tact_dir().map(|dir| dir.join("settings.json"))
    }

    /// `$HOME/.tact/mcp.json` — user-global MCP server declarations.
    ///
    /// Lower precedence than the project file: a project entry of the same
    /// server name overrides it.
    pub fn home_mcp_config_path() -> Option<PathBuf> {
        Self::home_tact_dir().map(|dir| dir.join(MCP_CONFIG_FILE))
    }

    /// `$HOME/.tact` — global tact config directory.
    pub fn home_tact_dir() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(TACT_DIR))
    }

    /// `$HOME/.tact/memory` — user-global persistent memory directory, shared
    /// across all projects (the tact analogue of a user-level global store).
    pub fn home_memory_dir() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|home| Self::home_memory_dir_for(Path::new(&home)))
    }

    /// [`home_memory_dir`](Self::home_memory_dir) with an explicit home path
    /// (kept separate so path derivation is unit-testable without env vars).
    #[must_use]
    pub fn home_memory_dir_for(home: &Path) -> PathBuf {
        home.join(TACT_DIR).join(MEMORY_DIR)
    }

    /// `$HOME/.agents` — global agents config directory.
    pub fn home_agents_dir() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(AGENTS_DIR))
    }
}

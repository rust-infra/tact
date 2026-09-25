//! Persisted window-layout preferences.
//!
//! The v1 shell is fixed: the sidebar and work pane keep the prototype's
//! widths and the pane arrangement is derived from the workspace preset. The
//! post-v1 layout store is the first thing that has to survive a restart, so
//! it lives in its own module rather than as more fields on [`TactApp`].
//!
//! The store is deliberately small and boring: one JSON document per user at
//! `~/.tact/gui-layout.json`, written atomically and read with `#[serde(default)]`
//! so a partial or older document still loads. A malformed document is
//! ignored rather than fatal — losing a pane width is not worth refusing to
//! start the app.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use gpui_kit::component::ThemeMode;
use serde::{Deserialize, Serialize};

use crate::pane::WorkPane;
use crate::shell::Workspace;
use crate::transcript::TranscriptDetail;

/// Prototype sidebar width, in rems (260 px at the default 16 px rem).
pub(crate) const SIDEBAR_WIDTH_REM: f32 = 16.25;
/// Work-pane width, in rems (384 px at the default 16 px rem).
///
/// The prototype draws 420 px; the shell opens narrower so the conversation
/// keeps more of the window. The divider drags back out to, and past, the
/// prototype's number.
pub(crate) const WORK_PANE_WIDTH_REM: f32 = 24.0;

/// Narrowest sidebar the user can drag the column to. Below the prototype's
/// 244 px narrow width, but still wide enough for a session title.
pub(crate) const SIDEBAR_MIN_REM: f32 = 12.0;
/// Widest sidebar, so a drag cannot swallow the transcript.
pub(crate) const SIDEBAR_MAX_REM: f32 = 24.0;
/// Narrowest work pane, matching the prototype's 374 px narrow column.
pub(crate) const WORK_PANE_MIN_REM: f32 = 20.0;
/// Widest work pane, so a drag cannot swallow the transcript.
pub(crate) const WORK_PANE_MAX_REM: f32 = 36.0;

/// Default base font size, in px: the value `rems(1.)` resolves to.
pub(crate) const ZOOM_DEFAULT: f32 = 16.0;
/// Workspace directories the shell remembers, newest first.
pub(crate) const RECENT_WORKSPACES: usize = 8;

/// Smallest zoom the shell allows. Below this the 10.5 px session metadata
/// stops being legible.
pub(crate) const ZOOM_MIN: f32 = 12.0;
/// Largest zoom. The shell's columns are `rem`-based, so past this the work
/// pane and sidebar together exceed the minimum window width.
pub(crate) const ZOOM_MAX: f32 = 24.0;

/// Which edge the work pane docks to.
///
/// The prototype fixes the pane to the right. `Left` and `Bottom` are the
/// shell's own extension: they move the same pane without introducing a
/// free-form splitter tree, so the layout stays something the app can describe
/// in one small document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum WorkPaneSide {
    #[default]
    Right,
    Left,
    Bottom,
}

impl WorkPaneSide {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Right => "Right",
            Self::Left => "Left",
            Self::Bottom => "Bottom",
        }
    }

    /// The next placement in the cycle the header button walks.
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Right => Self::Left,
            Self::Left => Self::Bottom,
            Self::Bottom => Self::Right,
        }
    }
}

/// A saved pane arrangement. Presets are a coarse shorthand over the four
/// independent fields; the fields win when the two disagree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LayoutPreset {
    /// Sidebar and work pane both open — the prototype's default.
    #[default]
    Split,
    /// Work pane closed, sidebar open: the reading view.
    Focus,
    /// Sidebar closed, work pane open: the reviewing view.
    Review,
    /// Both columns closed: the transcript alone.
    Zen,
}

impl LayoutPreset {
    /// Every preset in palette order.
    pub(crate) const ALL: [Self; 4] = [Self::Split, Self::Focus, Self::Review, Self::Zen];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Split => "Split",
            Self::Focus => "Focus",
            Self::Review => "Review",
            Self::Zen => "Zen",
        }
    }

    /// The arrangement this preset restores.
    ///
    /// Sidebar and work pane widths are deliberately *not* part of a preset:
    /// they are the user's own drag tuning, and a preset that reset them would
    /// throw that away every time the view changed.
    pub(crate) fn arrangement(self) -> (bool, bool) {
        match self {
            Self::Split => (true, true),
            Self::Focus => (true, false),
            Self::Review => (false, true),
            Self::Zen => (false, false),
        }
    }
}

/// Everything about the shell's arrangement that survives a restart.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct LayoutPrefs {
    /// The preset the shell last showed.
    pub preset: LayoutPreset,
    pub sidebar_open: bool,
    pub work_pane_open: bool,
    pub workspace: Workspace,
    pub work_pane: WorkPane,
    pub detail: TranscriptDetail,
    pub sidebar_width_rem: f32,
    pub work_pane_width_rem: f32,
    /// Which edge the work pane docks to.
    pub work_pane_side: WorkPaneSide,
    /// Workspace directories the user has opened, newest first.
    ///
    /// A workspace *is* a directory: the session store lives in
    /// `<workspace>/.tact/tact.db`, so remembering the path is the whole of
    /// remembering the project. Paths, not names, because two projects can
    /// share a directory name.
    pub recent_workspaces: Vec<PathBuf>,
    /// Base font size in px: the whole shell is `rem`-based, so this is the
    /// zoom control.
    pub zoom_rem: f32,
    /// Whether the sidebar lists archived sessions.
    pub show_archived: bool,
    /// The interface font, chosen from the machine's installed families.
    ///
    /// `None` keeps the theme's own family. Stored by name rather than by path
    /// so a font that is later uninstalled simply falls back instead of
    /// pointing at a file that is gone.
    pub ui_font: Option<String>,
    /// The light/dark mode the shell last showed.
    ///
    /// `None` means the user has never chosen: the shell keeps the prototype's
    /// dark opening rather than the UI framework's light default. Storing the
    /// absence is what keeps an old document from flipping a dark shell to
    /// light on the first launch after this field exists.
    pub theme_mode: Option<ThemeMode>,
    /// Whether the transcript keeps the newest output in view while a turn
    /// runs.
    pub follow_tail: bool,
}

impl Default for LayoutPrefs {
    fn default() -> Self {
        let (sidebar_open, work_pane_open) = LayoutPreset::default().arrangement();
        Self {
            preset: LayoutPreset::default(),
            sidebar_open,
            work_pane_open,
            workspace: Workspace::default(),
            work_pane: Workspace::default().work_pane(),
            detail: TranscriptDetail::default(),
            sidebar_width_rem: SIDEBAR_WIDTH_REM,
            work_pane_width_rem: WORK_PANE_WIDTH_REM,
            work_pane_side: WorkPaneSide::default(),
            recent_workspaces: Vec::new(),
            zoom_rem: ZOOM_DEFAULT,
            ui_font: None,
            show_archived: false,
            theme_mode: None,
            follow_tail: true,
        }
    }
}

impl LayoutPrefs {
    /// Clamp both widths into their draggable range.
    ///
    /// Hand-edited or truncated JSON is the only way an out-of-range value
    /// reaches here, but a width of `0` would collapse a column permanently,
    /// so the clamp runs on load and after every drag rather than trusting the
    /// document.
    pub(crate) fn clamp(&mut self) {
        self.sidebar_width_rem = clamp_width(
            self.sidebar_width_rem,
            SIDEBAR_MIN_REM,
            SIDEBAR_MAX_REM,
            SIDEBAR_WIDTH_REM,
        );
        self.work_pane_width_rem = clamp_width(
            self.work_pane_width_rem,
            WORK_PANE_MIN_REM,
            WORK_PANE_MAX_REM,
            WORK_PANE_WIDTH_REM,
        );
        self.zoom_rem = clamp_width(self.zoom_rem, ZOOM_MIN, ZOOM_MAX, ZOOM_DEFAULT);
    }

    /// Remember a workspace directory, newest first and de-duplicated.
    pub(crate) fn remember_workspace(&mut self, path: &Path) {
        self.recent_workspaces.retain(|seen| seen != path);
        self.recent_workspaces.insert(0, path.to_path_buf());
        self.recent_workspaces.truncate(RECENT_WORKSPACES);
    }

    /// Apply a preset's arrangement, keeping the dragged widths.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn apply_preset(&mut self, preset: LayoutPreset) {
        self.preset = preset;
        let (sidebar_open, work_pane_open) = preset.arrangement();
        self.sidebar_open = sidebar_open;
        self.work_pane_open = work_pane_open;
    }

    /// The preset that matches the current open flags, when one does.
    ///
    /// The title-bar control shows this instead of `preset` so a manual toggle
    /// that lands on another arrangement relabels itself honestly, and a
    /// hand-edited document that disagrees with its preset still renders the
    /// arrangement it actually describes.
    pub(crate) fn matching_preset(&self) -> Option<LayoutPreset> {
        LayoutPreset::ALL
            .into_iter()
            .find(|preset| preset.arrangement() == (self.sidebar_open, self.work_pane_open))
    }
}

/// Clamp a zoom level into its supported range.
pub(crate) fn clamp_zoom(value: f32) -> f32 {
    clamp_width(value, ZOOM_MIN, ZOOM_MAX, ZOOM_DEFAULT)
}

/// A base font size as a whole-number percentage of the default one.
///
/// The settings row and the status chip both report the level, so the rounding
/// lives in one place: two renderers computing "106%" independently is the kind
/// of pair that drifts a step apart and reads as a bug.
pub(crate) fn zoom_percent(zoom_rem: f32) -> i32 {
    (zoom_rem / ZOOM_DEFAULT * 100.0).round() as i32
}

/// Clamp a dragged sidebar width into its supported range.
pub(crate) fn clamp_sidebar_width(value: f32) -> f32 {
    clamp_width(value, SIDEBAR_MIN_REM, SIDEBAR_MAX_REM, SIDEBAR_WIDTH_REM)
}

/// Clamp a dragged work-pane width into its supported range.
pub(crate) fn clamp_work_pane_width(value: f32) -> f32 {
    clamp_width(
        value,
        WORK_PANE_MIN_REM,
        WORK_PANE_MAX_REM,
        WORK_PANE_WIDTH_REM,
    )
}

fn clamp_width(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}

/// Where the layout document lives, and how to read and write it.
#[derive(Clone, Debug)]
pub(crate) struct LayoutStore {
    path: Option<PathBuf>,
}

impl LayoutStore {
    /// The user's store: `$TACT_GUI_LAYOUT_PATH`, else `~/.tact/gui-layout.json`.
    ///
    /// The environment override exists so tests never touch the developer's
    /// real layout, and so a user can keep two shells apart without a second
    /// config system.
    pub(crate) fn user() -> Self {
        let path = std::env::var_os("TACT_GUI_LAYOUT_PATH")
            .map(PathBuf::from)
            .or_else(|| home_tact_dir().map(|dir| dir.join("gui-layout.json")));
        Self { path }
    }

    /// A store rooted at an explicit path, for tests and offline shells.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn at(path: impl Into<PathBuf>) -> Self {
        Self {
            path: Some(path.into()),
        }
    }

    /// A store that never reads or writes: the offline shell's default.
    pub(crate) fn disabled() -> Self {
        Self { path: None }
    }

    /// Read the document, falling back to defaults on any error.
    ///
    /// A missing file is the common case on first launch and is not an error.
    /// A malformed one is logged and ignored, because refusing to open the
    /// window over an unreadable preference is a worse failure than a
    /// default-sized pane.
    pub(crate) fn load(&self) -> LayoutPrefs {
        let Some(path) = self.path.as_deref() else {
            return LayoutPrefs::default();
        };
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return LayoutPrefs::default();
            }
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "could not read GUI layout");
                return LayoutPrefs::default();
            }
        };
        match serde_json::from_slice::<LayoutPrefs>(&bytes) {
            Ok(mut prefs) => {
                prefs.clamp();
                prefs
            }
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "ignoring malformed GUI layout");
                LayoutPrefs::default()
            }
        }
    }

    /// Write the document, creating `~/.tact` if needed.
    ///
    /// The write goes to a sibling temporary file and is then renamed, so a
    /// crash mid-write leaves the previous layout intact instead of a
    /// half-written document that would fail to parse on the next launch.
    pub(crate) fn save(&self, prefs: &LayoutPrefs) -> io::Result<()> {
        let Some(path) = self.path.as_deref() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut prefs = prefs.clone();
        prefs.clamp();
        let body = serde_json::to_vec_pretty(&prefs)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, body)?;
        fs::rename(&temporary, path)
    }

    /// Persist, logging rather than propagating: a failed save must not take
    /// down a UI callback.
    pub(crate) fn persist(&self, prefs: &LayoutPrefs) {
        if let Err(error) = self.save(prefs) {
            tracing::warn!(?error, "could not save GUI layout");
        }
    }
}

fn home_tact_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".tact"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_document_loads_the_prototype_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let store = LayoutStore::at(dir.path().join("gui-layout.json"));
        let prefs = store.load();
        assert_eq!(prefs, LayoutPrefs::default());
        assert!(prefs.sidebar_open);
        assert!(prefs.work_pane_open);
        assert_eq!(prefs.sidebar_width_rem, SIDEBAR_WIDTH_REM);
        assert_eq!(prefs.work_pane_width_rem, WORK_PANE_WIDTH_REM);
    }

    #[test]
    fn recent_workspaces_round_trip_newest_first_and_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let store = LayoutStore::at(dir.path().join("projects.json"));
        let mut prefs = LayoutPrefs::default();
        for index in 0..RECENT_WORKSPACES + 3 {
            prefs.remember_workspace(Path::new(&format!("/tmp/project-{index}")));
        }
        assert_eq!(prefs.recent_workspaces.len(), RECENT_WORKSPACES);
        assert_eq!(
            prefs.recent_workspaces[0],
            PathBuf::from(format!("/tmp/project-{}", RECENT_WORKSPACES + 2)),
            "the newest workspace leads"
        );

        // Re-opening an old project moves it back to the front instead of
        // appearing twice.
        let revisited = prefs.recent_workspaces[3].clone();
        prefs.remember_workspace(&revisited);
        assert_eq!(prefs.recent_workspaces[0], revisited);
        assert_eq!(
            prefs
                .recent_workspaces
                .iter()
                .filter(|path| **path == revisited)
                .count(),
            1,
            "a workspace is listed once"
        );

        store.save(&prefs).unwrap();
        assert_eq!(store.load().recent_workspaces, prefs.recent_workspaces);
    }

    #[test]
    fn the_work_pane_side_round_trips_and_cycles() {
        let dir = tempfile::tempdir().unwrap();
        let store = LayoutStore::at(dir.path().join("side.json"));
        let prefs = LayoutPrefs {
            work_pane_side: WorkPaneSide::Bottom,
            ..LayoutPrefs::default()
        };
        store.save(&prefs).unwrap();
        assert_eq!(store.load().work_pane_side, WorkPaneSide::Bottom);

        assert_eq!(WorkPaneSide::Right.next(), WorkPaneSide::Left);
        assert_eq!(WorkPaneSide::Left.next(), WorkPaneSide::Bottom);
        assert_eq!(
            WorkPaneSide::Bottom.next(),
            WorkPaneSide::Right,
            "the cycle returns to where it started"
        );
    }

    #[test]
    fn a_round_trip_preserves_the_arrangement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/gui-layout.json");
        let store = LayoutStore::at(&path);
        let mut prefs = LayoutPrefs::default();
        prefs.apply_preset(LayoutPreset::Review);
        prefs.workspace = Workspace::Code;
        prefs.work_pane = WorkPane::Diff;
        prefs.detail = TranscriptDetail::Verbose;
        prefs.sidebar_width_rem = 14.5;
        prefs.work_pane_width_rem = 30.0;
        store.save(&prefs).unwrap();

        let loaded = store.load();
        assert_eq!(loaded, prefs);
    }

    #[test]
    fn a_partial_document_fills_the_missing_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gui-layout.json");
        fs::write(&path, br#"{"preset":"zen","sidebar_width_rem":13.0}"#).unwrap();
        let prefs = LayoutStore::at(&path).load();
        assert!(
            prefs.sidebar_open,
            "the preset field alone does not move the flags"
        );
        assert_eq!(prefs.sidebar_width_rem, 13.0);
        assert_eq!(prefs.work_pane_width_rem, WORK_PANE_WIDTH_REM);
        assert_eq!(prefs.workspace, Workspace::default());
    }

    #[test]
    fn a_malformed_document_is_ignored_rather_than_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gui-layout.json");
        fs::write(&path, b"{not json").unwrap();
        assert_eq!(LayoutStore::at(&path).load(), LayoutPrefs::default());
    }

    #[test]
    fn zoom_is_clamped_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gui-layout.json");
        fs::write(&path, br#"{"zoom_rem":99.0}"#).unwrap();
        let prefs = LayoutStore::at(&path).load();
        assert_eq!(prefs.zoom_rem, ZOOM_MAX);

        let store = LayoutStore::at(dir.path().join("round.json"));
        let prefs = LayoutPrefs {
            zoom_rem: 20.0,
            ..LayoutPrefs::default()
        };
        store.save(&prefs).unwrap();
        assert_eq!(store.load().zoom_rem, 20.0);
    }

    #[test]
    fn the_zoom_percentage_reads_the_base_size() {
        assert_eq!(zoom_percent(ZOOM_DEFAULT), 100);
        assert_eq!(zoom_percent(ZOOM_MIN), 75, "the smallest base size");
        assert_eq!(zoom_percent(ZOOM_MAX), 150, "the largest base size");
        assert_eq!(
            zoom_percent(ZOOM_DEFAULT + 1.0),
            106,
            "one step in is the level the status chip reports"
        );
    }

    #[test]
    fn widths_are_clamped_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gui-layout.json");
        fs::write(
            &path,
            br#"{"sidebar_width_rem":0.0,"work_pane_width_rem":9999.0}"#,
        )
        .unwrap();
        let prefs = LayoutStore::at(&path).load();
        assert_eq!(prefs.sidebar_width_rem, SIDEBAR_MIN_REM);
        assert_eq!(prefs.work_pane_width_rem, WORK_PANE_MAX_REM);
    }

    #[test]
    fn a_preset_keeps_the_dragged_widths() {
        let mut prefs = LayoutPrefs {
            sidebar_width_rem: 15.0,
            work_pane_width_rem: 31.0,
            ..LayoutPrefs::default()
        };
        prefs.apply_preset(LayoutPreset::Zen);
        assert!(!prefs.sidebar_open);
        assert!(!prefs.work_pane_open);
        assert_eq!(prefs.sidebar_width_rem, 15.0);
        assert_eq!(prefs.work_pane_width_rem, 31.0);
        assert_eq!(prefs.matching_preset(), Some(LayoutPreset::Zen));
    }

    #[test]
    fn a_manual_toggle_reports_the_preset_it_actually_matches() {
        let mut prefs = LayoutPrefs::default();
        prefs.apply_preset(LayoutPreset::Split);
        prefs.work_pane_open = false;
        assert_eq!(prefs.matching_preset(), Some(LayoutPreset::Focus));
    }
}

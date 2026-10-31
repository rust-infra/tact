//! Offscreen visual capture harness for the real Tact Desktop shell.
//!
//! The harness renders the production `DesktopRoot` through GPUI's real
//! platform renderer into an off-screen window, so it produces the same pixels
//! the compositor would show without needing Screen Recording permission. It is
//! used by `examples/ui_capture.rs` to walk every surface and write PNGs for
//! visual review.

use std::{
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, OnceLock},
};

use anyhow::{Context as _, Result};
use gpui::{AnyWindowHandle, AppContext as _, Entity, VisualTestAppContext, px, size};
use gpui_component::Root;
use tact_catalog::SettingId;
use tact_desktop_core::{
    Appearance, DesktopConfig, DesktopController, DesktopSnapshot, ExtensionsTab, InspectorTab,
    SessionPhase,
};

use crate::root::{DesktopRoot, ReaderKind};

const WINDOW_WIDTH: f32 = 1280.0;
const WINDOW_HEIGHT: f32 = 840.0;
const NARROW_WIDTH: f32 = 900.0;
const NARROW_HEIGHT: f32 = 700.0;

/// Tact configuration is process-global; a second harness in the same process
/// must not reinstall it.
static PROCESS_CONFIG: OnceLock<Result<(), String>> = OnceLock::new();

pub struct Harness {
    pub cx: VisualTestAppContext,
    root: Option<Entity<DesktopRoot>>,
    pub window: AnyWindowHandle,
    pub controller: DesktopController,
    data_dir: PathBuf,
}

impl Harness {
    pub fn open(appearance: Appearance, text_scale: f32) -> Result<Self> {
        Self::open_sized(appearance, text_scale, WINDOW_WIDTH, WINDOW_HEIGHT)
    }

    pub fn open_sized(
        appearance: Appearance,
        text_scale: f32,
        width: f32,
        height: f32,
    ) -> Result<Self> {
        let platform = gpui_platform::current_platform(true);
        let mut cx = VisualTestAppContext::with_asset_source(
            platform,
            Arc::new(gpui_component_assets::Assets),
        );

        let data_dir = std::env::temp_dir().join(format!(
            "tact-ui-capture-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&data_dir).context("create capture data dir")?;
        let mut config = DesktopConfig::load(data_dir.clone())
            .map_err(|error| anyhow::anyhow!("load desktop config: {error:?}"))?;
        config.appearance = appearance;
        config.text_scale = text_scale;
        if config.initial_project.is_none() {
            config.initial_project = std::env::current_dir().ok();
        }
        let installed = PROCESS_CONFIG.get_or_init(|| {
            config
                .install_process_config()
                .map_err(|error| format!("install desktop config: {error:?}"))
        });
        if let Err(error) = installed {
            anyhow::bail!("{error}");
        }
        let (controller, _events) = DesktopController::start(config)
            .map_err(|error| anyhow::anyhow!("start desktop controller: {error:?}"))?;
        let shutdown = controller.clone();

        // Screenshots must show settled surfaces, so scripted entrances are
        // skipped the same way the reduced-motion setting skips them.
        cx.update(|cx| cx.set_reduce_motion(true));

        let root_slot: Rc<std::cell::RefCell<Option<Entity<DesktopRoot>>>> = Default::default();
        let root_slot_for_window = root_slot.clone();
        let window = cx
            .open_offscreen_window(size(px(width), px(height)), move |window, cx| {
                gpui_ai::init(cx);
                crate::theme::apply(appearance, Some(window), cx);
                let root = cx.new(|cx| DesktopRoot::new_for_test(shutdown, window, cx));
                *root_slot_for_window.borrow_mut() = Some(root.clone());
                cx.new(|cx| Root::new(root, window, cx))
            })
            .context("open offscreen window")?;
        let window: AnyWindowHandle = window.into();
        let root = root_slot
            .borrow_mut()
            .take()
            .context("root entity was not created")?;

        Ok(Self {
            cx,
            root: Some(root),
            window,
            controller,
            data_dir,
        })
    }

    pub fn draw(&mut self) {
        // Let scripted transitions (tab fades, dialog entrances) reach their
        // final frame before the pixels are captured.
        self.cx.advance_clock(std::time::Duration::from_millis(240));
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.draw(cx).clear(cx);
            })
            .expect("draw capture window");
        self.cx.advance_clock(std::time::Duration::from_millis(240));
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.draw(cx).clear(cx);
            })
            .expect("draw capture window");
        self.cx.run_until_parked();
    }

    pub fn snapshot(&self) -> DesktopSnapshot {
        self.cx.read(|cx| {
            self.root
                .as_ref()
                .expect("capture root")
                .read(cx)
                .snapshot()
                .as_ref()
                .clone()
        })
    }

    pub fn set_snapshot(&mut self, snapshot: DesktopSnapshot) {
        self.cx.update(|cx| {
            self.root
                .as_ref()
                .expect("capture root")
                .update(cx, |root, cx| {
                    root.set_snapshot_for_capture(Arc::new(snapshot), cx);
                });
        });
    }

    /// Runs `f` with the live root, window, and app context.
    pub(crate) fn with_root<R>(
        &mut self,
        f: impl FnOnce(&mut DesktopRoot, &mut gpui::Window, &mut gpui::Context<DesktopRoot>) -> R,
    ) -> R {
        let root = self.root.clone().expect("capture root");
        self.cx
            .update_window(self.window, |_, window, cx| {
                root.update(cx, |root, cx| f(root, window, cx))
            })
            .expect("update root")
    }

    pub fn set_appearance(&mut self, appearance: Appearance) {
        self.cx
            .update_window(self.window, |_, window, cx| {
                crate::theme::apply(appearance, Some(window), cx);
            })
            .expect("apply appearance");
        let mut snapshot = self.snapshot();
        snapshot.appearance = appearance;
        self.set_snapshot(snapshot);
    }

    pub fn simulate_keystrokes(&mut self, keys: &str) {
        self.cx.simulate_keystrokes(self.window, keys);
        self.draw();
    }

    pub fn simulate_input(&mut self, text: &str) {
        self.cx.simulate_input(self.window, text);
        self.draw();
    }

    pub fn click(&mut self, x: f32, y: f32) {
        self.cx.simulate_click(
            self.window,
            gpui::point(px(x), px(y)),
            gpui::Modifiers::default(),
        );
        self.draw();
    }

    pub fn capture(&mut self, out: &Path, name: &str) -> Result<PathBuf> {
        self.draw();
        let image = self
            .cx
            .capture_screenshot(self.window)
            .with_context(|| format!("capture {name}"))?;
        let path = out.join(format!("{name}.png"));
        image
            .save(&path)
            .with_context(|| format!("save {path:?}"))?;
        Ok(path)
    }

    pub fn resize(&mut self, width: f32, height: f32) {
        self.cx
            .update_window(self.window, |_, window, _| {
                window.resize(size(px(width), px(height)));
            })
            .expect("resize capture window");
        self.draw();
    }

    /// Stops the controller, removes the off-screen window, and skips the
    /// visual-test leak assertion at process exit.
    pub fn finish(mut self) {
        let _ = self.controller.shutdown_blocking();
        let _ = std::fs::remove_dir_all(&self.data_dir);
        let window = self.window;
        let _ = self
            .cx
            .update_window(window, |_, window, _| window.remove_window());
        self.cx.run_until_parked();
        // The harness owns the only remaining entity handles; the process is
        // exiting, so let them go with the app instead of tripping the
        // visual-test leak detector during teardown.
        std::mem::forget(self);
    }
}

/// Walks every shell surface and writes one PNG per surface.
pub fn capture_all(out: &Path) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(out)?;
    let mut harness = Harness::open(Appearance::Light, 1.0)?;
    let mut shots = Vec::new();
    let mut shoot = |harness: &mut Harness, name: &str| -> Result<()> {
        shots.push(harness.capture(out, name)?);
        Ok(())
    };

    eprintln!("capture: opened harness");
    harness.draw();
    shoot(&mut harness, "01-shell-light")?;
    eprintln!("capture: shell light");

    harness.set_appearance(Appearance::Dark);
    shoot(&mut harness, "02-shell-dark")?;
    harness.set_appearance(Appearance::Light);

    harness.with_root(|root, window, cx| root.capture_open_command_center(window, cx));
    shoot(&mut harness, "03-command-center")?;
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    harness.with_root(|root, window, cx| root.capture_open_slash(window, cx));
    shoot(&mut harness, "04-slash-overlay")?;
    harness.with_root(|root, window, cx| root.capture_reset_composer(window, cx));
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    harness.with_root(|root, _, _| root.capture_open_settings());
    shoot(&mut harness, "05-settings")?;
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    harness.with_root(|root, _, _| root.capture_open_session_manager());
    shoot(&mut harness, "06-session-manager")?;
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    for (name, tab) in [
        ("07-extensions-mcp", ExtensionsTab::Mcp),
        ("08-extensions-skills", ExtensionsTab::Skills),
        ("09-extensions-plugins", ExtensionsTab::Plugins),
        ("10-extensions-hooks", ExtensionsTab::Hooks),
    ] {
        harness.with_root(|root, _, _| root.capture_open_extensions(tab));
        shoot(&mut harness, name)?;
        harness.with_root(|root, _, cx| root.capture_close_overlay(cx));
    }

    harness.with_root(|root, _, _| root.capture_open_background());
    shoot(&mut harness, "11-background")?;
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    for (name, tab) in [
        ("12-inspector-run", InspectorTab::Run),
        ("13-inspector-goals", InspectorTab::Goals),
        ("14-inspector-subagents", InspectorTab::Subagents),
        ("15-inspector-context", InspectorTab::Context),
    ] {
        harness.with_root(|root, _, cx| root.capture_set_inspector(tab, cx));
        shoot(&mut harness, name)?;
    }
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    for (name, kind) in [
        ("16-reader-system-prompt", ReaderKind::SystemPrompt),
        ("17-reader-stats", ReaderKind::Stats),
        ("18-reader-history", ReaderKind::History),
        ("19-reader-tasks-dag", ReaderKind::TasksDag),
        ("20-reader-balance", ReaderKind::Balance),
    ] {
        harness.with_root(|root, _, _| root.capture_open_reader(kind));
        shoot(&mut harness, name)?;
        harness.with_root(|root, _, cx| root.capture_close_overlay(cx));
    }

    harness.with_root(|root, _, cx| root.capture_open_model_picker(SettingId::Model, cx));
    shoot(&mut harness, "21-model-picker")?;
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    harness.with_root(|root, window, cx| {
        root.capture_open_setting_form(SettingId::BaseUrl, window, cx);
    });
    shoot(&mut harness, "22-command-form")?;
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    // Seed representative runtime data so every transcript row kind and every
    // inspector panel renders with real content rather than an empty state.
    let mut snapshot = harness.snapshot();
    snapshot.transcript = tact_desktop_core::fixtures::demo_transcript();
    snapshot.inspector = tact_desktop_core::fixtures::demo_inspector();
    snapshot.failure = None;
    snapshot.tail_following = false;
    snapshot.session.phase = SessionPhase::Running;
    harness.set_snapshot(snapshot);
    harness.with_root(|root, _, cx| root.capture_scroll_to(0, cx));
    shoot(&mut harness, "26a-transcript-top")?;
    harness.with_root(|root, _, cx| root.capture_scroll_to(12, cx));
    shoot(&mut harness, "26b-transcript-bottom")?;

    harness.set_appearance(Appearance::Dark);
    harness.with_root(|root, _, cx| root.capture_scroll_to(0, cx));
    shoot(&mut harness, "27-transcript-dark")?;
    harness.set_appearance(Appearance::Light);

    for (name, tab) in [
        ("28-inspector-run-populated", InspectorTab::Run),
        ("29-inspector-goals-populated", InspectorTab::Goals),
        ("30-inspector-subagents-populated", InspectorTab::Subagents),
        ("31-inspector-context-populated", InspectorTab::Context),
    ] {
        harness.with_root(|root, _, cx| root.capture_set_inspector(tab, cx));
        shoot(&mut harness, name)?;
    }
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    harness.with_root(|root, _, _| root.capture_open_background());
    shoot(&mut harness, "32-background-populated")?;
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    harness.with_root(|root, _, _| {
        root.capture_open_gate(tact_desktop_core::fixtures::demo_gate());
    });
    shoot(&mut harness, "33-gate")?;
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    harness.with_root(|root, _, _| root.capture_open_reader(ReaderKind::TasksDag));
    shoot(&mut harness, "38-reader-tasks-dag-populated")?;
    harness.with_root(|root, _, cx| root.capture_close_overlay(cx));

    harness.simulate_keystrokes("tab");
    shoot(&mut harness, "39-focus-visible")?;

    let mut snapshot = harness.snapshot();
    snapshot.text_scale = 1.5;
    harness.set_snapshot(snapshot);
    shoot(&mut harness, "24-text-scale-150")?;

    let mut snapshot = harness.snapshot();
    snapshot.text_scale = 1.0;
    snapshot.language = tact_catalog::CatalogLocale::Zh;
    harness.set_snapshot(snapshot);
    shoot(&mut harness, "25-chinese")?;

    harness.finish();

    // A separate off-screen window is the only reliable way to capture a
    // different viewport size; resizing the first window does not reflow the
    // rendered scene in the visual test platform.
    let mut narrow = Harness::open_sized(Appearance::Light, 1.0, NARROW_WIDTH, NARROW_HEIGHT)?;
    shoot(&mut narrow, "23-narrow-window")?;
    narrow.finish();

    // A tall off-screen window renders the whole transcript without relying on
    // the virtualized list's scroll position.
    let mut tall = Harness::open_sized(Appearance::Light, 1.0, 1280.0, 2600.0)?;
    let mut snapshot = tall.snapshot();
    snapshot.transcript = tact_desktop_core::fixtures::demo_transcript();
    snapshot.inspector = tact_desktop_core::fixtures::demo_inspector();
    snapshot.failure = None;
    snapshot.session.phase = SessionPhase::Running;
    tall.set_snapshot(snapshot);
    shoot(&mut tall, "34-transcript-bottom")?;
    tall.with_root(|root, _, cx| root.capture_scroll_to(0, cx));
    shoot(&mut tall, "35-transcript-top")?;
    tall.with_root(|root, _, cx| root.capture_scroll_to(7, cx));
    shoot(&mut tall, "36-transcript-middle")?;
    tall.set_appearance(Appearance::Dark);
    shoot(&mut tall, "37-transcript-middle-dark")?;
    tall.finish();

    Ok(shots)
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.controller.shutdown_blocking();
        let _ = std::fs::remove_dir_all(&self.data_dir);
        // Release the root entity and the window before the app cell drops so
        // the visual-test leak detector stays quiet on error paths.
        let _ = self.root.take();
        let window = self.window;
        let _ = self
            .cx
            .update_window(window, |_, window, _| window.remove_window());
        self.cx.run_until_parked();
    }
}

//! Offscreen visual capture harness for the real Tact Desktop shell.
//!
//! The harness renders the production `DesktopRoot` through GPUI's real
//! platform renderer into an off-screen window, so it produces the same pixels
//! the compositor would show without needing Screen Recording permission. It is
//! used by `examples/ui_capture.rs` to walk every surface and write PNGs for
//! visual review.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context as _, Result};
use gpui::{AnyWindowHandle, App, AppContext as _, Entity, VisualTestAppContext, px, size};
use gpui_component::Root;
use tact_desktop_core::{Appearance, DesktopConfig, DesktopController, DesktopSnapshot};

use crate::root::DesktopRoot;

const WINDOW_WIDTH: f32 = 1280.0;
const WINDOW_HEIGHT: f32 = 840.0;

pub struct Harness {
    pub cx: VisualTestAppContext,
    pub root: Entity<DesktopRoot>,
    pub window: AnyWindowHandle,
    pub controller: DesktopController,
    data_dir: PathBuf,
}

impl Harness {
    pub fn open(appearance: Appearance, text_scale: f32) -> Result<Self> {
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
        config
            .install_process_config()
            .map_err(|error| anyhow::anyhow!("install desktop config: {error:?}"))?;
        let (controller, _events) = DesktopController::start(config)
            .map_err(|error| anyhow::anyhow!("start desktop controller: {error:?}"))?;
        let shutdown = controller.clone();

        let root_slot: std::rc::Rc<std::cell::RefCell<Option<Entity<DesktopRoot>>>> =
            Default::default();
        let root_slot_for_window = root_slot.clone();
        let window = cx
            .open_offscreen_window(
                size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)),
                move |window, cx| {
                    gpui_ai::init(cx);
                    crate::theme::apply(appearance, Some(window), cx);
                    let root = cx.new(|cx| DesktopRoot::new_for_test(shutdown, window, cx));
                    *root_slot_for_window.borrow_mut() = Some(root.clone());
                    cx.new(|cx| Root::new(root, window, cx))
                },
            )
            .context("open offscreen window")?;
        let window: AnyWindowHandle = window.into();
        let root = root_slot
            .borrow_mut()
            .take()
            .context("root entity was not created")?;

        Ok(Self {
            cx,
            root,
            window,
            controller,
            data_dir,
        })
    }

    pub fn draw(&mut self) {
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.draw(cx).clear(cx);
            })
            .expect("draw capture window");
        self.cx.run_until_parked();
    }

    pub fn snapshot(&self) -> DesktopSnapshot {
        self.cx
            .read(|cx| self.root.read(cx).snapshot().as_ref().clone())
    }

    pub fn set_snapshot(&mut self, snapshot: DesktopSnapshot) {
        self.cx.update(|cx| {
            self.root.update(cx, |root, cx| {
                root.set_snapshot_for_capture(Arc::new(snapshot), cx);
            });
        });
    }

    pub fn update_root<R>(&mut self, f: impl FnOnce(&mut DesktopRoot, &mut App) -> R) -> R {
        self.cx
            .update(|cx| self.root.update(cx, |root, cx| f(root, cx)))
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
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.controller.shutdown_blocking();
        let _ = std::fs::remove_dir_all(&self.data_dir);
    }
}

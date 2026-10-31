use std::{
    borrow::Cow,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use gpui::{App, AppContext as _, Bounds, WindowBounds, px, size};
use gpui_component::{Root, TitleBar};
use tact_desktop_core::{DesktopConfig, DesktopController, DesktopStartError};

use crate::root::DesktopRoot;

#[derive(Default)]
struct ShutdownState {
    started: AtomicBool,
    result: Mutex<Option<Result<(), String>>>,
}

impl ShutdownState {
    fn shutdown(&self, controller: &DesktopController) -> Result<(), String> {
        let mut result = self
            .result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(result) = result.as_ref() {
            return result.clone();
        }
        if self.started.swap(true, Ordering::SeqCst) {
            return Err("Tact shutdown is already in progress".into());
        }
        let controller = controller.clone();
        let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let _ = done_tx.send(controller.shutdown_blocking());
        });
        let next = match done_rx.recv_timeout(std::time::Duration::from_secs(20)) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(format!("Tact shutdown failed: {error:?}")),
            Err(_) => Err("Tact shutdown timed out while quitting".into()),
        };
        if let Err(error) = &next {
            eprintln!("{error}");
        }
        *result = Some(next.clone());
        next
    }

    fn result(&self) -> Option<Result<(), String>> {
        self.result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

pub fn run() -> Result<(), DesktopStartError> {
    run_inner(false)
}

pub fn run_smoke() -> Result<(), DesktopStartError> {
    run_inner(true)
}

fn run_inner(smoke: bool) -> Result<(), DesktopStartError> {
    let data_dir = tact_desktop_core::app_support_dir()
        .map_err(|error| DesktopStartError::Config(error.to_string()))?;
    let mut config = DesktopConfig::load(data_dir)?;
    if config.initial_project.is_none() {
        config.initial_project = std::env::current_dir().ok();
    }
    config.install_process_config()?;
    tact_desktop_core::install_exit_listener()?;
    let (controller, events) = DesktopController::start(config)?;
    let shutdown_state = Arc::new(ShutdownState::default());
    let shutdown_for_result = shutdown_state.clone();
    let shutdown_for_quit = shutdown_state.clone();

    gpui_platform::application()
        .with_assets(gpui_component_assets::Assets)
        .run(move |cx: &mut App| {
            gpui_ai::init(cx);
            if let Err(error) = cx.text_system().add_fonts(vec![
                Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf").as_slice()),
                Cow::Borrowed(include_bytes!("../assets/fonts/Lilex-Regular.ttf").as_slice()),
            ]) {
                eprintln!("failed to register Tact Desktop fonts: {error:#}");
            }
            let initial = controller.snapshot();
            let close_language = initial.language;
            let system_reduce_motion =
                cx.reduce_motion() || crate::platform::system_reduced_motion();
            cx.set_reduce_motion(initial.motion.reduce(system_reduce_motion));
            cx.bind_keys(crate::keymap::bindings());
            let shutdown_controller = controller.clone();
            let quit_subscription = cx.on_app_quit(move |_| {
                let _ = shutdown_for_quit.shutdown(&shutdown_controller);
                async {}
            });
            std::mem::forget(quit_subscription);
            crate::theme::apply(initial.appearance, None, cx);
            let controller_for_root = controller.clone();
            let controller_for_close = controller.clone();
            let shutdown_for_close = shutdown_state.clone();
            let bounds = Bounds::centered(None, size(px(1280.), px(840.)), cx);
            let mut window_options = TitleBar::window_options();
            if let Some(titlebar) = &mut window_options.titlebar {
                titlebar.appears_transparent = !crate::theme::reduced_transparency();
            }
            #[cfg(target_os = "linux")]
            {
                window_options.window_decorations = Some(gpui::WindowDecorations::Client);
            }
            window_options.window_bounds = Some(WindowBounds::Windowed(bounds));
            cx.open_window(window_options, move |window, cx| {
                crate::theme::apply(initial.appearance, Some(window), cx);
                window.on_window_should_close(cx, move |_, cx| {
                    match shutdown_for_close.shutdown(&controller_for_close) {
                        Ok(()) => true,
                        Err(error) => {
                            cx.show_system_notification(gpui::SystemNotification {
                                tag: "tact-desktop-close-blocked".into(),
                                title: match close_language {
                                    tact_catalog::CatalogLocale::En => "Tact is still stopping",
                                    tact_catalog::CatalogLocale::Zh => "Tact 正在停止",
                                }
                                .into(),
                                body: error.into(),
                                actions: Vec::new(),
                            });
                            false
                        }
                    }
                });
                let root = cx.new(|cx| DesktopRoot::new(controller_for_root, events, window, cx));
                cx.new(|cx| Root::new(root, window, cx))
            })
            .expect("failed to open Tact Desktop window");
            cx.activate(true);
            if smoke {
                cx.spawn(async move |cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(250))
                        .await;
                    cx.update(|cx| cx.quit());
                })
                .detach();
            }
        });
    match shutdown_for_result.result() {
        Some(Err(error)) => Err(DesktopStartError::Runtime(error)),
        Some(Ok(())) | None => Ok(()),
    }
}

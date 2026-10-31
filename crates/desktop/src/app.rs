use std::{
    borrow::Cow,
    sync::{Arc, Condvar, Mutex},
};

use gpui::{App, AppContext as _, Bounds, WindowBounds, px, size};
use gpui_component::{Root, TitleBar};
use tact_desktop_core::{DesktopConfig, DesktopController, DesktopStartError};

use crate::root::DesktopRoot;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ShutdownPhase {
    #[default]
    Idle,
    Running,
    Complete,
}

#[derive(Default)]
struct ShutdownInner {
    phase: ShutdownPhase,
    result: Option<Result<(), String>>,
}

#[derive(Default)]
struct ShutdownState {
    inner: Mutex<ShutdownInner>,
    finished: Condvar,
}

impl ShutdownState {
    fn shutdown(&self, controller: &DesktopController) -> Result<(), String> {
        self.shutdown_with(|| {
            controller
                .shutdown_blocking()
                .map_err(|error| format!("Tact shutdown failed: {error:?}"))
        })
    }

    fn shutdown_with(&self, shutdown: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            match (inner.phase, inner.result.as_ref()) {
                (ShutdownPhase::Complete, Some(Ok(()))) => return Ok(()),
                // A failed attempt is retryable. Only a successful terminal
                // shutdown may be replayed without invoking the controller.
                (ShutdownPhase::Complete, Some(Err(_))) => {
                    inner.phase = ShutdownPhase::Idle;
                    inner.result = None;
                }
                (ShutdownPhase::Running, _) => {
                    inner = self
                        .finished
                        .wait(inner)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                (ShutdownPhase::Idle, _) => {
                    inner.phase = ShutdownPhase::Running;
                    inner.result = None;
                    break;
                }
                (ShutdownPhase::Complete, None) => {
                    inner.phase = ShutdownPhase::Idle;
                }
            }
        }
        drop(inner);

        let next = shutdown();
        if let Err(error) = &next {
            eprintln!("{error}");
        }
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.phase = ShutdownPhase::Complete;
        inner.result = Some(next.clone());
        self.finished.notify_all();
        next
    }

    fn is_running(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .phase
            == ShutdownPhase::Running
    }

    fn is_succeeded(&self) -> bool {
        matches!(
            self.inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .result
                .as_ref(),
            Some(Ok(()))
        )
    }

    fn result(&self) -> Option<Result<(), String>> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .result
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
            let quit_subscription = cx.on_app_quit(move |cx| {
                let shutdown = shutdown_for_quit.clone();
                let controller = shutdown_controller.clone();
                let background = cx.background_executor().clone();
                async move {
                    background
                        .spawn(async move { shutdown.shutdown(&controller) })
                        .await
                        .ok();
                }
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
                    if shutdown_for_close.is_succeeded() {
                        return true;
                    }
                    if !shutdown_for_close.is_running() {
                        let controller = controller_for_close.clone();
                        let shutdown = shutdown_for_close.clone();
                        let close_language = close_language;
                        let background = cx.background_executor().clone();
                        cx.spawn(async move |cx| {
                            match background
                                .spawn(async move { shutdown.shutdown(&controller) })
                                .await
                            {
                                Ok(()) => cx.update(|cx| cx.quit()),
                                Err(error) => cx.update(|cx| {
                                    cx.show_system_notification(gpui::SystemNotification {
                                        tag: "tact-desktop-close-blocked".into(),
                                        title: match close_language {
                                            tact_catalog::CatalogLocale::En => {
                                                "Tact could not close"
                                            }
                                            tact_catalog::CatalogLocale::Zh => "Tact 无法关闭",
                                        }
                                        .into(),
                                        body: error.into(),
                                        actions: Vec::new(),
                                    });
                                }),
                            }
                        })
                        .detach();
                    }
                    cx.show_system_notification(gpui::SystemNotification {
                        tag: "tact-desktop-stopping".into(),
                        title: match close_language {
                            tact_catalog::CatalogLocale::En => "Tact is stopping",
                            tact_catalog::CatalogLocale::Zh => "Tact 正在停止",
                        }
                        .into(),
                        body: crate::i18n::text(close_language, crate::i18n::TextKey::Stopping)
                            .into(),
                        actions: Vec::new(),
                    });
                    false
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Barrier,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Duration;

    #[test]
    fn failed_shutdown_is_retryable() {
        let state = ShutdownState::default();
        assert_eq!(
            state.shutdown_with(|| Err("first attempt failed".into())),
            Err("first attempt failed".into())
        );
        assert_eq!(state.shutdown_with(|| Ok(())), Ok(()));
    }

    #[test]
    fn successful_shutdown_is_cached_for_later_requests() {
        let state = ShutdownState::default();
        assert_eq!(state.shutdown_with(|| Ok(())), Ok(()));
        assert_eq!(
            state.shutdown_with(|| panic!("successful shutdown must not run twice")),
            Ok(())
        );
    }

    #[test]
    fn concurrent_shutdown_requests_share_one_run() {
        let state = Arc::new(ShutdownState::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for _ in 0..2 {
            let state = state.clone();
            let calls = calls.clone();
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                state.shutdown_with(|| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(25));
                    Ok(())
                })
            }));
        }
        barrier.wait();
        for worker in workers {
            assert_eq!(worker.join().unwrap(), Ok(()));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

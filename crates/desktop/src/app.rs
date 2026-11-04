use std::sync::{Arc, Mutex, OnceLock};

use nsh_core::prelude::*;
use tact_desktop_core::{
    DesktopConfig, DesktopController, DesktopEffect, DesktopEvent, DesktopEvents,
    DesktopSnapshot, DesktopStartError, ReaderRequest,
};

/// The one controller of this process and the events it publishes.
pub struct Runtime {
    controller: DesktopController,
    events: DesktopEvents,
    initial: Arc<DesktopSnapshot>,
}

static RUNTIME: OnceLock<Runtime> = OnceLock::new();
static START: Mutex<()> = Mutex::new(());

/// Loads the desktop configuration and starts the controller before the
/// window opens, so a startup failure is reported instead of an empty window.
pub fn start() -> Result<(), DesktopStartError> {
    let _start = START.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if RUNTIME.get().is_some() {
        return Ok(());
    }
    let data_dir = tact_desktop_core::app_support_dir()
        .map_err(|error| DesktopStartError::Config(error.to_string()))?;
    let mut config = DesktopConfig::load(data_dir)?;
    if config.initial_project.is_none() {
        config.initial_project = std::env::current_dir().ok();
    }
    config.install_process_config()?;
    if !config.appearance_explicit
        && let Some(theme) = tact_desktop_core::configured_theme()
    {
        config.appearance = tact_desktop_core::appearance_from_theme(&theme);
    }
    tact_desktop_core::install_exit_listener()?;
    let (controller, events) = DesktopController::start(config)?;
    let initial = controller.snapshot();
    let _ = RUNTIME.set(Runtime {
        controller,
        events,
        initial,
    });
    Ok(())
}

/// Stops the controller after the window closes.
pub fn shutdown() {
    if let Some(runtime) = RUNTIME.get()
        && let Err(error) = runtime.controller.shutdown_blocking()
    {
        eprintln!("Tact shutdown failed: {error:?}");
    }
}

fn runtime() -> &'static Runtime {
    RUNTIME.get().expect("Tact Desktop runtime started")
}

/// The snapshot a commit carries. Compared by identity so a commit never
/// deep-compares two full view models on the engine thread.
#[derive(Clone)]
pub struct SnapshotRef(pub Arc<DesktopSnapshot>);

impl PartialEq for SnapshotRef {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// The voice worker's phase as the composer shows it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VoicePhase {
    #[default]
    Idle,
    Recording,
    Transcribing,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VoiceState {
    pub phase: VoicePhase,
    pub transcript: Option<String>,
    pub error: Option<String>,
}

/// A transient message the controller asked the shell to show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToastMessage {
    pub title: String,
    pub body: String,
}

/// The controller and the latest committed snapshot, available to every
/// component of the app.
#[derive(Clone)]
pub struct AppState {
    pub controller: DesktopController,
    pub snapshot: Signal<SnapshotRef>,
    pub reader: Signal<Option<ReaderRequest>>,
    pub voice: Signal<VoiceState>,
    pub toast: Signal<Option<ToastMessage>>,
}

/// Installs the controller bridge once, at the root component.
pub fn install() {
    if RUNTIME.get().is_none() {
        start().expect("Tact Desktop starts");
    }
    if use_context::<AppState>().is_some() {
        return;
    }
    let runtime = runtime();
    let snapshot = signal(SnapshotRef(runtime.initial.clone()));
    let reader = signal(None::<ReaderRequest>);
    let voice = signal(VoiceState::default());
    let toast = signal(None::<ToastMessage>);
    provide_context(AppState {
        controller: runtime.controller.clone(),
        snapshot,
        reader,
        voice,
        toast,
    });
    let events = &runtime.events;
    spawn(async move {
        while let Ok(event) = events.recv().await {
            match event {
                DesktopEvent::Committed(commit) => snapshot.set(SnapshotRef(commit.snapshot)),
                DesktopEvent::Effect(DesktopEffect::OpenReader(request)) => {
                    reader.set(Some(request));
                }
                DesktopEvent::Effect(DesktopEffect::CopyText(text)) => {
                    set_clipboard(text);
                }
                DesktopEvent::Effect(DesktopEffect::OpenExternal(url)) => {
                    let _ = open_url(url.to_string());
                }
                DesktopEvent::Effect(DesktopEffect::OpenPath(path)) => {
                    let _ = open_url(format!("file://{}", path.display()));
                }
                DesktopEvent::Effect(DesktopEffect::WindowCloseRequested) => {
                    nsh_core::element::desktop::close_window();
                }
                DesktopEvent::Effect(DesktopEffect::Notify { title, body }) => {
                    toast.set(Some(ToastMessage { title, body }));
                    set_timeout(std::time::Duration::from_secs(4), move || {
                        toast.set(None);
                    });
                }
                DesktopEvent::Fatal(failure) => {
                    toast.set(Some(ToastMessage {
                        title: "Tact stopped".to_owned(),
                        body: failure.message.to_string(),
                    }));
                }
                DesktopEvent::Voice(event) => {
                    use tact_desktop_core::VoiceEvent;
                    voice.update(|state| match event {
                        VoiceEvent::RecordingStarted => {
                            state.phase = VoicePhase::Recording;
                            state.error = None;
                        }
                        VoiceEvent::RecordingStopped | VoiceEvent::Transcribing => {
                            state.phase = VoicePhase::Transcribing;
                        }
                        VoiceEvent::Transcript(text) => {
                            state.phase = VoicePhase::Idle;
                            state.transcript = Some(text);
                        }
                        VoiceEvent::Error(message) => {
                            state.phase = VoicePhase::Idle;
                            state.error = Some(message);
                        }
                        VoiceEvent::Cancelled => {
                            state.phase = VoicePhase::Idle;
                        }
                    });
                }
                _ => {}
            }
        }
    });
}

/// The app state of the calling component tree.
pub fn use_app() -> AppState {
    use_context::<AppState>().expect("Tact Desktop installs its app state at the root")
}

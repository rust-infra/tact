use std::time::Instant;

use ratatui::layout::Rect;
use tact_protocol::{VoiceCommand, VoiceEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VoicePhase {
    Disabled,
    Idle,
    Recording { started_at: Instant },
    Transcribing,
}

pub(crate) struct VoiceState {
    pub(crate) phase: VoicePhase,
    pub(crate) button_area: Rect,
    pub(crate) missing_api_key: bool,
    pub(crate) worker: Option<std::sync::Arc<dyn crate::ports::VoicePort>>,
    /// The events the worker produced, drained each tick.
    ///
    /// Held here rather than behind the port because a receiver needs `&mut` and
    /// the port is shared; the view is its single consumer by construction.
    pub(crate) event_rx: Option<tokio::sync::mpsc::UnboundedReceiver<tact_protocol::VoiceEvent>>,
}

impl VoiceState {
    pub(crate) fn disabled() -> Self {
        Self {
            phase: VoicePhase::Disabled,
            button_area: Rect::default(),
            missing_api_key: false,
            worker: None,
            event_rx: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn idle_visible_for_tests() -> Self {
        Self {
            phase: VoicePhase::Idle,
            button_area: Rect::default(),
            missing_api_key: false,
            worker: None,
            event_rx: None,
        }
    }

    pub(crate) fn enabled(
        worker: std::sync::Arc<dyn crate::ports::VoicePort>,
        event_rx: tokio::sync::mpsc::UnboundedReceiver<tact_protocol::VoiceEvent>,
        missing_api_key: bool,
    ) -> Self {
        Self {
            phase: VoicePhase::Idle,
            button_area: Rect::default(),
            missing_api_key,
            worker: Some(worker),
            event_rx: Some(event_rx),
        }
    }

    pub(crate) fn set_button_area(&mut self, area: Rect) {
        self.button_area = area;
    }

    pub(crate) fn is_active(&self) -> bool {
        matches!(
            self.phase,
            VoicePhase::Recording { .. } | VoicePhase::Transcribing
        )
    }

    pub(crate) fn try_start(&mut self) -> VoiceStartResult {
        if !matches!(self.phase, VoicePhase::Idle) {
            return VoiceStartResult::Ignored;
        }
        if self.missing_api_key {
            return VoiceStartResult::MissingApiKey;
        }
        if let Some(worker) = &self.worker {
            worker.send(VoiceCommand::Start);
            VoiceStartResult::Started
        } else {
            VoiceStartResult::Ignored
        }
    }

    pub(crate) fn stop(&mut self) {
        if matches!(self.phase, VoicePhase::Recording { .. })
            && let Some(worker) = &self.worker
        {
            worker.send(VoiceCommand::Stop);
        }
    }

    pub(crate) fn cancel(&mut self) {
        if self.is_active()
            && let Some(worker) = &self.worker
        {
            worker.send(VoiceCommand::Cancel);
            self.phase = VoicePhase::Idle;
        }
    }

    pub(crate) fn apply_event(&mut self, event: VoiceEvent) -> VoiceEventOutcome {
        match event {
            VoiceEvent::RecordingStarted => {
                self.phase = VoicePhase::Recording {
                    started_at: Instant::now(),
                };
                VoiceEventOutcome::Repaint
            }
            VoiceEvent::RecordingStopped => VoiceEventOutcome::Repaint,
            VoiceEvent::Transcribing => {
                self.phase = VoicePhase::Transcribing;
                VoiceEventOutcome::Repaint
            }
            VoiceEvent::Transcript(text) => {
                self.phase = VoicePhase::Idle;
                VoiceEventOutcome::InsertTranscript(text)
            }
            VoiceEvent::Error(message) => {
                self.phase = VoicePhase::Idle;
                VoiceEventOutcome::FlashError(message)
            }
            VoiceEvent::Cancelled => {
                self.phase = VoicePhase::Idle;
                VoiceEventOutcome::Repaint
            }
        }
    }

    pub(crate) async fn shutdown(&mut self) {
        if let Some(worker) = self.worker.take() {
            // Ask the worker to stop through the port; awaiting its task is the
            // adapter's job, not the view's.
            worker.send(VoiceCommand::Shutdown);
        }
        self.phase = VoicePhase::Disabled;
    }
}

pub(crate) enum VoiceStartResult {
    Ignored,
    Started,
    MissingApiKey,
}

pub(crate) enum VoiceEventOutcome {
    Repaint,
    InsertTranscript(String),
    FlashError(String),
}

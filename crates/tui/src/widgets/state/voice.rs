use std::time::Instant;

use ratatui::layout::Rect;
use tact::voice::{VoiceCommand, VoiceEvent, VoiceWorkerHandle};

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
    pub(crate) worker: Option<VoiceWorkerHandle>,
}

impl VoiceState {
    pub(crate) fn disabled() -> Self {
        Self {
            phase: VoicePhase::Disabled,
            button_area: Rect::default(),
            missing_api_key: false,
            worker: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn idle_visible_for_tests() -> Self {
        Self {
            phase: VoicePhase::Idle,
            button_area: Rect::default(),
            missing_api_key: false,
            worker: None,
        }
    }

    pub(crate) fn enabled(worker: VoiceWorkerHandle, missing_api_key: bool) -> Self {
        Self {
            phase: VoicePhase::Idle,
            button_area: Rect::default(),
            missing_api_key,
            worker: Some(worker),
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
            let _ = worker.command_tx.send(VoiceCommand::Start);
            VoiceStartResult::Started
        } else {
            VoiceStartResult::Ignored
        }
    }

    pub(crate) fn stop(&mut self) {
        if matches!(self.phase, VoicePhase::Recording { .. })
            && let Some(worker) = &self.worker
        {
            let _ = worker.command_tx.send(VoiceCommand::Stop);
        }
    }

    pub(crate) fn cancel(&mut self) {
        if self.is_active()
            && let Some(worker) = &self.worker
        {
            let _ = worker.command_tx.send(VoiceCommand::Cancel);
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
            worker.shutdown().await;
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

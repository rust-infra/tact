use std::time::Instant;

use crate::handlers::insert_transcript;
use crate::widgets::state::{App, VoiceEventOutcome, VoicePhase, VoiceStartResult};

impl App {
    pub(crate) fn drain_voice_events(&mut self) {
        let mut pending = Vec::new();
        if let Some(worker) = self.voice.worker.as_mut() {
            while let Ok(event) = worker.event_rx.try_recv() {
                pending.push(event);
            }
        }
        for event in pending {
            match self.voice.apply_event(event) {
                VoiceEventOutcome::InsertTranscript(text) => {
                    insert_transcript(&mut self.input, &mut self.input_cursor, &text);
                    self.dirty = true;
                }
                VoiceEventOutcome::FlashError(message) => {
                    self.flash_msg = Some((message, Instant::now()));
                    self.dirty = true;
                }
                VoiceEventOutcome::Repaint => self.dirty = true,
            }
        }
    }

    /// Toggle voice recording on/off via keyboard shortcut.
    /// - Idle → start recording
    /// - Recording → stop recording
    /// - Transcribing / Disabled → no-op
    pub(crate) fn toggle_voice_recording(&mut self) {
        match self.voice.phase {
            VoicePhase::Idle => match self.voice.try_start() {
                VoiceStartResult::MissingApiKey => {
                    self.flash_msg =
                        Some((self.msgs().voice_missing_config.to_string(), Instant::now()));
                    self.dirty = true;
                }
                VoiceStartResult::Started => self.dirty = true,
                VoiceStartResult::Ignored => {}
            },
            VoicePhase::Recording { .. } => {
                self.voice.stop();
                self.dirty = true;
            }
            VoicePhase::Transcribing | VoicePhase::Disabled => {}
        }
    }

    pub(crate) async fn shutdown_voice(&mut self) {
        self.voice.shutdown().await;
    }
}

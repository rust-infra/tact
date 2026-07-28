use std::time::Instant;

use crate::handlers::insert_transcript;
use crate::widgets::state::{App, VoiceEventOutcome};

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

    pub(crate) async fn shutdown_voice(&mut self) {
        self.voice.shutdown().await;
    }
}

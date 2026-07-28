use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::VoiceSettings;

pub mod recorder;
pub mod transcriber;
pub mod wav;

pub use transcriber::{OpenAiTranscriber, Transcriber};
pub use wav::encode_wav_mono_16k;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceCommand {
    Start,
    Stop,
    Cancel,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceEvent {
    RecordingStarted,
    RecordingStopped,
    Transcribing,
    Transcript(String),
    Error(String),
    Cancelled,
}

#[async_trait]
pub trait Recorder: Send + Sync {
    async fn record(
        &self,
        max_duration: Duration,
        stop: CancellationToken,
        cancel: CancellationToken,
    ) -> anyhow::Result<Option<Vec<i16>>>;
}

enum InternalMsg {
    RecordingFinished {
        result: anyhow::Result<Option<Vec<i16>>>,
        generation: u64,
    },
    TranscriptionFinished {
        result: anyhow::Result<String>,
        generation: u64,
    },
}

struct ActiveRecording {
    stop: CancellationToken,
    cancel: CancellationToken,
    generation: u64,
}

struct ActiveTranscription {
    cancel: CancellationToken,
    generation: u64,
}

pub struct VoiceWorkerHandle {
    pub command_tx: UnboundedSender<VoiceCommand>,
    pub event_rx: UnboundedReceiver<VoiceEvent>,
    join: Option<JoinHandle<()>>,
    shutdown_token: CancellationToken,
}

impl VoiceWorkerHandle {
    pub async fn shutdown(self) {
        self.shutdown_token.cancel();
        let _ = self.command_tx.send(VoiceCommand::Shutdown);
        if let Some(join) = self.join {
            let _ = join.await;
        }
    }

    /// Test-only stub with disconnected background task (no microphone or HTTP).
    #[doc(hidden)]
    pub fn stub_for_test(
        command_tx: UnboundedSender<VoiceCommand>,
        event_rx: UnboundedReceiver<VoiceEvent>,
    ) -> Self {
        Self {
            command_tx,
            event_rx,
            join: None,
            shutdown_token: CancellationToken::new(),
        }
    }
}

pub fn spawn_worker(settings: VoiceSettings) -> VoiceWorkerHandle {
    spawn_worker_with_components(
        settings.clone(),
        Arc::new(recorder::CpalRecorder),
        Arc::new(OpenAiTranscriber::new(settings)),
    )
}

pub fn spawn_worker_with_components(
    settings: VoiceSettings,
    recorder: Arc<dyn Recorder>,
    transcriber: Arc<dyn Transcriber>,
) -> VoiceWorkerHandle {
    let (command_tx, mut command_rx) = unbounded_channel();
    let (event_tx, event_rx) = unbounded_channel();
    let (internal_tx, mut internal_rx) = unbounded_channel();
    let shutdown_token = CancellationToken::new();
    let worker_shutdown = shutdown_token.clone();

    let join = tokio::spawn(async move {
        let mut recording: Option<ActiveRecording> = None;
        let mut transcription: Option<ActiveTranscription> = None;
        let mut generation: u64 = 0;

        loop {
            tokio::select! {
                () = worker_shutdown.cancelled() => break,
                cmd = command_rx.recv() => {
                    let Some(cmd) = cmd else { break };
                    match cmd {
                        VoiceCommand::Shutdown => break,
                        VoiceCommand::Start if recording.is_some() || transcription.is_some() => {}
                        VoiceCommand::Start => {
                            generation = generation.wrapping_add(1);
                            let recording_gen = generation;
                            let stop = CancellationToken::new();
                            let cancel = CancellationToken::new();
                            recording = Some(ActiveRecording {
                                stop: stop.clone(),
                                cancel: cancel.clone(),
                                generation: recording_gen,
                            });
                            let _ = event_tx.send(VoiceEvent::RecordingStarted);
                            let recorder = Arc::clone(&recorder);
                            let internal_tx = internal_tx.clone();
                            let max_duration = Duration::from_secs(settings.max_duration_secs.max(1));
                            tokio::spawn(async move {
                                let result = recorder.record(max_duration, stop, cancel).await;
                                let _ = internal_tx.send(InternalMsg::RecordingFinished {
                                    result,
                                    generation: recording_gen,
                                });
                            });
                        }
                        VoiceCommand::Stop => {
                            if let Some(active) = recording.as_ref() {
                                active.stop.cancel();
                            }
                        }
                        VoiceCommand::Cancel => {
                            if let Some(active) = recording.as_ref() {
                                active.cancel.cancel();
                            }
                            if let Some(active) = transcription.take() {
                                active.cancel.cancel();
                                let _ = event_tx.send(VoiceEvent::Cancelled);
                            }
                        }
                    }
                }
                msg = internal_rx.recv() => {
                    let Some(msg) = msg else { continue };
                    match msg {
                        InternalMsg::RecordingFinished {
                            result,
                            generation: recording_gen,
                        } => {
                            if recording
                                .as_ref()
                                .is_some_and(|r| r.generation != recording_gen)
                            {
                                continue;
                            }
                            recording = None;
                            match result {
                                Ok(None) => {
                                    let _ = event_tx.send(VoiceEvent::Cancelled);
                                }
                                Ok(Some(samples)) if samples.is_empty() => {}
                                Ok(Some(samples)) => {
                                    let _ = event_tx.send(VoiceEvent::RecordingStopped);
                                    let _ = event_tx.send(VoiceEvent::Transcribing);
                                    generation = generation.wrapping_add(1);
                                    let transcription_gen = generation;
                                    let cancel = CancellationToken::new();
                                    transcription = Some(ActiveTranscription {
                                        cancel: cancel.clone(),
                                        generation: transcription_gen,
                                    });
                                    let transcriber = Arc::clone(&transcriber);
                                    let internal_tx = internal_tx.clone();
                                    tokio::spawn(async move {
                                        let wav = match encode_wav_mono_16k(&samples) {
                                            Ok(wav) => wav,
                                            Err(err) => {
                                                let _ = internal_tx.send(
                                                    InternalMsg::TranscriptionFinished {
                                                        result: Err(err),
                                                        generation: transcription_gen,
                                                    },
                                                );
                                                return;
                                            }
                                        };
                                        let result = transcriber.transcribe(wav, cancel).await;
                                        let _ = internal_tx.send(InternalMsg::TranscriptionFinished {
                                            result,
                                            generation: transcription_gen,
                                        });
                                    });
                                }
                                Err(err) => {
                                    let _ = event_tx.send(VoiceEvent::Error(err.to_string()));
                                }
                            }
                        }
                        InternalMsg::TranscriptionFinished {
                            result,
                            generation: transcription_gen,
                        } => {
                            if transcription
                                .as_ref()
                                .is_some_and(|t| t.generation != transcription_gen)
                            {
                                continue;
                            }
                            transcription = None;
                            match result {
                                Ok(text) => {
                                    let _ = event_tx.send(VoiceEvent::Transcript(text));
                                }
                                Err(err) => {
                                    let _ = event_tx.send(VoiceEvent::Error(err.to_string()));
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    VoiceWorkerHandle {
        command_tx,
        event_rx,
        join: Some(join),
        shutdown_token,
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Context;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::config::VoiceSettings;

    struct FakeRecorder {
        wait_stop: bool,
        samples: Option<Vec<i16>>,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl Recorder for FakeRecorder {
        async fn record(
            &self,
            _max_duration: Duration,
            stop: CancellationToken,
            cancel: CancellationToken,
        ) -> anyhow::Result<Option<Vec<i16>>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.wait_stop {
                tokio::select! {
                    () = stop.cancelled() => {}
                    () = cancel.cancelled() => return Ok(None),
                }
            }
            if cancel.is_cancelled() {
                return Ok(None);
            }
            Ok(self.samples.clone())
        }
    }

    struct FakeTranscriber {
        text: Option<String>,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl Transcriber for FakeTranscriber {
        async fn transcribe(
            &self,
            _wav: Vec<u8>,
            cancel: CancellationToken,
        ) -> anyhow::Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if cancel.is_cancelled() {
                anyhow::bail!("transcription cancelled");
            }
            self.text
                .clone()
                .context("missing fake transcript")
        }
    }

    fn test_settings() -> VoiceSettings {
        VoiceSettings {
            enabled: true,
            api_key: Some("voice-test".into()),
            base_url: "http://localhost".into(),
            model: "gpt-4o-mini-transcribe".into(),
            language: Some("zh".into()),
            max_duration_secs: 300,
        }
    }

    async fn assert_event(rx: &mut UnboundedReceiver<VoiceEvent>, expected: VoiceEvent) {
        let event = rx.recv().await.expect("event");
        assert_eq!(event, expected);
    }

    #[tokio::test]
    async fn start_stop_records_then_transcribes_and_emits_text() {
        let recorder = Arc::new(FakeRecorder {
            wait_stop: true,
            samples: Some(vec![1, 2, 3]),
            calls: AtomicUsize::new(0),
        });
        let transcriber = Arc::new(FakeTranscriber {
            text: Some("transcript".into()),
            calls: AtomicUsize::new(0),
        });
        let mut worker = spawn_worker_with_components(test_settings(), recorder, transcriber);
        worker.command_tx.send(VoiceCommand::Start).unwrap();
        assert_event(&mut worker.event_rx, VoiceEvent::RecordingStarted).await;
        worker.command_tx.send(VoiceCommand::Stop).unwrap();
        assert_event(&mut worker.event_rx, VoiceEvent::RecordingStopped).await;
        assert_event(&mut worker.event_rx, VoiceEvent::Transcribing).await;
        assert_event(
            &mut worker.event_rx,
            VoiceEvent::Transcript("transcript".into()),
        )
        .await;
        worker.shutdown().await;
    }

    #[tokio::test]
    async fn cancel_recording_emits_cancelled_and_never_transcribes() {
        let recorder = Arc::new(FakeRecorder {
            wait_stop: true,
            samples: Some(vec![1, 2, 3]),
            calls: AtomicUsize::new(0),
        });
        let transcriber = Arc::new(FakeTranscriber {
            text: Some("transcript".into()),
            calls: AtomicUsize::new(0),
        });
        let transcriber_calls = transcriber.calls.load(Ordering::SeqCst);
        let mut worker = spawn_worker_with_components(
            test_settings(),
            recorder,
            transcriber.clone(),
        );
        worker.command_tx.send(VoiceCommand::Start).unwrap();
        assert_event(&mut worker.event_rx, VoiceEvent::RecordingStarted).await;
        worker.command_tx.send(VoiceCommand::Cancel).unwrap();
        assert_event(&mut worker.event_rx, VoiceEvent::Cancelled).await;
        assert_eq!(transcriber.calls.load(Ordering::SeqCst), transcriber_calls);
        worker.shutdown().await;
    }

    #[tokio::test]
    async fn missing_api_key_is_reported_by_transcriber_without_recording_failure() {
        let recorder = Arc::new(FakeRecorder {
            wait_stop: false,
            samples: Some(vec![1, 2, 3]),
            calls: AtomicUsize::new(0),
        });
        let settings = VoiceSettings {
            api_key: None,
            ..test_settings()
        };
        let mut worker = spawn_worker_with_components(
            settings,
            recorder,
            Arc::new(OpenAiTranscriber::new(VoiceSettings {
                api_key: None,
                ..test_settings()
            })),
        );
        worker.command_tx.send(VoiceCommand::Start).unwrap();
        assert_event(&mut worker.event_rx, VoiceEvent::RecordingStarted).await;
        worker.command_tx.send(VoiceCommand::Stop).unwrap();
        assert_event(&mut worker.event_rx, VoiceEvent::RecordingStopped).await;
        assert_event(&mut worker.event_rx, VoiceEvent::Transcribing).await;
        let err = worker.event_rx.recv().await.expect("error event");
        match err {
            VoiceEvent::Error(msg) => assert!(msg.contains("[voice].api_key")),
            other => panic!("expected error, got {other:?}"),
        }
        worker.shutdown().await;
    }

    #[tokio::test]
    async fn second_start_while_active_is_ignored_and_shutdown_is_safe() {
        let recorder = Arc::new(FakeRecorder {
            wait_stop: true,
            samples: Some(vec![1]),
            calls: AtomicUsize::new(0),
        });
        let transcriber = Arc::new(FakeTranscriber {
            text: Some("x".into()),
            calls: AtomicUsize::new(0),
        });
        let mut worker = spawn_worker_with_components(test_settings(), recorder, transcriber);
        worker.command_tx.send(VoiceCommand::Start).unwrap();
        assert_event(&mut worker.event_rx, VoiceEvent::RecordingStarted).await;
        worker.command_tx.send(VoiceCommand::Start).unwrap();
        worker.shutdown().await;
    }

    #[tokio::test]
    async fn duration_limit_completes_recording() {
        let recorder = Arc::new(FakeRecorder {
            wait_stop: false,
            samples: Some(vec![9, 9]),
            calls: AtomicUsize::new(0),
        });
        let transcriber = Arc::new(FakeTranscriber {
            text: Some("done".into()),
            calls: AtomicUsize::new(0),
        });
        let mut worker = spawn_worker_with_components(test_settings(), recorder, transcriber);
        worker.command_tx.send(VoiceCommand::Start).unwrap();
        assert_event(&mut worker.event_rx, VoiceEvent::RecordingStarted).await;
        assert_event(&mut worker.event_rx, VoiceEvent::RecordingStopped).await;
        assert_event(&mut worker.event_rx, VoiceEvent::Transcribing).await;
        assert_event(&mut worker.event_rx, VoiceEvent::Transcript("done".into())).await;
        worker.shutdown().await;
    }
}

//! Shared streaming machinery for capturing a child process's stdout/stderr
//! pipes.
//!
//! The synchronous [`bash`](crate::tool::bash) tool and the asynchronous
//! [`background`](crate::background) runner both spawn `sh -c` children and
//! stream their output live to the TUI. The low-level pieces — pipe reader
//! tasks, incremental UTF-8 decoding, progress batching with size caps — are
//! identical in both call sites, so they live here. Each caller keeps its own
//! select loop, output capture, and termination policy.

use std::{collections::VecDeque, time::Duration};

use tact_protocol::{ToolOutputChunk, ToolOutputStream};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::mpsc,
};

pub(crate) const READ_BUFFER_BYTES: usize = 4096;
pub(crate) const PIPE_CHANNEL_CAPACITY: usize = 32;
pub(crate) const MAX_PROGRESS_BYTES: usize = 4096;
pub(crate) const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);
pub(crate) const OMITTED_MARKER: &str = "[intermediate output omitted]\n";

/// Incremental UTF-8 decoder that survives bytes split across pipe reads.
#[derive(Default)]
pub(crate) struct Utf8Decoder {
    pending: Vec<u8>,
}

impl Utf8Decoder {
    pub(crate) fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut output = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(valid) => {
                    output.push_str(valid);
                    self.pending.clear();
                    break;
                }
                Err(error) => {
                    let valid_up_to = error.valid_up_to();
                    if valid_up_to > 0 {
                        let valid = std::str::from_utf8(&self.pending[..valid_up_to])
                            .expect("valid_up_to identifies valid UTF-8");
                        output.push_str(valid);
                        self.pending.drain(..valid_up_to);
                    }
                    let Some(error_len) = error.error_len() else {
                        break;
                    };
                    output.push('\u{fffd}');
                    self.pending.drain(..error_len);
                }
            }
        }
        output
    }

    pub(crate) fn finish(&mut self) -> String {
        let output = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        output
    }
}

pub(crate) enum PipeEvent {
    Bytes(ToolOutputStream, Vec<u8>),
    Closed(ToolOutputStream),
    Failed(ToolOutputStream, std::io::Error),
}

pub(crate) async fn read_pipe<R>(mut reader: R, stream: ToolOutputStream, tx: mpsc::Sender<PipeEvent>)
where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0_u8; READ_BUFFER_BYTES];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => {
                let _ = tx.send(PipeEvent::Closed(stream)).await;
                return;
            }
            Ok(read) => {
                if tx
                    .send(PipeEvent::Bytes(stream, buffer[..read].to_vec()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Err(error) => {
                let _ = tx.send(PipeEvent::Failed(stream, error)).await;
                return;
            }
        }
    }
}

pub(crate) fn stream_index(stream: ToolOutputStream) -> usize {
    match stream {
        ToolOutputStream::Stdout => 0,
        ToolOutputStream::Stderr => 1,
        ToolOutputStream::Other => 2,
    }
}

pub(crate) fn utf8_tail(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut start = text.len() - max_bytes;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// Coalesced progress batch awaiting the next flush tick.
#[derive(Default)]
pub(crate) struct PendingProgress {
    chunks: VecDeque<ToolOutputChunk>,
    bytes: usize,
    omitted: bool,
}

impl PendingProgress {
    pub(crate) fn push(&mut self, mut chunk: ToolOutputChunk) {
        if chunk.text.is_empty() {
            return;
        }
        let data_limit = MAX_PROGRESS_BYTES.saturating_sub(OMITTED_MARKER.len());
        if chunk.text.len() > data_limit {
            chunk.text = utf8_tail(&chunk.text, data_limit).to_string();
            self.chunks.clear();
            self.bytes = 0;
            self.omitted = true;
        }
        self.bytes += chunk.text.len();
        self.chunks.push_back(chunk);
        while self.bytes > data_limit {
            let Some(removed) = self.chunks.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(removed.text.len());
            self.omitted = true;
        }
    }

    pub(crate) fn take(&mut self) -> Vec<ToolOutputChunk> {
        let mut chunks = Vec::with_capacity(self.chunks.len() + usize::from(self.omitted));
        if self.omitted {
            chunks.push(ToolOutputChunk::other(OMITTED_MARKER));
        }
        chunks.extend(self.chunks.drain(..));
        self.bytes = 0;
        self.omitted = false;
        chunks
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.chunks.is_empty() && !self.omitted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_decoder_preserves_characters_split_across_reads() {
        let bytes = "前进".as_bytes();
        for split in 1..bytes.len() {
            let mut decoder = Utf8Decoder::default();
            let mut decoded = decoder.push(&bytes[..split]);
            decoded.push_str(&decoder.push(&bytes[split..]));
            decoded.push_str(&decoder.finish());
            assert_eq!(decoded, "前进", "split at byte {split}");
        }
    }

    #[test]
    fn pending_progress_drops_middle_chunks_when_over_cap() {
        let mut pending = PendingProgress::default();
        let large = ToolOutputChunk::stdout("x".repeat(MAX_PROGRESS_BYTES + 100));
        pending.push(large);
        let chunks = pending.take();
        let text: String = chunks.iter().map(|c| c.text.as_str()).collect();
        assert!(text.starts_with(OMITTED_MARKER));
        assert!(text.len() <= MAX_PROGRESS_BYTES);
    }
}

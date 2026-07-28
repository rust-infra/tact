use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use async_trait::async_trait;
use cpal::SampleFormat;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio_util::sync::CancellationToken;

const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Captures microphone audio and returns mono 16-bit PCM at 16 kHz.
pub struct CpalRecorder;

#[async_trait]
impl super::Recorder for CpalRecorder {
    async fn record(
        &self,
        max_duration: Duration,
        stop: CancellationToken,
        cancel: CancellationToken,
    ) -> anyhow::Result<Option<Vec<i16>>> {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        tokio::task::spawn_blocking(move || record_blocking(max_duration, stop, cancel))
            .await
            .context("microphone recording task failed")?
    }
}

fn record_blocking(
    max_duration: Duration,
    stop: CancellationToken,
    cancel: CancellationToken,
) -> anyhow::Result<Option<Vec<i16>>> {
    if cancel.is_cancelled() {
        return Ok(None);
    }

    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .context("no usable microphone found")?;

    let supported = device
        .default_input_config()
        .context("no supported microphone input configuration")?;

    let source_rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let format = supported.sample_format();

    let samples: Arc<Mutex<Vec<i16>>> = Arc::new(Mutex::new(Vec::new()));
    let err_flag = Arc::new(Mutex::new(None::<String>));

    let stream = match format {
        SampleFormat::I16 => {
            let samples_cb = Arc::clone(&samples);
            let err_flag_cb = Arc::clone(&err_flag);
            device
                .build_input_stream(
                    &supported.into(),
                    move |data: &[i16], _| {
                        append_mono_frames(&samples_cb, convert_i16_frames(data, channels));
                    },
                    move |err| store_stream_error(&err_flag_cb, err),
                    None,
                )
                .context("failed to build microphone input stream")?
        }
        SampleFormat::U16 => {
            let samples_cb = Arc::clone(&samples);
            let err_flag_cb = Arc::clone(&err_flag);
            device
                .build_input_stream(
                    &supported.into(),
                    move |data: &[u16], _| {
                        append_mono_frames(&samples_cb, convert_u16_frames(data, channels));
                    },
                    move |err| store_stream_error(&err_flag_cb, err),
                    None,
                )
                .context("failed to build microphone input stream")?
        }
        SampleFormat::F32 => {
            let samples_cb = Arc::clone(&samples);
            let err_flag_cb = Arc::clone(&err_flag);
            device
                .build_input_stream(
                    &supported.into(),
                    move |data: &[f32], _| {
                        append_mono_frames(&samples_cb, convert_f32_frames(data, channels));
                    },
                    move |err| store_stream_error(&err_flag_cb, err),
                    None,
                )
                .context("failed to build microphone input stream")?
        }
        other => bail!("unsupported microphone sample format: {other:?}"),
    };

    stream.play().context(
        "failed to start microphone stream; check microphone access in macOS System Settings",
    )?;

    let deadline = Instant::now() + max_duration;
    loop {
        if cancel.is_cancelled() {
            drop(stream);
            return Ok(None);
        }
        if stop.is_cancelled() || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    drop(stream);

    if cancel.is_cancelled() {
        return Ok(None);
    }

    if let Some(msg) = err_flag.lock().expect("err lock").take() {
        bail!("microphone stream error: {msg}; check microphone access in macOS System Settings");
    }

    let raw = samples.lock().expect("samples lock").clone();
    if raw.is_empty() {
        return Ok(Some(Vec::new()));
    }

    Ok(Some(resample_to_16k(&raw, source_rate)))
}

fn append_mono_frames(samples: &Arc<Mutex<Vec<i16>>>, mono: Vec<i16>) {
    if let Ok(mut buf) = samples.lock() {
        buf.extend(mono);
    }
}

fn store_stream_error(err_flag: &Arc<Mutex<Option<String>>>, err: cpal::StreamError) {
    if let Ok(mut flag) = err_flag.lock() {
        *flag = Some(err.to_string());
    }
}

pub(crate) fn convert_i16_frames(data: &[i16], channels: usize) -> Vec<i16> {
    average_frames(data, channels, |sample| *sample)
}

pub(crate) fn convert_u16_frames(data: &[u16], channels: usize) -> Vec<i16> {
    average_frames(data, channels, |sample| {
        let signed = i32::from(*sample) - 32_768;
        ((signed * i32::from(i16::MAX)) / 32_767) as i16
    })
}

pub(crate) fn convert_f32_frames(data: &[f32], channels: usize) -> Vec<i16> {
    average_frames(data, channels, |sample| {
        let clamped = sample.clamp(-1.0, 1.0);
        if clamped >= 0.0 {
            (clamped * f32::from(i16::MAX)).round() as i16
        } else {
            (clamped * -(i16::MIN as f32)).round() as i16
        }
    })
}

fn average_frames<T, F>(data: &[T], channels: usize, map: F) -> Vec<i16>
where
    F: Fn(&T) -> i16,
{
    let channels = channels.max(1);
    if data.is_empty() {
        return Vec::new();
    }
    let frames = data.len() / channels;
    let mut out = Vec::with_capacity(frames);
    for frame in 0..frames {
        let mut sum = 0i32;
        for ch in 0..channels {
            sum += i32::from(map(&data[frame * channels + ch]));
        }
        out.push((sum / channels as i32) as i16);
    }
    out
}

/// Linear resample mono PCM from `source_rate` to 16 kHz.
pub(crate) fn resample_to_16k(samples: &[i16], source_rate: u32) -> Vec<i16> {
    if source_rate == 0 || samples.is_empty() {
        return Vec::new();
    }
    if source_rate == TARGET_SAMPLE_RATE {
        return samples.to_vec();
    }
    let out_len = ((samples.len() as u64) * u64::from(TARGET_SAMPLE_RATE) / u64::from(source_rate))
        .max(1) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = (i as f64) * f64::from(source_rate) / f64::from(TARGET_SAMPLE_RATE);
        let idx = src_pos.floor() as usize;
        let frac = src_pos - idx as f64;
        let a = samples.get(idx).copied().unwrap_or(0);
        let b = samples.get(idx + 1).copied().unwrap_or(a);
        let value = a as f64 + (b as f64 - a as f64) * frac;
        out.push(value.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_i16_mono_passthrough() {
        let mono = convert_i16_frames(&[100i16, -200, 300], 1);
        assert_eq!(mono, vec![100, -200, 300]);
    }

    #[test]
    fn convert_u16_to_i16() {
        let mono = convert_u16_frames(&[u16::MAX, 0u16], 1);
        assert_eq!(mono.len(), 2);
        assert!(mono[0] > 0);
        assert_eq!(mono[1], i16::MIN);
    }

    #[test]
    fn convert_f32_clamps() {
        let mono = convert_f32_frames(&[1.0f32, -1.0], 1);
        assert_eq!(mono, vec![i16::MAX, i16::MIN]);
    }

    #[test]
    fn convert_multi_channel_averages() {
        let mono = convert_i16_frames(&[100i16, 200, 300, 500], 2);
        assert_eq!(mono, vec![150, 400]);
    }

    #[test]
    fn convert_empty_input() {
        let mono = convert_i16_frames(&[], 2);
        assert!(mono.is_empty());
    }

    #[test]
    fn resample_doubles_rate_halves_length() {
        let input: Vec<i16> = (0..8).map(|i| i as i16 * 100).collect();
        let out = resample_to_16k(&input, 32_000);
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn resample_same_rate_copies() {
        let input = vec![1i16, 2, 3];
        let out = resample_to_16k(&input, TARGET_SAMPLE_RATE);
        assert_eq!(out, input);
    }

    #[test]
    #[ignore = "requires macOS microphone permission and hardware"]
    fn record_from_real_device() {
        // Manual validation only.
    }
}

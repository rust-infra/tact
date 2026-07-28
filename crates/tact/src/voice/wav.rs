use anyhow::Context;

const SAMPLE_RATE: u32 = 16_000;
const BITS_PER_SAMPLE: u16 = 16;
const NUM_CHANNELS: u16 = 1;

fn sample_data_byte_len(sample_count: usize) -> anyhow::Result<u32> {
    let data_len = sample_count
        .checked_mul(2)
        .context("sample data length overflow")?;
    u32::try_from(data_len).context("WAV data chunk exceeds u32::MAX bytes")
}

/// Encode mono 16-bit PCM samples at 16 kHz into a RIFF/WAV byte vector.
pub fn encode_wav_mono_16k(samples: &[i16]) -> anyhow::Result<Vec<u8>> {
    let data_len_u32 = sample_data_byte_len(samples.len())?;
    let data_len = usize::try_from(data_len_u32).expect("validated byte length fits usize");
    let file_len = 36u32
        .checked_add(data_len_u32)
        .context("WAV file size overflow")?;

    let mut wav = Vec::with_capacity(44 + data_len);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&file_len.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&NUM_CHANNELS.to_le_bytes());
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    let byte_rate = SAMPLE_RATE * u32::from(NUM_CHANNELS) * u32::from(BITS_PER_SAMPLE) / 8;
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    let block_align = NUM_CHANNELS * BITS_PER_SAMPLE / 8;
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len_u32.to_le_bytes());
    for &sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(wav)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_wav_has_pcm_mono_16k_header_and_samples() {
        let wav = encode_wav_mono_16k(&[0, i16::MAX, i16::MIN]).unwrap();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(u16::from_le_bytes([wav[20], wav[21]]), 1); // PCM
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 1); // mono
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u16::from_le_bytes([wav[34], wav[35]]), 16);
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(wav.len(), 44 + 6);
    }

    #[test]
    fn encode_empty_wav_has_zero_data_length() {
        let wav = encode_wav_mono_16k(&[]).unwrap();
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 0);
        assert_eq!(wav.len(), 44);
    }

    #[test]
    fn encode_oversized_sample_slice_errors() {
        let huge_len = (u32::MAX as usize / 2) + 1;
        let err = sample_data_byte_len(huge_len).unwrap_err();
        assert!(err.to_string().contains("u32::MAX"));
    }
}

//! Audio helpers: PCM16 WAV container encoding and RMS (energy) based
//! server-side voice-activity detection.

/// Wrap raw little-endian PCM16 mono samples in a minimal WAV container.
pub fn pcm16_to_wav(pcm: &[u8], sample_rate: u32, channels: u16) -> Vec<u8> {
    let data_len = pcm.len() as u32;
    let byte_rate = sample_rate * channels as u32 * 2;
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&(channels * 2).to_le_bytes()); // block align
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}

/// Root-mean-square of little-endian PCM16 samples (0.0 ..= 32767.0 scale).
pub fn pcm16_rms(pcm: &[u8]) -> f32 {
    let n = pcm.len() / 2;
    if n == 0 {
        return 0.0;
    }
    let sum_sq: f64 = pcm
        .chunks_exact(2)
        .map(|c| {
            let s = i16::from_le_bytes([c[0], c[1]]) as f64;
            s * s
        })
        .sum();
    (sum_sq / n as f64).sqrt() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_is_wellformed() {
        let wav = pcm16_to_wav(&[0u8, 0, 0, 0], 16000, 1);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(wav.len(), 44 + 4);
    }

    #[test]
    fn rms_zero_and_nonzero() {
        assert_eq!(pcm16_rms(&[]), 0.0);
        assert_eq!(pcm16_rms(&[0, 0, 0, 0]), 0.0);
        let loud = pcm16_rms(&[0x00, 0x40, 0x00, 0x40]); // 16384, 16384
        assert!((loud - 16384.0).abs() < 1.0);
    }
}

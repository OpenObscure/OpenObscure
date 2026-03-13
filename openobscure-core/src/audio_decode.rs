//! Audio format decoding — base64 → PCM f32 mono at 16kHz.
//!
//! Supports WAV, MP3, and OGG via symphonia (behind `voice` feature).
//! Used by the KWS engine for keyword spotting on audio blocks.

use crate::voice_detect::AudioFormat;

/// Audio decode error.
#[derive(Debug)]
pub enum AudioDecodeError {
    Base64(String),
    Format(String),
    Resample(String),
    #[allow(dead_code)]
    FeatureDisabled,
}

impl std::fmt::Display for AudioDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioDecodeError::Base64(e) => write!(f, "base64 decode: {}", e),
            AudioDecodeError::Format(e) => write!(f, "audio format: {}", e),
            AudioDecodeError::Resample(e) => write!(f, "resample: {}", e),
            AudioDecodeError::FeatureDisabled => write!(f, "voice feature not enabled"),
        }
    }
}

/// Decode base64-encoded audio to PCM f32 mono samples at the original sample rate.
///
/// Returns `(samples, sample_rate)` — the audio at its native rate.
/// The caller (KWS engine) can pass the sample rate to sherpa-onnx,
/// which handles resampling internally.
#[cfg(feature = "voice")]
pub fn decode_audio_to_pcm(
    base64_data: &str,
    format: AudioFormat,
) -> Result<(Vec<f32>, u32), AudioDecodeError> {
    use base64::Engine;

    let raw_bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_data)
        .map_err(|e| AudioDecodeError::Base64(format!("{}", e)))?;

    decode_bytes_to_pcm(&raw_bytes, format)
}

/// Stub when voice feature is disabled.
#[cfg(not(feature = "voice"))]
pub fn decode_audio_to_pcm(
    _base64_data: &str,
    _format: AudioFormat,
) -> Result<(Vec<f32>, u32), AudioDecodeError> {
    Err(AudioDecodeError::FeatureDisabled)
}

/// Decode raw audio bytes to PCM f32 mono samples via symphonia.
/// Returns `(samples, sample_rate)` at the original sample rate.
#[cfg(feature = "voice")]
fn decode_bytes_to_pcm(
    data: &[u8],
    format: AudioFormat,
) -> Result<(Vec<f32>, u32), AudioDecodeError> {
    use std::io::Cursor;
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let cursor = Cursor::new(data.to_vec());
    let mss = MediaSourceStream::new(Box::new(cursor), Default::default());

    let mut hint = Hint::new();
    match format {
        AudioFormat::Wav => hint.with_extension("wav"),
        AudioFormat::Mp3 => hint.with_extension("mp3"),
        AudioFormat::Ogg => hint.with_extension("ogg"),
        _ => &mut hint,
    };

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| AudioDecodeError::Format(format!("probe: {}", e)))?;

    let mut format_reader = probed.format;
    let track = format_reader
        .default_track()
        .ok_or_else(|| AudioDecodeError::Format("no audio track found".to_string()))?;
    let track_id = track.id;
    let sample_rate = track.codec_params.sample_rate.unwrap_or(16000);

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| AudioDecodeError::Format(format!("codec: {}", e)))?;

    let mut samples = Vec::new();

    loop {
        let packet = match format_reader.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => return Err(AudioDecodeError::Format(format!("packet: {}", e))),
        };

        if packet.track_id() != track_id {
            continue;
        }

        // Decode packet — skip decode errors gracefully
        let decoded = match decoder.decode(&packet) {
            Ok(buf) => buf,
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(e) => return Err(AudioDecodeError::Format(format!("decode: {}", e))),
        };

        let spec = *decoded.spec();
        let num_frames = decoded.frames();
        if num_frames == 0 {
            continue;
        }

        // Guard against a symphonia panic on malformed OGG/Vorbis packets.
        // `copy_interleaved_ref` can trigger "range start index N out of range for
        // slice of length 0" on corrupt audio — catch the unwind and skip the packet.
        let mut sample_buf = SampleBuffer::<f32>::new(num_frames as u64, spec);
        let copy_ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            sample_buf.copy_interleaved_ref(decoded);
        }));
        if copy_ok.is_err() {
            continue; // skip malformed packet
        }

        let channels = spec.channels.count();
        let buf = sample_buf.samples();

        // Mix to mono if multi-channel
        if channels > 1 {
            for frame in 0..num_frames {
                let mut sum = 0.0f32;
                for ch in 0..channels {
                    sum += buf[frame * channels + ch];
                }
                samples.push(sum / channels as f32);
            }
        } else {
            samples.extend_from_slice(buf);
        }
    }

    // Return samples at the original rate — let sherpa-onnx handle resampling
    Ok((samples, sample_rate))
}

/// Resample audio to a target sample rate using linear interpolation.
///
/// Not called by the current pipeline — sherpa-onnx performs its own internal
/// resampling to 16 kHz before running the Zipformer KWS model. Retained as a
/// utility for future audio consumers that require a specific sample rate.
#[cfg(feature = "voice")]
#[allow(dead_code)]
fn resample_linear(
    samples: &[f32],
    source_rate: u32,
    target_rate: u32,
) -> Result<Vec<f32>, AudioDecodeError> {
    if source_rate == 0 || target_rate == 0 {
        return Err(AudioDecodeError::Resample(
            "invalid sample rate".to_string(),
        ));
    }
    if samples.is_empty() {
        return Ok(Vec::new());
    }

    let ratio = source_rate as f64 / target_rate as f64;
    let output_len = (samples.len() as f64 / ratio).ceil() as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_pos = i as f64 * ratio;
        let idx = src_pos as usize;
        let frac = (src_pos - idx as f64) as f32;

        if idx + 1 < samples.len() {
            output.push(samples[idx] * (1.0 - frac) + samples[idx + 1] * frac);
        } else if idx < samples.len() {
            output.push(samples[idx]);
        }
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "voice")]
    #[test]
    fn test_resample_linear_same_rate() {
        let samples = vec![1.0, 2.0, 3.0, 4.0];
        let result = resample_linear(&samples, 16000, 16000).unwrap();
        assert_eq!(result.len(), samples.len());
        for (a, b) in result.iter().zip(samples.iter()) {
            assert!((a - b).abs() < 0.01);
        }
    }

    #[cfg(feature = "voice")]
    #[test]
    fn test_resample_linear_downsample() {
        // 32kHz → 16kHz should halve the samples
        let samples: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let result = resample_linear(&samples, 32000, 16000).unwrap();
        assert_eq!(result.len(), 50);
    }

    #[cfg(feature = "voice")]
    #[test]
    fn test_resample_linear_upsample() {
        // 8kHz → 16kHz should double the samples
        let samples: Vec<f32> = (0..50).map(|i| i as f32).collect();
        let result = resample_linear(&samples, 8000, 16000).unwrap();
        assert_eq!(result.len(), 100);
    }

    #[cfg(feature = "voice")]
    #[test]
    fn test_resample_linear_empty() {
        let result = resample_linear(&[], 44100, 16000).unwrap();
        assert!(result.is_empty());
    }

    #[cfg(feature = "voice")]
    #[test]
    fn test_resample_linear_invalid_rate() {
        let result = resample_linear(&[1.0], 0, 16000);
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_feature_disabled() {
        // When voice feature is not enabled, decode returns FeatureDisabled
        // This test runs on default features (voice disabled)
        #[cfg(not(feature = "voice"))]
        {
            let result = decode_audio_to_pcm("AAAA", AudioFormat::Wav);
            assert!(result.is_err());
        }
    }
}

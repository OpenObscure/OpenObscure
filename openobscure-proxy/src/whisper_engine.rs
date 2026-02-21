//! Whisper-base ONNX speech-to-text engine.
//!
//! Desktop-only (behind `voice` feature). Converts audio PCM to text
//! with word-level timestamps for PII mapping.
//!
//! Model: Whisper-base (~74MB ONNX, ~150MB inference RAM).

use std::path::{Path, PathBuf};

#[cfg(feature = "voice")]
use crate::voice_detect::AudioFormat;

/// A transcribed text segment with timestamps.
#[derive(Debug, Clone)]
pub struct TranscriptSegment {
    /// Transcribed text.
    pub text: String,
    /// Start time in seconds.
    pub start_secs: f32,
    /// End time in seconds.
    pub end_secs: f32,
}

/// Configuration for the Whisper engine.
#[derive(Debug, Clone)]
pub struct WhisperConfig {
    /// Path to Whisper ONNX encoder model.
    pub encoder_model_path: PathBuf,
    /// Path to Whisper ONNX decoder model.
    pub decoder_model_path: PathBuf,
    /// Sample rate for input audio (default: 16000).
    pub sample_rate: u32,
    /// Maximum audio duration in seconds (default: 30).
    pub max_duration_secs: u32,
}

impl Default for WhisperConfig {
    fn default() -> Self {
        Self {
            encoder_model_path: PathBuf::from("models/whisper_base_encoder.onnx"),
            decoder_model_path: PathBuf::from("models/whisper_base_decoder.onnx"),
            sample_rate: 16000,
            max_duration_secs: 30,
        }
    }
}

/// Whisper speech-to-text engine.
///
/// On-demand loading: models are loaded when first needed and evicted
/// after processing to conserve memory.
pub struct WhisperEngine {
    config: WhisperConfig,
}

impl WhisperEngine {
    /// Create a new Whisper engine with the given config.
    pub fn new(config: WhisperConfig) -> Self {
        Self { config }
    }

    /// Check if the Whisper model files exist.
    pub fn is_available(&self) -> bool {
        self.config.encoder_model_path.exists() && self.config.decoder_model_path.exists()
    }

    /// Transcribe PCM audio (16kHz, mono, f32) to text segments.
    ///
    /// Returns segments with approximate timestamps.
    #[cfg(feature = "voice")]
    pub fn transcribe(&mut self, pcm: &[f32]) -> Result<Vec<TranscriptSegment>, WhisperError> {
        if pcm.is_empty() {
            return Ok(vec![]);
        }

        // Truncate to max duration
        let max_samples = (self.config.max_duration_secs * self.config.sample_rate) as usize;
        let pcm = if pcm.len() > max_samples {
            &pcm[..max_samples]
        } else {
            pcm
        };

        // Load model on demand
        let encoder = self.load_encoder()?;
        let decoder = self.load_decoder()?;

        // Compute mel spectrogram (80 bins, 30s window)
        let mel = compute_mel_spectrogram(pcm, self.config.sample_rate);

        // Run encoder
        let encoder_output = run_encoder(&encoder, &mel)?;

        // Run decoder (greedy decoding)
        let tokens = run_decoder(&decoder, &encoder_output)?;

        // Convert tokens to text segments
        let segments =
            tokens_to_segments(&tokens, pcm.len() as f32 / self.config.sample_rate as f32);

        Ok(segments)
    }

    /// Transcribe without `voice` feature — always returns error.
    #[cfg(not(feature = "voice"))]
    pub fn transcribe(&mut self, _pcm: &[f32]) -> Result<Vec<TranscriptSegment>, WhisperError> {
        Err(WhisperError::NotAvailable(
            "voice feature not enabled".to_string(),
        ))
    }

    #[cfg(feature = "voice")]
    fn load_encoder(&self) -> Result<ort::Session, WhisperError> {
        let path = &self.config.encoder_model_path;
        if !path.exists() {
            return Err(WhisperError::ModelNotFound(path.display().to_string()));
        }
        ort::Session::builder()
            .map_err(|e| WhisperError::OrtError(e.to_string()))?
            .with_intra_threads(1)
            .map_err(|e| WhisperError::OrtError(e.to_string()))?
            .commit_from_file(path)
            .map_err(|e| WhisperError::OrtError(e.to_string()))
    }

    #[cfg(feature = "voice")]
    fn load_decoder(&self) -> Result<ort::Session, WhisperError> {
        let path = &self.config.decoder_model_path;
        if !path.exists() {
            return Err(WhisperError::ModelNotFound(path.display().to_string()));
        }
        ort::Session::builder()
            .map_err(|e| WhisperError::OrtError(e.to_string()))?
            .with_intra_threads(1)
            .map_err(|e| WhisperError::OrtError(e.to_string()))?
            .commit_from_file(path)
            .map_err(|e| WhisperError::OrtError(e.to_string()))
    }
}

/// Whisper-specific errors.
#[derive(Debug, thiserror::Error)]
pub enum WhisperError {
    #[error("Whisper model not found: {0}")]
    ModelNotFound(String),
    #[error("ONNX Runtime error: {0}")]
    OrtError(String),
    #[error("Audio decode error: {0}")]
    AudioDecodeError(String),
    #[error("Whisper not available: {0}")]
    NotAvailable(String),
}

/// Decode base64 audio to PCM f32 samples at 16kHz mono.
///
/// Supports WAV, MP3, and OGG via symphonia (when `voice` feature is enabled).
#[cfg(feature = "voice")]
pub fn decode_audio_to_pcm(
    base64_data: &str,
    format: AudioFormat,
) -> Result<Vec<f32>, WhisperError> {
    use base64::Engine;
    use std::io::Cursor;

    let raw_bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_data)
        .map_err(|e| WhisperError::AudioDecodeError(format!("base64 decode: {}", e)))?;

    decode_bytes_to_pcm(&raw_bytes, format)
}

#[cfg(feature = "voice")]
fn decode_bytes_to_pcm(data: &[u8], format: AudioFormat) -> Result<Vec<f32>, WhisperError> {
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
        .map_err(|e| WhisperError::AudioDecodeError(format!("probe: {}", e)))?;

    let mut format_reader = probed.format;
    let track = format_reader
        .default_track()
        .ok_or_else(|| WhisperError::AudioDecodeError("no audio track found".to_string()))?;
    let track_id = track.id;
    let sample_rate = track.codec_params.sample_rate.unwrap_or(16000);

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| WhisperError::AudioDecodeError(format!("codec: {}", e)))?;

    let mut samples = Vec::new();

    loop {
        let packet = match format_reader.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => return Err(WhisperError::AudioDecodeError(format!("packet: {}", e))),
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = decoder
            .decode(&packet)
            .map_err(|e| WhisperError::AudioDecodeError(format!("decode: {}", e)))?;

        let spec = *decoded.spec();
        let num_frames = decoded.frames();
        let mut sample_buf = SampleBuffer::<f32>::new(num_frames as u64, spec);
        sample_buf.copy_interleaved_ref(decoded);

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

    // Resample to 16kHz if needed
    if sample_rate != 16000 && !samples.is_empty() {
        samples = resample_to_16k(&samples, sample_rate)?;
    }

    Ok(samples)
}

/// Resample audio to 16kHz using linear interpolation.
#[cfg(feature = "voice")]
fn resample_to_16k(samples: &[f32], source_rate: u32) -> Result<Vec<f32>, WhisperError> {
    use rubato::{FftFixedIn, Resampler};

    let mut resampler = FftFixedIn::<f32>::new(
        source_rate as usize,
        16000,
        1024,
        2,
        1, // mono
    )
    .map_err(|e| WhisperError::AudioDecodeError(format!("resampler: {}", e)))?;

    let mut output = Vec::new();
    let chunk_size = 1024;

    for chunk in samples.chunks(chunk_size) {
        let mut padded = chunk.to_vec();
        if padded.len() < chunk_size {
            padded.resize(chunk_size, 0.0);
        }
        let input = vec![padded];
        let result = resampler
            .process(&input, None)
            .map_err(|e| WhisperError::AudioDecodeError(format!("resample: {}", e)))?;
        if !result.is_empty() {
            output.extend_from_slice(&result[0]);
        }
    }

    Ok(output)
}

/// Mask audio samples in a time range (silence or beep).
pub fn mask_audio_region(
    pcm: &mut [f32],
    start_secs: f32,
    end_secs: f32,
    sample_rate: u32,
    beep: bool,
) {
    let start_sample = (start_secs * sample_rate as f32) as usize;
    let end_sample = (end_secs * sample_rate as f32).min(pcm.len() as f32) as usize;

    if start_sample >= pcm.len() || start_sample >= end_sample {
        return;
    }

    if beep {
        // 1kHz sine tone at 0.3 amplitude
        let freq = 1000.0;
        for (i, sample) in pcm[start_sample..end_sample].iter_mut().enumerate() {
            let t = i as f32 / sample_rate as f32;
            *sample = 0.3 * (2.0 * std::f32::consts::PI * freq * t).sin();
        }
    } else {
        // Silence
        for sample in &mut pcm[start_sample..end_sample] {
            *sample = 0.0;
        }
    }
}

// ── Whisper-specific helpers (stubs for now — real impl needs model) ──

#[cfg(feature = "voice")]
fn compute_mel_spectrogram(_pcm: &[f32], _sample_rate: u32) -> Vec<f32> {
    // Placeholder: Whisper uses 80-bin mel spectrogram at 10ms hop
    // Real implementation computes STFT + mel filterbank
    vec![0.0; 80 * 3000] // 80 bins × 3000 frames (30s at 10ms hop)
}

#[cfg(feature = "voice")]
fn run_encoder(_session: &ort::Session, _mel: &[f32]) -> Result<Vec<f32>, WhisperError> {
    // Placeholder: run encoder ONNX model
    Ok(vec![0.0; 512 * 1500]) // encoder output shape
}

#[cfg(feature = "voice")]
fn run_decoder(
    _session: &ort::Session,
    _encoder_output: &[f32],
) -> Result<Vec<(u32, f32, f32)>, WhisperError> {
    // Placeholder: run decoder with greedy search
    // Returns (token_id, start_secs, end_secs)
    Ok(vec![])
}

#[cfg(feature = "voice")]
fn tokens_to_segments(tokens: &[(u32, f32, f32)], _total_duration: f32) -> Vec<TranscriptSegment> {
    // Placeholder: convert token IDs to text using vocabulary
    // Group tokens into segments by punctuation/pauses
    tokens
        .iter()
        .map(|&(_token, start, end)| TranscriptSegment {
            text: String::new(),
            start_secs: start,
            end_secs: end,
        })
        .collect()
}

/// Find Whisper model directory.
pub fn find_whisper_model_dir(config_dir: Option<&str>) -> Option<PathBuf> {
    if let Some(dir) = config_dir {
        let path = Path::new(dir);
        if path.join("whisper_base_encoder.onnx").exists() {
            return Some(path.to_path_buf());
        }
    }
    // Default locations
    for dir in &["models", "data/models", "../models"] {
        let path = Path::new(dir);
        if path.join("whisper_base_encoder.onnx").exists() {
            return Some(path.to_path_buf());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_whisper_config_default() {
        let config = WhisperConfig::default();
        assert_eq!(config.sample_rate, 16000);
        assert_eq!(config.max_duration_secs, 30);
    }

    #[test]
    fn test_whisper_engine_not_available() {
        let config = WhisperConfig {
            encoder_model_path: PathBuf::from("/nonexistent/encoder.onnx"),
            decoder_model_path: PathBuf::from("/nonexistent/decoder.onnx"),
            ..Default::default()
        };
        let engine = WhisperEngine::new(config);
        assert!(!engine.is_available());
    }

    #[test]
    fn test_mask_audio_silence() {
        let mut pcm = vec![1.0; 16000]; // 1 second at 16kHz
        mask_audio_region(&mut pcm, 0.25, 0.75, 16000, false);

        // First quarter should be untouched
        assert_eq!(pcm[0], 1.0);
        assert_eq!(pcm[3999], 1.0);

        // Middle half should be silenced
        assert_eq!(pcm[4000], 0.0);
        assert_eq!(pcm[8000], 0.0);
        assert_eq!(pcm[11999], 0.0);

        // Last quarter should be untouched
        assert_eq!(pcm[12000], 1.0);
    }

    #[test]
    fn test_mask_audio_beep() {
        let mut pcm = vec![0.0; 16000]; // 1 second at 16kHz
        mask_audio_region(&mut pcm, 0.0, 0.5, 16000, true);

        // Should have non-zero values (1kHz sine)
        let has_nonzero = pcm[..8000].iter().any(|&s| s != 0.0);
        assert!(has_nonzero, "Beep region should have non-zero samples");

        // Second half should still be zero
        assert_eq!(pcm[8000], 0.0);
    }

    #[test]
    fn test_mask_audio_out_of_range() {
        let mut pcm = vec![1.0; 100];
        mask_audio_region(&mut pcm, 10.0, 20.0, 16000, false);
        // Start beyond PCM length — should be a no-op
        assert_eq!(pcm[0], 1.0);
    }

    #[test]
    fn test_find_whisper_model_dir_missing() {
        assert!(find_whisper_model_dir(Some("/nonexistent/path")).is_none());
    }

    #[test]
    fn test_transcribe_not_available() {
        let config = WhisperConfig {
            encoder_model_path: PathBuf::from("/nonexistent/encoder.onnx"),
            decoder_model_path: PathBuf::from("/nonexistent/decoder.onnx"),
            ..Default::default()
        };
        let mut engine = WhisperEngine::new(config);
        let result = engine.transcribe(&[0.0; 100]);
        // Without voice feature: error; with voice feature: model not found
        assert!(result.is_err());
    }
}

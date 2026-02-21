//! Voice anonymization pipeline.
//!
//! Orchestrates: audio detection → transcription → PII scan → masking.
//!
//! Desktop: full pipeline (Whisper + PII scan + audio masking).
//! Mobile: detection only (delegates to gateway for anonymization).

use serde_json::Value;

use crate::hybrid_scanner::HybridScanner;
#[cfg(feature = "voice")]
use crate::voice_detect::AudioFormat;
use crate::voice_detect::{detect_audio_blocks, AudioBlock};
use crate::whisper_engine::{self, WhisperConfig, WhisperEngine, WhisperError};

/// Result of processing a single audio block.
#[derive(Debug)]
pub struct VoiceProcessResult {
    /// JSON path of the audio block.
    pub json_path: String,
    /// Number of PII segments detected and masked.
    pub pii_count: usize,
    /// Total duration of masked audio in seconds.
    pub masked_duration_secs: f32,
}

/// Voice pipeline configuration.
#[derive(Debug, Clone, Default)]
pub struct VoicePipelineConfig {
    /// Whether voice anonymization is enabled.
    pub enabled: bool,
    /// Use beep tone instead of silence for masking.
    pub use_beep: bool,
    /// Whisper model configuration.
    pub whisper: WhisperConfig,
}

/// Voice anonymization pipeline.
pub struct VoicePipeline {
    config: VoicePipelineConfig,
    engine: Option<WhisperEngine>,
}

impl VoicePipeline {
    /// Create a new voice pipeline.
    pub fn new(config: VoicePipelineConfig) -> Self {
        let engine = if config.enabled {
            let engine = WhisperEngine::new(config.whisper.clone());
            if engine.is_available() {
                Some(engine)
            } else {
                None
            }
        } else {
            None
        };

        Self { config, engine }
    }

    /// Check if the voice pipeline is ready (model available).
    pub fn is_ready(&self) -> bool {
        self.config.enabled && self.engine.is_some()
    }

    /// Detect audio blocks in a JSON body without processing them.
    ///
    /// Used by mobile (detection-only) and for metrics.
    pub fn detect_audio(json: &Value) -> Vec<AudioBlock> {
        detect_audio_blocks(json)
    }

    /// Process a JSON request body: detect audio, transcribe, scan PII, mask.
    ///
    /// Returns the modified JSON (with masked audio) and processing results.
    pub fn process_request(
        &mut self,
        json: &mut Value,
        scanner: &HybridScanner,
    ) -> Vec<VoiceProcessResult> {
        if !self.config.enabled {
            return vec![];
        }

        let blocks = detect_audio_blocks(json);
        if blocks.is_empty() {
            return vec![];
        }

        let engine = match &mut self.engine {
            Some(e) => e,
            None => return vec![], // No model available — fail open
        };

        let mut results = Vec::new();
        let use_beep = self.config.use_beep;
        let sample_rate = self.config.whisper.sample_rate;

        for block in &blocks {
            match process_audio_block(block, engine, scanner, sample_rate, use_beep) {
                Ok(result) => {
                    if result.pii_count > 0 {
                        // Replace audio data in the JSON body
                        if let Some(masked_data) = result.masked_data.as_ref() {
                            replace_audio_data(json, &block.json_path, masked_data);
                        }
                    }
                    results.push(VoiceProcessResult {
                        json_path: block.json_path.clone(),
                        pii_count: result.pii_count,
                        masked_duration_secs: result.masked_duration_secs,
                    });
                }
                Err(e) => {
                    // Fail open: log error, pass audio through unchanged
                    oo_warn!(
                        crate::oo_log::modules::VOICE,
                        "Voice pipeline error, passing through",
                        error = %e,
                        path = block.json_path
                    );
                }
            }
        }

        results
    }
}

fn process_audio_block(
    block: &AudioBlock,
    engine: &mut WhisperEngine,
    scanner: &HybridScanner,
    sample_rate: u32,
    use_beep: bool,
) -> Result<AudioProcessResult, WhisperError> {
    // Decode audio to PCM
    #[cfg(feature = "voice")]
    let mut pcm = {
        let format = AudioFormat::from_mime(&block.media_type);
        whisper_engine::decode_audio_to_pcm(&block.data, format)?
    };
    #[cfg(not(feature = "voice"))]
    let mut pcm: Vec<f32> = {
        let _ = block; // suppress unused warning when voice feature disabled
        Vec::new()
    };

    if pcm.is_empty() {
        return Ok(AudioProcessResult {
            pii_count: 0,
            masked_duration_secs: 0.0,
            masked_data: None,
        });
    }

    // Transcribe
    let segments = engine.transcribe(&pcm)?;

    // Scan transcription for PII
    let mut pii_count = 0;
    let mut masked_duration = 0.0f32;

    for segment in &segments {
        let matches = scanner.scan_text(&segment.text);
        if !matches.is_empty() {
            pii_count += matches.len();
            let duration = segment.end_secs - segment.start_secs;
            masked_duration += duration;

            // Mask the audio region
            whisper_engine::mask_audio_region(
                &mut pcm,
                segment.start_secs,
                segment.end_secs,
                sample_rate,
                use_beep,
            );
        }
    }

    // Re-encode to base64 if any masking was done
    let masked_data = if pii_count > 0 {
        Some(encode_pcm_to_base64_wav(&pcm, sample_rate))
    } else {
        None
    };

    Ok(AudioProcessResult {
        pii_count,
        masked_duration_secs: masked_duration,
        masked_data,
    })
}

/// Internal processing result with optional masked audio data.
struct AudioProcessResult {
    pii_count: usize,
    masked_duration_secs: f32,
    masked_data: Option<String>,
}

/// Encode PCM f32 samples to a base64 WAV string.
fn encode_pcm_to_base64_wav(pcm: &[f32], sample_rate: u32) -> String {
    use base64::Engine;

    let num_samples = pcm.len();
    let data_size = num_samples * 2; // 16-bit samples
    let file_size = 36 + data_size;

    let mut wav = Vec::with_capacity(44 + data_size);

    // RIFF header
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(file_size as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVE");

    // fmt chunk
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // chunk size
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM format
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    wav.extend_from_slice(&2u16.to_le_bytes()); // block align
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample

    // data chunk
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data_size as u32).to_le_bytes());

    // Convert f32 to i16
    for &sample in pcm {
        let clamped = sample.clamp(-1.0, 1.0);
        let i16_val = (clamped * 32767.0) as i16;
        wav.extend_from_slice(&i16_val.to_le_bytes());
    }

    base64::engine::general_purpose::STANDARD.encode(&wav)
}

/// Replace audio data at a JSON path.
fn replace_audio_data(json: &mut Value, json_path: &str, new_data: &str) {
    // Navigate to the audio block and replace its data
    // Paths like "messages[0].content[1].source.data"
    let parts: Vec<&str> = json_path.split('.').collect();
    let mut current = json;

    for part in &parts {
        if let Some(bracket_pos) = part.find('[') {
            let key = &part[..bracket_pos];
            let idx_str = &part[bracket_pos + 1..part.len() - 1];
            if let Ok(idx) = idx_str.parse::<usize>() {
                current = &mut current[key][idx];
            }
        } else {
            current = &mut current[part];
        }
    }

    // Replace data field in Anthropic format
    if let Some(source) = current.get_mut("source") {
        if let Some(data) = source.get_mut("data") {
            *data = Value::String(new_data.to_string());
            return;
        }
    }

    // Replace data field in OpenAI format
    if let Some(audio) = current.get_mut("input_audio") {
        if let Some(data) = audio.get_mut("data") {
            *data = Value::String(new_data.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_disabled_by_default() {
        let pipeline = VoicePipeline::new(VoicePipelineConfig::default());
        assert!(!pipeline.is_ready());
    }

    #[test]
    fn test_detect_audio_empty_json() {
        let json: Value = serde_json::from_str(r#"{"messages": []}"#).unwrap();
        let blocks = VoicePipeline::detect_audio(&json);
        assert!(blocks.is_empty());
    }

    #[test]
    fn test_detect_audio_anthropic() {
        let json: Value = serde_json::from_str(r#"{
            "messages": [{"content": [
                {"type": "audio", "source": {"type": "base64", "media_type": "audio/wav", "data": "dGVzdA=="}}
            ]}]
        }"#).unwrap();
        let blocks = VoicePipeline::detect_audio(&json);
        assert_eq!(blocks.len(), 1);
    }

    #[test]
    fn test_detect_audio_openai() {
        let json: Value = serde_json::from_str(
            r#"{
            "messages": [{"content": [
                {"type": "input_audio", "input_audio": {"data": "dGVzdA==", "format": "wav"}}
            ]}]
        }"#,
        )
        .unwrap();
        let blocks = VoicePipeline::detect_audio(&json);
        assert_eq!(blocks.len(), 1);
    }

    #[test]
    fn test_process_request_disabled() {
        let mut pipeline = VoicePipeline::new(VoicePipelineConfig::default());
        let scanner = HybridScanner::new(false, None);
        let mut json: Value = serde_json::from_str(r#"{"messages": []}"#).unwrap();
        let results = pipeline.process_request(&mut json, &scanner);
        assert!(results.is_empty());
    }

    #[test]
    fn test_process_request_no_model() {
        let config = VoicePipelineConfig {
            enabled: true,
            ..Default::default()
        };
        let mut pipeline = VoicePipeline::new(config);
        let scanner = HybridScanner::new(false, None);
        let mut json: Value = serde_json::from_str(r#"{
            "messages": [{"content": [
                {"type": "audio", "source": {"type": "base64", "media_type": "audio/wav", "data": "dGVzdA=="}}
            ]}]
        }"#).unwrap();
        let results = pipeline.process_request(&mut json, &scanner);
        assert!(results.is_empty()); // No model → fail open
    }

    #[test]
    fn test_encode_pcm_to_wav() {
        let pcm = vec![0.0f32; 160]; // 10ms at 16kHz
        let b64 = encode_pcm_to_base64_wav(&pcm, 16000);

        // Should be valid base64
        use base64::Engine;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .unwrap();

        // Check WAV header
        assert_eq!(&decoded[..4], b"RIFF");
        assert_eq!(&decoded[8..12], b"WAVE");
    }

    #[test]
    fn test_replace_audio_data_anthropic() {
        let mut json: Value = serde_json::from_str(r#"{
            "messages": [{"content": [
                {"type": "audio", "source": {"type": "base64", "media_type": "audio/wav", "data": "original"}}
            ]}]
        }"#).unwrap();

        replace_audio_data(&mut json, "messages[0].content[0]", "replaced");

        let data = json["messages"][0]["content"][0]["source"]["data"]
            .as_str()
            .unwrap();
        assert_eq!(data, "replaced");
    }

    #[test]
    fn test_replace_audio_data_openai() {
        let mut json: Value = serde_json::from_str(
            r#"{
            "messages": [{"content": [
                {"type": "input_audio", "input_audio": {"data": "original", "format": "wav"}}
            ]}]
        }"#,
        )
        .unwrap();

        replace_audio_data(&mut json, "messages[0].content[0]", "replaced");

        let data = json["messages"][0]["content"][0]["input_audio"]["data"]
            .as_str()
            .unwrap();
        assert_eq!(data, "replaced");
    }
}

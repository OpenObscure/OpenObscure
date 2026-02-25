//! Keyword Spotter (KWS) engine for audio PII detection.
//!
//! Uses sherpa-onnx's Zipformer keyword spotter via C bindings directly
//! (bypasses the `sherpa_rs::keyword_spot` Rust wrapper which has stream
//! reuse and multi-keyword detection bugs).
//!
//! Detects PII trigger phrases ("social security", "credit card", etc.)
//! in audio without full transcription.
//!
//! Model: sherpa-onnx-kws-zipformer-gigaspeech-3.3M (~5MB INT8).
//! Requires `voice` feature.

#[cfg(feature = "voice")]
use std::path::Path;

use crate::config::VoiceConfig;

/// KWS engine error.
#[derive(Debug)]
pub enum KwsError {
    ModelNotFound(String),
    InitError(String),
    InferenceError(String),
    #[allow(dead_code)]
    FeatureDisabled,
}

impl std::fmt::Display for KwsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KwsError::ModelNotFound(p) => write!(f, "KWS model not found: {}", p),
            KwsError::InitError(e) => write!(f, "KWS init: {}", e),
            KwsError::InferenceError(e) => write!(f, "KWS inference: {}", e),
            KwsError::FeatureDisabled => write!(f, "voice feature not enabled"),
        }
    }
}

/// Result of PII keyword detection on an audio segment.
#[derive(Debug, Clone)]
pub struct KwsResult {
    /// Whether any PII-related keywords were detected.
    pub pii_detected: bool,
    /// List of detected keyword phrases.
    pub keywords_found: Vec<String>,
    /// KWS inference time in milliseconds (excludes audio decode).
    pub inference_ms: u64,
}

/// Keyword spotter engine for PII detection in audio.
///
/// Uses the sherpa-onnx C API directly to fix two issues with the Rust wrapper:
/// 1. Stream reuse: the wrapper's stream dies after `extract_keyword` (InputFinished
///    is permanent). We create a fresh stream per `detect_pii` call.
/// 2. Multi-keyword detection: the wrapper calls `GetKeywordResult` once and never
///    calls `ResetKeywordStream`. We properly collect all keywords.
#[cfg(feature = "voice")]
pub struct KwsEngine {
    /// Raw pointer to the sherpa-onnx keyword spotter (model stays loaded).
    spotter: *const sherpa_rs_sys::SherpaOnnxKeywordSpotter,
    /// Keep CStrings alive for the lifetime of the engine (sherpa holds raw pointers).
    _strings: Vec<std::ffi::CString>,
}

#[cfg(not(feature = "voice"))]
pub struct KwsEngine {
    _phantom: std::marker::PhantomData<()>,
}

#[cfg(feature = "voice")]
impl KwsEngine {
    /// Create a new KWS engine from voice configuration.
    ///
    /// Loads the Zipformer ONNX models (encoder/decoder/joiner),
    /// tokens, and PII keywords file.
    pub fn new(config: &VoiceConfig) -> Result<Self, KwsError> {
        use sherpa_rs_sys::*;
        use std::ffi::CString;

        let model_dir = &config.kws_model_dir;

        // Resolve model file paths
        let encoder = find_model_file(model_dir, "encoder", ".int8.onnx")
            .or_else(|| find_model_file(model_dir, "encoder", ".onnx"))
            .ok_or_else(|| KwsError::ModelNotFound(format!("{}/encoder*.onnx", model_dir)))?;

        let decoder = find_model_file(model_dir, "decoder", ".int8.onnx")
            .or_else(|| find_model_file(model_dir, "decoder", ".onnx"))
            .ok_or_else(|| KwsError::ModelNotFound(format!("{}/decoder*.onnx", model_dir)))?;

        let joiner = find_model_file(model_dir, "joiner", ".int8.onnx")
            .or_else(|| find_model_file(model_dir, "joiner", ".onnx"))
            .ok_or_else(|| KwsError::ModelNotFound(format!("{}/joiner*.onnx", model_dir)))?;

        let tokens = Path::new(model_dir).join("tokens.txt");
        if !tokens.exists() {
            return Err(KwsError::ModelNotFound(tokens.display().to_string()));
        }

        let keywords_file = Path::new(&config.kws_keywords_file);
        if !keywords_file.exists() {
            return Err(KwsError::ModelNotFound(keywords_file.display().to_string()));
        }

        // Build CStrings that must outlive the spotter
        let c_encoder = CString::new(encoder).map_err(|e| KwsError::InitError(e.to_string()))?;
        let c_decoder = CString::new(decoder).map_err(|e| KwsError::InitError(e.to_string()))?;
        let c_joiner = CString::new(joiner).map_err(|e| KwsError::InitError(e.to_string()))?;
        let c_tokens = CString::new(tokens.display().to_string())
            .map_err(|e| KwsError::InitError(e.to_string()))?;
        let c_keywords = CString::new(keywords_file.display().to_string())
            .map_err(|e| KwsError::InitError(e.to_string()))?;
        let c_provider = CString::new("cpu").map_err(|e| KwsError::InitError(e.to_string()))?;
        let c_empty = CString::new("").map_err(|e| KwsError::InitError(e.to_string()))?;

        // Build the config struct — zero-initialize, then set the fields we need
        let kws_config = SherpaOnnxKeywordSpotterConfig {
            feat_config: SherpaOnnxFeatureConfig {
                sample_rate: 16000,
                feature_dim: 80,
            },
            model_config: SherpaOnnxOnlineModelConfig {
                transducer: SherpaOnnxOnlineTransducerModelConfig {
                    encoder: c_encoder.as_ptr(),
                    decoder: c_decoder.as_ptr(),
                    joiner: c_joiner.as_ptr(),
                },
                paraformer: SherpaOnnxOnlineParaformerModelConfig {
                    encoder: std::ptr::null(),
                    decoder: std::ptr::null(),
                },
                zipformer2_ctc: SherpaOnnxOnlineZipformer2CtcModelConfig {
                    model: std::ptr::null(),
                },
                tokens: c_tokens.as_ptr(),
                num_threads: 1,
                provider: c_provider.as_ptr(),
                debug: 0,
                model_type: c_empty.as_ptr(),
                modeling_unit: c_empty.as_ptr(),
                bpe_vocab: c_empty.as_ptr(),
                tokens_buf: std::ptr::null(),
                tokens_buf_size: 0,
                nemo_ctc: SherpaOnnxOnlineNemoCtcModelConfig {
                    model: std::ptr::null(),
                },
            },
            max_active_paths: 4,
            num_trailing_blanks: 1,
            keywords_score: config.kws_score,
            keywords_threshold: config.kws_threshold,
            keywords_file: c_keywords.as_ptr(),
            keywords_buf: std::ptr::null(),
            keywords_buf_size: 0,
        };

        let spotter = unsafe { SherpaOnnxCreateKeywordSpotter(&kws_config) };
        if spotter.is_null() {
            return Err(KwsError::InitError(
                "SherpaOnnxCreateKeywordSpotter returned null".to_string(),
            ));
        }

        // Keep CStrings alive — sherpa holds raw pointers into them
        let strings = vec![
            c_encoder, c_decoder, c_joiner, c_tokens, c_keywords, c_provider, c_empty,
        ];

        Ok(Self {
            spotter,
            _strings: strings,
        })
    }

    /// Detect PII keywords in PCM audio samples.
    ///
    /// Accepts mono f32 samples at ANY sample rate — sherpa-onnx resamples
    /// internally if `sample_rate` differs from the model's expected 16kHz.
    ///
    /// Creates a fresh stream per call (fixing the stream reuse bug in
    /// sherpa-rs) and collects ALL detected keywords (fixing the single-
    /// keyword limitation).
    pub fn detect_pii(
        &self,
        pcm_samples: Vec<f32>,
        sample_rate: u32,
    ) -> Result<KwsResult, KwsError> {
        use sherpa_rs_sys::*;

        // Create a fresh stream for this audio
        let stream = unsafe { SherpaOnnxCreateKeywordStream(self.spotter) };
        if stream.is_null() {
            return Err(KwsError::InferenceError(
                "SherpaOnnxCreateKeywordStream returned null".to_string(),
            ));
        }

        let infer_start = std::time::Instant::now();

        // Feed audio + decode inside catch_unwind to recover from sherpa-onnx panics.
        // The stream pointer is raw and must be destroyed regardless of outcome.
        let spotter = self.spotter;
        let decode_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            unsafe {
                SherpaOnnxOnlineStreamAcceptWaveform(
                    stream,
                    sample_rate as i32,
                    pcm_samples.as_ptr(),
                    pcm_samples.len() as i32,
                );
                SherpaOnnxOnlineStreamInputFinished(stream);
            }

            let mut keywords = Vec::new();

            unsafe {
                while SherpaOnnxIsKeywordStreamReady(spotter, stream) == 1 {
                    SherpaOnnxDecodeKeywordStream(spotter, stream);

                    let result_ptr = SherpaOnnxGetKeywordResult(spotter, stream);
                    if !result_ptr.is_null() {
                        let keyword_ptr = (*result_ptr).keyword;
                        if !keyword_ptr.is_null() {
                            let keyword_cstr = std::ffi::CStr::from_ptr(keyword_ptr);
                            let keyword = keyword_cstr.to_string_lossy().to_string();
                            if !keyword.is_empty() {
                                keywords.push(keyword);
                                SherpaOnnxResetKeywordStream(spotter, stream);
                            }
                        }
                        SherpaOnnxDestroyKeywordResult(result_ptr);
                    }
                }
            }

            keywords
        }));

        // Always destroy stream (even after panic recovery)
        unsafe {
            SherpaOnnxDestroyOnlineStream(stream);
        }

        let inference_ms = infer_start.elapsed().as_millis() as u64;

        match decode_result {
            Ok(keywords) => Ok(KwsResult {
                pii_detected: !keywords.is_empty(),
                keywords_found: keywords,
                inference_ms,
            }),
            Err(_) => Err(KwsError::InferenceError(
                "sherpa-onnx panicked during keyword detection".to_string(),
            )),
        }
    }
}

#[cfg(feature = "voice")]
impl Drop for KwsEngine {
    fn drop(&mut self) {
        unsafe {
            sherpa_rs_sys::SherpaOnnxDestroyKeywordSpotter(self.spotter);
        }
    }
}

#[cfg(not(feature = "voice"))]
impl KwsEngine {
    pub fn new(_config: &VoiceConfig) -> Result<Self, KwsError> {
        Err(KwsError::FeatureDisabled)
    }

    pub fn detect_pii(
        &self,
        _pcm_samples: Vec<f32>,
        _sample_rate: u32,
    ) -> Result<KwsResult, KwsError> {
        Err(KwsError::FeatureDisabled)
    }
}

// KwsEngine is safe to share across threads:
// - The spotter pointer is only used via SherpaOnnxCreateKeywordStream (creates
//   independent streams) and the model weights are read-only after init.
// - Each detect_pii call creates and destroys its own stream.
// - Multiple threads can call detect_pii concurrently since each gets its own stream.
unsafe impl Send for KwsEngine {}
unsafe impl Sync for KwsEngine {}

/// Find an ONNX model file in a directory by prefix and suffix.
#[cfg(feature = "voice")]
fn find_model_file(dir: &str, prefix: &str, suffix: &str) -> Option<String> {
    let dir_path = Path::new(dir);
    if !dir_path.is_dir() {
        return None;
    }

    if let Ok(entries) = std::fs::read_dir(dir_path) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.contains(prefix) && name_str.ends_with(suffix) {
                return Some(entry.path().display().to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kws_result_no_pii() {
        let result = KwsResult {
            pii_detected: false,
            keywords_found: Vec::new(),
            inference_ms: 0,
        };
        assert!(!result.pii_detected);
        assert!(result.keywords_found.is_empty());
    }

    #[test]
    fn test_kws_result_with_pii() {
        let result = KwsResult {
            pii_detected: true,
            keywords_found: vec!["social security".to_string()],
            inference_ms: 0,
        };
        assert!(result.pii_detected);
        assert_eq!(result.keywords_found.len(), 1);
    }

    #[test]
    fn test_kws_error_display() {
        let err = KwsError::ModelNotFound("/path/to/model".to_string());
        assert!(err.to_string().contains("model not found"));
    }

    #[cfg(feature = "voice")]
    #[test]
    fn test_find_model_file_missing_dir() {
        assert!(find_model_file("/nonexistent/path", "encoder", ".onnx").is_none());
    }

    #[test]
    fn test_kws_engine_missing_models() {
        let config = VoiceConfig {
            enabled: true,
            kws_model_dir: "/nonexistent/models/kws".to_string(),
            kws_keywords_file: "/nonexistent/keywords.txt".to_string(),
            kws_threshold: 0.1,
            kws_score: 3.0,
        };
        let result = KwsEngine::new(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_kws_catch_unwind_pattern() {
        let result: Result<Vec<String>, KwsError> =
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Vec<String> {
                panic!("simulated sherpa panic");
            })) {
                Ok(keywords) => Ok(keywords),
                Err(_) => Err(KwsError::InferenceError(
                    "sherpa-onnx panicked during keyword detection".to_string(),
                )),
            };
        assert!(result.is_err());
        assert!(matches!(&result, Err(KwsError::InferenceError(msg)) if msg.contains("panicked")));
    }
}

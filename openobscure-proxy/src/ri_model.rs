//! R2 Response Integrity model — TinyBERT multi-label classifier.
//!
//! Loads a fine-tuned TinyBERT ONNX model that classifies text into 4
//! EU AI Act Article 5 manipulation categories:
//!   [0] Art_5_1_a_Deceptive
//!   [1] Art_5_1_b_Age
//!   [2] Art_5_1_b_SocioEcon
//!   [3] Art_5_1_c_Social_Scoring
//!
//! The model outputs raw logits; sigmoid is applied here. Scores above the
//! configured threshold are considered positive detections.
//!
//! Supports first-window early exit: if the maximum sigmoid score on the first
//! 128 tokens is below `early_exit_threshold`, full-sequence inference is skipped.

use std::path::Path;

use ndarray::Array2;
use ort::session::Session;

use crate::wordpiece::WordPieceTokenizer;

/// The 4 Article 5 categories detected by R2.
pub const R2_CATEGORIES: [&str; 4] = [
    "Art_5_1_a_Deceptive",
    "Art_5_1_b_Age",
    "Art_5_1_b_SocioEcon",
    "Art_5_1_c_Social_Scoring",
];

/// Number of output labels.
pub const NUM_LABELS: usize = 4;

/// Maximum sequence length for the model (TinyBERT context window).
const MAX_SEQ_LEN: usize = 512;

/// First-window token count for early exit.
const EARLY_EXIT_WINDOW: usize = 128;

/// R2 classification result for a single text.
#[derive(Debug, Clone)]
pub struct RiPrediction {
    /// Raw sigmoid scores for each of the 4 categories.
    pub scores: [f32; NUM_LABELS],
    /// Binary predictions (score >= threshold).
    pub labels: [bool; NUM_LABELS],
    /// Whether early exit was triggered (max score below early_exit_threshold).
    pub early_exit: bool,
    /// Inference time in microseconds.
    pub inference_time_us: u64,
}

impl RiPrediction {
    /// True if any category is positively predicted.
    pub fn is_flagged(&self) -> bool {
        self.labels.iter().any(|&l| l)
    }

    /// Names of the positively predicted categories.
    pub fn flagged_categories(&self) -> Vec<&'static str> {
        self.labels
            .iter()
            .enumerate()
            .filter(|(_, &l)| l)
            .map(|(i, _)| R2_CATEGORIES[i])
            .collect()
    }

    /// Maximum score across all categories.
    pub fn max_score(&self) -> f32 {
        self.scores
            .iter()
            .cloned()
            .fold(f32::NEG_INFINITY, f32::max)
    }
}

/// R2 TinyBERT ONNX model for multi-label classification.
pub struct RiModel {
    session: Session,
    tokenizer: WordPieceTokenizer,
    threshold: f32,
    early_exit_threshold: f32,
}

impl RiModel {
    /// Load R2 model and tokenizer from a directory containing:
    /// - `model_int8.onnx` (or `model.onnx`)
    /// - `vocab.txt`
    pub fn load(
        model_dir: &Path,
        threshold: f32,
        early_exit_threshold: f32,
    ) -> Result<Self, RiModelError> {
        // Prefer FP32 model (better accuracy); fall back to INT8 if available
        let model_path = model_dir.join("model.onnx");
        let model_path = if model_path.exists() {
            model_path
        } else {
            let fallback = model_dir.join("model_int8.onnx");
            if fallback.exists() {
                fallback
            } else {
                return Err(RiModelError::ModelNotFound(model_dir.display().to_string()));
            }
        };

        let vocab_path = model_dir.join("vocab.txt");
        let tokenizer = WordPieceTokenizer::from_file(&vocab_path)
            .map_err(|e| RiModelError::Tokenizer(e.to_string()))?;

        let session = crate::ort_ep::build_session(&model_path)
            .map_err(|e| RiModelError::OnnxRuntime(e.to_string()))?;

        oo_info!(crate::oo_log::modules::RESPONSE_INTEGRITY, "R2 model loaded",
            model = %model_path.display(),
            vocab_size = tokenizer.vocab_size(),
            threshold = threshold,
            early_exit_threshold = early_exit_threshold);

        Ok(Self {
            session,
            tokenizer,
            threshold,
            early_exit_threshold,
        })
    }

    /// Classify text. Returns prediction with scores and binary labels.
    ///
    /// Uses first-window early exit: tokenizes the full text, but first runs
    /// inference on only the first `EARLY_EXIT_WINDOW` tokens. If the max
    /// sigmoid score is below `early_exit_threshold`, returns immediately
    /// with all-negative labels. Otherwise, runs full-sequence inference.
    pub fn predict(&mut self, text: &str) -> Result<RiPrediction, RiModelError> {
        if text.is_empty() {
            return Ok(RiPrediction {
                scores: [0.0; NUM_LABELS],
                labels: [false; NUM_LABELS],
                early_exit: true,
                inference_time_us: 0,
            });
        }

        let start = std::time::Instant::now();

        // Tokenize full text
        let encoded = self.tokenizer.tokenize(text);
        let full_len = encoded.input_ids.len().min(MAX_SEQ_LEN);

        // First-window early exit check
        if full_len > EARLY_EXIT_WINDOW + 2 {
            // +2 for [CLS] and [SEP]
            let window_len = EARLY_EXIT_WINDOW + 2; // Include [CLS] at start
            let window_scores = self.run_inference(
                &encoded.input_ids[..window_len],
                &encoded.attention_mask[..window_len],
                &encoded.token_type_ids[..window_len],
            )?;

            let max_score = window_scores
                .iter()
                .cloned()
                .fold(f32::NEG_INFINITY, f32::max);

            if max_score < self.early_exit_threshold {
                let elapsed = start.elapsed().as_micros() as u64;
                return Ok(RiPrediction {
                    scores: window_scores,
                    labels: [false; NUM_LABELS],
                    early_exit: true,
                    inference_time_us: elapsed,
                });
            }
        }

        // Full-sequence inference
        let scores = self.run_inference(
            &encoded.input_ids[..full_len],
            &encoded.attention_mask[..full_len],
            &encoded.token_type_ids[..full_len],
        )?;

        let labels = [
            scores[0] >= self.threshold,
            scores[1] >= self.threshold,
            scores[2] >= self.threshold,
            scores[3] >= self.threshold,
        ];

        let elapsed = start.elapsed().as_micros() as u64;

        Ok(RiPrediction {
            scores,
            labels,
            early_exit: false,
            inference_time_us: elapsed,
        })
    }

    /// Run raw ONNX inference on token sequences. Returns sigmoid scores.
    fn run_inference(
        &mut self,
        input_ids: &[i64],
        attention_mask: &[i64],
        token_type_ids: &[i64],
    ) -> Result<[f32; NUM_LABELS], RiModelError> {
        let seq_len = input_ids.len();

        let input_ids_arr = Array2::from_shape_vec((1, seq_len), input_ids.to_vec())
            .map_err(|e| RiModelError::Shape(e.to_string()))?;
        let attention_mask_arr = Array2::from_shape_vec((1, seq_len), attention_mask.to_vec())
            .map_err(|e| RiModelError::Shape(e.to_string()))?;
        let token_type_ids_arr = Array2::from_shape_vec((1, seq_len), token_type_ids.to_vec())
            .map_err(|e| RiModelError::Shape(e.to_string()))?;

        let input_ids_val = ort::value::Value::from_array(input_ids_arr)
            .map_err(|e| RiModelError::OnnxRuntime(e.to_string()))?;
        let attention_mask_val = ort::value::Value::from_array(attention_mask_arr)
            .map_err(|e| RiModelError::OnnxRuntime(e.to_string()))?;
        let token_type_ids_val = ort::value::Value::from_array(token_type_ids_arr)
            .map_err(|e| RiModelError::OnnxRuntime(e.to_string()))?;

        // Panic-safe inference
        let outputs = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.session.run(ort::inputs![
                "input_ids" => input_ids_val,
                "attention_mask" => attention_mask_val,
                "token_type_ids" => token_type_ids_val,
            ])
        })) {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => return Err(RiModelError::OnnxRuntime(e.to_string())),
            Err(_) => {
                return Err(RiModelError::OnnxRuntime(
                    "ONNX Runtime panicked during R2 inference".to_string(),
                ))
            }
        };

        // Extract logits: shape [1, NUM_LABELS]
        let (_logits_shape, logits_data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e: ort::Error| RiModelError::OnnxRuntime(e.to_string()))?;

        // Apply sigmoid to convert logits to probabilities
        let mut scores = [0.0f32; NUM_LABELS];
        for i in 0..NUM_LABELS.min(logits_data.len()) {
            scores[i] = sigmoid(logits_data[i]);
        }

        Ok(scores)
    }

    /// Run a warm-up inference to prime the model. Returns warm-up duration.
    pub fn warm(&mut self) -> std::time::Duration {
        let start = std::time::Instant::now();
        let _ = self.predict("This is a warm-up sentence for the R2 model.");
        start.elapsed()
    }
}

/// Sigmoid activation function.
#[inline]
fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

#[derive(Debug, thiserror::Error)]
pub enum RiModelError {
    #[error("R2 model not found in: {0}")]
    ModelNotFound(String),
    #[error("ONNX Runtime error: {0}")]
    OnnxRuntime(String),
    #[error("Tokenizer error: {0}")]
    Tokenizer(String),
    #[error("Tensor shape error: {0}")]
    Shape(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sigmoid() {
        assert!((sigmoid(0.0) - 0.5).abs() < 1e-6);
        assert!(sigmoid(10.0) > 0.999);
        assert!(sigmoid(-10.0) < 0.001);
        assert!((sigmoid(1.0) - 0.7310586).abs() < 1e-5);
    }

    #[test]
    fn test_r2_categories() {
        assert_eq!(R2_CATEGORIES.len(), 4);
        assert_eq!(R2_CATEGORIES[0], "Art_5_1_a_Deceptive");
        assert_eq!(R2_CATEGORIES[1], "Art_5_1_b_Age");
        assert_eq!(R2_CATEGORIES[2], "Art_5_1_b_SocioEcon");
        assert_eq!(R2_CATEGORIES[3], "Art_5_1_c_Social_Scoring");
    }

    #[test]
    fn test_prediction_is_flagged() {
        let pred = RiPrediction {
            scores: [0.1, 0.2, 0.3, 0.4],
            labels: [false, false, false, false],
            early_exit: false,
            inference_time_us: 0,
        };
        assert!(!pred.is_flagged());

        let pred = RiPrediction {
            scores: [0.9, 0.2, 0.3, 0.4],
            labels: [true, false, false, false],
            early_exit: false,
            inference_time_us: 0,
        };
        assert!(pred.is_flagged());
    }

    #[test]
    fn test_prediction_flagged_categories() {
        let pred = RiPrediction {
            scores: [0.9, 0.1, 0.8, 0.2],
            labels: [true, false, true, false],
            early_exit: false,
            inference_time_us: 0,
        };
        let cats = pred.flagged_categories();
        assert_eq!(cats.len(), 2);
        assert!(cats.contains(&"Art_5_1_a_Deceptive"));
        assert!(cats.contains(&"Art_5_1_b_SocioEcon"));
    }

    #[test]
    fn test_prediction_max_score() {
        let pred = RiPrediction {
            scores: [0.1, 0.9, 0.3, 0.4],
            labels: [false, true, false, false],
            early_exit: false,
            inference_time_us: 0,
        };
        assert!((pred.max_score() - 0.9).abs() < 1e-6);
    }

    #[test]
    fn test_prediction_empty_text() {
        // Verify RiPrediction for empty text scenario
        let pred = RiPrediction {
            scores: [0.0; NUM_LABELS],
            labels: [false; NUM_LABELS],
            early_exit: true,
            inference_time_us: 0,
        };
        assert!(!pred.is_flagged());
        assert!(pred.flagged_categories().is_empty());
        assert!(pred.early_exit);
    }

    #[test]
    fn test_model_not_found() {
        let result = RiModel::load(Path::new("/nonexistent/r2_model"), 0.7, 0.3);
        assert!(result.is_err());
        assert!(matches!(&result, Err(RiModelError::ModelNotFound(_))));
    }

    #[test]
    fn test_catch_unwind_r2_panic_to_error() {
        let result: Result<(), RiModelError> = match std::panic::catch_unwind(
            std::panic::AssertUnwindSafe(|| -> Result<(), String> {
                panic!("simulated R2 panic");
            }),
        ) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(RiModelError::OnnxRuntime(e)),
            Err(_) => Err(RiModelError::OnnxRuntime(
                "ONNX Runtime panicked during R2 inference".to_string(),
            )),
        };
        assert!(result.is_err());
        assert!(matches!(&result, Err(RiModelError::OnnxRuntime(msg)) if msg.contains("panicked")));
    }
}

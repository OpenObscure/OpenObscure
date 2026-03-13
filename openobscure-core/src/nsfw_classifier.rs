//! NSFW image classifier supporting two model architectures:
//!
//! **5-class ViT-base** (`nsfw_5class_int8.onnx`, 87MB INT8):
//!   - Input: [1, 3, 224, 224] NCHW, float32, mean=0.5 std=0.5
//!   - Output: [1, 5] logits — drawings / hentai / neutral / porn / sexy
//!   - NSFW score = P(hentai) + P(porn) + P(sexy)
//!
//! **Binary ViT-tiny** (`nsfw_classifier.onnx`, 22MB FP32):
//!   - Input: dynamic shape — expects 384×384 (pos_embed [1,577,192])
//!   - Output: [1, 2] logits — [safe, nsfw]
//!   - NSFW score = P(nsfw) = prob[1] after softmax
//!
//! The correct input size and output interpretation are detected automatically
//! from the loaded session's metadata at load time.

use std::path::Path;

use image::{DynamicImage, GenericImageView};
use ndarray::Array4;
use ort::session::Session;

use crate::image_pipeline::ImageError;

/// ImageNet-style normalization used by both models.
const NORM_MEAN: [f32; 3] = [0.5, 0.5, 0.5];
const NORM_STD: [f32; 3] = [0.5, 0.5, 0.5];

/// 5-class output class indices.
const IDX_HENTAI: usize = 1;
const IDX_PORN: usize = 3;
const IDX_SEXY: usize = 4;

/// 5-class output class names.
const CLASS_NAMES_5: [&str; 5] = ["drawings", "hentai", "neutral", "porn", "sexy"];
/// Binary output class names (model outputs [safe_logit, nsfw_logit]).
const CLASS_NAMES_2: [&str; 2] = ["safe", "nsfw"];

/// NSFW image classifier — auto-adapts to 5-class or binary model.
pub struct NsfwClassifier {
    session: Session,
    threshold: f32,
    /// Pixel size of the square input the model expects (224 or 384).
    input_size: u32,
    /// Number of output logits (5 or 2).
    num_classes: usize,
}

/// Result of NSFW classification.
#[derive(Debug, Clone)]
pub struct ClassifierResult {
    /// Whether the image is classified as NSFW (nsfw_score >= threshold).
    pub is_nsfw: bool,
    /// NSFW probability: P(hentai)+P(porn)+P(sexy) for 5-class, or P(nsfw) for binary.
    pub nsfw_score: f32,
    /// Name of the top-scoring class.
    pub top_class: String,
    /// Per-class probabilities (5 or 2 values, zero-padded to 5 for callers).
    pub class_probs: [f32; 5],
}

impl NsfwClassifier {
    /// Load an NSFW classifier from a model directory.
    ///
    /// Automatically detects model type (5-class or binary) and expected input
    /// size (224 or 384) from the session's input/output metadata.
    pub fn load(model_dir: &Path, threshold: f32) -> Result<Self, ImageError> {
        let model_path = find_model_file(model_dir)?;

        let session = crate::ort_ep::build_session(&model_path)
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        // Detect output class count from the first output dimension.
        let num_classes = detect_num_classes(&session);

        // Detect expected input size from the first input's static dimensions.
        // - 5-class INT8 model has fixed input [batch, 3, 224, 224].
        // - Binary FP32 model has dynamic input but pos_embed [1,577,192] → 384×384.
        let input_size = detect_input_size(&session);

        oo_info!(crate::oo_log::modules::IMAGE, "NSFW classifier loaded",
            model = %model_path.display(),
            input_size = input_size,
            num_classes = num_classes,
            threshold = threshold);

        Ok(Self {
            session,
            threshold,
            input_size,
            num_classes,
        })
    }

    /// Classify an image for NSFW content.
    pub fn classify(&mut self, img: &DynamicImage) -> Result<ClassifierResult, ImageError> {
        let sz = self.input_size;

        // Resize shortest edge to sz, center-crop to sz×sz.
        let (w, h) = img.dimensions();
        let resized = if w == h {
            img.resize_exact(sz, sz, image::imageops::FilterType::CatmullRom)
        } else {
            let scale = sz as f32 / w.min(h) as f32;
            let new_w = (w as f32 * scale).round() as u32;
            let new_h = (h as f32 * scale).round() as u32;
            let scaled = img.resize_exact(new_w, new_h, image::imageops::FilterType::CatmullRom);
            let crop_x = (new_w.saturating_sub(sz)) / 2;
            let crop_y = (new_h.saturating_sub(sz)) / 2;
            scaled.crop_imm(crop_x, crop_y, sz, sz)
        };

        // Build [1, 3, sz, sz] NCHW tensor: (pixel/255 - 0.5) / 0.5
        let input =
            Array4::<f32>::from_shape_fn((1, 3, sz as usize, sz as usize), |(_, c, h, w)| {
                let pixel = resized.get_pixel(w as u32, h as u32);
                (pixel[c] as f32 / 255.0 - NORM_MEAN[c]) / NORM_STD[c]
            });

        let input_val = ort::value::Value::from_array(input)
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        let input_name = self.session.inputs()[0].name().to_string();
        let outputs = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.session
                .run(ort::inputs![input_name.as_str() => input_val])
        })) {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => return Err(ImageError::OnnxRuntime(e.to_string())),
            Err(_) => {
                return Err(ImageError::OnnxRuntime(
                    "ONNX Runtime panicked during NSFW classifier inference".to_string(),
                ))
            }
        };

        let (_shape, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        if data.len() < self.num_classes {
            return Err(ImageError::OnnxRuntime(format!(
                "Expected {} output logits, got {}",
                self.num_classes,
                data.len()
            )));
        }

        let logits: Vec<f32> = data[..self.num_classes].to_vec();
        let probs = softmax(&logits);

        let (nsfw_score, top_class, class_probs) = if self.num_classes == 2 {
            // Binary model: logits = [safe_logit, nsfw_logit]
            let score = probs[1]; // P(nsfw)
            let top_name = if probs[1] >= probs[0] {
                CLASS_NAMES_2[1]
            } else {
                CLASS_NAMES_2[0]
            };
            let mut cp = [0.0f32; 5];
            cp[0] = probs[0]; // safe → slot 0
            cp[1] = probs[1]; // nsfw → slot 1
            (score, top_name.to_string(), cp)
        } else {
            // 5-class model: hentai + porn + sexy
            let score = probs[IDX_HENTAI] + probs[IDX_PORN] + probs[IDX_SEXY];
            let top_idx = probs
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i)
                .unwrap_or(2); // default neutral
            let mut cp = [0.0f32; 5];
            cp.copy_from_slice(&probs[..5]);
            (score, CLASS_NAMES_5[top_idx].to_string(), cp)
        };

        Ok(ClassifierResult {
            is_nsfw: nsfw_score >= self.threshold,
            nsfw_score,
            top_class,
            class_probs,
        })
    }
}

/// Detect the expected input size from session metadata.
///
/// The 5-class INT8 model has a fixed input shape [batch, 3, 224, 224].
/// The binary FP32 model has dynamic shapes but needs 384×384 (its positional
/// embedding is [1, 577, 192] → 576 = 24×24 patches → 24 × 16px = 384).
fn detect_input_size(session: &Session) -> u32 {
    if let Some(input) = session.inputs().first() {
        if let ort::value::ValueType::Tensor { ref shape, .. } = *input.dtype() {
            // Fixed shape [batch, 3, H, W] → use H (index 2), positive means fixed dim
            if shape.len() == 4 && shape[2] > 0 {
                return shape[2] as u32;
            }
        }
    }
    // Dynamic input — use 384 (binary ViT-tiny model default)
    384
}

/// Detect the number of output classes from session metadata.
fn detect_num_classes(session: &Session) -> usize {
    if let Some(output) = session.outputs().first() {
        if let ort::value::ValueType::Tensor { ref shape, .. } = *output.dtype() {
            // Output shape [batch, num_classes] → use index 1
            if shape.len() == 2 {
                let n = shape[1];
                if n == 2 || n == 5 {
                    return n as usize;
                }
            }
        }
    }
    // Default to 5-class
    5
}

/// Softmax over a slice of logits.
fn softmax(logits: &[f32]) -> Vec<f32> {
    let max_val = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = logits.iter().map(|&x| (x - max_val).exp()).collect();
    let sum: f32 = exps.iter().sum();
    exps.iter().map(|&e| e / sum).collect()
}

/// Find a classifier model file in the given directory.
fn find_model_file(model_dir: &Path) -> Result<std::path::PathBuf, ImageError> {
    let candidates = [
        "nsfw_5class_int8.onnx",
        "nsfw_5class.onnx",
        "model.onnx",
        "nsfw_classifier.onnx",
    ];
    for name in &candidates {
        let path = model_dir.join(name);
        if path.exists() {
            return Ok(path);
        }
    }
    Err(ImageError::Decode(format!(
        "No NSFW classifier model found in {}",
        model_dir.display()
    )))
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_softmax_sum_to_one() {
        let probs = softmax(&[1.0, 2.0, 0.5, 3.0, 0.1]);
        let sum: f32 = probs.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6, "softmax must sum to 1, got {sum}");
    }

    #[test]
    fn test_softmax_uniform() {
        let probs = softmax(&[0.0; 5]);
        for p in &probs {
            assert!((*p - 0.2).abs() < 1e-6);
        }
    }

    #[test]
    fn test_softmax_numerical_stability() {
        let probs = softmax(&[1000.0, 1001.0, 999.0, 1002.0, 998.0]);
        let sum: f32 = probs.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_nsfw_score_5class() {
        let probs: [f32; 5] = [0.01, 0.05, 0.04, 0.85, 0.05]; // draw, hentai, neutral, porn, sexy
        let nsfw_score = probs[IDX_HENTAI] + probs[IDX_PORN] + probs[IDX_SEXY];
        assert!((nsfw_score - 0.95).abs() < 1e-6);
    }

    #[test]
    fn test_nsfw_score_5class_safe() {
        let probs = [0.01, 0.001, 0.98, 0.005, 0.004];
        let nsfw_score = probs[IDX_HENTAI] + probs[IDX_PORN] + probs[IDX_SEXY];
        assert!(nsfw_score < 0.05);
    }

    #[test]
    fn test_load_model_not_found() {
        let result = NsfwClassifier::load(Path::new("/nonexistent"), 0.5);
        assert!(result.is_err());
    }

    #[test]
    fn test_classifier_result_threshold() {
        let result = ClassifierResult {
            is_nsfw: true,
            nsfw_score: 0.80,
            top_class: "porn".to_string(),
            class_probs: [0.0, 0.1, 0.1, 0.7, 0.1],
        };
        assert!(result.is_nsfw);
        assert!((result.nsfw_score - 0.80).abs() < 1e-6);
    }
}

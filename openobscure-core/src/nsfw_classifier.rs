//! NSFW image classifier using ViT-base 5-class ONNX model.
//!
//! Single-model NSFW detection replacing the previous two-model cascade
//! (NudeNet 320n detector + ViT-tiny binary classifier). The ViT-base model
//! provides holistic image classification into 5 categories and catches both
//! explicit nudity and semi-nude/suggestive content that region-based detectors miss.
//!
//! Model: LukeJacob2023/nsfw-image-detector (ViT-base-patch16-224, Apache 2.0)
//! Input: [1, 3, 224, 224] NCHW, float32 normalized with mean=0.5, std=0.5
//! Output: [1, 5] logits — 5 classes:
//!   0 = drawings  (safe cartoon/art)
//!   1 = hentai    (NSFW drawn)
//!   2 = neutral   (safe photo)
//!   3 = porn      (explicit)
//!   4 = sexy      (semi-nude / suggestive)
//!
//! NSFW score = P(hentai) + P(porn) + P(sexy) — sum of all NSFW class probabilities.

use std::path::Path;

use image::{DynamicImage, GenericImageView};
use ndarray::Array4;
use ort::session::Session;

use crate::image_pipeline::ImageError;

/// ViT-base input size (224×224).
const INPUT_SIZE: u32 = 224;

/// ImageNet-style normalization: mean=0.5, std=0.5 per channel.
const NORM_MEAN: [f32; 3] = [0.5, 0.5, 0.5];
const NORM_STD: [f32; 3] = [0.5, 0.5, 0.5];

/// Number of output classes.
const NUM_CLASSES: usize = 5;

/// Output class indices.
#[allow(dead_code)]
const IDX_DRAWINGS: usize = 0;
const IDX_HENTAI: usize = 1;
const IDX_NEUTRAL: usize = 2;
const IDX_PORN: usize = 3;
const IDX_SEXY: usize = 4;

/// NSFW class names for logging/metadata.
const CLASS_NAMES: [&str; NUM_CLASSES] = ["drawings", "hentai", "neutral", "porn", "sexy"];

/// NSFW image classifier.
pub struct NsfwClassifier {
    session: Session,
    threshold: f32,
}

/// Result of 5-class NSFW classification.
#[derive(Debug, Clone)]
pub struct ClassifierResult {
    /// Whether the image is classified as NSFW (nsfw_score >= threshold).
    pub is_nsfw: bool,
    /// Combined P(hentai) + P(porn) + P(sexy).
    pub nsfw_score: f32,
    /// Name of the top-scoring class.
    pub top_class: String,
    /// Per-class probabilities [drawings, hentai, neutral, porn, sexy].
    pub class_probs: [f32; NUM_CLASSES],
}

impl NsfwClassifier {
    /// Load the ViT-base NSFW classifier from a model directory.
    pub fn load(model_dir: &Path, threshold: f32) -> Result<Self, ImageError> {
        let model_path = find_model_file(model_dir)?;

        let session = crate::ort_ep::build_session(&model_path)
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        oo_info!(crate::oo_log::modules::IMAGE, "NSFW classifier loaded",
            model = %model_path.display(),
            threshold = threshold);

        Ok(Self { session, threshold })
    }

    /// Classify an image into 5 NSFW categories.
    pub fn classify(&mut self, img: &DynamicImage) -> Result<ClassifierResult, ImageError> {
        // Preprocessing: resize to 224×224, normalize with mean=0.5, std=0.5
        let (w, h) = img.dimensions();
        let resized = if w == h {
            img.resize_exact(
                INPUT_SIZE,
                INPUT_SIZE,
                image::imageops::FilterType::CatmullRom,
            )
        } else {
            // Resize shortest edge to INPUT_SIZE, preserving aspect ratio, then center crop
            let scale = INPUT_SIZE as f32 / w.min(h) as f32;
            let new_w = (w as f32 * scale).round() as u32;
            let new_h = (h as f32 * scale).round() as u32;
            let scaled = img.resize_exact(new_w, new_h, image::imageops::FilterType::CatmullRom);

            let crop_x = (new_w.saturating_sub(INPUT_SIZE)) / 2;
            let crop_y = (new_h.saturating_sub(INPUT_SIZE)) / 2;
            scaled.crop_imm(crop_x, crop_y, INPUT_SIZE, INPUT_SIZE)
        };

        // Build input tensor [1, 3, 224, 224]: (pixel/255 - mean) / std
        let input = Array4::<f32>::from_shape_fn(
            (1, 3, INPUT_SIZE as usize, INPUT_SIZE as usize),
            |(_, c, h, w)| {
                let pixel = resized.get_pixel(w as u32, h as u32);
                (pixel[c] as f32 / 255.0 - NORM_MEAN[c]) / NORM_STD[c]
            },
        );

        let input_val = ort::value::Value::from_array(input)
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        // Run inference
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

        // Output: [1, 5] logits — extract and apply softmax
        let (_shape, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        if data.len() < NUM_CLASSES {
            return Err(ImageError::OnnxRuntime(format!(
                "Expected {} output logits, got {}",
                NUM_CLASSES,
                data.len()
            )));
        }

        let logits: Vec<f32> = data[..NUM_CLASSES].to_vec();
        let probs = softmax(&logits);

        // NSFW score = P(hentai) + P(porn) + P(sexy)
        let nsfw_score = probs[IDX_HENTAI] + probs[IDX_PORN] + probs[IDX_SEXY];

        // Find top class
        let top_idx = probs
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
            .unwrap_or(IDX_NEUTRAL);

        let mut class_probs = [0.0f32; NUM_CLASSES];
        class_probs.copy_from_slice(&probs[..NUM_CLASSES]);

        Ok(ClassifierResult {
            is_nsfw: nsfw_score >= self.threshold,
            nsfw_score,
            top_class: CLASS_NAMES[top_idx].to_string(),
            class_probs,
        })
    }
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
        "nsfw_classifier.onnx",
        "model.onnx",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_model_not_found() {
        let result = NsfwClassifier::load(Path::new("/nonexistent"), 0.5);
        assert!(result.is_err());
    }

    #[test]
    fn test_softmax_normalization() {
        let probs = softmax(&[1.0, 2.0, 0.5, 3.0, 0.1]);
        let sum: f32 = probs.iter().sum();
        assert!(
            (sum - 1.0).abs() < 1e-5,
            "Softmax should sum to 1.0, got {}",
            sum
        );
    }

    #[test]
    fn test_softmax_equal_inputs() {
        let probs = softmax(&[0.0; 5]);
        for p in &probs {
            assert!((*p - 0.2).abs() < 1e-6);
        }
    }

    #[test]
    fn test_softmax_large_values() {
        let probs = softmax(&[1000.0, 1001.0, 999.0, 1002.0, 998.0]);
        let sum: f32 = probs.iter().sum();
        assert!(
            (sum - 1.0).abs() < 1e-4,
            "Softmax should be numerically stable"
        );
    }

    #[test]
    fn test_nsfw_score_calculation() {
        // Simulated probabilities: mostly porn
        let probs: [f32; 5] = [0.01, 0.05, 0.04, 0.85, 0.05]; // draw, hentai, neutral, porn, sexy
        let nsfw_score = probs[IDX_HENTAI] + probs[IDX_PORN] + probs[IDX_SEXY];
        assert!((nsfw_score - 0.95).abs() < 1e-6);
    }

    #[test]
    fn test_safe_score_calculation() {
        // Simulated probabilities: clearly neutral
        let probs = [0.01, 0.001, 0.98, 0.005, 0.004];
        let nsfw_score = probs[IDX_HENTAI] + probs[IDX_PORN] + probs[IDX_SEXY];
        assert!(nsfw_score < 0.05);
    }

    #[test]
    fn test_threshold_boundary() {
        let result = ClassifierResult {
            is_nsfw: 0.50 >= 0.50,
            nsfw_score: 0.50,
            top_class: "sexy".to_string(),
            class_probs: [0.1, 0.1, 0.4, 0.1, 0.3],
        };
        assert!(result.is_nsfw);

        let result2 = ClassifierResult {
            is_nsfw: 0.49 >= 0.50,
            nsfw_score: 0.49,
            top_class: "neutral".to_string(),
            class_probs: [0.1, 0.1, 0.51, 0.1, 0.19],
        };
        assert!(!result2.is_nsfw);
    }

    #[test]
    fn test_class_names() {
        assert_eq!(CLASS_NAMES[IDX_DRAWINGS], "drawings");
        assert_eq!(CLASS_NAMES[IDX_HENTAI], "hentai");
        assert_eq!(CLASS_NAMES[IDX_NEUTRAL], "neutral");
        assert_eq!(CLASS_NAMES[IDX_PORN], "porn");
        assert_eq!(CLASS_NAMES[IDX_SEXY], "sexy");
    }

    #[test]
    fn test_norm_constants() {
        assert_eq!(NORM_MEAN, [0.5, 0.5, 0.5]);
        assert_eq!(NORM_STD, [0.5, 0.5, 0.5]);
    }

    #[test]
    fn test_input_size() {
        assert_eq!(INPUT_SIZE, 224);
    }
}

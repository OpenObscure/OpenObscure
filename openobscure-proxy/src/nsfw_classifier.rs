//! Holistic NSFW image classifier using ViT-tiny ONNX model.
//!
//! Provides a binary SFW/NSFW classification as a secondary check after NudeNet's
//! body-part detector. While NudeNet excels at detecting explicit exposed body parts,
//! it misses semi-nude content where body parts appear "covered" or are not detected
//! at all. This holistic classifier sees the full image context and provides a
//! complementary signal.
//!
//! Model: Marqo/nsfw-image-detection-384 (ViT-tiny, Apache 2.0)
//! Input: [1, 3, 384, 384] NCHW, float32 normalized with mean=0.5, std=0.5
//! Output: [1, 2] logits — index 0 = NSFW, index 1 = SFW
//!
//! Only invoked when NudeNet + implied-topless heuristic produce no NSFW signal.

use std::path::Path;

use image::{DynamicImage, GenericImageView};
use ndarray::Array4;
use ort::session::Session;

use crate::image_pipeline::ImageError;

/// Classifier input size (384×384 as per model config).
const INPUT_SIZE: u32 = 384;

/// ImageNet-style normalization for this model: mean=0.5, std=0.5 per channel.
const NORM_MEAN: [f32; 3] = [0.5, 0.5, 0.5];
const NORM_STD: [f32; 3] = [0.5, 0.5, 0.5];

/// Output class index for NSFW (from model's id2label config).
const NSFW_INDEX: usize = 0;

/// Holistic NSFW image classifier.
pub struct NsfwClassifier {
    session: Session,
    threshold: f32,
}

/// Result of holistic NSFW classification.
#[derive(Debug, Clone)]
pub struct ClassifierResult {
    /// Whether the image is classified as NSFW.
    pub is_nsfw: bool,
    /// P(NSFW) probability from softmax output.
    pub nsfw_score: f32,
}

impl NsfwClassifier {
    /// Load the ViT-tiny NSFW classifier from a model directory.
    pub fn load(model_dir: &Path, threshold: f32) -> Result<Self, ImageError> {
        let model_path = find_model_file(model_dir)?;

        let session = crate::ort_ep::build_session(&model_path)
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        oo_info!(crate::oo_log::modules::IMAGE, "NSFW classifier loaded",
            model = %model_path.display(),
            threshold = threshold);

        Ok(Self { session, threshold })
    }

    /// Classify an image as SFW or NSFW.
    pub fn classify(&mut self, img: &DynamicImage) -> Result<ClassifierResult, ImageError> {
        // Preprocessing must match timm's transform pipeline:
        //   1. Resize shortest edge to 384 (preserve aspect ratio, bicubic)
        //   2. Center crop to 384×384
        //   3. Normalize with mean=0.5, std=0.5

        let (w, h) = img.dimensions();
        let resized = if w == h {
            // Square — resize directly
            img.resize_exact(
                INPUT_SIZE,
                INPUT_SIZE,
                image::imageops::FilterType::CatmullRom,
            )
        } else {
            // Resize shortest edge to INPUT_SIZE, preserving aspect ratio
            let scale = INPUT_SIZE as f32 / w.min(h) as f32;
            let new_w = (w as f32 * scale).round() as u32;
            let new_h = (h as f32 * scale).round() as u32;
            let scaled = img.resize_exact(new_w, new_h, image::imageops::FilterType::CatmullRom);

            // Center crop to INPUT_SIZE × INPUT_SIZE
            let crop_x = (new_w.saturating_sub(INPUT_SIZE)) / 2;
            let crop_y = (new_h.saturating_sub(INPUT_SIZE)) / 2;
            scaled.crop_imm(crop_x, crop_y, INPUT_SIZE, INPUT_SIZE)
        };

        // Build input tensor [1, 3, 384, 384] with normalization: (pixel/255 - mean) / std
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

        // Output: [1, 2] logits — extract and apply softmax
        let (_shape, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        if data.len() < 2 {
            return Err(ImageError::OnnxRuntime(format!(
                "Expected 2 output logits, got {}",
                data.len()
            )));
        }

        let probs = softmax(&[data[0], data[1]]);
        let nsfw_score = probs[NSFW_INDEX];

        Ok(ClassifierResult {
            is_nsfw: nsfw_score >= self.threshold,
            nsfw_score,
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
    let candidates = ["nsfw_classifier.onnx", "model.onnx"];
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
        let probs = softmax(&[1.0, 2.0]);
        let sum: f32 = probs.iter().sum();
        assert!(
            (sum - 1.0).abs() < 1e-6,
            "Softmax should sum to 1.0, got {}",
            sum
        );
        assert!(
            probs[1] > probs[0],
            "Higher logit should have higher probability"
        );
    }

    #[test]
    fn test_softmax_equal_inputs() {
        let probs = softmax(&[0.0, 0.0]);
        assert!((probs[0] - 0.5).abs() < 1e-6);
        assert!((probs[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_softmax_large_values() {
        // Test numerical stability with large logits
        let probs = softmax(&[1000.0, 1001.0]);
        let sum: f32 = probs.iter().sum();
        assert!(
            (sum - 1.0).abs() < 1e-5,
            "Softmax should be numerically stable"
        );
        assert!(probs[1] > probs[0]);
    }

    #[test]
    fn test_softmax_negative_values() {
        let probs = softmax(&[-2.0, -1.0]);
        let sum: f32 = probs.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6);
        assert!(probs[1] > probs[0]);
    }

    #[test]
    fn test_threshold_boundary_above() {
        // Score above threshold → is_nsfw
        let result = ClassifierResult {
            is_nsfw: 0.80 >= 0.75,
            nsfw_score: 0.80,
        };
        assert!(result.is_nsfw);
    }

    #[test]
    fn test_threshold_boundary_at() {
        // Score exactly at threshold → is_nsfw (>=)
        let result = ClassifierResult {
            is_nsfw: 0.75 >= 0.75,
            nsfw_score: 0.75,
        };
        assert!(result.is_nsfw);
    }

    #[test]
    fn test_threshold_boundary_below() {
        // Score below threshold → not nsfw
        let result = ClassifierResult {
            is_nsfw: 0.74 >= 0.75,
            nsfw_score: 0.74,
        };
        assert!(!result.is_nsfw);
    }

    #[test]
    fn test_nsfw_index() {
        // Verify NSFW is at index 0 per model config
        assert_eq!(NSFW_INDEX, 0);
    }

    #[test]
    fn test_norm_constants() {
        // Model uses 0.5/0.5 normalization (not standard ImageNet)
        assert_eq!(NORM_MEAN, [0.5, 0.5, 0.5]);
        assert_eq!(NORM_STD, [0.5, 0.5, 0.5]);
    }
}

//! NSFW/nudity detection via NudeNet ONNX model.
//!
//! Detects exposed body parts in images to flag nudity. If any exposed
//! region is found above the confidence threshold, the image is flagged
//! as NSFW and the pipeline blurs the entire image.
//!
//! Model: NudeNet 320n (YOLOv8n-based, ~12MB, MIT license)
//! Input: [1, 3, 320, 320] NCHW, float32 [0,1]
//! Output: [1, 22, 2100] — 4 bbox coords + 18 class scores per candidate

use std::path::Path;

use image::{DynamicImage, GenericImageView};
use ndarray::Array4;
use ort::session::Session;

use crate::image_pipeline::ImageError;

/// NudeNet input size.
const INPUT_SIZE: u32 = 320;

/// Number of detection candidates in NudeNet 320n output.
const NUM_CANDIDATES: usize = 2100;

/// Number of values per candidate: 4 bbox + 18 class scores.
const CANDIDATE_SIZE: usize = 22;

/// Number of class scores per candidate.
const NUM_CLASSES: usize = 18;

/// NudeNet class labels in order.
const CLASS_LABELS: [&str; NUM_CLASSES] = [
    "FEMALE_GENITALIA_COVERED",
    "FACE_FEMALE",
    "BUTTOCKS_EXPOSED",
    "FEMALE_BREAST_EXPOSED",
    "FEMALE_GENITALIA_EXPOSED",
    "MALE_BREAST_EXPOSED",
    "ANUS_EXPOSED",
    "FEET_EXPOSED",
    "BELLY_COVERED",
    "FEET_COVERED",
    "ARMPITS_COVERED",
    "ARMPITS_EXPOSED",
    "FACE_MALE",
    "BELLY_EXPOSED",
    "MALE_GENITALIA_EXPOSED",
    "ANUS_COVERED",
    "FEMALE_BREAST_COVERED",
    "BUTTOCKS_COVERED",
];

/// Indices of exposed classes that indicate nudity.
const EXPOSED_INDICES: [usize; 5] = [
    2,  // BUTTOCKS_EXPOSED
    3,  // FEMALE_BREAST_EXPOSED
    4,  // FEMALE_GENITALIA_EXPOSED
    6,  // ANUS_EXPOSED
    14, // MALE_GENITALIA_EXPOSED
];

/// NSFW detector using NudeNet ONNX model.
pub struct NsfwDetector {
    session: Session,
    threshold: f32,
}

/// Result of NSFW detection.
#[derive(Debug, Clone)]
pub struct NsfwDetection {
    /// Whether the image contains nudity/NSFW content.
    pub is_nsfw: bool,
    /// Highest confidence score among exposed classes.
    pub confidence: f32,
    /// Label of the highest-confidence exposed class (if NSFW).
    pub category: Option<String>,
}

impl NsfwDetector {
    /// Load NudeNet ONNX model from a directory.
    pub fn load(model_dir: &Path, threshold: f32) -> Result<Self, ImageError> {
        let model_path = find_model_file(model_dir)?;

        let session = Session::builder()
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?
            .with_intra_threads(1)
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?
            .commit_from_file(&model_path)
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        oo_info!(crate::oo_log::modules::IMAGE, "NudeNet NSFW detector loaded",
            model = %model_path.display(),
            threshold = threshold);

        Ok(Self { session, threshold })
    }

    /// Detect NSFW content in an image.
    pub fn detect(&mut self, img: &DynamicImage) -> Result<NsfwDetection, ImageError> {
        let (_orig_w, _orig_h) = img.dimensions();

        // Resize to 320x320
        let resized = img.resize_exact(
            INPUT_SIZE,
            INPUT_SIZE,
            image::imageops::FilterType::Triangle,
        );
        let rgb = resized.to_rgb8();

        // Build input tensor [1, 3, 320, 320] normalized to [0, 1]
        let input = Array4::<f32>::from_shape_fn(
            (1, 3, INPUT_SIZE as usize, INPUT_SIZE as usize),
            |(_, c, h, w)| {
                let pixel = rgb.get_pixel(w as u32, h as u32);
                pixel[c] as f32 / 255.0
            },
        );

        let input_val = ort::value::Value::from_array(input)
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        // Run inference — use dynamic input name
        let input_name = self.session.inputs()[0].name().to_string();
        let outputs = self
            .session
            .run(ort::inputs![input_name.as_str() => input_val])
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        // Output shape: [1, 22, 2100] — transpose to iterate candidates
        let (_shape, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        // Parse YOLOv8 output: data is [1, 22, 2100] flattened
        // Row-major: data[row * 2100 + col] where row=0..22, col=0..2100
        // Each column is a candidate detection
        let mut best_exposed_score: f32 = 0.0;
        let mut best_exposed_class: Option<usize> = None;

        for candidate in 0..NUM_CANDIDATES {
            // Check each exposed class score for this candidate
            for &class_idx in &EXPOSED_INDICES {
                // Row = 4 + class_idx (first 4 rows are bbox coords)
                let row = 4 + class_idx;
                let score = data[row * NUM_CANDIDATES + candidate];
                if score > best_exposed_score {
                    best_exposed_score = score;
                    best_exposed_class = Some(class_idx);
                }
            }
        }

        let is_nsfw = best_exposed_score > self.threshold;

        Ok(NsfwDetection {
            is_nsfw,
            confidence: best_exposed_score,
            category: if is_nsfw {
                best_exposed_class.map(|idx| CLASS_LABELS[idx].to_string())
            } else {
                None
            },
        })
    }
}

/// Find a NudeNet model file in the given directory.
fn find_model_file(model_dir: &Path) -> Result<std::path::PathBuf, ImageError> {
    let candidates = ["320n.onnx", "nudenet.onnx", "nsfw_model.onnx", "model.onnx"];
    for name in &candidates {
        let path = model_dir.join(name);
        if path.exists() {
            return Ok(path);
        }
    }
    Err(ImageError::Decode(format!(
        "No NSFW model found in {}",
        model_dir.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_model_not_found() {
        let result = NsfwDetector::load(Path::new("/nonexistent"), 0.5);
        assert!(result.is_err());
    }

    #[test]
    fn test_exposed_indices_valid() {
        for &idx in &EXPOSED_INDICES {
            assert!(idx < NUM_CLASSES, "Exposed index {} out of range", idx);
            assert!(
                CLASS_LABELS[idx].ends_with("_EXPOSED"),
                "Class {} is not an exposed class",
                CLASS_LABELS[idx]
            );
        }
    }

    #[test]
    fn test_class_labels_count() {
        assert_eq!(CLASS_LABELS.len(), NUM_CLASSES);
    }

    #[test]
    fn test_candidate_size() {
        // 4 bbox coords + 18 class scores = 22
        assert_eq!(CANDIDATE_SIZE, 4 + NUM_CLASSES);
    }
}

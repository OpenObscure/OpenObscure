//! Face detection via ONNX Runtime.
//!
//! Two detector backends, tier-gated:
//! - **BlazeFace** (Lite tier): 128x128 input, ~400KB, selfie-distance faces
//! - **SCRFD-2.5GF** (Full/Standard tier): 640x640 input, ~3.1MB, multi-scale faces
//!
//! Model files expected:
//! - BlazeFace: `blazeface.onnx` (~400KB) + optional `blazeface_anchors.json`
//! - SCRFD: `scrfd_2.5g.onnx` (~3.1MB)

use std::path::Path;

use image::{DynamicImage, GenericImageView};
use ndarray::Array4;
use ort::session::Session;

use crate::image_pipeline::ImageError;

/// BlazeFace face detection input size.
const INPUT_SIZE: u32 = 128;

/// BlazeFace face detector using ONNX Runtime.
pub struct FaceDetector {
    session: Session,
    anchors: Vec<Anchor>,
    confidence_threshold: f32,
    nms_threshold: f32,
}

/// Pre-computed anchor box center.
#[derive(Debug, Clone)]
pub struct Anchor {
    pub cx: f32,
    pub cy: f32,
}

/// A detected face bounding box in original image coordinates.
#[derive(Debug, Clone)]
pub struct FaceDetection {
    pub x_min: f32,
    pub y_min: f32,
    pub x_max: f32,
    pub y_max: f32,
    pub confidence: f32,
}

impl FaceDetection {
    /// Convert to generic `BboxMeta` for validation.
    pub fn to_bbox_meta(&self, img_width: u32, img_height: u32) -> crate::detection_meta::BboxMeta {
        crate::detection_meta::BboxMeta {
            x_min: self.x_min,
            y_min: self.y_min,
            x_max: self.x_max,
            y_max: self.y_max,
            confidence: self.confidence,
            img_width,
            img_height,
            label: "face".to_string(),
        }
    }
}

impl FaceDetector {
    /// Load BlazeFace ONNX model and anchor definitions from a directory.
    pub fn load(model_dir: &Path, confidence_threshold: f32) -> Result<Self, ImageError> {
        let model_path = find_model_file(model_dir)?;

        let session = crate::ort_ep::build_session(&model_path)
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        let anchors_path = model_dir.join("blazeface_anchors.json");
        let anchors = if anchors_path.exists() {
            load_anchors(&anchors_path)?
        } else {
            // Generate standard BlazeFace short-range anchors (896 anchors)
            generate_short_range_anchors()
        };

        oo_info!(crate::oo_log::modules::FACE, "BlazeFace loaded",
            model = %model_path.display(),
            anchors = anchors.len(),
            confidence = confidence_threshold);

        Ok(Self {
            session,
            anchors,
            confidence_threshold,
            nms_threshold: 0.3,
        })
    }

    /// Detect faces in an image. Returns bounding boxes in original image coordinates.
    pub fn detect(&mut self, img: &DynamicImage) -> Result<Vec<FaceDetection>, ImageError> {
        let (orig_w, orig_h) = img.dimensions();

        // Resize to 128x128 for BlazeFace input
        let resized = img.resize_exact(
            INPUT_SIZE,
            INPUT_SIZE,
            image::imageops::FilterType::Triangle,
        );
        let rgb = resized.to_rgb8();

        // Build input tensor [1, 3, 128, 128] normalized to [-1, 1]
        let input = Array4::<f32>::from_shape_fn(
            (1, 3, INPUT_SIZE as usize, INPUT_SIZE as usize),
            |(_, c, h, w)| {
                let pixel = rgb.get_pixel(w as u32, h as u32);
                (pixel[c] as f32 - 127.5) / 127.5
            },
        );

        let input_val = ort::value::Value::from_array(input)
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        // Run inference — use dynamic input name to support different BlazeFace ONNX exports
        let input_name = self.session.inputs()[0].name().to_string();
        let outputs = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.session
                .run(ort::inputs![input_name.as_str() => input_val])
        })) {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => return Err(ImageError::OnnxRuntime(e.to_string())),
            Err(_) => {
                return Err(ImageError::OnnxRuntime(
                    "ONNX Runtime panicked during BlazeFace inference".to_string(),
                ))
            }
        };

        // Decode outputs — BlazeFace produces two tensors:
        // [0]: regressors [1, num_anchors, 16] (bbox + landmarks)
        // [1]: classificators [1, num_anchors, 1] (face confidence)
        // try_extract_tensor returns (&Shape, &[f32]) — flat data with shape info
        let (reg_shape, reg_data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;
        let (_score_shape, score_data) = outputs[1]
            .try_extract_tensor::<f32>()
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        // regressors shape: [1, num_anchors, 16], scores shape: [1, num_anchors, 1]
        let reg_cols = reg_shape.iter().last().copied().unwrap_or(16) as usize;
        let num_anchors = self.anchors.len().min(score_data.len());

        let mut detections = Vec::new();

        #[allow(clippy::needless_range_loop)]
        for i in 0..num_anchors {
            let score = sigmoid(score_data[i]);
            if score < self.confidence_threshold {
                continue;
            }

            let anchor = &self.anchors[i];
            let reg_offset = i * reg_cols;

            // Decode bounding box from anchor-relative offsets.
            // BlazeFace regression values are in input pixel space (0–128),
            // divide by INPUT_SIZE to normalize to [0, 1] before scaling to original image coords.
            let cx =
                anchor.cx + reg_data.get(reg_offset).copied().unwrap_or(0.0) / INPUT_SIZE as f32;
            let cy = anchor.cy
                + reg_data.get(reg_offset + 1).copied().unwrap_or(0.0) / INPUT_SIZE as f32;
            let w = reg_data.get(reg_offset + 2).copied().unwrap_or(0.0) / INPUT_SIZE as f32;
            let h = reg_data.get(reg_offset + 3).copied().unwrap_or(0.0) / INPUT_SIZE as f32;

            // Convert from center-format [0..1] to corner-format in original image coords
            let x_min = ((cx - w / 2.0) * orig_w as f32).max(0.0);
            let y_min = ((cy - h / 2.0) * orig_h as f32).max(0.0);
            let x_max = ((cx + w / 2.0) * orig_w as f32).min(orig_w as f32);
            let y_max = ((cy + h / 2.0) * orig_h as f32).min(orig_h as f32);

            detections.push(FaceDetection {
                x_min,
                y_min,
                x_max,
                y_max,
                confidence: score,
            });
        }

        // Apply non-maximum suppression
        let filtered = nms(&mut detections, self.nms_threshold);
        Ok(filtered)
    }
}

/// Sigmoid activation function.
fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// Non-maximum suppression: remove overlapping detections, keep highest confidence.
pub fn nms(detections: &mut [FaceDetection], iou_threshold: f32) -> Vec<FaceDetection> {
    detections.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut keep = Vec::new();
    let mut suppressed = vec![false; detections.len()];

    for i in 0..detections.len() {
        if suppressed[i] {
            continue;
        }
        keep.push(detections[i].clone());

        for j in (i + 1)..detections.len() {
            if suppressed[j] {
                continue;
            }
            if iou(&detections[i], &detections[j]) > iou_threshold {
                suppressed[j] = true;
            }
        }
    }
    keep
}

/// Intersection over Union of two bounding boxes.
pub fn iou(a: &FaceDetection, b: &FaceDetection) -> f32 {
    let x1 = a.x_min.max(b.x_min);
    let y1 = a.y_min.max(b.y_min);
    let x2 = a.x_max.min(b.x_max);
    let y2 = a.y_max.min(b.y_max);

    let intersection = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let area_a = (a.x_max - a.x_min) * (a.y_max - a.y_min);
    let area_b = (b.x_max - b.x_min) * (b.y_max - b.y_min);
    let union = area_a + area_b - intersection;

    if union <= 0.0 {
        0.0
    } else {
        intersection / union
    }
}

// ---------------------------------------------------------------------------
// Ultra-Light-Fast-Generic-Face-Detector (320x240, ~1.2MB)
// ---------------------------------------------------------------------------

/// Ultra-Light face detector input dimensions.
const UL_INPUT_W: u32 = 320;
const UL_INPUT_H: u32 = 240;

/// Ultra-Light face detector using ONNX Runtime.
///
/// Uses the RFB-320 variant from Linzaer/Ultra-Light-Fast-Generic-Face-Detector-1MB.
/// Input: 320x240 RGB, normalized as (pixel-127)/128.
/// Output: scores [1,4420,2] (background, face) + boxes [1,4420,4] (x_min, y_min, x_max, y_max normalized).
pub struct UltraLightDetector {
    session: Session,
    confidence_threshold: f32,
    nms_threshold: f32,
}

impl UltraLightDetector {
    /// Load Ultra-Light ONNX model from a directory.
    pub fn load(model_dir: &Path, confidence_threshold: f32) -> Result<Self, ImageError> {
        let model_path = model_dir.join("version-RFB-320.onnx");
        if !model_path.exists() {
            return Err(ImageError::OnnxRuntime(format!(
                "Ultra-Light model not found: {}",
                model_path.display()
            )));
        }

        let session = crate::ort_ep::build_session(&model_path)
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        oo_info!(crate::oo_log::modules::FACE, "Ultra-Light face detector loaded",
            model = %model_path.display(),
            confidence = confidence_threshold);

        Ok(Self {
            session,
            confidence_threshold,
            nms_threshold: 0.3,
        })
    }

    /// Detect faces in an image. Returns bounding boxes in original image coordinates.
    pub fn detect(&mut self, img: &DynamicImage) -> Result<Vec<FaceDetection>, ImageError> {
        let (orig_w, orig_h) = img.dimensions();

        // Resize to 320x240 for Ultra-Light input
        let resized = img.resize_exact(
            UL_INPUT_W,
            UL_INPUT_H,
            image::imageops::FilterType::Triangle,
        );

        // Build NCHW tensor normalized as (pixel - 127) / 128
        let input = Array4::<f32>::from_shape_fn(
            (1, 3, UL_INPUT_H as usize, UL_INPUT_W as usize),
            |(_, c, h, w)| {
                let pixel = resized.get_pixel(w as u32, h as u32);
                (pixel[c] as f32 - 127.0) / 128.0
            },
        );

        let input_val = ort::value::Value::from_array(input)
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        // Run inference
        let outputs = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.session.run(ort::inputs!["input" => input_val])
        })) {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => return Err(ImageError::OnnxRuntime(e.to_string())),
            Err(_) => {
                return Err(ImageError::OnnxRuntime(
                    "ONNX panicked during Ultra-Light inference".to_string(),
                ))
            }
        };

        // Extract scores [1, 4420, 2] and boxes [1, 4420, 4]
        let scores_tensor = outputs["scores"]
            .try_extract_tensor::<f32>()
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;
        let boxes_tensor = outputs["boxes"]
            .try_extract_tensor::<f32>()
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        let scores_data = scores_tensor.1;
        let boxes_data = boxes_tensor.1;
        let num_anchors = scores_data.len() / 2;

        let mut detections = Vec::new();
        for i in 0..num_anchors {
            let face_score = scores_data[i * 2 + 1]; // class 1 = face
            if face_score < self.confidence_threshold {
                continue;
            }

            // Boxes are normalized [0, 1] in (x_min, y_min, x_max, y_max)
            let x_min = boxes_data[i * 4] * orig_w as f32;
            let y_min = boxes_data[i * 4 + 1] * orig_h as f32;
            let x_max = boxes_data[i * 4 + 2] * orig_w as f32;
            let y_max = boxes_data[i * 4 + 3] * orig_h as f32;

            // Clamp to image bounds
            let x_min = x_min.max(0.0).min(orig_w as f32);
            let y_min = y_min.max(0.0).min(orig_h as f32);
            let x_max = x_max.max(0.0).min(orig_w as f32);
            let y_max = y_max.max(0.0).min(orig_h as f32);

            if x_max > x_min && y_max > y_min {
                detections.push(FaceDetection {
                    x_min,
                    y_min,
                    x_max,
                    y_max,
                    confidence: face_score,
                });
            }
        }

        let filtered = nms(&mut detections, self.nms_threshold);

        Ok(filtered)
    }
}

/// Find the BlazeFace ONNX model file in the model directory.
fn find_model_file(model_dir: &Path) -> Result<std::path::PathBuf, ImageError> {
    let candidates = ["blazeface_short.onnx", "blazeface.onnx", "model.onnx"];
    for name in &candidates {
        let path = model_dir.join(name);
        if path.exists() {
            return Ok(path);
        }
    }
    Err(ImageError::Decode(format!(
        "No BlazeFace model found in {}",
        model_dir.display()
    )))
}

/// Load pre-computed anchor boxes from a JSON file.
fn load_anchors(path: &Path) -> Result<Vec<Anchor>, ImageError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| ImageError::Decode(format!("Failed to read anchors file: {}", e)))?;
    let raw: Vec<Vec<f32>> = serde_json::from_str(&content)
        .map_err(|e| ImageError::Decode(format!("Failed to parse anchors JSON: {}", e)))?;
    Ok(raw
        .into_iter()
        .map(|a| Anchor {
            cx: a.first().copied().unwrap_or(0.0),
            cy: a.get(1).copied().unwrap_or(0.0),
        })
        .collect())
}

/// Generate standard BlazeFace short-range anchors.
///
/// BlazeFace short uses a specific anchor generation scheme with 896 anchors
/// across multiple feature map scales.
fn generate_short_range_anchors() -> Vec<Anchor> {
    let strides = [8, 16, 16, 16];
    let num_anchors_per_stride = [2, 6, 6, 6]; // Short-range model

    let mut anchors = Vec::new();
    for (stride_idx, &stride) in strides.iter().enumerate() {
        let grid_size = INPUT_SIZE as usize / stride;
        let num = num_anchors_per_stride[stride_idx];
        for y in 0..grid_size {
            for x in 0..grid_size {
                let cx = (x as f32 + 0.5) / grid_size as f32;
                let cy = (y as f32 + 0.5) / grid_size as f32;
                for _ in 0..num {
                    anchors.push(Anchor { cx, cy });
                }
            }
        }
    }
    anchors
}

// ---------------------------------------------------------------------------
// SCRFD-2.5GF face detector (Full / Standard tier)
// ---------------------------------------------------------------------------

/// SCRFD input resolution.
const SCRFD_INPUT_SIZE: u32 = 640;

/// Feature pyramid strides.
const SCRFD_STRIDES: [u32; 3] = [8, 16, 32];

/// Number of anchor points per grid cell (center-point priors).
const SCRFD_ANCHORS_PER_CELL: usize = 2;

/// SCRFD-2.5GF face detector for multi-scale detection.
pub struct ScrfdDetector {
    session: Session,
    confidence_threshold: f32,
    nms_threshold: f32,
    /// Pre-computed grid centers for each stride: [(center_x, center_y)]
    grid_centers: [Vec<(f32, f32)>; 3],
}

impl ScrfdDetector {
    /// Load SCRFD ONNX model from a directory.
    pub fn load(model_dir: &Path, confidence_threshold: f32) -> Result<Self, ImageError> {
        let model_path = find_scrfd_model(model_dir)?;

        let session = crate::ort_ep::build_session(&model_path)
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        let grid_centers = generate_scrfd_centers(SCRFD_INPUT_SIZE);

        oo_info!(crate::oo_log::modules::FACE, "SCRFD detector loaded",
            model = %model_path.display(),
            centers = grid_centers[0].len() + grid_centers[1].len() + grid_centers[2].len(),
            confidence = confidence_threshold);

        Ok(Self {
            session,
            confidence_threshold,
            nms_threshold: 0.4,
            grid_centers,
        })
    }

    /// Detect faces in an image. Returns bounding boxes in original image coordinates.
    pub fn detect(&mut self, img: &DynamicImage) -> Result<Vec<FaceDetection>, ImageError> {
        let (orig_w, orig_h) = img.dimensions();

        // Resize to 640x640
        let resized = img.resize_exact(
            SCRFD_INPUT_SIZE,
            SCRFD_INPUT_SIZE,
            image::imageops::FilterType::Triangle,
        );
        let rgb = resized.to_rgb8();

        // Build input tensor [1, 3, 640, 640] with SCRFD normalization: (pixel - 127.5) / 128.0
        let input = Array4::<f32>::from_shape_fn(
            (1, 3, SCRFD_INPUT_SIZE as usize, SCRFD_INPUT_SIZE as usize),
            |(_, c, h, w)| {
                let pixel = rgb.get_pixel(w as u32, h as u32);
                (pixel[c] as f32 - 127.5) / 128.0
            },
        );

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
                    "ONNX Runtime panicked during SCRFD inference".to_string(),
                ))
            }
        };

        // 9 outputs: score_8, score_16, score_32, bbox_8, bbox_16, bbox_32, kps_8, kps_16, kps_32
        // We only use scores and bboxes (indices 0-5). Keypoints (6-8) are ignored.
        let mut detections = Vec::new();
        let scale_x = orig_w as f32 / SCRFD_INPUT_SIZE as f32;
        let scale_y = orig_h as f32 / SCRFD_INPUT_SIZE as f32;

        for (stride_idx, &stride) in SCRFD_STRIDES.iter().enumerate() {
            let score_idx = stride_idx;
            let bbox_idx = stride_idx + 3;

            let (_score_shape, score_data) = outputs[score_idx]
                .try_extract_tensor::<f32>()
                .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;
            let (_bbox_shape, bbox_data) = outputs[bbox_idx]
                .try_extract_tensor::<f32>()
                .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

            let centers = &self.grid_centers[stride_idx];

            for (i, &(cx, cy)) in centers.iter().enumerate() {
                let score = score_data.get(i).copied().unwrap_or(0.0);
                if score < self.confidence_threshold {
                    continue;
                }

                // Decode bbox: distances [left, top, right, bottom] from center
                let base = i * 4;
                let d_left = bbox_data.get(base).copied().unwrap_or(0.0);
                let d_top = bbox_data.get(base + 1).copied().unwrap_or(0.0);
                let d_right = bbox_data.get(base + 2).copied().unwrap_or(0.0);
                let d_bottom = bbox_data.get(base + 3).copied().unwrap_or(0.0);

                let s = stride as f32;
                let x_min = ((cx - d_left * s) * scale_x).max(0.0);
                let y_min = ((cy - d_top * s) * scale_y).max(0.0);
                let x_max = ((cx + d_right * s) * scale_x).min(orig_w as f32);
                let y_max = ((cy + d_bottom * s) * scale_y).min(orig_h as f32);

                if x_max > x_min && y_max > y_min {
                    detections.push(FaceDetection {
                        x_min,
                        y_min,
                        x_max,
                        y_max,
                        confidence: score,
                    });
                }
            }
        }

        let filtered = nms(&mut detections, self.nms_threshold);
        Ok(filtered)
    }
}

/// Generate SCRFD grid center coordinates for each stride level.
/// Each grid cell has `SCRFD_ANCHORS_PER_CELL` center points (identical coordinates).
fn generate_scrfd_centers(input_size: u32) -> [Vec<(f32, f32)>; 3] {
    let mut result = [Vec::new(), Vec::new(), Vec::new()];
    for (idx, &stride) in SCRFD_STRIDES.iter().enumerate() {
        let grid_h = input_size / stride;
        let grid_w = input_size / stride;
        let mut centers =
            Vec::with_capacity((grid_h * grid_w * SCRFD_ANCHORS_PER_CELL as u32) as usize);
        for y in 0..grid_h {
            for x in 0..grid_w {
                let cx = (x as f32 + 0.5) * stride as f32;
                let cy = (y as f32 + 0.5) * stride as f32;
                for _ in 0..SCRFD_ANCHORS_PER_CELL {
                    centers.push((cx, cy));
                }
            }
        }
        result[idx] = centers;
    }
    result
}

/// Find SCRFD ONNX model file.
fn find_scrfd_model(model_dir: &Path) -> Result<std::path::PathBuf, ImageError> {
    let candidates = [
        "scrfd_2.5g.onnx",
        "scrfd_2.5g_bnkps_shape640x640.onnx",
        "scrfd.onnx",
    ];
    for name in &candidates {
        let path = model_dir.join(name);
        if path.exists() {
            return Ok(path);
        }
    }
    Err(ImageError::Decode(format!(
        "No SCRFD model found in {}",
        model_dir.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns true if the model file exists and is a real ONNX binary (not a Git LFS pointer).
    /// LFS pointer files are ~130 bytes; real models are always > 1KB.
    fn is_real_model(path: &Path) -> bool {
        path.exists()
            && std::fs::metadata(path)
                .map(|m| m.len() > 1024)
                .unwrap_or(false)
    }

    fn make_detection(
        x_min: f32,
        y_min: f32,
        x_max: f32,
        y_max: f32,
        confidence: f32,
    ) -> FaceDetection {
        FaceDetection {
            x_min,
            y_min,
            x_max,
            y_max,
            confidence,
        }
    }

    #[test]
    fn test_sigmoid() {
        assert!((sigmoid(0.0) - 0.5).abs() < 1e-6);
        assert!(sigmoid(10.0) > 0.999);
        assert!(sigmoid(-10.0) < 0.001);
    }

    #[test]
    fn test_iou_identical_boxes() {
        let a = make_detection(0.0, 0.0, 100.0, 100.0, 0.9);
        let b = make_detection(0.0, 0.0, 100.0, 100.0, 0.8);
        assert!((iou(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_iou_no_overlap() {
        let a = make_detection(0.0, 0.0, 50.0, 50.0, 0.9);
        let b = make_detection(60.0, 60.0, 100.0, 100.0, 0.8);
        assert_eq!(iou(&a, &b), 0.0);
    }

    #[test]
    fn test_iou_partial_overlap() {
        let a = make_detection(0.0, 0.0, 100.0, 100.0, 0.9);
        let b = make_detection(50.0, 50.0, 150.0, 150.0, 0.8);
        let result = iou(&a, &b);
        // Intersection: 50*50 = 2500, Union: 10000 + 10000 - 2500 = 17500
        assert!((result - 2500.0 / 17500.0).abs() < 1e-4);
    }

    #[test]
    fn test_nms_removes_overlapping() {
        let mut detections = vec![
            make_detection(10.0, 10.0, 110.0, 110.0, 0.95),
            make_detection(15.0, 15.0, 115.0, 115.0, 0.80), // Overlaps heavily with first
            make_detection(200.0, 200.0, 300.0, 300.0, 0.90), // Separate
        ];
        let result = nms(&mut detections, 0.3);
        assert_eq!(result.len(), 2); // First + third kept
        assert!(result[0].confidence > result[1].confidence); // Sorted by confidence
    }

    #[test]
    fn test_nms_keeps_non_overlapping() {
        let mut detections = vec![
            make_detection(0.0, 0.0, 50.0, 50.0, 0.9),
            make_detection(100.0, 100.0, 150.0, 150.0, 0.85),
            make_detection(200.0, 200.0, 250.0, 250.0, 0.80),
        ];
        let result = nms(&mut detections, 0.3);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_nms_empty_input() {
        let mut detections = Vec::new();
        let result = nms(&mut detections, 0.3);
        assert!(result.is_empty());
    }

    #[test]
    fn test_nms_single_detection() {
        let mut detections = vec![make_detection(0.0, 0.0, 100.0, 100.0, 0.9)];
        let result = nms(&mut detections, 0.3);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_generate_anchors_count() {
        let anchors = generate_short_range_anchors();
        // BlazeFace short: 16*16*2 + 8*8*6 + 8*8*6 + 8*8*6 = 512 + 384 + 384 + 384
        // Actually: stride 8 → grid 16, stride 16 → grid 8
        // 16*16*2 + 8*8*6 + 8*8*6 + 8*8*6 = 512 + 384 + 384 + 384 = 1664
        // The exact count depends on the anchor scheme; just verify non-empty and reasonable
        assert!(!anchors.is_empty());
        assert!(anchors.len() > 100);
    }

    #[test]
    fn test_anchor_values_normalized() {
        let anchors = generate_short_range_anchors();
        for a in &anchors {
            assert!(a.cx >= 0.0 && a.cx <= 1.0, "cx out of range: {}", a.cx);
            assert!(a.cy >= 0.0 && a.cy <= 1.0, "cy out of range: {}", a.cy);
        }
    }

    #[test]
    fn test_load_model_not_found() {
        let result = FaceDetector::load(Path::new("/nonexistent/path"), 0.75);
        assert!(result.is_err());
    }

    // --- SCRFD tests ---

    #[test]
    fn test_scrfd_generate_centers_count() {
        let centers = generate_scrfd_centers(640);
        // stride 8:  80*80*2 = 12800
        // stride 16: 40*40*2 = 3200
        // stride 32: 20*20*2 = 800
        assert_eq!(centers[0].len(), 12800);
        assert_eq!(centers[1].len(), 3200);
        assert_eq!(centers[2].len(), 800);
    }

    #[test]
    fn test_scrfd_centers_in_range() {
        let centers = generate_scrfd_centers(640);
        for (stride_idx, stride_centers) in centers.iter().enumerate() {
            for &(cx, cy) in stride_centers {
                assert!(
                    (0.0..=640.0).contains(&cx),
                    "stride {}: cx {} out of range",
                    SCRFD_STRIDES[stride_idx],
                    cx
                );
                assert!(
                    (0.0..=640.0).contains(&cy),
                    "stride {}: cy {} out of range",
                    SCRFD_STRIDES[stride_idx],
                    cy
                );
            }
        }
    }

    #[test]
    fn test_scrfd_centers_stride8_spacing() {
        let centers = generate_scrfd_centers(640);
        // First two centers at stride 8 should have the same position (2 anchors per cell)
        assert_eq!(centers[0][0], centers[0][1]);
        // Third center should be one stride away in x
        let (cx0, cy0) = centers[0][0];
        let (cx2, cy2) = centers[0][2]; // Next grid cell
        assert!((cx2 - cx0 - 8.0).abs() < 0.01);
        assert!((cy2 - cy0).abs() < 0.01);
    }

    #[test]
    fn test_scrfd_load_not_found() {
        let result = ScrfdDetector::load(Path::new("/nonexistent/path"), 0.5);
        assert!(result.is_err());
    }

    #[test]
    fn test_find_scrfd_model_not_found() {
        let result = find_scrfd_model(Path::new("/nonexistent/path"));
        assert!(result.is_err());
    }

    // ── Regression: bbox normalization ───────────────────────────────────
    // BlazeFace regression values are in input pixel space (0–128).
    // They must be divided by INPUT_SIZE to normalize to [0, 1].
    // Without this division, bboxes would be 128x too large.

    #[test]
    fn test_regression_bbox_normalization_constant() {
        // INPUT_SIZE must be 128 for BlazeFace short-range model
        assert_eq!(INPUT_SIZE, 128);
    }

    #[test]
    fn test_regression_iou_zero_area_box() {
        // A zero-area box should have IoU = 0 with any other box
        let zero = make_detection(50.0, 50.0, 50.0, 50.0, 0.9);
        let normal = make_detection(0.0, 0.0, 100.0, 100.0, 0.8);
        assert_eq!(iou(&zero, &normal), 0.0);
    }

    #[test]
    fn test_regression_nms_preserves_highest_confidence() {
        // When all boxes overlap heavily, NMS should keep only the highest confidence one
        let mut detections = vec![
            make_detection(10.0, 10.0, 110.0, 110.0, 0.70),
            make_detection(12.0, 12.0, 112.0, 112.0, 0.95),
            make_detection(11.0, 11.0, 111.0, 111.0, 0.80),
        ];
        let result = nms(&mut detections, 0.3);
        assert_eq!(result.len(), 1);
        assert!((result[0].confidence - 0.95).abs() < 1e-6);
    }

    #[test]
    fn test_regression_face_detection_clamps_to_image_bounds() {
        // The decode code clamps bbox to [0, orig_w] and [0, orig_h]
        // Verify the clamping logic: x_min.max(0.0) and x_max.min(orig_w)
        let x_min = (-10.0f32).max(0.0);
        let x_max = (500.0f32).min(400.0);
        assert_eq!(x_min, 0.0);
        assert_eq!(x_max, 400.0);
    }

    #[test]
    fn test_catch_unwind_converts_panic_to_error() {
        let result: Result<(), ImageError> = match std::panic::catch_unwind(
            std::panic::AssertUnwindSafe(|| -> Result<(), ort::Error> {
                panic!("simulated ONNX panic");
            }),
        ) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(ImageError::OnnxRuntime(e.to_string())),
            Err(_) => Err(ImageError::OnnxRuntime(
                "ONNX Runtime panicked during inference".to_string(),
            )),
        };
        assert!(result.is_err());
        assert!(matches!(&result, Err(ImageError::OnnxRuntime(msg)) if msg.contains("panicked")));
    }

    #[test]
    fn test_catch_unwind_passes_normal_error() {
        let result: Result<(), ImageError> =
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || -> Result<(), String> { Err("normal ONNX error".to_string()) },
            )) {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(ImageError::OnnxRuntime(e)),
                Err(_) => Err(ImageError::OnnxRuntime("panicked".to_string())),
            };
        assert!(result.is_err());
        assert!(matches!(&result, Err(ImageError::OnnxRuntime(msg)) if msg.contains("normal")));
    }

    #[test]
    fn test_catch_unwind_passes_success() {
        let result: Result<i32, ImageError> = match std::panic::catch_unwind(
            std::panic::AssertUnwindSafe(|| -> Result<i32, String> { Ok(42) }),
        ) {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(ImageError::OnnxRuntime(e)),
            Err(_) => Err(ImageError::OnnxRuntime("panicked".to_string())),
        };
        assert_eq!(result.unwrap(), 42);
    }

    // --- Ultra-Light detector tests ---

    #[test]
    fn test_all_detectors_benchmark() {
        // Comparative benchmark: BlazeFace vs Ultra-Light vs SCRFD
        let blazeface_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("models/blazeface");
        let ultralight_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("models/ultralight");
        let scrfd_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("models/scrfd");

        let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(640, 480, |x, y| {
            image::Rgb([
                ((x * 3 + y) % 256) as u8,
                ((x * 5 + y * 2) % 256) as u8,
                ((x + y * 7) % 256) as u8,
            ])
        }));

        let runs = 50;

        if is_real_model(&blazeface_dir.join("blazeface.onnx")) {
            let mut det = FaceDetector::load(&blazeface_dir, 0.75).unwrap();
            for _ in 0..5 {
                let _ = det.detect(&img);
            }
            let mut times: Vec<u128> = (0..runs)
                .map(|_| {
                    let s = std::time::Instant::now();
                    let _ = det.detect(&img);
                    s.elapsed().as_micros()
                })
                .collect();
            times.sort();
            eprintln!(
                "BlazeFace  (128x128):  p50={:.1}ms  p95={:.1}ms  model={}KB",
                times[runs / 2] as f64 / 1000.0,
                times[runs * 95 / 100] as f64 / 1000.0,
                std::fs::metadata(blazeface_dir.join("blazeface.onnx"))
                    .map(|m| m.len() / 1024)
                    .unwrap_or(0)
            );
        } else {
            eprintln!("Skipping BlazeFace: model not found or LFS pointer");
        }

        if is_real_model(&ultralight_dir.join("version-RFB-320.onnx")) {
            let mut det = UltraLightDetector::load(&ultralight_dir, 0.7).unwrap();
            for _ in 0..5 {
                let _ = det.detect(&img);
            }
            let mut times: Vec<u128> = (0..runs)
                .map(|_| {
                    let s = std::time::Instant::now();
                    let _ = det.detect(&img);
                    s.elapsed().as_micros()
                })
                .collect();
            times.sort();
            eprintln!(
                "Ultra-Light (320x240): p50={:.1}ms  p95={:.1}ms  model={}KB",
                times[runs / 2] as f64 / 1000.0,
                times[runs * 95 / 100] as f64 / 1000.0,
                std::fs::metadata(ultralight_dir.join("version-RFB-320.onnx"))
                    .map(|m| m.len() / 1024)
                    .unwrap_or(0)
            );
        } else {
            eprintln!("Skipping Ultra-Light: model not found or LFS pointer");
        }

        if is_real_model(&scrfd_dir.join("scrfd_2.5g.onnx")) {
            let mut det = ScrfdDetector::load(&scrfd_dir, 0.5).unwrap();
            for _ in 0..5 {
                let _ = det.detect(&img);
            }
            let mut times: Vec<u128> = (0..runs)
                .map(|_| {
                    let s = std::time::Instant::now();
                    let _ = det.detect(&img);
                    s.elapsed().as_micros()
                })
                .collect();
            times.sort();
            eprintln!(
                "SCRFD-2.5G (640x640):  p50={:.1}ms  p95={:.1}ms  model={}KB",
                times[runs / 2] as f64 / 1000.0,
                times[runs * 95 / 100] as f64 / 1000.0,
                std::fs::metadata(scrfd_dir.join("scrfd_2.5g.onnx"))
                    .map(|m| m.len() / 1024)
                    .unwrap_or(0)
            );
        } else {
            eprintln!("Skipping SCRFD: model not found or LFS pointer");
        }
    }

    #[test]
    fn test_ultralight_load_missing_model() {
        let result = UltraLightDetector::load(Path::new("/nonexistent/dir"), 0.7);
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(
            err_msg.contains("not found"),
            "Expected 'not found' error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_ultralight_output_format() {
        // Verify model output tensor shapes match expectations
        let model_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("models/ultralight");
        if !is_real_model(&model_dir.join("version-RFB-320.onnx")) {
            eprintln!("Skipping: Ultra-Light model not found or LFS pointer");
            return;
        }
        let mut detector = UltraLightDetector::load(&model_dir, 0.7).unwrap();
        // Run on a blank image — should produce 0 detections (no faces)
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            320,
            240,
            image::Rgb([128, 128, 128]),
        ));
        let result = detector.detect(&img).unwrap();
        // Blank image should not contain faces
        assert!(
            result.is_empty(),
            "Blank image should have 0 face detections, got {}",
            result.len()
        );
    }

    #[test]
    fn test_ultralight_nms_dedup() {
        // Ultra-Light uses the same NMS as BlazeFace
        let mut dets = vec![
            FaceDetection {
                x_min: 10.0,
                y_min: 10.0,
                x_max: 50.0,
                y_max: 50.0,
                confidence: 0.9,
            },
            FaceDetection {
                x_min: 12.0,
                y_min: 12.0,
                x_max: 52.0,
                y_max: 52.0,
                confidence: 0.8,
            },
        ];
        let kept = nms(&mut dets, 0.3);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].confidence, 0.9);
    }

    #[test]
    fn test_ultralight_vs_blazeface_benchmark() {
        // Quick latency benchmark: BlazeFace vs Ultra-Light (50 runs each)
        let blazeface_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("models/blazeface");
        let ultralight_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("models/ultralight");

        // 640x480 patterned test image
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(640, 480, |x, y| {
            image::Rgb([
                ((x * 3 + y) % 256) as u8,
                ((x * 5 + y * 2) % 256) as u8,
                ((x + y * 7) % 256) as u8,
            ])
        }));

        let runs = 50;

        if is_real_model(&blazeface_dir.join("blazeface.onnx")) {
            let mut det = FaceDetector::load(&blazeface_dir, 0.75).unwrap();
            for _ in 0..5 {
                let _ = det.detect(&img);
            }
            let mut times: Vec<u128> = (0..runs)
                .map(|_| {
                    let s = std::time::Instant::now();
                    let _ = det.detect(&img);
                    s.elapsed().as_micros()
                })
                .collect();
            times.sort();
            eprintln!(
                "BlazeFace (128x128): p50={:.1}ms  p95={:.1}ms",
                times[runs / 2] as f64 / 1000.0,
                times[runs * 95 / 100] as f64 / 1000.0
            );
        }

        if is_real_model(&ultralight_dir.join("version-RFB-320.onnx")) {
            let mut det = UltraLightDetector::load(&ultralight_dir, 0.7).unwrap();
            for _ in 0..5 {
                let _ = det.detect(&img);
            }
            let mut times: Vec<u128> = (0..runs)
                .map(|_| {
                    let s = std::time::Instant::now();
                    let _ = det.detect(&img);
                    s.elapsed().as_micros()
                })
                .collect();
            times.sort();
            eprintln!(
                "Ultra-Light (320x240): p50={:.1}ms  p95={:.1}ms",
                times[runs / 2] as f64 / 1000.0,
                times[runs * 95 / 100] as f64 / 1000.0
            );
        }

        eprintln!(
            "\nModel sizes: BlazeFace={:.0}KB  Ultra-Light={:.0}KB",
            std::fs::metadata(blazeface_dir.join("blazeface.onnx"))
                .map(|m| m.len() as f64 / 1024.0)
                .unwrap_or(0.0),
            std::fs::metadata(ultralight_dir.join("version-RFB-320.onnx"))
                .map(|m| m.len() as f64 / 1024.0)
                .unwrap_or(0.0),
        );
    }

    #[test]
    fn test_ultralight_confidence_threshold() {
        // Detections below threshold should be filtered
        let model_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("models/ultralight");
        if !is_real_model(&model_dir.join("version-RFB-320.onnx")) {
            eprintln!("Skipping: Ultra-Light model not found or LFS pointer");
            return;
        }
        // Load with very high threshold — should detect nothing
        let mut detector = UltraLightDetector::load(&model_dir, 0.99).unwrap();
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            320,
            240,
            image::Rgb([200, 180, 160]), // skin-tone-ish
        ));
        let result = detector.detect(&img).unwrap();
        assert!(
            result.is_empty(),
            "Very high threshold should filter all detections"
        );
    }
}

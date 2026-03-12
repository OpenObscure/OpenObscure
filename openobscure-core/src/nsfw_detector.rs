//! NSFW/nudity detection via NudeNet ONNX model.
//!
//! Detects exposed body parts in images to flag nudity. If any exposed
//! region is found above the confidence threshold, the image is flagged
//! as NSFW and the pipeline redacts the entire image.
//!
//! Also detects **implied toplessness** via spatial heuristic: if a face
//! anchor and secondary exposed skin (armpits/belly) are present below
//! the face, but no upper-body clothing is detected, the image is flagged
//! as implied NSFW.
//!
//! Model: NudeNet 320n (YOLOv8n-based, ~12MB, AGPL-3.0)
//! Input: [1, 3, 320, 320] NCHW, float32 [0,1]
//! Output: [1, 22, 2100] — 4 bbox coords + 18 class scores per candidate
//!
//! Postprocessing matches official NudeNet Python implementation:
//!   1. Transpose [1,22,2100] → [2100,22]
//!   2. Pre-filter candidates with max class score < 0.2
//!   3. Convert bbox from center (cx,cy,w,h) to corner (x,y,w,h)
//!   4. Apply greedy NMS (iou_threshold=0.45)
//!   5. Flag NSFW if any surviving detection is an exposed class
//!   6. If no explicit nudity, apply implied-topless spatial heuristic

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
#[cfg(test)]
const CANDIDATE_SIZE: usize = 22;

/// Number of class scores per candidate.
const NUM_CLASSES: usize = 18;

/// Pre-NMS confidence filter (matches official NudeNet).
const PRE_FILTER_THRESHOLD: f32 = 0.2;

/// NMS IoU threshold (matches official NudeNet).
const NMS_IOU_THRESHOLD: f32 = 0.45;

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

// ── Implied-topless heuristic class indices ─────────────────────────────────

/// Face anchor classes (establish subject presence and vertical baseline).
const FACE_INDICES: [usize; 2] = [
    1,  // FACE_FEMALE
    12, // FACE_MALE
];

/// Secondary exposed skin classes (signal bare upper body).
const SECONDARY_SKIN_INDICES: [usize; 2] = [
    11, // ARMPITS_EXPOSED
    13, // BELLY_EXPOSED
];

/// Upper-body clothing classes (presence negates implied topless).
const UPPER_CLOTHING_INDICES: [usize; 2] = [
    16, // FEMALE_BREAST_COVERED
    8,  // BELLY_COVERED
];

/// Lower-body covered classes used for swimwear guard.
/// If these overlap the upper torso region, treat as one-piece clothing.
const SWIMWEAR_GUARD_INDICES: [usize; 2] = [
    0,  // FEMALE_GENITALIA_COVERED
    17, // BUTTOCKS_COVERED
];

/// A candidate detection after postprocessing.
#[derive(Clone)]
struct NsfwCandidate {
    /// Top-left x (in model coordinates).
    x: f32,
    /// Top-left y (in model coordinates).
    y: f32,
    /// Width.
    w: f32,
    /// Height.
    h: f32,
    /// Best class index.
    class_id: usize,
    /// Best class score.
    score: f32,
}

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
    /// True if flagged by implied-topless heuristic rather than explicit detection.
    pub implied: bool,
}

impl NsfwDetector {
    /// Load NudeNet ONNX model from a directory.
    pub fn load(model_dir: &Path, threshold: f32) -> Result<Self, ImageError> {
        let model_path = find_model_file(model_dir)?;

        let session = crate::ort_ep::build_session(&model_path)
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        oo_info!(crate::oo_log::modules::IMAGE, "NudeNet NSFW detector loaded",
            model = %model_path.display(),
            threshold = threshold);

        Ok(Self { session, threshold })
    }

    /// Detect NSFW content in an image.
    pub fn detect(&mut self, img: &DynamicImage) -> Result<NsfwDetection, ImageError> {
        let (orig_w, orig_h) = img.dimensions();

        // Letterbox: pad to square with bottom-right padding (matching official
        // NudeNet cv2.copyMakeBorder(mat, 0, y_pad, 0, x_pad, BORDER_CONSTANT)).
        // Image placed at top-left, black padding fills bottom and right edges.
        let max_dim = orig_w.max(orig_h);
        let mut padded = image::RgbImage::from_pixel(max_dim, max_dim, image::Rgb([0u8, 0u8, 0u8]));
        let rgb_orig = img.to_rgb8();
        image::imageops::overlay(&mut padded, &rgb_orig, 0, 0);

        let resized = image::imageops::resize(
            &padded,
            INPUT_SIZE,
            INPUT_SIZE,
            image::imageops::FilterType::Triangle,
        );

        // Build input tensor [1, 3, 320, 320] normalized to [0, 1]
        let input = Array4::<f32>::from_shape_fn(
            (1, 3, INPUT_SIZE as usize, INPUT_SIZE as usize),
            |(_, c, h, w)| {
                let pixel = resized.get_pixel(w as u32, h as u32);
                pixel[c] as f32 / 255.0
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
                    "ONNX Runtime panicked during NSFW inference".to_string(),
                ))
            }
        };

        // Output: [1, 22, 2100] row-major
        // Logical layout after transpose: [2100, 22] — each candidate has
        // 4 bbox coords (cx, cy, w, h) + 18 class scores.
        // Access: element [cand][feat] in transposed = data[feat * 2100 + cand] in original.
        let (_shape, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        // Step 1: Extract candidates with max class score >= PRE_FILTER_THRESHOLD
        let mut candidates: Vec<NsfwCandidate> = Vec::new();
        let model_size = INPUT_SIZE as f32;

        for cand in 0..NUM_CANDIDATES {
            // Find best class for this candidate
            let mut max_score: f32 = 0.0;
            let mut best_class: usize = 0;
            for class_idx in 0..NUM_CLASSES {
                let feat_row = 4 + class_idx;
                let score = data[feat_row * NUM_CANDIDATES + cand];
                if score > max_score {
                    max_score = score;
                    best_class = class_idx;
                }
            }

            if max_score < PRE_FILTER_THRESHOLD {
                continue;
            }

            // Extract bbox (center format) and convert to corner format
            // Transposed layout: row R for candidate C is at data[R * NUM_CANDIDATES + C]
            let cx = data[cand]; // row 0
            let cy = data[NUM_CANDIDATES + cand]; // row 1
            let bw = data[2 * NUM_CANDIDATES + cand]; // row 2
            let bh = data[3 * NUM_CANDIDATES + cand]; // row 3

            // Center → top-left corner, clamp to model bounds
            let x = ((cx - bw / 2.0) / model_size).clamp(0.0, 1.0);
            let y = ((cy - bh / 2.0) / model_size).clamp(0.0, 1.0);
            let w = (bw / model_size).min(1.0 - x);
            let h = (bh / model_size).min(1.0 - y);

            candidates.push(NsfwCandidate {
                x,
                y,
                w,
                h,
                class_id: best_class,
                score: max_score,
            });
        }

        // Step 2: Apply NMS (greedy, sorted by confidence)
        let detections = nms(&mut candidates, NMS_IOU_THRESHOLD);

        // Step 3: Find best exposed detection (explicit nudity)
        let mut best_exposed_score: f32 = 0.0;
        let mut best_exposed_class: Option<usize> = None;

        for det in &detections {
            if det.score < self.threshold {
                continue;
            }
            if EXPOSED_INDICES.contains(&det.class_id) && det.score > best_exposed_score {
                best_exposed_score = det.score;
                best_exposed_class = Some(det.class_id);
            }
        }

        let mut is_nsfw = best_exposed_class.is_some();
        let mut implied = false;

        // Step 4: If no explicit nudity, check implied topless via spatial heuristic
        if !is_nsfw {
            if let Some((score, reason)) = check_implied_topless(&detections, self.threshold) {
                is_nsfw = true;
                best_exposed_score = score;
                implied = true;
                return Ok(NsfwDetection {
                    is_nsfw,
                    confidence: best_exposed_score,
                    category: Some(reason.to_string()),
                    implied,
                });
            }
        }

        Ok(NsfwDetection {
            is_nsfw,
            confidence: best_exposed_score,
            category: if is_nsfw {
                best_exposed_class.map(|idx| CLASS_LABELS[idx].to_string())
            } else {
                None
            },
            implied,
        })
    }
}

/// Greedy NMS: sort by score descending, suppress overlapping boxes.
fn nms(candidates: &mut [NsfwCandidate], iou_threshold: f32) -> Vec<NsfwCandidate> {
    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut keep = Vec::new();
    let mut suppressed = vec![false; candidates.len()];

    for i in 0..candidates.len() {
        if suppressed[i] {
            continue;
        }
        keep.push(candidates[i].clone());

        for j in (i + 1)..candidates.len() {
            if suppressed[j] {
                continue;
            }
            if candidate_iou(&candidates[i], &candidates[j]) > iou_threshold {
                suppressed[j] = true;
            }
        }
    }
    keep
}

/// IoU between two candidate bounding boxes (normalized coordinates).
fn candidate_iou(a: &NsfwCandidate, b: &NsfwCandidate) -> f32 {
    let x1 = a.x.max(b.x);
    let y1 = a.y.max(b.y);
    let x2 = (a.x + a.w).min(b.x + b.w);
    let y2 = (a.y + a.h).min(b.y + b.h);

    let intersection = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let area_a = a.w * a.h;
    let area_b = b.w * b.h;
    let union = area_a + area_b - intersection;

    if union <= 0.0 {
        0.0
    } else {
        intersection / union
    }
}

/// Swimwear guard multiplier: 1.25× face height below chin covers the upper
/// torso/chest region. Scale-invariant — adapts to subject size in frame.
const SWIMWEAR_GUARD_FACE_MULTIPLIER: f32 = 1.25;

/// Spatial heuristic: detect implied toplessness from NudeNet detections.
///
/// Checks for the pattern: face anchor + secondary exposed skin below the face +
/// absence of upper-body clothing = implied topless. Includes a scale-invariant
/// swimwear guard to prevent false positives on one-piece swimsuits.
///
/// Returns `Some((confidence, reason))` if implied topless is detected.
fn check_implied_topless(
    detections: &[NsfwCandidate],
    threshold: f32,
) -> Option<(f32, &'static str)> {
    // Find face anchors above threshold
    let faces: Vec<&NsfwCandidate> = detections
        .iter()
        .filter(|d| d.score >= threshold && FACE_INDICES.contains(&d.class_id))
        .collect();

    if faces.is_empty() {
        return None;
    }

    // Find the lowest face bottom edge and its associated face height.
    // We need the height to compute a scale-invariant swimwear guard.
    let mut face_bottom = 0.0f32;
    let mut ref_face_height = 0.0f32;
    for face in &faces {
        let bottom = face.y + face.h;
        if bottom > face_bottom {
            face_bottom = bottom;
            ref_face_height = face.h;
        }
    }

    // Check for upper-body clothing — if present, not topless
    let has_upper_clothing = detections
        .iter()
        .any(|d| d.score >= threshold && UPPER_CLOTHING_INDICES.contains(&d.class_id));

    if has_upper_clothing {
        return None;
    }

    // Swimwear guard (scale-invariant): if lower-body covered classes reach into
    // the upper torso, treat as one-piece swimsuit → don't flag. The guard line
    // is drawn at 1.25× face height below the chin, which covers the chest region
    // proportionally regardless of subject size in the frame.
    let guard_threshold = face_bottom + (ref_face_height * SWIMWEAR_GUARD_FACE_MULTIPLIER);

    let has_swimwear = detections.iter().any(|d| {
        d.score >= threshold
            && SWIMWEAR_GUARD_INDICES.contains(&d.class_id)
            && d.y < guard_threshold
    });

    if has_swimwear {
        return None;
    }

    // Find secondary exposed skin that is physically below the face
    let below_face_skin: Vec<&NsfwCandidate> = detections
        .iter()
        .filter(|d| {
            d.score >= threshold
                && SECONDARY_SKIN_INDICES.contains(&d.class_id)
                && d.y > face_bottom
        })
        .collect();

    if below_face_skin.is_empty() {
        return None;
    }

    // Best skin score as the confidence
    let best_score = below_face_skin
        .iter()
        .map(|d| d.score)
        .fold(0.0f32, f32::max);

    Some((best_score, "IMPLIED_TOPLESS"))
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

    #[test]
    fn test_nms_empty() {
        let mut candidates = vec![];
        let result = nms(&mut candidates, 0.45);
        assert!(result.is_empty());
    }

    #[test]
    fn test_nms_single() {
        let mut candidates = vec![NsfwCandidate {
            x: 0.1,
            y: 0.1,
            w: 0.3,
            h: 0.3,
            class_id: 3,
            score: 0.8,
        }];
        let result = nms(&mut candidates, 0.45);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].class_id, 3);
    }

    #[test]
    fn test_nms_removes_overlapping() {
        let mut candidates = vec![
            NsfwCandidate {
                x: 0.1,
                y: 0.1,
                w: 0.3,
                h: 0.3,
                class_id: 3,
                score: 0.9,
            },
            NsfwCandidate {
                x: 0.12,
                y: 0.12,
                w: 0.3,
                h: 0.3,
                class_id: 3,
                score: 0.7,
            },
        ];
        let result = nms(&mut candidates, 0.45);
        assert_eq!(result.len(), 1);
        assert!((result[0].score - 0.9).abs() < 0.001);
    }

    #[test]
    fn test_nms_keeps_non_overlapping() {
        let mut candidates = vec![
            NsfwCandidate {
                x: 0.0,
                y: 0.0,
                w: 0.2,
                h: 0.2,
                class_id: 3,
                score: 0.9,
            },
            NsfwCandidate {
                x: 0.7,
                y: 0.7,
                w: 0.2,
                h: 0.2,
                class_id: 2,
                score: 0.8,
            },
        ];
        let result = nms(&mut candidates, 0.45);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_candidate_iou_identical() {
        let a = NsfwCandidate {
            x: 0.1,
            y: 0.1,
            w: 0.3,
            h: 0.3,
            class_id: 0,
            score: 0.5,
        };
        let iou = candidate_iou(&a, &a);
        assert!((iou - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_candidate_iou_no_overlap() {
        let a = NsfwCandidate {
            x: 0.0,
            y: 0.0,
            w: 0.1,
            h: 0.1,
            class_id: 0,
            score: 0.5,
        };
        let b = NsfwCandidate {
            x: 0.5,
            y: 0.5,
            w: 0.1,
            h: 0.1,
            class_id: 0,
            score: 0.5,
        };
        let iou = candidate_iou(&a, &b);
        assert!(iou < 0.001);
    }

    #[test]
    fn test_pre_filter_threshold() {
        let pre = PRE_FILTER_THRESHOLD;
        let nms = NMS_IOU_THRESHOLD;
        assert!(pre < nms, "pre-filter must be less than NMS threshold");
        assert!(pre > 0.0, "pre-filter must be positive");
    }

    // ── Implied-topless heuristic tests ─────────────────────────────────────

    /// Helper to create a candidate with given class, score, and bbox.
    fn make_candidate(
        class_id: usize,
        score: f32,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    ) -> NsfwCandidate {
        NsfwCandidate {
            x,
            y,
            w,
            h,
            class_id,
            score,
        }
    }

    #[test]
    fn test_implied_topless_basic() {
        // Face at top, armpits exposed below → implied topless
        let detections = vec![
            make_candidate(1, 0.73, 0.3, 0.05, 0.15, 0.15), // FACE_FEMALE at top
            make_candidate(11, 0.53, 0.35, 0.30, 0.10, 0.10), // ARMPITS_EXPOSED below face
        ];
        let result = check_implied_topless(&detections, 0.25);
        assert!(result.is_some());
        let (score, reason) = result.unwrap();
        assert!((score - 0.53).abs() < 0.01);
        assert_eq!(reason, "IMPLIED_TOPLESS");
    }

    #[test]
    fn test_implied_topless_with_clothing() {
        // Face + armpits exposed + breast covered → NOT flagged
        let detections = vec![
            make_candidate(1, 0.73, 0.3, 0.05, 0.15, 0.15), // FACE_FEMALE
            make_candidate(11, 0.53, 0.35, 0.30, 0.10, 0.10), // ARMPITS_EXPOSED
            make_candidate(16, 0.65, 0.25, 0.22, 0.20, 0.15), // FEMALE_BREAST_COVERED
        ];
        let result = check_implied_topless(&detections, 0.25);
        assert!(result.is_none());
    }

    #[test]
    fn test_implied_topless_belly_covered_negates() {
        // Face + armpits exposed + belly covered → NOT flagged
        let detections = vec![
            make_candidate(1, 0.73, 0.3, 0.05, 0.15, 0.15), // FACE_FEMALE
            make_candidate(11, 0.53, 0.35, 0.30, 0.10, 0.10), // ARMPITS_EXPOSED
            make_candidate(8, 0.60, 0.25, 0.25, 0.20, 0.20), // BELLY_COVERED
        ];
        let result = check_implied_topless(&detections, 0.25);
        assert!(result.is_none());
    }

    #[test]
    fn test_implied_topless_no_face() {
        // Armpits exposed but no face → NOT flagged (no anchor)
        let detections = vec![
            make_candidate(11, 0.53, 0.35, 0.30, 0.10, 0.10), // ARMPITS_EXPOSED
            make_candidate(17, 0.70, 0.30, 0.50, 0.20, 0.20), // BUTTOCKS_COVERED
        ];
        let result = check_implied_topless(&detections, 0.25);
        assert!(result.is_none());
    }

    #[test]
    fn test_implied_topless_skin_above_face() {
        // Face at middle, armpits ABOVE face → NOT flagged (spatial check)
        let detections = vec![
            make_candidate(1, 0.73, 0.3, 0.40, 0.15, 0.15), // FACE_FEMALE at y=0.40
            make_candidate(11, 0.53, 0.35, 0.10, 0.10, 0.10), // ARMPITS_EXPOSED at y=0.10 (above face)
        ];
        let result = check_implied_topless(&detections, 0.25);
        assert!(result.is_none());
    }

    #[test]
    fn test_implied_topless_swimwear_guard() {
        // Face + armpits exposed + buttocks_covered overlapping upper torso → NOT flagged
        // Face: y=0.05, h=0.15 → face_bottom=0.20, guard = 0.20 + (0.15 * 1.25) = 0.3875
        // Buttocks_covered at y=0.30 < 0.3875 → swimwear guard triggers (one-piece)
        let detections = vec![
            make_candidate(1, 0.73, 0.3, 0.05, 0.15, 0.15), // FACE_FEMALE
            make_candidate(11, 0.53, 0.35, 0.30, 0.10, 0.10), // ARMPITS_EXPOSED below face
            make_candidate(17, 0.70, 0.25, 0.30, 0.20, 0.25), // BUTTOCKS_COVERED at y=0.30 (one-piece, overlaps chest)
        ];
        let result = check_implied_topless(&detections, 0.25);
        assert!(result.is_none());
    }

    #[test]
    fn test_implied_topless_swimwear_guard_low_body() {
        // Face + armpits exposed + buttocks_covered far below → NOT a one-piece, should flag
        // Face: y=0.05, h=0.15 → face_bottom=0.20, guard = 0.20 + (0.15 * 1.25) = 0.3875
        // Buttocks_covered at y=0.65 > 0.3875 → swimwear guard does NOT trigger
        let detections = vec![
            make_candidate(1, 0.73, 0.3, 0.05, 0.15, 0.15), // FACE_FEMALE
            make_candidate(11, 0.53, 0.35, 0.30, 0.10, 0.10), // ARMPITS_EXPOSED below face
            make_candidate(17, 0.70, 0.25, 0.65, 0.20, 0.25), // BUTTOCKS_COVERED far below (pants)
        ];
        let result = check_implied_topless(&detections, 0.25);
        assert!(result.is_some());
    }

    #[test]
    fn test_implied_topless_swimwear_guard_full_body() {
        // Full-body shot: tiny face (h=0.05) → guard is tight (1.25 × 0.05 = 0.0625)
        // Face: y=0.05, h=0.05 → face_bottom=0.10, guard = 0.10 + 0.0625 = 0.1625
        // Bikini bottom at y=0.45 > 0.1625 → guard OFF → correctly flagged
        let detections = vec![
            make_candidate(1, 0.60, 0.1, 0.05, 0.08, 0.05), // FACE_FEMALE (small, far away)
            make_candidate(11, 0.50, 0.10, 0.15, 0.05, 0.05), // ARMPITS_EXPOSED below face
            make_candidate(17, 0.65, 0.15, 0.45, 0.10, 0.10), // BUTTOCKS_COVERED (bikini bottom, at hips)
        ];
        let result = check_implied_topless(&detections, 0.25);
        assert!(
            result.is_some(),
            "Full-body topless with bikini bottom should be flagged"
        );
    }

    #[test]
    fn test_implied_topless_swimwear_guard_closeup() {
        // Close-up: large face (h=0.30) → guard extends further (1.25 × 0.30 = 0.375)
        // Face: y=0.05, h=0.30 → face_bottom=0.35, guard = 0.35 + 0.375 = 0.725
        // One-piece swimsuit top at y=0.50 < 0.725 → guard ON → correctly NOT flagged
        let detections = vec![
            make_candidate(1, 0.80, 0.1, 0.05, 0.25, 0.30), // FACE_FEMALE (close-up)
            make_candidate(11, 0.55, 0.15, 0.40, 0.10, 0.10), // ARMPITS_EXPOSED below face
            make_candidate(17, 0.70, 0.20, 0.50, 0.25, 0.30), // BUTTOCKS_COVERED (one-piece, upper torso)
        ];
        let result = check_implied_topless(&detections, 0.25);
        assert!(
            result.is_none(),
            "Close-up with one-piece swimsuit should NOT be flagged"
        );
    }

    #[test]
    fn test_implied_topless_belly_exposed() {
        // Face + belly exposed below → implied topless
        let detections = vec![
            make_candidate(12, 0.65, 0.3, 0.05, 0.15, 0.15), // FACE_MALE
            make_candidate(13, 0.55, 0.30, 0.35, 0.15, 0.15), // BELLY_EXPOSED below face
        ];
        let result = check_implied_topless(&detections, 0.25);
        assert!(result.is_some());
        let (score, _) = result.unwrap();
        assert!((score - 0.55).abs() < 0.01);
    }

    #[test]
    fn test_implied_topless_below_threshold() {
        // Face above threshold but armpits below threshold → NOT flagged
        let detections = vec![
            make_candidate(1, 0.73, 0.3, 0.05, 0.15, 0.15), // FACE_FEMALE
            make_candidate(11, 0.15, 0.35, 0.30, 0.10, 0.10), // ARMPITS_EXPOSED (low confidence)
        ];
        let result = check_implied_topless(&detections, 0.25);
        assert!(result.is_none());
    }

    #[test]
    fn test_implied_topless_class_indices_valid() {
        for &idx in &FACE_INDICES {
            assert!(idx < NUM_CLASSES, "Face index {} out of range", idx);
            assert!(
                CLASS_LABELS[idx].starts_with("FACE_"),
                "Index {} is not a face class",
                idx
            );
        }
        for &idx in &SECONDARY_SKIN_INDICES {
            assert!(idx < NUM_CLASSES, "Skin index {} out of range", idx);
            assert!(
                CLASS_LABELS[idx].ends_with("_EXPOSED"),
                "Index {} is not an exposed class",
                idx
            );
        }
        for &idx in &UPPER_CLOTHING_INDICES {
            assert!(idx < NUM_CLASSES, "Clothing index {} out of range", idx);
            assert!(
                CLASS_LABELS[idx].ends_with("_COVERED"),
                "Index {} is not a covered class",
                idx
            );
        }
        for &idx in &SWIMWEAR_GUARD_INDICES {
            assert!(idx < NUM_CLASSES, "Swimwear index {} out of range", idx);
            assert!(
                CLASS_LABELS[idx].ends_with("_COVERED"),
                "Index {} is not a covered class",
                idx
            );
        }
    }
}

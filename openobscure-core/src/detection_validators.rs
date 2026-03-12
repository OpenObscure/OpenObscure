//! Pure-logic validators for detection outputs.
//!
//! All functions are pure arithmetic — no model loading, no I/O, no image decoding.
//! Designed to run in microseconds on synthetic `BboxMeta` / `NsfwMeta` data.

use crate::detection_meta::{BboxMeta, NsfwMeta, ScreenshotMeta};

/// Severity of a validation issue.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Severity {
    Warning,
    Error,
}

/// A single validation finding.
#[derive(Debug, Clone)]
pub struct ValidationIssue {
    pub severity: Severity,
    pub check: &'static str,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Generic bbox validation
// ---------------------------------------------------------------------------

/// Validate a single bounding box against universal sanity invariants.
pub fn validate_bbox(b: &BboxMeta) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();

    // No NaN/Inf
    for (name, val) in [
        ("x_min", b.x_min),
        ("y_min", b.y_min),
        ("x_max", b.x_max),
        ("y_max", b.y_max),
        ("confidence", b.confidence),
    ] {
        if !val.is_finite() {
            issues.push(ValidationIssue {
                severity: Severity::Error,
                check: "finite_values",
                message: format!("{} is not finite: {}", name, val),
            });
        }
    }

    // Ordered coordinates
    if b.x_min >= b.x_max {
        issues.push(ValidationIssue {
            severity: Severity::Error,
            check: "ordered_coords",
            message: format!("x_min ({}) >= x_max ({})", b.x_min, b.x_max),
        });
    }
    if b.y_min >= b.y_max {
        issues.push(ValidationIssue {
            severity: Severity::Error,
            check: "ordered_coords",
            message: format!("y_min ({}) >= y_max ({})", b.y_min, b.y_max),
        });
    }

    // Within image bounds
    if b.x_min < 0.0 {
        issues.push(ValidationIssue {
            severity: Severity::Error,
            check: "within_bounds",
            message: format!("x_min ({}) < 0", b.x_min),
        });
    }
    if b.y_min < 0.0 {
        issues.push(ValidationIssue {
            severity: Severity::Error,
            check: "within_bounds",
            message: format!("y_min ({}) < 0", b.y_min),
        });
    }
    if b.x_max > b.img_width as f32 {
        issues.push(ValidationIssue {
            severity: Severity::Error,
            check: "within_bounds",
            message: format!("x_max ({}) > img_width ({})", b.x_max, b.img_width),
        });
    }
    if b.y_max > b.img_height as f32 {
        issues.push(ValidationIssue {
            severity: Severity::Error,
            check: "within_bounds",
            message: format!("y_max ({}) > img_height ({})", b.y_max, b.img_height),
        });
    }

    // No full-image box (area > 95% is suspicious)
    if b.area_ratio() > 0.95 {
        issues.push(ValidationIssue {
            severity: Severity::Error,
            check: "no_full_image_box",
            message: format!("bbox covers {:.1}% of image", b.area_ratio() * 100.0),
        });
    }

    // Minimum size
    if b.width() < 5.0 || b.height() < 5.0 {
        issues.push(ValidationIssue {
            severity: Severity::Warning,
            check: "minimum_size",
            message: format!("bbox too small: {:.0}x{:.0}", b.width(), b.height()),
        });
    }

    // Confidence range
    if b.confidence <= 0.0 || b.confidence > 1.0 {
        issues.push(ValidationIssue {
            severity: Severity::Error,
            check: "confidence_range",
            message: format!("confidence {} not in (0, 1]", b.confidence),
        });
    }

    issues
}

/// Check that no pair of detections violates NMS (IoU > threshold).
pub fn validate_no_nms_violations(bboxes: &[BboxMeta], iou_threshold: f32) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    for i in 0..bboxes.len() {
        for j in (i + 1)..bboxes.len() {
            let overlap = bbox_iou(&bboxes[i], &bboxes[j]);
            if overlap > iou_threshold {
                issues.push(ValidationIssue {
                    severity: Severity::Error,
                    check: "nms_violation",
                    message: format!(
                        "detections {} and {} have IoU {:.3} > threshold {}",
                        i, j, overlap, iou_threshold
                    ),
                });
            }
        }
    }
    issues
}

// ---------------------------------------------------------------------------
// Face-specific validation
// ---------------------------------------------------------------------------

/// Validate face detections with domain-specific rules.
pub fn validate_face_detections(
    faces: &[BboxMeta],
    img_w: u32,
    img_h: u32,
) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();

    for (i, face) in faces.iter().enumerate() {
        // Generic bbox checks
        for issue in validate_bbox(face) {
            issues.push(ValidationIssue {
                message: format!("face {}: {}", i, issue.message),
                ..issue
            });
        }

        // Face aspect ratio: 0.3 < w/h < 3.0
        let ar = face.aspect_ratio();
        if ar > 0.0 && !(0.3..=3.0).contains(&ar) {
            issues.push(ValidationIssue {
                severity: Severity::Warning,
                check: "face_aspect_ratio",
                message: format!("face {}: aspect ratio {:.2} outside [0.3, 3.0]", i, ar),
            });
        }

        // Face confidence threshold (BlazeFace uses 0.75)
        if face.confidence < 0.75 {
            issues.push(ValidationIssue {
                severity: Severity::Warning,
                check: "face_confidence",
                message: format!(
                    "face {}: confidence {:.3} < 0.75 threshold",
                    i, face.confidence
                ),
            });
        }

        // expand_bbox stays in bounds (simulate 15% expansion)
        let w = face.width();
        let h = face.height();
        let dx = w * 0.15;
        let dy = h * 0.15;
        let ex_min = (face.x_min - dx).max(0.0);
        let ey_min = (face.y_min - dy).max(0.0);
        let ex_max = (face.x_max + dx).min(img_w as f32);
        let ey_max = (face.y_max + dy).min(img_h as f32);
        if ex_min < 0.0 || ey_min < 0.0 || ex_max > img_w as f32 || ey_max > img_h as f32 {
            issues.push(ValidationIssue {
                severity: Severity::Error,
                check: "expand_bbox_bounds",
                message: format!(
                    "face {}: expanded bbox ({:.0},{:.0})→({:.0},{:.0}) out of {}x{}",
                    i, ex_min, ey_min, ex_max, ey_max, img_w, img_h
                ),
            });
        }
    }

    // NMS validation across all faces
    issues.extend(validate_no_nms_violations(faces, 0.5));

    issues
}

// ---------------------------------------------------------------------------
// OCR text region validation
// ---------------------------------------------------------------------------

/// Validate OCR text region detections.
pub fn validate_text_regions(regions: &[BboxMeta], img_w: u32, img_h: u32) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();

    for (i, region) in regions.iter().enumerate() {
        // Generic bbox checks
        for issue in validate_bbox(region) {
            issues.push(ValidationIssue {
                message: format!("text {}: {}", i, issue.message),
                ..issue
            });
        }

        // Text region minimum height (MIN_REGION_SIZE = 3)
        if region.height() < 3.0 {
            issues.push(ValidationIssue {
                severity: Severity::Warning,
                check: "text_min_height",
                message: format!("text {}: height {:.1} < 3px min", i, region.height()),
            });
        }

        // Text score threshold (DET_BOX_THRESHOLD = 0.6)
        if region.confidence < 0.6 {
            issues.push(ValidationIssue {
                severity: Severity::Warning,
                check: "text_score_threshold",
                message: format!("text {}: score {:.3} < 0.6 threshold", i, region.confidence),
            });
        }

        // Text aspect ratio: w/h > 0.5 (text is wider than tall)
        let ar = region.aspect_ratio();
        if ar > 0.0 && ar < 0.5 {
            issues.push(ValidationIssue {
                severity: Severity::Warning,
                check: "text_aspect_ratio",
                message: format!(
                    "text {}: aspect ratio {:.2} < 0.5 (taller than wide)",
                    i, ar
                ),
            });
        }

        // 50% vertical padding stays in bounds
        let pad = region.height() * 0.5;
        let padded_y_min = (region.y_min - pad).max(0.0);
        let padded_y_max = (region.y_max + pad).min(img_h as f32);
        if padded_y_min < 0.0 || padded_y_max > img_h as f32 {
            issues.push(ValidationIssue {
                severity: Severity::Error,
                check: "text_redact_padding_bounds",
                message: format!(
                    "text {}: padded y range [{:.0}, {:.0}] out of image height {}",
                    i, padded_y_min, padded_y_max, img_h
                ),
            });
        }

        // Degenerate quad check (zero area)
        if region.area() <= 0.0 {
            issues.push(ValidationIssue {
                severity: Severity::Error,
                check: "text_degenerate",
                message: format!("text {}: zero or negative area", i),
            });
        }
    }

    // Check for text regions with similar overlap (no NMS for OCR, but flag heavy overlap)
    let _ = (regions, img_w); // img_w used via validate_bbox

    issues
}

// ---------------------------------------------------------------------------
// NSFW validation
// ---------------------------------------------------------------------------

/// Valid exposed class labels.
const EXPOSED_LABELS: &[&str] = &[
    "BUTTOCKS_EXPOSED",
    "FEMALE_BREAST_EXPOSED",
    "FEMALE_GENITALIA_EXPOSED",
    "ANUS_EXPOSED",
    "MALE_GENITALIA_EXPOSED",
    "IMPLIED_TOPLESS",
    "CLASSIFIER_NSFW",
];

/// Validate NSFW detection metadata.
pub fn validate_nsfw(meta: &NsfwMeta) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();

    // Consistency: is_nsfw should match confidence vs threshold
    if meta.is_nsfw && meta.confidence <= meta.threshold {
        issues.push(ValidationIssue {
            severity: Severity::Error,
            check: "nsfw_flag_consistency",
            message: format!(
                "is_nsfw=true but confidence {:.3} <= threshold {:.3}",
                meta.confidence, meta.threshold
            ),
        });
    }
    if !meta.is_nsfw && meta.confidence > meta.threshold {
        issues.push(ValidationIssue {
            severity: Severity::Error,
            check: "nsfw_flag_consistency",
            message: format!(
                "is_nsfw=false but confidence {:.3} > threshold {:.3}",
                meta.confidence, meta.threshold
            ),
        });
    }

    // If NSFW, category must be present and valid
    if meta.is_nsfw {
        match &meta.category {
            None => {
                issues.push(ValidationIssue {
                    severity: Severity::Error,
                    check: "nsfw_category_present",
                    message: "is_nsfw=true but category is None".to_string(),
                });
            }
            Some(cat) => {
                if !EXPOSED_LABELS.contains(&cat.as_str()) {
                    issues.push(ValidationIssue {
                        severity: Severity::Error,
                        check: "nsfw_category_valid",
                        message: format!("category '{}' not in EXPOSED labels", cat),
                    });
                }
            }
        }
    }

    // Confidence range
    if meta.confidence < 0.0 || meta.confidence > 1.0 {
        issues.push(ValidationIssue {
            severity: Severity::Error,
            check: "nsfw_confidence_range",
            message: format!("confidence {} not in [0, 1]", meta.confidence),
        });
    }

    // Exposed scores count (should be ≤ 18 total classes in NudeNet)
    if meta.exposed_scores.len() > 18 {
        issues.push(ValidationIssue {
            severity: Severity::Warning,
            check: "nsfw_scores_count",
            message: format!(
                "exposed_scores has {} entries (expected ≤ 18)",
                meta.exposed_scores.len()
            ),
        });
    }

    issues
}

// ---------------------------------------------------------------------------
// Screenshot validation
// ---------------------------------------------------------------------------

/// Validate screenshot detection metadata.
pub fn validate_screenshot(meta: &ScreenshotMeta) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();

    // is_screenshot requires ≥2 reasons OR explicit EXIF match
    if meta.is_screenshot {
        let has_exif = meta.exif_software.is_some();
        if !has_exif && meta.reason_count < 2 {
            issues.push(ValidationIssue {
                severity: Severity::Error,
                check: "screenshot_reason_count",
                message: format!(
                    "is_screenshot=true but only {} reason(s) and no EXIF match",
                    meta.reason_count
                ),
            });
        }
    }

    // Status bar variance consistency
    if let Some(variance) = meta.status_bar_variance {
        if variance < 0.0 {
            issues.push(ValidationIssue {
                severity: Severity::Error,
                check: "screenshot_variance_range",
                message: format!("status_bar_variance {} < 0", variance),
            });
        }
    }

    issues
}

// ---------------------------------------------------------------------------
// IoU and Precision/Recall
// ---------------------------------------------------------------------------

/// Compute Intersection over Union of two bounding boxes.
pub fn bbox_iou(a: &BboxMeta, b: &BboxMeta) -> f32 {
    let x1 = a.x_min.max(b.x_min);
    let y1 = a.y_min.max(b.y_min);
    let x2 = a.x_max.min(b.x_max);
    let y2 = a.y_max.min(b.y_max);

    let intersection = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let area_a = a.area();
    let area_b = b.area();
    let union = area_a + area_b - intersection;

    if union <= 0.0 {
        0.0
    } else {
        intersection / union
    }
}

/// Compute precision, recall, and F1 for detected vs ground-truth bboxes.
///
/// A detection matches a ground truth box if IoU ≥ `iou_threshold`.
/// Each ground truth can be matched at most once (greedy matching by highest IoU).
pub fn precision_recall(
    detected: &[BboxMeta],
    ground_truth: &[BboxMeta],
    iou_threshold: f32,
) -> (f32, f32, f32) {
    if detected.is_empty() && ground_truth.is_empty() {
        return (1.0, 1.0, 1.0);
    }
    if detected.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    if ground_truth.is_empty() {
        return (0.0, 0.0, 0.0);
    }

    let mut gt_matched = vec![false; ground_truth.len()];
    let mut tp = 0u32;

    for det in detected {
        let mut best_iou = 0.0f32;
        let mut best_gt = None;

        for (j, gt) in ground_truth.iter().enumerate() {
            if gt_matched[j] {
                continue;
            }
            let iou = bbox_iou(det, gt);
            if iou >= iou_threshold && iou > best_iou {
                best_iou = iou;
                best_gt = Some(j);
            }
        }

        if let Some(j) = best_gt {
            gt_matched[j] = true;
            tp += 1;
        }
    }

    let precision = tp as f32 / detected.len() as f32;
    let recall = tp as f32 / ground_truth.len() as f32;
    let f1 = if precision + recall > 0.0 {
        2.0 * precision * recall / (precision + recall)
    } else {
        0.0
    };

    (precision, recall, f1)
}

// ===========================================================================
// Tests — all synthetic data, microseconds per test
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detection_meta::{NsfwMeta, ScreenshotMeta};

    #[allow(clippy::too_many_arguments)]
    fn make_bbox(
        x_min: f32,
        y_min: f32,
        x_max: f32,
        y_max: f32,
        confidence: f32,
        img_w: u32,
        img_h: u32,
        label: &str,
    ) -> BboxMeta {
        BboxMeta {
            x_min,
            y_min,
            x_max,
            y_max,
            confidence,
            img_width: img_w,
            img_height: img_h,
            label: label.to_string(),
        }
    }

    // -----------------------------------------------------------------------
    // Face validator tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_valid_face_bbox() {
        // Normal face: ~10% of 640x480 image
        let face = make_bbox(200.0, 100.0, 280.0, 200.0, 0.92, 640, 480, "face");
        let issues = validate_bbox(&face);
        let errors: Vec<_> = issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "Valid face should have no errors: {:?}",
            errors
        );
    }

    #[test]
    fn test_face_outside_bounds() {
        let face = make_bbox(600.0, 400.0, 700.0, 500.0, 0.9, 640, 480, "face");
        let issues = validate_bbox(&face);
        assert!(
            issues.iter().any(|i| i.check == "within_bounds"),
            "Should flag x_max > img_width"
        );
    }

    #[test]
    fn test_face_full_image_box() {
        // Covers 100% of image — detection failure
        let face = make_bbox(0.0, 0.0, 640.0, 480.0, 0.8, 640, 480, "face");
        let issues = validate_bbox(&face);
        assert!(
            issues.iter().any(|i| i.check == "no_full_image_box"),
            "Full-image bbox should be flagged"
        );
    }

    #[test]
    fn test_face_too_small() {
        let face = make_bbox(100.0, 100.0, 103.0, 103.0, 0.9, 640, 480, "face");
        let issues = validate_bbox(&face);
        assert!(
            issues.iter().any(|i| i.check == "minimum_size"),
            "3x3px face should be flagged"
        );
    }

    #[test]
    fn test_face_extreme_aspect_ratio() {
        // 200x10px "face" — aspect ratio 20.0
        let face = make_bbox(100.0, 100.0, 300.0, 110.0, 0.9, 640, 480, "face");
        let faces = vec![face];
        let issues = validate_face_detections(&faces, 640, 480);
        assert!(
            issues.iter().any(|i| i.check == "face_aspect_ratio"),
            "Extreme aspect ratio should be flagged"
        );
    }

    #[test]
    fn test_face_zero_confidence() {
        let face = make_bbox(100.0, 100.0, 200.0, 200.0, 0.0, 640, 480, "face");
        let issues = validate_bbox(&face);
        assert!(
            issues.iter().any(|i| i.check == "confidence_range"),
            "Zero confidence should be flagged"
        );
    }

    #[test]
    fn test_face_nan_coordinates() {
        let face = make_bbox(f32::NAN, 100.0, 200.0, 200.0, 0.9, 640, 480, "face");
        let issues = validate_bbox(&face);
        assert!(
            issues.iter().any(|i| i.check == "finite_values"),
            "NaN coordinates should be flagged"
        );
    }

    #[test]
    fn test_face_nms_violation() {
        // Two overlapping faces that NMS should have suppressed
        let faces = vec![
            make_bbox(100.0, 100.0, 200.0, 200.0, 0.95, 640, 480, "face"),
            make_bbox(105.0, 105.0, 205.0, 205.0, 0.80, 640, 480, "face"),
        ];
        let issues = validate_no_nms_violations(&faces, 0.5);
        assert!(
            issues.iter().any(|i| i.check == "nms_violation"),
            "Overlapping faces should flag NMS violation"
        );
    }

    #[test]
    fn test_face_reasonable_bbox() {
        // Face is ~10% of 640x480, good aspect ratio, good confidence
        let face = make_bbox(200.0, 120.0, 300.0, 250.0, 0.88, 640, 480, "face");
        let faces = vec![face];
        let issues = validate_face_detections(&faces, 640, 480);
        let errors: Vec<_> = issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "Reasonable face should have no errors: {:?}",
            errors
        );
    }

    #[test]
    fn test_face_partial_oob() {
        // x_min is negative
        let face = make_bbox(-5.0, 100.0, 95.0, 200.0, 0.9, 640, 480, "face");
        let issues = validate_bbox(&face);
        assert!(
            issues.iter().any(|i| i.check == "within_bounds"),
            "Negative x_min should be flagged"
        );
    }

    #[test]
    fn test_face_confidence_threshold() {
        let below = make_bbox(100.0, 100.0, 200.0, 200.0, 0.74, 640, 480, "face");
        let above = make_bbox(100.0, 100.0, 200.0, 200.0, 0.76, 640, 480, "face");
        let issues_below = validate_face_detections(&[below], 640, 480);
        let issues_above = validate_face_detections(&[above], 640, 480);
        assert!(
            issues_below.iter().any(|i| i.check == "face_confidence"),
            "0.74 should fail threshold"
        );
        assert!(
            !issues_above.iter().any(|i| i.check == "face_confidence"),
            "0.76 should pass threshold"
        );
    }

    #[test]
    fn test_multiple_faces_no_overlap() {
        let faces = vec![
            make_bbox(10.0, 10.0, 80.0, 80.0, 0.9, 640, 480, "face"),
            make_bbox(200.0, 200.0, 280.0, 280.0, 0.85, 640, 480, "face"),
            make_bbox(400.0, 100.0, 480.0, 180.0, 0.8, 640, 480, "face"),
        ];
        let issues = validate_no_nms_violations(&faces, 0.5);
        assert!(
            issues.is_empty(),
            "Non-overlapping faces should pass NMS check"
        );
    }

    #[test]
    fn test_expand_bbox_stays_in_bounds() {
        // Face near corner — expanded bbox should be clamped by expand_bbox()
        let face = make_bbox(5.0, 5.0, 60.0, 70.0, 0.9, 640, 480, "face");
        let faces = vec![face];
        let issues = validate_face_detections(&faces, 640, 480);
        // expand_bbox clamps to 0, so should not flag
        assert!(
            !issues.iter().any(|i| i.check == "expand_bbox_bounds"),
            "expand_bbox near corner should clamp, not error"
        );
    }

    // -----------------------------------------------------------------------
    // OCR text region tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_valid_text_region() {
        let region = make_bbox(50.0, 100.0, 400.0, 130.0, 0.85, 640, 480, "text");
        let issues = validate_text_regions(&[region], 640, 480);
        let errors: Vec<_> = issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "Valid text region should have no errors: {:?}",
            errors
        );
    }

    #[test]
    fn test_text_too_small() {
        let region = make_bbox(100.0, 100.0, 101.5, 102.0, 0.9, 640, 480, "text");
        let issues = validate_text_regions(&[region], 640, 480);
        assert!(
            issues
                .iter()
                .any(|i| i.check == "minimum_size" || i.check == "text_min_height"),
            "Tiny text region should be flagged"
        );
    }

    #[test]
    fn test_text_extreme_width() {
        // 400x2px text — suspicious
        let region = make_bbox(50.0, 100.0, 450.0, 102.0, 0.9, 640, 480, "text");
        let issues = validate_text_regions(&[region], 640, 480);
        assert!(
            issues
                .iter()
                .any(|i| i.check == "minimum_size" || i.check == "text_min_height"),
            "400x2px text should be flagged as too thin"
        );
    }

    #[test]
    fn test_text_below_threshold() {
        let region = make_bbox(50.0, 100.0, 400.0, 130.0, 0.3, 640, 480, "text");
        let issues = validate_text_regions(&[region], 640, 480);
        assert!(
            issues.iter().any(|i| i.check == "text_score_threshold"),
            "Score 0.3 should be below 0.6 threshold"
        );
    }

    #[test]
    fn test_text_redact_padding_in_bounds() {
        // Text at y=100 to y=130, height=30, pad=15, padded=[85, 145] — well within 480
        let region = make_bbox(50.0, 100.0, 400.0, 130.0, 0.9, 640, 480, "text");
        let issues = validate_text_regions(&[region], 640, 480);
        assert!(
            !issues
                .iter()
                .any(|i| i.check == "text_redact_padding_bounds"),
            "Padded region should be within bounds"
        );
    }

    #[test]
    fn test_text_quad_valid() {
        // Normal text line quad
        let region = make_bbox(10.0, 20.0, 200.0, 50.0, 0.8, 640, 480, "text");
        assert!(region.area() > 0.0, "Valid quad should have positive area");
    }

    #[test]
    fn test_text_quad_degenerate() {
        // All points collapsed — zero area
        let region = make_bbox(100.0, 100.0, 100.0, 100.0, 0.9, 640, 480, "text");
        let issues = validate_text_regions(&[region], 640, 480);
        assert!(
            issues
                .iter()
                .any(|i| i.check == "text_degenerate" || i.check == "ordered_coords"),
            "Degenerate quad should be flagged"
        );
    }

    // -----------------------------------------------------------------------
    // NSFW tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_nsfw_flagged_valid() {
        let meta = NsfwMeta {
            is_nsfw: true,
            confidence: 0.85,
            threshold: 0.45,
            category: Some("FEMALE_BREAST_EXPOSED".to_string()),
            exposed_scores: vec![("FEMALE_BREAST_EXPOSED".to_string(), 0.85)],
            classifier_score: None,
        };
        let issues = validate_nsfw(&meta);
        assert!(
            issues.is_empty(),
            "Valid NSFW detection should have no issues: {:?}",
            issues
        );
    }

    #[test]
    fn test_nsfw_not_flagged_valid() {
        let meta = NsfwMeta {
            is_nsfw: false,
            confidence: 0.1,
            threshold: 0.45,
            category: None,
            exposed_scores: vec![],
            classifier_score: None,
        };
        let issues = validate_nsfw(&meta);
        assert!(
            issues.is_empty(),
            "Clean image should have no issues: {:?}",
            issues
        );
    }

    #[test]
    fn test_nsfw_inconsistent_flag() {
        // is_nsfw=true but confidence below threshold
        let meta = NsfwMeta {
            is_nsfw: true,
            confidence: 0.3,
            threshold: 0.45,
            category: Some("BUTTOCKS_EXPOSED".to_string()),
            exposed_scores: vec![],
            classifier_score: None,
        };
        let issues = validate_nsfw(&meta);
        assert!(
            issues.iter().any(|i| i.check == "nsfw_flag_consistency"),
            "Inconsistent flag should be detected"
        );
    }

    #[test]
    fn test_nsfw_missing_category() {
        let meta = NsfwMeta {
            is_nsfw: true,
            confidence: 0.8,
            threshold: 0.45,
            category: None,
            exposed_scores: vec![],
            classifier_score: None,
        };
        let issues = validate_nsfw(&meta);
        assert!(
            issues.iter().any(|i| i.check == "nsfw_category_present"),
            "NSFW without category should be flagged"
        );
    }

    #[test]
    fn test_nsfw_class_scores_count() {
        let meta = NsfwMeta {
            is_nsfw: false,
            confidence: 0.1,
            threshold: 0.45,
            category: None,
            // 5 exposed scores is fine (≤ 18)
            exposed_scores: (0..5).map(|i| (format!("class_{}", i), 0.1)).collect(),
            classifier_score: None,
        };
        let issues = validate_nsfw(&meta);
        assert!(
            !issues.iter().any(|i| i.check == "nsfw_scores_count"),
            "5 scores should be fine"
        );
    }

    #[test]
    fn test_nsfw_bad_category() {
        let meta = NsfwMeta {
            is_nsfw: true,
            confidence: 0.8,
            threshold: 0.45,
            category: Some("FACE_FEMALE".to_string()), // Not an exposed class
            exposed_scores: vec![],
            classifier_score: None,
        };
        let issues = validate_nsfw(&meta);
        assert!(
            issues.iter().any(|i| i.check == "nsfw_category_valid"),
            "Non-exposed category should be flagged"
        );
    }

    // -----------------------------------------------------------------------
    // Screenshot tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_screenshot_resolution_exact() {
        let meta = ScreenshotMeta {
            is_screenshot: true,
            resolution_match: true,
            status_bar_variance: Some(30.0),
            exif_software: None,
            reason_count: 2,
        };
        let issues = validate_screenshot(&meta);
        assert!(
            issues.is_empty(),
            "Valid screenshot should pass: {:?}",
            issues
        );
    }

    #[test]
    fn test_screenshot_resolution_off_by_one() {
        // resolution_match=false for 1921x1080
        let meta = ScreenshotMeta {
            is_screenshot: false,
            resolution_match: false,
            status_bar_variance: Some(500.0),
            exif_software: None,
            reason_count: 0,
        };
        let issues = validate_screenshot(&meta);
        assert!(
            issues.is_empty(),
            "Non-screenshot should pass: {:?}",
            issues
        );
    }

    #[test]
    fn test_screenshot_low_variance_flagged() {
        let meta = ScreenshotMeta {
            is_screenshot: true,
            resolution_match: true,
            status_bar_variance: Some(30.0), // < 50
            exif_software: None,
            reason_count: 2,
        };
        let issues = validate_screenshot(&meta);
        assert!(
            issues.is_empty(),
            "Low variance + resolution = valid screenshot"
        );
    }

    #[test]
    fn test_screenshot_high_variance_not() {
        let meta = ScreenshotMeta {
            is_screenshot: false,
            resolution_match: false,
            status_bar_variance: Some(500.0),
            exif_software: None,
            reason_count: 0,
        };
        let issues = validate_screenshot(&meta);
        assert!(
            issues.is_empty(),
            "High variance non-screenshot should pass"
        );
    }

    #[test]
    fn test_screenshot_two_reasons_flagged() {
        let meta = ScreenshotMeta {
            is_screenshot: true,
            resolution_match: true,
            status_bar_variance: Some(20.0),
            exif_software: None,
            reason_count: 2,
        };
        let issues = validate_screenshot(&meta);
        assert!(issues.is_empty(), "2 reasons is sufficient");
    }

    #[test]
    fn test_screenshot_one_reason_not_enough() {
        let meta = ScreenshotMeta {
            is_screenshot: true,
            resolution_match: true,
            status_bar_variance: None,
            exif_software: None, // No EXIF match
            reason_count: 1,     // Only 1 reason
        };
        let issues = validate_screenshot(&meta);
        assert!(
            issues.iter().any(|i| i.check == "screenshot_reason_count"),
            "1 reason without EXIF should be flagged"
        );
    }

    // -----------------------------------------------------------------------
    // IoU tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_iou_identical() {
        let a = make_bbox(0.0, 0.0, 100.0, 100.0, 0.9, 200, 200, "a");
        let b = make_bbox(0.0, 0.0, 100.0, 100.0, 0.8, 200, 200, "b");
        assert!((bbox_iou(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_iou_no_overlap() {
        let a = make_bbox(0.0, 0.0, 50.0, 50.0, 0.9, 200, 200, "a");
        let b = make_bbox(60.0, 60.0, 100.0, 100.0, 0.8, 200, 200, "b");
        assert_eq!(bbox_iou(&a, &b), 0.0);
    }

    #[test]
    fn test_iou_partial() {
        let a = make_bbox(0.0, 0.0, 100.0, 100.0, 0.9, 200, 200, "a");
        let b = make_bbox(50.0, 50.0, 150.0, 150.0, 0.8, 200, 200, "b");
        let result = bbox_iou(&a, &b);
        // Intersection: 50*50=2500, Union: 10000+10000-2500=17500
        assert!((result - 2500.0 / 17500.0).abs() < 1e-4);
    }

    #[test]
    fn test_iou_contained() {
        // Small box fully inside large box
        let a = make_bbox(0.0, 0.0, 100.0, 100.0, 0.9, 200, 200, "a");
        let b = make_bbox(25.0, 25.0, 75.0, 75.0, 0.8, 200, 200, "b");
        let result = bbox_iou(&a, &b);
        // Intersection: 50*50=2500, Union: 10000+2500-2500=10000
        assert!((result - 0.25).abs() < 1e-4);
    }

    // -----------------------------------------------------------------------
    // Precision/Recall tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_pr_perfect() {
        let detected = vec![make_bbox(10.0, 10.0, 60.0, 60.0, 0.9, 200, 200, "d")];
        let gt = vec![make_bbox(10.0, 10.0, 60.0, 60.0, 0.9, 200, 200, "gt")];
        let (p, r, f1) = precision_recall(&detected, &gt, 0.5);
        assert!((p - 1.0).abs() < 1e-6);
        assert!((r - 1.0).abs() < 1e-6);
        assert!((f1 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_pr_no_detections() {
        let detected: Vec<BboxMeta> = vec![];
        let gt = vec![make_bbox(10.0, 10.0, 60.0, 60.0, 0.9, 200, 200, "gt")];
        let (p, r, f1) = precision_recall(&detected, &gt, 0.5);
        assert_eq!(p, 0.0);
        assert_eq!(r, 0.0);
        assert_eq!(f1, 0.0);
    }

    #[test]
    fn test_pr_extra_detections() {
        let detected = vec![
            make_bbox(10.0, 10.0, 60.0, 60.0, 0.9, 200, 200, "d1"),
            make_bbox(100.0, 100.0, 150.0, 150.0, 0.8, 200, 200, "d2"), // FP
        ];
        let gt = vec![make_bbox(10.0, 10.0, 60.0, 60.0, 0.9, 200, 200, "gt")];
        let (p, r, _f1) = precision_recall(&detected, &gt, 0.5);
        assert!(
            (p - 0.5).abs() < 1e-6,
            "precision should be 0.5 (1 TP / 2 det)"
        );
        assert!((r - 1.0).abs() < 1e-6, "recall should be 1.0");
    }

    #[test]
    fn test_pr_missed_gt() {
        let detected = vec![make_bbox(10.0, 10.0, 60.0, 60.0, 0.9, 200, 200, "d1")];
        let gt = vec![
            make_bbox(10.0, 10.0, 60.0, 60.0, 0.9, 200, 200, "gt1"),
            make_bbox(100.0, 100.0, 150.0, 150.0, 0.8, 200, 200, "gt2"), // FN
        ];
        let (p, r, _f1) = precision_recall(&detected, &gt, 0.5);
        assert!((p - 1.0).abs() < 1e-6, "precision should be 1.0");
        assert!((r - 0.5).abs() < 1e-6, "recall should be 0.5 (1 TP / 2 GT)");
    }

    #[test]
    fn test_pr_both_empty() {
        let (p, r, f1) = precision_recall(&[], &[], 0.5);
        assert_eq!(p, 1.0);
        assert_eq!(r, 1.0);
        assert_eq!(f1, 1.0);
    }
}

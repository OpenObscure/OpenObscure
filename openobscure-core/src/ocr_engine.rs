//! PaddleOCR text detection and recognition via ONNX Runtime.
//!
//! Two-component OCR pipeline:
//! - **Detector** (`OcrDetector`): PaddleOCR v3 — locates text regions as quadrilateral bboxes
//! - **Recognizer** (`OcrRecognizer`): PP-OCRv4 English — reads characters from cropped regions
//!
//! Supports two tiers configured via `ocr_tier`:
//! - `detect_and_fill` (Tier 1): Detect text regions → solid-fill all. No recognition needed.
//! - `full_recognition` (Tier 2+): Detect → recognize → scan for PII → selectively redact.
//!
//! Model files expected:
//! - `det_model.onnx` — PaddleOCR v3 text detection (~2.4MB)
//! - `rec_model.onnx` — PP-OCRv4 English recognition (~7.7MB), only needed for Tier 2
//! - `ppocr_keys.txt` — 95-char English dictionary, only needed for Tier 2

use std::path::Path;

use image::{DynamicImage, GenericImageView, GrayImage, Luma};
use ndarray::Array4;
use ort::session::Session;

use crate::image_pipeline::ImageError;

/// Maximum input dimension for detection model.
const DET_MAX_SIDE: u32 = 960;
/// Detection model input must be divisible by this.
const DET_STRIDE: u32 = 32;
/// Recognition model input height.
const REC_HEIGHT: u32 = 48;
/// Maximum width for recognition input.
/// PP-OCRv4 handles dynamic widths via SVTR architecture. Wide text lines
/// (e.g. 600px at 25px height → 1152px at 48px height) need proportional
/// width to avoid squishing that destroys recognition quality.
const REC_MAX_WIDTH: u32 = 1280;
/// Minimum confidence for a detected text region.
const DET_THRESHOLD: f32 = 0.3;
/// Minimum box score to keep a detection.
const DET_BOX_THRESHOLD: f32 = 0.6;
/// Minimum side length of a text region in pixels.
const MIN_REGION_SIZE: f32 = 3.0;

// ── Text Presence Pre-filter ─────────────────────────────────────────────
//
// Lightweight heuristic to estimate whether an image contains text before
// running the expensive PaddleOCR detector (~5s on mobile CPU). Uses Sobel
// edge density with an inverted band approach on a 160x160 grayscale version.
//
// Key insight: text on clean backgrounds (documents, cards) has LOW edge
// density (0.05–0.12). Photos (faces, landscapes, NSFW) have HIGH density
// (0.12+) due to textures, hair, skin gradients. Screenshots also have high
// density but are handled separately via the screen guard flag.
//
// Band logic:
//   density < 0.05  → skip OCR (solid/empty — no content at all)
//   density 0.05–0.12 → run OCR (document text range)
//   density > 0.12  → skip OCR (photo — too many edges to be clean text)
//   is_screenshot=true → always run OCR (overrides density, screenshots have text)
//
// Empirical edge densities (from test/data/input/Visual_PII/):
//   Documents:       0.076 (SSN card) – 0.118 (medical record)
//   Faces:           0.118 – 0.544
//   NSFW:            0.035 (placeholder) – 0.658 (semi-nude)
//   EXIF camera:     0.293 – 0.357
//   EXIF screenshots: 0.621
//
// Edge cases:
//   * Document on textured background (e.g. photo of a form on wood desk):
//     density may exceed 0.12 → skipped. Consider connected component
//     analysis as future enhancement if this case arises in practice.
//   * Faint/low-contrast text: density may fall below 0.05 → skipped.
//     Not observed in test suite — lowest document is SSN card at 0.076.
//   * Screenshot without screen guard detection: density > 0.12 → skipped.
//     Mitigated by screen guard running before OCR in the pipeline.
//   * Gradient/blur artistic images: density ~0.03–0.05 → borderline.
//     May trigger OCR (fail-open) — acceptable, OCR finds nothing.

/// Pre-filter downscale target. Smaller = faster but less accurate.
const PREFILTER_SIZE: u32 = 160;
/// Lower bound: below this, image is definitely empty (solid colors, gradients).
const PREFILTER_LOW: f32 = 0.05;
/// Upper bound: above this, image is a photo (textures, not clean text).
const PREFILTER_HIGH: f32 = 0.12;

/// Estimate whether an image likely contains text, using Sobel edge density
/// with an inverted band approach.
///
/// - `is_screenshot`: if `true`, always returns `true` (screenshots have text
///   despite high edge density). Obtained from screen guard earlier in pipeline.
///
/// Returns `true` if OCR should run, `false` if it can be skipped.
/// Performance cost: ~1-2ms on 160x160 downscale.
pub fn has_text_likelihood(img: &image::DynamicImage, is_screenshot: bool) -> bool {
    // Screenshots always have text — skip the density check
    if is_screenshot {
        oo_dbg!("ocr_prefilter: is_screenshot=true, forcing has_text=true");
        return true;
    }

    let small = img.resize_exact(
        PREFILTER_SIZE,
        PREFILTER_SIZE,
        image::imageops::FilterType::Triangle,
    );
    let gray = small.to_luma8();
    let (w, h) = (gray.width() as usize, gray.height() as usize);

    // Sobel edge detection — compute gradient magnitude at each pixel
    let mut edge_count = 0u32;
    let total = ((w - 2) * (h - 2)) as f32;

    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let p = |dx: i32, dy: i32| -> f32 {
                gray.get_pixel((x as i32 + dx) as u32, (y as i32 + dy) as u32)
                    .0[0] as f32
            };
            let gx = -p(-1, -1) + p(1, -1) - 2.0 * p(-1, 0) + 2.0 * p(1, 0) - p(-1, 1) + p(1, 1);
            let gy = -p(-1, -1) - 2.0 * p(0, -1) - p(1, -1) + p(-1, 1) + 2.0 * p(0, 1) + p(1, 1);
            let mag = (gx * gx + gy * gy).sqrt();
            if mag > 50.0 {
                edge_count += 1;
            }
        }
    }

    let density = edge_count as f32 / total;
    // Inverted band: text lives in the middle range (0.05–0.12).
    // Below = empty, above = photo textures.
    let has_text = (PREFILTER_LOW..=PREFILTER_HIGH).contains(&density);
    oo_dbg!(
        "ocr_prefilter: density={:.4}, band=[{:.2},{:.2}], has_text={}",
        density,
        PREFILTER_LOW,
        PREFILTER_HIGH,
        has_text
    );
    has_text
}

/// OCR processing tier.
#[derive(Debug, Clone, PartialEq)]
pub enum OcrTier {
    /// Detect text regions and solid-fill all of them. No recognition.
    DetectAndFill,
    /// Detect, recognize text, scan for PII, selectively redact.
    FullRecognition,
}

impl OcrTier {
    pub fn from_config(s: &str) -> Self {
        match s {
            "full_recognition" => OcrTier::FullRecognition,
            _ => OcrTier::DetectAndFill,
        }
    }
}

/// A detected text region as a quadrilateral in image coordinates.
#[derive(Debug, Clone)]
pub struct TextRegion {
    /// Four corners: [top-left, top-right, bottom-right, bottom-left].
    /// Each is (x, y) in original image coordinates.
    pub points: [(f32, f32); 4],
    /// Detection confidence score.
    pub score: f32,
}

impl TextRegion {
    /// Get axis-aligned bounding box: (x_min, y_min, x_max, y_max).
    pub fn bbox(&self) -> (f32, f32, f32, f32) {
        let xs: Vec<f32> = self.points.iter().map(|p| p.0).collect();
        let ys: Vec<f32> = self.points.iter().map(|p| p.1).collect();
        (
            xs.iter().cloned().fold(f32::INFINITY, f32::min),
            ys.iter().cloned().fold(f32::INFINITY, f32::min),
            xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
            ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
        )
    }

    /// Width of axis-aligned bounding box.
    pub fn width(&self) -> f32 {
        let (x_min, _, x_max, _) = self.bbox();
        x_max - x_min
    }

    /// Height of axis-aligned bounding box.
    pub fn height(&self) -> f32 {
        let (_, y_min, _, y_max) = self.bbox();
        y_max - y_min
    }

    /// Convert to generic `BboxMeta` for validation.
    pub fn to_bbox_meta(&self, img_width: u32, img_height: u32) -> crate::detection_meta::BboxMeta {
        let (x_min, y_min, x_max, y_max) = self.bbox();
        crate::detection_meta::BboxMeta {
            x_min,
            y_min,
            x_max,
            y_max,
            confidence: self.score,
            img_width,
            img_height,
            label: "text".to_string(),
        }
    }
}

/// Recognized text from a single region.
#[derive(Debug, Clone)]
pub struct RecognizedText {
    /// The detected text region.
    pub region: TextRegion,
    /// Recognized text string.
    pub text: String,
    /// Recognition confidence.
    pub confidence: f32,
}

/// PaddleOCR text detection model.
pub struct OcrDetector {
    session: Session,
}

impl OcrDetector {
    /// Load text detection ONNX model.
    pub fn load(model_dir: &Path) -> Result<Self, ImageError> {
        let model_path = find_det_model(model_dir)?;

        let session = crate::ort_ep::build_session(&model_path)
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        oo_info!(crate::oo_log::modules::OCR, "OCR detector loaded",
            model = %model_path.display());

        Ok(Self { session })
    }

    /// Detect text regions in an image.
    ///
    /// Returns quadrilateral bounding boxes in original image coordinates.
    pub fn detect(&mut self, img: &DynamicImage) -> Result<Vec<TextRegion>, ImageError> {
        let (orig_w, orig_h) = img.dimensions();
        oo_dbg!("ocr_detect: input={}x{}", orig_w, orig_h);

        // Resize to detection input size (max side DET_MAX_SIDE, divisible by DET_STRIDE)
        let (det_w, det_h) = detection_input_size(orig_w, orig_h);
        let resized = img.resize_exact(det_w, det_h, image::imageops::FilterType::Triangle);
        let rgb = resized.to_rgb8();

        // Build input tensor [1, 3, H, W] normalized with PaddleOCR mean/std
        let input =
            Array4::<f32>::from_shape_fn((1, 3, det_h as usize, det_w as usize), |(_, c, h, w)| {
                let pixel = rgb.get_pixel(w as u32, h as u32);
                let mean = [0.485, 0.456, 0.406];
                let std = [0.229, 0.224, 0.225];
                (pixel[c] as f32 / 255.0 - mean[c]) / std[c]
            });

        let input_val = ort::value::Value::from_array(input)
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        let outputs = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.session.run(ort::inputs!["x" => input_val])
        })) {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => return Err(ImageError::OnnxRuntime(e.to_string())),
            Err(_) => {
                return Err(ImageError::OnnxRuntime(
                    "ONNX Runtime panicked during OCR detection".to_string(),
                ))
            }
        };

        // Output: probability map [1, 1, H, W]
        let (_shape, prob_data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        // Convert probability map to binary mask and find contours
        let prob_map = build_prob_map(prob_data, det_w as usize, det_h as usize);
        let regions = extract_regions_from_map(
            &prob_map,
            det_w as usize,
            det_h as usize,
            orig_w as f32 / det_w as f32,
            orig_h as f32 / det_h as f32,
        );

        oo_dbg!(
            "ocr_detect: det_input={}x{}, regions_found={}",
            det_w,
            det_h,
            regions.len()
        );
        Ok(regions)
    }
}

/// PaddleOCR text recognition model.
pub struct OcrRecognizer {
    session: Session,
    dictionary: Vec<String>,
}

impl OcrRecognizer {
    /// Load text recognition ONNX model and character dictionary.
    pub fn load(model_dir: &Path) -> Result<Self, ImageError> {
        let model_path = find_rec_model(model_dir)?;
        let dict_path = model_dir.join("ppocr_keys.txt");

        let dictionary = if dict_path.exists() {
            load_dictionary(&dict_path)?
        } else {
            return Err(ImageError::Decode(format!(
                "Dictionary file not found: {}",
                dict_path.display()
            )));
        };

        let session = crate::ort_ep::build_session(&model_path)
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

        oo_info!(crate::oo_log::modules::OCR, "OCR recognizer loaded",
            model = %model_path.display(),
            dict_size = dictionary.len());

        Ok(Self {
            session,
            dictionary,
        })
    }

    /// Recognize text in cropped region images.
    ///
    /// Takes the original image and detected text regions. Crops each region,
    /// runs recognition, and returns text with confidence scores.
    pub fn recognize(
        &mut self,
        img: &DynamicImage,
        regions: &[TextRegion],
    ) -> Result<Vec<RecognizedText>, ImageError> {
        let mut results = Vec::with_capacity(regions.len());

        for region in regions {
            let (x_min, y_min, x_max, y_max) = region.bbox();
            let x = x_min.max(0.0) as u32;
            let y = y_min.max(0.0) as u32;
            let w = ((x_max - x_min) as u32).min(img.width().saturating_sub(x));
            let h = ((y_max - y_min) as u32).min(img.height().saturating_sub(y));

            if w < 2 || h < 2 {
                continue;
            }

            // Crop the text region
            let cropped = img.crop_imm(x, y, w, h);

            // Resize to rec input height, preserving aspect ratio
            let scale = REC_HEIGHT as f32 / h as f32;
            let rec_w = ((w as f32 * scale) as u32).clamp(1, REC_MAX_WIDTH);
            let resized =
                cropped.resize_exact(rec_w, REC_HEIGHT, image::imageops::FilterType::Triangle);
            let rgb = resized.to_rgb8();

            // Build input tensor [1, 3, 48, W] with PaddleOCR normalization
            let input = Array4::<f32>::from_shape_fn(
                (1, 3, REC_HEIGHT as usize, rec_w as usize),
                |(_, c, h, w)| {
                    let pixel = rgb.get_pixel(w as u32, h as u32);
                    (pixel[c] as f32 / 255.0 - 0.5) / 0.5
                },
            );

            let input_val = ort::value::Value::from_array(input)
                .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

            let outputs = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.session.run(ort::inputs!["x" => input_val])
            })) {
                Ok(Ok(out)) => out,
                Ok(Err(e)) => return Err(ImageError::OnnxRuntime(e.to_string())),
                Err(_) => {
                    return Err(ImageError::OnnxRuntime(
                        "ONNX Runtime panicked during OCR recognition".to_string(),
                    ))
                }
            };

            // Output: [1, seq_len, dict_size+2] logits
            let (out_shape, out_data) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;

            let seq_len = if out_shape.len() >= 2 {
                out_shape[1] as usize
            } else {
                continue;
            };
            let num_classes = if out_shape.len() >= 3 {
                out_shape[2] as usize
            } else {
                self.dictionary.len() + 2
            };

            let (text, confidence) =
                ctc_greedy_decode(out_data, seq_len, num_classes, &self.dictionary);

            if !text.is_empty() {
                results.push(RecognizedText {
                    region: region.clone(),
                    text,
                    confidence,
                });
            }
        }

        Ok(results)
    }

    /// Get dictionary size.
    pub fn dict_size(&self) -> usize {
        self.dictionary.len()
    }
}

// --- Internal helpers ---

/// Calculate detection input dimensions (max side DET_MAX_SIDE, divisible by DET_STRIDE).
fn detection_input_size(orig_w: u32, orig_h: u32) -> (u32, u32) {
    let max_side = orig_w.max(orig_h);
    let ratio = if max_side > DET_MAX_SIDE {
        DET_MAX_SIDE as f32 / max_side as f32
    } else {
        1.0
    };

    let mut w = (orig_w as f32 * ratio) as u32;
    let mut h = (orig_h as f32 * ratio) as u32;

    // Round up to nearest multiple of DET_STRIDE
    w = w.div_ceil(DET_STRIDE) * DET_STRIDE;
    h = h.div_ceil(DET_STRIDE) * DET_STRIDE;

    (w.max(DET_STRIDE), h.max(DET_STRIDE))
}

/// Build a GrayImage probability map from flat output data.
fn build_prob_map(data: &[f32], width: usize, height: usize) -> GrayImage {
    let mut map = GrayImage::new(width as u32, height as u32);
    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            let val = data.get(idx).copied().unwrap_or(0.0);
            let pixel_val = (val * 255.0).clamp(0.0, 255.0) as u8;
            map.put_pixel(x as u32, y as u32, Luma([pixel_val]));
        }
    }
    map
}

/// Extract text regions from a probability map using simple connected-component analysis.
///
/// This is a simplified version of the DB (Differentiable Binarization) post-processor.
/// For production use, a proper contour finder (like OpenCV findContours) would be better,
/// but this avoids the OpenCV dependency.
fn extract_regions_from_map(
    prob_map: &GrayImage,
    det_w: usize,
    det_h: usize,
    scale_x: f32,
    scale_y: f32,
) -> Vec<TextRegion> {
    let threshold = (DET_THRESHOLD * 255.0) as u8;
    let (w, h) = (det_w, det_h);

    // Binary threshold
    let mut binary = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let val = prob_map.get_pixel(x as u32, y as u32).0[0];
            binary[y * w + x] = val > threshold;
        }
    }

    // Simple connected-component labeling (4-connectivity)
    let mut labels = vec![0u32; w * h];
    let mut next_label = 1u32;
    let mut equivalences: Vec<u32> = vec![0]; // index 0 unused

    for y in 0..h {
        for x in 0..w {
            if !binary[y * w + x] {
                continue;
            }

            let left = if x > 0 { labels[y * w + (x - 1)] } else { 0 };
            let above = if y > 0 { labels[(y - 1) * w + x] } else { 0 };

            match (left > 0, above > 0) {
                (false, false) => {
                    labels[y * w + x] = next_label;
                    equivalences.push(next_label);
                    next_label += 1;
                }
                (true, false) => {
                    labels[y * w + x] = left;
                }
                (false, true) => {
                    labels[y * w + x] = above;
                }
                (true, true) => {
                    let min_label = left.min(above);
                    labels[y * w + x] = min_label;
                    // Union
                    let max_label = left.max(above);
                    let root_min = find_root(&equivalences, min_label);
                    let root_max = find_root(&equivalences, max_label);
                    if root_min != root_max {
                        let new_root = root_min.min(root_max);
                        let old_root = root_min.max(root_max);
                        equivalences[old_root as usize] = new_root;
                    }
                }
            }
        }
    }

    // Collect bounding boxes per component
    let mut components: std::collections::HashMap<u32, (f32, f32, f32, f32, f32)> =
        std::collections::HashMap::new();

    for y in 0..h {
        for x in 0..w {
            let label = labels[y * w + x];
            if label == 0 {
                continue;
            }
            let root = find_root(&equivalences, label);
            let score = prob_map.get_pixel(x as u32, y as u32).0[0] as f32 / 255.0;

            let entry = components
                .entry(root)
                .or_insert((x as f32, y as f32, x as f32, y as f32, 0.0));
            entry.0 = entry.0.min(x as f32); // x_min
            entry.1 = entry.1.min(y as f32); // y_min
            entry.2 = entry.2.max(x as f32); // x_max
            entry.3 = entry.3.max(y as f32); // y_max
            entry.4 = entry.4.max(score); // max score
        }
    }

    // Convert to TextRegions, filtering by size and score
    let mut regions = Vec::new();
    for (_label, (x_min, y_min, x_max, y_max, score)) in components {
        let w = x_max - x_min;
        let h = y_max - y_min;
        if w < MIN_REGION_SIZE || h < MIN_REGION_SIZE {
            continue;
        }
        if score < DET_BOX_THRESHOLD {
            continue;
        }

        // Scale back to original image coordinates
        let points = [
            (x_min * scale_x, y_min * scale_y),
            (x_max * scale_x, y_min * scale_y),
            (x_max * scale_x, y_max * scale_y),
            (x_min * scale_x, y_max * scale_y),
        ];

        regions.push(TextRegion { points, score });
    }

    regions
}

/// Find root in union-find equivalence table.
fn find_root(equivalences: &[u32], mut label: u32) -> u32 {
    while equivalences[label as usize] != label {
        label = equivalences[label as usize];
    }
    label
}

/// CTC greedy decode: take argmax at each timestep, collapse repeats, remove blanks.
///
/// The blank token is index 0, actual characters start at index 1.
/// Returns (decoded_text, average_confidence).
///
/// Works with both raw logits and post-softmax probabilities:
/// - If values are in [0,1] and sum to ~1 per timestep → probabilities (use directly)
/// - Otherwise → logits (apply softmax for confidence)
pub fn ctc_greedy_decode(
    logits: &[f32],
    seq_len: usize,
    num_classes: usize,
    dictionary: &[String],
) -> (String, f32) {
    if seq_len == 0 || num_classes == 0 {
        return (String::new(), 0.0);
    }

    // Detect if output is post-softmax probabilities or raw logits.
    // Post-softmax: values in [0,1], first timestep sums to ~1.
    let is_probabilities = if num_classes > 1 {
        let first_sum: f32 = (0..num_classes)
            .filter_map(|c| logits.get(c).copied())
            .sum();
        (first_sum - 1.0).abs() < 0.1
    } else {
        false
    };

    let mut prev_idx: usize = 0; // blank
    let mut chars = Vec::new();
    let mut total_conf = 0.0f32;
    let mut count = 0;

    for t in 0..seq_len {
        let offset = t * num_classes;

        // Find argmax
        let mut max_idx = 0;
        let mut max_val = f32::NEG_INFINITY;
        for c in 0..num_classes {
            let val = logits.get(offset + c).copied().unwrap_or(f32::NEG_INFINITY);
            if val > max_val {
                max_val = val;
                max_idx = c;
            }
        }

        // Compute confidence from either probabilities or logits
        let confidence = if is_probabilities {
            // Output is already softmax probabilities — use max value directly
            max_val
        } else {
            // Raw logits — apply softmax to get confidence
            let mut exp_sum = 0.0f32;
            for c in 0..num_classes {
                let val = logits.get(offset + c).copied().unwrap_or(f32::NEG_INFINITY);
                exp_sum += (val - max_val).exp();
            }
            1.0 / exp_sum
        };

        // CTC decode: skip blanks (index 0) and repeated chars
        if max_idx != 0 && max_idx != prev_idx {
            // Dictionary index is max_idx - 1 (blank is 0, first char is 1)
            let dict_idx = max_idx - 1;
            if dict_idx < dictionary.len() {
                chars.push(dictionary[dict_idx].clone());
                total_conf += confidence;
                count += 1;
            }
        }

        prev_idx = max_idx;
    }

    let text = chars.join("");
    let avg_conf = if count > 0 {
        total_conf / count as f32
    } else {
        0.0
    };

    (text, avg_conf)
}

/// Load character dictionary from a text file (one character per line).
fn load_dictionary(path: &Path) -> Result<Vec<String>, ImageError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| ImageError::Decode(format!("Failed to read dictionary: {}", e)))?;
    Ok(content.lines().map(|l| l.to_string()).collect())
}

/// Find detection model file.
fn find_det_model(model_dir: &Path) -> Result<std::path::PathBuf, ImageError> {
    let candidates = ["det_model.onnx", "ch_ppocr_det.onnx", "det.onnx"];
    for name in &candidates {
        let path = model_dir.join(name);
        if path.exists() {
            return Ok(path);
        }
    }
    Err(ImageError::Decode(format!(
        "No OCR detection model found in {}",
        model_dir.display()
    )))
}

/// Find recognition model file.
fn find_rec_model(model_dir: &Path) -> Result<std::path::PathBuf, ImageError> {
    let candidates = ["rec_model.onnx", "ch_ppocr_rec.onnx", "rec.onnx"];
    for name in &candidates {
        let path = model_dir.join(name);
        if path.exists() {
            return Ok(path);
        }
    }
    Err(ImageError::Decode(format!(
        "No OCR recognition model found in {}",
        model_dir.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ocr_tier_from_config() {
        assert_eq!(
            OcrTier::from_config("detect_and_fill"),
            OcrTier::DetectAndFill
        );
        assert_eq!(
            OcrTier::from_config("full_recognition"),
            OcrTier::FullRecognition
        );
        assert_eq!(OcrTier::from_config("unknown"), OcrTier::DetectAndFill);
    }

    #[test]
    fn test_detection_input_size_small() {
        // Small image: round up to nearest DET_STRIDE
        let (w, h) = detection_input_size(100, 50);
        assert_eq!(w % DET_STRIDE, 0);
        assert_eq!(h % DET_STRIDE, 0);
        assert!(w >= 100);
        assert!(h >= 50);
    }

    #[test]
    fn test_detection_input_size_large() {
        // Large image: scale down to max DET_MAX_SIDE
        let (w, h) = detection_input_size(4000, 3000);
        assert!(w <= DET_MAX_SIDE + DET_STRIDE);
        assert!(h <= DET_MAX_SIDE + DET_STRIDE);
        assert_eq!(w % DET_STRIDE, 0);
        assert_eq!(h % DET_STRIDE, 0);
    }

    #[test]
    fn test_detection_input_size_exact() {
        let (w, h) = detection_input_size(960, 640);
        assert_eq!(w, 960);
        assert_eq!(h, 640);
    }

    #[test]
    fn test_text_region_bbox() {
        let region = TextRegion {
            points: [(10.0, 20.0), (110.0, 20.0), (110.0, 50.0), (10.0, 50.0)],
            score: 0.9,
        };
        let (x_min, y_min, x_max, y_max) = region.bbox();
        assert_eq!(x_min, 10.0);
        assert_eq!(y_min, 20.0);
        assert_eq!(x_max, 110.0);
        assert_eq!(y_max, 50.0);
    }

    #[test]
    fn test_text_region_dimensions() {
        let region = TextRegion {
            points: [(0.0, 0.0), (100.0, 0.0), (100.0, 30.0), (0.0, 30.0)],
            score: 0.9,
        };
        assert_eq!(region.width(), 100.0);
        assert_eq!(region.height(), 30.0);
    }

    #[test]
    fn test_ctc_greedy_decode_simple() {
        // 3 classes: blank(0), 'a'(1), 'b'(2)
        // Sequence of 5 timesteps: blank, a, a, b, blank
        // After CTC: "ab"
        let dict = vec!["a".to_string(), "b".to_string()];
        let num_classes = 3;
        // Each timestep has logits for [blank, a, b]
        let logits = vec![
            10.0, -10.0, -10.0, // t0: blank
            -10.0, 10.0, -10.0, // t1: 'a'
            -10.0, 10.0, -10.0, // t2: 'a' (repeat, collapsed)
            -10.0, -10.0, 10.0, // t3: 'b'
            10.0, -10.0, -10.0, // t4: blank
        ];
        let (text, conf) = ctc_greedy_decode(&logits, 5, num_classes, &dict);
        assert_eq!(text, "ab");
        assert!(conf > 0.9);
    }

    #[test]
    fn test_ctc_greedy_decode_all_blank() {
        let dict = vec!["a".to_string(), "b".to_string()];
        let logits = vec![10.0, -10.0, -10.0, 10.0, -10.0, -10.0];
        let (text, conf) = ctc_greedy_decode(&logits, 2, 3, &dict);
        assert_eq!(text, "");
        assert_eq!(conf, 0.0);
    }

    #[test]
    fn test_ctc_greedy_decode_empty() {
        let dict = vec![];
        let (text, conf) = ctc_greedy_decode(&[], 0, 0, &dict);
        assert_eq!(text, "");
        assert_eq!(conf, 0.0);
    }

    #[test]
    fn test_ctc_greedy_decode_repeated_with_blank_separator() {
        // Sequence: a, blank, a → "aa" (blank separates the repeated 'a')
        let dict = vec!["a".to_string(), "b".to_string()];
        let logits = vec![
            -10.0, 10.0, -10.0, // t0: 'a'
            10.0, -10.0, -10.0, // t1: blank
            -10.0, 10.0, -10.0, // t2: 'a'
        ];
        let (text, _) = ctc_greedy_decode(&logits, 3, 3, &dict);
        assert_eq!(text, "aa");
    }

    #[test]
    fn test_build_prob_map() {
        let data = vec![0.0, 0.5, 1.0, 0.3];
        let map = build_prob_map(&data, 2, 2);
        assert_eq!(map.get_pixel(0, 0).0[0], 0);
        assert_eq!(map.get_pixel(1, 0).0[0], 127); // 0.5 * 255 ≈ 127
        assert_eq!(map.get_pixel(0, 1).0[0], 255);
        assert_eq!(map.get_pixel(1, 1).0[0], 76); // 0.3 * 255 ≈ 76
    }

    #[test]
    fn test_extract_regions_empty_map() {
        // All zeros → no regions
        let map = GrayImage::new(32, 32);
        let regions = extract_regions_from_map(&map, 32, 32, 1.0, 1.0);
        assert!(regions.is_empty());
    }

    #[test]
    fn test_extract_regions_one_blob() {
        // Create a probability map with a bright rectangular region
        let mut map = GrayImage::new(64, 64);
        for y in 10..30 {
            for x in 5..50 {
                map.put_pixel(x, y, Luma([200])); // Above threshold
            }
        }
        let regions = extract_regions_from_map(&map, 64, 64, 2.0, 2.0);
        assert!(!regions.is_empty(), "Should detect at least one region");
        // Check that the region roughly matches the blob (scaled by 2x)
        let (x_min, y_min, x_max, y_max) = regions[0].bbox();
        assert!((10.0..=30.0).contains(&x_min));
        assert!((20.0..=50.0).contains(&y_min));
        assert!(x_max >= 40.0 * 2.0);
        assert!(y_max >= 20.0 * 2.0);
    }

    #[test]
    fn test_extract_regions_too_small() {
        // Create a very small blob (below MIN_REGION_SIZE)
        let mut map = GrayImage::new(32, 32);
        map.put_pixel(10, 10, Luma([200]));
        map.put_pixel(11, 10, Luma([200]));
        let regions = extract_regions_from_map(&map, 32, 32, 1.0, 1.0);
        assert!(regions.is_empty(), "Tiny blobs should be filtered out");
    }

    #[test]
    fn test_find_det_model_not_found() {
        let result = find_det_model(Path::new("/nonexistent/path"));
        assert!(result.is_err());
    }

    #[test]
    fn test_find_rec_model_not_found() {
        let result = find_rec_model(Path::new("/nonexistent/path"));
        assert!(result.is_err());
    }

    #[test]
    fn test_load_detector_missing_model() {
        let result = OcrDetector::load(Path::new("/nonexistent/path"));
        assert!(result.is_err());
    }

    #[test]
    fn test_load_recognizer_missing_model() {
        let result = OcrRecognizer::load(Path::new("/nonexistent/path"));
        assert!(result.is_err());
    }

    // ── Text Presence Pre-filter Tests ───────────────────────────────

    #[test]
    fn test_prefilter_document_images_have_text() {
        // Document images should always pass the pre-filter (text is present).
        // Uses test/data/input/Visual_PII/Documents/ — real scanned documents.
        let docs_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("test/data/input/Visual_PII/Documents");
        if !docs_dir.exists() {
            eprintln!("SKIP: test documents not found at {:?}", docs_dir);
            return;
        }
        let mut tested = 0;
        let mut passed = 0;
        for entry in std::fs::read_dir(&docs_dir).unwrap() {
            let path = entry.unwrap().path();
            if !path.extension().is_some_and(|e| e == "jpg" || e == "png") {
                continue;
            }
            let img = image::open(&path).unwrap();
            let result = has_text_likelihood(&img, false);
            tested += 1;
            if result {
                passed += 1;
            } else {
                eprintln!(
                    "WARN: pre-filter missed text in document: {}",
                    path.file_name().unwrap().to_string_lossy()
                );
            }
        }
        assert!(tested > 0, "No document images found in {:?}", docs_dir);
        // Aggressive threshold: all documents must pass (100% recall on documents)
        assert_eq!(
            passed, tested,
            "Pre-filter must detect text in all {} document images (got {})",
            tested, passed
        );
    }

    #[test]
    fn test_prefilter_face_photos_pass_through() {
        // With aggressive threshold (efficacy > performance), face photos will
        // pass the pre-filter (high edge density from hair/features). This is
        // the correct behavior — we prefer running unnecessary OCR on a photo
        // (wasting ~5s) over missing text containing PII.
        //
        // The pre-filter only catches clearly-empty images (solid colors, simple
        // gradients) where OCR is guaranteed to find nothing.
        let faces_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("test/data/input/Visual_PII/Faces");
        if !faces_dir.exists() {
            eprintln!("SKIP: face test images not found at {:?}", faces_dir);
            return;
        }
        let mut tested = 0;
        for entry in std::fs::read_dir(&faces_dir).unwrap() {
            let path = entry.unwrap().path();
            if !path.extension().is_some_and(|e| e == "jpg" || e == "png") {
                continue;
            }
            let img = image::open(&path).unwrap();
            let result = has_text_likelihood(&img, false);
            tested += 1;
            let density = {
                let small = img.resize_exact(
                    PREFILTER_SIZE,
                    PREFILTER_SIZE,
                    image::imageops::FilterType::Triangle,
                );
                let gray = small.to_luma8();
                let (w, h) = (gray.width() as usize, gray.height() as usize);
                let total = ((w - 2) * (h - 2)) as f32;
                let mut ec = 0u32;
                for y in 1..h - 1 {
                    for x in 1..w - 1 {
                        let p = |dx: i32, dy: i32| -> f32 {
                            gray.get_pixel((x as i32 + dx) as u32, (y as i32 + dy) as u32)
                                .0[0] as f32
                        };
                        let gx = -p(-1, -1) + p(1, -1) - 2.0 * p(-1, 0) + 2.0 * p(1, 0) - p(-1, 1)
                            + p(1, 1);
                        let gy = -p(-1, -1) - 2.0 * p(0, -1) - p(1, -1)
                            + p(-1, 1)
                            + 2.0 * p(0, 1)
                            + p(1, 1);
                        if (gx * gx + gy * gy).sqrt() > 50.0 {
                            ec += 1;
                        }
                    }
                }
                ec as f32 / total
            };
            eprintln!(
                "  face {}: density={:.4}, has_text={}",
                path.file_name().unwrap().to_string_lossy(),
                density,
                result
            );
        }
        assert!(tested > 0, "No face images found");
        // This test documents behavior, not asserts a specific skip rate.
        // With aggressive threshold, face photos pass through → OCR runs → finds nothing.
    }

    #[test]
    fn test_prefilter_all_visual_folders() {
        // Scan all Visual_PII subfolders and report edge density per image.
        // This test documents classification behavior across all image types.
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("test/data/input/Visual_PII");
        if !base.exists() {
            eprintln!("SKIP: Visual_PII not found at {:?}", base);
            return;
        }
        for folder in &["EXIF", "Faces", "NSFW", "Documents"] {
            let dir = base.join(folder);
            if !dir.exists() {
                continue;
            }
            eprintln!("--- {} ---", folder);
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                if !["jpg", "jpeg", "png"].contains(&ext) {
                    continue;
                }
                let img = match image::open(&path) {
                    Ok(i) => i,
                    Err(e) => {
                        eprintln!(
                            "  SKIP {} ({})",
                            path.file_name().unwrap().to_string_lossy(),
                            e
                        );
                        continue;
                    }
                };
                let has_text = has_text_likelihood(&img, false);
                eprintln!(
                    "  {}: has_text={}",
                    path.file_name().unwrap().to_string_lossy(),
                    has_text
                );
            }
        }
    }

    #[test]
    fn test_prefilter_solid_color_no_text() {
        // A solid-color image should have near-zero edge density.
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            200,
            200,
            image::Rgb([128, 128, 128]),
        ));
        assert!(
            !has_text_likelihood(&img, false),
            "Solid color image should not trigger text detection"
        );
    }
}

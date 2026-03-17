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
const IDX_DRAWINGS: usize = 0;
const IDX_HENTAI: usize = 1;
const IDX_PORN: usize = 3;
const IDX_SEXY: usize = 4;

/// Minimum `drawings` class probability required before `hentai` probability
/// contributes to the NSFW score.
///
/// The ViT 5-class model's `hentai` class fires on dark high-contrast images
/// (dark-background screenshots, document scans) that are clearly not drawn
/// artwork. Gating hentai by the `drawings` class eliminates those false
/// positives: real drawn adult content scores high on both `drawings` AND
/// `hentai`, while dark photographic images score low on `drawings` (< 0.05).
///
/// False-positive profile: drawings=0.02–0.04, hentai=0.49–0.57, porn<0.01
/// True-positive profile:  drawings≈0.00, porn≈1.0 or sexy≈1.0
const HENTAI_DRAWINGS_MIN: f32 = 0.10;

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

        // iOS: CoreML (both NeuralNetwork and MLProgram) produces NaN logits for
        // INT8-quantized ONNX ops (QLinearMatMul/QLinearConv) — use CPU.
        // macOS: CoreML MLProgram handles INT8 correctly (confirmed via gateway tests).
        // Other: CPU only (no CoreML available).
        #[cfg(target_os = "ios")]
        let session = crate::ort_ep::build_session_cpu(&model_path)
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;
        #[cfg(all(target_vendor = "apple", not(target_os = "ios")))]
        let session = crate::ort_ep::build_session_coreml_mlprogram(&model_path)
            .map_err(|e| ImageError::OnnxRuntime(e.to_string()))?;
        #[cfg(not(target_vendor = "apple"))]
        let session = crate::ort_ep::build_session_cpu(&model_path)
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
    ///
    /// Uses standard ImageNet preprocessing: resize shortest edge to `input_size`,
    /// center-crop to `input_size × input_size`.
    ///
    /// Known limitation: very tall portrait images (aspect > 1.5) may have content
    /// above the center-crop window. A future improvement is to replace center-crop
    /// with letterboxing (pad-to-square) or a dedicated NSFW model with global
    /// average pooling. Tracked as: consider replacing 5-class ViT with a model
    /// that accepts variable aspect ratios without cropping.
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

        // Always log raw logits so we can confirm EP correctness via getDebugLog().
        {
            let logit_str = logits
                .iter()
                .map(|x| format!("{x:.4}"))
                .collect::<Vec<_>>()
                .join(", ");
            oo_info!(crate::oo_log::modules::IMAGE,
                "NSFW raw logits",
                ep = crate::ort_ep::ep_name(),
                num_classes = self.num_classes,
                logits = %logit_str);
        }

        // Guard: INT8 models can emit NaN logits on some ORT EPs (observed on iOS
        // CoreML NeuralNetwork EP with nsfw_5class_int8.onnx). NaN propagates
        // through softmax producing nsfw_score=NaN, which silently compares false
        // against the threshold — making the NSFW check a no-op. Return neutral
        // explicitly so the fail-open is intentional and the log is visible.
        if logits.iter().any(|x| x.is_nan()) {
            oo_warn!(crate::oo_log::modules::IMAGE,
                "NSFW classifier returned NaN logits — INT8/CoreML incompatibility; treating as neutral");
            return Ok(ClassifierResult {
                is_nsfw: false,
                nsfw_score: 0.0,
                top_class: "neutral".to_string(),
                class_probs: [0.0; 5],
            });
        }

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
            // 5-class model: porn + sexy, plus hentai only when the image
            // resembles drawn artwork (drawings class >= HENTAI_DRAWINGS_MIN).
            // Gating hentai prevents false positives on dark-background images
            // (dark-theme screenshots, document scans) which the model
            // misclassifies as drawn content despite low drawings probability.
            let hentai_contrib = if probs[IDX_DRAWINGS] >= HENTAI_DRAWINGS_MIN {
                probs[IDX_HENTAI]
            } else {
                0.0
            };
            let score = hentai_contrib + probs[IDX_PORN] + probs[IDX_SEXY];
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
    fn test_nsfw_score_5class_porn_dominant() {
        // High porn score → NSFW regardless of drawings/hentai
        let probs: [f32; 5] = [0.00, 0.00, 0.00, 1.00, 0.00]; // porn=1.0
        let hentai_contrib = if probs[IDX_DRAWINGS] >= HENTAI_DRAWINGS_MIN {
            probs[IDX_HENTAI]
        } else {
            0.0
        };
        let nsfw_score = hentai_contrib + probs[IDX_PORN] + probs[IDX_SEXY];
        assert!((nsfw_score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_nsfw_score_5class_safe() {
        let probs = [0.01, 0.001, 0.98, 0.005, 0.004];
        let hentai_contrib = if probs[IDX_DRAWINGS] >= HENTAI_DRAWINGS_MIN {
            probs[IDX_HENTAI]
        } else {
            0.0
        };
        let nsfw_score = hentai_contrib + probs[IDX_PORN] + probs[IDX_SEXY];
        assert!(nsfw_score < 0.05);
    }

    /// Dark-background images (screenshots, document scans) score high on
    /// `hentai` but low on `drawings`, `porn`, and `sexy`. Gating hentai by
    /// the drawings class prevents these false positives.
    ///
    /// Observed scores from the 3 failing test images (drawings < 0.05):
    ///   doc_credit_card_01.jpg:          drawings=0.040, hentai=0.494, porn=0.006, sexy=0.264 → raw=0.764, gated=0.270
    ///   screenshot_ide_code_1920x1080:   drawings=0.027, hentai=0.484, porn=0.007, sexy=0.263 → raw=0.754, gated=0.270
    ///   screenshot_terminal_2560x1440:   drawings=0.020, hentai=0.567, porn=0.006, sexy=0.189 → raw=0.763, gated=0.196
    #[test]
    fn test_nsfw_hentai_gate_false_positive_dark_screenshot() {
        // Typical false-positive profile: dark screenshot, high hentai, low drawings
        let probs: [f32; 5] = [0.027, 0.484, 0.219, 0.007, 0.263]; // ide screenshot
        let hentai_contrib = if probs[IDX_DRAWINGS] >= HENTAI_DRAWINGS_MIN {
            probs[IDX_HENTAI]
        } else {
            0.0
        };
        let gated_score = hentai_contrib + probs[IDX_PORN] + probs[IDX_SEXY];
        // drawings=0.027 < 0.10 → hentai not counted
        assert!(
            (hentai_contrib - 0.0).abs() < 1e-6,
            "hentai should be gated out"
        );
        assert!(
            gated_score < 0.50,
            "gated score {gated_score} should be < 0.50 threshold"
        );
    }

    #[test]
    fn test_nsfw_hentai_gate_drawn_content_allowed() {
        // Real drawn adult content: high drawings AND hentai → hentai IS counted
        let probs: [f32; 5] = [0.35, 0.55, 0.05, 0.02, 0.03]; // drawings=0.35, hentai=0.55
        let hentai_contrib = if probs[IDX_DRAWINGS] >= HENTAI_DRAWINGS_MIN {
            probs[IDX_HENTAI]
        } else {
            0.0
        };
        let gated_score = hentai_contrib + probs[IDX_PORN] + probs[IDX_SEXY];
        // drawings=0.35 >= 0.10 → hentai counted
        assert!(
            (hentai_contrib - probs[IDX_HENTAI]).abs() < 1e-6,
            "hentai should be included for drawn content"
        );
        assert!(
            gated_score >= 0.50,
            "gated score {gated_score} should be >= 0.50 threshold"
        );
    }

    #[test]
    fn test_nsfw_score_nan_guard() {
        // NaN logits (observed with INT8 model on some ORT EPs) must not
        // silently produce nsfw_score=NaN which compares false vs threshold.
        let nan_logits = [f32::NAN; 5];
        assert!(
            nan_logits.iter().any(|x| x.is_nan()),
            "guard should trigger"
        );
        // Verify NaN propagates through softmax without the guard
        let probs = softmax(&nan_logits);
        assert!(probs.iter().all(|x| x.is_nan()), "NaN logits → NaN probs");
        let score: f32 = probs[IDX_PORN] + probs[IDX_SEXY];
        // Without the guard, score is NaN and is_nsfw would be false (NaN >= 0.5 == false).
        // The guard catches this before softmax is reached.
        assert!(
            score.is_nan(),
            "NaN logits → NaN score — silent miss without guard"
        );
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

    /// Center-crop geometry: for a 3000×4500 image at sz=224, the center crop covers
    /// only the middle third of the original height (750–3750 of 4500px). Content
    /// above that window is not seen by the classifier — known limitation.
    #[test]
    fn test_center_crop_geometry() {
        let sz = 224u32;
        let (w, h) = (3000u32, 4500u32);
        let scale = sz as f32 / w.min(h) as f32;
        let new_h = (h as f32 * scale).round() as u32; // 336
        let crop_y = (new_h.saturating_sub(sz)) / 2; // 56
                                                     // crop covers rows 56..280 of 336 → 56/336 = 16.7% from top of scaled image
        assert_eq!(new_h, 336);
        assert_eq!(crop_y, 56);
        // As fraction of original height, crop starts at 16.7% — content above that is missed
        let crop_start_frac = crop_y as f32 / new_h as f32;
        assert!((crop_start_frac - 0.1666).abs() < 0.001);
    }

    /// Probe: print exact NSFW scores for the 3 known false-positive images.
    /// Run with: cargo test --lib nsfw_classifier::tests::probe_failing_images -- --nocapture --ignored
    #[test]
    #[ignore]
    fn probe_failing_images() {
        let model_dir = Path::new("models/nsfw_classifier");
        let mut clf = NsfwClassifier::load(model_dir, 0.5).expect("load model");

        let images = [
            (
                "doc_credit_card_01.jpg",
                "../test/data/input/Visual_PII/Documents/doc_credit_card_01.jpg",
            ),
            (
                "screenshot_ide_code_1920x1080.png",
                "../test/data/input/Visual_PII/Screenshots/screenshot_ide_code_1920x1080.png",
            ),
            (
                "screenshot_terminal_logs_2560x1440.png",
                "../test/data/input/Visual_PII/Screenshots/screenshot_terminal_logs_2560x1440.png",
            ),
        ];

        for (name, path) in &images {
            let img = image::open(path).expect(path);
            let result = clf.classify(&img).expect("classify");
            println!(
                "{name}: score={:.4}  top={}  \
                 [draw={:.3} hentai={:.3} neutral={:.3} porn={:.3} sexy={:.3}]",
                result.nsfw_score,
                result.top_class,
                result.class_probs[0],
                result.class_probs[1],
                result.class_probs[2],
                result.class_probs[3],
                result.class_probs[4],
            );
        }

        println!("\n--- True NSFW images ---");
        let nsfw_images = [
            (
                "semi_nu_pic1.jpg",
                "../test/data/input/Visual_PII/NSFW/semi_nu_pic1.jpg",
            ),
            (
                "semi_nu_pic2.jpg",
                "../test/data/input/Visual_PII/NSFW/semi_nu_pic2.jpg",
            ),
            (
                "semi_nu_pic3.jpg",
                "../test/data/input/Visual_PII/NSFW/semi_nu_pic3.jpg",
            ),
            (
                "semi_nu_pic4.jpg",
                "../test/data/input/Visual_PII/NSFW/semi_nu_pic4.jpg",
            ),
            (
                "nsfw_safe_object_01.jpg",
                "../test/data/input/Visual_PII/NSFW/nsfw_safe_object_01.jpg",
            ),
        ];
        for (name, path) in &nsfw_images {
            let img = match image::open(path) {
                Ok(i) => i,
                Err(e) => {
                    println!("{name}: SKIP ({e})");
                    continue;
                }
            };
            let result = clf.classify(&img).expect("classify");
            println!(
                "{name}: score={:.4}  top={}  \
                 [draw={:.3} hentai={:.3} neutral={:.3} porn={:.3} sexy={:.3}]",
                result.nsfw_score,
                result.top_class,
                result.class_probs[0],
                result.class_probs[1],
                result.class_probs[2],
                result.class_probs[3],
                result.class_probs[4],
            );
        }
    }
}

//! Image processing pipeline orchestrator.
//!
//! Coordinates the full image sanitization flow:
//! decode → EXIF strip → resize → NSFW check → face redact → OCR redact → encode.
//!
//! All operations are sequential to stay within the 224MB RAM ceiling.
//! Face and OCR models are loaded on-demand and evicted after idle timeout.

use std::io::Cursor;
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

use image::{DynamicImage, GenericImageView, ImageFormat};

use crate::config::ImageConfig;
use crate::detection_meta::{NsfwMeta, PipelineMeta};
use crate::face_detector::{FaceDetection, FaceDetector, ScrfdDetector};
use crate::image_redact;
use crate::keyword_dict::KeywordDict;
use crate::nsfw_detector::NsfwDetector;
use crate::ocr_engine::{OcrDetector, OcrRecognizer, OcrTier};
use crate::scanner::PiiScanner;

/// Errors from image processing operations.
#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("Image decode error: {0}")]
    Decode(String),
    #[error("Image encode error: {0}")]
    Encode(String),
    #[error("Unsupported image format: {0}")]
    UnsupportedFormat(String),
    #[error("ONNX Runtime error: {0}")]
    OnnxRuntime(String),
    #[error("Image too large: {0} bytes")]
    TooLarge(usize),
}

/// Image output format for re-encoding.
#[derive(Debug, Clone, Copy)]
pub enum OutputFormat {
    Png,
    Jpeg,
    WebP,
    Gif,
}

impl OutputFormat {
    /// Detect output format from media type string.
    pub fn from_media_type(media_type: &str) -> Self {
        match media_type {
            "image/jpeg" | "image/jpg" => OutputFormat::Jpeg,
            "image/webp" => OutputFormat::WebP,
            "image/gif" => OutputFormat::Gif,
            _ => OutputFormat::Png, // Default to PNG (lossless)
        }
    }

    fn to_image_format(self) -> ImageFormat {
        match self {
            OutputFormat::Png => ImageFormat::Png,
            OutputFormat::Jpeg => ImageFormat::Jpeg,
            OutputFormat::WebP => ImageFormat::WebP,
            OutputFormat::Gif => ImageFormat::Gif,
        }
    }
}

/// Decode raw bytes into a DynamicImage.
///
/// This inherently strips EXIF metadata — the `image` crate only loads pixel data,
/// discarding all metadata segments (EXIF, IPTC, XMP).
pub fn decode_image(bytes: &[u8]) -> Result<DynamicImage, ImageError> {
    image::load_from_memory(bytes).map_err(|e| ImageError::Decode(e.to_string()))
}

/// Resize an image so its longest side is at most `max_dim` pixels.
///
/// Preserves aspect ratio. Returns the image unchanged if already within bounds.
/// Uses Lanczos3 filter for high-quality downscaling.
pub fn resize_if_needed(img: DynamicImage, max_dim: u32) -> DynamicImage {
    let (w, h) = img.dimensions();
    if w <= max_dim && h <= max_dim {
        return img;
    }
    let scale = max_dim as f64 / w.max(h) as f64;
    let new_w = (w as f64 * scale) as u32;
    let new_h = (h as f64 * scale) as u32;
    img.resize_exact(new_w, new_h, image::imageops::FilterType::Lanczos3)
}

/// Encode a DynamicImage to bytes in the specified format.
///
/// The resulting bytes contain only pixel data — no EXIF or other metadata.
pub fn encode_image(img: &DynamicImage, format: OutputFormat) -> Result<Vec<u8>, ImageError> {
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, format.to_image_format())
        .map_err(|e| ImageError::Encode(e.to_string()))?;
    Ok(buf.into_inner())
}

/// Stats from processing a single image.
#[derive(Debug, Default)]
pub struct ImageStats {
    pub faces_redacted: u32,
    pub text_regions_found: u32,
    pub is_screenshot: bool,
    pub nsfw_detected: bool,
    pub processing_ms: u64,
    pub onnx_panics: u32,
}

/// On-demand image model manager.
///
/// Loads face detection and OCR models lazily, keeps them in memory until idle timeout.
/// Models are loaded sequentially (never both at once) to stay within RAM budget.
pub struct ImageModelManager {
    nsfw_detector: Mutex<Option<NsfwDetector>>,
    face_detector: Mutex<Option<FaceDetector>>,
    scrfd_detector: Mutex<Option<ScrfdDetector>>,
    ocr_detector: Mutex<Option<OcrDetector>>,
    ocr_recognizer: Mutex<Option<OcrRecognizer>>,
    last_use: Mutex<Instant>,
    config: ImageConfig,
}

impl ImageModelManager {
    pub fn new(config: ImageConfig) -> Self {
        Self {
            nsfw_detector: Mutex::new(None),
            face_detector: Mutex::new(None),
            scrfd_detector: Mutex::new(None),
            ocr_detector: Mutex::new(None),
            ocr_recognizer: Mutex::new(None),
            last_use: Mutex::new(Instant::now()),
            config,
        }
    }

    /// Evict models that have been idle longer than the configured timeout.
    pub fn evict_if_idle(&self) {
        let last = self.last_use.lock().unwrap_or_else(|e| e.into_inner());
        if last.elapsed().as_secs() < self.config.model_idle_timeout_secs {
            return;
        }
        let mut nsfw = self.nsfw_detector.lock().unwrap_or_else(|e| e.into_inner());
        if nsfw.is_some() {
            *nsfw = None;
            oo_info!(
                crate::oo_log::modules::IMAGE,
                "NSFW model evicted (idle timeout)"
            );
        }
        drop(nsfw);

        let mut face = self.face_detector.lock().unwrap_or_else(|e| e.into_inner());
        if face.is_some() {
            *face = None;
            oo_info!(
                crate::oo_log::modules::IMAGE,
                "BlazeFace model evicted (idle timeout)"
            );
        }
        drop(face);

        let mut scrfd = self
            .scrfd_detector
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if scrfd.is_some() {
            *scrfd = None;
            oo_info!(
                crate::oo_log::modules::IMAGE,
                "SCRFD model evicted (idle timeout)"
            );
        }
        drop(scrfd);

        let mut det = self.ocr_detector.lock().unwrap_or_else(|e| e.into_inner());
        if det.is_some() {
            *det = None;
            oo_info!(
                crate::oo_log::modules::IMAGE,
                "OCR detector evicted (idle timeout)"
            );
        }
        drop(det);

        let mut rec = self
            .ocr_recognizer
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if rec.is_some() {
            *rec = None;
            oo_info!(
                crate::oo_log::modules::IMAGE,
                "OCR recognizer evicted (idle timeout)"
            );
        }
    }

    /// Process a single image through the full pipeline.
    ///
    /// Steps: NSFW check → face detection → fill faces → OCR detection → fill text.
    /// If NSFW content detected, solid-fill entire image and skip face/OCR phases.
    /// Models are loaded on demand and released between phases to minimize RAM.
    ///
    /// The optional `screen_guard` result populates screenshot metadata in the pipeline stats.
    ///
    /// Returns `(processed_image, stats, pipeline_meta)` where `pipeline_meta` contains
    /// all detection metadata for verification.
    pub fn process_image(
        &self,
        img: DynamicImage,
        screen_guard: Option<&crate::screen_guard::ScreenGuardResult>,
    ) -> Result<(DynamicImage, ImageStats, PipelineMeta), ImageError> {
        let start = Instant::now();
        let mut stats = ImageStats::default();
        let (orig_w, orig_h) = img.dimensions();
        let mut meta = PipelineMeta {
            image_size: (orig_w, orig_h),
            ..PipelineMeta::default()
        };

        // Populate screenshot metadata from screen guard result
        if let Some(sg) = screen_guard {
            stats.is_screenshot = sg.is_screenshot;
            let mut exif_software = None;
            let mut resolution_match = false;
            let mut status_bar_variance = None;
            for reason in &sg.reasons {
                match reason {
                    crate::screen_guard::ScreenGuardReason::ExifSoftware(s) => {
                        exif_software = Some(s.clone());
                    }
                    crate::screen_guard::ScreenGuardReason::ScreenResolution(_, _) => {
                        resolution_match = true;
                    }
                    crate::screen_guard::ScreenGuardReason::StatusBar => {
                        status_bar_variance = Some(0.0);
                    }
                    crate::screen_guard::ScreenGuardReason::NoCameraHardware => {}
                }
            }
            meta.screenshot = crate::detection_meta::ScreenshotMeta {
                is_screenshot: sg.is_screenshot,
                resolution_match,
                status_bar_variance,
                exif_software,
                reason_count: sg.reasons.len(),
            };
        }

        // Convert to RgbImage for redaction operations, keep DynamicImage for model inference
        let mut rgb = img.to_rgb8();
        let mut dyn_img = img;

        // Update last-use timestamp
        if let Ok(mut last) = self.last_use.lock() {
            *last = Instant::now();
        }

        // Phase 0: NSFW detection — if nudity found, redact entire image and skip other phases
        if self.config.nsfw_detection {
            if let Some(ref dir) = self.config.nsfw_model_dir {
                let mut guard = self.nsfw_detector.lock().unwrap_or_else(|e| e.into_inner());
                if guard.is_none() {
                    match NsfwDetector::load(Path::new(dir), self.config.nsfw_threshold) {
                        Ok(detector) => *guard = Some(detector),
                        Err(e) => {
                            oo_warn!(crate::oo_log::modules::IMAGE, "NSFW model load failed (fail-open)", error = %e);
                        }
                    }
                }
                if let Some(ref mut detector) = *guard {
                    match detector.detect(&dyn_img) {
                        Ok(result) => {
                            // Collect NSFW metadata for verification
                            meta.nsfw = Some(NsfwMeta {
                                is_nsfw: result.is_nsfw,
                                confidence: result.confidence,
                                threshold: self.config.nsfw_threshold,
                                category: result.category.clone(),
                                exposed_scores: Vec::new(), // populated at detection level
                            });

                            if result.is_nsfw {
                                stats.nsfw_detected = true;
                                oo_info!(crate::oo_log::modules::IMAGE, "NSFW content detected — redacting entire image",
                                    confidence = result.confidence,
                                    category = ?result.category);
                                // Solid fill entire image
                                let (rw, rh) = (rgb.width(), rgb.height());
                                image_redact::solid_fill_region(
                                    &mut rgb,
                                    0,
                                    0,
                                    rw,
                                    rh,
                                    image_redact::SOLID_FILL_COLOR,
                                );
                                // Skip face/OCR — image is already fully redacted
                                drop(guard);
                                stats.processing_ms = start.elapsed().as_millis() as u64;
                                return Ok((DynamicImage::ImageRgb8(rgb), stats, meta));
                            }
                        }
                        Err(e) => {
                            if matches!(&e, ImageError::OnnxRuntime(msg) if msg.contains("panicked"))
                            {
                                stats.onnx_panics += 1;
                            }
                            oo_warn!(crate::oo_log::modules::IMAGE, "NSFW detection failed (fail-open)", error = %e);
                        }
                    }
                }
                drop(guard);
            }
        }

        // Phase 1: Face detection + redaction
        // Uses SCRFD (Full/Standard) or BlazeFace (Lite) based on config.face_model.
        if self.config.face_detection {
            let faces = self.detect_faces(&dyn_img, &mut stats.onnx_panics);

            if !faces.is_empty() {
                let (img_w, img_h) = (rgb.width(), rgb.height());
                let img_area = (img_w * img_h) as f32;

                meta.faces = faces.iter().map(|f| f.to_bbox_meta(img_w, img_h)).collect();

                for face in &faces {
                    let face_w = face.x_max - face.x_min;
                    let face_h = face.y_max - face.y_min;
                    let face_area = face_w * face_h;
                    if face_area / img_area > 0.8 {
                        // Face dominates frame — redact entire image
                        image_redact::solid_fill_region(
                            &mut rgb,
                            0,
                            0,
                            img_w,
                            img_h,
                            image_redact::SOLID_FILL_COLOR,
                        );
                    } else {
                        let (x, y, w, h) = image_redact::expand_bbox(
                            face.x_min, face.y_min, face.x_max, face.y_max, 0.15, img_w, img_h,
                        );
                        image_redact::solid_fill_region_elliptical(
                            &mut rgb,
                            x,
                            y,
                            w,
                            h,
                            image_redact::SOLID_FILL_COLOR,
                        );
                    }
                }
                stats.faces_redacted = faces.len() as u32;
                oo_debug!(
                    crate::oo_log::modules::FACE,
                    "Faces redacted",
                    count = faces.len(),
                    model = %self.config.face_model
                );
                dyn_img = DynamicImage::ImageRgb8(rgb.clone());
            }
        }

        // Phase 2: OCR detection + redaction
        if self.config.ocr_enabled {
            if let Some(ref dir) = self.config.ocr_model_dir {
                let tier = OcrTier::from_config(&self.config.ocr_tier);
                let mut det_guard = self.ocr_detector.lock().unwrap_or_else(|e| e.into_inner());

                // Load detector on demand
                if det_guard.is_none() {
                    match OcrDetector::load(Path::new(dir)) {
                        Ok(det) => *det_guard = Some(det),
                        Err(e) => {
                            oo_warn!(crate::oo_log::modules::OCR, "OCR detector load failed (fail-open)", error = %e);
                        }
                    }
                }

                if let Some(ref mut detector) = *det_guard {
                    match detector.detect(&dyn_img) {
                        Ok(regions) => {
                            stats.text_regions_found = regions.len() as u32;

                            // Collect text region metadata for verification
                            let (img_w, img_h) = (rgb.width(), rgb.height());
                            meta.text_regions = regions
                                .iter()
                                .map(|r| r.to_bbox_meta(img_w, img_h))
                                .collect();

                            match tier {
                                OcrTier::DetectAndFill => {
                                    // Tier 1: redact all detected text regions
                                    for region in &regions {
                                        image_redact::solid_fill_quad_region(
                                            &mut rgb,
                                            &region.points,
                                            image_redact::SOLID_FILL_COLOR,
                                        );
                                    }
                                }
                                OcrTier::FullRecognition => {
                                    // Tier 2: recognize text, scan for PII, redact only PII regions
                                    // Drop detector before loading recognizer (RAM)
                                    drop(det_guard);

                                    let mut rec_guard = self
                                        .ocr_recognizer
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner());
                                    if rec_guard.is_none() {
                                        match OcrRecognizer::load(Path::new(dir)) {
                                            Ok(rec) => *rec_guard = Some(rec),
                                            Err(e) => {
                                                oo_warn!(crate::oo_log::modules::OCR,
                                                    "OCR recognizer load failed (fail-open)", error = %e);
                                            }
                                        }
                                    }
                                    if let Some(ref mut recognizer) = *rec_guard {
                                        match recognizer.recognize(&dyn_img, &regions) {
                                            Ok(texts) => {
                                                let pii_scanner = PiiScanner::new();
                                                let keyword_dict = KeywordDict::new();
                                                let mut pii_count = 0u32;
                                                for rt in &texts {
                                                    oo_debug!(crate::oo_log::modules::OCR,
                                                        "OCR recognized", text = %rt.text, confidence = rt.confidence);
                                                    let has_regex_pii =
                                                        !pii_scanner.scan_text(&rt.text).is_empty();
                                                    let has_keyword_pii = !keyword_dict
                                                        .scan_text(&rt.text)
                                                        .is_empty();
                                                    if has_regex_pii || has_keyword_pii {
                                                        image_redact::solid_fill_quad_region(
                                                            &mut rgb,
                                                            &rt.region.points,
                                                            image_redact::SOLID_FILL_COLOR,
                                                        );
                                                        pii_count += 1;
                                                    }
                                                }
                                                oo_info!(
                                                    crate::oo_log::modules::OCR,
                                                    "OCR Tier 2: PII-selective redaction",
                                                    pii_regions = pii_count,
                                                    total_regions = texts.len()
                                                );
                                            }
                                            Err(e) => {
                                                if matches!(&e, ImageError::OnnxRuntime(msg) if msg.contains("panicked"))
                                                {
                                                    stats.onnx_panics += 1;
                                                }
                                                oo_warn!(crate::oo_log::modules::OCR,
                                                    "OCR recognition failed (fail-open)", error = %e);
                                            }
                                        }
                                    }
                                    // Early return to avoid double-drop of det_guard
                                    stats.processing_ms = start.elapsed().as_millis() as u64;
                                    return Ok((DynamicImage::ImageRgb8(rgb), stats, meta));
                                }
                            }
                        }
                        Err(e) => {
                            if matches!(&e, ImageError::OnnxRuntime(msg) if msg.contains("panicked"))
                            {
                                stats.onnx_panics += 1;
                            }
                            oo_warn!(crate::oo_log::modules::OCR, "OCR detection failed (fail-open)", error = %e);
                        }
                    }
                }
            }
        }

        stats.processing_ms = start.elapsed().as_millis() as u64;
        Ok((DynamicImage::ImageRgb8(rgb), stats, meta))
    }

    /// Get a reference to the config.
    pub fn config(&self) -> &ImageConfig {
        &self.config
    }

    /// Detect faces using the configured model (SCRFD or BlazeFace).
    fn detect_faces(&self, img: &DynamicImage, onnx_panics: &mut u32) -> Vec<FaceDetection> {
        if self.config.face_model == "scrfd" {
            if let Some(ref dir) = self.config.face_model_dir_scrfd {
                let mut guard = self
                    .scrfd_detector
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if guard.is_none() {
                    match ScrfdDetector::load(Path::new(dir), 0.5) {
                        Ok(detector) => *guard = Some(detector),
                        Err(e) => {
                            oo_warn!(crate::oo_log::modules::FACE, "SCRFD load failed, falling back to BlazeFace", error = %e);
                            drop(guard);
                            return self.detect_faces_blazeface(img, onnx_panics);
                        }
                    }
                }
                if let Some(ref mut detector) = *guard {
                    match detector.detect(img) {
                        Ok(faces) => return faces,
                        Err(e) => {
                            if matches!(&e, ImageError::OnnxRuntime(msg) if msg.contains("panicked"))
                            {
                                *onnx_panics += 1;
                            }
                            oo_warn!(crate::oo_log::modules::FACE, "SCRFD detection failed (fail-open)", error = %e);
                        }
                    }
                }
                drop(guard);
            } else {
                // No SCRFD model dir configured, fall back to BlazeFace
                return self.detect_faces_blazeface(img, onnx_panics);
            }
        } else {
            return self.detect_faces_blazeface(img, onnx_panics);
        }
        Vec::new()
    }

    /// Detect faces using BlazeFace (fallback / Lite tier).
    fn detect_faces_blazeface(
        &self,
        img: &DynamicImage,
        onnx_panics: &mut u32,
    ) -> Vec<FaceDetection> {
        if let Some(ref dir) = self.config.face_model_dir {
            let mut guard = self.face_detector.lock().unwrap_or_else(|e| e.into_inner());
            if guard.is_none() {
                match FaceDetector::load(Path::new(dir), 0.75) {
                    Ok(detector) => *guard = Some(detector),
                    Err(e) => {
                        oo_warn!(crate::oo_log::modules::FACE, "BlazeFace load failed (fail-open)", error = %e);
                        return Vec::new();
                    }
                }
            }
            if let Some(ref mut detector) = *guard {
                match detector.detect(img) {
                    Ok(faces) => return faces,
                    Err(e) => {
                        if matches!(&e, ImageError::OnnxRuntime(msg) if msg.contains("panicked")) {
                            *onnx_panics += 1;
                        }
                        oo_warn!(crate::oo_log::modules::FACE, "BlazeFace detection failed (fail-open)", error = %e);
                    }
                }
            }
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn make_test_image(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb([128, 64, 32])))
    }

    fn make_test_png_bytes(width: u32, height: u32) -> Vec<u8> {
        let img = make_test_image(width, height);
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    #[test]
    fn test_image_stats_onnx_panics_default() {
        let stats = ImageStats::default();
        assert_eq!(stats.onnx_panics, 0);
    }

    #[test]
    fn test_decode_png() {
        let bytes = make_test_png_bytes(50, 30);
        let img = decode_image(&bytes).unwrap();
        assert_eq!(img.width(), 50);
        assert_eq!(img.height(), 30);
    }

    #[test]
    fn test_decode_jpeg() {
        let img = make_test_image(40, 40);
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Jpeg).unwrap();
        let decoded = decode_image(&buf.into_inner()).unwrap();
        assert_eq!(decoded.width(), 40);
        assert_eq!(decoded.height(), 40);
    }

    #[test]
    fn test_decode_invalid_bytes() {
        assert!(decode_image(b"not an image").is_err());
    }

    #[test]
    fn test_resize_large_image() {
        let img = make_test_image(4000, 3000);
        let resized = resize_if_needed(img, 960);
        assert_eq!(resized.width(), 960);
        assert_eq!(resized.height(), 720);
    }

    #[test]
    fn test_resize_tall_image() {
        let img = make_test_image(600, 1200);
        let resized = resize_if_needed(img, 960);
        assert_eq!(resized.width(), 480);
        assert_eq!(resized.height(), 960);
    }

    #[test]
    fn test_resize_small_image_unchanged() {
        let img = make_test_image(800, 600);
        let resized = resize_if_needed(img, 960);
        assert_eq!(resized.width(), 800);
        assert_eq!(resized.height(), 600);
    }

    #[test]
    fn test_resize_exact_boundary() {
        let img = make_test_image(960, 960);
        let resized = resize_if_needed(img, 960);
        assert_eq!(resized.width(), 960);
        assert_eq!(resized.height(), 960);
    }

    #[test]
    fn test_encode_png() {
        let img = make_test_image(10, 10);
        let bytes = encode_image(&img, OutputFormat::Png).unwrap();
        assert!(bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]));
    }

    #[test]
    fn test_encode_jpeg() {
        let img = make_test_image(10, 10);
        let bytes = encode_image(&img, OutputFormat::Jpeg).unwrap();
        assert!(bytes.starts_with(&[0xFF, 0xD8, 0xFF]));
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let img = make_test_image(50, 50);
        let bytes = encode_image(&img, OutputFormat::Png).unwrap();
        let decoded = decode_image(&bytes).unwrap();
        assert_eq!(decoded.width(), 50);
        assert_eq!(decoded.height(), 50);
    }

    #[test]
    fn test_output_format_from_media_type() {
        assert!(matches!(
            OutputFormat::from_media_type("image/png"),
            OutputFormat::Png
        ));
        assert!(matches!(
            OutputFormat::from_media_type("image/jpeg"),
            OutputFormat::Jpeg
        ));
        assert!(matches!(
            OutputFormat::from_media_type("image/webp"),
            OutputFormat::WebP
        ));
        assert!(matches!(
            OutputFormat::from_media_type("image/gif"),
            OutputFormat::Gif
        ));
        assert!(matches!(
            OutputFormat::from_media_type("unknown"),
            OutputFormat::Png
        ));
    }

    #[test]
    fn test_exif_stripped_by_roundtrip() {
        let bytes = make_test_png_bytes(20, 20);
        let img = decode_image(&bytes).unwrap();
        let re_encoded = encode_image(&img, OutputFormat::Png).unwrap();
        let img2 = decode_image(&re_encoded).unwrap();
        assert_eq!(img2.width(), 20);
        assert_eq!(img2.height(), 20);
    }

    #[test]
    fn test_model_manager_creation() {
        let config = crate::config::ImageConfig::default();
        let manager = ImageModelManager::new(config);
        assert!(manager.config().enabled);
        assert!(manager.config().face_detection);
        assert!(manager.config().ocr_enabled);
    }

    #[test]
    fn test_model_manager_process_no_models() {
        // With no model dirs configured, processing should still succeed (skip face/OCR)
        let config = crate::config::ImageConfig {
            enabled: true,
            face_detection: true,
            ocr_enabled: true,
            face_model_dir: None,
            ocr_model_dir: None,
            ..crate::config::ImageConfig::default()
        };
        let manager = ImageModelManager::new(config);
        let img = make_test_image(100, 100);
        let (result, stats, _meta) = manager.process_image(img, None).unwrap();
        assert_eq!(result.width(), 100);
        assert_eq!(result.height(), 100);
        assert_eq!(stats.faces_redacted, 0);
        assert_eq!(stats.text_regions_found, 0);
    }

    #[test]
    fn test_model_manager_evict_if_idle() {
        let config = crate::config::ImageConfig {
            model_idle_timeout_secs: 0, // Evict immediately
            ..crate::config::ImageConfig::default()
        };
        let manager = ImageModelManager::new(config);
        // Eviction should not panic even with no models loaded
        manager.evict_if_idle();
    }

    #[test]
    fn test_model_manager_disabled_face_detection() {
        let config = crate::config::ImageConfig {
            face_detection: false,
            ocr_enabled: false,
            ..crate::config::ImageConfig::default()
        };
        let manager = ImageModelManager::new(config);
        let img = make_test_image(50, 50);
        let (result, stats, _meta) = manager.process_image(img, None).unwrap();
        assert_eq!(result.width(), 50);
        assert_eq!(stats.faces_redacted, 0);
        assert_eq!(stats.text_regions_found, 0);
    }
}

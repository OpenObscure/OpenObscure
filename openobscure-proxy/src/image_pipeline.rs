//! Image processing pipeline orchestrator.
//!
//! Coordinates the full image sanitization flow:
//! decode → EXIF strip → resize → face blur → OCR → encode.
//!
//! All operations are sequential to stay within the 224MB RAM ceiling.
//! Face and OCR models are loaded on-demand and evicted after idle timeout.

use std::io::Cursor;
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

use image::{DynamicImage, ImageFormat, GenericImageView};

use crate::config::ImageConfig;
use crate::face_detector::FaceDetector;
use crate::image_blur;
use crate::ocr_engine::{OcrDetector, OcrRecognizer, OcrTier};

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
    pub faces_blurred: u32,
    pub text_regions_found: u32,
    pub is_screenshot: bool,
    pub processing_ms: u64,
}

/// On-demand image model manager.
///
/// Loads face detection and OCR models lazily, keeps them in memory until idle timeout.
/// Models are loaded sequentially (never both at once) to stay within RAM budget.
pub struct ImageModelManager {
    face_detector: Mutex<Option<FaceDetector>>,
    ocr_detector: Mutex<Option<OcrDetector>>,
    ocr_recognizer: Mutex<Option<OcrRecognizer>>,
    last_use: Mutex<Instant>,
    config: ImageConfig,
}

impl ImageModelManager {
    pub fn new(config: ImageConfig) -> Self {
        Self {
            face_detector: Mutex::new(None),
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
        let mut face = self.face_detector.lock().unwrap_or_else(|e| e.into_inner());
        if face.is_some() {
            *face = None;
            cg_info!(crate::cg_log::modules::IMAGE, "Face model evicted (idle timeout)");
        }
        drop(face);

        let mut det = self.ocr_detector.lock().unwrap_or_else(|e| e.into_inner());
        if det.is_some() {
            *det = None;
            cg_info!(crate::cg_log::modules::IMAGE, "OCR detector evicted (idle timeout)");
        }
        drop(det);

        let mut rec = self.ocr_recognizer.lock().unwrap_or_else(|e| e.into_inner());
        if rec.is_some() {
            *rec = None;
            cg_info!(crate::cg_log::modules::IMAGE, "OCR recognizer evicted (idle timeout)");
        }
    }

    /// Process a single image through the full pipeline.
    ///
    /// Steps: face detection → blur faces → OCR detection → blur/recognize text → return modified image.
    /// Models are loaded on demand and released between phases to minimize RAM.
    pub fn process_image(&self, img: DynamicImage) -> Result<(DynamicImage, ImageStats), ImageError> {
        let start = Instant::now();
        let mut stats = ImageStats::default();
        // Convert to RgbImage for blur operations, keep DynamicImage for model inference
        let mut rgb = img.to_rgb8();
        let mut dyn_img = img;

        // Update last-use timestamp
        if let Ok(mut last) = self.last_use.lock() {
            *last = Instant::now();
        }

        // Phase 1: Face detection + blur
        if self.config.face_detection {
            if let Some(ref dir) = self.config.face_model_dir {
                let mut guard = self.face_detector.lock().unwrap_or_else(|e| e.into_inner());
                // Load on demand
                if guard.is_none() {
                    match FaceDetector::load(Path::new(dir), 0.75) {
                        Ok(detector) => *guard = Some(detector),
                        Err(e) => {
                            cg_warn!(crate::cg_log::modules::FACE, "Face model load failed (fail-open)", error = %e);
                        }
                    }
                }
                if let Some(ref mut detector) = *guard {
                    match detector.detect(&dyn_img) {
                        Ok(faces) => {
                            for face in &faces {
                                let x = face.x_min as u32;
                                let y = face.y_min as u32;
                                let w = (face.x_max - face.x_min) as u32;
                                let h = (face.y_max - face.y_min) as u32;
                                image_blur::blur_region(&mut rgb, x, y, w, h, self.config.face_blur_sigma);
                            }
                            stats.faces_blurred = faces.len() as u32;
                            if !faces.is_empty() {
                                cg_debug!(crate::cg_log::modules::FACE, "Faces blurred", count = faces.len());
                                // Update DynamicImage from blurred RGB for OCR phase
                                dyn_img = DynamicImage::ImageRgb8(rgb.clone());
                            }
                        }
                        Err(e) => {
                            cg_warn!(crate::cg_log::modules::FACE, "Face detection failed (fail-open)", error = %e);
                        }
                    }
                }
                // Drop face detector guard to free RAM before OCR
                drop(guard);
            }
        }

        // Phase 2: OCR detection + blur
        if self.config.ocr_enabled {
            if let Some(ref dir) = self.config.ocr_model_dir {
                let tier = OcrTier::from_config(&self.config.ocr_tier);
                let mut det_guard = self.ocr_detector.lock().unwrap_or_else(|e| e.into_inner());

                // Load detector on demand
                if det_guard.is_none() {
                    match OcrDetector::load(Path::new(dir)) {
                        Ok(det) => *det_guard = Some(det),
                        Err(e) => {
                            cg_warn!(crate::cg_log::modules::OCR, "OCR detector load failed (fail-open)", error = %e);
                        }
                    }
                }

                if let Some(ref mut detector) = *det_guard {
                    match detector.detect(&dyn_img) {
                        Ok(regions) => {
                            stats.text_regions_found = regions.len() as u32;

                            match tier {
                                OcrTier::DetectAndBlur => {
                                    // Tier 1: blur all detected text regions
                                    for region in &regions {
                                        image_blur::blur_quad_region(
                                            &mut rgb,
                                            &region.points,
                                            self.config.text_blur_sigma,
                                        );
                                    }
                                }
                                OcrTier::FullRecognition => {
                                    // Tier 2: recognize text, blur only PII regions
                                    // Drop detector before loading recognizer (RAM)
                                    drop(det_guard);

                                    let mut rec_guard = self.ocr_recognizer.lock()
                                        .unwrap_or_else(|e| e.into_inner());
                                    if rec_guard.is_none() {
                                        match OcrRecognizer::load(Path::new(dir)) {
                                            Ok(rec) => *rec_guard = Some(rec),
                                            Err(e) => {
                                                cg_warn!(crate::cg_log::modules::OCR,
                                                    "OCR recognizer load failed (fail-open)", error = %e);
                                            }
                                        }
                                    }
                                    if let Some(ref mut recognizer) = *rec_guard {
                                        match recognizer.recognize(&dyn_img, &regions) {
                                            Ok(texts) => {
                                                for rt in &texts {
                                                    image_blur::blur_quad_region(
                                                        &mut rgb,
                                                        &rt.region.points,
                                                        self.config.text_blur_sigma,
                                                    );
                                                }
                                            }
                                            Err(e) => {
                                                cg_warn!(crate::cg_log::modules::OCR,
                                                    "OCR recognition failed (fail-open)", error = %e);
                                            }
                                        }
                                    }
                                    // Early return to avoid double-drop of det_guard
                                    stats.processing_ms = start.elapsed().as_millis() as u64;
                                    return Ok((DynamicImage::ImageRgb8(rgb), stats));
                                }
                            }
                        }
                        Err(e) => {
                            cg_warn!(crate::cg_log::modules::OCR, "OCR detection failed (fail-open)", error = %e);
                        }
                    }
                }
            }
        }

        stats.processing_ms = start.elapsed().as_millis() as u64;
        Ok((DynamicImage::ImageRgb8(rgb), stats))
    }

    /// Get a reference to the config.
    pub fn config(&self) -> &ImageConfig {
        &self.config
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
        assert!(matches!(OutputFormat::from_media_type("image/png"), OutputFormat::Png));
        assert!(matches!(OutputFormat::from_media_type("image/jpeg"), OutputFormat::Jpeg));
        assert!(matches!(OutputFormat::from_media_type("image/webp"), OutputFormat::WebP));
        assert!(matches!(OutputFormat::from_media_type("image/gif"), OutputFormat::Gif));
        assert!(matches!(OutputFormat::from_media_type("unknown"), OutputFormat::Png));
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
        let (result, stats) = manager.process_image(img).unwrap();
        assert_eq!(result.width(), 100);
        assert_eq!(result.height(), 100);
        assert_eq!(stats.faces_blurred, 0);
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
        let (result, stats) = manager.process_image(img).unwrap();
        assert_eq!(result.width(), 50);
        assert_eq!(stats.faces_blurred, 0);
        assert_eq!(stats.text_regions_found, 0);
    }
}

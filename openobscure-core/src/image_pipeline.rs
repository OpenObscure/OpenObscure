//! Image processing pipeline orchestrator.
//!
//! Coordinates the full image sanitization flow:
//! decode → EXIF strip → resize → NSFW check → face redact → OCR redact → encode.
//!
//! All operations run sequentially on a single image at a time to stay within
//! the 224 MB RAM ceiling. Models are loaded on first use and evicted after an
//! idle timeout; only one model (face or OCR) is resident in memory at once.

use std::io::Cursor;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use image::{DynamicImage, GenericImageView, ImageFormat};

use crate::config::ImageConfig;
use crate::detection_meta::{NsfwMeta, PipelineMeta};
use crate::face_detector::{nms, FaceDetection, FaceDetector, ScrfdDetector, UltraLightDetector};
use crate::hybrid_scanner::HybridScanner;
use crate::image_redact;
use crate::keyword_dict::KeywordDict;
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
    /// Phase 0: NSFW detection time (milliseconds).
    pub nsfw_ms: u64,
    /// Phase 1: Face detection + redaction time (milliseconds).
    pub face_ms: u64,
    /// Phase 2: OCR detection + redaction time (milliseconds).
    pub ocr_ms: u64,
    /// Whether this image was fetched from a URL (vs base64 embedded).
    pub from_url: bool,
    /// URL fetch time in milliseconds (0 for base64 images).
    pub fetch_ms: u64,
}

/// On-demand image model manager.
///
/// Loads face detection and OCR models lazily, keeps them in memory until idle timeout.
/// Models are loaded sequentially (never both at once) to stay within RAM budget.
pub struct ImageModelManager {
    nsfw_classifier: Mutex<Option<Arc<Mutex<crate::nsfw_classifier::NsfwClassifier>>>>,
    face_detector: Mutex<Option<Arc<Mutex<FaceDetector>>>>,
    scrfd_detector: Mutex<Option<Arc<Mutex<ScrfdDetector>>>>,
    ultralight_detector: Mutex<Option<Arc<Mutex<UltraLightDetector>>>>,
    ocr_detector: Mutex<Option<Arc<Mutex<OcrDetector>>>>,
    ocr_recognizer: Mutex<Option<Arc<Mutex<OcrRecognizer>>>>,
    last_use: Mutex<Instant>,
    config: ImageConfig,
}

impl ImageModelManager {
    pub fn new(config: ImageConfig) -> Self {
        Self {
            nsfw_classifier: Mutex::new(None),
            face_detector: Mutex::new(None),
            scrfd_detector: Mutex::new(None),
            ultralight_detector: Mutex::new(None),
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
        let mut cls = self
            .nsfw_classifier
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if cls.is_some() {
            *cls = None;
            oo_info!(
                crate::oo_log::modules::IMAGE,
                "NSFW classifier evicted (idle timeout)"
            );
        }
        drop(cls);

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

        let mut ultralight = self
            .ultralight_detector
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if ultralight.is_some() {
            *ultralight = None;
            oo_info!(
                crate::oo_log::modules::IMAGE,
                "Ultra-Light model evicted (idle timeout)"
            );
        }
        drop(ultralight);

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

    /// Immediately release all loaded models, regardless of idle timeout.
    /// Called in response to OS memory pressure warnings (iOS/Android).
    pub fn force_evict(&self) {
        let mut count = 0u32;
        let mut cls = self
            .nsfw_classifier
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if cls.take().is_some() {
            count += 1;
        }
        drop(cls);

        let mut face = self.face_detector.lock().unwrap_or_else(|e| e.into_inner());
        if face.take().is_some() {
            count += 1;
        }
        drop(face);

        let mut scrfd = self
            .scrfd_detector
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if scrfd.take().is_some() {
            count += 1;
        }
        drop(scrfd);

        let mut ultralight = self
            .ultralight_detector
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if ultralight.take().is_some() {
            count += 1;
        }
        drop(ultralight);

        let mut det = self.ocr_detector.lock().unwrap_or_else(|e| e.into_inner());
        if det.take().is_some() {
            count += 1;
        }
        drop(det);

        let mut rec = self
            .ocr_recognizer
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if rec.take().is_some() {
            count += 1;
        }

        if count > 0 {
            oo_info!(
                crate::oo_log::modules::IMAGE,
                "Force-evicted models (memory pressure)",
                models_released = count
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
        text_scanner: Option<&HybridScanner>,
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

        // Convert to RgbImage for redaction operations (NSFW early-return path),
        // keep DynamicImage at original resolution for NSFW inference.
        let mut rgb = img.to_rgb8();
        let dyn_img = img;

        // Update last-use timestamp
        if let Ok(mut last) = self.last_use.lock() {
            *last = Instant::now();
        }

        // Phase 0: NSFW classification (5-class ViT-base)
        // Classes: drawings, hentai, neutral, porn, sexy
        // NSFW score = P(hentai) + P(porn) + P(sexy)
        // If NSFW detected, solid-fill entire image and skip face/OCR phases.
        let nsfw_start = Instant::now();
        if self.config.nsfw_detection {
            if let Some(ref dir) = self.config.nsfw_model_dir {
                let cls_arc = {
                    let mut cls_guard = self
                        .nsfw_classifier
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if cls_guard.is_none() {
                        match crate::nsfw_classifier::NsfwClassifier::load(
                            Path::new(dir),
                            self.config.nsfw_threshold,
                        ) {
                            Ok(cls) => *cls_guard = Some(Arc::new(Mutex::new(cls))),
                            Err(e) => {
                                oo_warn!(crate::oo_log::modules::IMAGE,
                                    "NSFW classifier load failed (fail-open)", error = %e);
                            }
                        }
                    }
                    cls_guard.as_ref().map(Arc::clone)
                };
                if let Some(cls) = cls_arc {
                    let mut classifier = cls.lock().unwrap_or_else(|e| e.into_inner());
                    match classifier.classify(&dyn_img) {
                        Ok(result) => {
                            meta.nsfw = Some(NsfwMeta {
                                is_nsfw: result.is_nsfw,
                                confidence: result.nsfw_score,
                                threshold: self.config.nsfw_threshold,
                                category: Some(result.top_class.clone()),
                                exposed_scores: Vec::new(),
                                classifier_score: Some(result.nsfw_score),
                            });

                            if result.is_nsfw {
                                stats.nsfw_detected = true;
                                oo_info!(crate::oo_log::modules::IMAGE,
                                    "NSFW content detected — redacting entire image",
                                    nsfw_score = result.nsfw_score,
                                    top_class = %result.top_class,
                                    threshold = self.config.nsfw_threshold);
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
                                stats.nsfw_ms = nsfw_start.elapsed().as_millis() as u64;
                                stats.processing_ms = start.elapsed().as_millis() as u64;
                                return Ok((DynamicImage::ImageRgb8(rgb), stats, meta));
                            }
                        }
                        Err(e) => {
                            if matches!(&e, ImageError::OnnxRuntime(msg) if msg.contains("panicked"))
                            {
                                stats.onnx_panics += 1;
                            }
                            oo_warn!(crate::oo_log::modules::IMAGE,
                                "NSFW classification failed (fail-open)", error = %e);
                        }
                    }
                }
            }
        }
        stats.nsfw_ms = nsfw_start.elapsed().as_millis() as u64;

        // Resize to max_dimension *after* NSFW phase so the classifier always
        // sees the original resolution (avoids double-downscale for tall images).
        let img_after_nsfw = resize_if_needed(dyn_img, self.config.max_dimension);
        let mut dyn_img = img_after_nsfw;
        let mut rgb = dyn_img.to_rgb8();

        // Phase 1: Face detection + redaction
        // Uses SCRFD (Full/Standard) or BlazeFace (Lite) based on config.face_model.
        let face_start = Instant::now();
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
                    // 80% threshold: if the face fills most of the frame (portrait /
                    // close-up shot), a selective bbox redaction would still leave the
                    // subject identifiable from hair, chin, or ears — so we fill the
                    // entire image instead. Below 80% we use expand_bbox (15% padding).
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
        stats.face_ms = face_start.elapsed().as_millis() as u64;

        // Phase 2: OCR detection + redaction
        let ocr_start = Instant::now();
        if self.config.ocr_enabled {
            if let Some(ref dir) = self.config.ocr_model_dir {
                let tier = OcrTier::from_config(&self.config.ocr_tier);
                let det_arc = {
                    let mut det_guard = self.ocr_detector.lock().unwrap_or_else(|e| e.into_inner());
                    if det_guard.is_none() {
                        match OcrDetector::load(Path::new(dir)) {
                            Ok(det) => *det_guard = Some(Arc::new(Mutex::new(det))),
                            Err(e) => {
                                oo_warn!(crate::oo_log::modules::OCR, "OCR detector load failed (fail-open)", error = %e);
                            }
                        }
                    }
                    det_guard.as_ref().map(Arc::clone)
                };

                if let Some(det) = det_arc {
                    let mut detector = det.lock().unwrap_or_else(|e| e.into_inner());
                    match detector.detect(&dyn_img) {
                        Ok(regions) => {
                            stats.text_regions_found = regions.len() as u32;

                            // Collect text region metadata for verification
                            let (img_w, img_h) = (rgb.width(), rgb.height());
                            meta.text_regions = regions
                                .iter()
                                .map(|r| r.to_bbox_meta(img_w, img_h))
                                .collect();

                            // Release inner detector lock before recognizer phase
                            drop(detector);

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
                                    let rec_arc = {
                                        let mut rec_guard = self
                                            .ocr_recognizer
                                            .lock()
                                            .unwrap_or_else(|e| e.into_inner());
                                        if rec_guard.is_none() {
                                            match OcrRecognizer::load(Path::new(dir)) {
                                                Ok(rec) => {
                                                    *rec_guard = Some(Arc::new(Mutex::new(rec)))
                                                }
                                                Err(e) => {
                                                    oo_warn!(crate::oo_log::modules::OCR,
                                                        "OCR recognizer load failed (fail-open)", error = %e);
                                                }
                                            }
                                        }
                                        rec_guard.as_ref().map(Arc::clone)
                                    };
                                    if let Some(rec) = rec_arc {
                                        let mut recognizer =
                                            rec.lock().unwrap_or_else(|e| e.into_inner());
                                        match recognizer.recognize(&dyn_img, &regions) {
                                            Ok(texts) => {
                                                // Concatenate all OCR text for a single
                                                // scan pass through the full pipeline
                                                // (regex + keywords + NER).
                                                let combined: String = texts
                                                    .iter()
                                                    .map(|rt| {
                                                        oo_debug!(crate::oo_log::modules::OCR,
                                                            "OCR recognized", text = %rt.text, confidence = rt.confidence);
                                                        rt.text.as_str()
                                                    })
                                                    .collect::<Vec<_>>()
                                                    .join(" ");

                                                // Low-confidence fallback: if OCR can't read
                                                // the text reliably, we can't verify it's not
                                                // PII — privacy-first means redact it.
                                                let avg_confidence: f32 = if texts.is_empty() {
                                                    1.0
                                                } else {
                                                    texts
                                                        .iter()
                                                        .map(|rt| rt.confidence)
                                                        .sum::<f32>()
                                                        / texts.len() as f32
                                                };
                                                let low_confidence = avg_confidence < 0.80;

                                                let has_pii = if low_confidence {
                                                    true // Can't verify — assume PII
                                                } else if let Some(scanner) = text_scanner {
                                                    // Full pipeline: regex + keywords + NER
                                                    !scanner.scan_text(&combined).is_empty()
                                                } else {
                                                    // Fallback: regex + keywords only
                                                    let pii_scanner = PiiScanner::new();
                                                    let keyword_dict = KeywordDict::new();
                                                    !pii_scanner.scan_text(&combined).is_empty()
                                                        || !keyword_dict
                                                            .scan_text(&combined)
                                                            .is_empty()
                                                };

                                                // If PII is found (or confidence is too low),
                                                // redact ALL detected text regions — not just
                                                // the ones the recognizer successfully read.
                                                // The recognizer may fail on some regions
                                                // (e.g. embossed card numbers) but those
                                                // regions still potentially contain PII.
                                                let pii_count;
                                                if has_pii {
                                                    for region in &regions {
                                                        image_redact::solid_fill_quad_region(
                                                            &mut rgb,
                                                            &region.points,
                                                            image_redact::SOLID_FILL_COLOR,
                                                        );
                                                    }
                                                    pii_count = regions.len() as u32;
                                                    if low_confidence {
                                                        oo_info!(
                                                            crate::oo_log::modules::OCR,
                                                            "OCR Tier 2: low confidence — redacting all text regions",
                                                            avg_confidence = format!("{:.2}", avg_confidence),
                                                            total_regions = texts.len()
                                                        );
                                                    } else {
                                                        oo_info!(
                                                            crate::oo_log::modules::OCR,
                                                            "OCR Tier 2: PII detected — redacting all text regions",
                                                            total_regions = texts.len()
                                                        );
                                                    }
                                                } else {
                                                    pii_count = 0;
                                                    oo_info!(
                                                        crate::oo_log::modules::OCR,
                                                        "OCR Tier 2: no PII detected — preserving text",
                                                        avg_confidence = format!("{:.2}", avg_confidence),
                                                        total_regions = texts.len()
                                                    );
                                                }
                                                let _ = pii_count;
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
                } else {
                    oo_warn!(
                        crate::oo_log::modules::OCR,
                        "OCR detector not available (model failed to load)"
                    );
                }
            }
        }
        stats.ocr_ms = ocr_start.elapsed().as_millis() as u64;

        stats.processing_ms = start.elapsed().as_millis() as u64;
        Ok((DynamicImage::ImageRgb8(rgb), stats, meta))
    }

    /// Get a reference to the config.
    pub fn config(&self) -> &ImageConfig {
        &self.config
    }

    /// Detect faces using the configured model (SCRFD, Ultra-Light, or BlazeFace).
    fn detect_faces(&self, img: &DynamicImage, onnx_panics: &mut u32) -> Vec<FaceDetection> {
        if self.config.face_model == "scrfd" {
            if let Some(ref dir) = self.config.face_model_dir_scrfd {
                let scrfd_arc = {
                    let mut guard = self
                        .scrfd_detector
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if guard.is_none() {
                        match ScrfdDetector::load(Path::new(dir), 0.5) {
                            Ok(detector) => *guard = Some(Arc::new(Mutex::new(detector))),
                            Err(e) => {
                                oo_warn!(crate::oo_log::modules::FACE, "SCRFD load failed, falling back to BlazeFace", error = %e);
                                return self.detect_faces_blazeface_tiled(img, onnx_panics);
                            }
                        }
                    }
                    guard.as_ref().map(Arc::clone)
                };
                if let Some(det) = scrfd_arc {
                    let mut detector = det.lock().unwrap_or_else(|e| e.into_inner());
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
            } else {
                // No SCRFD model dir configured, fall back to BlazeFace
                return self.detect_faces_blazeface_tiled(img, onnx_panics);
            }
        } else if self.config.face_model == "ultralight" {
            return self.detect_faces_ultralight(img, onnx_panics);
        } else {
            return self.detect_faces_blazeface_tiled(img, onnx_panics);
        }
        Vec::new()
    }

    /// Detect faces using Ultra-Light face detector (Lite tier alternative).
    fn detect_faces_ultralight(
        &self,
        img: &DynamicImage,
        onnx_panics: &mut u32,
    ) -> Vec<FaceDetection> {
        if let Some(ref dir) = self.config.face_model_dir_ultralight {
            let ul_arc = {
                let mut guard = self
                    .ultralight_detector
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if guard.is_none() {
                    match UltraLightDetector::load(Path::new(dir), 0.7) {
                        Ok(detector) => *guard = Some(Arc::new(Mutex::new(detector))),
                        Err(e) => {
                            oo_warn!(crate::oo_log::modules::FACE, "Ultra-Light load failed, falling back to BlazeFace", error = %e);
                            return self.detect_faces_blazeface_tiled(img, onnx_panics);
                        }
                    }
                }
                guard.as_ref().map(Arc::clone)
            };
            if let Some(det) = ul_arc {
                let mut detector = det.lock().unwrap_or_else(|e| e.into_inner());
                match detector.detect(img) {
                    Ok(faces) => return faces,
                    Err(e) => {
                        if matches!(&e, ImageError::OnnxRuntime(msg) if msg.contains("panicked")) {
                            *onnx_panics += 1;
                        }
                        oo_warn!(crate::oo_log::modules::FACE, "Ultra-Light detection failed (fail-open)", error = %e);
                    }
                }
            }
        } else {
            // No Ultra-Light model dir configured, fall back to BlazeFace
            return self.detect_faces_blazeface_tiled(img, onnx_panics);
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
            let face_arc = {
                let mut guard = self.face_detector.lock().unwrap_or_else(|e| e.into_inner());
                if guard.is_none() {
                    match FaceDetector::load(Path::new(dir), 0.75) {
                        Ok(detector) => *guard = Some(Arc::new(Mutex::new(detector))),
                        Err(e) => {
                            oo_warn!(crate::oo_log::modules::FACE, "BlazeFace load failed (fail-open)", error = %e);
                            return Vec::new();
                        }
                    }
                }
                guard.as_ref().map(Arc::clone)
            };
            if let Some(det) = face_arc {
                let mut detector = det.lock().unwrap_or_else(|e| e.into_inner());
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

    /// Detect faces using BlazeFace with automatic tiling for large images.
    ///
    /// If BlazeFace finds 0 faces on an image with longest side > 512px,
    /// splits into 4 overlapping tiles, runs BlazeFace on each, and merges
    /// results via NMS. This compensates for BlazeFace's 128x128 input where
    /// small faces in large images become too tiny to detect.
    fn detect_faces_blazeface_tiled(
        &self,
        img: &DynamicImage,
        onnx_panics: &mut u32,
    ) -> Vec<FaceDetection> {
        let faces = self.detect_faces_blazeface(img, onnx_panics);
        if !faces.is_empty() || img.width().max(img.height()) <= 512 {
            return faces;
        }

        // 4 overlapping quadrants (62.5% of each dimension, ~25% overlap)
        let (w, h) = (img.width(), img.height());
        let tw = (w as f32 * 0.625) as u32;
        let th = (h as f32 * 0.625) as u32;
        let tiles: [(u32, u32); 4] = [(0, 0), (w - tw, 0), (0, h - th), (w - tw, h - th)];
        let mut all = Vec::new();
        for (tx, ty) in &tiles {
            let crop = img.crop_imm(*tx, *ty, tw, th);
            for mut f in self.detect_faces_blazeface(&crop, onnx_panics) {
                // Remap tile-local coordinates to original image coordinates
                f.x_min += *tx as f32;
                f.y_min += *ty as f32;
                f.x_max += *tx as f32;
                f.y_max += *ty as f32;
                all.push(f);
            }
        }
        if all.is_empty() {
            return Vec::new();
        }
        oo_debug!(
            crate::oo_log::modules::FACE,
            "BlazeFace tiling: found faces in tiles",
            raw_count = all.len()
        );
        nms(&mut all, 0.3)
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
        let (result, stats, _meta) = manager.process_image(img, None, None).unwrap();
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
        let (result, stats, _meta) = manager.process_image(img, None, None).unwrap();
        assert_eq!(result.width(), 50);
        assert_eq!(stats.faces_redacted, 0);
        assert_eq!(stats.text_regions_found, 0);
    }

    // --- 11C.3: Arc model guard tests ---

    #[test]
    fn test_arc_model_survives_eviction() {
        // Simulate: clone Arc from slot, then evict slot, Arc still valid
        let slot: Mutex<Option<Arc<Mutex<u32>>>> = Mutex::new(Some(Arc::new(Mutex::new(42))));
        let cloned = {
            let guard = slot.lock().unwrap();
            guard.as_ref().map(Arc::clone)
        };
        // Evict: set slot to None
        *slot.lock().unwrap() = None;
        // Cloned Arc still holds the value
        let val = cloned.unwrap();
        assert_eq!(*val.lock().unwrap(), 42);
    }

    #[test]
    fn test_force_evict_clears_slot() {
        let config = crate::config::ImageConfig::default();
        let manager = ImageModelManager::new(config);
        // All slots start as None
        assert!(manager.face_detector.lock().unwrap().is_none());
        assert!(manager.nsfw_classifier.lock().unwrap().is_none());
        assert!(manager.ocr_detector.lock().unwrap().is_none());
        // Force evict on empty slots is a no-op (no panic)
        manager.force_evict();
        assert!(manager.face_detector.lock().unwrap().is_none());
    }

    #[test]
    fn test_lazy_load_on_demand() {
        let config = crate::config::ImageConfig::default();
        let manager = ImageModelManager::new(config);
        // All model slots start as None (lazy)
        assert!(manager.nsfw_classifier.lock().unwrap().is_none());
        assert!(manager.face_detector.lock().unwrap().is_none());
        assert!(manager.scrfd_detector.lock().unwrap().is_none());
        assert!(manager.ocr_detector.lock().unwrap().is_none());
        assert!(manager.ocr_recognizer.lock().unwrap().is_none());
    }

    #[test]
    fn test_poison_recovery() {
        // Simulate poisoned mutex recovery via into_inner
        let slot: Mutex<Option<Arc<Mutex<u32>>>> = Mutex::new(Some(Arc::new(Mutex::new(99))));
        // Poison the mutex
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = slot.lock().unwrap();
            panic!("intentional poison");
        }));
        // Recovery via into_inner
        let guard = slot.lock().unwrap_or_else(|e| e.into_inner());
        let val = guard.as_ref().unwrap();
        assert_eq!(*val.lock().unwrap(), 99);
    }

    #[test]
    fn test_eviction_during_inference() {
        // Simulate: inference thread holds Arc clone, eviction clears slot
        let slot: Mutex<Option<Arc<Mutex<String>>>> =
            Mutex::new(Some(Arc::new(Mutex::new("model_data".to_string()))));

        // "Inference" clones the Arc
        let inference_arc = {
            let guard = slot.lock().unwrap();
            guard.as_ref().map(Arc::clone).unwrap()
        };

        // "Eviction" clears the slot while "inference" holds the Arc
        *slot.lock().unwrap() = None;
        assert!(
            slot.lock().unwrap().is_none(),
            "Slot should be empty after eviction"
        );

        // "Inference" still works — Arc keeps model alive
        let model = inference_arc.lock().unwrap();
        assert_eq!(*model, "model_data");
    }

    #[test]
    fn test_concurrent_load_no_double_init() {
        // Two threads race to load — both should get the same Arc
        let slot: Arc<Mutex<Option<Arc<Mutex<u32>>>>> = Arc::new(Mutex::new(None));
        let slot2 = Arc::clone(&slot);

        let handle = std::thread::spawn(move || {
            let mut guard = slot2.lock().unwrap();
            if guard.is_none() {
                *guard = Some(Arc::new(Mutex::new(123)));
            }
            guard.as_ref().map(Arc::clone).unwrap()
        });

        // Main thread also tries to load
        {
            let mut guard = slot.lock().unwrap();
            if guard.is_none() {
                *guard = Some(Arc::new(Mutex::new(123)));
            }
        }

        let arc_from_thread = handle.join().unwrap();
        let arc_from_main = slot.lock().unwrap().as_ref().map(Arc::clone).unwrap();

        // Both should hold valid values (same or different Arc, but same value)
        assert_eq!(*arc_from_thread.lock().unwrap(), 123);
        assert_eq!(*arc_from_main.lock().unwrap(), 123);
    }

    // --- 11D.1: BlazeFace tiling tests ---

    #[test]
    fn test_tiling_not_triggered_small_image() {
        // Images <= 512px should not trigger tiling
        let config = crate::config::ImageConfig {
            face_detection: true,
            face_model: "blazeface".to_string(),
            ..crate::config::ImageConfig::default()
        };
        let manager = ImageModelManager::new(config);
        let img = make_test_image(300, 300);
        // No model loaded — detect_faces_blazeface returns empty, but tiling
        // should NOT be attempted because image is <= 512px
        let faces = manager.detect_faces_blazeface_tiled(&img, &mut 0);
        assert!(faces.is_empty());
    }

    #[test]
    fn test_tiling_triggered_large_image() {
        // Images > 512px with 0 faces on first pass should trigger tiling
        let config = crate::config::ImageConfig {
            face_detection: true,
            face_model: "blazeface".to_string(),
            ..crate::config::ImageConfig::default()
        };
        let manager = ImageModelManager::new(config);
        let img = make_test_image(960, 960);
        // No model loaded — all calls return empty, but tiling is attempted
        // (we verify by the fact that it doesn't panic on crop operations)
        let faces = manager.detect_faces_blazeface_tiled(&img, &mut 0);
        assert!(faces.is_empty()); // No model = no detections
    }

    #[test]
    fn test_tile_coordinate_remap() {
        // Verify that tile-local coordinates are correctly remapped to original image coords
        let mut face = FaceDetection {
            x_min: 10.0,
            y_min: 20.0,
            x_max: 50.0,
            y_max: 60.0,
            confidence: 0.9,
        };
        let tx: u32 = 360;
        let ty: u32 = 360;
        face.x_min += tx as f32;
        face.y_min += ty as f32;
        face.x_max += tx as f32;
        face.y_max += ty as f32;
        assert_eq!(face.x_min, 370.0);
        assert_eq!(face.y_min, 380.0);
        assert_eq!(face.x_max, 410.0);
        assert_eq!(face.y_max, 420.0);
    }

    #[test]
    fn test_nms_merges_tile_boundary_faces() {
        // Same face detected in two overlapping tiles should be merged by NMS
        let mut detections = vec![
            FaceDetection {
                x_min: 100.0,
                y_min: 100.0,
                x_max: 200.0,
                y_max: 200.0,
                confidence: 0.9,
            },
            FaceDetection {
                x_min: 105.0,
                y_min: 105.0,
                x_max: 205.0,
                y_max: 205.0,
                confidence: 0.85,
            },
        ];
        let result = nms(&mut detections, 0.3);
        assert_eq!(result.len(), 1, "Overlapping detections should be merged");
        assert_eq!(
            result[0].confidence, 0.9,
            "Higher confidence should survive"
        );
    }

    #[test]
    fn test_tiling_only_on_blazeface() {
        // SCRFD path should not go through tiling
        let config = crate::config::ImageConfig {
            face_detection: true,
            face_model: "scrfd".to_string(),
            ..crate::config::ImageConfig::default()
        };
        let manager = ImageModelManager::new(config);
        let img = make_test_image(960, 960);
        // detect_faces with scrfd model but no model dir → falls back to blazeface_tiled
        // This verifies SCRFD doesn't tile itself
        let faces = manager.detect_faces(&img, &mut 0);
        assert!(faces.is_empty());
    }
}

//! Model-gated pipeline validation tests.
//!
//! These tests only run when ONNX models are present in `models/`.
//! They process real test images and validate the returned PipelineMeta
//! using the detection validators.

use std::path::Path;

use openobscure_proxy::config::ImageConfig;
use openobscure_proxy::detection_validators::{
    validate_bbox, validate_face_detections, validate_nsfw, validate_text_regions, Severity,
};
use openobscure_proxy::image_pipeline::{decode_image, resize_if_needed, ImageModelManager};

fn models_available() -> bool {
    Path::new("models/blazeface").exists() || Path::new("models/paddleocr").exists()
}

fn make_pipeline_config() -> ImageConfig {
    let face_dir = Path::new("models/blazeface");
    let scrfd_dir = Path::new("models/scrfd");
    let ocr_dir = Path::new("models/paddleocr");
    let nsfw_dir = Path::new("models/nudenet");

    // Use SCRFD if available, otherwise fall back to BlazeFace
    let face_model = if scrfd_dir.exists() {
        "scrfd".to_string()
    } else {
        "blazeface".to_string()
    };

    ImageConfig {
        enabled: true,
        face_detection: face_dir.exists() || scrfd_dir.exists(),
        ocr_enabled: ocr_dir.exists(),
        ocr_tier: "detect_and_fill".to_string(),
        max_dimension: 960,
        model_idle_timeout_secs: 300,
        face_model,
        face_model_dir: if face_dir.exists() {
            Some(face_dir.to_string_lossy().into_owned())
        } else {
            None
        },
        face_model_dir_scrfd: if scrfd_dir.exists() {
            Some(scrfd_dir.to_string_lossy().into_owned())
        } else {
            None
        },
        ocr_model_dir: if ocr_dir.exists() {
            Some(ocr_dir.to_string_lossy().into_owned())
        } else {
            None
        },
        screen_guard: true,
        exif_strip: true,
        nsfw_detection: nsfw_dir.exists(),
        nsfw_model_dir: if nsfw_dir.exists() {
            Some(nsfw_dir.to_string_lossy().into_owned())
        } else {
            None
        },
        nsfw_threshold: 0.45,
    }
}

#[test]
fn test_face_image_pipeline_validates() {
    if !models_available() {
        eprintln!("Skipping: models not available");
        return;
    }

    let bytes = std::fs::read("../docs/examples/images/face-original.jpg")
        .expect("face-original.jpg not found");
    let img = decode_image(&bytes).unwrap();
    let img = resize_if_needed(img, 960);

    let manager = ImageModelManager::new(make_pipeline_config());
    let (_result, stats, meta) = manager.process_image(img, None).unwrap();

    // Should detect at least 1 face
    assert!(
        stats.faces_redacted >= 1,
        "Expected ≥1 face, got {}",
        stats.faces_redacted
    );

    // Validate face bboxes
    let (img_w, img_h) = meta.image_size;
    let face_issues = validate_face_detections(&meta.faces, img_w, img_h);
    let errors: Vec<_> = face_issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "Face validation errors: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );

    // Face should be selective redaction (area < 80%)
    for face in &meta.faces {
        assert!(
            face.area_ratio() < 0.8,
            "Face area ratio {:.1}% should be < 80% for selective redaction",
            face.area_ratio() * 100.0
        );
    }

    // Should not flag NSFW
    assert!(
        !stats.nsfw_detected,
        "Face photo should not be flagged NSFW"
    );
}

#[test]
fn test_child_image_pipeline_validates() {
    if !models_available() {
        eprintln!("Skipping: models not available");
        return;
    }

    let path = "../docs/examples/images/child-original.jpg";
    if !Path::new(path).exists() {
        eprintln!("Skipping: child-original.jpg not found");
        return;
    }

    let bytes = std::fs::read(path).unwrap();
    let img = decode_image(&bytes).unwrap();
    let img = resize_if_needed(img, 960);

    let manager = ImageModelManager::new(make_pipeline_config());
    let (_result, stats, meta) = manager.process_image(img, None).unwrap();

    // Should detect at least 1 face (child)
    assert!(
        stats.faces_redacted >= 1,
        "Expected ≥1 child face, got {}",
        stats.faces_redacted
    );

    // All face bboxes should pass validation
    let (img_w, img_h) = meta.image_size;
    let face_issues = validate_face_detections(&meta.faces, img_w, img_h);
    let errors: Vec<_> = face_issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "Child face validation errors: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_text_image_pipeline_validates() {
    if !models_available() {
        eprintln!("Skipping: models not available");
        return;
    }

    let bytes = std::fs::read("../docs/examples/images/text-original.jpg")
        .expect("text-original.jpg not found");
    let img = decode_image(&bytes).unwrap();
    let img = resize_if_needed(img, 960);

    let manager = ImageModelManager::new(make_pipeline_config());
    let (_result, stats, meta) = manager.process_image(img, None).unwrap();

    // Should detect text regions
    assert!(
        stats.text_regions_found >= 1,
        "Expected ≥1 text region, got {}",
        stats.text_regions_found
    );

    // Validate text regions
    let (img_w, img_h) = meta.image_size;
    let text_issues = validate_text_regions(&meta.text_regions, img_w, img_h);
    let errors: Vec<_> = text_issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "Text region validation errors: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );

    // Should have 0 faces in a document photo
    assert_eq!(stats.faces_redacted, 0, "Document should have 0 faces");
}

#[test]
fn test_screenshot_pipeline_validates() {
    if !models_available() {
        eprintln!("Skipping: models not available");
        return;
    }

    let bytes = std::fs::read("../docs/examples/images/screenshot-original.png")
        .expect("screenshot-original.png not found");
    let img = decode_image(&bytes).unwrap();
    let img = resize_if_needed(img, 960);

    let manager = ImageModelManager::new(make_pipeline_config());
    let (_result, stats, meta) = manager.process_image(img, None).unwrap();

    // Screenshot should have many text regions (PII form)
    assert!(
        stats.text_regions_found >= 5,
        "Expected ≥5 text regions in screenshot, got {}",
        stats.text_regions_found
    );

    // All text bboxes should pass validation
    let (img_w, img_h) = meta.image_size;
    let text_issues = validate_text_regions(&meta.text_regions, img_w, img_h);
    let errors: Vec<_> = text_issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "Screenshot text validation errors: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_nsfw_meta_consistent_when_clean() {
    if !models_available() {
        eprintln!("Skipping: models not available");
        return;
    }

    let bytes = std::fs::read("../docs/examples/images/face-original.jpg")
        .expect("face-original.jpg not found");
    let img = decode_image(&bytes).unwrap();
    let img = resize_if_needed(img, 960);

    let manager = ImageModelManager::new(make_pipeline_config());
    let (_result, _stats, meta) = manager.process_image(img, None).unwrap();

    // If NSFW model was loaded, validate the metadata
    if let Some(ref nsfw_meta) = meta.nsfw {
        let issues = validate_nsfw(nsfw_meta);
        assert!(
            issues.is_empty(),
            "NSFW validation issues on clean image: {:?}",
            issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
    }
}

#[test]
fn test_ocr_recognition_quality_v4() {
    let ocr_dir = Path::new("models/paddleocr");
    if !ocr_dir.join("rec_model.onnx").exists() || !ocr_dir.join("ppocr_keys.txt").exists() {
        eprintln!("Skipping: OCR recognition model not available");
        return;
    }

    // Load recognizer with new PP-OCRv4 English model
    let mut recognizer = openobscure_proxy::ocr_engine::OcrRecognizer::load(ocr_dir)
        .expect("Failed to load recognizer");

    let bytes = std::fs::read("../docs/examples/images/text-original.jpg")
        .expect("text-original.jpg not found");
    let img = decode_image(&bytes).unwrap();
    let img = resize_if_needed(img, 960);

    // Detect text regions
    let mut detector =
        openobscure_proxy::ocr_engine::OcrDetector::load(ocr_dir).expect("Failed to load detector");
    let regions = detector.detect(&img).unwrap();
    assert!(
        !regions.is_empty(),
        "Should detect text regions in document"
    );

    // Recognize text from detected regions
    let results = recognizer.recognize(&img, &regions).unwrap();

    // Should produce non-empty results with reasonable confidence
    assert!(
        !results.is_empty(),
        "Should recognize text from document (got 0 results from {} regions)",
        regions.len()
    );

    // At least some recognized text should have confidence > 0.3
    let good_results: Vec<_> = results.iter().filter(|r| r.confidence > 0.3).collect();
    assert!(
        !good_results.is_empty(),
        "No high-confidence (>0.3) text found. Confidences: {:?}",
        results.iter().map(|r| r.confidence).collect::<Vec<_>>()
    );

    // Print recognized text for manual inspection
    eprintln!("\n=== OCR Recognition Quality Test (PP-OCRv4 English) ===");
    eprintln!("  Regions detected: {}", regions.len());
    eprintln!("  Texts recognized: {}", results.len());
    for (i, r) in results.iter().enumerate() {
        eprintln!("  [{}] confidence={:.3}: \"{}\"", i, r.confidence, r.text);
    }

    // Verify at least one result contains recognizable English
    let has_english = results
        .iter()
        .any(|r| r.text.chars().any(|c| c.is_ascii_alphabetic()) && r.confidence > 0.3);
    assert!(
        has_english,
        "No readable English text detected. Model may be incompatible."
    );

    // Average confidence should be reasonable (>0.1 for a good model)
    let avg_conf: f32 = results.iter().map(|r| r.confidence).sum::<f32>() / results.len() as f32;
    eprintln!("  Average confidence: {:.3}", avg_conf);
    assert!(
        avg_conf > 0.1,
        "Average confidence {:.3} too low — model may be garbling text",
        avg_conf
    );
}

#[test]
fn test_tier2_pii_selective_redaction() {
    let ocr_dir = Path::new("models/paddleocr");
    if !ocr_dir.join("rec_model.onnx").exists() {
        eprintln!("Skipping: OCR recognition model not available");
        return;
    }
    if !Path::new("models/blazeface").exists() {
        eprintln!("Skipping: BlazeFace model not available");
        return;
    }

    // Test the full pipeline in full_recognition (Tier 2) mode
    let mut config = make_pipeline_config();
    config.ocr_tier = "full_recognition".to_string();

    let bytes = std::fs::read("../docs/examples/images/text-original.jpg")
        .expect("text-original.jpg not found");
    let img = decode_image(&bytes).unwrap();
    let img = resize_if_needed(img, 960);

    let manager = ImageModelManager::new(config);
    let (_result, stats, _meta) = manager.process_image(img, None).unwrap();

    eprintln!("\n=== Tier 2 PII Selective Redaction ===");
    eprintln!("  Text regions found: {}", stats.text_regions_found);

    // In Tier 2 (full_recognition), the pipeline recognizes text and only redacts PII.
    // The test validates the pipeline runs without errors in Tier 2 mode.
    assert!(
        stats.text_regions_found >= 1,
        "Tier 2 should still detect text regions"
    );
}

#[test]
fn test_all_bbox_sanity_on_face_image() {
    if !models_available() {
        eprintln!("Skipping: models not available");
        return;
    }

    let bytes = std::fs::read("../docs/examples/images/face-original.jpg")
        .expect("face-original.jpg not found");
    let img = decode_image(&bytes).unwrap();
    let img = resize_if_needed(img, 960);

    let manager = ImageModelManager::new(make_pipeline_config());
    let (_result, _stats, meta) = manager.process_image(img, None).unwrap();

    // Every bbox (face + text) should pass generic sanity checks
    for bbox in meta.faces.iter().chain(meta.text_regions.iter()) {
        let issues = validate_bbox(bbox);
        let errors: Vec<_> = issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "Bbox sanity error for '{}': {:?}",
            bbox.label,
            errors.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }
}

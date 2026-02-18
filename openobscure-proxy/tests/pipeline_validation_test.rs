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
    let ocr_dir = Path::new("models/paddleocr");
    let nsfw_dir = Path::new("models/nudenet");

    ImageConfig {
        enabled: true,
        face_detection: face_dir.exists(),
        ocr_enabled: ocr_dir.exists(),
        ocr_tier: "detect_and_blur".to_string(),
        max_dimension: 960,
        face_blur_sigma: 25.0,
        text_blur_sigma: 20.0,
        model_idle_timeout_secs: 300,
        face_model_dir: if face_dir.exists() {
            Some(face_dir.to_string_lossy().into_owned())
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
    let (_result, stats, meta) = manager.process_image(img).unwrap();

    // Should detect at least 1 face
    assert!(
        stats.faces_blurred >= 1,
        "Expected ≥1 face, got {}",
        stats.faces_blurred
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

    // Face should be selective blur (area < 80%)
    for face in &meta.faces {
        assert!(
            face.area_ratio() < 0.8,
            "Face area ratio {:.1}% should be < 80% for selective blur",
            face.area_ratio() * 100.0
        );
    }

    // Should not flag NSFW
    assert!(!stats.nsfw_detected, "Face photo should not be flagged NSFW");
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
    let (_result, stats, meta) = manager.process_image(img).unwrap();

    // Should detect at least 1 face (child)
    assert!(
        stats.faces_blurred >= 1,
        "Expected ≥1 child face, got {}",
        stats.faces_blurred
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
    let (_result, stats, meta) = manager.process_image(img).unwrap();

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
    assert_eq!(stats.faces_blurred, 0, "Document should have 0 faces");
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
    let (_result, stats, meta) = manager.process_image(img).unwrap();

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
    let (_result, _stats, meta) = manager.process_image(img).unwrap();

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
    let (_result, _stats, meta) = manager.process_image(img).unwrap();

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

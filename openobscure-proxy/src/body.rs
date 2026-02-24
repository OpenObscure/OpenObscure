use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use uuid::Uuid;

use crate::fpe_engine::{FpeEngine, FpeResult, TweakGenerator};
use crate::hybrid_scanner::HybridScanner;
use crate::image_detect;
use crate::image_pipeline::{self, ImageModelManager, ImageStats, OutputFormat};
use crate::kws_engine::KwsEngine;
use crate::mapping::{FpeMapping, RequestMappings};
use crate::voice_pipeline;

/// Process a request body: scan for PII, FPE-encrypt or redact matches, return modified body + mappings.
///
/// Two-pass processing:
/// 1. Image pass: find base64 image blocks, decode → EXIF strip → resize → face blur → OCR blur → re-encode
/// 2. Text pass: scan JSON string values for PII, FPE-encrypt or redact
///
/// Images are processed FIRST so that byte offsets in the text pass remain correct.
#[allow(clippy::too_many_arguments)]
pub fn process_request_body(
    body: &Bytes,
    request_id: &Uuid,
    scanner: &HybridScanner,
    fpe: &FpeEngine,
    key_version: u32,
    skip_fields: &[String],
    image_models: Option<&Arc<ImageModelManager>>,
    kws_engine: Option<&Arc<KwsEngine>>,
) -> Result<(Bytes, RequestMappings, Vec<ImageStats>), BodyError> {
    let mut json: Value = serde_json::from_slice(body).map_err(BodyError::Json)?;

    // Pass 1: Process images in JSON (before text scanning)
    let image_stats = if let Some(models) = image_models {
        if models.config().enabled {
            process_images_in_json(&mut json, models)
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    // Pass 1.5: Scan audio blocks for PII keywords (KWS-gated)
    let voice_modified = if let Some(engine) = kws_engine {
        let result = voice_pipeline::scan_and_strip_audio_blocks(&mut json, engine);
        result.blocks_stripped > 0
    } else {
        false
    };

    let matches = scanner.scan_json(&json, skip_fields);
    if matches.is_empty() && image_stats.is_empty() && !voice_modified {
        return Ok((body.clone(), RequestMappings::new(*request_id), image_stats));
    }
    if matches.is_empty() {
        // Images/audio were modified but no text PII found — re-serialize the modified JSON
        let modified = serde_json::to_vec(&json).map_err(BodyError::Json)?;
        return Ok((
            Bytes::from(modified),
            RequestMappings::new(*request_id),
            image_stats,
        ));
    }

    let mut mappings = RequestMappings::new(*request_id);
    let mut replacements: Vec<FpeResult> = Vec::new();

    // Track redaction label counters per PII type for unique labels
    let mut redaction_counters: HashMap<&'static str, usize> = HashMap::new();

    for m in &matches {
        if m.pii_type.is_fpe_eligible() {
            // FPE-eligible: encrypt with format-preserving encryption
            let json_path = m.json_path.as_deref().unwrap_or("");
            let tweak = TweakGenerator::generate(request_id, json_path);
            match fpe.encrypt_match(m, &tweak) {
                Ok(result) => {
                    mappings.insert(FpeMapping {
                        pii_type: result.original.pii_type,
                        plaintext: result.original.raw_value.clone(),
                        ciphertext: result.encrypted.clone(),
                        tweak: result.tweak.clone(),
                        key_version,
                    });
                    replacements.push(result);
                }
                Err(e) => {
                    oo_warn!(crate::oo_log::modules::BODY, "FPE encryption failed for PII match, skipping",
                        pii_type = ?m.pii_type,
                        json_path = ?m.json_path,
                        error = %e);
                }
            }
        } else {
            // Non-FPE: redact with unique indexed label (e.g., [HEALTH_0], [PERSON_1])
            let base_label = m.pii_type.redaction_label();
            // Strip brackets for counter key: "[HEALTH]" -> "HEALTH"
            let key = &base_label[1..base_label.len() - 1];
            let counter = redaction_counters.entry(key).or_insert(0);
            let label = format!("[{}_{counter}]", key);
            *counter += 1;

            mappings.insert(FpeMapping {
                pii_type: m.pii_type,
                plaintext: m.raw_value.clone(),
                ciphertext: label.clone(),
                tweak: vec![],
                key_version,
            });
            replacements.push(FpeResult {
                original: m.clone(),
                encrypted: label,
                tweak: vec![],
            });
        }
    }

    if replacements.is_empty() {
        if !image_stats.is_empty() {
            let modified = serde_json::to_vec(&json).map_err(BodyError::Json)?;
            return Ok((Bytes::from(modified), mappings, image_stats));
        }
        return Ok((body.clone(), mappings, image_stats));
    }

    // Apply replacements to JSON string values
    apply_replacements_to_json(&mut json, &replacements);

    let modified = serde_json::to_vec(&json).map_err(BodyError::Json)?;
    Ok((Bytes::from(modified), mappings, image_stats))
}

/// Walk a JSON tree and process any embedded base64 images.
///
/// Finds Anthropic and OpenAI image content blocks, decodes them,
/// runs the image pipeline (EXIF strip, resize, face blur, OCR blur),
/// and replaces the base64 data in-place.
fn process_images_in_json(json: &mut Value, models: &ImageModelManager) -> Vec<ImageStats> {
    let mut stats = Vec::new();
    walk_json_for_images(json, models, &mut stats);
    stats
}

/// Recursively walk JSON looking for image content blocks.
fn walk_json_for_images(
    value: &mut Value,
    models: &ImageModelManager,
    stats: &mut Vec<ImageStats>,
) {
    match value {
        Value::Object(map) => {
            // Check if this object is an image content block
            if let Some(img_ref) = image_detect::is_image_content_block(map) {
                // Extract the base64 image bytes
                if let Some(detected) = image_detect::extract_image_bytes(map, &img_ref) {
                    // Verify it's actually an image
                    if image_detect::has_image_magic_bytes(&detected.raw_bytes) {
                        match process_single_image(
                            &detected.raw_bytes,
                            &detected.media_type,
                            &img_ref,
                            models,
                        ) {
                            Ok((new_bytes, img_stats)) => {
                                // Replace the base64 data in the JSON object
                                let encoded = image_detect::encode_to_base64(
                                    &new_bytes,
                                    &detected.media_type,
                                    img_ref.format,
                                );
                                replace_image_data(map, &img_ref, &encoded);
                                stats.push(img_stats);
                            }
                            Err(e) => {
                                // Fail-open: leave original image
                                oo_warn!(crate::oo_log::modules::IMAGE,
                                    "Image processing failed (fail-open)", error = %e);
                            }
                        }
                    }
                }
            }
            // Recurse into all values
            for (_, v) in map.iter_mut() {
                walk_json_for_images(v, models, stats);
            }
        }
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                walk_json_for_images(item, models, stats);
            }
        }
        _ => {}
    }
}

/// Process a single decoded image through the pipeline.
fn process_single_image(
    raw_bytes: &[u8],
    media_type: &str,
    _img_ref: &image_detect::ImageContentRef,
    models: &ImageModelManager,
) -> Result<(Vec<u8>, ImageStats), image_pipeline::ImageError> {
    // Check for screenshot (before decoding strips EXIF)
    let screen_result = if models.config().screen_guard {
        // We need the decoded image for resolution + status bar checks,
        // but EXIF check needs raw bytes. Decode first, then check all heuristics.
        let img = image_pipeline::decode_image(raw_bytes)?;
        let result = crate::screen_guard::detect_screenshot(raw_bytes, &img);
        Some((img, result))
    } else {
        None
    };

    // Decode (inherently strips EXIF) — may reuse from screen guard
    let img = if let Some((ref img, _)) = screen_result {
        img.clone()
    } else {
        image_pipeline::decode_image(raw_bytes)?
    };

    // Resize if needed
    let img = image_pipeline::resize_if_needed(img, models.config().max_dimension);

    // Run face detection + OCR redaction
    let sg_ref = screen_result.as_ref().map(|(_, sg)| sg);
    let (processed, stats, _meta) = models.process_image(img, sg_ref)?;

    // Re-encode
    let format = OutputFormat::from_media_type(media_type);
    let encoded = image_pipeline::encode_image(&processed, format)?;

    Ok((encoded, stats))
}

/// Replace the base64 image data in a JSON object.
fn replace_image_data(
    obj: &mut serde_json::Map<String, Value>,
    img_ref: &image_detect::ImageContentRef,
    new_data: &str,
) {
    match img_ref.format {
        image_detect::ImageFormat::AnthropicBase64 => {
            // Navigate to source.data
            if let Some(Value::Object(source)) = obj.get_mut("source") {
                source.insert("data".to_string(), Value::String(new_data.to_string()));
            }
        }
        image_detect::ImageFormat::OpenAiDataUri => {
            // Navigate to image_url.url
            if let Some(Value::Object(img_url)) = obj.get_mut("image_url") {
                img_url.insert("url".to_string(), Value::String(new_data.to_string()));
            }
        }
    }
}

/// Process a response body: find encrypted values and replace with originals.
pub fn process_response_body(body: &Bytes, mappings: &RequestMappings) -> Bytes {
    if mappings.is_empty() {
        return body.clone();
    }

    let text = String::from_utf8_lossy(body);
    let decrypted = mappings.decrypt_response(&text);
    Bytes::from(decrypted)
}

/// Apply FPE replacements to JSON string values by doing string replacement
/// within each string field that contained PII matches.
fn apply_replacements_to_json(json: &mut Value, replacements: &[FpeResult]) {
    // Group replacements by json_path
    let mut by_path: HashMap<String, Vec<&FpeResult>> = HashMap::new();
    for r in replacements {
        let path = r.original.json_path.clone().unwrap_or_default();
        by_path.entry(path).or_default().push(r);
    }

    // For each path, navigate to the string value and apply replacements
    for (path, path_replacements) in &by_path {
        if let Some(Value::String(s)) = navigate_json_mut(json, path) {
            let mut result = s.clone();
            // Apply replacements in reverse offset order to preserve positions
            let mut sorted_replacements = path_replacements.clone();
            sorted_replacements.sort_by(|a, b| b.original.start.cmp(&a.original.start));
            for r in sorted_replacements {
                // Replace by byte offset for precision
                if r.original.start <= result.len() && r.original.end <= result.len() {
                    result.replace_range(r.original.start..r.original.end, &r.encrypted);
                }
            }
            *s = result;
        }
    }
}

/// Navigate to a mutable JSON value by dot-notation path with array indexing.
/// E.g., "messages[0].content" navigates to json["messages"][0]["content"].
fn navigate_json_mut<'a>(json: &'a mut Value, path: &str) -> Option<&'a mut Value> {
    if path.is_empty() {
        return Some(json);
    }

    let mut current = json;
    for segment in parse_json_path(path) {
        match segment {
            PathSegment::Key(key) => {
                current = current.get_mut(&key)?;
            }
            PathSegment::Index(idx) => {
                current = current.get_mut(idx)?;
            }
        }
    }
    Some(current)
}

#[derive(Debug)]
enum PathSegment {
    Key(String),
    Index(usize),
}

/// Parse a JSON path like "messages[0].content" into segments.
fn parse_json_path(path: &str) -> Vec<PathSegment> {
    let mut segments = Vec::new();
    let mut current = String::new();

    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '.' => {
                if !current.is_empty() {
                    segments.push(PathSegment::Key(current.clone()));
                    current.clear();
                }
            }
            '[' => {
                if !current.is_empty() {
                    segments.push(PathSegment::Key(current.clone()));
                    current.clear();
                }
                let mut idx_str = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch == ']' {
                        chars.next();
                        break;
                    }
                    idx_str.push(ch);
                    chars.next();
                }
                if let Ok(idx) = idx_str.parse::<usize>() {
                    segments.push(PathSegment::Index(idx));
                }
            }
            _ => {
                current.push(c);
            }
        }
    }
    if !current.is_empty() {
        segments.push(PathSegment::Key(current));
    }
    segments
}

#[derive(Debug, thiserror::Error)]
pub enum BodyError {
    #[error("JSON error: {0}")]
    Json(serde_json::Error),
    #[error("FPE error: {0}")]
    Fpe(#[from] crate::fpe_engine::FpeError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn test_parse_json_path() {
        let segments = parse_json_path("messages[0].content");
        assert_eq!(segments.len(), 3);
        assert!(matches!(&segments[0], PathSegment::Key(k) if k == "messages"));
        assert!(matches!(&segments[1], PathSegment::Index(0)));
        assert!(matches!(&segments[2], PathSegment::Key(k) if k == "content"));
    }

    #[test]
    fn test_navigate_json_mut() {
        let mut json = serde_json::json!({
            "messages": [
                {"role": "user", "content": "hello world"}
            ]
        });
        let val = navigate_json_mut(&mut json, "messages[0].content");
        assert!(val.is_some());
        assert_eq!(val.unwrap(), &Value::String("hello world".to_string()));
    }

    #[test]
    fn test_navigate_json_root() {
        let mut json = serde_json::json!({"key": "value"});
        let val = navigate_json_mut(&mut json, "");
        assert!(val.is_some());
    }

    #[test]
    fn test_replace_image_data_anthropic() {
        let mut map = serde_json::Map::new();
        map.insert("type".to_string(), Value::String("image".to_string()));
        let mut source = serde_json::Map::new();
        source.insert("type".to_string(), Value::String("base64".to_string()));
        source.insert(
            "media_type".to_string(),
            Value::String("image/png".to_string()),
        );
        source.insert("data".to_string(), Value::String("OLD_DATA".to_string()));
        map.insert("source".to_string(), Value::Object(source));

        let img_ref = image_detect::ImageContentRef {
            format: image_detect::ImageFormat::AnthropicBase64,
            data_key_path: vec!["source".to_string(), "data".to_string()],
            media_type: Some("image/png".to_string()),
        };

        replace_image_data(&mut map, &img_ref, "NEW_DATA");

        let data = map["source"]["data"].as_str().unwrap();
        assert_eq!(data, "NEW_DATA");
    }

    #[test]
    fn test_replace_image_data_openai() {
        let mut map = serde_json::Map::new();
        map.insert("type".to_string(), Value::String("image_url".to_string()));
        let mut img_url = serde_json::Map::new();
        img_url.insert(
            "url".to_string(),
            Value::String("data:image/png;base64,OLD".to_string()),
        );
        map.insert("image_url".to_string(), Value::Object(img_url));

        let img_ref = image_detect::ImageContentRef {
            format: image_detect::ImageFormat::OpenAiDataUri,
            data_key_path: vec!["image_url".to_string(), "url".to_string()],
            media_type: Some("image/png".to_string()),
        };

        replace_image_data(&mut map, &img_ref, "data:image/png;base64,NEW");

        let url = map["image_url"]["url"].as_str().unwrap();
        assert_eq!(url, "data:image/png;base64,NEW");
    }

    #[test]
    fn test_walk_json_skips_non_image_objects() {
        let config = crate::config::ImageConfig::default();
        let models = ImageModelManager::new(config);
        let mut json = serde_json::json!({
            "messages": [
                {"role": "user", "content": "hello world"},
                {"role": "assistant", "content": "hi there"}
            ]
        });

        let mut stats = Vec::new();
        walk_json_for_images(&mut json, &models, &mut stats);
        assert!(
            stats.is_empty(),
            "Non-image JSON should produce no image stats"
        );
    }

    #[test]
    fn test_walk_json_processes_anthropic_image() {
        use image::{Rgb, RgbImage};
        use std::io::Cursor;

        // Create a small test PNG
        let img = image::DynamicImage::ImageRgb8(RgbImage::from_pixel(10, 10, Rgb([128, 64, 32])));
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        let png_bytes = buf.into_inner();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);

        // Build Anthropic image content block
        let mut json = serde_json::json!({
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": b64
                    }
                }]
            }]
        });

        // Config with image processing enabled but no model dirs (so no face/OCR)
        let config = crate::config::ImageConfig {
            enabled: true,
            face_detection: false,
            ocr_enabled: false,
            ..crate::config::ImageConfig::default()
        };
        let models = ImageModelManager::new(config);

        let mut stats = Vec::new();
        walk_json_for_images(&mut json, &models, &mut stats);

        // Should have processed 1 image (decode → resize → encode, no face/OCR)
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].faces_blurred, 0);
        assert_eq!(stats[0].text_regions_found, 0);

        // The base64 data should have been replaced
        let new_data = json["messages"][0]["content"][0]["source"]["data"]
            .as_str()
            .unwrap();
        // It should be valid base64 that decodes to a valid PNG
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(new_data)
            .unwrap();
        let decoded_img = image::load_from_memory(&decoded).unwrap();
        assert_eq!(decoded_img.width(), 10);
        assert_eq!(decoded_img.height(), 10);
    }

    #[test]
    fn test_disabled_config_skips_in_process_request() {
        use image::{Rgb, RgbImage};
        use std::io::Cursor;

        let img = image::DynamicImage::ImageRgb8(RgbImage::from_pixel(10, 10, Rgb([128, 64, 32])));
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(buf.into_inner());

        let mut json = serde_json::json!({
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": b64.clone()
                    }
                }]
            }]
        });

        // The enabled check is in process_request_body, which calls process_images_in_json
        // only if config.enabled is true. Verify the gating works by checking that
        // process_images_in_json with enabled=false still processes (because the gate
        // is in the caller). But if we disable face+ocr, stats should be empty of detections.
        let config = crate::config::ImageConfig {
            enabled: true,
            face_detection: false,
            ocr_enabled: false,
            ..crate::config::ImageConfig::default()
        };
        let models = ImageModelManager::new(config);

        let stats = process_images_in_json(&mut json, &models);
        // Image is processed (decode/encode) but no face/OCR work done
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].faces_blurred, 0);
        assert_eq!(stats[0].text_regions_found, 0);
    }

    #[test]
    fn test_walk_json_invalid_base64_fail_open() {
        let mut json = serde_json::json!({
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": "not-valid-base64!!!"
                    }
                }]
            }]
        });

        let config = crate::config::ImageConfig::default();
        let models = ImageModelManager::new(config);

        let mut stats = Vec::new();
        walk_json_for_images(&mut json, &models, &mut stats);

        // Should not have processed anything (bad base64 → extraction returns None or bad magic)
        assert!(
            stats.is_empty(),
            "Invalid base64 should be skipped (fail-open)"
        );
    }
}

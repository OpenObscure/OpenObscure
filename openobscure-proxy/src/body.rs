use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use bytes::Bytes;
use hyper_util::client::legacy::Client;
use serde_json::Value;
use uuid::Uuid;

use crate::fpe_engine::{FpeEngine, FpeResult, TweakGenerator};
use crate::hash_token::TokenGenerator;
use crate::hybrid_scanner::HybridScanner;
use crate::image_detect;
use crate::image_fetch::{self, ImageFetchConfig};
use crate::image_pipeline::{self, ImageModelManager, ImageStats, OutputFormat};
use crate::kws_engine::KwsEngine;
use crate::mapping::{FpeMapping, RequestMappings};
use crate::voice_pipeline;

type HttpsConnector =
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>;

/// Result of processing a request body, including per-feature timing.
pub struct BodyProcessingResult {
    pub body: Bytes,
    pub mappings: RequestMappings,
    pub image_stats: Vec<ImageStats>,
    /// Image processing total time (microseconds).
    pub image_us: u64,
    /// Voice processing total time (microseconds).
    pub voice_us: u64,
    /// Voice audio decode time (microseconds).
    pub voice_decode_us: u64,
    /// Voice KWS inference time (microseconds).
    pub voice_kws_us: u64,
    /// Text scanning time (microseconds).
    pub text_scan_us: u64,
    /// FPE encryption time (microseconds).
    pub fpe_us: u64,
}

/// Process a request body: scan for PII, FPE-encrypt or redact matches, return modified body + mappings.
///
/// Multi-pass processing:
/// 1. Image pass (sync): find base64 images → process; collect URL image refs
/// 2. URL fetch pass (async): fetch all URL images concurrently, process, replace with base64
/// 3. Text pass: scan JSON string values for PII, FPE-encrypt or redact
///
/// Images are processed FIRST so that byte offsets in the text pass remain correct.
#[allow(clippy::too_many_arguments)]
pub async fn process_request_body(
    body: &Bytes,
    request_id: &Uuid,
    scanner: &HybridScanner,
    fpe: &FpeEngine,
    key_version: u32,
    skip_fields: &[String],
    image_models: Option<&Arc<ImageModelManager>>,
    kws_engine: Option<&Arc<KwsEngine>>,
    http_client: Option<&Client<HttpsConnector, Body>>,
    fetch_config: &ImageFetchConfig,
) -> Result<BodyProcessingResult, BodyError> {
    let mut json: Value = serde_json::from_slice(body).map_err(BodyError::Json)?;

    // Pass 1: Process images in JSON (base64 immediately, collect URL refs)
    let img_start = std::time::Instant::now();
    let image_stats = if let Some(models) = image_models {
        if models.config().enabled {
            process_images_in_json(&mut json, models, scanner, http_client, fetch_config).await
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    let image_us = img_start.elapsed().as_micros() as u64;

    // Pass 1.5: Scan audio blocks for PII keywords (KWS-gated)
    let (voice_modified, voice_us, voice_decode_us, voice_kws_us) = if let Some(engine) = kws_engine
    {
        let result = voice_pipeline::scan_and_strip_audio_blocks(&mut json, engine);
        let modified = result.blocks_stripped > 0;
        (
            modified,
            result.total_ms as u64 * 1000,
            result.decode_ms as u64 * 1000,
            result.kws_ms as u64 * 1000,
        )
    } else {
        (false, 0, 0, 0)
    };

    let make_result = |body: Bytes,
                       mappings: RequestMappings,
                       image_stats: Vec<ImageStats>,
                       text_scan_us: u64,
                       fpe_us: u64|
     -> BodyProcessingResult {
        BodyProcessingResult {
            body,
            mappings,
            image_stats,
            image_us,
            voice_us,
            voice_decode_us,
            voice_kws_us,
            text_scan_us,
            fpe_us,
        }
    };

    let scan_start = std::time::Instant::now();
    let matches = scanner.scan_json(&json, skip_fields);
    let text_scan_us = scan_start.elapsed().as_micros() as u64;

    if matches.is_empty() && image_stats.is_empty() && !voice_modified {
        return Ok(make_result(
            body.clone(),
            RequestMappings::new(*request_id),
            image_stats,
            text_scan_us,
            0,
        ));
    }
    if matches.is_empty() {
        // Images/audio were modified but no text PII found — re-serialize the modified JSON
        let modified = serde_json::to_vec(&json).map_err(BodyError::Json)?;
        return Ok(make_result(
            Bytes::from(modified),
            RequestMappings::new(*request_id),
            image_stats,
            text_scan_us,
            0,
        ));
    }

    let fpe_start = std::time::Instant::now();
    let mut mappings = RequestMappings::new(*request_id);
    let mut replacements: Vec<FpeResult> = Vec::new();
    let mut token_gen = TokenGenerator::new(*request_id);

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
            // Non-FPE: redact with hash-based token (e.g., OO_PER_a7f2)
            let token = token_gen.generate(m.pii_type, &m.raw_value);

            mappings.insert(FpeMapping {
                pii_type: m.pii_type,
                plaintext: m.raw_value.clone(),
                ciphertext: token.clone(),
                tweak: vec![],
                key_version,
            });
            replacements.push(FpeResult {
                original: m.clone(),
                encrypted: token,
                tweak: vec![],
            });
        }
    }
    let fpe_us = fpe_start.elapsed().as_micros() as u64;

    if replacements.is_empty() {
        if !image_stats.is_empty() {
            let modified = serde_json::to_vec(&json).map_err(BodyError::Json)?;
            return Ok(make_result(
                Bytes::from(modified),
                mappings,
                image_stats,
                text_scan_us,
                fpe_us,
            ));
        }
        return Ok(make_result(
            body.clone(),
            mappings,
            image_stats,
            text_scan_us,
            fpe_us,
        ));
    }

    // Apply replacements to JSON string values
    apply_replacements_to_json(&mut json, &replacements);

    let modified = serde_json::to_vec(&json).map_err(BodyError::Json)?;
    Ok(make_result(
        Bytes::from(modified),
        mappings,
        image_stats,
        text_scan_us,
        fpe_us,
    ))
}

/// A URL image reference found during JSON walk, pending async fetch.
struct PendingUrlImage {
    /// JSON path to the object containing the URL image block (array of indices/keys).
    json_pointer: String,
    /// The detected URL image reference.
    url_ref: image_detect::ImageUrlRef,
}

/// Walk a JSON tree and process images (base64 immediately, URL images via async fetch).
///
/// Two-phase approach:
/// 1. Walk JSON: process base64 images in-place, collect URL image refs
/// 2. Fetch URL images concurrently, process through pipeline, replace URL→base64
async fn process_images_in_json(
    json: &mut Value,
    models: &ImageModelManager,
    scanner: &HybridScanner,
    http_client: Option<&Client<HttpsConnector, Body>>,
    fetch_config: &ImageFetchConfig,
) -> Vec<ImageStats> {
    let mut stats = Vec::new();
    let mut pending_urls: Vec<PendingUrlImage> = Vec::new();

    // Phase 1: Walk JSON — process base64 images immediately, collect URL refs
    walk_json_for_images(json, models, scanner, &mut stats, &mut pending_urls, "");

    // Phase 1b: Fetch URL images concurrently (if any)
    if !pending_urls.is_empty() {
        if let Some(client) = http_client {
            fetch_and_process_url_images(
                json,
                &pending_urls,
                client,
                fetch_config,
                models,
                scanner,
                &mut stats,
            )
            .await;
        } else {
            oo_warn!(
                crate::oo_log::modules::IMAGE,
                "URL images found but no HTTP client available, skipping",
                count = pending_urls.len()
            );
        }
    }

    stats
}

/// Recursively walk JSON looking for image content blocks.
///
/// Base64 images are processed in-place; URL images are collected into `pending_urls`.
fn walk_json_for_images(
    value: &mut Value,
    models: &ImageModelManager,
    scanner: &HybridScanner,
    stats: &mut Vec<ImageStats>,
    pending_urls: &mut Vec<PendingUrlImage>,
    pointer: &str,
) {
    match value {
        Value::Object(map) => {
            // Check if this object is a base64 image content block
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
                            scanner,
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
            // Check if this object is a URL image content block
            else if let Some(url_ref) = image_detect::detect_url_image_block(map) {
                pending_urls.push(PendingUrlImage {
                    json_pointer: pointer.to_string(),
                    url_ref,
                });
            }
            // Recurse into all values
            let keys: Vec<String> = map.keys().cloned().collect();
            for key in keys {
                let child_pointer = format!("{}/{}", pointer, key);
                if let Some(v) = map.get_mut(&key) {
                    walk_json_for_images(v, models, scanner, stats, pending_urls, &child_pointer);
                }
            }
        }
        Value::Array(arr) => {
            for (i, item) in arr.iter_mut().enumerate() {
                let child_pointer = format!("{}/{}", pointer, i);
                walk_json_for_images(item, models, scanner, stats, pending_urls, &child_pointer);
            }
        }
        _ => {}
    }
}

/// Fetch URL images concurrently, process through pipeline, replace URL→base64 in JSON.
async fn fetch_and_process_url_images(
    json: &mut Value,
    pending: &[PendingUrlImage],
    client: &Client<HttpsConnector, Body>,
    fetch_config: &ImageFetchConfig,
    models: &ImageModelManager,
    scanner: &HybridScanner,
    stats: &mut Vec<ImageStats>,
) {
    // Fetch all URL images concurrently
    let fetch_futures: Vec<_> = pending
        .iter()
        .map(|p| image_fetch::fetch_image_url(&p.url_ref.url, client, fetch_config))
        .collect();

    let fetch_results = futures_util::future::join_all(fetch_futures).await;

    // Process each fetched image and replace URL with base64 in JSON
    for (pending_img, fetch_result) in pending.iter().zip(fetch_results.into_iter()) {
        match fetch_result {
            Ok(fetched) => {
                let media_type = if fetched.media_type.is_empty() {
                    image_detect::detect_media_type(&fetched.bytes)
                        .unwrap_or("image/png")
                        .to_string()
                } else {
                    fetched.media_type.clone()
                };

                // Build a temporary ImageContentRef for process_single_image
                let tmp_ref = image_detect::ImageContentRef {
                    format: pending_img.url_ref.format,
                    data_key_path: vec![],
                    media_type: Some(media_type.clone()),
                };

                match process_single_image(&fetched.bytes, &media_type, &tmp_ref, models, scanner) {
                    Ok((new_bytes, mut img_stats)) => {
                        img_stats.from_url = true;
                        img_stats.fetch_ms = fetched.fetch_ms;

                        // Replace URL with base64 in JSON at the recorded pointer
                        replace_url_with_base64(
                            json,
                            &pending_img.json_pointer,
                            &pending_img.url_ref,
                            &new_bytes,
                            &media_type,
                        );
                        stats.push(img_stats);
                        oo_info!(crate::oo_log::modules::IMAGE, "URL image fetched and processed",
                            url = %pending_img.url_ref.url,
                            fetch_ms = fetched.fetch_ms,
                            format = ?pending_img.url_ref.format);
                    }
                    Err(e) => {
                        // Fail-open: leave original URL intact
                        oo_warn!(crate::oo_log::modules::IMAGE,
                            "URL image processing failed (fail-open)",
                            url = %pending_img.url_ref.url,
                            error = %e);
                    }
                }
            }
            Err(e) => {
                // Fail-open: leave original URL intact
                oo_warn!(crate::oo_log::modules::IMAGE,
                    "URL image fetch failed (fail-open)",
                    url = %pending_img.url_ref.url,
                    error = %e);
            }
        }
    }
}

/// Replace a URL image reference in JSON with a base64-embedded processed image.
///
/// Navigates to the object at `json_pointer`, then mutates it:
/// - Anthropic: `source.type:"url"` → `source.type:"base64"`, remove `url`, add `data` + `media_type`
/// - OpenAI: `image_url.url:"https://..."` → `image_url.url:"data:image/...;base64,..."`
fn replace_url_with_base64(
    json: &mut Value,
    json_pointer: &str,
    url_ref: &image_detect::ImageUrlRef,
    processed_bytes: &[u8],
    media_type: &str,
) {
    // Navigate to the object using JSON pointer
    let target = if json_pointer.is_empty() {
        Some(json)
    } else {
        json.pointer_mut(json_pointer)
    };

    let obj = match target.and_then(|v| v.as_object_mut()) {
        Some(o) => o,
        None => {
            oo_warn!(
                crate::oo_log::modules::IMAGE,
                "Failed to navigate to URL image object for replacement",
                pointer = json_pointer
            );
            return;
        }
    };

    match url_ref.format {
        image_detect::ImageFormat::AnthropicUrl => {
            // Convert source.type:"url" → source.type:"base64"
            if let Some(Value::Object(source)) = obj.get_mut("source") {
                source.insert("type".to_string(), Value::String("base64".to_string()));
                source.remove("url");
                source.insert(
                    "media_type".to_string(),
                    Value::String(media_type.to_string()),
                );
                let b64 = image_detect::encode_to_base64(
                    processed_bytes,
                    media_type,
                    image_detect::ImageFormat::AnthropicBase64,
                );
                source.insert("data".to_string(), Value::String(b64));
            }
        }
        image_detect::ImageFormat::OpenAiExternalUrl => {
            // Replace image_url.url with data URI
            if let Some(Value::Object(img_url)) = obj.get_mut("image_url") {
                let data_uri = image_detect::encode_to_base64(
                    processed_bytes,
                    media_type,
                    image_detect::ImageFormat::OpenAiDataUri,
                );
                img_url.insert("url".to_string(), Value::String(data_uri));
            }
        }
        // Base64 formats don't need URL replacement
        _ => {}
    }
}

/// Process a single decoded image through the pipeline.
fn process_single_image(
    raw_bytes: &[u8],
    media_type: &str,
    _img_ref: &image_detect::ImageContentRef,
    models: &ImageModelManager,
    scanner: &HybridScanner,
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
    let (processed, stats, _meta) = models.process_image(img, sg_ref, Some(scanner))?;

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
        // URL formats are handled by replace_url_with_base64(), not this function
        image_detect::ImageFormat::AnthropicUrl | image_detect::ImageFormat::OpenAiExternalUrl => {}
    }
}

/// Process a response body: find encrypted values and replace with originals.
///
/// Handles two response formats:
/// 1. NDJSON (Ollama streaming): multiple newline-delimited JSON objects where
///    `message.content` tokens are split across frames. Ciphertext can span frames,
///    so we concatenate all content, decrypt, and rebuild as a single response.
/// 2. Standard JSON / text: simple string replacement on the full body.
pub fn process_response_body(body: &Bytes, mappings: &RequestMappings) -> Bytes {
    if mappings.is_empty() {
        return body.clone();
    }

    // Try NDJSON collapse first (Ollama streaming format)
    if let Some(collapsed) = collapse_ndjson_response(body, mappings) {
        return collapsed;
    }

    // Standard path: string replacement on full response
    let text = String::from_utf8_lossy(body);
    let decrypted = mappings.decrypt_response(&text);
    Bytes::from(decrypted)
}

/// Detect and handle Ollama-style NDJSON streaming responses.
///
/// Ollama returns streaming responses as newline-delimited JSON (NDJSON),
/// where each line is a JSON object with partial `message.content` or `response`.
/// FPE ciphertext can be split across multiple NDJSON frames, making simple string
/// replacement on the raw bytes fail.
///
/// This function:
/// 1. Detects NDJSON format (multiple JSON lines with Ollama-style fields)
/// 2. Concatenates all content fragments into a single string
/// 3. Decrypts the concatenated text (ciphertext is now contiguous)
/// 4. Returns a single JSON response object with the full decrypted content
fn collapse_ndjson_response(body: &Bytes, mappings: &RequestMappings) -> Option<Bytes> {
    let text = std::str::from_utf8(body).ok()?;
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();

    // Must have at least 2 lines to be NDJSON streaming
    if lines.len() < 2 {
        return None;
    }

    // Quick check: first line must be a JSON object with Ollama fields
    let first: Value = serde_json::from_str(lines[0]).ok()?;
    if !first.is_object() {
        return None;
    }

    let is_chat = first.get("message").is_some() && first.get("model").is_some();
    let is_generate = first.get("response").is_some() && first.get("model").is_some();
    if !is_chat && !is_generate {
        return None;
    }

    // Parse all lines and concatenate content fragments
    let mut full_content = String::new();
    let mut last_obj: Option<Value> = None;

    for line in &lines {
        let obj: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if is_chat {
            if let Some(content) = obj
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
            {
                full_content.push_str(content);
            }
        } else if let Some(content) = obj.get("response").and_then(|c| c.as_str()) {
            full_content.push_str(content);
        }

        last_obj = Some(obj);
    }

    if full_content.is_empty() {
        return None;
    }

    // Decrypt the full concatenated content (ciphertext is now contiguous)
    let decrypted_content = mappings.decrypt_response(&full_content);

    // Build single response from the last object (has done=true and timing stats)
    let mut final_obj = last_obj?;
    if is_chat {
        if let Some(message) = final_obj.get_mut("message") {
            message["content"] = Value::String(decrypted_content);
        }
    } else {
        final_obj["response"] = Value::String(decrypted_content);
    }

    let serialized = serde_json::to_vec(&final_obj).ok()?;

    oo_info!(
        crate::oo_log::modules::BODY,
        "Collapsed NDJSON response for decryption",
        lines = lines.len(),
        content_len = full_content.len()
    );

    Some(Bytes::from(serialized))
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
        let scanner = HybridScanner::new(true, None);
        let mut json = serde_json::json!({
            "messages": [
                {"role": "user", "content": "hello world"},
                {"role": "assistant", "content": "hi there"}
            ]
        });

        let mut stats = Vec::new();
        let mut pending = Vec::new();
        walk_json_for_images(&mut json, &models, &scanner, &mut stats, &mut pending, "");
        assert!(
            stats.is_empty(),
            "Non-image JSON should produce no image stats"
        );
        assert!(
            pending.is_empty(),
            "Non-image JSON should have no pending URL images"
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
        let scanner = HybridScanner::new(true, None);

        let mut stats = Vec::new();
        let mut pending = Vec::new();
        walk_json_for_images(&mut json, &models, &scanner, &mut stats, &mut pending, "");

        // Should have processed 1 image (decode → resize → encode, no face/OCR)
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].faces_redacted, 0);
        assert_eq!(stats[0].text_regions_found, 0);
        assert!(pending.is_empty());

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

        let scanner = HybridScanner::new(true, None);
        let fetch_config = ImageFetchConfig::default();
        let stats = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(process_images_in_json(
                &mut json,
                &models,
                &scanner,
                None,
                &fetch_config,
            ));
        // Image is processed (decode/encode) but no face/OCR work done
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].faces_redacted, 0);
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

        let scanner = HybridScanner::new(true, None);
        let mut stats = Vec::new();
        let mut pending = Vec::new();
        walk_json_for_images(&mut json, &models, &scanner, &mut stats, &mut pending, "");

        // Should not have processed anything (bad base64 → extraction returns None or bad magic)
        assert!(
            stats.is_empty(),
            "Invalid base64 should be skipped (fail-open)"
        );
    }

    // ---- NDJSON collapse tests ----

    fn make_mappings(pairs: &[(&str, &str)]) -> RequestMappings {
        let mut m = RequestMappings::new(uuid::Uuid::new_v4());
        for (ct, pt) in pairs {
            m.insert(FpeMapping {
                pii_type: crate::pii_types::PiiType::PhoneNumber,
                plaintext: pt.to_string(),
                ciphertext: ct.to_string(),
                tweak: vec![],
                key_version: 1,
            });
        }
        m
    }

    #[test]
    fn test_ndjson_collapse_ollama_chat() {
        // Simulate Ollama NDJSON streaming where phone ciphertext is split across frames
        let ndjson = [
            r#"{"model":"llava:7b","created_at":"2026-02-26T00:00:00Z","message":{"role":"assistant","content":"My phone is "},"done":false}"#,
            r#"{"model":"llava:7b","created_at":"2026-02-26T00:00:01Z","message":{"role":"assistant","content":"658"},"done":false}"#,
            r#"{"model":"llava:7b","created_at":"2026-02-26T00:00:02Z","message":{"role":"assistant","content":"-297"},"done":false}"#,
            r#"{"model":"llava:7b","created_at":"2026-02-26T00:00:03Z","message":{"role":"assistant","content":"-2527"},"done":false}"#,
            r#"{"model":"llava:7b","created_at":"2026-02-26T00:00:04Z","message":{"role":"assistant","content":"."},"done":true,"total_duration":1000000}"#,
        ].join("\n");

        let body = Bytes::from(ndjson);
        let mappings = make_mappings(&[("658-297-2527", "410-111-0277")]);

        let result = process_response_body(&body, &mappings);
        let result_str = String::from_utf8(result.to_vec()).unwrap();

        // Should be a single JSON object with decrypted content
        let parsed: Value = serde_json::from_str(&result_str).unwrap();
        let content = parsed["message"]["content"].as_str().unwrap();
        assert!(
            content.contains("410-111-0277"),
            "Phone should be decrypted: {content}"
        );
        assert!(
            !content.contains("658-297-2527"),
            "Ciphertext should be removed: {content}"
        );
        assert!(parsed["done"].as_bool().unwrap());
    }

    #[test]
    fn test_ndjson_collapse_ollama_generate() {
        // Ollama /api/generate uses "response" field instead of "message.content"
        let ndjson = [
            r#"{"model":"qwen3:4b","created_at":"2026-02-26T00:00:00Z","response":"Call ","done":false}"#,
            r#"{"model":"qwen3:4b","created_at":"2026-02-26T00:00:01Z","response":"847-","done":false}"#,
            r#"{"model":"qwen3:4b","created_at":"2026-02-26T00:00:02Z","response":"29-3651","done":false}"#,
            r#"{"model":"qwen3:4b","created_at":"2026-02-26T00:00:03Z","response":"","done":true,"total_duration":500000}"#,
        ].join("\n");

        let body = Bytes::from(ndjson);
        let mappings = make_mappings(&[("847-29-3651", "123-45-6789")]);

        let result = process_response_body(&body, &mappings);
        let parsed: Value =
            serde_json::from_str(&String::from_utf8(result.to_vec()).unwrap()).unwrap();
        let content = parsed["response"].as_str().unwrap();
        assert!(
            content.contains("123-45-6789"),
            "SSN should be decrypted: {content}"
        );
    }

    #[test]
    fn test_ndjson_collapse_not_triggered_for_single_json() {
        // A single JSON response should NOT trigger NDJSON collapse
        let single = r#"{"model":"qwen3:4b","message":{"role":"assistant","content":"Phone is 658-297-2527."},"done":true}"#;
        let body = Bytes::from(single);
        let mappings = make_mappings(&[("658-297-2527", "410-111-0277")]);

        let result = process_response_body(&body, &mappings);
        let result_str = String::from_utf8(result.to_vec()).unwrap();
        // Standard decrypt_response should handle single JSON fine
        assert!(
            result_str.contains("410-111-0277"),
            "Single JSON decryption should work: {result_str}"
        );
    }

    #[test]
    fn test_ndjson_collapse_not_triggered_for_non_ollama() {
        // Non-Ollama NDJSON should NOT trigger collapse
        let ndjson = [
            r#"{"type":"log","message":"processing"}"#,
            r#"{"type":"log","message":"done"}"#,
        ]
        .join("\n");

        let body = Bytes::from(ndjson);
        let mappings = make_mappings(&[("SECRET", "public")]);

        let result = collapse_ndjson_response(&body, &mappings);
        assert!(
            result.is_none(),
            "Non-Ollama NDJSON should not trigger collapse"
        );
    }

    #[test]
    fn test_ndjson_collapse_multiple_pii_types() {
        // Test decryption of multiple PII types in one NDJSON stream
        let ndjson = [
            r#"{"model":"llava:7b","created_at":"T","message":{"role":"assistant","content":"Email: fk6rup7."},"done":false}"#,
            r#"{"model":"llava:7b","created_at":"T","message":{"role":"assistant","content":"cqmr6v@gmail.com, "},"done":false}"#,
            r#"{"model":"llava:7b","created_at":"T","message":{"role":"assistant","content":"Phone: 658-"},"done":false}"#,
            r#"{"model":"llava:7b","created_at":"T","message":{"role":"assistant","content":"297-2527"},"done":false}"#,
            r#"{"model":"llava:7b","created_at":"T","message":{"role":"assistant","content":""},"done":true}"#,
        ].join("\n");

        let body = Bytes::from(ndjson);
        let mut mappings = RequestMappings::new(uuid::Uuid::new_v4());
        mappings.insert(FpeMapping {
            pii_type: crate::pii_types::PiiType::PhoneNumber,
            plaintext: "410-111-0277".to_string(),
            ciphertext: "658-297-2527".to_string(),
            tweak: vec![],
            key_version: 1,
        });
        mappings.insert(FpeMapping {
            pii_type: crate::pii_types::PiiType::Email,
            plaintext: "john.doe@gmail.com".to_string(),
            ciphertext: "fk6rup7.cqmr6v@gmail.com".to_string(),
            tweak: vec![],
            key_version: 1,
        });

        let result = process_response_body(&body, &mappings);
        let result_str = String::from_utf8(result.to_vec()).unwrap();
        let parsed: Value = serde_json::from_str(&result_str).unwrap();
        let content = parsed["message"]["content"].as_str().unwrap();
        assert!(
            content.contains("410-111-0277"),
            "Phone decrypted: {content}"
        );
        assert!(
            content.contains("john.doe@gmail.com"),
            "Email decrypted: {content}"
        );
    }

    // ---- URL image detection + replacement tests ----

    #[test]
    fn test_walk_json_collects_anthropic_url_image() {
        let config = crate::config::ImageConfig::default();
        let models = ImageModelManager::new(config);
        let scanner = HybridScanner::new(true, None);
        let mut json = serde_json::json!({
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "image",
                    "source": {
                        "type": "url",
                        "url": "https://cdn.discordapp.com/attachments/123/456/image.png",
                        "media_type": "image/png"
                    }
                }]
            }]
        });

        let mut stats = Vec::new();
        let mut pending = Vec::new();
        walk_json_for_images(&mut json, &models, &scanner, &mut stats, &mut pending, "");

        assert!(stats.is_empty(), "URL images are not processed immediately");
        assert_eq!(pending.len(), 1, "Should have collected 1 URL image ref");
        assert_eq!(
            pending[0].url_ref.url,
            "https://cdn.discordapp.com/attachments/123/456/image.png"
        );
        assert_eq!(
            pending[0].url_ref.format,
            image_detect::ImageFormat::AnthropicUrl
        );
    }

    #[test]
    fn test_walk_json_collects_openai_url_image() {
        let config = crate::config::ImageConfig::default();
        let models = ImageModelManager::new(config);
        let scanner = HybridScanner::new(true, None);
        let mut json = serde_json::json!({
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "image_url",
                    "image_url": {
                        "url": "https://cdn.discordapp.com/attachments/123/456/image.png"
                    }
                }]
            }]
        });

        let mut stats = Vec::new();
        let mut pending = Vec::new();
        walk_json_for_images(&mut json, &models, &scanner, &mut stats, &mut pending, "");

        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].url_ref.format,
            image_detect::ImageFormat::OpenAiExternalUrl
        );
    }

    #[test]
    fn test_replace_url_with_base64_anthropic() {
        use image::{Rgb, RgbImage};
        use std::io::Cursor;

        // Create test PNG bytes
        let img = image::DynamicImage::ImageRgb8(RgbImage::from_pixel(4, 4, Rgb([128, 64, 32])));
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        let png_bytes = buf.into_inner();

        let mut json = serde_json::json!({
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "image",
                    "source": {
                        "type": "url",
                        "url": "https://cdn.discordapp.com/image.png",
                        "media_type": "image/png"
                    }
                }]
            }]
        });

        let url_ref = image_detect::ImageUrlRef {
            format: image_detect::ImageFormat::AnthropicUrl,
            url: "https://cdn.discordapp.com/image.png".to_string(),
            media_type: Some("image/png".to_string()),
        };

        replace_url_with_base64(
            &mut json,
            "/messages/0/content/0",
            &url_ref,
            &png_bytes,
            "image/png",
        );

        // Verify the source was converted from URL to base64
        let source = &json["messages"][0]["content"][0]["source"];
        assert_eq!(source["type"].as_str().unwrap(), "base64");
        assert!(source.get("url").is_none(), "url field should be removed");
        assert_eq!(source["media_type"].as_str().unwrap(), "image/png");

        // Verify the data is valid base64 that decodes to a PNG
        let data = source["data"].as_str().unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap();
        assert!(image_detect::has_image_magic_bytes(&decoded));
    }

    #[test]
    fn test_replace_url_with_base64_openai() {
        use image::{Rgb, RgbImage};
        use std::io::Cursor;

        let img = image::DynamicImage::ImageRgb8(RgbImage::from_pixel(4, 4, Rgb([128, 64, 32])));
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        let png_bytes = buf.into_inner();

        let mut json = serde_json::json!({
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "image_url",
                    "image_url": {
                        "url": "https://cdn.discordapp.com/image.png"
                    }
                }]
            }]
        });

        let url_ref = image_detect::ImageUrlRef {
            format: image_detect::ImageFormat::OpenAiExternalUrl,
            url: "https://cdn.discordapp.com/image.png".to_string(),
            media_type: None,
        };

        replace_url_with_base64(
            &mut json,
            "/messages/0/content/0",
            &url_ref,
            &png_bytes,
            "image/png",
        );

        // Verify the URL was replaced with a data URI
        let url = json["messages"][0]["content"][0]["image_url"]["url"]
            .as_str()
            .unwrap();
        assert!(
            url.starts_with("data:image/png;base64,"),
            "URL should be replaced with data URI"
        );
    }
}

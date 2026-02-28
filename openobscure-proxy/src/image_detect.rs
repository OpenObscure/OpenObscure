//! Detection of images within JSON request bodies (base64 and URL).
//!
//! LLM providers embed images in JSON in two ways:
//! - **Base64**: Anthropic `source.type:"base64"`, OpenAI `data:image/...;base64,...`
//! - **URL**: Anthropic `source.type:"url"`, OpenAI `image_url.url:"https://..."`
//!
//! This module detects both patterns. Base64 images are extracted directly;
//! URL images are collected for async fetching by the caller.

use base64::Engine;
use serde_json::Value;

/// Detected image within a JSON content block.
#[derive(Debug)]
pub struct DetectedImage {
    /// The provider format this image was found in.
    pub format: ImageFormat,
    /// Raw image bytes after base64 decoding.
    pub raw_bytes: Vec<u8>,
    /// MIME type (image/png, image/jpeg, image/webp, image/gif).
    pub media_type: String,
}

/// Which LLM provider format the image was encoded in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    /// Anthropic: base64 data in "source.data" field.
    AnthropicBase64,
    /// OpenAI: data URI in "image_url.url" field.
    OpenAiDataUri,
    /// Anthropic: URL reference in "source.url" field.
    AnthropicUrl,
    /// OpenAI: external URL in "image_url.url" field (not a data URI).
    OpenAiExternalUrl,
}

/// Reference to an image content block's location in the JSON tree.
#[derive(Debug)]
pub struct ImageContentRef {
    pub format: ImageFormat,
    /// The JSON key path to the base64 data string (relative to this object).
    /// Anthropic: "source.data", OpenAI: "image_url.url"
    pub data_key_path: Vec<String>,
    /// The media type (from the JSON structure, not detected from bytes).
    pub media_type: Option<String>,
}

/// Check if a JSON object represents an image content block.
///
/// Detects Anthropic and OpenAI image content block structures.
/// Returns `None` for non-image objects (fast path — most objects are not images).
pub fn is_image_content_block(obj: &serde_json::Map<String, Value>) -> Option<ImageContentRef> {
    // Check the "type" field first — fast rejection for non-image objects
    let type_val = obj.get("type")?.as_str()?;

    match type_val {
        // Anthropic: {"type": "image", "source": {"type": "base64", "media_type": "...", "data": "..."}}
        "image" => {
            let source = obj.get("source")?.as_object()?;
            let source_type = source.get("type")?.as_str()?;
            if source_type != "base64" {
                return None; // URL-based images handled by detect_url_image_block()
            }
            let media_type = source
                .get("media_type")
                .and_then(|v| v.as_str())
                .map(String::from);
            // Verify "data" field exists and is a string
            source.get("data")?.as_str()?;
            Some(ImageContentRef {
                format: ImageFormat::AnthropicBase64,
                data_key_path: vec!["source".to_string(), "data".to_string()],
                media_type,
            })
        }
        // OpenAI: {"type": "image_url", "image_url": {"url": "data:image/...;base64,..."}}
        "image_url" => {
            let image_url = obj.get("image_url")?.as_object()?;
            let url = image_url.get("url")?.as_str()?;
            if !url.starts_with("data:image/") {
                return None; // External URL images handled by detect_url_image_block()
            }
            // Extract media type from data URI: "data:image/png;base64,..."
            let media_type = url
                .strip_prefix("data:")
                .and_then(|s| s.split(';').next())
                .map(String::from);
            Some(ImageContentRef {
                format: ImageFormat::OpenAiDataUri,
                data_key_path: vec!["image_url".to_string(), "url".to_string()],
                media_type,
            })
        }
        _ => None,
    }
}

/// Reference to a URL-based image in a JSON content block.
#[derive(Debug, Clone)]
pub struct ImageUrlRef {
    pub format: ImageFormat,
    /// The external URL to fetch.
    pub url: String,
    /// The media type hint from the JSON structure (may be None).
    pub media_type: Option<String>,
}

/// Check if a JSON object is a URL-based image content block.
///
/// Complement to `is_image_content_block` — detects Anthropic `source.type:"url"`
/// and OpenAI external `image_url.url:"https://..."` blocks.
/// Returns `None` for base64 images or non-image objects.
pub fn detect_url_image_block(obj: &serde_json::Map<String, Value>) -> Option<ImageUrlRef> {
    let type_val = obj.get("type")?.as_str()?;

    match type_val {
        // Anthropic: {"type": "image", "source": {"type": "url", "url": "https://..."}}
        "image" => {
            let source = obj.get("source")?.as_object()?;
            let source_type = source.get("type")?.as_str()?;
            if source_type != "url" {
                return None; // base64 or unknown, not a URL image
            }
            let url = source.get("url")?.as_str()?;
            let media_type = source
                .get("media_type")
                .and_then(|v| v.as_str())
                .map(String::from);
            Some(ImageUrlRef {
                format: ImageFormat::AnthropicUrl,
                url: url.to_string(),
                media_type,
            })
        }
        // OpenAI: {"type": "image_url", "image_url": {"url": "https://..."}}
        "image_url" => {
            let image_url = obj.get("image_url")?.as_object()?;
            let url = image_url.get("url")?.as_str()?;
            if url.starts_with("data:image/") {
                return None; // data URI, not an external URL
            }
            Some(ImageUrlRef {
                format: ImageFormat::OpenAiExternalUrl,
                url: url.to_string(),
                media_type: None, // inferred from response Content-Type or magic bytes
            })
        }
        _ => None,
    }
}

/// Extract raw image bytes from the base64 data in a detected image content block.
///
/// For Anthropic: decodes the "source.data" field directly.
/// For OpenAI: strips the "data:image/...;base64," prefix, then decodes.
pub fn extract_image_bytes(
    obj: &serde_json::Map<String, Value>,
    image_ref: &ImageContentRef,
) -> Option<DetectedImage> {
    let data_str = navigate_to_str(obj, &image_ref.data_key_path)?;

    let (base64_data, media_type) = match image_ref.format {
        ImageFormat::AnthropicBase64 => {
            let media = image_ref
                .media_type
                .clone()
                .unwrap_or_else(|| "image/png".to_string());
            (data_str, media)
        }
        ImageFormat::OpenAiDataUri => {
            // Parse "data:image/png;base64,iVBOR..."
            let after_comma = data_str.split_once(',')?.1;
            let media = image_ref
                .media_type
                .clone()
                .unwrap_or_else(|| "image/png".to_string());
            (after_comma, media)
        }
        // URL formats don't have base64 data — caller should use fetch instead
        ImageFormat::AnthropicUrl | ImageFormat::OpenAiExternalUrl => return None,
    };

    let raw_bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_data)
        .ok()?;

    // Verify this looks like an image by checking magic bytes
    if !has_image_magic_bytes(&raw_bytes) {
        return None;
    }

    Some(DetectedImage {
        format: image_ref.format,
        raw_bytes,
        media_type,
    })
}

/// Navigate a JSON object by a key path to get a string value.
fn navigate_to_str<'a>(
    obj: &'a serde_json::Map<String, Value>,
    keys: &[String],
) -> Option<&'a str> {
    if keys.is_empty() {
        return None;
    }
    let mut current: &'a Value = obj.get(&keys[0])?;
    for key in &keys[1..] {
        current = current.get(key)?;
    }
    current.as_str()
}

/// Check if raw bytes start with known image format magic bytes.
pub fn has_image_magic_bytes(bytes: &[u8]) -> bool {
    if bytes.len() < 4 {
        return false;
    }
    // PNG: 89 50 4E 47
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        return true;
    }
    // JPEG: FF D8 FF
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return true;
    }
    // GIF: GIF87a or GIF89a
    if bytes.starts_with(b"GIF8") {
        return true;
    }
    // WebP: RIFF....WEBP
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return true;
    }
    false
}

/// Detect the image media type from raw bytes (magic byte sniffing).
pub fn detect_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() < 4 {
        return None;
    }
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF8") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

/// Encode raw image bytes to base64 string suitable for the given format.
///
/// For Anthropic: returns plain base64.
/// For OpenAI: returns `data:{media_type};base64,{data}`.
pub fn encode_to_base64(bytes: &[u8], media_type: &str, format: ImageFormat) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    match format {
        ImageFormat::AnthropicBase64 | ImageFormat::AnthropicUrl => b64,
        ImageFormat::OpenAiDataUri | ImageFormat::OpenAiExternalUrl => {
            format!("data:{};base64,{}", media_type, b64)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Minimal 1x1 red PNG (67 bytes)
    fn tiny_png_bytes() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(1, 1, image::Rgb([255, 0, 0]));
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        buf
    }

    fn tiny_png_base64() -> String {
        base64::engine::general_purpose::STANDARD.encode(tiny_png_bytes())
    }

    #[test]
    fn test_detect_anthropic_image_block() {
        let obj = json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": "image/png",
                "data": tiny_png_base64()
            }
        });
        let map = obj.as_object().unwrap();
        let result = is_image_content_block(map);
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.format, ImageFormat::AnthropicBase64);
        assert_eq!(r.media_type.as_deref(), Some("image/png"));
        assert_eq!(r.data_key_path, vec!["source", "data"]);
    }

    #[test]
    fn test_detect_openai_image_block() {
        let data_uri = format!("data:image/png;base64,{}", tiny_png_base64());
        let obj = json!({
            "type": "image_url",
            "image_url": {"url": data_uri}
        });
        let map = obj.as_object().unwrap();
        let result = is_image_content_block(map);
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.format, ImageFormat::OpenAiDataUri);
        assert_eq!(r.media_type.as_deref(), Some("image/png"));
    }

    #[test]
    fn test_reject_non_image_object() {
        let obj = json!({"type": "text", "text": "hello world"});
        assert!(is_image_content_block(obj.as_object().unwrap()).is_none());
    }

    #[test]
    fn test_base64_detector_skips_url_image() {
        let obj = json!({
            "type": "image",
            "source": {"type": "url", "url": "https://example.com/image.png"}
        });
        // is_image_content_block should return None for URL images
        assert!(is_image_content_block(obj.as_object().unwrap()).is_none());
    }

    #[test]
    fn test_base64_detector_skips_external_url_openai() {
        let obj = json!({
            "type": "image_url",
            "image_url": {"url": "https://example.com/image.png"}
        });
        assert!(is_image_content_block(obj.as_object().unwrap()).is_none());
    }

    // --- URL image detection tests ---

    #[test]
    fn test_detect_anthropic_url_image() {
        let obj = json!({
            "type": "image",
            "source": {
                "type": "url",
                "url": "https://cdn.discordapp.com/attachments/123/456/image.png",
                "media_type": "image/png"
            }
        });
        let result = detect_url_image_block(obj.as_object().unwrap());
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.format, ImageFormat::AnthropicUrl);
        assert_eq!(
            r.url,
            "https://cdn.discordapp.com/attachments/123/456/image.png"
        );
        assert_eq!(r.media_type.as_deref(), Some("image/png"));
    }

    #[test]
    fn test_detect_openai_external_url() {
        let obj = json!({
            "type": "image_url",
            "image_url": {"url": "https://cdn.discordapp.com/attachments/123/456/image.png"}
        });
        let result = detect_url_image_block(obj.as_object().unwrap());
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.format, ImageFormat::OpenAiExternalUrl);
        assert_eq!(
            r.url,
            "https://cdn.discordapp.com/attachments/123/456/image.png"
        );
        assert!(r.media_type.is_none());
    }

    #[test]
    fn test_url_detector_skips_base64() {
        // Anthropic base64 should not be detected as URL
        let obj = json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": "image/png",
                "data": tiny_png_base64()
            }
        });
        assert!(detect_url_image_block(obj.as_object().unwrap()).is_none());
    }

    #[test]
    fn test_url_detector_skips_data_uri() {
        // OpenAI data URI should not be detected as URL
        let data_uri = format!("data:image/png;base64,{}", tiny_png_base64());
        let obj = json!({
            "type": "image_url",
            "image_url": {"url": data_uri}
        });
        assert!(detect_url_image_block(obj.as_object().unwrap()).is_none());
    }

    #[test]
    fn test_url_detector_skips_text() {
        let obj = json!({"type": "text", "text": "hello"});
        assert!(detect_url_image_block(obj.as_object().unwrap()).is_none());
    }

    #[test]
    fn test_extract_anthropic_image_bytes() {
        let png_b64 = tiny_png_base64();
        let obj = json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": "image/png",
                "data": png_b64
            }
        });
        let map = obj.as_object().unwrap();
        let image_ref = is_image_content_block(map).unwrap();
        let detected = extract_image_bytes(map, &image_ref).unwrap();
        assert_eq!(detected.format, ImageFormat::AnthropicBase64);
        assert_eq!(detected.media_type, "image/png");
        assert!(detected.raw_bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]));
    }

    #[test]
    fn test_extract_openai_image_bytes() {
        let data_uri = format!("data:image/png;base64,{}", tiny_png_base64());
        let obj = json!({
            "type": "image_url",
            "image_url": {"url": data_uri}
        });
        let map = obj.as_object().unwrap();
        let image_ref = is_image_content_block(map).unwrap();
        let detected = extract_image_bytes(map, &image_ref).unwrap();
        assert_eq!(detected.format, ImageFormat::OpenAiDataUri);
        assert!(detected.raw_bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]));
    }

    #[test]
    fn test_reject_invalid_base64() {
        let obj = json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": "image/png",
                "data": "not-valid-base64!!!"
            }
        });
        let map = obj.as_object().unwrap();
        let image_ref = is_image_content_block(map).unwrap();
        assert!(extract_image_bytes(map, &image_ref).is_none());
    }

    #[test]
    fn test_reject_non_image_base64() {
        let text_b64 = base64::engine::general_purpose::STANDARD.encode(b"hello world");
        let obj = json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": "image/png",
                "data": text_b64
            }
        });
        let map = obj.as_object().unwrap();
        let image_ref = is_image_content_block(map).unwrap();
        assert!(extract_image_bytes(map, &image_ref).is_none());
    }

    #[test]
    fn test_magic_bytes_png() {
        assert!(has_image_magic_bytes(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A]));
    }

    #[test]
    fn test_magic_bytes_jpeg() {
        assert!(has_image_magic_bytes(&[0xFF, 0xD8, 0xFF, 0xE0]));
    }

    #[test]
    fn test_magic_bytes_gif() {
        assert!(has_image_magic_bytes(b"GIF89a"));
    }

    #[test]
    fn test_magic_bytes_webp() {
        assert!(has_image_magic_bytes(b"RIFF\x00\x00\x00\x00WEBP"));
    }

    #[test]
    fn test_magic_bytes_rejects_text() {
        assert!(!has_image_magic_bytes(b"hello world"));
    }

    #[test]
    fn test_encode_anthropic_base64() {
        let bytes = tiny_png_bytes();
        let encoded = encode_to_base64(&bytes, "image/png", ImageFormat::AnthropicBase64);
        assert!(!encoded.starts_with("data:"));
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&encoded)
            .unwrap();
        assert_eq!(decoded, bytes);
    }

    #[test]
    fn test_encode_openai_data_uri() {
        let bytes = tiny_png_bytes();
        let encoded = encode_to_base64(&bytes, "image/png", ImageFormat::OpenAiDataUri);
        assert!(encoded.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn test_detect_media_type() {
        assert_eq!(
            detect_media_type(&[0x89, 0x50, 0x4E, 0x47]),
            Some("image/png")
        );
        assert_eq!(
            detect_media_type(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some("image/jpeg")
        );
        assert_eq!(detect_media_type(b"GIF89a"), Some("image/gif"));
        assert_eq!(
            detect_media_type(b"RIFF\x00\x00\x00\x00WEBP"),
            Some("image/webp")
        );
        assert_eq!(detect_media_type(b"hello"), None);
    }
}

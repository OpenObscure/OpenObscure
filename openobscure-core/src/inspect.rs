//! Inspect mode — developer/demo tool for viewing proxy data flow.
//!
//! When `--inspect` is active:
//! - Console: prints incoming text, PII matches, redacted text, and response text
//! - Files: saves original + processed images/audio to ~/.openobscure/inspect/

use std::path::PathBuf;
use uuid::Uuid;

use crate::mapping::RequestMappings;

/// Resolve the inspect output directory (~/.openobscure/inspect/).
pub fn inspect_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(std::path::Path::new(&home).join(".openobscure/inspect"))
}

/// Print the incoming request text content to console.
pub fn print_incoming_text(request_id: &Uuid, body: &[u8]) {
    let json: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => {
            eprintln!(
                "[INSPECT {request_id}] (non-JSON body, {} bytes)",
                body.len()
            );
            return;
        }
    };

    let texts = extract_message_texts(&json);
    if texts.is_empty() {
        eprintln!("[INSPECT {request_id}] (no text content in request)");
        return;
    }

    eprintln!("\n[INSPECT {request_id}] ──── INCOMING TEXT ────");
    for text in &texts {
        eprintln!("{text}");
    }
    eprintln!("[INSPECT {request_id}] ────────────────────────\n");
}

/// Print the redacted/encrypted text content and PII matches.
pub fn print_redacted_text(request_id: &Uuid, body: &[u8], mappings: &RequestMappings) {
    // Print PII matches
    if !mappings.is_empty() {
        eprintln!("[INSPECT {request_id}] ──── PII MATCHES ────");
        for mapping in mappings.by_ciphertext.values() {
            eprintln!(
                "  {:14} | {} -> {}",
                format!("{:?}", mapping.pii_type),
                mapping.plaintext,
                mapping.ciphertext
            );
        }
        eprintln!("[INSPECT {request_id}] ─────────────────────\n");
    }

    let json: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return,
    };

    let texts = extract_message_texts(&json);
    if !texts.is_empty() {
        eprintln!("[INSPECT {request_id}] ──── REDACTED TEXT ────");
        for text in &texts {
            eprintln!("{text}");
        }
        eprintln!("[INSPECT {request_id}] ──────────────────────\n");
    }
}

/// Print the decrypted response text.
pub fn print_response_text(request_id: &Uuid, body: &[u8]) {
    let format = crate::response_format::detect(None, body);
    if let Some(extracted) = crate::response_format::extract_text(body, format) {
        eprintln!("[INSPECT {request_id}] ──── RESPONSE TEXT ────");
        // Truncate very long responses for console readability
        if extracted.len() > 4000 {
            eprintln!(
                "{}...\n(truncated, {} total chars)",
                &extracted[..4000],
                extracted.len()
            );
        } else {
            eprintln!("{extracted}");
        }
        eprintln!("[INSPECT {request_id}] ────────────────────────\n");
        return;
    }

    // Fallback: try raw text
    if let Ok(text) = std::str::from_utf8(body) {
        if !text.is_empty() && text.len() < 4000 {
            eprintln!("[INSPECT {request_id}] ──── RESPONSE TEXT ────");
            eprintln!("{text}");
            eprintln!("[INSPECT {request_id}] ────────────────────────\n");
        }
    }
}

/// Print SSE response text as it accumulates (called once at stream completion).
pub fn print_sse_response_text(request_id: &Uuid, text: &str) {
    if !text.is_empty() {
        eprintln!("[INSPECT {request_id}] ──── SSE RESPONSE TEXT ────");
        if text.len() > 4000 {
            eprintln!(
                "{}...\n(truncated, {} total chars)",
                &text[..4000],
                text.len()
            );
        } else {
            eprintln!("{text}");
        }
        eprintln!("[INSPECT {request_id}] ──────────────────────────\n");
    }
}

/// Save original and processed image bytes to inspect directory.
pub fn save_image(
    request_id: &Uuid,
    index: usize,
    original_bytes: &[u8],
    processed_bytes: &[u8],
    media_type: &str,
) {
    let dir = match inspect_dir() {
        Some(d) => d,
        None => return,
    };
    let _ = std::fs::create_dir_all(&dir);

    let ext = media_type_to_ext(media_type);
    let in_path = dir.join(format!("in_{request_id}_{index}.{ext}"));
    let out_path = dir.join(format!("out_{request_id}_{index}.{ext}"));

    if let Err(e) = std::fs::write(&in_path, original_bytes) {
        eprintln!("[INSPECT] Failed to write {}: {e}", in_path.display());
    } else {
        eprintln!(
            "[INSPECT {request_id}] Saved input image: {} ({} bytes)",
            in_path.display(),
            original_bytes.len()
        );
    }
    if let Err(e) = std::fs::write(&out_path, processed_bytes) {
        eprintln!("[INSPECT] Failed to write {}: {e}", out_path.display());
    } else {
        eprintln!(
            "[INSPECT {request_id}] Saved output image: {} ({} bytes)",
            out_path.display(),
            processed_bytes.len()
        );
    }
}

/// Save original audio data to inspect directory.
///
/// For the output: if PII was detected, saves the notice text; otherwise saves a passthrough marker.
pub fn save_audio(
    request_id: &Uuid,
    index: usize,
    original_base64: &str,
    output_notice: Option<&str>,
    media_type: &str,
) {
    let dir = match inspect_dir() {
        Some(d) => d,
        None => return,
    };
    let _ = std::fs::create_dir_all(&dir);

    let ext = audio_media_type_to_ext(media_type);

    // Save original audio (decode base64)
    use base64::Engine;
    if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(original_base64) {
        let in_path = dir.join(format!("in_{request_id}_{index}.{ext}"));
        if let Err(e) = std::fs::write(&in_path, &decoded) {
            eprintln!("[INSPECT] Failed to write {}: {e}", in_path.display());
        } else {
            eprintln!(
                "[INSPECT {request_id}] Saved input audio: {} ({} bytes)",
                in_path.display(),
                decoded.len()
            );
        }
    }

    // Save output notice or passthrough marker
    let out_path = dir.join(format!("out_{request_id}_{index}.txt"));
    if let Some(notice) = output_notice {
        let _ = std::fs::write(&out_path, notice);
        eprintln!(
            "[INSPECT {request_id}] Audio PII stripped, notice saved: {}",
            out_path.display()
        );
    } else {
        let _ = std::fs::write(
            &out_path,
            "[PASSTHROUGH — no PII detected, audio unchanged]",
        );
        eprintln!("[INSPECT {request_id}] Audio passed through (no PII)");
    }
}

/// Extract text content from LLM request/response JSON (messages[].content).
fn extract_message_texts(json: &serde_json::Value) -> Vec<String> {
    let mut texts = Vec::new();

    // OpenAI/Anthropic/Gemini: messages[].content
    if let Some(messages) = json.get("messages").and_then(|m| m.as_array()) {
        for msg in messages {
            // String content: {"role": "...", "content": "text"}
            if let Some(text) = msg.get("content").and_then(|c| c.as_str()) {
                texts.push(text.to_string());
            }
            // Array content: {"role": "...", "content": [{"type":"text","text":"..."}]}
            if let Some(blocks) = msg.get("content").and_then(|c| c.as_array()) {
                for block in blocks {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        texts.push(text.to_string());
                    }
                }
            }
        }
    }

    // Ollama generate: {"prompt": "..."}
    if let Some(prompt) = json.get("prompt").and_then(|p| p.as_str()) {
        texts.push(prompt.to_string());
    }

    texts
}

fn media_type_to_ext(media_type: &str) -> &str {
    match media_type {
        t if t.contains("png") => "png",
        t if t.contains("gif") => "gif",
        t if t.contains("webp") => "webp",
        _ => "jpg",
    }
}

fn audio_media_type_to_ext(media_type: &str) -> &str {
    match media_type {
        t if t.contains("wav") || t.contains("wave") => "wav",
        t if t.contains("mp3") || t.contains("mpeg") => "mp3",
        t if t.contains("ogg") => "ogg",
        t if t.contains("webm") => "webm",
        _ => "bin",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_message_texts_string_content() {
        let json = serde_json::json!({
            "messages": [
                {"role": "user", "content": "Hello, my SSN is 123-45-6789"}
            ]
        });
        let texts = extract_message_texts(&json);
        assert_eq!(texts.len(), 1);
        assert!(texts[0].contains("123-45-6789"));
    }

    #[test]
    fn test_extract_message_texts_array_content() {
        let json = serde_json::json!({
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "Check this image"},
                    {"type": "image", "source": {"type": "base64", "data": "..."}}
                ]}
            ]
        });
        let texts = extract_message_texts(&json);
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0], "Check this image");
    }

    #[test]
    fn test_extract_message_texts_ollama_prompt() {
        let json = serde_json::json!({
            "model": "llama3",
            "prompt": "Tell me about John Smith"
        });
        let texts = extract_message_texts(&json);
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0], "Tell me about John Smith");
    }

    #[test]
    fn test_extract_message_texts_empty() {
        let json = serde_json::json!({"model": "gpt-4o"});
        let texts = extract_message_texts(&json);
        assert!(texts.is_empty());
    }

    #[test]
    fn test_extract_message_texts_multi_message() {
        let json = serde_json::json!({
            "messages": [
                {"role": "system", "content": "You are helpful."},
                {"role": "user", "content": "My phone is 555-123-4567"}
            ]
        });
        let texts = extract_message_texts(&json);
        assert_eq!(texts.len(), 2);
        assert_eq!(texts[0], "You are helpful.");
        assert!(texts[1].contains("555-123-4567"));
    }

    #[test]
    fn test_media_type_to_ext() {
        assert_eq!(media_type_to_ext("image/png"), "png");
        assert_eq!(media_type_to_ext("image/jpeg"), "jpg");
        assert_eq!(media_type_to_ext("image/gif"), "gif");
        assert_eq!(media_type_to_ext("image/webp"), "webp");
    }

    #[test]
    fn test_audio_media_type_to_ext() {
        assert_eq!(audio_media_type_to_ext("audio/wav"), "wav");
        assert_eq!(audio_media_type_to_ext("audio/mp3"), "mp3");
        assert_eq!(audio_media_type_to_ext("audio/ogg"), "ogg");
        assert_eq!(audio_media_type_to_ext("audio/webm"), "webm");
        assert_eq!(audio_media_type_to_ext("audio/flac"), "bin");
    }

    #[test]
    fn test_inspect_dir() {
        let dir = inspect_dir();
        assert!(dir.is_some());
        let path = dir.unwrap();
        assert!(path.to_str().unwrap().contains(".openobscure"));
        assert!(path.to_str().unwrap().contains("inspect"));
    }
}

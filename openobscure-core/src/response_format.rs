//! Multi-format LLM response detection, text extraction, and warning injection.
//!
//! Supports response formats from top LLM providers:
//!   - Anthropic JSON (`content[].text`)
//!   - OpenAI JSON (`choices[].message.content`)
//!   - Google Gemini JSON (`candidates[].content.parts[].text`)
//!   - Cohere JSON (`text` or `generations[].text`)
//!   - Ollama JSON (`message.content` or `response`)
//!   - Plain text / Markdown (entire body is text)
//!
//! Used by the response integrity scanner to extract text for RI scanning
//! and to inject warning labels into the appropriate text field.

use bytes::Bytes;
use serde_json::Value;

/// Detected response format from an LLM provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseFormat {
    /// Anthropic: `{ "content": [{ "type": "text", "text": "..." }] }`
    AnthropicJson,
    /// OpenAI: `{ "choices": [{ "message": { "content": "..." } }] }`
    OpenAiJson,
    /// Google Gemini: `{ "candidates": [{ "content": { "parts": [{ "text": "..." }] } }] }`
    GeminiJson,
    /// Cohere v2 (`text`) or v1 (`generations[].text`)
    CohereJson,
    /// Ollama chat (`message.content`) or generate (`response`)
    OllamaJson,
    /// `text/plain` or `text/markdown` — entire body is text
    PlainText,
    /// Valid JSON but unrecognized structure — pass through
    UnknownJson,
    /// Binary, unparseable, or unsupported content type — pass through
    Opaque,
}

impl ResponseFormat {
    /// Returns true if this format supports text extraction for RI scanning.
    pub fn supports_ri(&self) -> bool {
        !matches!(self, Self::UnknownJson | Self::Opaque)
    }
}

/// Detect the response format from content-type header and body bytes.
///
/// Detection order:
/// 1. Content-type `text/plain` or `text/markdown` → PlainText
/// 2. Non-JSON content-type (and not missing) → Opaque
/// 3. JSON structure probe (Anthropic → OpenAI → Gemini → Cohere → Ollama)
/// 4. Valid JSON, no match → UnknownJson
/// 5. Not valid JSON → Opaque
pub fn detect(content_type: Option<&str>, body: &[u8]) -> ResponseFormat {
    // Check content-type first
    if let Some(ct) = content_type {
        let ct_lower = ct.to_ascii_lowercase();

        if ct_lower.starts_with("text/plain") || ct_lower.starts_with("text/markdown") {
            return ResponseFormat::PlainText;
        }

        // Non-JSON, non-text content types → Opaque
        if !ct_lower.starts_with("application/json")
            && !ct_lower.contains("json")
            && !ct_lower.starts_with("text/")
        {
            return ResponseFormat::Opaque;
        }
    }

    // Empty body
    if body.is_empty() {
        return ResponseFormat::Opaque;
    }

    // Try JSON parse
    let json: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => {
            // Not valid JSON — could be plain text with no content-type
            if content_type.is_none() && std::str::from_utf8(body).is_ok() {
                return ResponseFormat::PlainText;
            }
            return ResponseFormat::Opaque;
        }
    };

    detect_json_format(&json)
}

/// Probe a parsed JSON value to determine the provider format.
fn detect_json_format(json: &Value) -> ResponseFormat {
    // Anthropic: content array with type/text blocks
    if let Some(content) = json.get("content").and_then(|c| c.as_array()) {
        if content
            .iter()
            .any(|block| block.get("type").is_some() && block.get("text").is_some())
        {
            return ResponseFormat::AnthropicJson;
        }
    }

    // OpenAI: choices array with message.content
    if let Some(choices) = json.get("choices").and_then(|c| c.as_array()) {
        if choices.iter().any(|choice| {
            choice
                .get("message")
                .and_then(|m| m.get("content"))
                .is_some()
        }) {
            return ResponseFormat::OpenAiJson;
        }
    }

    // Gemini: candidates array with content.parts
    if let Some(candidates) = json.get("candidates").and_then(|c| c.as_array()) {
        if candidates.iter().any(|cand| {
            cand.get("content")
                .and_then(|c| c.get("parts"))
                .and_then(|p| p.as_array())
                .is_some()
        }) {
            return ResponseFormat::GeminiJson;
        }
    }

    // Cohere: top-level "text" string (v2) or "generations" array (v1)
    if json.get("text").and_then(|t| t.as_str()).is_some() {
        return ResponseFormat::CohereJson;
    }
    if let Some(gens) = json.get("generations").and_then(|g| g.as_array()) {
        if gens
            .iter()
            .any(|g| g.get("text").and_then(|t| t.as_str()).is_some())
        {
            return ResponseFormat::CohereJson;
        }
    }

    // Ollama: message+model (chat) or response+model (generate)
    if json.get("model").is_some() {
        if json.get("message").and_then(|m| m.get("content")).is_some() {
            return ResponseFormat::OllamaJson;
        }
        if json.get("response").and_then(|r| r.as_str()).is_some() {
            return ResponseFormat::OllamaJson;
        }
    }

    // Valid JSON but unrecognized structure
    ResponseFormat::UnknownJson
}

/// Extract text content from a response body given its detected format.
///
/// Returns `None` for UnknownJson and Opaque formats (fail-open).
pub fn extract_text(body: &[u8], format: ResponseFormat) -> Option<String> {
    match format {
        ResponseFormat::PlainText => std::str::from_utf8(body).ok().map(String::from),
        ResponseFormat::Opaque | ResponseFormat::UnknownJson => None,
        _ => {
            let json: Value = serde_json::from_slice(body).ok()?;
            extract_text_from_json(&json, format)
        }
    }
}

/// Extract text from a parsed JSON value based on format.
fn extract_text_from_json(json: &Value, format: ResponseFormat) -> Option<String> {
    match format {
        ResponseFormat::AnthropicJson => {
            let content = json.get("content")?.as_array()?;
            let texts: Vec<&str> = content
                .iter()
                .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
                .collect();
            if texts.is_empty() {
                None
            } else {
                Some(texts.join(" "))
            }
        }
        ResponseFormat::OpenAiJson => {
            let choices = json.get("choices")?.as_array()?;
            let texts: Vec<&str> = choices
                .iter()
                .filter_map(|choice| {
                    choice
                        .get("message")
                        .and_then(|m| m.get("content"))
                        .and_then(|c| c.as_str())
                })
                .collect();
            if texts.is_empty() {
                None
            } else {
                Some(texts.join(" "))
            }
        }
        ResponseFormat::GeminiJson => {
            let candidates = json.get("candidates")?.as_array()?;
            let mut texts = Vec::new();
            for cand in candidates {
                if let Some(parts) = cand
                    .get("content")
                    .and_then(|c| c.get("parts"))
                    .and_then(|p| p.as_array())
                {
                    for part in parts {
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            texts.push(text);
                        }
                    }
                }
            }
            if texts.is_empty() {
                None
            } else {
                Some(texts.join(" "))
            }
        }
        ResponseFormat::CohereJson => {
            // v2: top-level "text"
            if let Some(text) = json.get("text").and_then(|t| t.as_str()) {
                return Some(text.to_string());
            }
            // v1: generations[].text
            if let Some(gens) = json.get("generations").and_then(|g| g.as_array()) {
                let texts: Vec<&str> = gens
                    .iter()
                    .filter_map(|g| g.get("text").and_then(|t| t.as_str()))
                    .collect();
                if !texts.is_empty() {
                    return Some(texts.join(" "));
                }
            }
            None
        }
        ResponseFormat::OllamaJson => {
            // Chat: message.content
            if let Some(content) = json
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
            {
                return Some(content.to_string());
            }
            // Generate: response
            if let Some(resp) = json.get("response").and_then(|r| r.as_str()) {
                return Some(resp.to_string());
            }
            None
        }
        ResponseFormat::PlainText | ResponseFormat::UnknownJson | ResponseFormat::Opaque => None,
    }
}

/// Inject a warning label into the first text field of a response body.
///
/// Returns the modified body bytes, or `None` if injection is not possible
/// (unknown format, parse error, etc.). Fail-open: caller should use original body.
pub fn inject_warning(body: &[u8], format: ResponseFormat, label: &str) -> Option<Bytes> {
    match format {
        ResponseFormat::PlainText => {
            let text = std::str::from_utf8(body).ok()?;
            Some(Bytes::from(format!("{}{}", label, text)))
        }
        ResponseFormat::Opaque | ResponseFormat::UnknownJson => None,
        _ => {
            let mut json: Value = serde_json::from_slice(body).ok()?;
            inject_warning_json(&mut json, format, label)?;
            let serialized = serde_json::to_vec(&json).ok()?;
            Some(Bytes::from(serialized))
        }
    }
}

/// Inject warning into parsed JSON. Returns Some(()) on success.
fn inject_warning_json(json: &mut Value, format: ResponseFormat, label: &str) -> Option<()> {
    match format {
        ResponseFormat::AnthropicJson => {
            let content = json.get_mut("content")?.as_array_mut()?;
            for block in content.iter_mut() {
                if let Some(text) = block
                    .get_mut("text")
                    .and_then(|t| t.as_str().map(String::from))
                {
                    block["text"] = Value::String(format!("{}{}", label, text));
                    return Some(());
                }
            }
            None
        }
        ResponseFormat::OpenAiJson => {
            let choices = json.get_mut("choices")?.as_array_mut()?;
            for choice in choices.iter_mut() {
                if let Some(content) = choice
                    .get_mut("message")
                    .and_then(|m| m.get_mut("content"))
                    .and_then(|c| c.as_str().map(String::from))
                {
                    choice["message"]["content"] = Value::String(format!("{}{}", label, content));
                    return Some(());
                }
            }
            None
        }
        ResponseFormat::GeminiJson => {
            let candidates = json.get_mut("candidates")?.as_array_mut()?;
            for cand in candidates.iter_mut() {
                if let Some(parts) = cand
                    .get_mut("content")
                    .and_then(|c| c.get_mut("parts"))
                    .and_then(|p| p.as_array_mut())
                {
                    for part in parts.iter_mut() {
                        if let Some(text) = part
                            .get_mut("text")
                            .and_then(|t| t.as_str().map(String::from))
                        {
                            part["text"] = Value::String(format!("{}{}", label, text));
                            return Some(());
                        }
                    }
                }
            }
            None
        }
        ResponseFormat::CohereJson => {
            // v2: top-level "text"
            if let Some(text) = json.get("text").and_then(|t| t.as_str()).map(String::from) {
                json["text"] = Value::String(format!("{}{}", label, text));
                return Some(());
            }
            // v1: generations[].text
            if let Some(gens) = json.get_mut("generations").and_then(|g| g.as_array_mut()) {
                for gen in gens.iter_mut() {
                    if let Some(text) = gen.get("text").and_then(|t| t.as_str()).map(String::from) {
                        gen["text"] = Value::String(format!("{}{}", label, text));
                        return Some(());
                    }
                }
            }
            None
        }
        ResponseFormat::OllamaJson => {
            // Chat: message.content
            if let Some(content) = json
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
                .map(String::from)
            {
                json["message"]["content"] = Value::String(format!("{}{}", label, content));
                return Some(());
            }
            // Generate: response
            if let Some(resp) = json
                .get("response")
                .and_then(|r| r.as_str())
                .map(String::from)
            {
                json["response"] = Value::String(format!("{}{}", label, resp));
                return Some(());
            }
            None
        }
        ResponseFormat::PlainText | ResponseFormat::UnknownJson | ResponseFormat::Opaque => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Detection tests ──────────────────────────────────────────────────

    #[test]
    fn test_detect_anthropic_json() {
        let body = serde_json::json!({
            "id": "msg_123",
            "type": "message",
            "content": [{"type": "text", "text": "Hello world"}],
            "model": "claude-sonnet-4-6-20250514",
            "stop_reason": "end_turn"
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        assert_eq!(
            detect(Some("application/json"), &bytes),
            ResponseFormat::AnthropicJson
        );
    }

    #[test]
    fn test_detect_openai_json() {
        let body = serde_json::json!({
            "id": "chatcmpl-abc",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "Hello!"},
                "finish_reason": "stop"
            }],
            "model": "gpt-4o"
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        assert_eq!(
            detect(Some("application/json"), &bytes),
            ResponseFormat::OpenAiJson
        );
    }

    #[test]
    fn test_detect_gemini_json() {
        let body = serde_json::json!({
            "candidates": [{
                "content": {
                    "parts": [{"text": "Hello from Gemini"}],
                    "role": "model"
                },
                "finishReason": "STOP"
            }]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        assert_eq!(
            detect(Some("application/json"), &bytes),
            ResponseFormat::GeminiJson
        );
    }

    #[test]
    fn test_detect_cohere_v2_json() {
        let body = serde_json::json!({
            "id": "gen-abc",
            "text": "Hello from Cohere",
            "generation_id": "abc-123"
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        assert_eq!(
            detect(Some("application/json"), &bytes),
            ResponseFormat::CohereJson
        );
    }

    #[test]
    fn test_detect_cohere_v1_json() {
        let body = serde_json::json!({
            "id": "gen-abc",
            "generations": [{"id": "0", "text": "Hello from Cohere v1"}],
            "meta": {"api_version": {"version": "1"}}
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        assert_eq!(
            detect(Some("application/json"), &bytes),
            ResponseFormat::CohereJson
        );
    }

    #[test]
    fn test_detect_ollama_chat_json() {
        let body = serde_json::json!({
            "model": "llama3",
            "message": {"role": "assistant", "content": "Hello from Ollama"},
            "done": true
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        assert_eq!(
            detect(Some("application/json"), &bytes),
            ResponseFormat::OllamaJson
        );
    }

    #[test]
    fn test_detect_ollama_generate_json() {
        let body = serde_json::json!({
            "model": "llama3",
            "response": "Hello from Ollama generate",
            "done": true
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        assert_eq!(
            detect(Some("application/json"), &bytes),
            ResponseFormat::OllamaJson
        );
    }

    #[test]
    fn test_detect_plain_text_content_type() {
        let body = b"Just plain text response";
        assert_eq!(detect(Some("text/plain"), body), ResponseFormat::PlainText);
    }

    #[test]
    fn test_detect_markdown_content_type() {
        let body = b"# Heading\n\nSome markdown content";
        assert_eq!(
            detect(Some("text/markdown"), body),
            ResponseFormat::PlainText
        );
    }

    #[test]
    fn test_detect_unknown_json() {
        let body = serde_json::json!({"foo": "bar", "baz": 42});
        let bytes = serde_json::to_vec(&body).unwrap();
        assert_eq!(
            detect(Some("application/json"), &bytes),
            ResponseFormat::UnknownJson
        );
    }

    #[test]
    fn test_detect_binary_opaque() {
        let body = b"\x89PNG\r\n\x1a\n";
        assert_eq!(detect(Some("image/png"), body), ResponseFormat::Opaque);
    }

    #[test]
    fn test_detect_empty_body() {
        assert_eq!(
            detect(Some("application/json"), b""),
            ResponseFormat::Opaque
        );
    }

    #[test]
    fn test_detect_no_content_type_valid_json() {
        let body = serde_json::json!({
            "choices": [{"message": {"content": "test"}}]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        assert_eq!(detect(None, &bytes), ResponseFormat::OpenAiJson);
    }

    #[test]
    fn test_detect_no_content_type_plain_text() {
        let body = b"Hello, this is just text";
        assert_eq!(detect(None, body), ResponseFormat::PlainText);
    }

    #[test]
    fn test_detect_application_json_charset() {
        let body = serde_json::json!({
            "candidates": [{"content": {"parts": [{"text": "hi"}]}}]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        assert_eq!(
            detect(Some("application/json; charset=utf-8"), &bytes),
            ResponseFormat::GeminiJson
        );
    }

    // ── Extract text tests ───────────────────────────────────────────────

    #[test]
    fn test_extract_anthropic_text() {
        let body = serde_json::json!({
            "content": [
                {"type": "text", "text": "Hello "},
                {"type": "text", "text": "world"}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let text = extract_text(&bytes, ResponseFormat::AnthropicJson).unwrap();
        assert_eq!(text, "Hello  world");
    }

    #[test]
    fn test_extract_openai_text() {
        let body = serde_json::json!({
            "choices": [
                {"message": {"content": "First choice"}},
                {"message": {"content": "Second choice"}}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let text = extract_text(&bytes, ResponseFormat::OpenAiJson).unwrap();
        assert_eq!(text, "First choice Second choice");
    }

    #[test]
    fn test_extract_gemini_text() {
        let body = serde_json::json!({
            "candidates": [{
                "content": {
                    "parts": [
                        {"text": "Part one"},
                        {"text": "Part two"}
                    ]
                }
            }]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let text = extract_text(&bytes, ResponseFormat::GeminiJson).unwrap();
        assert_eq!(text, "Part one Part two");
    }

    #[test]
    fn test_extract_cohere_v2_text() {
        let body = serde_json::json!({
            "text": "Cohere response text"
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let text = extract_text(&bytes, ResponseFormat::CohereJson).unwrap();
        assert_eq!(text, "Cohere response text");
    }

    #[test]
    fn test_extract_cohere_v1_text() {
        let body = serde_json::json!({
            "generations": [
                {"text": "Gen one"},
                {"text": "Gen two"}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let text = extract_text(&bytes, ResponseFormat::CohereJson).unwrap();
        assert_eq!(text, "Gen one Gen two");
    }

    #[test]
    fn test_extract_ollama_chat_text() {
        let body = serde_json::json!({
            "model": "llama3",
            "message": {"role": "assistant", "content": "Ollama chat reply"},
            "done": true
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let text = extract_text(&bytes, ResponseFormat::OllamaJson).unwrap();
        assert_eq!(text, "Ollama chat reply");
    }

    #[test]
    fn test_extract_ollama_generate_text() {
        let body = serde_json::json!({
            "model": "llama3",
            "response": "Ollama generate reply",
            "done": true
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let text = extract_text(&bytes, ResponseFormat::OllamaJson).unwrap();
        assert_eq!(text, "Ollama generate reply");
    }

    #[test]
    fn test_extract_plain_text() {
        let body = b"This is a plain text response.";
        let text = extract_text(body, ResponseFormat::PlainText).unwrap();
        assert_eq!(text, "This is a plain text response.");
    }

    #[test]
    fn test_extract_unknown_returns_none() {
        let body = serde_json::json!({"foo": "bar"});
        let bytes = serde_json::to_vec(&body).unwrap();
        assert!(extract_text(&bytes, ResponseFormat::UnknownJson).is_none());
    }

    #[test]
    fn test_extract_opaque_returns_none() {
        let body = b"\x00\x01\x02\x03";
        assert!(extract_text(body, ResponseFormat::Opaque).is_none());
    }

    // ── Inject warning tests ─────────────────────────────────────────────

    const TEST_LABEL: &str = "[WARNING] ";

    #[test]
    fn test_inject_anthropic_warning() {
        let body = serde_json::json!({
            "content": [{"type": "text", "text": "Original text"}]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let result = inject_warning(&bytes, ResponseFormat::AnthropicJson, TEST_LABEL).unwrap();
        let parsed: Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(
            parsed["content"][0]["text"].as_str().unwrap(),
            "[WARNING] Original text"
        );
    }

    #[test]
    fn test_inject_openai_warning() {
        let body = serde_json::json!({
            "choices": [{"message": {"content": "Original text"}}]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let result = inject_warning(&bytes, ResponseFormat::OpenAiJson, TEST_LABEL).unwrap();
        let parsed: Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(
            parsed["choices"][0]["message"]["content"].as_str().unwrap(),
            "[WARNING] Original text"
        );
    }

    #[test]
    fn test_inject_gemini_warning() {
        let body = serde_json::json!({
            "candidates": [{
                "content": {
                    "parts": [{"text": "Gemini text"}]
                }
            }]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let result = inject_warning(&bytes, ResponseFormat::GeminiJson, TEST_LABEL).unwrap();
        let parsed: Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(
            parsed["candidates"][0]["content"]["parts"][0]["text"]
                .as_str()
                .unwrap(),
            "[WARNING] Gemini text"
        );
    }

    #[test]
    fn test_inject_cohere_v2_warning() {
        let body = serde_json::json!({"text": "Cohere text"});
        let bytes = serde_json::to_vec(&body).unwrap();
        let result = inject_warning(&bytes, ResponseFormat::CohereJson, TEST_LABEL).unwrap();
        let parsed: Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(parsed["text"].as_str().unwrap(), "[WARNING] Cohere text");
    }

    #[test]
    fn test_inject_cohere_v1_warning() {
        let body = serde_json::json!({
            "generations": [{"text": "Gen text"}]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let result = inject_warning(&bytes, ResponseFormat::CohereJson, TEST_LABEL).unwrap();
        let parsed: Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(
            parsed["generations"][0]["text"].as_str().unwrap(),
            "[WARNING] Gen text"
        );
    }

    #[test]
    fn test_inject_ollama_chat_warning() {
        let body = serde_json::json!({
            "model": "llama3",
            "message": {"content": "Ollama text"},
            "done": true
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let result = inject_warning(&bytes, ResponseFormat::OllamaJson, TEST_LABEL).unwrap();
        let parsed: Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(
            parsed["message"]["content"].as_str().unwrap(),
            "[WARNING] Ollama text"
        );
    }

    #[test]
    fn test_inject_ollama_generate_warning() {
        let body = serde_json::json!({
            "model": "llama3",
            "response": "Ollama gen text",
            "done": true
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let result = inject_warning(&bytes, ResponseFormat::OllamaJson, TEST_LABEL).unwrap();
        let parsed: Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(
            parsed["response"].as_str().unwrap(),
            "[WARNING] Ollama gen text"
        );
    }

    #[test]
    fn test_inject_plain_text_warning() {
        let body = b"Some plain text";
        let result = inject_warning(body, ResponseFormat::PlainText, TEST_LABEL).unwrap();
        assert_eq!(result, Bytes::from("[WARNING] Some plain text"));
    }

    #[test]
    fn test_inject_unknown_returns_none() {
        let body = serde_json::json!({"foo": "bar"});
        let bytes = serde_json::to_vec(&body).unwrap();
        assert!(inject_warning(&bytes, ResponseFormat::UnknownJson, TEST_LABEL).is_none());
    }

    #[test]
    fn test_inject_opaque_returns_none() {
        let body = b"\x00\x01";
        assert!(inject_warning(body, ResponseFormat::Opaque, TEST_LABEL).is_none());
    }

    #[test]
    fn test_inject_invalid_json_returns_none() {
        let body = b"not json at all";
        assert!(inject_warning(body, ResponseFormat::AnthropicJson, TEST_LABEL).is_none());
    }

    // ── supports_ri tests ────────────────────────────────────────────────

    #[test]
    fn test_supports_ri() {
        assert!(ResponseFormat::AnthropicJson.supports_ri());
        assert!(ResponseFormat::OpenAiJson.supports_ri());
        assert!(ResponseFormat::GeminiJson.supports_ri());
        assert!(ResponseFormat::CohereJson.supports_ri());
        assert!(ResponseFormat::OllamaJson.supports_ri());
        assert!(ResponseFormat::PlainText.supports_ri());
        assert!(!ResponseFormat::UnknownJson.supports_ri());
        assert!(!ResponseFormat::Opaque.supports_ri());
    }

    // ── Edge case tests ──────────────────────────────────────────────────

    #[test]
    fn test_detect_anthropic_multiple_content_types() {
        // Anthropic with mixed content blocks (text + tool_use)
        let body = serde_json::json!({
            "content": [
                {"type": "text", "text": "Here's the result:"},
                {"type": "tool_use", "id": "tool_1", "name": "calculator", "input": {}}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        assert_eq!(
            detect(Some("application/json"), &bytes),
            ResponseFormat::AnthropicJson
        );
        // Extract should only get text blocks
        let text = extract_text(&bytes, ResponseFormat::AnthropicJson).unwrap();
        assert_eq!(text, "Here's the result:");
    }

    #[test]
    fn test_gemini_multi_candidate() {
        let body = serde_json::json!({
            "candidates": [
                {"content": {"parts": [{"text": "Candidate 1"}]}},
                {"content": {"parts": [{"text": "Candidate 2"}]}}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let text = extract_text(&bytes, ResponseFormat::GeminiJson).unwrap();
        assert_eq!(text, "Candidate 1 Candidate 2");
    }

    #[test]
    fn test_extract_empty_content_array() {
        let body = serde_json::json!({"content": []});
        let bytes = serde_json::to_vec(&body).unwrap();
        // Empty content array won't have type+text blocks → UnknownJson
        assert_eq!(
            detect(Some("application/json"), &bytes),
            ResponseFormat::UnknownJson
        );
    }

    #[test]
    fn test_detect_event_stream_opaque() {
        // text/event-stream is SSE — handled by streaming path, not buffered format detection
        let body = b"data: {\"text\": \"hello\"}\n\n";
        assert_eq!(
            detect(Some("text/event-stream"), body),
            ResponseFormat::Opaque
        );
    }
}

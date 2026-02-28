//! SSE frame accumulation buffer for cross-frame PII token detection.
//!
//! SSE frames can split FPE ciphertext tokens at arbitrary byte boundaries.
//! This buffer holds a trailing window of unconfirmed bytes so that split
//! tokens are reassembled before decryption.

use bytes::Bytes;

use crate::mapping::RequestMappings;

/// Accumulation buffer for cross-SSE-frame PII token detection.
///
/// When a new frame arrives:
/// 1. Prepend the buffer to the new frame data
/// 2. Scan the combined text for ciphertext matches
/// 3. Emit the "confirmed clean" prefix (everything before the last potential match boundary)
/// 4. Retain the trailing bytes as the new buffer contents
pub struct SseAccumulator {
    /// Pending bytes not yet emitted (potential partial token at the end).
    buffer: Vec<u8>,
    /// Maximum buffer size — if exceeded, force flush everything.
    max_buffer_size: usize,
}

impl SseAccumulator {
    pub fn new(max_buffer_size: usize) -> Self {
        Self {
            buffer: Vec::new(),
            max_buffer_size,
        }
    }

    /// Feed a new SSE frame and produce bytes safe to emit.
    ///
    /// The returned bytes have all complete ciphertext matches replaced.
    /// Bytes that might be part of a split token are retained in the buffer.
    pub fn feed(&mut self, data: &Bytes, mappings: &RequestMappings) -> Bytes {
        if mappings.is_empty() {
            // No mappings — pass through immediately, no buffering needed
            return data.clone();
        }

        // Combine buffer + new data
        let mut combined = std::mem::take(&mut self.buffer);
        combined.extend_from_slice(data);

        // Find the maximum ciphertext length to determine safe emit boundary
        let max_ct_len = mappings.max_ciphertext_len();

        if max_ct_len == 0 || combined.len() <= max_ct_len {
            // Not enough data to safely emit — buffer everything
            // But enforce max buffer size
            if combined.len() > self.max_buffer_size {
                let text = String::from_utf8_lossy(&combined);
                let processed = mappings.decrypt_response(&text);
                return Bytes::from(processed.into_bytes());
            }
            self.buffer = combined;
            return Bytes::new();
        }

        // Safe emit boundary: everything except the last max_ct_len bytes
        // (those trailing bytes could be the start of a split ciphertext)
        let safe_end = combined.len() - max_ct_len;

        // Snap to a valid UTF-8 char boundary
        let safe_end = snap_to_char_boundary(&combined, safe_end);

        let safe_text = String::from_utf8_lossy(&combined[..safe_end]);
        let processed = mappings.decrypt_response(&safe_text);

        // Retain trailing bytes as new buffer
        self.buffer = combined[safe_end..].to_vec();

        Bytes::from(processed.into_bytes())
    }

    /// Flush all remaining buffered bytes (called on stream end or timeout).
    pub fn flush(&mut self, mappings: &RequestMappings) -> Bytes {
        if self.buffer.is_empty() {
            return Bytes::new();
        }
        let buf = std::mem::take(&mut self.buffer);
        let text = String::from_utf8_lossy(&buf);
        let processed = mappings.decrypt_response(&text);
        Bytes::from(processed.into_bytes())
    }

    /// Returns true if the buffer has pending data.
    pub fn has_pending(&self) -> bool {
        !self.buffer.is_empty()
    }
}

/// Default maximum text bytes for SseRiBuffer.
const RI_BUFFER_DEFAULT_MAX: usize = 256 * 1024; // 256KB

/// Parallel text accumulator for SSE streams — extracts text content for RI scanning.
///
/// Runs alongside `SseAccumulator` (which handles FPE decryption). This buffer
/// accumulates the LLM's text output from SSE delta events so that Response
/// Integrity scanning can run on the complete text after the stream ends.
///
/// Supports Anthropic, OpenAI, and Gemini SSE delta formats, plus a generic
/// fallback that extracts any `"text"` or `"content"` string values.
pub struct SseRiBuffer {
    text: String,
    max_text_bytes: usize,
    truncated: bool,
}

impl Default for SseRiBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl SseRiBuffer {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            max_text_bytes: RI_BUFFER_DEFAULT_MAX,
            truncated: false,
        }
    }

    pub fn with_max_bytes(max_text_bytes: usize) -> Self {
        Self {
            text: String::new(),
            max_text_bytes,
            truncated: false,
        }
    }

    /// Feed raw SSE frame bytes. Parses `data:` lines and extracts text deltas.
    pub fn feed_sse_data(&mut self, raw: &[u8]) {
        if self.truncated {
            return;
        }

        let text = match std::str::from_utf8(raw) {
            Ok(s) => s,
            Err(_) => return,
        };

        for line in text.lines() {
            let data = if let Some(d) = line.strip_prefix("data: ") {
                d.trim()
            } else if let Some(d) = line.strip_prefix("data:") {
                d.trim()
            } else {
                continue;
            };

            // Skip SSE termination and comments
            if data == "[DONE]" || data.is_empty() {
                continue;
            }

            // Try JSON parse
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                self.extract_delta_text(&json);
            }
        }
    }

    /// Extract text from a single SSE JSON delta event.
    fn extract_delta_text(&mut self, json: &serde_json::Value) {
        // Anthropic: content_block_delta → delta.text
        if let Some(text) = json
            .get("delta")
            .and_then(|d| d.get("text"))
            .and_then(|t| t.as_str())
        {
            self.append(text);
            return;
        }

        // OpenAI: choices[0].delta.content
        if let Some(content) = json
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|choice| choice.get("delta"))
            .and_then(|d| d.get("content"))
            .and_then(|c| c.as_str())
        {
            self.append(content);
            return;
        }

        // Gemini: candidates[0].content.parts[0].text
        if let Some(text) = json
            .get("candidates")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|cand| cand.get("content"))
            .and_then(|c| c.get("parts"))
            .and_then(|p| p.as_array())
            .and_then(|parts| parts.first())
            .and_then(|part| part.get("text"))
            .and_then(|t| t.as_str())
        {
            self.append(text);
            return;
        }

        // Generic fallback: any "text" or "content" string at top level
        if let Some(text) = json.get("text").and_then(|t| t.as_str()) {
            self.append(text);
        } else if let Some(content) = json.get("content").and_then(|c| c.as_str()) {
            self.append(content);
        }
    }

    fn append(&mut self, text: &str) {
        if self.text.len() + text.len() > self.max_text_bytes {
            // Truncate at limit
            let remaining = self.max_text_bytes.saturating_sub(self.text.len());
            if remaining > 0 {
                // Snap to char boundary (MSRV-safe: floor_char_boundary requires 1.91)
                let mut end = remaining.min(text.len());
                while end > 0 && !text.is_char_boundary(end) {
                    end -= 1;
                }
                let safe = &text[..end];
                self.text.push_str(safe);
            }
            self.truncated = true;
        } else {
            self.text.push_str(text);
        }
    }

    /// Finish accumulation and return the collected text for RI scanning.
    /// Returns `None` if no text was accumulated.
    pub fn finish(self) -> Option<String> {
        if self.text.is_empty() {
            None
        } else {
            Some(self.text)
        }
    }

    /// Whether the buffer had to truncate text at the max limit.
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }
}

/// Snap a byte offset backward to a valid UTF-8 character boundary.
fn snap_to_char_boundary(data: &[u8], offset: usize) -> usize {
    let mut pos = offset;
    while pos > 0 && (data[pos] & 0xC0) == 0x80 {
        pos -= 1;
    }
    pos
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::{FpeMapping, RequestMappings};
    use crate::pii_types::PiiType;

    fn mappings_with(pairs: &[(&str, &str)]) -> RequestMappings {
        let mut m = RequestMappings::new(uuid::Uuid::new_v4());
        for (ct, pt) in pairs {
            m.insert(FpeMapping {
                pii_type: PiiType::Email,
                plaintext: pt.to_string(),
                ciphertext: ct.to_string(),
                tweak: vec![],
                key_version: 1,
            });
        }
        m
    }

    #[test]
    fn test_accumulator_no_mappings_passthrough() {
        let empty = RequestMappings::new(uuid::Uuid::new_v4());
        let mut acc = SseAccumulator::new(512);
        let data = Bytes::from("Hello world");
        let result = acc.feed(&data, &empty);
        assert_eq!(result, Bytes::from("Hello world"));
        assert!(!acc.has_pending());
    }

    #[test]
    fn test_accumulator_complete_token_single_frame() {
        let m = mappings_with(&[("ENCRYPTED", "secret@email.com")]);
        let mut acc = SseAccumulator::new(512);
        // Feed enough data that combined > max_ct_len
        let data = Bytes::from("The value is ENCRYPTED and more text follows here padding");
        let result = acc.feed(&data, &m);
        // The safe prefix should have the replacement
        let result_str = String::from_utf8(result.to_vec()).unwrap();
        assert!(
            result_str.contains("secret@email.com") || acc.has_pending(),
            "Token should be replaced in emitted output or pending in buffer"
        );
        // Flush remaining
        let flush = acc.flush(&m);
        let combined = format!(
            "{}{}",
            result_str,
            String::from_utf8(flush.to_vec()).unwrap()
        );
        assert!(combined.contains("secret@email.com"));
        assert!(!combined.contains("ENCRYPTED"));
    }

    #[test]
    fn test_accumulator_split_token_across_frames() {
        let m = mappings_with(&[("ABCDEFGHIJ", "plaintext!")]);
        let mut acc = SseAccumulator::new(512);

        // Frame 1: contains partial token "ABCDE"
        let frame1 = Bytes::from("prefix ABCDE");
        let out1 = acc.feed(&frame1, &m);

        // Frame 2: completes the token "FGHIJ" + more text
        let frame2 = Bytes::from("FGHIJ suffix and more padding data here");
        let out2 = acc.feed(&frame2, &m);

        // Flush
        let out3 = acc.flush(&m);

        let combined = format!(
            "{}{}{}",
            String::from_utf8_lossy(&out1),
            String::from_utf8_lossy(&out2),
            String::from_utf8_lossy(&out3),
        );
        assert!(
            combined.contains("plaintext!"),
            "Split token should be reassembled and replaced: {:?}",
            combined
        );
        assert!(!combined.contains("ABCDEFGHIJ"));
    }

    #[test]
    fn test_accumulator_flush_emits_remaining() {
        let m = mappings_with(&[("TOKEN", "value")]);
        let mut acc = SseAccumulator::new(512);
        // Feed a small amount (less than max_ct_len) — should buffer
        let data = Bytes::from("TOK");
        let result = acc.feed(&data, &m);
        assert!(result.is_empty());
        assert!(acc.has_pending());

        // Flush should emit the buffered data
        let flush = acc.flush(&m);
        assert_eq!(flush, Bytes::from("TOK"));
        assert!(!acc.has_pending());
    }

    #[test]
    fn test_accumulator_max_buffer_forced_flush() {
        let m = mappings_with(&[("VERY_LONG_CIPHERTEXT_TOKEN", "short")]);
        // Set max buffer to 10 bytes — much smaller than the token
        let mut acc = SseAccumulator::new(10);

        // Feed data larger than max_buffer but within combined range
        let data = Bytes::from("some text that is longer than 10 bytes");
        let result = acc.feed(&data, &m);
        // Should have forced a flush since combined > max_buffer
        assert!(!result.is_empty());
    }

    #[test]
    fn test_accumulator_empty_frame() {
        let m = mappings_with(&[("TOKEN", "value")]);
        let mut acc = SseAccumulator::new(512);
        let data = Bytes::new();
        let result = acc.feed(&data, &m);
        assert!(result.is_empty());
    }

    // ── SseRiBuffer tests ────────────────────────────────────────────────

    #[test]
    fn test_ri_buffer_anthropic_sse_deltas() {
        let mut buf = SseRiBuffer::new();
        buf.feed_sse_data(b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello \"}}\n\n");
        buf.feed_sse_data(b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"world\"}}\n\n");
        let text = buf.finish().unwrap();
        assert_eq!(text, "Hello world");
    }

    #[test]
    fn test_ri_buffer_openai_sse_deltas() {
        let mut buf = SseRiBuffer::new();
        buf.feed_sse_data(b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello \"}}]}\n\n");
        buf.feed_sse_data(b"data: {\"choices\":[{\"delta\":{\"content\":\"world\"}}]}\n\n");
        let text = buf.finish().unwrap();
        assert_eq!(text, "Hello world");
    }

    #[test]
    fn test_ri_buffer_gemini_sse_deltas() {
        let mut buf = SseRiBuffer::new();
        buf.feed_sse_data(
            b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello \"}]}}]}\n\n",
        );
        buf.feed_sse_data(
            b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Gemini\"}]}}]}\n\n",
        );
        let text = buf.finish().unwrap();
        assert_eq!(text, "Hello Gemini");
    }

    #[test]
    fn test_ri_buffer_ignores_done() {
        let mut buf = SseRiBuffer::new();
        buf.feed_sse_data(b"data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n");
        buf.feed_sse_data(b"data: [DONE]\n\n");
        let text = buf.finish().unwrap();
        assert_eq!(text, "Hi");
    }

    #[test]
    fn test_ri_buffer_max_bytes_truncation() {
        let mut buf = SseRiBuffer::with_max_bytes(10);
        buf.feed_sse_data(b"data: {\"text\":\"Hello world this is long\"}\n\n");
        assert!(buf.is_truncated());
        let text = buf.finish().unwrap();
        assert!(text.len() <= 10);
        assert!(text.starts_with("Hello"));
    }

    #[test]
    fn test_ri_buffer_empty_stream() {
        let buf = SseRiBuffer::new();
        assert!(buf.finish().is_none());
    }

    #[test]
    fn test_ri_buffer_non_json_data() {
        let mut buf = SseRiBuffer::new();
        buf.feed_sse_data(b"data: this is not json\n\n");
        assert!(buf.finish().is_none());
    }

    #[test]
    fn test_ri_buffer_generic_fallback() {
        let mut buf = SseRiBuffer::new();
        // Non-standard format with top-level "content" string
        buf.feed_sse_data(b"data: {\"content\":\"fallback text\"}\n\n");
        let text = buf.finish().unwrap();
        assert_eq!(text, "fallback text");
    }
}

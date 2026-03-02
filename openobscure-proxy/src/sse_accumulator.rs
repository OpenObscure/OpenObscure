//! SSE frame accumulation buffer for cross-frame PII token detection.
//!
//! SSE frames can split FPE ciphertext tokens at arbitrary byte boundaries.
//! This buffer holds a trailing window of unconfirmed bytes so that split
//! tokens are reassembled before decryption.

use bytes::Bytes;

use crate::mapping::RequestMappings;

/// Detected SSE stream format — used to emit RI warnings in the correct delta format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SseFormat {
    /// Not yet detected (no deltas parsed yet).
    Unknown,
    /// Anthropic: `content_block_delta` → `delta.text`
    Anthropic,
    /// OpenAI / OpenRouter: `choices[0].delta.content`
    OpenAi,
    /// Google Gemini: `candidates[0].content.parts[0].text`
    Gemini,
    /// Generic fallback: top-level `text` or `content` string
    Generic,
}

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
    format: SseFormat,
    seen_done: bool,
    /// Incomplete line from previous HTTP chunk.
    line_buffer: String,
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
            format: SseFormat::Unknown,
            seen_done: false,
            line_buffer: String::new(),
        }
    }

    pub fn with_max_bytes(max_text_bytes: usize) -> Self {
        Self {
            text: String::new(),
            max_text_bytes,
            truncated: false,
            format: SseFormat::Unknown,
            seen_done: false,
            line_buffer: String::new(),
        }
    }

    /// Feed raw SSE frame bytes. Parses `data:` lines and extracts text deltas.
    ///
    /// Handles partial lines spanning HTTP chunks via an internal line buffer.
    pub fn feed_sse_data(&mut self, raw: &[u8]) {
        if self.truncated {
            return;
        }

        let text = match std::str::from_utf8(raw) {
            Ok(s) => s,
            Err(_) => return,
        };

        // Prepend any partial line from the previous chunk
        let combined;
        let input = if self.line_buffer.is_empty() {
            text
        } else {
            combined = format!("{}{}", std::mem::take(&mut self.line_buffer), text);
            &combined
        };

        // If chunk doesn't end with \n, save trailing partial line
        let (lines_text, trailing) = if input.ends_with('\n') {
            (input, "")
        } else {
            match input.rfind('\n') {
                Some(pos) => (&input[..=pos], &input[pos + 1..]),
                None => {
                    self.line_buffer = input.to_string();
                    return;
                }
            }
        };
        self.line_buffer = trailing.to_string();

        for line in lines_text.lines() {
            let data = if let Some(d) = line.strip_prefix("data: ") {
                d.trim()
            } else if let Some(d) = line.strip_prefix("data:") {
                d.trim()
            } else {
                continue;
            };

            // Skip SSE termination and comments
            if data == "[DONE]" {
                self.seen_done = true;
                continue;
            }
            if data.is_empty() {
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
            if self.format == SseFormat::Unknown {
                self.format = SseFormat::Anthropic;
            }
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
            if self.format == SseFormat::Unknown {
                self.format = SseFormat::OpenAi;
            }
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
            if self.format == SseFormat::Unknown {
                self.format = SseFormat::Gemini;
            }
            self.append(text);
            return;
        }

        // Generic fallback: any "text" or "content" string at top level
        if let Some(text) = json.get("text").and_then(|t| t.as_str()) {
            if self.format == SseFormat::Unknown {
                self.format = SseFormat::Generic;
            }
            self.append(text);
        } else if let Some(content) = json.get("content").and_then(|c| c.as_str()) {
            if self.format == SseFormat::Unknown {
                self.format = SseFormat::Generic;
            }
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

    /// Move text out without consuming the buffer. Returns `None` if empty.
    pub fn take_text(&mut self) -> Option<String> {
        if self.text.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.text))
        }
    }

    /// Whether `data: [DONE]` was seen in the SSE stream.
    pub fn has_seen_done(&self) -> bool {
        self.seen_done
    }

    /// The SSE format detected from parsed delta events.
    pub fn detected_format(&self) -> SseFormat {
        self.format
    }

    /// Whether the buffer had to truncate text at the max limit.
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }
}

/// Format text content as an SSE delta event in the detected stream format.
///
/// Creates a synthetic SSE event containing the given text. Used by
/// `SseContentDecryptor` to emit FPE-processed content and by
/// `format_sse_warning_chunk` for RI warnings.
pub fn format_sse_delta(format: SseFormat, text: &str) -> String {
    match format {
        SseFormat::Anthropic => {
            let json = serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {
                    "type": "text_delta",
                    "text": text,
                }
            });
            format!("event: content_block_delta\ndata: {json}\n\n")
        }
        SseFormat::Gemini => {
            let json = serde_json::json!({
                "candidates": [{
                    "content": {
                        "parts": [{"text": text}]
                    }
                }]
            });
            format!("data: {json}\n\n")
        }
        // OpenAI, Generic, Unknown all use OpenAI chunk format
        _ => {
            let json = serde_json::json!({
                "id": "oo-proxy",
                "object": "chat.completion.chunk",
                "choices": [{
                    "index": 0,
                    "delta": {"content": text},
                    "finish_reason": serde_json::Value::Null,
                }]
            });
            format!("data: {json}\n\n")
        }
    }
}

/// Format an RI warning as a standard SSE content delta chunk.
///
/// Emits the warning in the same format the LLM stream is using, so clients
/// process it as normal response text. Falls back to OpenAI format when the
/// stream format is unknown.
pub fn format_sse_warning_chunk(format: SseFormat, label: &str) -> String {
    let content = format!("\n\n{label}");
    match format {
        SseFormat::Anthropic => {
            let json = serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {
                    "type": "text_delta",
                    "text": content,
                }
            });
            format!("event: content_block_delta\ndata: {json}\n\n")
        }
        SseFormat::Gemini => {
            let json = serde_json::json!({
                "candidates": [{
                    "content": {
                        "parts": [{"text": content}]
                    }
                }]
            });
            format!("data: {json}\n\n")
        }
        // OpenAI, Generic, Unknown all use OpenAI chunk format
        _ => {
            let json = serde_json::json!({
                "id": "oo-ri-warning",
                "object": "chat.completion.chunk",
                "choices": [{
                    "index": 0,
                    "delta": {"content": content},
                    "finish_reason": serde_json::Value::Null,
                }]
            });
            format!("data: {json}\n\n")
        }
    }
}

/// Content-level SSE FPE decryptor.
///
/// Unlike [`SseAccumulator`] (which works on raw bytes), this decryptor
/// extracts text content from SSE delta events and performs ciphertext
/// replacement at the content level. This correctly handles ciphertexts
/// that are split across multiple SSE delta events by the LLM tokenizer.
///
/// Content is buffered until [`flush`](SseContentDecryptor::flush) is called
/// (at `[DONE]`, stream end, or error), at which point all ciphertexts in
/// the accumulated text are replaced and emitted as a single synthetic SSE
/// delta event. Non-content events (metadata, pings) are passed through
/// immediately.
pub struct SseContentDecryptor {
    /// Accumulated content text not yet emitted.
    content_buffer: String,
    /// The SSE format detected from the stream.
    format: SseFormat,
    /// Incomplete line from the previous HTTP chunk.
    /// HTTP chunks can split SSE events at arbitrary byte boundaries.
    /// This holds the trailing partial line until the next chunk completes it.
    line_buffer: String,
}

impl Default for SseContentDecryptor {
    fn default() -> Self {
        Self::new()
    }
}

impl SseContentDecryptor {
    pub fn new() -> Self {
        Self {
            content_buffer: String::new(),
            format: SseFormat::Unknown,
            line_buffer: String::new(),
        }
    }

    /// Feed a raw SSE frame. Extracts content text into the internal buffer
    /// and passes through non-content events immediately.
    ///
    /// Content text is NOT emitted here — call [`flush`](Self::flush) at
    /// stream end to emit the processed content with all ciphertexts replaced.
    pub fn feed(&mut self, raw: &[u8], mappings: &RequestMappings) -> Bytes {
        if mappings.is_empty() {
            // No mappings — still extract content for format detection but pass through raw
            let _ = self.parse_and_buffer(raw);
            return Bytes::copy_from_slice(raw);
        }

        let passthrough = self.parse_and_buffer(raw);

        if passthrough.is_empty() {
            Bytes::new()
        } else {
            Bytes::from(passthrough)
        }
    }

    /// Parse an SSE frame, buffer content text, and return any passthrough output.
    ///
    /// Handles partial lines that span HTTP chunks: if a chunk doesn't end with
    /// `\n`, the trailing incomplete line is saved in `line_buffer` and prepended
    /// to the next chunk.
    fn parse_and_buffer(&mut self, raw: &[u8]) -> String {
        let text = match std::str::from_utf8(raw) {
            Ok(s) => s,
            Err(_) => return String::new(),
        };

        // Prepend any partial line from the previous chunk
        let combined;
        let input = if self.line_buffer.is_empty() {
            text
        } else {
            combined = format!("{}{}", std::mem::take(&mut self.line_buffer), text);
            &combined
        };

        // If the chunk doesn't end with \n, the last "line" is incomplete —
        // save it for the next chunk.
        let (lines_text, trailing) = if input.ends_with('\n') {
            (input, "")
        } else {
            match input.rfind('\n') {
                Some(pos) => (&input[..=pos], &input[pos + 1..]),
                None => {
                    // Entire chunk is one incomplete line — buffer it all
                    self.line_buffer = input.to_string();
                    return String::new();
                }
            }
        };
        self.line_buffer = trailing.to_string();

        let mut passthrough = String::new();
        let mut pending_event_line = String::new();

        for line in lines_text.lines() {
            if line.starts_with("event: ") {
                // SSE event type — remember it for the next data line
                pending_event_line = format!("{line}\n");
                continue;
            }

            let data = if let Some(d) = line.strip_prefix("data: ") {
                d.trim()
            } else if let Some(d) = line.strip_prefix("data:") {
                d.trim()
            } else {
                // Other SSE fields (id:, retry:) or blank lines — pass through
                if !line.is_empty() {
                    passthrough.push_str(line);
                    passthrough.push('\n');
                }
                continue;
            };

            // Skip [DONE] — handled by proxy's [DONE] logic
            if data == "[DONE]" {
                pending_event_line.clear();
                continue;
            }

            if data.is_empty() {
                pending_event_line.clear();
                continue;
            }

            // Try to extract content from JSON delta
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                if let Some((content, fmt)) = Self::extract_content(&json) {
                    if self.format == SseFormat::Unknown {
                        self.format = fmt;
                    }
                    self.content_buffer.push_str(&content);
                    pending_event_line.clear();
                    continue;
                }
            }

            // Non-content data line — pass through with its event prefix
            passthrough.push_str(&pending_event_line);
            passthrough.push_str(line);
            passthrough.push_str("\n\n");
            pending_event_line.clear();
        }

        passthrough
    }

    /// Flush all buffered content with ciphertext replacement.
    ///
    /// Call this at `[DONE]`, stream end, or error to emit the accumulated
    /// content as a single synthetic SSE delta event with all ciphertexts
    /// replaced by their original plaintexts.
    pub fn flush(&mut self, mappings: &RequestMappings) -> Bytes {
        if self.content_buffer.is_empty() {
            return Bytes::new();
        }
        let text = std::mem::take(&mut self.content_buffer);
        if mappings.is_empty() {
            return Bytes::from(format_sse_delta(self.format, &text));
        }
        let processed = mappings.decrypt_response(&text);
        Bytes::from(format_sse_delta(self.format, &processed))
    }

    /// Returns true if there's pending content in the buffer.
    pub fn has_pending(&self) -> bool {
        !self.content_buffer.is_empty()
    }

    /// Number of chars currently buffered.
    pub fn buffer_len(&self) -> usize {
        self.content_buffer.len()
    }

    /// The SSE format detected from parsed delta events.
    pub fn detected_format(&self) -> SseFormat {
        self.format
    }

    /// Extract content text and format from a JSON delta event.
    fn extract_content(json: &serde_json::Value) -> Option<(String, SseFormat)> {
        // Anthropic: delta.text
        if let Some(text) = json
            .get("delta")
            .and_then(|d| d.get("text"))
            .and_then(|t| t.as_str())
        {
            return Some((text.to_string(), SseFormat::Anthropic));
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
            return Some((content.to_string(), SseFormat::OpenAi));
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
            return Some((text.to_string(), SseFormat::Gemini));
        }

        // Generic fallback: top-level "text" or "content" string
        if let Some(text) = json.get("text").and_then(|t| t.as_str()) {
            return Some((text.to_string(), SseFormat::Generic));
        }
        if let Some(content) = json.get("content").and_then(|c| c.as_str()) {
            return Some((content.to_string(), SseFormat::Generic));
        }

        None
    }
}

/// Split raw SSE frame bytes at the `data: [DONE]` marker.
///
/// Returns `(bytes_before_done, found_done)`. If `[DONE]` is not found,
/// returns the original data unchanged with `false`.
pub fn split_at_done(data: &[u8]) -> (bytes::Bytes, bool) {
    let text = match std::str::from_utf8(data) {
        Ok(s) => s,
        Err(_) => return (bytes::Bytes::copy_from_slice(data), false),
    };

    // Match both "data: [DONE]" and "data:[DONE]" (with/without space)
    let needle = if let Some(pos) = text.find("data: [DONE]") {
        Some(pos)
    } else {
        text.find("data:[DONE]")
    };

    match needle {
        Some(pos) => {
            let before = text[..pos].to_string();
            (bytes::Bytes::from(before), true)
        }
        None => (bytes::Bytes::copy_from_slice(data), false),
    }
}

/// Snap a byte offset backward to a valid UTF-8 character boundary (raw bytes).
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

    // ── SseFormat detection tests ────────────────────────────────────────

    #[test]
    fn test_ri_buffer_detects_openai_format() {
        let mut buf = SseRiBuffer::new();
        assert_eq!(buf.detected_format(), SseFormat::Unknown);
        buf.feed_sse_data(b"data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n");
        assert_eq!(buf.detected_format(), SseFormat::OpenAi);
    }

    #[test]
    fn test_ri_buffer_detects_anthropic_format() {
        let mut buf = SseRiBuffer::new();
        buf.feed_sse_data(b"data: {\"delta\":{\"text\":\"Hi\"}}\n\n");
        assert_eq!(buf.detected_format(), SseFormat::Anthropic);
    }

    #[test]
    fn test_ri_buffer_detects_gemini_format() {
        let mut buf = SseRiBuffer::new();
        buf.feed_sse_data(
            b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hi\"}]}}]}\n\n",
        );
        assert_eq!(buf.detected_format(), SseFormat::Gemini);
    }

    #[test]
    fn test_ri_buffer_detects_generic_format() {
        let mut buf = SseRiBuffer::new();
        buf.feed_sse_data(b"data: {\"text\":\"Hi\"}\n\n");
        assert_eq!(buf.detected_format(), SseFormat::Generic);
    }

    #[test]
    fn test_ri_buffer_format_locked_on_first_match() {
        let mut buf = SseRiBuffer::new();
        buf.feed_sse_data(b"data: {\"choices\":[{\"delta\":{\"content\":\"A\"}}]}\n\n");
        assert_eq!(buf.detected_format(), SseFormat::OpenAi);
        // Second delta with different format doesn't change detected format
        buf.feed_sse_data(b"data: {\"delta\":{\"text\":\"B\"}}\n\n");
        assert_eq!(buf.detected_format(), SseFormat::OpenAi);
    }

    // ── seen_done tests ──────────────────────────────────────────────────

    #[test]
    fn test_ri_buffer_seen_done() {
        let mut buf = SseRiBuffer::new();
        assert!(!buf.has_seen_done());
        buf.feed_sse_data(b"data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n");
        assert!(!buf.has_seen_done());
        buf.feed_sse_data(b"data: [DONE]\n\n");
        assert!(buf.has_seen_done());
    }

    #[test]
    fn test_ri_buffer_seen_done_no_space() {
        let mut buf = SseRiBuffer::new();
        buf.feed_sse_data(b"data:[DONE]\n\n");
        assert!(buf.has_seen_done());
    }

    // ── take_text tests ──────────────────────────────────────────────────

    #[test]
    fn test_ri_buffer_take_text() {
        let mut buf = SseRiBuffer::new();
        buf.feed_sse_data(b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n");
        let text = buf.take_text().unwrap();
        assert_eq!(text, "Hello");
        // After take, text is empty
        assert!(buf.take_text().is_none());
        // But format is preserved
        assert_eq!(buf.detected_format(), SseFormat::OpenAi);
    }

    // ── format_sse_warning_chunk tests ───────────────────────────────────

    #[test]
    fn test_format_warning_chunk_openai() {
        let chunk = format_sse_warning_chunk(SseFormat::OpenAi, "[Warning] Test");
        assert!(chunk.starts_with("data: "));
        assert!(chunk.ends_with("\n\n"));
        let json_str = chunk.strip_prefix("data: ").unwrap().trim();
        let json: serde_json::Value = serde_json::from_str(json_str).unwrap();
        assert_eq!(json["object"], "chat.completion.chunk");
        let content = json["choices"][0]["delta"]["content"].as_str().unwrap();
        assert!(content.contains("[Warning] Test"));
    }

    #[test]
    fn test_format_warning_chunk_anthropic() {
        let chunk = format_sse_warning_chunk(SseFormat::Anthropic, "[Warning] Test");
        assert!(chunk.starts_with("event: content_block_delta\n"));
        let data_line = chunk.lines().find(|l| l.starts_with("data: ")).unwrap();
        let json: serde_json::Value =
            serde_json::from_str(data_line.strip_prefix("data: ").unwrap()).unwrap();
        assert_eq!(json["type"], "content_block_delta");
        let text = json["delta"]["text"].as_str().unwrap();
        assert!(text.contains("[Warning] Test"));
    }

    #[test]
    fn test_format_warning_chunk_gemini() {
        let chunk = format_sse_warning_chunk(SseFormat::Gemini, "[Warning] Test");
        assert!(chunk.starts_with("data: "));
        let json_str = chunk.strip_prefix("data: ").unwrap().trim();
        let json: serde_json::Value = serde_json::from_str(json_str).unwrap();
        let text = json["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .unwrap();
        assert!(text.contains("[Warning] Test"));
    }

    #[test]
    fn test_format_warning_chunk_unknown_uses_openai() {
        let chunk = format_sse_warning_chunk(SseFormat::Unknown, "test");
        assert!(chunk.starts_with("data: "));
        let json_str = chunk.strip_prefix("data: ").unwrap().trim();
        let json: serde_json::Value = serde_json::from_str(json_str).unwrap();
        assert_eq!(json["object"], "chat.completion.chunk");
    }

    // ── split_at_done tests ──────────────────────────────────────────────

    #[test]
    fn test_split_at_done_found() {
        let data = b"data: {\"choices\":[{\"delta\":{}}]}\n\ndata: [DONE]\n\n";
        let (before, found) = split_at_done(data);
        assert!(found);
        let before_str = std::str::from_utf8(&before).unwrap();
        assert!(!before_str.contains("[DONE]"));
        assert!(before_str.contains("choices"));
    }

    #[test]
    fn test_split_at_done_not_found() {
        let data = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n";
        let (before, found) = split_at_done(data);
        assert!(!found);
        assert_eq!(before.len(), data.len());
    }

    #[test]
    fn test_split_at_done_only_done() {
        let data = b"data: [DONE]\n\n";
        let (before, found) = split_at_done(data);
        assert!(found);
        assert!(before.is_empty());
    }

    #[test]
    fn test_split_at_done_no_space() {
        let data = b"data:[DONE]\n\n";
        let (before, found) = split_at_done(data);
        assert!(found);
        assert!(before.is_empty());
    }

    // ── SseContentDecryptor tests ───────────────────────────────────────

    #[test]
    fn test_content_decryptor_no_mappings_passthrough() {
        let empty = RequestMappings::new(uuid::Uuid::new_v4());
        let mut dec = SseContentDecryptor::new();
        let data =
            Bytes::from("data: {\"choices\":[{\"delta\":{\"content\":\"Hello world\"}}]}\n\n");
        let result = dec.feed(&data, &empty);
        assert_eq!(
            String::from_utf8_lossy(&result),
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello world\"}}]}\n\n"
        );
    }

    #[test]
    fn test_content_decryptor_single_event_replacement() {
        let m = mappings_with(&[("131-464-5515", "415-555-0198")]);
        let mut dec = SseContentDecryptor::new();

        // Single event with complete ciphertext
        let data = Bytes::from(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Call 131-464-5515 now\"}}]}\n\n",
        );
        let out = dec.feed(&data, &m);
        let flush = dec.flush(&m);

        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&out),
            String::from_utf8_lossy(&flush),
        );
        assert!(
            combined.contains("415-555-0198"),
            "Ciphertext should be replaced: {}",
            combined
        );
        assert!(
            !combined.contains("131-464-5515"),
            "Ciphertext should not remain: {}",
            combined
        );
    }

    #[test]
    fn test_content_decryptor_cross_delta_replacement() {
        let m = mappings_with(&[("131-464-5515", "415-555-0198")]);
        let mut dec = SseContentDecryptor::new();

        // Phone number split across 3 delta events (mimics real LLM streaming)
        let frame1 =
            Bytes::from("data: {\"choices\":[{\"delta\":{\"content\":\"Call 131\"}}]}\n\n");
        let frame2 = Bytes::from("data: {\"choices\":[{\"delta\":{\"content\":\"-464\"}}]}\n\n");
        let frame3 =
            Bytes::from("data: {\"choices\":[{\"delta\":{\"content\":\"-5515 now\"}}]}\n\n");

        let out1 = dec.feed(&frame1, &m);
        let out2 = dec.feed(&frame2, &m);
        let out3 = dec.feed(&frame3, &m);
        let flush = dec.flush(&m);

        let combined = format!(
            "{}{}{}{}",
            String::from_utf8_lossy(&out1),
            String::from_utf8_lossy(&out2),
            String::from_utf8_lossy(&out3),
            String::from_utf8_lossy(&flush),
        );

        // Extract text content from all synthetic SSE events
        let mut extracted_text = String::new();
        for line in combined.lines() {
            let data = line.strip_prefix("data: ").unwrap_or("");
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                if let Some(c) = json["choices"][0]["delta"]["content"].as_str() {
                    extracted_text.push_str(c);
                }
            }
        }

        assert!(
            extracted_text.contains("415-555-0198"),
            "Cross-delta ciphertext should be replaced: {:?}",
            extracted_text
        );
        assert!(
            !extracted_text.contains("131-464-5515"),
            "Ciphertext should not remain: {:?}",
            extracted_text
        );
    }

    #[test]
    fn test_content_decryptor_hash_token_replacement() {
        let m = mappings_with(&[("PER_a7f2", "Chris Wise")]);
        let mut dec = SseContentDecryptor::new();

        // Hash token split across delta events
        let frame1 =
            Bytes::from("data: {\"choices\":[{\"delta\":{\"content\":\"Dear PER_\"}}]}\n\n");
        let frame2 =
            Bytes::from("data: {\"choices\":[{\"delta\":{\"content\":\"a7f2, congrats\"}}]}\n\n");

        let out1 = dec.feed(&frame1, &m);
        let out2 = dec.feed(&frame2, &m);
        let flush = dec.flush(&m);

        let combined = format!(
            "{}{}{}",
            String::from_utf8_lossy(&out1),
            String::from_utf8_lossy(&out2),
            String::from_utf8_lossy(&flush),
        );

        let mut extracted = String::new();
        for line in combined.lines() {
            let data = line.strip_prefix("data: ").unwrap_or("");
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                if let Some(c) = json["choices"][0]["delta"]["content"].as_str() {
                    extracted.push_str(c);
                }
            }
        }

        assert!(
            extracted.contains("Chris Wise"),
            "Hash token should be replaced: {:?}",
            extracted
        );
        assert!(
            !extracted.contains("PER_a7f2"),
            "Hash token should not remain: {:?}",
            extracted
        );
    }

    #[test]
    fn test_content_decryptor_passes_through_non_content_events() {
        let m = mappings_with(&[("SECRET", "plaintext")]);
        let mut dec = SseContentDecryptor::new();

        // Non-content event (e.g., message_start) should pass through
        let frame = Bytes::from(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\"}}\n\n",
        );
        let result = dec.feed(&frame, &m);
        let result_str = String::from_utf8_lossy(&result);
        assert!(
            result_str.contains("message_start"),
            "Non-content event should pass through: {}",
            result_str
        );
    }

    #[test]
    fn test_content_decryptor_skips_done() {
        let m = mappings_with(&[("SECRET", "plaintext")]);
        let mut dec = SseContentDecryptor::new();

        let frame = Bytes::from("data: [DONE]\n\n");
        let result = dec.feed(&frame, &m);
        let result_str = String::from_utf8_lossy(&result);
        assert!(
            !result_str.contains("[DONE]"),
            "[DONE] should be skipped by content decryptor"
        );
    }

    #[test]
    fn test_content_decryptor_detects_format() {
        let empty = RequestMappings::new(uuid::Uuid::new_v4());
        let mut dec = SseContentDecryptor::new();
        assert_eq!(dec.detected_format(), SseFormat::Unknown);

        let frame = Bytes::from("data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n");
        let _ = dec.feed(&frame, &empty);
        assert_eq!(dec.detected_format(), SseFormat::OpenAi);
    }

    #[test]
    fn test_content_decryptor_anthropic_format() {
        let m = mappings_with(&[("CIPHER", "PLAIN")]);
        let mut dec = SseContentDecryptor::new();

        let frame = Bytes::from(
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"value is CIPHER here\"}}\n\n",
        );
        let out = dec.feed(&frame, &m);
        let flush = dec.flush(&m);

        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&out),
            String::from_utf8_lossy(&flush),
        );
        assert_eq!(dec.detected_format(), SseFormat::Anthropic);

        // Extract text from Anthropic format
        let mut extracted = String::new();
        for line in combined.lines() {
            let data = line.strip_prefix("data: ").unwrap_or("");
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                if let Some(t) = json["delta"]["text"].as_str() {
                    extracted.push_str(t);
                }
            }
        }
        assert!(
            extracted.contains("PLAIN"),
            "Anthropic content should be decrypted: {:?}",
            extracted
        );
    }

    #[test]
    fn test_content_decryptor_flush_emits_remaining() {
        let m = mappings_with(&[("ABCDEFGHIJKL", "replaced!")]);
        let mut dec = SseContentDecryptor::new();

        // Feed content shorter than max_ct_len — should be buffered
        let frame = Bytes::from("data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n");
        let out = dec.feed(&frame, &m);
        assert!(out.is_empty(), "Short content should be buffered");
        assert!(dec.has_pending());

        // Flush should emit the buffered content
        let flush = dec.flush(&m);
        assert!(!flush.is_empty(), "Flush should emit buffered content");
        assert!(!dec.has_pending());
    }

    #[test]
    fn test_format_sse_delta_openai() {
        let chunk = format_sse_delta(SseFormat::OpenAi, "Hello");
        assert!(chunk.starts_with("data: "));
        assert!(chunk.ends_with("\n\n"));
        let json_str = chunk.strip_prefix("data: ").unwrap().trim();
        let json: serde_json::Value = serde_json::from_str(json_str).unwrap();
        assert_eq!(json["choices"][0]["delta"]["content"], "Hello");
    }

    #[test]
    fn test_format_sse_delta_anthropic() {
        let chunk = format_sse_delta(SseFormat::Anthropic, "Hello");
        assert!(chunk.starts_with("event: content_block_delta\n"));
        let data_line = chunk.lines().find(|l| l.starts_with("data: ")).unwrap();
        let json: serde_json::Value =
            serde_json::from_str(data_line.strip_prefix("data: ").unwrap()).unwrap();
        assert_eq!(json["delta"]["text"], "Hello");
    }

    // --- Partial-line (cross-chunk) tests ---

    #[test]
    fn test_content_decryptor_partial_line_across_chunks() {
        let m = mappings_with(&[("709-103-6674", "415-555-1022")]);
        let mut dec = SseContentDecryptor::new();

        // Chunk 1: partial JSON line (split mid-JSON)
        let chunk1 = b"data: {\"choices\":[{\"delta\":{\"conten";
        let out1 = dec.feed(chunk1, &m);
        assert!(
            out1.is_empty(),
            "Partial line should be buffered, not emitted"
        );

        // Chunk 2: rest of JSON + complete event
        let chunk2 = b"t\":\"Call HR at 709-103-6674\"}}]}\n\ndata: [DONE]\n\n";
        let _out2 = dec.feed(chunk2, &m);

        // Flush should decrypt the phone number
        let flush = dec.flush(&m);
        let flush_str = String::from_utf8(flush.to_vec()).unwrap();
        assert!(
            flush_str.contains("415-555-1022"),
            "Partial-line reassembly should allow FPE decryption. Got: {}",
            flush_str
        );
    }

    #[test]
    fn test_content_decryptor_multiple_partial_chunks() {
        let m = mappings_with(&[("CIPHER", "plain")]);
        let mut dec = SseContentDecryptor::new();

        // Chunk splits across three pieces
        dec.feed(b"data: {\"choices\":", &m);
        dec.feed(b"[{\"delta\":{\"content\":", &m);
        dec.feed(b"\"has CIPHER in it\"}}]}\n\n", &m);

        let flush = dec.flush(&m);
        let s = String::from_utf8(flush.to_vec()).unwrap();
        assert!(
            s.contains("has plain in it"),
            "Multi-chunk reassembly should work. Got: {}",
            s
        );
    }

    #[test]
    fn test_ri_buffer_partial_line_done_detection() {
        let mut buf = SseRiBuffer::new();

        // [DONE] split across chunks
        buf.feed_sse_data(b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\nda");
        assert!(!buf.has_seen_done(), "Partial 'da' should not trigger done");

        buf.feed_sse_data(b"ta: [DONE]\n\n");
        assert!(
            buf.has_seen_done(),
            "Reassembled 'data: [DONE]' should trigger done"
        );

        let text = buf.take_text().unwrap();
        assert_eq!(text, "hi");
    }

    #[test]
    fn test_content_decryptor_complete_lines_no_buffer() {
        let m = mappings_with(&[("ABC", "xyz")]);
        let mut dec = SseContentDecryptor::new();

        // Complete event ending with \n\n — no partial line
        let chunk = b"data: {\"choices\":[{\"delta\":{\"content\":\"has ABC\"}}]}\n\n";
        let out = dec.feed(chunk, &m);
        assert!(out.is_empty(), "Content should be buffered for flush");

        let flush = dec.flush(&m);
        let s = String::from_utf8(flush.to_vec()).unwrap();
        assert!(s.contains("has xyz"), "Flush should decrypt. Got: {}", s);
    }
}

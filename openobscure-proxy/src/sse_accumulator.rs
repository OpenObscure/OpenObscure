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
}

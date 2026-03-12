//! PII scrubbing writer for tracing output — defense-in-depth.
//!
//! Wraps any `MakeWriter` to scrub PII patterns (SSN, CC, email, phone, API key)
//! from formatted log output before it reaches the final destination (stderr, file).
//! This catches accidental PII leaks from developer mistakes or third-party crates.

use std::io::{self, Write};
use std::sync::Arc;

use regex::Regex;

/// Compiled PII scrub patterns: (regex, replacement label).
pub fn pii_scrub_patterns() -> Vec<(Regex, &'static str)> {
    vec![
        // SSN: 123-45-6789 or 123 45 6789
        (
            Regex::new(r"\b\d{3}[-\s]\d{2}[-\s]\d{4}\b").unwrap(),
            "[REDACTED-SSN]",
        ),
        // Credit card: 13-19 digits with optional separators
        (
            Regex::new(r"\b(?:\d{4}[-\s]?){3,4}\d{1,4}\b").unwrap(),
            "[REDACTED-CC]",
        ),
        // Email
        (
            Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b").unwrap(),
            "[REDACTED-EMAIL]",
        ),
        // Phone: various formats with separators or + prefix
        (
            Regex::new(r"(?:\+\d{1,3}[-.\s]?)?\(?\d{3}\)?[-.\s]\d{3}[-.\s]\d{4}\b").unwrap(),
            "[REDACTED-PHONE]",
        ),
        // API keys: sk-..., sk_live_..., etc (32+ chars)
        (
            Regex::new(r"\b(?:sk[-_](?:live|test|proj)[-_])[A-Za-z0-9]{20,}\b").unwrap(),
            "[REDACTED-KEY]",
        ),
    ]
}

/// A `MakeWriter` wrapper that scrubs PII from log output.
pub struct PiiScrubMakeWriter<M> {
    inner: M,
    patterns: Arc<Vec<(Regex, &'static str)>>,
}

impl<M> PiiScrubMakeWriter<M> {
    pub fn new(inner: M) -> Self {
        Self {
            inner,
            patterns: Arc::new(pii_scrub_patterns()),
        }
    }
}

impl<'a, M> tracing_subscriber::fmt::MakeWriter<'a> for PiiScrubMakeWriter<M>
where
    M: tracing_subscriber::fmt::MakeWriter<'a>,
{
    type Writer = PiiScrubWriter<M::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        PiiScrubWriter {
            inner: self.inner.make_writer(),
            patterns: Arc::clone(&self.patterns),
            buffer: Vec::with_capacity(256),
        }
    }
}

/// Buffering writer that scrubs PII on drop.
///
/// Collects all writes during a single log event, then scrubs and forwards
/// the output when the writer is dropped (end of event formatting).
pub struct PiiScrubWriter<W: Write> {
    inner: W,
    patterns: Arc<Vec<(Regex, &'static str)>>,
    buffer: Vec<u8>,
}

impl<W: Write> Write for PiiScrubWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        // Flush is a no-op; real flush happens on drop
        Ok(())
    }
}

impl<W: Write> Drop for PiiScrubWriter<W> {
    fn drop(&mut self) {
        if self.buffer.is_empty() {
            return;
        }

        let text = String::from_utf8_lossy(&self.buffer);
        let mut scrubbed = text.into_owned();
        for (pattern, replacement) in self.patterns.iter() {
            scrubbed = pattern.replace_all(&scrubbed, *replacement).into_owned();
        }
        let _ = self.inner.write_all(scrubbed.as_bytes());
        let _ = self.inner.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: run a string through PII scrub patterns.
    fn scrub(input: &str) -> String {
        let patterns = pii_scrub_patterns();
        let mut result = input.to_string();
        for (pattern, replacement) in &patterns {
            result = pattern.replace_all(&result, *replacement).into_owned();
        }
        result
    }

    #[test]
    fn test_scrub_ssn() {
        assert_eq!(
            scrub("User SSN is 123-45-6789"),
            "User SSN is [REDACTED-SSN]"
        );
    }

    #[test]
    fn test_scrub_ssn_spaces() {
        assert_eq!(scrub("SSN: 123 45 6789"), "SSN: [REDACTED-SSN]");
    }

    #[test]
    fn test_scrub_email() {
        assert_eq!(
            scrub("Contact user@example.com for info"),
            "Contact [REDACTED-EMAIL] for info"
        );
    }

    #[test]
    fn test_scrub_phone() {
        assert_eq!(scrub("Call (555) 123-4567"), "Call [REDACTED-PHONE]");
    }

    #[test]
    fn test_scrub_api_key() {
        let input = "Key: sk-live-abc123def456ghi789jkl012mno345pqrs";
        let result = scrub(input);
        assert!(result.contains("[REDACTED-KEY]"));
        assert!(!result.contains("sk-live-"));
    }

    #[test]
    fn test_scrub_clean_text_unchanged() {
        let input = "INFO proxy: Incoming request method=POST uri=/anthropic/v1/messages";
        assert_eq!(scrub(input), input);
    }

    #[test]
    fn test_scrub_multiple_pii() {
        let input = "SSN 123-45-6789 and email user@test.com";
        let result = scrub(input);
        assert!(result.contains("[REDACTED-SSN]"));
        assert!(result.contains("[REDACTED-EMAIL]"));
        assert!(!result.contains("123-45-6789"));
        assert!(!result.contains("user@test.com"));
    }

    #[test]
    fn test_pii_scrub_writer_buffers_and_scrubs() {
        use std::sync::Mutex;

        let output: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let output_clone = Arc::clone(&output);

        {
            let mut writer = PiiScrubWriter {
                inner: SharedWriter(output_clone),
                patterns: Arc::new(pii_scrub_patterns()),
                buffer: Vec::new(),
            };
            write!(writer, "User SSN is 123-45-6789 end").unwrap();
        }
        // Writer dropped — buffer scrubbed and flushed
        let result = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        assert_eq!(result, "User SSN is [REDACTED-SSN] end");
    }

    /// Test helper: a thread-safe Write impl backed by a shared Vec<u8>.
    struct SharedWriter(Arc<std::sync::Mutex<Vec<u8>>>);

    impl Write for SharedWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}

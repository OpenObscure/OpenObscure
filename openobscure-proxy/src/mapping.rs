use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use uuid::Uuid;

use crate::pii_types::PiiType;

/// A single FPE substitution record.
#[derive(Debug, Clone)]
pub struct FpeMapping {
    pub pii_type: PiiType,
    pub plaintext: String,
    pub ciphertext: String,
    pub tweak: Vec<u8>,
    /// Which FPE key version was used to encrypt this mapping.
    pub key_version: u32,
}

/// All FPE mappings for a single proxied request-response cycle.
#[derive(Debug, Clone)]
pub struct RequestMappings {
    pub request_id: Uuid,
    pub created_at: std::time::Instant,
    /// Ciphertext -> FpeMapping for fast response-path lookup.
    pub by_ciphertext: HashMap<String, FpeMapping>,
}

impl RequestMappings {
    pub fn new(request_id: Uuid) -> Self {
        Self {
            request_id,
            created_at: std::time::Instant::now(),
            by_ciphertext: HashMap::new(),
        }
    }

    pub fn insert(&mut self, mapping: FpeMapping) {
        self.by_ciphertext
            .insert(mapping.ciphertext.clone(), mapping);
    }

    pub fn is_empty(&self) -> bool {
        self.by_ciphertext.is_empty()
    }

    /// Maximum ciphertext length across all mappings.
    ///
    /// Used to size the SSE accumulation buffer window: bytes within
    /// `max_ciphertext_len` of the end of a frame could be the start
    /// of a split ciphertext.
    pub fn max_ciphertext_len(&self) -> usize {
        self.by_ciphertext
            .keys()
            .map(|k| k.len())
            .max()
            .unwrap_or(0)
    }

    /// Replace all ciphertexts found in the response text with their original plaintexts.
    /// Sorts by ciphertext length descending to avoid partial matches.
    ///
    /// Normalizes Unicode dash variants (en-dash, em-dash, non-breaking hyphen, figure dash)
    /// to ASCII hyphens before matching, since LLMs commonly substitute these in responses.
    pub fn decrypt_response(&self, response_text: &str) -> String {
        let mut result = normalize_unicode_dashes(response_text);
        let mut replaced_count = 0u32;
        let mut mappings: Vec<&FpeMapping> = self.by_ciphertext.values().collect();
        mappings.sort_by(|a, b| b.ciphertext.len().cmp(&a.ciphertext.len()));
        for mapping in &mappings {
            if result.contains(&mapping.ciphertext) {
                result = result.replace(&mapping.ciphertext, &mapping.plaintext);
                replaced_count += 1;
            }
        }
        if replaced_count == 0 && !mappings.is_empty() {
            // Log first 3 ciphertexts and first 300 chars of response for debugging
            let sample_cts: Vec<String> = mappings
                .iter()
                .take(3)
                .map(|m| format!("{}→{} ({:?})", m.ciphertext, m.plaintext, m.pii_type))
                .collect();
            let preview: String = result.chars().take(300).collect();
            oo_info!(crate::oo_log::modules::MAPPING, "decrypt_response: zero ciphertexts matched",
                total_mappings = mappings.len(),
                sample_mappings = ?sample_cts,
                response_preview = %preview);
        } else if replaced_count > 0 {
            oo_info!(
                crate::oo_log::modules::MAPPING,
                "decrypt_response: ciphertexts replaced",
                replaced = replaced_count,
                total_mappings = mappings.len()
            );
        }
        result
    }
}

/// Global store of active request mappings, indexed by request UUID.
/// Entries are evicted after TTL to bound memory usage.
#[derive(Clone)]
pub struct MappingStore {
    inner: Arc<RwLock<HashMap<Uuid, RequestMappings>>>,
    ttl: std::time::Duration,
}

impl MappingStore {
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            ttl: std::time::Duration::from_secs(ttl_secs),
        }
    }

    pub async fn insert(&self, mappings: RequestMappings) {
        self.inner
            .write()
            .await
            .insert(mappings.request_id, mappings);
    }

    pub async fn get(&self, request_id: &Uuid) -> Option<RequestMappings> {
        self.inner.read().await.get(request_id).cloned()
    }

    pub async fn remove(&self, request_id: &Uuid) {
        self.inner.write().await.remove(request_id);
    }

    /// Background task: evict expired mappings every 60 seconds.
    pub async fn eviction_loop(&self) {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            let now = std::time::Instant::now();
            let mut store = self.inner.write().await;
            let before = store.len();
            store.retain(|_, mappings| now.duration_since(mappings.created_at) < self.ttl);
            let evicted = before - store.len();
            if evicted > 0 {
                oo_debug!(
                    crate::oo_log::modules::MAPPING,
                    "Evicted expired mappings",
                    evicted,
                    remaining = store.len()
                );
            }
        }
    }
}

/// Normalize Unicode dash/hyphen variants to ASCII hyphen-minus (U+002D).
///
/// LLMs frequently substitute ASCII hyphens with typographic alternatives
/// (en-dash, non-breaking hyphen, figure dash, etc.) in formatted output,
/// which breaks exact-match FPE ciphertext replacement.
fn normalize_unicode_dashes(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\u{2010}' // HYPHEN
            | '\u{2011}' // NON-BREAKING HYPHEN
            | '\u{2012}' // FIGURE DASH
            | '\u{2013}' // EN DASH
            | '\u{2014}' // EM DASH
            | '\u{2015}' // HORIZONTAL BAR
            | '\u{FE58}' // SMALL EM DASH
            | '\u{FE63}' // SMALL HYPHEN-MINUS
            | '\u{FF0D}' // FULLWIDTH HYPHEN-MINUS
            => result.push('-'),
            _ => result.push(ch),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decrypt_response() {
        let mut mappings = RequestMappings::new(Uuid::new_v4());
        mappings.insert(FpeMapping {
            pii_type: PiiType::Ssn,
            plaintext: "123-45-6789".to_string(),
            ciphertext: "847-29-3651".to_string(),
            tweak: vec![],
            key_version: 1,
        });
        mappings.insert(FpeMapping {
            pii_type: PiiType::CreditCard,
            plaintext: "4532-0151-1283-0366".to_string(),
            ciphertext: "8714-3927-6051-2483".to_string(),
            tweak: vec![],
            key_version: 1,
        });

        let response = "Your SSN 847-29-3651 and card 8714-3927-6051-2483 are on file.";
        let decrypted = mappings.decrypt_response(response);
        assert_eq!(
            decrypted,
            "Your SSN 123-45-6789 and card 4532-0151-1283-0366 are on file."
        );
    }

    #[test]
    fn test_empty_mappings() {
        let mappings = RequestMappings::new(Uuid::new_v4());
        assert!(mappings.is_empty());
        let response = "No PII here";
        assert_eq!(mappings.decrypt_response(response), response);
    }

    #[test]
    fn test_decrypt_response_unicode_dashes() {
        let mut mappings = RequestMappings::new(Uuid::new_v4());
        mappings.insert(FpeMapping {
            pii_type: PiiType::PhoneNumber,
            plaintext: "415-555-0132".to_string(),
            ciphertext: "370-133-6132".to_string(),
            tweak: vec![],
            key_version: 1,
        });

        // LLM uses non-breaking hyphens (U+2011) instead of ASCII hyphens
        let response = "Call HR at 370\u{2011}133\u{2011}6132 for details.";
        let decrypted = mappings.decrypt_response(response);
        assert_eq!(decrypted, "Call HR at 415-555-0132 for details.");
    }

    #[test]
    fn test_decrypt_response_en_dashes() {
        let mut mappings = RequestMappings::new(Uuid::new_v4());
        mappings.insert(FpeMapping {
            pii_type: PiiType::Ssn,
            plaintext: "123-45-6789".to_string(),
            ciphertext: "847-29-3651".to_string(),
            tweak: vec![],
            key_version: 1,
        });

        // LLM uses en-dashes (U+2013) instead of ASCII hyphens
        let response = "SSN: 847\u{2013}29\u{2013}3651";
        let decrypted = mappings.decrypt_response(response);
        assert_eq!(decrypted, "SSN: 123-45-6789");
    }

    #[test]
    fn test_normalize_unicode_dashes() {
        assert_eq!(normalize_unicode_dashes("no\u{2011}break"), "no-break");
        assert_eq!(normalize_unicode_dashes("en\u{2013}dash"), "en-dash");
        assert_eq!(normalize_unicode_dashes("em\u{2014}dash"), "em-dash");
        assert_eq!(normalize_unicode_dashes("plain-ascii"), "plain-ascii");
        assert_eq!(normalize_unicode_dashes("no dashes"), "no dashes");
    }
}

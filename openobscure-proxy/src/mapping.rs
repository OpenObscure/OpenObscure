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

    /// Replace all ciphertexts found in the response text with their original plaintexts.
    /// Sorts by ciphertext length descending to avoid partial matches.
    pub fn decrypt_response(&self, response_text: &str) -> String {
        let mut result = response_text.to_string();
        let mut mappings: Vec<&FpeMapping> = self.by_ciphertext.values().collect();
        mappings.sort_by(|a, b| b.ciphertext.len().cmp(&a.ciphertext.len()));
        for mapping in mappings {
            result = result.replace(&mapping.ciphertext, &mapping.plaintext);
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
                oo_debug!(crate::oo_log::modules::MAPPING, "Evicted expired mappings", evicted, remaining = store.len());
            }
        }
    }
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
}

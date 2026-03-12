use std::collections::HashMap;
use std::sync::Arc;

use regex::{NoExpand, Regex};
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
    ///
    /// For numeric PII types (phone, SSN, CC), falls back to fuzzy regex matching
    /// when exact match fails, handling LLM reformatting of separators
    /// (e.g., `370-133-6132` → `(370) 133-6132`).
    pub fn decrypt_response(&self, response_text: &str) -> String {
        let mut result = normalize_unicode_dashes(response_text);
        let mut replaced_count = 0u32;
        let mut fuzzy_count = 0u32;
        let mut mappings: Vec<&FpeMapping> = self.by_ciphertext.values().collect();
        mappings.sort_by(|a, b| b.ciphertext.len().cmp(&a.ciphertext.len()));

        // Phase 1: Exact match (fast path)
        let mut unmatched_numeric: Vec<&FpeMapping> = Vec::new();
        for mapping in &mappings {
            if result.contains(&mapping.ciphertext) {
                result = result.replace(&mapping.ciphertext, &mapping.plaintext);
                replaced_count += 1;
            } else if matches!(
                mapping.pii_type,
                PiiType::PhoneNumber | PiiType::Ssn | PiiType::CreditCard | PiiType::Iban
            ) {
                unmatched_numeric.push(mapping);
            }
        }

        // Phase 2: Fuzzy match for unmatched numeric PII (handles LLM reformatting)
        for mapping in &unmatched_numeric {
            if let Some(re) = build_fpe_fuzzy_regex(&mapping.ciphertext, &mapping.pii_type) {
                let before = result.clone();
                result = re
                    .replace_all(&result, NoExpand(&mapping.plaintext))
                    .to_string();
                if result != before {
                    replaced_count += 1;
                    fuzzy_count += 1;
                    oo_debug!(
                        crate::oo_log::modules::MAPPING,
                        "decrypt_response: fuzzy match replaced ciphertext",
                        pii_type = ?mapping.pii_type,
                        ciphertext = %mapping.ciphertext
                    );
                }
            }
        }

        if replaced_count == 0 && !mappings.is_empty() {
            // Hex-encode ciphertexts to bypass pii_scrub_layer log redaction.
            // Also check if each ciphertext's digit sequence exists anywhere in the response.
            let resp_digits: String = result.chars().filter(|c| c.is_ascii_digit()).collect();
            let ct_diag: Vec<String> = mappings
                .iter()
                .filter(|m| {
                    matches!(
                        m.pii_type,
                        PiiType::PhoneNumber | PiiType::Ssn | PiiType::CreditCard
                    )
                })
                .take(5)
                .map(|m| {
                    let ct_digits: String = m
                        .ciphertext
                        .chars()
                        .filter(|c| c.is_ascii_digit())
                        .collect();
                    let digits_found = resp_digits.contains(&ct_digits);
                    let ct_hex: String =
                        m.ciphertext.bytes().map(|b| format!("{:02x}", b)).collect();
                    let pt_hex: String =
                        m.plaintext.bytes().map(|b| format!("{:02x}", b)).collect();
                    format!(
                        "ct_hex={} pt_hex={} digits_in_resp={} type={:?}",
                        ct_hex, pt_hex, digits_found, m.pii_type
                    )
                })
                .collect();
            let token_cts: Vec<String> = mappings
                .iter()
                .filter(|m| !m.pii_type.is_fpe_eligible())
                .take(3)
                .map(|m| {
                    let ct_hex: String =
                        m.ciphertext.bytes().map(|b| format!("{:02x}", b)).collect();
                    format!("ct_hex={} type={:?}", ct_hex, m.pii_type)
                })
                .collect();
            // Hex-encode any 7+ digit sequences found in the response (to see what's there)
            let resp_phone_hex: Vec<String> = Regex::new(r"\d[\d\s.()-]{8,}\d")
                .ok()
                .map(|re| {
                    re.find_iter(&result)
                        .take(3)
                        .map(|m| {
                            m.as_str()
                                .bytes()
                                .map(|b| format!("{:02x}", b))
                                .collect::<String>()
                        })
                        .collect()
                })
                .unwrap_or_default();
            oo_debug!(crate::oo_log::modules::MAPPING, "decrypt_response: zero ciphertexts matched",
                total_mappings = mappings.len(),
                ct_diagnostics = ?ct_diag,
                token_diagnostics = ?token_cts,
                resp_phone_patterns_hex = ?resp_phone_hex,
                response_len = result.len(),
                response_text = %result);
        } else if replaced_count > 0 {
            oo_info!(
                crate::oo_log::modules::MAPPING,
                "decrypt_response: ciphertexts replaced",
                replaced = replaced_count,
                fuzzy = fuzzy_count,
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

/// Build a fuzzy regex for an FPE ciphertext to handle LLM reformatting.
///
/// LLMs frequently change separator formatting in numeric PII:
/// `370-133-6132` → `(370) 133-6132`, `370.133.6132`, `370 133 6132`, etc.
///
/// Extracts digits from the ciphertext and builds a regex that matches
/// those exact digits with any combination of common separators between groups.
pub(crate) fn build_fpe_fuzzy_regex(ciphertext: &str, pii_type: &PiiType) -> Option<Regex> {
    let digits: String = ciphertext.chars().filter(|c| c.is_ascii_digit()).collect();
    let sep = r"[-.\s]*";

    let pattern = match pii_type {
        PiiType::PhoneNumber => match digits.len() {
            10 => {
                // Two alternations: with parens `(XXX)`, or plain digits with word boundary.
                // This prevents matching digits embedded in a longer number.
                Some(format!(
                    r"(?:\({}\)|\b{}){}{}{}{}(?:\b|$)",
                    &digits[0..3],
                    &digits[0..3],
                    sep,
                    &digits[3..6],
                    sep,
                    &digits[6..10]
                ))
            }
            11 if digits.starts_with('1') => Some(format!(
                r"(?:\+?1{sep})?(?:\({}\)|\b{}){}{}{}{}(?:\b|$)",
                &digits[1..4],
                &digits[1..4],
                sep,
                &digits[4..7],
                sep,
                &digits[7..11],
                sep = sep
            )),
            _ => None,
        },
        PiiType::Ssn if digits.len() == 9 => Some(format!(
            r"\b{}{}{}{}{}(?:\b|$)",
            &digits[0..3],
            sep,
            &digits[3..5],
            sep,
            &digits[5..9]
        )),
        PiiType::CreditCard if digits.len() == 16 => Some(format!(
            r"\b{}{}{}{}{}{}{}(?:\b|$)",
            &digits[0..4],
            sep,
            &digits[4..8],
            sep,
            &digits[8..12],
            sep,
            &digits[12..16]
        )),
        PiiType::Iban => {
            // IBANs: LLMs commonly add spaces in 4-char groups (ES91 2100 0418 ...)
            let alnum: String = ciphertext.chars().filter(|c| c.is_alphanumeric()).collect();
            if alnum.len() >= 15 {
                let sep_iban = r"[\s-]?";
                let mut pattern = String::from(r"\b");
                for (i, ch) in alnum.chars().enumerate() {
                    if i > 0 && i % 4 == 0 {
                        pattern.push_str(sep_iban);
                    }
                    // Case-insensitive match for the alphabetic characters
                    if ch.is_ascii_alphabetic() {
                        pattern.push_str(&format!(
                            "[{}{}]",
                            ch.to_ascii_lowercase(),
                            ch.to_ascii_uppercase()
                        ));
                    } else {
                        pattern.push(ch);
                    }
                }
                pattern.push_str(r"\b");
                Some(pattern)
            } else {
                None
            }
        }
        _ => None,
    };

    pattern.and_then(|p| Regex::new(&p).ok())
}

/// Normalize Unicode dash/hyphen variants to ASCII hyphen-minus (U+002D).
///
/// LLMs frequently substitute ASCII hyphens with typographic alternatives
/// (en-dash, non-breaking hyphen, figure dash, etc.) in formatted output,
/// which breaks exact-match FPE ciphertext replacement.
pub(crate) fn normalize_unicode_dashes(text: &str) -> String {
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

    #[test]
    fn test_fuzzy_phone_parens_to_hyphens() {
        let mut mappings = RequestMappings::new(Uuid::new_v4());
        // Ciphertext has parens: (370) 133-6132
        mappings.insert(FpeMapping {
            pii_type: PiiType::PhoneNumber,
            plaintext: "(415) 555-0132".to_string(),
            ciphertext: "(370) 133-6132".to_string(),
            tweak: vec![],
            key_version: 1,
        });

        // LLM outputs with plain hyphens instead of parens
        let response = "Call HR at 370-133-6132 for details.";
        let decrypted = mappings.decrypt_response(response);
        assert_eq!(decrypted, "Call HR at (415) 555-0132 for details.");
    }

    #[test]
    fn test_fuzzy_phone_hyphens_to_parens() {
        let mut mappings = RequestMappings::new(Uuid::new_v4());
        // Ciphertext has hyphens: 370-133-6132
        mappings.insert(FpeMapping {
            pii_type: PiiType::PhoneNumber,
            plaintext: "415-555-0132".to_string(),
            ciphertext: "370-133-6132".to_string(),
            tweak: vec![],
            key_version: 1,
        });

        // LLM adds parens
        let response = "Call HR at (370) 133-6132 for details.";
        let decrypted = mappings.decrypt_response(response);
        assert_eq!(decrypted, "Call HR at 415-555-0132 for details.");
    }

    #[test]
    fn test_fuzzy_phone_dots() {
        let mut mappings = RequestMappings::new(Uuid::new_v4());
        mappings.insert(FpeMapping {
            pii_type: PiiType::PhoneNumber,
            plaintext: "415-555-0132".to_string(),
            ciphertext: "370-133-6132".to_string(),
            tweak: vec![],
            key_version: 1,
        });

        // LLM uses dots
        let response = "Phone: 370.133.6132";
        let decrypted = mappings.decrypt_response(response);
        assert_eq!(decrypted, "Phone: 415-555-0132");
    }

    #[test]
    fn test_fuzzy_phone_spaces() {
        let mut mappings = RequestMappings::new(Uuid::new_v4());
        mappings.insert(FpeMapping {
            pii_type: PiiType::PhoneNumber,
            plaintext: "415-555-0132".to_string(),
            ciphertext: "370-133-6132".to_string(),
            tweak: vec![],
            key_version: 1,
        });

        // LLM uses spaces
        let response = "Phone: 370 133 6132";
        let decrypted = mappings.decrypt_response(response);
        assert_eq!(decrypted, "Phone: 415-555-0132");
    }

    #[test]
    fn test_fuzzy_ssn_spaces() {
        let mut mappings = RequestMappings::new(Uuid::new_v4());
        mappings.insert(FpeMapping {
            pii_type: PiiType::Ssn,
            plaintext: "123-45-6789".to_string(),
            ciphertext: "847-29-3651".to_string(),
            tweak: vec![],
            key_version: 1,
        });

        // LLM uses spaces instead of hyphens
        let response = "SSN: 847 29 3651";
        let decrypted = mappings.decrypt_response(response);
        assert_eq!(decrypted, "SSN: 123-45-6789");
    }

    #[test]
    fn test_fuzzy_cc_spaces() {
        let mut mappings = RequestMappings::new(Uuid::new_v4());
        mappings.insert(FpeMapping {
            pii_type: PiiType::CreditCard,
            plaintext: "4532-0151-1283-0366".to_string(),
            ciphertext: "8714-3927-6051-2483".to_string(),
            tweak: vec![],
            key_version: 1,
        });

        // LLM uses spaces
        let response = "Card: 8714 3927 6051 2483";
        let decrypted = mappings.decrypt_response(response);
        assert_eq!(decrypted, "Card: 4532-0151-1283-0366");
    }

    #[test]
    fn test_exact_match_preferred_over_fuzzy() {
        let mut mappings = RequestMappings::new(Uuid::new_v4());
        mappings.insert(FpeMapping {
            pii_type: PiiType::PhoneNumber,
            plaintext: "415-555-0132".to_string(),
            ciphertext: "370-133-6132".to_string(),
            tweak: vec![],
            key_version: 1,
        });

        // Exact match — should use fast path, not fuzzy
        let response = "Phone: 370-133-6132";
        let decrypted = mappings.decrypt_response(response);
        assert_eq!(decrypted, "Phone: 415-555-0132");
    }

    #[test]
    fn test_fuzzy_regex_phone() {
        let re = build_fpe_fuzzy_regex("370-133-6132", &PiiType::PhoneNumber).unwrap();
        assert!(re.is_match("370-133-6132"));
        assert!(re.is_match("(370) 133-6132"));
        assert!(re.is_match("370.133.6132"));
        assert!(re.is_match("370 133 6132"));
        // Should not match digits embedded in longer number
        assert!(!re.is_match("13701336132"));
    }

    #[test]
    fn test_fuzzy_regex_ssn() {
        let re = build_fpe_fuzzy_regex("847-29-3651", &PiiType::Ssn).unwrap();
        assert!(re.is_match("847-29-3651"));
        assert!(re.is_match("847 29 3651"));
        assert!(re.is_match("847.29.3651"));
    }

    #[test]
    fn test_fuzzy_regex_cc() {
        let re = build_fpe_fuzzy_regex("8714-3927-6051-2483", &PiiType::CreditCard).unwrap();
        assert!(re.is_match("8714-3927-6051-2483"));
        assert!(re.is_match("8714 3927 6051 2483"));
    }

    #[test]
    fn test_decrypt_response_ipv4() {
        let mut mappings = RequestMappings::new(Uuid::new_v4());
        mappings.insert(FpeMapping {
            pii_type: PiiType::Ipv4Address,
            plaintext: "192.168.1.42".to_string(),
            ciphertext: "847.293.6.51".to_string(),
            tweak: vec![],
            key_version: 1,
        });

        let response = "Server at 847.293.6.51 is down";
        let decrypted = mappings.decrypt_response(response);
        assert_eq!(decrypted, "Server at 192.168.1.42 is down");
    }

    #[test]
    fn test_decrypt_response_gps() {
        let mut mappings = RequestMappings::new(Uuid::new_v4());
        mappings.insert(FpeMapping {
            pii_type: PiiType::GpsCoordinate,
            plaintext: "45.5231, -122.6765".to_string(),
            ciphertext: "83.7294, -651.0428".to_string(),
            tweak: vec![],
            key_version: 1,
        });

        let response = "Location: 83.7294, -651.0428";
        let decrypted = mappings.decrypt_response(response);
        assert_eq!(decrypted, "Location: 45.5231, -122.6765");
    }

    #[test]
    fn test_decrypt_response_mac() {
        let mut mappings = RequestMappings::new(Uuid::new_v4());
        mappings.insert(FpeMapping {
            pii_type: PiiType::MacAddress,
            plaintext: "00:1a:2b:3c:4d:5e".to_string(),
            ciphertext: "a3:f7:c2:9d:e1:b5".to_string(),
            tweak: vec![],
            key_version: 1,
        });

        let response = "MAC address: a3:f7:c2:9d:e1:b5";
        let decrypted = mappings.decrypt_response(response);
        assert_eq!(decrypted, "MAC address: 00:1a:2b:3c:4d:5e");
    }

    #[test]
    fn test_decrypt_response_iban_exact() {
        let mut mappings = RequestMappings::new(Uuid::new_v4());
        mappings.insert(FpeMapping {
            pii_type: PiiType::Iban,
            plaintext: "ES9121000418450200051332".to_string(),
            ciphertext: "ESa7f3c29de1b504827391".to_string(),
            tweak: vec![],
            key_version: 1,
        });

        let response = "IBAN: ESa7f3c29de1b504827391";
        let decrypted = mappings.decrypt_response(response);
        assert_eq!(decrypted, "IBAN: ES9121000418450200051332");
    }

    #[test]
    fn test_fuzzy_regex_iban() {
        let re = build_fpe_fuzzy_regex("ESa7f3c29de1b504827391", &PiiType::Iban).unwrap();
        // Exact match
        assert!(re.is_match("ESa7f3c29de1b504827391"));
        // Spaces in 4-char groups (common IBAN formatting)
        assert!(re.is_match("ESa7 f3c2 9de1 b504 8273 91"));
        // Hyphens
        assert!(re.is_match("ESa7-f3c2-9de1-b504-8273-91"));
    }

    #[test]
    fn test_decrypt_response_iban_fuzzy_spaces() {
        let mut mappings = RequestMappings::new(Uuid::new_v4());
        mappings.insert(FpeMapping {
            pii_type: PiiType::Iban,
            plaintext: "DE89370400440532013000".to_string(),
            ciphertext: "DEa7f3c29de1b5048200".to_string(),
            tweak: vec![],
            key_version: 1,
        });

        // LLM adds spaces in 4-char groups
        let response = "IBAN: DEa7 f3c2 9de1 b504 8200";
        let decrypted = mappings.decrypt_response(response);
        assert_eq!(decrypted, "IBAN: DE89370400440532013000");
    }
}

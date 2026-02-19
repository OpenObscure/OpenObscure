use std::collections::HashMap;

use aes::Aes256;
use fpe::ff1::{FlexibleNumeralString, FF1};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::pii_types::{find_api_key_prefix, AlphabetMapper, FormatTemplate, PiiType};
use crate::scanner::PiiMatch;

/// The core FPE engine. Holds pre-built FF1 instances per radix.
pub struct FpeEngine {
    ff1_radix10: FF1<Aes256>,
    ff1_radix36: FF1<Aes256>,
    ff1_radix62: FF1<Aes256>,
    mappers: HashMap<PiiType, AlphabetMapper>,
}

/// The result of encrypting a single PII match.
#[derive(Debug, Clone)]
pub struct FpeResult {
    pub original: PiiMatch,
    pub encrypted: String,
    pub tweak: Vec<u8>,
}

impl FpeEngine {
    /// Create a new FPE engine from a 32-byte AES-256 key.
    pub fn new(key: &[u8; 32]) -> Result<Self, FpeError> {
        Ok(Self {
            ff1_radix10: FF1::<Aes256>::new(key, 10).map_err(FpeError::InvalidRadix)?,
            ff1_radix36: FF1::<Aes256>::new(key, 36).map_err(FpeError::InvalidRadix)?,
            ff1_radix62: FF1::<Aes256>::new(key, 62).map_err(FpeError::InvalidRadix)?,
            mappers: crate::pii_types::build_mappers(),
        })
    }

    /// Encrypt a single PII match, returning the encrypted replacement.
    pub fn encrypt_match(&self, pii_match: &PiiMatch, tweak: &[u8]) -> Result<FpeResult, FpeError> {
        let config = pii_match.pii_type.config();
        let mapper = self
            .mappers
            .get(&pii_match.pii_type)
            .ok_or(FpeError::UnsupportedType)?;

        // Extract the encryptable portion and any preserved context
        let (encryptable, prefix, suffix) =
            extract_encryptable(&pii_match.raw_value, &pii_match.pii_type);

        // Strip formatting separators, keeping track of their positions
        let template = FormatTemplate::from_raw(&encryptable, mapper);

        // For email local parts, lowercase before encryption
        let naked = if pii_match.pii_type == PiiType::Email {
            template.naked.to_lowercase()
        } else {
            template.naked.clone()
        };

        // Convert to numerals
        let numerals = mapper
            .string_to_numerals(&naked)
            .ok_or(FpeError::InvalidCharacter)?;

        // Validate minimum domain size: radix^len >= 1,000,000
        if numerals.len() < config.min_length {
            return Err(FpeError::DomainTooSmall {
                radix: config.radix,
                len: numerals.len(),
                min_len: config.min_length,
            });
        }

        // Encrypt via FF1
        let fns = FlexibleNumeralString::from(numerals);
        let ff1 = self.get_ff1(config.radix)?;
        let encrypted_fns = ff1.encrypt(tweak, &fns).map_err(FpeError::NumeralString)?;
        let encrypted_numerals: Vec<u16> = encrypted_fns.into();

        // Convert back to string
        let encrypted_naked = mapper
            .numerals_to_string(&encrypted_numerals)
            .ok_or(FpeError::InvalidNumeral)?;

        // Re-apply formatting template (dashes, spaces, etc.)
        let encrypted_formatted = template.apply(&encrypted_naked);

        // Re-attach preserved prefix/suffix
        let final_encrypted = format!("{}{}{}", prefix, encrypted_formatted, suffix);

        Ok(FpeResult {
            original: pii_match.clone(),
            encrypted: final_encrypted,
            tweak: tweak.to_vec(),
        })
    }

    /// Decrypt a single encrypted PII value back to plaintext.
    pub fn decrypt_value(
        &self,
        encrypted: &str,
        pii_type: PiiType,
        tweak: &[u8],
    ) -> Result<String, FpeError> {
        let config = pii_type.config();
        let mapper = self
            .mappers
            .get(&pii_type)
            .ok_or(FpeError::UnsupportedType)?;

        let (encryptable, prefix, suffix) = extract_encryptable(encrypted, &pii_type);

        let template = FormatTemplate::from_raw(&encryptable, mapper);

        let naked = if pii_type == PiiType::Email {
            template.naked.to_lowercase()
        } else {
            template.naked.clone()
        };

        let numerals = mapper
            .string_to_numerals(&naked)
            .ok_or(FpeError::InvalidCharacter)?;

        let fns = FlexibleNumeralString::from(numerals);
        let ff1 = self.get_ff1(config.radix)?;
        let decrypted_fns = ff1.decrypt(tweak, &fns).map_err(FpeError::NumeralString)?;
        let decrypted_numerals: Vec<u16> = decrypted_fns.into();

        let decrypted_naked = mapper
            .numerals_to_string(&decrypted_numerals)
            .ok_or(FpeError::InvalidNumeral)?;

        let decrypted_formatted = template.apply(&decrypted_naked);
        Ok(format!("{}{}{}", prefix, decrypted_formatted, suffix))
    }

    fn get_ff1(&self, radix: u32) -> Result<&FF1<Aes256>, FpeError> {
        match radix {
            10 => Ok(&self.ff1_radix10),
            36 => Ok(&self.ff1_radix36),
            62 => Ok(&self.ff1_radix62),
            _ => Err(FpeError::UnsupportedRadix(radix)),
        }
    }
}

/// Extract the encryptable portion of a PII value, returning (encryptable, prefix, suffix).
/// - Email: encrypt local part only, preserve @domain
/// - ApiKey: encrypt post-prefix, preserve known prefix
/// - Others: encrypt the whole value
fn extract_encryptable(raw: &str, pii_type: &PiiType) -> (String, String, String) {
    match pii_type {
        PiiType::Email => {
            if let Some(at_pos) = raw.find('@') {
                let local = &raw[..at_pos];
                let domain = &raw[at_pos..]; // includes @
                (local.to_string(), String::new(), domain.to_string())
            } else {
                (raw.to_string(), String::new(), String::new())
            }
        }
        PiiType::ApiKey => {
            if let Some(prefix) = find_api_key_prefix(raw) {
                let remainder = &raw[prefix.len()..];
                (remainder.to_string(), prefix.to_string(), String::new())
            } else {
                (raw.to_string(), String::new(), String::new())
            }
        }
        _ => (raw.to_string(), String::new(), String::new()),
    }
}

/// Generate per-record tweaks to prevent frequency analysis.
pub struct TweakGenerator;

impl TweakGenerator {
    /// Generate a tweak from request ID + JSON path.
    /// Same PII in different requests or different JSON paths produces different ciphertexts.
    pub fn generate(request_id: &Uuid, json_path: &str) -> Vec<u8> {
        let mut tweak = Vec::with_capacity(32);
        tweak.extend_from_slice(request_id.as_bytes()); // 16 bytes

        let mut hasher = Sha256::new();
        hasher.update(json_path.as_bytes());
        let hash = hasher.finalize();
        tweak.extend_from_slice(&hash[..16]); // 16 bytes

        tweak
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FpeError {
    #[error("Invalid radix: {0}")]
    InvalidRadix(fpe::ff1::InvalidRadix),
    #[error("Numeral string error: {0}")]
    NumeralString(fpe::ff1::NumeralStringError),
    #[error("FPE domain too small: radix={radix}, len={len} (need min_len={min_len})")]
    DomainTooSmall {
        radix: u32,
        len: usize,
        min_len: usize,
    },
    #[error("Invalid character in FPE input")]
    InvalidCharacter,
    #[error("Invalid numeral in FPE output")]
    InvalidNumeral,
    #[error("Unsupported radix: {0}")]
    UnsupportedRadix(u32),
    #[error("Unsupported PII type for FPE")]
    UnsupportedType,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; 32] {
        // Deterministic test key — never use in production
        let mut key = [0u8; 32];
        for (i, byte) in key.iter_mut().enumerate() {
            *byte = i as u8;
        }
        key
    }

    #[test]
    fn test_fpe_roundtrip_credit_card() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        let tweak = b"test-tweak-value";

        let pii_match = PiiMatch {
            pii_type: PiiType::CreditCard,
            start: 0,
            end: 19,
            raw_value: "4532-0151-1283-0366".to_string(),
            json_path: Some("content".to_string()),
            confidence: 1.0,
        };

        let result = engine.encrypt_match(&pii_match, tweak).unwrap();
        assert_ne!(result.encrypted, pii_match.raw_value);
        // Verify format preserved (same dash positions)
        assert_eq!(result.encrypted.len(), pii_match.raw_value.len());
        assert_eq!(result.encrypted.chars().nth(4), Some('-'));
        assert_eq!(result.encrypted.chars().nth(9), Some('-'));
        assert_eq!(result.encrypted.chars().nth(14), Some('-'));

        // Decrypt and verify roundtrip
        let decrypted = engine
            .decrypt_value(&result.encrypted, PiiType::CreditCard, tweak)
            .unwrap();
        assert_eq!(decrypted, pii_match.raw_value);
    }

    #[test]
    fn test_fpe_roundtrip_ssn() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        let tweak = b"ssn-tweak";

        let pii_match = PiiMatch {
            pii_type: PiiType::Ssn,
            start: 0,
            end: 11,
            raw_value: "123-45-6789".to_string(),
            json_path: None,
            confidence: 1.0,
        };

        let result = engine.encrypt_match(&pii_match, tweak).unwrap();
        assert_ne!(result.encrypted, "123-45-6789");
        assert_eq!(result.encrypted.len(), 11);

        let decrypted = engine
            .decrypt_value(&result.encrypted, PiiType::Ssn, tweak)
            .unwrap();
        assert_eq!(decrypted, "123-45-6789");
    }

    #[test]
    fn test_fpe_roundtrip_phone() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        let tweak = b"phone-tweak";

        let pii_match = PiiMatch {
            pii_type: PiiType::PhoneNumber,
            start: 0,
            end: 14,
            raw_value: "(555) 123-4567".to_string(),
            json_path: None,
            confidence: 1.0,
        };

        let result = engine.encrypt_match(&pii_match, tweak).unwrap();
        assert_ne!(result.encrypted, "(555) 123-4567");

        let decrypted = engine
            .decrypt_value(&result.encrypted, PiiType::PhoneNumber, tweak)
            .unwrap();
        assert_eq!(decrypted, "(555) 123-4567");
    }

    #[test]
    fn test_fpe_roundtrip_email() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        let tweak = b"email-tweak";

        let pii_match = PiiMatch {
            pii_type: PiiType::Email,
            start: 0,
            end: 20,
            raw_value: "johndoe@example.com".to_string(),
            json_path: None,
            confidence: 1.0,
        };

        let result = engine.encrypt_match(&pii_match, tweak).unwrap();
        // Domain should be preserved
        assert!(result.encrypted.ends_with("@example.com"));
        assert_ne!(result.encrypted, "johndoe@example.com");

        let decrypted = engine
            .decrypt_value(&result.encrypted, PiiType::Email, tweak)
            .unwrap();
        assert_eq!(decrypted, "johndoe@example.com");
    }

    #[test]
    fn test_different_tweaks_produce_different_ciphertexts() {
        let engine = FpeEngine::new(&test_key()).unwrap();

        let pii_match = PiiMatch {
            pii_type: PiiType::Ssn,
            start: 0,
            end: 11,
            raw_value: "123-45-6789".to_string(),
            json_path: None,
            confidence: 1.0,
        };

        let result1 = engine.encrypt_match(&pii_match, b"tweak-1").unwrap();
        let result2 = engine.encrypt_match(&pii_match, b"tweak-2").unwrap();
        assert_ne!(result1.encrypted, result2.encrypted);
    }

    #[test]
    fn test_deterministic_with_same_tweak() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        let tweak = b"same-tweak";

        let pii_match = PiiMatch {
            pii_type: PiiType::Ssn,
            start: 0,
            end: 11,
            raw_value: "123-45-6789".to_string(),
            json_path: None,
            confidence: 1.0,
        };

        let result1 = engine.encrypt_match(&pii_match, tweak).unwrap();
        let result2 = engine.encrypt_match(&pii_match, tweak).unwrap();
        assert_eq!(result1.encrypted, result2.encrypted);
    }

    #[test]
    fn test_tweak_generator() {
        let id = Uuid::new_v4();
        let tweak1 = TweakGenerator::generate(&id, "messages[0].content");
        let tweak2 = TweakGenerator::generate(&id, "messages[1].content");
        assert_eq!(tweak1.len(), 32);
        assert_eq!(tweak2.len(), 32);
        // Same request ID but different paths should produce different tweaks
        assert_ne!(tweak1, tweak2);
    }
}

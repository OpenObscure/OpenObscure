use std::collections::HashMap;

use aes::Aes256;
use fpe::ff1::{FlexibleNumeralString, FF1};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::pii_types::{AlphabetMapper, FormatTemplate, PiiType};
use crate::scanner::PiiMatch;

/// Format-preserving encryption engine backed by FF1/AES-256 (NIST SP 800-38G).
///
/// One `FF1` cipher instance is pre-built per radix at construction time so that
/// key schedule computation is paid only once. Radix selection is driven by each
/// `PiiType`'s alphabet: decimal (10), hex (16), alphanumeric (36), or base-62 (62).
pub struct FpeEngine {
    ff1_radix10: FF1<Aes256>,
    ff1_radix16: FF1<Aes256>,
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
            ff1_radix16: FF1::<Aes256>::new(key, 16).map_err(FpeError::InvalidRadix)?,
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

        // Normalise to lowercase before template stripping so that uppercase hex
        // digits (e.g., 'A' in MAC "00-1A-2B-...") map to alphabet characters
        // rather than being rejected as unknown separators by FormatTemplate.
        let encryptable = if matches!(
            pii_match.pii_type,
            PiiType::Email | PiiType::Ipv6Address | PiiType::MacAddress | PiiType::Iban
        ) {
            encryptable.to_lowercase()
        } else {
            encryptable
        };

        // Strip formatting separators, keeping track of their positions
        let template = FormatTemplate::from_raw(&encryptable, mapper);
        let naked = template.naked.clone();

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

        let encryptable = if matches!(
            pii_type,
            PiiType::Email | PiiType::Ipv6Address | PiiType::MacAddress | PiiType::Iban
        ) {
            encryptable.to_lowercase()
        } else {
            encryptable
        };

        let template = FormatTemplate::from_raw(&encryptable, mapper);
        let naked = template.naked.clone();

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
            16 => Ok(&self.ff1_radix16),
            36 => Ok(&self.ff1_radix36),
            62 => Ok(&self.ff1_radix62),
            _ => Err(FpeError::UnsupportedRadix(radix)),
        }
    }
}

/// Extract the encryptable portion of a PII value, returning (encryptable, prefix, suffix).
///
/// - Email: encrypt local part only; `@domain` is preserved as suffix (domain is not PII)
/// - IBAN: preserve 2-letter country code as prefix; encrypt the numeric check + BBAN
/// - ApiKey: encrypt the entire key string — no prefix preservation, because keeping
///   `sk-ant-` or `AKIA` would reveal the key provider to the upstream model
/// - Others: encrypt the whole value with empty prefix/suffix
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
        PiiType::Iban => {
            // Preserve 2-letter country code prefix, encrypt the rest
            if raw.len() >= 2 && raw[..2].chars().all(|c| c.is_ascii_alphabetic()) {
                let prefix = raw[..2].to_uppercase();
                let rest = &raw[2..];
                (rest.to_string(), prefix, String::new())
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
    fn test_fpe_roundtrip_email_short_local() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        let tweak = b"short-email-tweak";

        // 5-char local part: "admin" — should encrypt (>= min_length 4)
        let pii_admin = PiiMatch {
            pii_type: PiiType::Email,
            start: 0,
            end: 22,
            raw_value: "admin@meridian-tech.com".to_string(),
            json_path: None,
            confidence: 1.0,
        };
        let result = engine.encrypt_match(&pii_admin, tweak).unwrap();
        assert!(result.encrypted.ends_with("@meridian-tech.com"));
        assert_ne!(result.encrypted, "admin@meridian-tech.com");
        let decrypted = engine
            .decrypt_value(&result.encrypted, PiiType::Email, tweak)
            .unwrap();
        assert_eq!(decrypted, "admin@meridian-tech.com");

        // 4-char local part: "info" — should encrypt (== min_length 4)
        let pii_info = PiiMatch {
            pii_type: PiiType::Email,
            start: 0,
            end: 22,
            raw_value: "info@meridian-tech.com".to_string(),
            json_path: None,
            confidence: 1.0,
        };
        let result = engine.encrypt_match(&pii_info, tweak).unwrap();
        assert!(result.encrypted.ends_with("@meridian-tech.com"));
        assert_ne!(result.encrypted, "info@meridian-tech.com");
        let decrypted = engine
            .decrypt_value(&result.encrypted, PiiType::Email, tweak)
            .unwrap();
        assert_eq!(decrypted, "info@meridian-tech.com");

        // 3-char local part: "dba" — DomainTooSmall (FF1 requires >= 4 numerals)
        let pii_dba = PiiMatch {
            pii_type: PiiType::Email,
            start: 0,
            end: 21,
            raw_value: "dba@meridian-tech.com".to_string(),
            json_path: None,
            confidence: 1.0,
        };
        assert!(engine.encrypt_match(&pii_dba, tweak).is_err());

        // 3-char local part: "ops" — DomainTooSmall
        let pii_ops = PiiMatch {
            pii_type: PiiType::Email,
            start: 0,
            end: 21,
            raw_value: "ops@meridian-tech.com".to_string(),
            json_path: None,
            confidence: 1.0,
        };
        assert!(engine.encrypt_match(&pii_ops, tweak).is_err());
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

    #[test]
    fn test_fpe_roundtrip_ipv4() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        let tweak = b"ipv4-tweak";

        let pii_match = PiiMatch {
            pii_type: PiiType::Ipv4Address,
            start: 0,
            end: 12,
            raw_value: "192.168.1.42".to_string(),
            json_path: None,
            confidence: 1.0,
        };

        let result = engine.encrypt_match(&pii_match, tweak).unwrap();
        assert_ne!(result.encrypted, "192.168.1.42");
        // Dots preserved at same positions
        let dots: Vec<usize> = result
            .encrypted
            .match_indices('.')
            .map(|(i, _)| i)
            .collect();
        assert_eq!(dots, vec![3, 7, 9]);

        let decrypted = engine
            .decrypt_value(&result.encrypted, PiiType::Ipv4Address, tweak)
            .unwrap();
        assert_eq!(decrypted, "192.168.1.42");
    }

    #[test]
    fn test_fpe_roundtrip_ipv6_full() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        let tweak = b"ipv6-tweak";

        let pii_match = PiiMatch {
            pii_type: PiiType::Ipv6Address,
            start: 0,
            end: 39,
            raw_value: "2001:0db8:85a3:0000:0000:8a2e:0370:7334".to_string(),
            json_path: None,
            confidence: 1.0,
        };

        let result = engine.encrypt_match(&pii_match, tweak).unwrap();
        assert_ne!(result.encrypted, pii_match.raw_value);
        // 7 colons preserved
        assert_eq!(result.encrypted.matches(':').count(), 7);

        let decrypted = engine
            .decrypt_value(&result.encrypted, PiiType::Ipv6Address, tweak)
            .unwrap();
        assert_eq!(decrypted, pii_match.raw_value);
    }

    #[test]
    fn test_fpe_roundtrip_ipv6_compressed() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        let tweak = b"ipv6c-tweak";

        let pii_match = PiiMatch {
            pii_type: PiiType::Ipv6Address,
            start: 0,
            end: 15,
            raw_value: "fe80::1234:5678".to_string(),
            json_path: None,
            confidence: 1.0,
        };

        let result = engine.encrypt_match(&pii_match, tweak).unwrap();
        assert_ne!(result.encrypted, pii_match.raw_value);
        // :: and : positions preserved
        assert!(result.encrypted.contains("::"));

        let decrypted = engine
            .decrypt_value(&result.encrypted, PiiType::Ipv6Address, tweak)
            .unwrap();
        assert_eq!(decrypted, pii_match.raw_value);
    }

    #[test]
    fn test_fpe_roundtrip_gps() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        let tweak = b"gps-tweak";

        let pii_match = PiiMatch {
            pii_type: PiiType::GpsCoordinate,
            start: 0,
            end: 18,
            raw_value: "45.5231, -122.6765".to_string(),
            json_path: None,
            confidence: 1.0,
        };

        let result = engine.encrypt_match(&pii_match, tweak).unwrap();
        assert_ne!(result.encrypted, pii_match.raw_value);
        // Signs, dots, comma, space preserved
        assert!(result.encrypted.contains(", -"));
        assert_eq!(result.encrypted.matches('.').count(), 2);

        let decrypted = engine
            .decrypt_value(&result.encrypted, PiiType::GpsCoordinate, tweak)
            .unwrap();
        assert_eq!(decrypted, pii_match.raw_value);
    }

    #[test]
    fn test_fpe_roundtrip_mac_colon() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        let tweak = b"mac-tweak";

        let pii_match = PiiMatch {
            pii_type: PiiType::MacAddress,
            start: 0,
            end: 17,
            raw_value: "00:1a:2b:3c:4d:5e".to_string(),
            json_path: None,
            confidence: 1.0,
        };

        let result = engine.encrypt_match(&pii_match, tweak).unwrap();
        assert_ne!(result.encrypted, pii_match.raw_value);
        // 5 colons preserved
        assert_eq!(result.encrypted.matches(':').count(), 5);

        let decrypted = engine
            .decrypt_value(&result.encrypted, PiiType::MacAddress, tweak)
            .unwrap();
        assert_eq!(decrypted, pii_match.raw_value);
    }

    #[test]
    fn test_fpe_roundtrip_mac_dash() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        let tweak = b"macd-tweak";

        // Mixed case input — should be lowercased for encryption
        let pii_match = PiiMatch {
            pii_type: PiiType::MacAddress,
            start: 0,
            end: 17,
            raw_value: "00-1A-2B-3C-4D-5E".to_string(),
            json_path: None,
            confidence: 1.0,
        };

        let result = engine.encrypt_match(&pii_match, tweak).unwrap();
        assert_ne!(result.encrypted, pii_match.raw_value);
        // 5 dashes preserved
        assert_eq!(result.encrypted.matches('-').count(), 5);

        let decrypted = engine
            .decrypt_value(&result.encrypted, PiiType::MacAddress, tweak)
            .unwrap();
        // Decryption returns lowercase since we lowercase-normalize before encryption.
        // Dashes preserved at original positions; hex chars lowercase.
        assert_eq!(decrypted, "00-1a-2b-3c-4d-5e");
    }

    #[test]
    fn test_fpe_roundtrip_iban() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        let tweak = b"iban-tweak";

        let pii_match = PiiMatch {
            pii_type: PiiType::Iban,
            start: 0,
            end: 24,
            raw_value: "ES9121000418450200051332".to_string(),
            json_path: None,
            confidence: 1.0,
        };

        let result = engine.encrypt_match(&pii_match, tweak).unwrap();
        assert_ne!(result.encrypted, pii_match.raw_value);
        // Country code "ES" preserved
        assert!(result.encrypted.starts_with("ES"));
        // Same total length
        assert_eq!(result.encrypted.len(), pii_match.raw_value.len());

        let decrypted = engine
            .decrypt_value(&result.encrypted, PiiType::Iban, tweak)
            .unwrap();
        // Decrypted BBAN is lowercase (we lowercase-normalize), country code is uppercase
        assert!(decrypted.starts_with("ES"));
        // The numeric portion should match (digits are same in upper/lower)
        let orig_digits: String = pii_match.raw_value[2..]
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect();
        let dec_digits: String = decrypted[2..]
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect();
        assert_eq!(dec_digits, orig_digits);
    }

    #[test]
    fn test_fpe_roundtrip_iban_with_spaces() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        let tweak = b"ibans-tweak";

        // IBAN with space separators
        let pii_match = PiiMatch {
            pii_type: PiiType::Iban,
            start: 0,
            end: 29,
            raw_value: "DE89 3704 0044 0532 0130 00".to_string(),
            json_path: None,
            confidence: 1.0,
        };

        let result = engine.encrypt_match(&pii_match, tweak).unwrap();
        assert!(result.encrypted.starts_with("DE"));
        // Spaces preserved at same positions
        assert_eq!(result.encrypted.matches(' ').count(), 5);

        let decrypted = engine
            .decrypt_value(&result.encrypted, PiiType::Iban, tweak)
            .unwrap();
        assert!(decrypted.starts_with("DE"));
    }
}

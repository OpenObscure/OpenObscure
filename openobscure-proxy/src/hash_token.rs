//! Hash-based token generation for non-FPE PII redaction.
//!
//! Replaces indexed bracket labels like `[PERSON_0]` with short, deterministic
//! tokens like `OO_PER_a7f2` that LLMs treat as opaque identifiers and echo
//! back verbatim.  The token is derived from SHA-256(request_id || plaintext)
//! so identical plaintext within a request always maps to the same token.

use std::collections::HashMap;

use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::pii_types::PiiType;

/// Generates deterministic, collision-free redaction tokens for non-FPE PII.
pub struct TokenGenerator {
    request_id: Uuid,
    /// plaintext → generated token (dedup: same value = same token)
    seen: HashMap<String, String>,
    /// token → plaintext (collision detection)
    used_tokens: HashMap<String, String>,
}

impl TokenGenerator {
    pub fn new(request_id: Uuid) -> Self {
        Self {
            request_id,
            seen: HashMap::new(),
            used_tokens: HashMap::new(),
        }
    }

    /// Generate a redaction token for a non-FPE PII match.
    ///
    /// - Same plaintext within a request → same token (deterministic dedup).
    /// - Different plaintexts → different tokens (collision resolution via suffix).
    pub fn generate(&mut self, pii_type: PiiType, plaintext: &str) -> String {
        // Dedup key includes PII type so same text as different types gets different tokens
        let prefix = pii_type.hash_token_prefix();
        let dedup_key = format!("{}:{}", prefix, plaintext);

        if let Some(token) = self.seen.get(&dedup_key) {
            return token.clone();
        }

        let hash = Self::compute_hash(&self.request_id, plaintext);
        let base_token = format!("OO_{}_{}", prefix, hash);

        let token = self.resolve_collision(base_token, plaintext);

        self.seen.insert(dedup_key, token.clone());
        self.used_tokens
            .insert(token.clone(), plaintext.to_string());
        token
    }

    /// SHA-256(request_id || plaintext) → first 4 bytes → base36 (4 chars).
    fn compute_hash(request_id: &Uuid, plaintext: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(request_id.as_bytes());
        hasher.update(plaintext.as_bytes());
        let digest = hasher.finalize();

        encode_base36_4(&digest[..4])
    }

    /// If `base_token` collides with a different plaintext, append b/c/d/… suffix.
    fn resolve_collision(&self, base: String, plaintext: &str) -> String {
        match self.used_tokens.get(&base) {
            None => return base,
            Some(existing) if existing == plaintext => return base,
            _ => {}
        }
        for suffix in b'b'..=b'z' {
            let candidate = format!("{}{}", base, suffix as char);
            match self.used_tokens.get(&candidate) {
                None => return candidate,
                Some(existing) if existing == plaintext => return candidate,
                _ => {}
            }
        }
        // Extremely unlikely: 25 collisions for the same 4-char hash.
        // Fall back to a longer hash to guarantee uniqueness.
        let mut hasher = Sha256::new();
        hasher.update(base.as_bytes());
        hasher.update(plaintext.as_bytes());
        let digest = hasher.finalize();
        format!("{}_{}", base, encode_base36_4(&digest[..4]))
    }
}

/// Encode 4 bytes as a 4-character base36 string (0-9, a-z).
fn encode_base36_4(bytes: &[u8]) -> String {
    let val = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let mut result = [b'0'; 4];
    let mut v = val;
    for ch in result.iter_mut().rev() {
        *ch = BASE36_CHARS[(v % 36) as usize];
        v /= 36;
    }
    // Safety: all chars are ASCII
    String::from_utf8(result.to_vec()).unwrap()
}

const BASE36_CHARS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

#[cfg(test)]
mod tests {
    use super::*;

    fn test_uuid() -> Uuid {
        Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap()
    }

    #[test]
    fn test_generate_deterministic() {
        let mut gen = TokenGenerator::new(test_uuid());
        let t1 = gen.generate(PiiType::Person, "John Smith");
        let t2 = gen.generate(PiiType::Person, "John Smith");
        assert_eq!(t1, t2, "Same plaintext must produce same token");
    }

    #[test]
    fn test_generate_unique_for_different_inputs() {
        let mut gen = TokenGenerator::new(test_uuid());
        let t1 = gen.generate(PiiType::Person, "John Smith");
        let t2 = gen.generate(PiiType::Person, "Jane Doe");
        assert_ne!(t1, t2, "Different plaintexts must produce different tokens");
    }

    #[test]
    fn test_token_format() {
        let mut gen = TokenGenerator::new(test_uuid());
        let token = gen.generate(PiiType::Person, "Alice");
        assert!(
            token.starts_with("OO_PER_"),
            "Token must start with OO_PER_: {}",
            token
        );
        // Base token: "OO_" (3) + "PER" (3) + "_" (1) + hash (4) = 11 chars
        assert!(token.len() >= 11, "Token too short: {}", token);
        assert!(token.len() <= 13, "Token too long: {}", token);
    }

    #[test]
    fn test_token_prefixes() {
        let mut gen = TokenGenerator::new(test_uuid());
        assert!(gen
            .generate(PiiType::Location, "NYC")
            .starts_with("OO_LOC_"));
        assert!(gen
            .generate(PiiType::Organization, "ACME")
            .starts_with("OO_ORG_"));
        assert!(gen
            .generate(PiiType::HealthKeyword, "diabetes")
            .starts_with("OO_HLT_"));
        assert!(gen
            .generate(PiiType::ChildKeyword, "minor")
            .starts_with("OO_CHL_"));
        assert!(gen
            .generate(PiiType::Ipv4Address, "192.168.1.1")
            .starts_with("OO_IP4_"));
        assert!(gen
            .generate(PiiType::Ipv6Address, "::1")
            .starts_with("OO_IP6_"));
        assert!(gen
            .generate(PiiType::GpsCoordinate, "40.7,-74.0")
            .starts_with("OO_GPS_"));
        assert!(gen
            .generate(PiiType::MacAddress, "AA:BB:CC:DD:EE:FF")
            .starts_with("OO_MAC_"));
    }

    #[test]
    fn test_different_request_ids_different_tokens() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let mut gen1 = TokenGenerator::new(id1);
        let mut gen2 = TokenGenerator::new(id2);
        let t1 = gen1.generate(PiiType::Person, "John Smith");
        let t2 = gen2.generate(PiiType::Person, "John Smith");
        assert_ne!(
            t1, t2,
            "Different request IDs must produce different tokens"
        );
    }

    #[test]
    fn test_collision_resolution() {
        let mut gen = TokenGenerator::new(test_uuid());

        // Manually insert a token to simulate collision
        let token_a = gen.generate(PiiType::Person, "Alice");

        // Force collision by inserting a fake entry with the hash of "Bob"
        let prefix = PiiType::Person.hash_token_prefix();
        let hash_bob = TokenGenerator::compute_hash(&test_uuid(), "Bob");
        let fake_token = format!("OO_{}_{}", prefix, hash_bob);
        gen.used_tokens
            .insert(fake_token.clone(), "NotBob".to_string());

        // Now generate for "Bob" — should get suffix since hash is taken
        let token_b = gen.generate(PiiType::Person, "Bob");
        assert_ne!(
            token_b, fake_token,
            "Collision should be resolved with suffix"
        );
        assert!(
            token_b.starts_with(&fake_token),
            "Suffixed token should extend the base: {} vs {}",
            token_b,
            fake_token
        );

        // Token for Alice should still be unchanged
        let token_a2 = gen.generate(PiiType::Person, "Alice");
        assert_eq!(token_a, token_a2);
    }

    #[test]
    fn test_encode_base36_4() {
        // Zero → "0000"
        assert_eq!(encode_base36_4(&[0, 0, 0, 0]), "0000");
        // Small value
        let result = encode_base36_4(&[0, 0, 0, 1]);
        assert_eq!(result, "0001");
        // All results should be 4 chars
        assert_eq!(encode_base36_4(&[0xff, 0xff, 0xff, 0xff]).len(), 4);
    }

    #[test]
    fn test_multiple_types_same_plaintext() {
        let mut gen = TokenGenerator::new(test_uuid());
        // Same text as different PII types should get different prefixes
        // but may or may not collide on hash (they use same plaintext+request_id)
        let t_per = gen.generate(PiiType::Person, "test");
        let t_loc = gen.generate(PiiType::Location, "test");
        // Tokens should differ because prefix differs
        assert_ne!(t_per, t_loc);
        assert!(t_per.starts_with("OO_PER_"));
        assert!(t_loc.starts_with("OO_LOC_"));
    }
}

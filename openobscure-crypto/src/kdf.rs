//! Argon2id key derivation function.
//!
//! Derives a 32-byte AES-256 key from a user passphrase using Argon2id
//! (winner of the Password Hashing Competition, recommended by OWASP).

use argon2::{Algorithm, Argon2, Params, Version};
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::CryptoError;

/// KDF parameters stored alongside encrypted data for reproducible derivation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KdfParams {
    /// 16-byte random salt (base64-encoded in serialized form)
    pub salt: Vec<u8>,
    /// Memory cost in KiB (default: 19456 = 19MB — OWASP minimum for Argon2id)
    pub memory_kib: u32,
    /// Time cost / iterations (default: 2)
    pub iterations: u32,
    /// Parallelism (default: 1 — single-threaded for portability)
    pub parallelism: u32,
}

impl KdfParams {
    /// Generate new KDF params with a random salt.
    pub fn new() -> Self {
        let mut salt = vec![0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut salt);
        Self {
            salt,
            memory_kib: 19456, // 19MB — OWASP minimum
            iterations: 2,
            parallelism: 1,
        }
    }
}

impl Default for KdfParams {
    fn default() -> Self {
        Self::new()
    }
}

/// Derive a 32-byte AES-256 key from a passphrase using Argon2id.
pub fn derive_key(passphrase: &str, params: &KdfParams) -> Result<[u8; 32], CryptoError> {
    let argon2_params = Params::new(
        params.memory_kib,
        params.iterations,
        params.parallelism,
        Some(32), // 32-byte output
    )
    .map_err(|e| CryptoError::Kdf(format!("Invalid Argon2id params: {}", e)))?;

    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, argon2_params);

    let mut key = [0u8; 32];
    argon2
        .hash_password_into(passphrase.as_bytes(), &params.salt, &mut key)
        .map_err(|e| CryptoError::Kdf(format!("Argon2id derivation failed: {}", e)))?;

    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_key_deterministic() {
        let params = KdfParams {
            salt: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
            memory_kib: 19456,
            iterations: 2,
            parallelism: 1,
        };
        let key1 = derive_key("test-passphrase", &params).unwrap();
        let key2 = derive_key("test-passphrase", &params).unwrap();
        assert_eq!(key1, key2);
    }

    #[test]
    fn test_different_passphrases_different_keys() {
        let params = KdfParams {
            salt: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
            memory_kib: 19456,
            iterations: 2,
            parallelism: 1,
        };
        let key1 = derive_key("passphrase-a", &params).unwrap();
        let key2 = derive_key("passphrase-b", &params).unwrap();
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_different_salts_different_keys() {
        let params1 = KdfParams {
            salt: vec![1; 16],
            memory_kib: 19456,
            iterations: 2,
            parallelism: 1,
        };
        let params2 = KdfParams {
            salt: vec![2; 16],
            memory_kib: 19456,
            iterations: 2,
            parallelism: 1,
        };
        let key1 = derive_key("same-passphrase", &params1).unwrap();
        let key2 = derive_key("same-passphrase", &params2).unwrap();
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_key_length() {
        let params = KdfParams::new();
        let key = derive_key("test", &params).unwrap();
        assert_eq!(key.len(), 32);
    }
}

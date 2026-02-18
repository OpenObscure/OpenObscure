//! OpenObscure Encryption Layer (L2)
//!
//! Provides AES-256-GCM encryption for session transcripts at rest,
//! with Argon2id KDF for deriving encryption keys from user passphrases.
//!
//! # Architecture
//!
//! - `kdf`: Argon2id key derivation from passphrase → 32-byte AES key
//! - `cipher`: AES-256-GCM encrypt/decrypt with random nonces
//! - `store`: Encrypted file storage (write/read encrypted JSON)

mod cipher;
mod kdf;
mod store;

pub use cipher::{decrypt, encrypt};
pub use kdf::{derive_key, KdfParams};
pub use store::{EncryptedStore, EncryptedTranscript};

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("KDF error: {0}")]
    Kdf(String),
    #[error("Encryption error: {0}")]
    Encrypt(String),
    #[error("Decryption error: {0}")]
    Decrypt(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Base64 decode error: {0}")]
    Base64(#[from] base64::DecodeError),
}

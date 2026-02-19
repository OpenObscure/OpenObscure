//! Encrypted file storage for session transcripts.
//!
//! Stores encrypted transcripts as JSON files containing:
//! - KDF params (salt, memory, iterations) for key re-derivation
//! - Base64-encoded ciphertext (nonce || AES-GCM ciphertext || tag)
//! - Metadata (session ID, timestamp, version)

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::kdf::KdfParams;
use crate::{cipher, kdf, CryptoError};

/// On-disk format for an encrypted transcript.
#[derive(Debug, Serialize, Deserialize)]
pub struct EncryptedTranscript {
    pub version: u32,
    pub session_id: String,
    pub created_at: String,
    pub kdf_params: KdfParams,
    pub ciphertext_b64: String,
}

/// Manages reading/writing encrypted transcript files.
pub struct EncryptedStore {
    dir: std::path::PathBuf,
}

impl EncryptedStore {
    /// Create a new store backed by a directory. Creates the directory if needed.
    pub fn new(dir: impl Into<std::path::PathBuf>) -> Result<Self, CryptoError> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// Encrypt and store a transcript.
    pub fn write(
        &self,
        session_id: &str,
        plaintext: &[u8],
        passphrase: &str,
    ) -> Result<std::path::PathBuf, CryptoError> {
        let kdf_params = KdfParams::new();
        let key = kdf::derive_key(passphrase, &kdf_params)?;
        let ciphertext = cipher::encrypt(&key, plaintext)?;

        let transcript = EncryptedTranscript {
            version: 1,
            session_id: session_id.to_string(),
            created_at: chrono_now(),
            kdf_params,
            ciphertext_b64: BASE64.encode(&ciphertext),
        };

        let filename = format!("{}.enc.json", session_id);
        let path = self.dir.join(&filename);
        let json = serde_json::to_string_pretty(&transcript)?;
        std::fs::write(&path, json)?;

        Ok(path)
    }

    /// Read and decrypt a transcript.
    pub fn read(&self, session_id: &str, passphrase: &str) -> Result<Vec<u8>, CryptoError> {
        let filename = format!("{}.enc.json", session_id);
        let path = self.dir.join(&filename);
        self.read_file(&path, passphrase)
    }

    /// Read and decrypt a transcript from a specific file path.
    pub fn read_file(&self, path: &Path, passphrase: &str) -> Result<Vec<u8>, CryptoError> {
        let json = std::fs::read_to_string(path)?;
        let transcript: EncryptedTranscript = serde_json::from_str(&json)?;

        let key = kdf::derive_key(passphrase, &transcript.kdf_params)?;
        let ciphertext = BASE64.decode(&transcript.ciphertext_b64)?;
        cipher::decrypt(&key, &ciphertext)
    }

    /// List all encrypted transcript session IDs in the store.
    pub fn list(&self) -> Result<Vec<String>, CryptoError> {
        let mut sessions = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(session_id) = name.strip_suffix(".enc.json") {
                sessions.push(session_id.to_string());
            }
        }
        sessions.sort();
        Ok(sessions)
    }

    /// Delete an encrypted transcript.
    pub fn delete(&self, session_id: &str) -> Result<(), CryptoError> {
        let filename = format!("{}.enc.json", session_id);
        let path = self.dir.join(&filename);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }
}

/// Simple ISO 8601 timestamp without chrono dependency.
fn chrono_now() -> String {
    use std::time::SystemTime;
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    // Basic UTC timestamp format
    let days = secs / 86400;
    let rem = secs % 86400;
    let hours = rem / 3600;
    let minutes = (rem % 3600) / 60;
    let seconds = rem % 60;

    // Days since epoch to Y-M-D (simplified — accurate for 2024-2030)
    let mut y = 1970;
    let mut remaining_days = days as i64;
    loop {
        let days_in_year = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
            366
        } else {
            365
        };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut m = 0;
    for (i, &d) in month_days.iter().enumerate() {
        if remaining_days < d as i64 {
            m = i + 1;
            break;
        }
        remaining_days -= d as i64;
    }
    let d = remaining_days + 1;

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, m, d, hours, minutes, seconds
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_store_write_read_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let store = EncryptedStore::new(tmp.path()).unwrap();

        let plaintext = b"Session transcript content with PII: SSN 123-45-6789";
        let passphrase = "strong-passphrase-123";

        let path = store.write("session-001", plaintext, passphrase).unwrap();
        assert!(path.exists());

        let decrypted = store.read("session-001", passphrase).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_store_wrong_passphrase() {
        let tmp = TempDir::new().unwrap();
        let store = EncryptedStore::new(tmp.path()).unwrap();

        store
            .write("session-002", b"secret data", "correct-pass")
            .unwrap();

        let result = store.read("session-002", "wrong-pass");
        assert!(result.is_err());
    }

    #[test]
    fn test_store_list() {
        let tmp = TempDir::new().unwrap();
        let store = EncryptedStore::new(tmp.path()).unwrap();

        store.write("alpha", b"data", "pass").unwrap();
        store.write("beta", b"data", "pass").unwrap();
        store.write("gamma", b"data", "pass").unwrap();

        let sessions = store.list().unwrap();
        assert_eq!(sessions, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn test_store_delete() {
        let tmp = TempDir::new().unwrap();
        let store = EncryptedStore::new(tmp.path()).unwrap();

        store.write("to-delete", b"data", "pass").unwrap();
        assert_eq!(store.list().unwrap().len(), 1);

        store.delete("to-delete").unwrap();
        assert_eq!(store.list().unwrap().len(), 0);
    }

    #[test]
    fn test_encrypted_file_format() {
        let tmp = TempDir::new().unwrap();
        let store = EncryptedStore::new(tmp.path()).unwrap();

        store.write("format-test", b"plaintext", "pass").unwrap();

        let path = tmp.path().join("format-test.enc.json");
        let json = std::fs::read_to_string(&path).unwrap();
        let transcript: EncryptedTranscript = serde_json::from_str(&json).unwrap();

        assert_eq!(transcript.version, 1);
        assert_eq!(transcript.session_id, "format-test");
        assert!(!transcript.ciphertext_b64.is_empty());
        assert_eq!(transcript.kdf_params.salt.len(), 16);
        assert_eq!(transcript.kdf_params.memory_kib, 19456);
    }

    #[test]
    fn test_large_transcript() {
        let tmp = TempDir::new().unwrap();
        let store = EncryptedStore::new(tmp.path()).unwrap();

        // Simulate a long chat session (~100KB)
        let plaintext =
            "User: Tell me about privacy.\nAssistant: Privacy is important.\n".repeat(1000);

        store
            .write("large-session", plaintext.as_bytes(), "pass")
            .unwrap();
        let decrypted = store.read("large-session", "pass").unwrap();
        assert_eq!(decrypted, plaintext.as_bytes());
    }
}

use std::sync::Arc;

use tokio::sync::RwLock;

use crate::fpe_engine::FpeEngine;
use crate::vault::{Vault, VaultError};

/// A versioned FPE engine: key version + engine instance.
pub struct VersionedEngine {
    pub version: u32,
    pub engine: FpeEngine,
}

/// Single-key FPE manager.
///
/// Holds one active engine behind a `RwLock` for safe concurrent reads.
pub struct KeyManager {
    current: RwLock<Arc<VersionedEngine>>,
}

impl KeyManager {
    /// Build a KeyManager from the vault's current key.
    pub fn new(vault: Arc<Vault>) -> Result<Self, KeyManagerError> {
        let key = vault.get_fpe_key().map_err(KeyManagerError::Vault)?;
        let engine = FpeEngine::new(&key).map_err(KeyManagerError::Fpe)?;

        oo_info!(
            crate::oo_log::modules::FPE,
            "KeyManager initialized",
            version = 1u32
        );

        Ok(Self {
            current: RwLock::new(Arc::new(VersionedEngine { version: 1, engine })),
        })
    }

    /// Build a KeyManager from an existing engine (for testing).
    #[cfg(test)]
    pub fn from_engine(engine: FpeEngine, version: u32) -> Self {
        Self {
            current: RwLock::new(Arc::new(VersionedEngine { version, engine })),
        }
    }

    /// Get the current versioned engine (fast read lock).
    pub async fn current(&self) -> Arc<VersionedEngine> {
        self.current.read().await.clone()
    }

    /// Get the current key version.
    pub async fn current_version(&self) -> u32 {
        self.current.read().await.version
    }
}

#[derive(Debug, thiserror::Error)]
pub enum KeyManagerError {
    #[error("Vault error: {0}")]
    Vault(VaultError),
    #[error("FPE engine error: {0}")]
    Fpe(crate::fpe_engine::FpeError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; 32] {
        let mut key = [0u8; 32];
        for (i, byte) in key.iter_mut().enumerate() {
            *byte = i as u8;
        }
        key
    }

    #[tokio::test]
    async fn test_key_manager_from_engine() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        let km = KeyManager::from_engine(engine, 1);
        assert_eq!(km.current_version().await, 1);
    }

    #[tokio::test]
    async fn test_current_returns_versioned_engine() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        let km = KeyManager::from_engine(engine, 3);
        let ve = km.current().await;
        assert_eq!(ve.version, 3);
    }

    #[tokio::test]
    async fn test_encrypt_decrypt_with_single_key() {
        use crate::pii_types::PiiType;
        use crate::scanner::PiiMatch;

        let engine = FpeEngine::new(&test_key()).unwrap();
        let km = KeyManager::from_engine(engine, 1);

        let pii_match = PiiMatch {
            pii_type: PiiType::Ssn,
            start: 0,
            end: 11,
            raw_value: "123-45-6789".to_string(),
            json_path: None,
            confidence: 1.0,
        };
        let tweak = b"test-tweak";

        // Encrypt with the current engine
        let ve = km.current().await;
        let result = ve.engine.encrypt_match(&pii_match, tweak).unwrap();
        assert_ne!(result.encrypted, "123-45-6789");

        // Decrypt with the same engine
        let decrypted = ve
            .engine
            .decrypt_value(&result.encrypted, PiiType::Ssn, tweak)
            .unwrap();
        assert_eq!(decrypted, "123-45-6789");
    }

    #[tokio::test]
    async fn test_concurrent_reads_during_write() {
        use std::sync::Arc as StdArc;

        let engine = FpeEngine::new(&test_key()).unwrap();
        let km = StdArc::new(KeyManager::from_engine(engine, 1));

        // Spawn multiple concurrent readers
        let mut handles = vec![];
        for _ in 0..10 {
            let km_clone = StdArc::clone(&km);
            handles.push(tokio::spawn(async move {
                let ve = km_clone.current().await;
                assert!(ve.version >= 1);
            }));
        }

        for h in handles {
            h.await.unwrap();
        }
    }
}

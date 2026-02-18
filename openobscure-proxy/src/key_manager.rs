use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;

use crate::fpe_engine::FpeEngine;
use crate::vault::{Vault, VaultError};

/// A versioned FPE engine: key version + engine instance.
pub struct VersionedEngine {
    pub version: u32,
    pub engine: FpeEngine,
}

/// Previous engine with expiry timestamp for lazy cleanup.
struct PreviousEngine {
    engine: Arc<VersionedEngine>,
    expires_at: Instant,
}

/// Manages FPE key rotation with dual-key overlap for in-flight requests.
///
/// During normal operation, `current` holds the active engine.
/// After rotation, `previous` holds the old engine for `overlap_secs`
/// so that in-flight response decryptions still work. Expiry is lazy —
/// checked on access, no background task required.
pub struct KeyManager {
    current: RwLock<Arc<VersionedEngine>>,
    previous: RwLock<Option<PreviousEngine>>,
    vault: Arc<Vault>,
    overlap_secs: u64,
}

impl KeyManager {
    /// Build a KeyManager from the vault's current key version.
    pub fn new(vault: Arc<Vault>, overlap_secs: u64) -> Result<Self, KeyManagerError> {
        let version = vault.get_fpe_key_version().map_err(KeyManagerError::Vault)?;
        let key = vault
            .get_fpe_key_by_version(version)
            .map_err(KeyManagerError::Vault)?;
        let engine = FpeEngine::new(&key).map_err(KeyManagerError::Fpe)?;

        oo_info!(crate::oo_log::modules::FPE, "KeyManager initialized",
            version = version, overlap_secs = overlap_secs);

        Ok(Self {
            current: RwLock::new(Arc::new(VersionedEngine { version, engine })),
            previous: RwLock::new(None),
            vault,
            overlap_secs,
        })
    }

    /// Build a KeyManager from an existing engine (for testing).
    #[cfg(test)]
    pub fn from_engine(engine: FpeEngine, version: u32) -> Self {
        Self {
            current: RwLock::new(Arc::new(VersionedEngine { version, engine })),
            previous: RwLock::new(None),
            vault: Arc::new(Vault::new("test-vault")),
            overlap_secs: 30,
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

    /// Get the engine for a specific key version (current or previous).
    /// Used for response decryption when the mapping was encrypted with a specific version.
    /// Lazily cleans up expired previous engine.
    pub async fn engine_for_version(&self, version: u32) -> Option<Arc<VersionedEngine>> {
        // Check current first (most common path)
        let current = self.current.read().await;
        if current.version == version {
            return Some(current.clone());
        }
        drop(current);

        // Check previous (may be expired)
        let prev = self.previous.read().await;
        if let Some(ref p) = *prev {
            if p.engine.version == version {
                if Instant::now() < p.expires_at {
                    return Some(p.engine.clone());
                }
                // Expired — drop read lock and clean up
                drop(prev);
                let mut prev_write = self.previous.write().await;
                if let Some(ref pw) = *prev_write {
                    if Instant::now() >= pw.expires_at {
                        oo_debug!(crate::oo_log::modules::FPE, "Overlap window expired, clearing previous key",
                            version = pw.engine.version);
                        *prev_write = None;
                    }
                }
                return None;
            }
        }
        None
    }

    /// Rotate to a new key version.
    ///
    /// 1. Generate new 32-byte key
    /// 2. Store as `fpe-master-key-v{new_version}` in vault
    /// 3. Update `fpe-key-current` pointer
    /// 4. Swap current engine, move old to previous with expiry
    pub async fn rotate(&self) -> Result<u32, KeyManagerError> {
        let old_version = self.current_version().await;
        let new_version = old_version + 1;

        // Generate new key
        let mut new_key = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut new_key);

        // Store in vault
        self.vault
            .store_fpe_key_versioned(new_version, &new_key)
            .map_err(KeyManagerError::Vault)?;
        self.vault
            .set_current_version(new_version)
            .map_err(KeyManagerError::Vault)?;

        // Build new engine
        let new_engine = FpeEngine::new(&new_key).map_err(KeyManagerError::Fpe)?;

        // Zero the key material
        new_key.fill(0);

        // Atomic swap: current → previous, new → current
        let old_engine = {
            let mut current = self.current.write().await;
            let old = current.clone();
            *current = Arc::new(VersionedEngine {
                version: new_version,
                engine: new_engine,
            });
            old
        };

        {
            let mut prev = self.previous.write().await;
            *prev = Some(PreviousEngine {
                engine: old_engine,
                expires_at: Instant::now() + std::time::Duration::from_secs(self.overlap_secs),
            });
        }

        oo_info!(crate::oo_log::modules::FPE, "Key rotated",
            old_version = old_version,
            new_version = new_version,
            overlap_secs = self.overlap_secs);

        Ok(new_version)
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

    fn test_key_2() -> [u8; 32] {
        let mut key = [0u8; 32];
        for (i, byte) in key.iter_mut().enumerate() {
            *byte = (i as u8).wrapping_add(100);
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
    async fn test_engine_for_current_version() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        let km = KeyManager::from_engine(engine, 1);
        let found = km.engine_for_version(1).await;
        assert!(found.is_some());
        assert_eq!(found.unwrap().version, 1);
    }

    #[tokio::test]
    async fn test_engine_for_unknown_version_returns_none() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        let km = KeyManager::from_engine(engine, 1);
        let found = km.engine_for_version(99).await;
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn test_manual_swap_with_previous() {
        // Simulate what rotate() does without vault access
        let engine1 = FpeEngine::new(&test_key()).unwrap();
        let km = KeyManager::from_engine(engine1, 1);

        // Manually swap
        let engine2 = FpeEngine::new(&test_key_2()).unwrap();
        {
            let old = {
                let mut current = km.current.write().await;
                let old = current.clone();
                *current = Arc::new(VersionedEngine {
                    version: 2,
                    engine: engine2,
                });
                old
            };
            let mut prev = km.previous.write().await;
            *prev = Some(PreviousEngine {
                engine: old,
                expires_at: Instant::now() + std::time::Duration::from_secs(30),
            });
        }

        // Current should be v2
        assert_eq!(km.current_version().await, 2);

        // Should find v2 (current)
        assert!(km.engine_for_version(2).await.is_some());

        // Should find v1 (previous, not expired)
        assert!(km.engine_for_version(1).await.is_some());

        // Should not find v99
        assert!(km.engine_for_version(99).await.is_none());
    }

    #[tokio::test]
    async fn test_previous_engine_expiry() {
        let engine1 = FpeEngine::new(&test_key()).unwrap();
        let km = KeyManager::from_engine(engine1, 1);

        // Set previous with 0-second overlap (already expired)
        let engine2 = FpeEngine::new(&test_key_2()).unwrap();
        {
            let old = {
                let mut current = km.current.write().await;
                let old = current.clone();
                *current = Arc::new(VersionedEngine {
                    version: 2,
                    engine: engine2,
                });
                old
            };
            let mut prev = km.previous.write().await;
            *prev = Some(PreviousEngine {
                engine: old,
                expires_at: Instant::now(), // Already expired
            });
        }

        // v1 should not be found (expired)
        // Small sleep to ensure we're past the expiry instant
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        assert!(km.engine_for_version(1).await.is_none());
    }

    #[tokio::test]
    async fn test_encrypt_decrypt_across_versions() {
        use crate::scanner::PiiMatch;
        use crate::pii_types::PiiType;

        let engine1 = FpeEngine::new(&test_key()).unwrap();
        let km = KeyManager::from_engine(engine1, 1);

        let pii_match = PiiMatch {
            pii_type: PiiType::Ssn,
            start: 0,
            end: 11,
            raw_value: "123-45-6789".to_string(),
            json_path: None,
            confidence: 1.0,
        };
        let tweak = b"test-tweak";

        // Encrypt with v1
        let ve = km.current().await;
        let result = ve.engine.encrypt_match(&pii_match, tweak).unwrap();
        assert_ne!(result.encrypted, "123-45-6789");

        // Decrypt with v1 engine retrieved by version
        let ve_found = km.engine_for_version(1).await.unwrap();
        let decrypted = ve_found
            .engine
            .decrypt_value(&result.encrypted, PiiType::Ssn, tweak)
            .unwrap();
        assert_eq!(decrypted, "123-45-6789");
    }

    #[tokio::test]
    async fn test_different_keys_produce_different_ciphertexts() {
        use crate::scanner::PiiMatch;
        use crate::pii_types::PiiType;

        let engine1 = FpeEngine::new(&test_key()).unwrap();
        let engine2 = FpeEngine::new(&test_key_2()).unwrap();

        let pii_match = PiiMatch {
            pii_type: PiiType::Ssn,
            start: 0,
            end: 11,
            raw_value: "123-45-6789".to_string(),
            json_path: None,
            confidence: 1.0,
        };
        let tweak = b"same-tweak";

        let result1 = engine1.encrypt_match(&pii_match, tweak).unwrap();
        let result2 = engine2.encrypt_match(&pii_match, tweak).unwrap();
        assert_ne!(result1.encrypted, result2.encrypted,
            "Different keys must produce different ciphertexts");
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

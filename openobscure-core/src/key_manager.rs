//! FPE key lifecycle management: active key + 30-second rotation overlap.
//!
//! `KeyManager` holds the current `FpeEngine` under an `RwLock`. On rotation,
//! the previous engine is retained as `PreviousEngine` for 30 seconds so that
//! in-flight requests encrypted with the old key can still be decrypted.
//! After the overlap window expires, the previous engine is evicted.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

use crate::fpe_engine::FpeEngine;
use crate::vault::{Vault, VaultError};

/// Default overlap window: 30 seconds during which both old and new keys are valid.
const DEFAULT_OVERLAP_SECS: u64 = 30;

/// A versioned FPE engine: key version + engine instance.
pub struct VersionedEngine {
    pub version: u32,
    pub engine: FpeEngine,
}

/// Single-key FPE manager with rotation support.
///
/// Holds one active engine behind a `RwLock` for safe concurrent reads.
/// During rotation, the previous engine is retained for an overlap window
/// so in-flight requests encrypted with the old key can still be decrypted.
pub struct KeyManager {
    current: RwLock<Arc<VersionedEngine>>,
    previous: RwLock<Option<PreviousEngine>>,
    overlap_duration: Duration,
}

/// A retired engine kept alive during the overlap window.
struct PreviousEngine {
    engine: Arc<VersionedEngine>,
    retired_at: Instant,
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
            previous: RwLock::new(None),
            overlap_duration: Duration::from_secs(DEFAULT_OVERLAP_SECS),
        })
    }

    /// Build a KeyManager from an existing engine (for testing).
    #[cfg(test)]
    pub fn from_engine(engine: FpeEngine, version: u32) -> Self {
        Self {
            current: RwLock::new(Arc::new(VersionedEngine { version, engine })),
            previous: RwLock::new(None),
            overlap_duration: Duration::from_secs(DEFAULT_OVERLAP_SECS),
        }
    }

    /// Build a KeyManager with a custom overlap duration (for testing).
    #[cfg(test)]
    pub fn from_engine_with_overlap(engine: FpeEngine, version: u32, overlap: Duration) -> Self {
        Self {
            current: RwLock::new(Arc::new(VersionedEngine { version, engine })),
            previous: RwLock::new(None),
            overlap_duration: overlap,
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

    /// Rotate to a new key from the vault.
    ///
    /// The old engine is kept for `overlap_duration` so in-flight requests
    /// encrypted with the previous key can still be decrypted.
    pub async fn rotate(&self, vault: &Vault) -> Result<u32, KeyManagerError> {
        let key = vault.get_fpe_key().map_err(KeyManagerError::Vault)?;
        let new_engine = FpeEngine::new(&key).map_err(KeyManagerError::Fpe)?;

        self.rotate_with_engine(new_engine).await
    }

    /// Rotate to a new engine directly (also used internally and in tests).
    pub async fn rotate_with_engine(&self, new_engine: FpeEngine) -> Result<u32, KeyManagerError> {
        let mut current = self.current.write().await;
        let old_version = current.version;
        let new_version = old_version + 1;

        // Move current to previous (overlap window starts now)
        let old = Arc::clone(&current);
        {
            let mut prev = self.previous.write().await;
            *prev = Some(PreviousEngine {
                engine: old,
                retired_at: Instant::now(),
            });
        }

        // Install new engine
        *current = Arc::new(VersionedEngine {
            version: new_version,
            engine: new_engine,
        });

        oo_info!(
            crate::oo_log::modules::FPE,
            "Key rotated",
            old_version = old_version,
            new_version = new_version,
            overlap_secs = self.overlap_duration.as_secs()
        );

        Ok(new_version)
    }

    /// Return the previous FPE engine if it is still within the overlap window.
    ///
    /// During key rotation, responses encrypted with the old key may still be in-flight.
    /// The proxy tries the current engine first; if decryption fails, it falls back to
    /// `previous()`. After `overlap_duration` (default 30 s) the old engine is dropped.
    /// Returns `None` if no previous engine exists or the overlap window has expired.
    pub async fn previous(&self) -> Option<Arc<VersionedEngine>> {
        let mut prev = self.previous.write().await;
        if let Some(ref p) = *prev {
            if p.retired_at.elapsed() < self.overlap_duration {
                return Some(Arc::clone(&p.engine));
            }
            // Overlap expired — drop the old engine
            *prev = None;
        }
        None
    }

    /// Check if an overlap window is currently active.
    pub async fn has_overlap(&self) -> bool {
        self.previous().await.is_some()
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

    // ─── Rotation tests ───

    #[tokio::test]
    async fn test_rotate_increments_version() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        let km = KeyManager::from_engine(engine, 1);
        assert_eq!(km.current_version().await, 1);

        let new_engine = FpeEngine::new(&test_key_2()).unwrap();
        let new_ver = km.rotate_with_engine(new_engine).await.unwrap();
        assert_eq!(new_ver, 2);
        assert_eq!(km.current_version().await, 2);
    }

    #[tokio::test]
    async fn test_rotate_multiple_times() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        let km = KeyManager::from_engine(engine, 1);

        for expected in 2..=5 {
            let new_engine = FpeEngine::new(&test_key()).unwrap();
            let ver = km.rotate_with_engine(new_engine).await.unwrap();
            assert_eq!(ver, expected);
        }
        assert_eq!(km.current_version().await, 5);
    }

    #[tokio::test]
    async fn test_rotate_preserves_previous_in_overlap() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        // Use a long overlap so it doesn't expire during the test
        let km = KeyManager::from_engine_with_overlap(engine, 1, Duration::from_secs(60));

        // No previous before rotation
        assert!(!km.has_overlap().await);

        let new_engine = FpeEngine::new(&test_key_2()).unwrap();
        km.rotate_with_engine(new_engine).await.unwrap();

        // Previous should exist during overlap
        assert!(km.has_overlap().await);
        let prev = km.previous().await.unwrap();
        assert_eq!(prev.version, 1);
        assert_eq!(km.current_version().await, 2);
    }

    #[tokio::test]
    async fn test_overlap_expires() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        // Use a very short overlap (10ms)
        let km = KeyManager::from_engine_with_overlap(engine, 1, Duration::from_millis(10));

        let new_engine = FpeEngine::new(&test_key_2()).unwrap();
        km.rotate_with_engine(new_engine).await.unwrap();

        // Previous should exist immediately after rotation
        assert!(km.has_overlap().await);

        // Wait for overlap to expire
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Previous should be gone now
        assert!(!km.has_overlap().await);
        assert!(km.previous().await.is_none());
    }

    #[tokio::test]
    async fn test_rotate_replaces_previous() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        let km = KeyManager::from_engine_with_overlap(engine, 1, Duration::from_secs(60));

        // First rotation: v1 → v2
        let e2 = FpeEngine::new(&test_key_2()).unwrap();
        km.rotate_with_engine(e2).await.unwrap();
        let prev = km.previous().await.unwrap();
        assert_eq!(prev.version, 1);

        // Second rotation: v2 → v3 (previous should now be v2, not v1)
        let e3 = FpeEngine::new(&test_key()).unwrap();
        km.rotate_with_engine(e3).await.unwrap();
        let prev = km.previous().await.unwrap();
        assert_eq!(prev.version, 2);
        assert_eq!(km.current_version().await, 3);
    }

    #[tokio::test]
    async fn test_decrypt_with_old_key_during_overlap() {
        use crate::pii_types::PiiType;
        use crate::scanner::PiiMatch;

        let key1 = test_key();
        let key2 = test_key_2();

        let engine1 = FpeEngine::new(&key1).unwrap();
        let km = KeyManager::from_engine_with_overlap(engine1, 1, Duration::from_secs(60));

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
        let ve1 = km.current().await;
        let encrypted = ve1.engine.encrypt_match(&pii_match, tweak).unwrap();

        // Rotate to v2
        let engine2 = FpeEngine::new(&key2).unwrap();
        km.rotate_with_engine(engine2).await.unwrap();

        // Current engine (v2) can't decrypt v1's ciphertext correctly
        let ve2 = km.current().await;
        assert_eq!(ve2.version, 2);

        // But previous engine (v1) can still decrypt during overlap
        let prev = km.previous().await.unwrap();
        assert_eq!(prev.version, 1);
        let decrypted = prev
            .engine
            .decrypt_value(&encrypted.encrypted, PiiType::Ssn, tweak)
            .unwrap();
        assert_eq!(decrypted, "123-45-6789");
    }

    #[tokio::test]
    async fn test_new_key_produces_different_ciphertext() {
        use crate::pii_types::PiiType;
        use crate::scanner::PiiMatch;

        let key1 = test_key();
        let key2 = test_key_2();

        let engine1 = FpeEngine::new(&key1).unwrap();
        let km = KeyManager::from_engine_with_overlap(engine1, 1, Duration::from_secs(60));

        let pii_match = PiiMatch {
            pii_type: PiiType::Ssn,
            start: 0,
            end: 11,
            raw_value: "123-45-6789".to_string(),
            json_path: None,
            confidence: 1.0,
        };
        let tweak = b"same-tweak";

        // Encrypt with v1
        let ve1 = km.current().await;
        let result1 = ve1.engine.encrypt_match(&pii_match, tweak).unwrap();

        // Rotate to v2 (different key)
        let engine2 = FpeEngine::new(&key2).unwrap();
        km.rotate_with_engine(engine2).await.unwrap();

        // Encrypt same value with v2
        let ve2 = km.current().await;
        let result2 = ve2.engine.encrypt_match(&pii_match, tweak).unwrap();

        // Different keys → different ciphertexts
        assert_ne!(result1.encrypted, result2.encrypted);
    }

    #[tokio::test]
    async fn test_concurrent_reads_during_rotation() {
        use std::sync::Arc as StdArc;

        let engine = FpeEngine::new(&test_key()).unwrap();
        let km = StdArc::new(KeyManager::from_engine(engine, 1));

        // Spawn readers continuously
        let mut handles = vec![];
        for _ in 0..20 {
            let km_clone = StdArc::clone(&km);
            handles.push(tokio::spawn(async move {
                let ve = km_clone.current().await;
                // Version should be either 1 (pre-rotation) or 2 (post-rotation)
                assert!(ve.version == 1 || ve.version == 2);
            }));
        }

        // Rotate in the middle of reads
        let new_engine = FpeEngine::new(&test_key_2()).unwrap();
        km.rotate_with_engine(new_engine).await.unwrap();

        for h in handles {
            h.await.unwrap();
        }

        assert_eq!(km.current_version().await, 2);
    }

    #[tokio::test]
    async fn test_no_previous_before_first_rotation() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        let km = KeyManager::from_engine(engine, 1);

        assert!(!km.has_overlap().await);
        assert!(km.previous().await.is_none());
    }

    #[tokio::test]
    async fn test_default_overlap_is_30s() {
        let engine = FpeEngine::new(&test_key()).unwrap();
        let km = KeyManager::from_engine(engine, 1);
        assert_eq!(km.overlap_duration, Duration::from_secs(30));
    }
}

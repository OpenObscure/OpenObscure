use keyring::Entry;
use rand::RngCore;

pub struct Vault {
    service: String,
}

impl Vault {
    pub fn new(service: &str) -> Self {
        Self {
            service: service.to_string(),
        }
    }

    /// Retrieve the 32-byte FPE master key.
    ///
    /// Resolution order:
    /// 1. `OPENOBSCURE_MASTER_KEY` env var (64 hex chars → 32 bytes) — for Docker/VPS/CI
    /// 2. OS keychain via `keyring` — for desktop environments
    /// 3. Fail with error listing both options
    pub fn get_fpe_key(&self) -> Result<[u8; 32], VaultError> {
        // 1. Check env var first (headless/Docker/CI)
        if let Ok(hex_key) = std::env::var("OPENOBSCURE_MASTER_KEY") {
            return self.parse_hex_key(&hex_key);
        }

        // 2. Try OS keychain
        match self.get_fpe_key_from_keychain() {
            Ok(key) => Ok(key),
            Err(e) => Err(VaultError::KeyNotFound(format!(
                "No FPE key available. Options:\n  \
                 1. Set OPENOBSCURE_MASTER_KEY env var (64 hex chars)\n  \
                 2. Run with --init-key to store in OS keychain\n  \
                 Keychain error: {}",
                e
            ))),
        }
    }

    /// Parse a hex-encoded key from an environment variable.
    fn parse_hex_key(&self, hex_key: &str) -> Result<[u8; 32], VaultError> {
        let decoded = hex::decode(hex_key.trim()).map_err(|e| {
            VaultError::EnvVar(format!(
                "OPENOBSCURE_MASTER_KEY contains invalid hex: {}",
                e
            ))
        })?;
        if decoded.len() != 32 {
            return Err(VaultError::InvalidKeyLength(decoded.len()));
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&decoded);
        oo_info!(
            crate::oo_log::modules::VAULT,
            "FPE key loaded from OPENOBSCURE_MASTER_KEY environment variable"
        );
        Ok(key)
    }

    /// Retrieve the FPE key from the OS keychain only.
    fn get_fpe_key_from_keychain(&self) -> Result<[u8; 32], VaultError> {
        let entry = Entry::new(&self.service, "fpe-master-key").map_err(VaultError::Keyring)?;
        let secret = entry.get_secret().map_err(VaultError::Keyring)?;
        if secret.len() != 32 {
            return Err(VaultError::InvalidKeyLength(secret.len()));
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&secret);
        Ok(key)
    }

    /// Generate a new random 32-byte FPE key and store it in the OS keychain.
    ///
    /// When `OPENOBSCURE_HEADLESS=1` is set, also prints the key as hex to stdout
    /// so it can be captured for `OPENOBSCURE_MASTER_KEY` env var usage.
    pub fn init_fpe_key(&self) -> Result<(), VaultError> {
        let mut key = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut key);

        // In headless mode, print the key so the user can capture it
        if std::env::var("OPENOBSCURE_HEADLESS").is_ok() {
            println!("OPENOBSCURE_MASTER_KEY={}", hex::encode(key));
        }

        let entry = Entry::new(&self.service, "fpe-master-key").map_err(VaultError::Keyring)?;
        entry.set_secret(&key).map_err(VaultError::Keyring)?;
        key.fill(0);
        oo_info!(
            crate::oo_log::modules::VAULT,
            "FPE master key initialized in OS keychain"
        );
        Ok(())
    }

    /// Check if the FPE key already exists (env var or keychain).
    pub fn fpe_key_exists(&self) -> bool {
        if std::env::var("OPENOBSCURE_MASTER_KEY").is_ok() {
            return true;
        }
        let entry = match Entry::new(&self.service, "fpe-master-key") {
            Ok(e) => e,
            Err(_) => return false,
        };
        entry.get_secret().is_ok()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("Keychain error: {0}")]
    Keyring(keyring::Error),
    #[error("FPE key has invalid length: expected 32 bytes, got {0}")]
    InvalidKeyLength(usize),
    #[error("Environment variable error: {0}")]
    EnvVar(String),
    #[error("{0}")]
    KeyNotFound(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize env var tests to avoid races
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_env_var_key_valid() {
        let _lock = ENV_LOCK.lock().unwrap();
        let key_hex = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6a7b8c9d0e1f2a3b4c5d6a7b8c9d0e1f2";
        std::env::set_var("OPENOBSCURE_MASTER_KEY", key_hex);

        let vault = Vault::new("test-vault-env");
        let result = vault.get_fpe_key();
        std::env::remove_var("OPENOBSCURE_MASTER_KEY");

        assert!(result.is_ok());
        let key = result.unwrap();
        assert_eq!(key[0], 0xa1);
        assert_eq!(key[1], 0xb2);
        assert_eq!(key.len(), 32);
    }

    #[test]
    fn test_env_var_key_invalid_hex() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("OPENOBSCURE_MASTER_KEY", "not-valid-hex-string-at-all!!");

        let vault = Vault::new("test-vault-env");
        let result = vault.get_fpe_key();
        std::env::remove_var("OPENOBSCURE_MASTER_KEY");

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid hex"), "Error was: {}", err);
    }

    #[test]
    fn test_env_var_key_wrong_length() {
        let _lock = ENV_LOCK.lock().unwrap();
        // 24 hex chars = 12 bytes, not 32
        std::env::set_var("OPENOBSCURE_MASTER_KEY", "a1b2c3d4e5f6a7b8c9d0e1f2");

        let vault = Vault::new("test-vault-env");
        let result = vault.get_fpe_key();
        std::env::remove_var("OPENOBSCURE_MASTER_KEY");

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("invalid length") || err.contains("expected 32"),
            "Error was: {}",
            err
        );
    }

    #[test]
    fn test_env_var_with_whitespace_trimmed() {
        let _lock = ENV_LOCK.lock().unwrap();
        let key_hex = "  a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6a7b8c9d0e1f2a3b4c5d6a7b8c9d0e1f2\n";
        std::env::set_var("OPENOBSCURE_MASTER_KEY", key_hex);

        let vault = Vault::new("test-vault-env");
        let result = vault.get_fpe_key();
        std::env::remove_var("OPENOBSCURE_MASTER_KEY");

        assert!(result.is_ok());
    }

    #[test]
    fn test_fpe_key_exists_with_env_var() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var(
            "OPENOBSCURE_MASTER_KEY",
            "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6a7b8c9d0e1f2a3b4c5d6a7b8c9d0e1f2",
        );

        let vault = Vault::new("test-vault-env");
        assert!(vault.fpe_key_exists());

        std::env::remove_var("OPENOBSCURE_MASTER_KEY");
    }
}

//! FPE master key storage: OS keychain with env-var and file fallbacks.
//!
//! `Vault` resolves the 32-byte AES-256 key used by `FpeEngine` through a
//! 5-step priority chain:
//!
//! 1. `OPENOBSCURE_MASTER_KEY` env var (64 hex chars) — Docker/CI/headless
//! 2. `/run/secrets/openobscure-master-key` file — Kubernetes/Docker Secrets standard path
//! 3. `OPENOBSCURE_KEY_FILE` env var (path to a hex key file) — custom mount points
//! 4. `~/.openobscure/master-key` file — home-dir volume mounts
//! 5. OS keychain — native desktop/laptop install
//!
//! The `--init-key` CLI subcommand calls `init_fpe_key()` to generate a random
//! key and store it in the keychain on first run.

use keyring::Entry;
use rand::RngCore;

/// Secure key storage backed by the OS keychain (via `keyring`) with an env-var override.
///
/// Resolution order for `get_fpe_key()`:
/// 1. `OPENOBSCURE_MASTER_KEY` env var (64 hex chars → 32 bytes) — Docker/CI/headless
/// 2. OS keychain entry keyed by `service` name — desktop/laptop interactive use
///
/// Use `init_fpe_key()` to generate and store a new random key. In headless mode
/// (`OPENOBSCURE_HEADLESS=1`) the hex-encoded key is also printed to stdout so it
/// can be captured and stored in an environment variable or secrets manager.
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
    /// 1. `OPENOBSCURE_MASTER_KEY` env var (64 hex chars) — CI/CD, secret stores that inject into env
    /// 2. `/run/secrets/openobscure-master-key` file — Kubernetes Secrets / Docker Secrets standard path
    /// 3. `OPENOBSCURE_KEY_FILE` env var — custom file path for non-standard mounts
    /// 4. `~/.openobscure/master-key` file — volume-mounted home dir for local Docker dev
    /// 5. OS keychain — native install (desktop/laptop)
    pub fn get_fpe_key(&self) -> Result<[u8; 32], VaultError> {
        // 1. Env var (CI/CD, secret stores that inject into environment)
        if let Ok(hex_key) = std::env::var("OPENOBSCURE_MASTER_KEY") {
            return self.parse_hex_key(&hex_key);
        }

        // 2. Kubernetes/Docker Secrets standard path
        let k8s_path = std::path::Path::new("/run/secrets/openobscure-master-key");
        if k8s_path.exists() {
            return self.load_key_file(k8s_path);
        }

        // 3. Custom file path override
        if let Ok(path_str) = std::env::var("OPENOBSCURE_KEY_FILE") {
            return self.load_key_file(std::path::Path::new(&path_str));
        }

        // 4. Default file path (useful when home dir is volume-mounted)
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
        if let Some(home_dir) = home {
            let file_path = std::path::Path::new(&home_dir).join(".openobscure/master-key");
            if file_path.exists() {
                return self.load_key_file(&file_path);
            }
        }

        // 5. OS keychain
        match self.get_fpe_key_from_keychain() {
            Ok(key) => Ok(key),
            Err(e) => Err(VaultError::KeyNotFound(format!(
                "No FPE key found. Tried:\n  \
                 1. OPENOBSCURE_MASTER_KEY env var\n  \
                 2. /run/secrets/openobscure-master-key\n  \
                 3. OPENOBSCURE_KEY_FILE env var\n  \
                 4. ~/.openobscure/master-key file\n  \
                 5. OS keychain — {}\n\n  \
                 Run --init-key to generate and store a key.",
                e
            ))),
        }
    }

    /// Load and parse a 64-hex-char key from a file.
    fn load_key_file(&self, path: &std::path::Path) -> Result<[u8; 32], VaultError> {
        let contents = std::fs::read_to_string(path).map_err(|e| {
            VaultError::Io(format!("Cannot read key file {}: {}", path.display(), e))
        })?;
        oo_info!(
            crate::oo_log::modules::VAULT,
            "FPE key loaded from file",
            path = %path.display()
        );
        self.parse_hex_key(&contents)
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

    /// Retrieve the FPE key from the OS keychain only (step 5).
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
        let is_headless = std::env::var("OPENOBSCURE_HEADLESS").is_ok();
        if is_headless {
            println!("OPENOBSCURE_MASTER_KEY={}", hex::encode(key));
            eprintln!();
            eprintln!("  Save this key securely. It cannot be recovered if lost.");
            eprintln!("  All FPE-encrypted data depends on this key.");
        }

        let entry = Entry::new(&self.service, "fpe-master-key").map_err(VaultError::Keyring)?;
        entry.set_secret(&key).map_err(VaultError::Keyring)?;
        key.fill(0); // Zero the stack copy; the keychain holds the only live reference.
        oo_debug!(
            crate::oo_log::modules::VAULT,
            "FPE master key initialized in OS keychain"
        );

        if !is_headless {
            println!();
            println!("  ╔══════════════════════════════════════════════════════════════╗");
            println!("  ║  FPE master key generated and stored in OS keychain.        ║");
            println!("  ║                                                             ║");
            println!("  ║  WARNING: If you lose access to this keychain (OS           ║");
            println!("  ║  reinstall, machine change), all FPE-encrypted              ║");
            println!("  ║  conversation history becomes permanently unreadable.       ║");
            println!("  ║                                                             ║");
            println!("  ║  To back up your key for recovery:                          ║");
            println!("  ║    OPENOBSCURE_HEADLESS=1 openobscure --init-key      ║");
            println!("  ║  (prints key as hex — store it securely)                    ║");
            println!("  ╚══════════════════════════════════════════════════════════════╝");
            println!();
        }

        Ok(())
    }

    /// Check if the FPE key is available through any resolution step.
    pub fn fpe_key_exists(&self) -> bool {
        // 1. Env var
        if std::env::var("OPENOBSCURE_MASTER_KEY").is_ok() {
            return true;
        }
        // 2. /run/secrets standard path
        if std::path::Path::new("/run/secrets/openobscure-master-key").exists() {
            return true;
        }
        // 3. Custom file path env var
        if let Ok(path_str) = std::env::var("OPENOBSCURE_KEY_FILE") {
            if std::path::Path::new(&path_str).exists() {
                return true;
            }
        }
        // 4. ~/.openobscure/master-key
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
        if let Some(home_dir) = home {
            if std::path::Path::new(&home_dir)
                .join(".openobscure/master-key")
                .exists()
            {
                return true;
            }
        }
        // 5. OS keychain
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
    #[error("Key file I/O error: {0}")]
    Io(String),
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

    // ── New resolution-chain tests ────────────────────────────────────────────

    /// Helper: write a valid hex key to a temp file, return the path.
    fn write_temp_key(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(
            &path,
            "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6a7b8c9d0e1f2a3b4c5d6a7b8c9d0e1f2",
        )
        .unwrap();
        path
    }

    #[test]
    fn test_key_resolution_env_var_priority() {
        // Env var takes priority even when KEY_FILE also points to a valid key.
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let file_path = write_temp_key(tmp.path(), "other.key");

        std::env::set_var(
            "OPENOBSCURE_MASTER_KEY",
            "b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6a7b8c9d0e1f2a3b4c5d6a7b8c9d0e1f2a1",
        );
        std::env::set_var("OPENOBSCURE_KEY_FILE", file_path.to_str().unwrap());

        let vault = Vault::new("test-priority");
        let key = vault.get_fpe_key().unwrap();
        // First byte 0xb2 comes from the env var key, not the file key (0xa1)
        assert_eq!(key[0], 0xb2);

        std::env::remove_var("OPENOBSCURE_MASTER_KEY");
        std::env::remove_var("OPENOBSCURE_KEY_FILE");
    }

    #[test]
    fn test_key_resolution_key_file_env() {
        // OPENOBSCURE_KEY_FILE is used when no env var or /run/secrets.
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let file_path = write_temp_key(tmp.path(), "custom.key");

        std::env::remove_var("OPENOBSCURE_MASTER_KEY");
        std::env::set_var("OPENOBSCURE_KEY_FILE", file_path.to_str().unwrap());

        let vault = Vault::new("test-key-file");
        let result = vault.get_fpe_key();
        std::env::remove_var("OPENOBSCURE_KEY_FILE");

        assert!(result.is_ok());
        assert_eq!(result.unwrap()[0], 0xa1);
    }

    #[test]
    fn test_key_resolution_home_file() {
        // ~/.openobscure/master-key is read when HOME points to a temp dir.
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let oo_dir = tmp.path().join(".openobscure");
        std::fs::create_dir_all(&oo_dir).unwrap();
        write_temp_key(&oo_dir, "master-key");

        std::env::remove_var("OPENOBSCURE_MASTER_KEY");
        std::env::remove_var("OPENOBSCURE_KEY_FILE");
        std::env::set_var("HOME", tmp.path().to_str().unwrap());

        let vault = Vault::new("test-home-file");
        let result = vault.get_fpe_key();
        // Restore HOME regardless of outcome
        std::env::remove_var("HOME");

        assert!(result.is_ok());
        assert_eq!(result.unwrap()[0], 0xa1);
    }

    #[test]
    fn test_key_resolution_error_lists_all_options() {
        // When no key source is available the error mentions all 5 steps.
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::remove_var("OPENOBSCURE_MASTER_KEY");
        std::env::remove_var("OPENOBSCURE_KEY_FILE");
        // Point HOME at an empty temp dir so ~/.openobscure/master-key won't exist
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path().to_str().unwrap());

        let vault = Vault::new("test-error-msg-no-keychain");
        let result = vault.get_fpe_key();
        std::env::remove_var("HOME");

        // Will fail (no keychain in CI) — check the error text
        if let Err(e) = result {
            let msg = e.to_string();
            assert!(
                msg.contains("OPENOBSCURE_MASTER_KEY"),
                "missing step 1: {msg}"
            );
            assert!(msg.contains("/run/secrets"), "missing step 2: {msg}");
            assert!(
                msg.contains("OPENOBSCURE_KEY_FILE"),
                "missing step 3: {msg}"
            );
            assert!(
                msg.contains("~/.openobscure/master-key") || msg.contains("master-key file"),
                "missing step 4: {msg}"
            );
            assert!(
                msg.contains("keychain") || msg.contains("OS keychain"),
                "missing step 5: {msg}"
            );
        }
        // If the OS keychain happens to have a key in CI, the test is vacuously OK.
    }

    #[test]
    fn test_load_key_file_invalid_hex() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bad.key");
        std::fs::write(&path, "this-is-not-hex!!").unwrap();

        std::env::remove_var("OPENOBSCURE_MASTER_KEY");
        std::env::set_var("OPENOBSCURE_KEY_FILE", path.to_str().unwrap());

        let vault = Vault::new("test-bad-hex");
        let result = vault.get_fpe_key();
        std::env::remove_var("OPENOBSCURE_KEY_FILE");

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("invalid hex") || msg.contains("Invalid character"),
            "Error was: {msg}"
        );
    }

    #[test]
    fn test_load_key_file_wrong_length() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("short.key");
        // 24 hex chars = 12 bytes, not 32
        std::fs::write(&path, "a1b2c3d4e5f6a7b8c9d0e1f2").unwrap();

        std::env::remove_var("OPENOBSCURE_MASTER_KEY");
        std::env::set_var("OPENOBSCURE_KEY_FILE", path.to_str().unwrap());

        let vault = Vault::new("test-short-key");
        let result = vault.get_fpe_key();
        std::env::remove_var("OPENOBSCURE_KEY_FILE");

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("invalid length") || msg.contains("expected 32"),
            "Error was: {msg}"
        );
    }

    #[test]
    fn test_fpe_key_exists_via_key_file() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let path = write_temp_key(tmp.path(), "exists.key");

        std::env::remove_var("OPENOBSCURE_MASTER_KEY");
        std::env::set_var("OPENOBSCURE_KEY_FILE", path.to_str().unwrap());

        let vault = Vault::new("test-exists-file");
        let exists = vault.fpe_key_exists();
        std::env::remove_var("OPENOBSCURE_KEY_FILE");

        assert!(exists);
    }
}

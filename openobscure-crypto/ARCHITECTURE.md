# OpenObscure Crypto — Architecture

> Layer 2 of the OpenObscure privacy firewall. See `../project-plan/MASTER_PLAN.md` for full system architecture.

---

## Role in OpenObscure

The encryption layer provides **at-rest encryption** for session transcripts. When the agent stores conversation history, OpenObscure encrypts it before it touches disk — ensuring that even if storage is compromised, transcripts are unreadable without the passphrase.

```
┌──────────────┐         ┌──────────────────┐         ┌──────────────┐
│   AI Agent   │  bytes  │  openobscure-crypto │  .enc   │  Local Disk  │
│              │ ──────► │  (encrypt + KDF)  │ ──────► │  (JSON files)│
│              │ ◄────── │                   │ ◄────── │              │
└──────────────┘ decrypt └──────────────────┘  read   └──────────────┘
                          │                  │
                          │  Argon2id KDF    │
                          │  AES-256-GCM     │
                          │  Random nonces   │
                          └──────────────────┘
```

## Module Map

```
src/
├── lib.rs       Crate root, module exports, CryptoError enum
├── kdf.rs       Argon2id key derivation (passphrase → 32-byte AES key)
├── cipher.rs    AES-256-GCM authenticated encryption/decryption
└── store.rs     Encrypted file storage (write/read/list/delete transcripts)
```

## Cryptographic Design

### Key Derivation (kdf.rs)

**Algorithm:** Argon2id (OWASP recommended, RFC 9106)

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| Memory | 19,456 KiB (~19MB) | OWASP minimum for interactive login |
| Iterations | 2 | Balance between security and UX latency |
| Parallelism | 1 | Single-threaded for predictable resource use |
| Output | 32 bytes | AES-256 key size |
| Salt | 16 bytes (random) | Unique per transcript |

Each encrypted transcript stores its own KDF params (including salt), so files are self-contained and independently decryptable.

### Authenticated Encryption (cipher.rs)

**Algorithm:** AES-256-GCM (NIST SP 800-38D)

```
encrypt(key, plaintext):
  nonce ← random(12 bytes)
  ciphertext || tag ← AES-256-GCM(key, nonce, plaintext)
  return nonce || ciphertext || tag

decrypt(key, data):
  nonce ← data[0..12]
  ciphertext || tag ← data[12..]
  return AES-256-GCM-decrypt(key, nonce, ciphertext || tag)
```

- **12-byte random nonce** — prepended to ciphertext, extracted on decrypt
- **16-byte authentication tag** — appended by GCM, verified on decrypt
- **No AAD (additional authenticated data)** — transcript metadata is not bound to ciphertext (simplicity for Phase 1; can add in future)

### On-Disk Format (store.rs)

Each transcript is stored as a pretty-printed JSON file:

```json
{
  "version": 1,
  "session_id": "session-001",
  "created_at": "2026-02-16T18:30:00Z",
  "kdf_params": {
    "salt": [/* 16 random bytes */],
    "memory_kib": 19456,
    "iterations": 2,
    "parallelism": 1
  },
  "ciphertext_b64": "base64(nonce || ciphertext || tag)"
}
```

**Filename convention:** `{session_id}.enc.json`

The `.enc.json` suffix is recognized by L1 File Access Guard as a sensitive file (blocked from tool access by default).

## EncryptedStore API

```rust
let store = EncryptedStore::new("/path/to/transcripts")?;

// Write
let path = store.write("session-001", plaintext, "passphrase")?;

// Read
let decrypted = store.read("session-001", "passphrase")?;

// Read from specific file
let decrypted = store.read_file(&path, "passphrase")?;

// List session IDs
let sessions = store.list()?;  // → ["alpha", "beta", "gamma"]

// Delete
store.delete("session-001")?;
```

## Error Handling

All operations return `Result<T, CryptoError>`:

| Variant | Cause |
|---------|-------|
| `Kdf` | Argon2id failure (invalid params, OOM) |
| `Encrypt` | AES-GCM encryption failure |
| `Decrypt` | Wrong passphrase, tampered data, truncated file |
| `Io` | File read/write errors |
| `Json` | Corrupt JSON format |
| `Base64` | Invalid base64 in ciphertext field |

## Resource Budget

| Metric | Value |
|--------|-------|
| RAM (KDF peak) | ~19MB (Argon2id memory parameter) |
| RAM (steady state) | ~6MB |
| Storage per transcript | ~1.4x plaintext size (base64 overhead + JSON wrapper) |
| KDF latency | ~200ms per derive (tuned for interactive use) |

## Security Properties

1. **Confidentiality** — AES-256-GCM with random nonces; each transcript encrypted with a unique derived key (unique salt)
2. **Integrity** — GCM authentication tag detects any tampering
3. **Key stretching** — Argon2id resists brute-force passphrase attacks (19MB memory-hard)
4. **No key reuse** — Fresh random salt per transcript → unique AES key per file
5. **No plaintext metadata leakage** — Session ID is in the filename (by design, for lookup), but transcript content is fully encrypted

## Technology Stack

| Component | Choice | Why |
|-----------|--------|-----|
| KDF | argon2 0.5 (Argon2id) | OWASP recommended, RFC 9106, memory-hard |
| AEAD | aes-gcm 0.10 (RustCrypto) | NIST standard, pure Rust, audited |
| RNG | rand 0.8 (OsRng) | Cryptographically secure random |
| Encoding | base64 0.22 | Standard base64 for JSON-safe ciphertext |
| Serialization | serde + serde_json | Self-describing JSON format |

## Future Work

- **AAD binding:** Include session_id + version in AES-GCM AAD to prevent file renaming attacks
- **Key rotation:** Re-encrypt transcripts with new passphrase without full decrypt/re-encrypt cycle
- **Streaming encryption:** For very large transcripts, use AES-GCM-SIV or chunked encryption
- **Integration with L0 vault:** Derive transcript encryption key from the FPE master key + passphrase (two-factor)

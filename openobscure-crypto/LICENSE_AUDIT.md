# OpenObscure Crypto — Dependency License Audit

> **Audit date:** 2026-02-17
> **Project license:** MIT OR Apache-2.0
> **Verdict:** All dependencies are open source with permissive licenses. No copyleft (GPL/LGPL/AGPL/MPL) found.

---

## Direct Dependencies

| # | Crate | Version | License | Status |
|---|-------|---------|---------|--------|
| 1 | `aes-gcm` | 0.10 | MIT OR Apache-2.0 | OK |
| 2 | `argon2` | 0.5 | MIT OR Apache-2.0 | OK |
| 3 | `rand` | 0.8 | MIT OR Apache-2.0 | OK |
| 4 | `thiserror` | 2.x | MIT OR Apache-2.0 | OK |
| 5 | `serde` | 1.x | MIT OR Apache-2.0 | OK |
| 6 | `serde_json` | 1.x | MIT OR Apache-2.0 | OK |
| 7 | `base64` | 0.22 | MIT OR Apache-2.0 | OK |

Dev-only:

| # | Crate | Version | License | Status |
|---|-------|---------|---------|--------|
| 8 | `tempfile` | 3.x | MIT OR Apache-2.0 | OK |

---

## Notable Transitive Dependencies

| Crate | License | Notes |
|-------|---------|-------|
| `aes` 0.8 | MIT OR Apache-2.0 | AES block cipher (RustCrypto) |
| `ghash` 0.5 | MIT OR Apache-2.0 | GHASH for GCM (RustCrypto) |
| `ctr` 0.9 | MIT OR Apache-2.0 | CTR mode (RustCrypto) |
| `cipher` 0.4 | MIT OR Apache-2.0 | Cipher traits (RustCrypto) |
| `blake2` 0.10 | MIT OR Apache-2.0 | Used internally by Argon2 |
| `password-hash` 0.5 | MIT OR Apache-2.0 | Password hashing traits |
| RustCrypto utilities (`crypto-common`, `block-buffer`, `digest`, `inout`, `cpufeatures`) | MIT OR Apache-2.0 | All permissive |

---

## License Distribution (~25 transitive crates)

| License | Count | Examples |
|---------|-------|---------|
| MIT OR Apache-2.0 | ~25 | All RustCrypto crates, serde, rand, argon2 |

---

## Action Items

1. No copyleft dependencies — crate can be released under MIT OR Apache-2.0
2. All cryptographic dependencies are from the audited RustCrypto project
3. No platform-specific native dependencies (pure Rust throughout)

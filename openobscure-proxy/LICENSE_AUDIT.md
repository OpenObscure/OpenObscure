# OpenObscure Proxy — Dependency License Audit

> **Audit date:** 2026-02-17
> **Project license:** MIT OR Apache-2.0
> **Verdict:** All dependencies are open source with permissive licenses. No copyleft (GPL/LGPL/AGPL/MPL) found.

---

## Direct Dependencies

| # | Crate | Version | License | Added | Status |
|---|-------|---------|---------|-------|--------|
| 1 | `axum` | 0.8 | MIT | Phase 1 | OK |
| 2 | `hyper` | 1.x | MIT | Phase 1 | OK |
| 3 | `hyper-util` | 0.1 | MIT | Phase 1 | OK |
| 4 | `http-body-util` | 0.1 | MIT | Phase 1 | OK |
| 5 | `tokio` | 1.x | MIT | Phase 1 | OK |
| 6 | `tower` | 0.5 | MIT | Phase 1 | OK |
| 7 | `tower-http` | 0.6 | MIT | Phase 1 | OK |
| 8 | `hyper-rustls` | 0.27 | Apache-2.0 OR ISC OR MIT | Phase 1 | OK |
| 9 | `rustls` | 0.23 | Apache-2.0 OR ISC OR MIT | Phase 1 | OK |
| 10 | `fpe` | 0.6 | MIT OR Apache-2.0 | Phase 1 | OK |
| 11 | `aes` | 0.8 | MIT OR Apache-2.0 | Phase 1 | OK |
| 12 | `regex` | 1.x | MIT OR Apache-2.0 | Phase 1 | OK |
| 13 | `serde` | 1.x | MIT OR Apache-2.0 | Phase 1 | OK |
| 14 | `serde_json` | 1.x | MIT OR Apache-2.0 | Phase 1 | OK |
| 15 | `toml` | 0.8 | MIT OR Apache-2.0 | Phase 1 | OK |
| 16 | `keyring` | 3.x | MIT OR Apache-2.0 | Phase 1 | OK |
| 17 | `clap` | 4.x | MIT OR Apache-2.0 | Phase 1 | OK |
| 18 | `tracing` | 0.1 | MIT | Phase 1 | OK |
| 19 | `tracing-subscriber` | 0.3 | MIT | Phase 1 | OK |
| 20 | `bytes` | 1.x | MIT | Phase 1 | OK |
| 21 | `uuid` | 1.x | Apache-2.0 OR MIT | Phase 1 | OK |
| 22 | `rand` | 0.8 | MIT OR Apache-2.0 | Phase 1 | OK |
| 23 | `thiserror` | 2.x | MIT OR Apache-2.0 | Phase 1 | OK |
| 24 | `anyhow` | 1.x | MIT OR Apache-2.0 | Phase 1 | OK |
| 25 | `sha2` | 0.10 | MIT OR Apache-2.0 | Phase 1 | OK |
| 26 | `hex` | 0.4 | MIT OR Apache-2.0 | Phase 1 | OK |
| 27 | `ort` | 2.0.0-rc.11 | Apache-2.0 | Phase 2 | OK |
| 28 | `ndarray` | 0.17 | MIT OR Apache-2.0 | Phase 2 | OK |
| 29 | `libc` | 0.2 | MIT OR Apache-2.0 | Phase 2 | OK |
| 30 | `tracing-appender` | 0.2 | MIT | Phase 2.5 | OK |
| 31 | `memmap2` | 0.9 | MIT OR Apache-2.0 | Phase 2.5 | OK |
| 32 | `image` | 0.25 | MIT OR Apache-2.0 | Phase 3 | OK |
| 33 | `base64` | 0.22 | MIT OR Apache-2.0 | Phase 3 | OK |
| 34 | `kamadak-exif` | 0.5 | BSD-2-Clause | Phase 3 | OK |

Platform-specific:

| # | Crate | Version | License | Platform | Added | Status |
|---|-------|---------|---------|----------|-------|--------|
| 35 | `tracing-oslog` | 0.2 | MIT | macOS | Phase 3 | OK |
| 36 | `tracing-journald` | 0.3 | MIT OR Apache-2.0 | Linux | Phase 3 | OK |

Dev-only:

| # | Crate | Version | License | Status |
|---|-------|---------|---------|--------|
| 37 | `tokio-test` | 0.4 | MIT | OK |
| 38 | `tempfile` | 3.x | MIT OR Apache-2.0 | OK |
| 39 | `wiremock` | 0.6 | MIT OR Apache-2.0 | OK |

---

## Notable Transitive Dependencies

| Crate | License | Notes |
|-------|---------|-------|
| `ring` 0.17 | Apache-2.0 AND ISC | Must comply with both (both permissive). Attribution required. |
| `aws-lc-rs` 1.x | ISC AND (Apache-2.0 OR ISC) | All permissive. |
| `aws-lc-sys` 0.37 | ISC AND (Apache-2.0 OR ISC) AND OpenSSL | **OpenSSL advertising clause** — must include attribution in docs. See THIRD_PARTY_LICENSES. |
| `rustls-webpki` 0.103 | ISC | Permissive. |
| `webpki-roots` 1.x | CDLA-Permissive-2.0 | Data license for CA certs. Permissive. |
| `subtle` 2.x | BSD-3-Clause | Permissive. |
| RustCrypto (`cipher`, `cbc`, `digest`, `crypto-common`, `block-buffer`, `cpufeatures`, `inout`) | MIT OR Apache-2.0 | All permissive. |
| macOS security (`security-framework`, `core-foundation`) | MIT OR Apache-2.0 | All permissive. |
| `num-bigint`, `num-integer`, `num-traits` | MIT OR Apache-2.0 | Used by `fpe` crate. |
| `onnxruntime` (native) | MIT | Pre-compiled binary pulled by `ort-sys`. MIT-licensed, but may bundle platform-specific native dependencies. |

---

## License Distribution (~140 transitive crates)

| License | Count | Examples |
|---------|-------|---------|
| MIT OR Apache-2.0 | ~80 | serde, clap, RustCrypto, rand, keyring, image, ndarray |
| MIT | ~25 | axum, hyper, tokio, tower, tracing, tracing-appender |
| Apache-2.0 | ~3 | ort |
| Apache-2.0 OR ISC OR MIT | ~4 | rustls, hyper-rustls |
| ISC | ~3 | rustls-webpki, untrusted |
| Apache-2.0 AND ISC | 1 | ring |
| ISC AND (Apache-2.0 OR ISC) AND OpenSSL | 1 | aws-lc-sys |
| CDLA-Permissive-2.0 | 1 | webpki-roots |
| BSD-3-Clause | 1 | subtle |
| BSD-2-Clause | 1 | kamadak-exif |

---

## Action Items

1. **THIRD_PARTY_LICENSES file created** — includes OpenSSL, ISC, BSD-3-Clause, BSD-2-Clause, and CDLA-Permissive-2.0 license texts
2. No copyleft dependencies — project can be released under MIT OR Apache-2.0
3. Binary distribution requires OpenSSL attribution notice (from `aws-lc-sys`)
4. `ort` pulls ONNX Runtime as a pre-compiled native library (MIT). The `ort-sys` build script downloads platform-specific binaries — verify these don't bundle additional non-permissive licenses on each target platform
5. `kamadak-exif` is **BSD-2-Clause** (not MIT/Apache) — still permissive, attribution required

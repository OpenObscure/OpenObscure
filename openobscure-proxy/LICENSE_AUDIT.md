# OpenObscure Proxy — Dependency License Audit

> **Audit date:** 2026-02-20
> **Project license:** MIT OR Apache-2.0
> **Verdict:** All crate dependencies are open source and permissive. MPL-2.0 deps (`symphonia`, `uniffi`) are optional features only — file-level copyleft, no project-level impact. No GPL/LGPL/AGPL in crate deps. All ONNX models are permissive (Apache-2.0, MIT) or trained in-house — no GPL/AGPL model dependencies.

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
| 35 | `futures-util` | 0.3 | MIT OR Apache-2.0 | Phase 5 | OK |
| 36 | `whatlang` | 0.16 | MIT | Phase 10 | OK |
| 37 | `symphonia` | 0.5 | MPL-2.0 | Phase 10 (voice, optional) | **Review** |
| 38 | `symphonia-format-ogg` | 0.5 | MPL-2.0 | Phase 10 (voice, optional) | **Review** |
| 39 | `symphonia-codec-vorbis` | 0.5 | MPL-2.0 | Phase 10 (voice, optional) | **Review** |
| 40 | `rubato` | 0.15 | MIT | Phase 10 (voice, optional) | OK |

UniFFI (optional, mobile feature):

| # | Crate | Version | License | Added | Status |
|---|-------|---------|---------|-------|--------|
| 41 | `uniffi` | 0.29 | MPL-2.0 | Phase 7 | **Review** |

Platform-specific:

| # | Crate | Version | License | Platform | Added | Status |
|---|-------|---------|---------|----------|-------|--------|
| 42 | `tracing-oslog` | 0.2 | MIT | macOS | Phase 3 | OK |
| 43 | `tracing-journald` | 0.3 | MIT OR Apache-2.0 | Linux | Phase 3 | OK |
| 44 | `windows` | 0.62 | MIT OR Apache-2.0 | Windows | Phase 7 | OK |

Dev-only:

| # | Crate | Version | License | Status |
|---|-------|---------|---------|--------|
| 45 | `tokio-test` | 0.4 | MIT | OK |
| 46 | `tempfile` | 3.x | MIT OR Apache-2.0 | OK |
| 47 | `wiremock` | 0.6 | MIT OR Apache-2.0 | OK |
| 48 | `criterion` | 0.5 | Apache-2.0 OR MIT | OK |

---

## ONNX Model Licenses

| Model | File | License | Source |
|-------|------|---------|--------|
| ViT-base NSFW Classifier | `nsfw_classifier/nsfw_classifier.onnx` | Apache-2.0 | LukeJacob2023/nsfw-image-detector |
| SCRFD-2.5GF | `scrfd/scrfd_2.5g_bnkps.onnx` | MIT | InsightFace |
| BlazeFace | `blazeface/face_detection_short_range.onnx` | Apache-2.0 | MediaPipe |
| PaddleOCR det | `paddleocr/det_model.onnx` | Apache-2.0 | PaddlePaddle |
| PaddleOCR rec | `paddleocr/rec_model.onnx` | Apache-2.0 | PaddlePaddle |
| R2 TinyBERT | `ri/model.onnx` | Custom (trained in-house) | OpenObscure |
| TinyBERT NER | `ner/model.onnx` | Custom (trained in-house) | OpenObscure |
| KWS Zipformer | `kws/*.onnx` | Apache-2.0 | k2-fsa/sherpa-onnx |

> **Note:** All models are permissive (Apache-2.0, MIT) or trained in-house. No GPL/AGPL model dependencies.
>
> No new Rust crate dependencies were added for the NSFW classifier — it reuses existing `ort`, `image`, and `ndarray`.

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

## License Distribution (~150 transitive crates)

| License | Count | Examples |
|---------|-------|---------|
| MIT OR Apache-2.0 | ~85 | serde, clap, RustCrypto, rand, keyring, image, ndarray, rubato, windows |
| MIT | ~28 | axum, hyper, tokio, tower, tracing, tracing-appender, whatlang |
| Apache-2.0 | ~3 | ort |
| Apache-2.0 OR ISC OR MIT | ~4 | rustls, hyper-rustls |
| ISC | ~3 | rustls-webpki, untrusted |
| Apache-2.0 AND ISC | 1 | ring |
| ISC AND (Apache-2.0 OR ISC) AND OpenSSL | 1 | aws-lc-sys |
| MPL-2.0 | ~4 | symphonia, symphonia-format-ogg, symphonia-codec-vorbis, uniffi |
| CDLA-Permissive-2.0 | 1 | webpki-roots |
| BSD-3-Clause | 1 | subtle |
| BSD-2-Clause | 1 | kamadak-exif |

---

## Action Items

1. **THIRD_PARTY_LICENSES file created** — includes OpenSSL, ISC, BSD-3-Clause, BSD-2-Clause, MPL-2.0, and CDLA-Permissive-2.0 license texts
2. **MPL-2.0 dependencies** — `symphonia` (voice feature, optional) and `uniffi` (mobile feature, optional) are MPL-2.0. MPL-2.0 is file-level copyleft (not project-level like GPL). Modifications to symphonia/uniffi source files must remain MPL-2.0, but the rest of the project is unaffected. No issue for binary distribution.
3. Binary distribution requires OpenSSL attribution notice (from `aws-lc-sys`)
4. `ort` pulls ONNX Runtime as a pre-compiled native library (MIT). The `ort-sys` build script downloads platform-specific binaries — verify these don't bundle additional non-permissive licenses on each target platform
5. `kamadak-exif` is **BSD-2-Clause** (not MIT/Apache) — still permissive, attribution required
6. `whatlang` is MIT — no concerns

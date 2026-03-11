# OpenObscure

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE)
[![Security: Kerckhoffs](https://img.shields.io/badge/Security-Kerckhoffs-success)](SECURITY.md)

OpenObscure is an open-source, on-device privacy firewall that intercepts AI agent traffic to encrypt PII before it leaves your machine and scan LLM responses for manipulation before they reach you.

- **PII protection** — detects 15+ PII types (regex + NER + keywords + multilingual IDs), encrypts with FF1 FPE, redacts faces/text/NSFW in images, and catches PII in voice transcripts
- **Cognitive firewall** — scans every LLM response for persuasion techniques across 7 categories using a dictionary + TinyBERT classifier cascade
- **Runs anywhere** — Gateway proxy (macOS/Linux/Windows) or embedded native library (iOS/Android) via UniFFI bindings, with automatic hardware tier detection

## Choose Your Path

| I want to... | Start here |
|--------------|------------|
| **Try it now** | [Get Started](docs/get-started/) — build, run, and verify in 5 minutes |
| **Understand the architecture** | [Architecture](docs/architecture/system-overview.md) — system overview, data flow, design decisions |
| **Configure** | [Configure](docs/configure/) — FPE, detection engines, full config reference |
| **Integrate with my app** | [Integrate](docs/integrate/) — LLM providers, third-party embedding |
| **Look up API types** | [Reference](docs/reference/) — FFI types, PII coverage, config keys |
| **Contribute** | [Contribute](docs/contribute/) — dev setup, conventions, testing |

## Prerequisites

| Tool | Minimum version | Required for |
|------|----------------|--------------|
| Rust | **1.75** | All builds. Install via [rustup.rs](https://rustup.rs). |
| Cargo | ships with Rust | All builds. No separate minimum. |
| Node.js | **18** | L1 plugin (`openobscure-plugin/`) only. Not needed for the embedded library. CI tests on Node 22. |
| npm | ships with Node.js | L1 plugin only. No separate minimum. |
| Git LFS | any | Downloading NER, NSFW, KWS, and RI model files (`git lfs pull`). Not needed for a regex-only build. |
| Xcode | — | iOS and macOS embedded builds. No minimum pinned; CI runs on macOS 14. |
| Android NDK | — | Android embedded builds. No minimum pinned in build scripts. |
| cargo-ndk | — | Android embedded builds (`./build/build_android.sh`). Install: `cargo install cargo-ndk`. |
| ONNX Runtime | auto-downloaded | All builds. The `ort` crate (`=2.0.0-rc.11`, `download-binaries` feature) fetches the native library at build time — no manual installation needed. |

**Platform support:** macOS (Apple Silicon, x86_64), Linux (x64, ARM64), Windows (x64), iOS, Android.

## License

Dual-licensed under [MIT or Apache-2.0](LICENSE), at your option.

> **Export compliance — DRAFT.** See [EXPORT_CONTROL_NOTICE.md](EXPORT_CONTROL_NOTICE.md). ERN and CCATS are pending. **Binary distribution (GitHub Releases, signed installers, app store submissions) must not begin until the BIS notification and self-classification process are finalized.** Source code distribution is unaffected.

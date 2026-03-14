# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1] - 2026-03-14

### Added

#### NAPI Addon

- First npm publish: `@openobscure/scanner-napi` v0.1.1 and three platform packages live on npm — `darwin-arm64`, `linux-x64-gnu`, `linux-arm64-gnu`
- Each platform package bundles the compiled `.node` binary and the TinyBERT INT8 NER model (`models/ner/`), enabling 15-type detection immediately after `npm install` with no extra model download
- `index.js` and `index.d.ts` committed to the repository so the umbrella package loader is included in the npm tarball

#### L1 Plugin

- `@openobscure/scanner-napi` wired as `optionalDependencies` in `openobscure-plugin/package.json` — the 15-type Rust HybridScanner auto-loads on supported platforms without any configuration

### Fixed

- `openobscure-napi/index.js` was excluded from the published tarball (gitignored) — umbrella package had no loader, causing silent fallback to JS regex on all platforms. Fixed by removing `index.js` and `index.d.ts` from `.gitignore`.

### Notes

- musl (Alpine) and macOS Intel platform packages are deferred — `ort` 2.0 provides no prebuilt binaries for musl targets. Installs on those platforms continue to use the JS regex fallback (5 PII types).

## [0.1.0] - 2026-03-11

### Added

#### L0 Core (Rust)

- FF1 Format-Preserving Encryption for 15 PII types: structured (SSN, credit card, phone, email, address), network/device (IP, MAC, UUID, IBAN, routing number), semantic (person names, organizations, locations via NER), multilingual national IDs (9 languages: es/fr/de/pt/ja/zh/ko/ar), and health/child identifiers
- 4-engine detection ensemble: regex with post-validation (Luhn algorithm, SSN range checks), CRF context scoring, TinyBERT NER (4L-312D, 13.7MB INT8, 0.8ms p50 latency), and keyword/gazetteer dictionary
- Image pipeline: ViT-base 5-class NSFW classifier, SCRFD-2.5GF face detection (Full/Standard tiers) and BlazeFace (Lite tier), PaddleOCR v4 text region detection, EXIF metadata strip — all with solid-fill redaction
- Voice pipeline: sherpa-onnx Zipformer KWS keyword spotting (~5MB INT8) for PII trigger phrase detection in audio
- Cognitive firewall: R1 dictionary (250 phrases across 7 persuasion categories) and R2 TinyBERT classifier cascade (EU AI Act Article 5 alignment)
- Gateway deployment model: sidecar HTTP proxy on 127.0.0.1:18790 (axum/hyper)
- Embedded deployment model: iOS/Android native library via UniFFI Swift/Kotlin bindings
- Hardware tier auto-detection (Full ≥8GB, Standard 4–8GB, Lite <4GB) with `FeatureBudget` struct — compile error if any tier field is omitted
- SSE streaming support for real-time LLM responses
- 4 built-in LLM provider routes: OpenAI, Anthropic, OpenRouter, Ollama; configurable custom providers via `config/openobscure.toml`
- CLI subcommands: `serve`, `key-rotate`, `passthrough`, service install/uninstall
- OS keychain integration for FPE master key storage with environment variable fallback for headless/Docker deployments
- Multilingual PII detection with national ID check-digit validation for 9 languages
- Mobile execution provider selection: CoreML (Apple), NNAPI (Android), CPU fallback
- Mobile API: `sanitizeText`, `sanitizeImage`, `sanitizeAudioTranscript`, `checkAudioPii` via UniFFI

#### L1 Plugin (TypeScript)

- In-process plugin for AI agent frameworks with 3-tier detection engine auto-selection: JS regex (always available), NAPI HybridScanner (15 PII types when addon present), L0 NER endpoint (when proxy running)
- Tool result interception for PII in web scrapes, file reads, and other tool outputs that bypass the proxy
- Memory governance with retention tiers and privacy commands

#### NAPI Addon

- Native addon bridge upgrading L1 JS regex (5 types) to Rust HybridScanner (15 types)
- Persuasion scanner bridge for cognitive firewall access from TypeScript

#### L2 Encrypted Storage (Enterprise)

- AES-256-GCM encrypted storage with Argon2id KDF

#### Testing

- 1,801 tests across L0 Core proxy (1,667), L1 plugin (112), NAPI addon (6), and L2 crypto (16)
- 99.7% recall on PII benchmark corpus (~400 labeled samples)
- Model-gated accuracy tests for NER (TinyBERT F1: 85.6%), image pipeline, and voice pipeline
- Detection validation framework: 40 pure-logic validator tests covering bbox sanity, OCR region validity, NSFW consistency, and precision/recall metrics

[Unreleased]: https://github.com/openobscure/openobscure/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/openobscure/openobscure/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/openobscure/openobscure/releases/tag/v0.1.0

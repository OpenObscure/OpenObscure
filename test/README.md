# test/ — Developer Navigation Guide

This directory contains all test documentation, scripts, data corpora, and reference apps for OpenObscure. It covers three deployment models: **Gateway** (HTTP proxy), **L0 Embedded** (native mobile library), and **L1 Plugin** (in-process TypeScript).

---

## Quick Reference

| File | Audience | Purpose |
|------|----------|---------|
| [TESTING_GUIDE.md](TESTING_GUIDE.md) | All developers | Test infrastructure, corpus, scripts, validation |
| [GATEWAY_TEST.md](GATEWAY_TEST.md) | Gateway users | Hands-on Gateway walkthrough |
| [EMBEDDED_TEST.md](EMBEDDED_TEST.md) | Mobile developers | Hands-on L0 Embedded walkthrough (iOS/Android) |
| [PERFORMANCE.md](PERFORMANCE.md) | All developers | Latency benchmarks and throughput targets |

---

## Guide Descriptions

### [TESTING_GUIDE.md](TESTING_GUIDE.md)
**Start here.** Covers the full test infrastructure:
- Architecture overview: how Gateway, L0 Embedded, and L1 Plugin fit together
- Detection coverage table: which PII types each layer detects and how output differs
- Test corpus: 400-sample PII benchmark dataset in `data/`
- Script inventory: every script in `scripts/` with usage examples
- Gateway vs L1 Plugin comparison (when to use which)
- NAPI native addon smoke tests (upgrades L1 Plugin to Rust HybridScanner)
- Snapshot testing: `generate_snapshot.sh` + `snapshot.json`
- Validation: `expected_results.json` + `validate_results.sh`
- Troubleshooting reference for both deployment models

### [GATEWAY_TEST.md](GATEWAY_TEST.md)
**Gateway hands-on guide.** Step-by-step walkthrough for the HTTP proxy model:
- Prerequisites and setup (`openobscure serve`, mock LLM backend)
- Text PII sanitization and FPE restore via HTTP
- All 15 PII types with expected encrypted output
- Image sanitization (NSFW/face/OCR) over the proxy
- Response integrity checks (R1 dictionary + R2 TinyBERT cognitive firewall)
- SSE streaming, fail-mode, key rotation, auth token passthrough
- Feature parity table: **canonical reference** for all three deployment models

### [EMBEDDED_TEST.md](EMBEDDED_TEST.md)
**L0 Embedded hands-on guide.** Covers the mobile native library (UniFFI Swift/Kotlin):
- Build instructions: iOS XCFramework, Android `.so` (all ABIs), UniFFI binding generation
- Rust API examples: `OpenObscureMobile::new()`, `sanitize_text()`, `restore_text()`, `sanitize_image()`
- Hardware auto-detection and capability tiers (Full / Standard / Lite by RAM)
- Keyword detection (health, child keywords)
- Image sanitization on device
- Swift (iOS) and Kotlin (Android) UniFFI binding examples
- Cross-reference to [GATEWAY_TEST.md#feature-parity](GATEWAY_TEST.md#feature-parity) for the full comparison

### [PERFORMANCE.md](PERFORMANCE.md)
Latency benchmarks and throughput targets for both Gateway and Embedded:
- P50/P99 latency by PII type and scanner mode
- Throughput under concurrent load
- Image pipeline timing (NSFW → face → OCR stages)
- Memory footprint measurements

---

## Directory Structure

```
test/
├── README.md                   # This file
├── TESTING_GUIDE.md            # Test infrastructure + validation reference
├── GATEWAY_TEST.md             # Gateway hands-on guide (canonical feature parity)
├── EMBEDDED_TEST.md            # L0 Embedded (mobile) hands-on guide
├── PERFORMANCE.md              # Latency / throughput benchmarks
├── expected_results.json       # Ground-truth PII extraction for validation
├── snapshot.json               # Snapshot baseline for regression detection
│
├── scripts/                    # Test scripts
│   ├── test_gateway_all.sh     # Run full Gateway PII corpus
│   ├── test_gateway_file.sh    # Test single file through Gateway
│   ├── test_gateway_category.sh # Test by PII category
│   ├── test_embedded_all.mjs   # Run full Embedded PII corpus
│   ├── test_embedded_file.mjs  # Test single file through Embedded
│   ├── test_embedded_category.mjs
│   ├── test_napi_smoke.mjs     # NAPI native addon smoke test
│   ├── test_visual.sh          # Image pipeline test
│   ├── test_cognitive_firewall.sh
│   ├── test_response_integrity.sh
│   ├── test_sse_streaming.sh
│   ├── test_key_rotation.sh
│   ├── test_auth.sh
│   ├── test_fail_mode.sh
│   ├── test_body_limits.sh
│   ├── test_audio.sh
│   ├── test_device_tier.sh
│   ├── test_agent_json.sh      # Nested JSON / agent tool result test
│   ├── test_health.sh
│   ├── generate_snapshot.sh    # Regenerate snapshot.json baseline
│   ├── validate_results.sh     # Compare actual vs expected_results.json
│   ├── echo_server.mjs         # Lightweight echo server for Gateway tests
│   └── mock/                   # Mock servers and data generators
│       ├── ri_mock_server.mjs      # Response integrity mock LLM
│       ├── sse_mock_server.mjs     # SSE streaming mock LLM
│       ├── generate_mock_ner_model.py
│       ├── generate_mock_crf_model.py
│       ├── generate_screenshot.py
│       ├── generate_exif_images.py
│       └── generate_finetune_dataset.py
│
├── data/                       # PII test corpus (400 samples)
│   ├── DATA_COLLECTION_PROMPTS.md  # How the corpus was assembled
│   ├── input/                  # Raw input files, organized by PII category
│   │   ├── PII_Detection/
│   │   ├── Structured_Data_PII/
│   │   ├── Code_Config_PII/
│   │   ├── Multilingual_PII/
│   │   ├── Visual_PII/
│   │   ├── Audio_PII/
│   │   ├── Cognitive_Firewall/
│   │   └── Agent_Tool_Results/
│   └── output/                 # Expected sanitized output, mirroring input/
│       └── (same category folders)
│
├── config/                     # Test-specific TOML config variants
│   ├── test_audit.toml         # Audit/compliance mode config
│   ├── test_fail_closed.toml   # Fail-closed mode (block on FPE error)
│   ├── test_fpe.toml           # FPE key rotation test config
│   ├── test_ri.toml            # Response integrity config
│   └── test_sse.toml           # SSE streaming config
│
└── apps/                       # Reference mobile test apps
    ├── ios/                    # Swift/XCTest app (SwiftUI runner + XCTests)
    │   ├── Package.swift
    │   ├── COpenObscure/       # C bridging header
    │   ├── OpenObscure/        # Swift source
    │   ├── OpenObscureTests/   # XCTest suite
    │   └── XCTests/
    └── android/                # Kotlin/Compose instrumented test app
        ├── app/
        ├── setup.sh
        └── build.gradle.kts
```

---

## Choosing a Starting Point

| I want to… | Go to… |
|------------|--------|
| Understand how the test suite is organized | [TESTING_GUIDE.md](TESTING_GUIDE.md) |
| Run tests against a live Gateway | [GATEWAY_TEST.md](GATEWAY_TEST.md) → scripts/test_gateway_all.sh |
| Test the iOS or Android library | [EMBEDDED_TEST.md](EMBEDDED_TEST.md) → apps/ios/ or apps/android/ |
| Check which PII types each layer detects | [TESTING_GUIDE.md — Detection Coverage](TESTING_GUIDE.md#detection-coverage) |
| See the canonical feature comparison | [GATEWAY_TEST.md — Feature Parity](GATEWAY_TEST.md#feature-parity) |
| Run benchmark / perf tests | [PERFORMANCE.md](PERFORMANCE.md) |
| Add samples to the corpus | [data/input/DATA_COLLECTION_PROMPTS.md](data/input/DATA_COLLECTION_PROMPTS.md) |

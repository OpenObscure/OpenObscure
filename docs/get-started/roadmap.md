# Roadmap

## Current Capabilities

| Category | What's Detected / Protected | Tier |
|----------|----------------------------|------|
| **Structured PII** | Credit cards (Luhn), SSNs (range-validated), phone numbers, emails, API keys (`sk-`, `AKIA`, etc.), IBANs | All |
| **Network / Device** | IPv4, IPv6 (full + compressed), GPS coordinates (4+ decimal), MAC addresses | All |
| **Semantic PII** | Person names, addresses, organizations (NER/CRF), name gazetteer | All |
| **Health / Child** | ~700 keyword terms (multilingual) | All |
| **Multilingual** | 9 languages (es/fr/de/pt/ja/zh/ko/ar) + national ID check-digit validation (DNI, NIR, CPF, My Number, etc.) | All |
| **Visual — Faces** | SCRFD-2.5GF solid-fill redaction | Full / Standard |
| **Visual — Faces** | Ultra-Light RFB-320 solid-fill redaction | Lite |
| **Visual — Text** | PaddleOCR PP-OCRv4 solid-fill redaction in screenshots/images | All (with models) |
| **Visual — NSFW** | ViT-base 5-class classifier (LukeJacob2023/nsfw-image-detector, ~83MB INT8, 224x224 NCHW) — NSFW score = P(hentai) + P(porn) + P(sexy) threshold 0.50, solid-fill entire image | All (with models) |
| **Visual — Metadata** | EXIF strip, screenshot detection (heuristics) | All |
| **Voice** | KWS keyword spotting (sherpa-onnx Zipformer, ~5MB INT8) — PII trigger phrase detection + audio transcript sanitization | All (`voice` feature) |
| **FPE Encryption** | FF1 (NIST SP 800-38G) — format-preserving, per-record tweaks, key rotation with 30s overlap | All |
| **Ensemble Voting** | Cluster-based overlap resolution + agreement bonus across scanner types | Full |
| **Cognitive Firewall** | R1 dictionary (~250 phrases, 7 Cialdini categories) + R2 TinyBERT classifier (4 EU AI Act Article 5 categories), R1→R2 cascade | Full / Standard |
| **SSE Streaming** | Frame accumulation buffer for cross-frame PII/FPE reassembly | All |
| **Platforms** | macOS, Linux (x64 + ARM64), Windows (x64), iOS (device + simulator), Android (arm64-v8a, x86_64) | All |

**Recall:** 99.7% (regex scanner), 100% precision. Hybrid scanner 99.7% overall across ~400-sample benchmark corpus.

## Next Up (0.2.0)

### Embedded Integration Refinements

These improvements address architectural feedback from the v0.1 review. The current implementations are correct and secure; these refinements improve the integration boundary for new adopters.

- **Persistence delegate protocol** — Replace the `mappingJson` column on the host app's conversation entity with a `OpenObscurePersistenceDelegate` protocol. The host app implements `save(state, conversationId)` and `load(conversationId)` on its existing persistence layer. OO owns the serialization format. No new DB columns, no OO internals visible to the integrator.
- **OO-owned sanitize cache** — Move `sanitizedContent` from the host's data model (iOS `MessageSD`) to an internal OO-managed cache keyed by content hash. The host app shouldn't know this cache exists. Android already uses this pattern (`sanitizeCache` on `OpenObscureManager`); iOS should follow.
- **Unified `RestoredContent` return type** — `restore_text()` currently returns a plain `String`. Change to a struct: `RestoredContent { text: String, ri_warning: Option<String>, severity: Option<String> }`. The host app renders whatever OO gives it, matching the Gateway model's behavior. Eliminates the split RI warning assembly (iOS `riWarningLabel()` vs Android `setRiWarning()` + `riVersion` recompose).
- **Android NNAPI / XNNPACK acceleration** — NER latency is 7x slower on Android (580ms) vs iOS (80ms). Evaluate XNNPACK (CPU-optimized SIMD, all devices) and QNN (Qualcomm NPU, Snapdragon-only) as post-launch acceleration paths. See [review notes](../../review-notes/embedded_integration_review_2026-03-19.md#post-launch-android-acceleration-options).

### Gateway & Platform

- **Protection status header** — `X-OpenObscure-Protection` response header so UI clients can display a real-time privacy indicator
- **Real-time breach monitoring** — Rolling window anomaly detection in the live proxy path
- **Streaming redaction** — Incremental redaction for large tool results (requires asynchronous hook support in the host agent)
- **Docker image** — Official container image for sidecar proxy deployments; includes a docker-compose example for common agent setups
- **Pre-built binaries** — GitHub Releases artifacts for macOS (arm64/x86_64), Linux (x64/ARM64), and Windows (x64); no Rust toolchain required to run
- **Homebrew / cargo-binstall support** — One-command install for macOS and Linux users

## On the Horizon (0.3.0+)

- **Document classification** — Detect financial, medical, and legal document types and apply sensitivity-tiered encryption policies automatically
- **Post-quantum FPE migration path** — Tooling to re-encrypt existing vaults as NIST PQC standards for symmetric encryption mature
- **Plugin SDK for custom detectors** — Allow teams to register their own regex patterns, NER models, or validation logic without forking the proxy
- **Protection metrics dashboard** — Local web UI showing per-session PII detection counts, type breakdown, and cognitive firewall trigger history
- **MCP server integration** — Expose OpenObscure as a Model Context Protocol server so agents using the MCP standard can route through it without proxy configuration

## Community Driven

Contributions welcome in any of these areas — no large architectural changes required:

- **Additional NER language models** — Fine-tuned TinyBERT variants for languages beyond the current 9 (es/fr/de/pt/ja/zh/ko/ar)
- **National ID validators** — Check-digit and format validators for additional countries (currently 12 implemented)
- **Agent framework integration guides** — Step-by-step guides for wiring L1 into LangChain, AutoGen, CrewAI, Semantic Kernel, and other frameworks
- **ARM / embedded platform benchmarks** — Performance data for Raspberry Pi, NVIDIA Jetson, and other edge devices running the Gateway model

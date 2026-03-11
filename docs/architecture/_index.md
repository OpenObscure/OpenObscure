# Architecture

- [System Overview](system-overview.md) — two-layer defense model, data flow, detection engines, design decisions
- [L0 Proxy](l0-proxy.md) — Rust proxy internals: module map, request flow, provider routing
- [L1 Plugin](l1-plugin.md) — TypeScript plugin: tool-result redaction, consent, hooks
- [Semantic PII Detection](semantic-pii-detection.md) — HybridScanner: regex, NER, CRF, keywords, ensemble voting
- [Image Pipeline](image-pipeline.md) — NSFW classification, face detection, OCR, screenshot guard
- [Response Integrity](response-integrity.md) — cognitive firewall: R1 dictionary + R2 TinyBERT cascade
- [NAPI Scanner](napi-scanner.md) — native Node.js addon bridging Rust HybridScanner to L1

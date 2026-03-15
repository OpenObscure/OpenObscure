# System Overview

OpenObscure is an on-device privacy firewall that intercepts AI agent traffic at two layers. It encrypts PII before it reaches LLM providers, redacts PII in tool results before they are persisted, and scans LLM responses for manipulation techniques before they reach the user.

Everything runs locally. No cloud components, no telemetry, no external dependencies at runtime.

---

**Contents**

- [Two-Layer Defense-in-Depth](#two-layer-defense-in-depth)
- [Roadmap](#roadmap)
- [Key Design Decisions](#key-design-decisions)
- [Threat Model](#threat-model)
- [Further Reading](#further-reading)

## Two-Layer Defense-in-Depth

```
┌──────────────────────────────────────────────────────────────────┐
│                        AI Agent (host)                           │
│                                                                  │
│   ┌──────────────────────────────────────────────────────────┐   │
│   │  L1 Plugin (in-process TypeScript)                       │   │
│   │                                                          │   │
│   │  • Hooks tool_result_persist (synchronous)               │   │
│   │  • Redacts PII in tool outputs (web scrapes, file reads) │   │
│   │  • NAPI addon: 15 types in-process / JS regex: 5 types   │   │
│   │  • Heartbeat monitor → warns user if L0 is down          │   │
│   └──────────────────────────────────────────────────────────┘   │
│                              │                                   │
│                         HTTP request                             │
└──────────────────────────────┼───────────────────────────────────┘
                               │
                               ▼
┌──────────────────────────────────────────────────────────────────┐
│  L0 Core (standalone Rust binary — 127.0.0.1:18790)            │
│                                                                  │
│  REQUEST PATH                        RESPONSE PATH               │
│  ─────────────                       ──────────────              │
│  1. Parse JSON + code fences         1. FPE decrypt ciphertexts  │
│  2. Image pipeline                   2. Cognitive firewall scan  │
│     • NSFW classifier (ViT-base)     3. Return to agent          │
│     • Face solid-fill (SCRFD)                                    │
│     • OCR solid-fill (PaddleOCR)                                 │
│     • EXIF strip                                                 │
│  3. Voice KWS (sherpa-onnx)                                      │
│  4. Hybrid Scanner                                               │
│     • Regex (10 types, conf=1.0)                                 │
│     • Keywords (~700 terms)                                      │
│     • NER (TinyBERT / DistilBERT)                                │
│     • CRF (fallback)                                             │
│     • Ensemble confidence voting                                 │
│  5. FPE encrypt (FF1-AES256)                                     │
│                                                                  │
└──────────────────────┬───────────────────────────────────────────┘
                       │
                  sanitized HTTPS
                       │
                       ▼
              ┌─────────────────┐
              │  LLM Provider   │
              │  (OpenAI,       │
              │   Anthropic,    │
              │   OpenRouter,   │
              │   Ollama, ...)  │
              └─────────────────┘
```

**Why two layers?** Neither alone is sufficient:

| | L0 (Proxy) | L1 (Plugin) |
|---|------------|-------------|
| **Intercept point** | HTTP requests/responses to LLMs | Tool results within the host agent |
| **PII handling** | FPE encryption (reversible) | Redaction (destructive) |
| **Catches** | All LLM API traffic | Web scrapes, file reads, API outputs |
| **Bypass possible?** | Only if the agent's `base_url` is misconfigured — see [gateway quick start](../get-started/gateway-quick-start.md) for verification | Only if the host agent skips the hook |
| **Runs in** | Standalone Rust binary | Host agent process (Node.js/TypeScript) |

L0 can't see tool results — they're generated inside the host agent and never pass through HTTP. L1 can't intercept before the LLM sees data — it hooks tool result persistence, not outbound requests. Together, they cover both directions.

---

## Roadmap

See [roadmap](../get-started/roadmap.md) — current capability matrix (all 15 PII types, all platforms, all tiers) and planned features.

---

## Key Design Decisions

See [design-decisions.md](design-decisions.md) — rationale for FF1-only, fail-open, per-record tweaks, solid-fill redaction, sequential model loading, and all other core choices.

---

## Threat Model

The proxy intercepts data-in-transit to LLM providers but does not protect data at rest, the LLM provider itself, or the agent host. Compromised OS/root access and side-channel attacks are explicitly out of scope.

---

## Deep-Dives Reading

| Topic | Page |
|-------|------|
| Gateway vs Embedded deployment | [Deployment Models](../get-started/deployment-models.md) |
| Hardware tiers and feature gating | [Deployment Tiers](../get-started/deployment-tiers.md) |
| FPE encryption, key rotation, fail modes | [FPE Configuration](../configure/fpe-configuration.md) |
| Scanner engines, tier mapping, TOML keys | [Detection Engine Configuration](../configure/detection-engine-configuration.md) |
| Full TOML config reference | [Config Reference](../configure/config-reference.md) |
| LLM provider integration (SDK examples) | [Integration Reference](../integrate/integration-reference.md) |
| Embedding in third-party apps | [Third-Party Embedding](../integrate/third-party-embedding.md) |
| L0 Core internals (module map, request flow) | [L0 Core Architecture](l0-core.md) |
| L1 Plugin internals (hooks, detection paths) | [L1 Plugin Architecture](l1-plugin.md) |
| Semantic PII detection (regex, NER, CRF, voting) | [Semantic PII Detection](semantic-pii-detection.md) |
| Image pipeline (NSFW, face, OCR, screenshots) | [Image Pipeline](image-pipeline.md) |
| Cognitive firewall (R1 + R2 cascade) | [Response Integrity](response-integrity.md) |
| NAPI native scanner addon | [NAPI Scanner](napi-scanner.md) |
| Core architectural choices with rationale | [Design Decisions](design-decisions.md) |
| Current capability matrix and planned features | [Roadmap](../get-started/roadmap.md) |
| Export control (cryptography) | [EXPORT_CONTROL_NOTICE.md](../../EXPORT_CONTROL_NOTICE.md) |

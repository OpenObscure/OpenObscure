# System Overview

OpenObscure is an on-device privacy firewall that intercepts AI agent traffic at two layers. It encrypts PII before it reaches LLM providers, redacts PII in tool results before they are persisted, and scans LLM responses for manipulation techniques before they reach the user.

Everything runs locally. No cloud components, no telemetry, no external dependencies at runtime.

---

**Contents**

- [Two-Layer Defense-in-Depth](#two-layer-defense-in-depth)
- [Data Flow](#data-flow)
- [Detection Engines](#detection-engines)
- [Resource Budget](#resource-budget)
- [Authentication Model](#authentication-model)
- [Key Design Decisions](#key-design-decisions)
- [Host Agent Constraints (OpenClaw Reference)](#host-agent-constraints-openclaw-reference)
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

## Data Flow

### Outbound (user message → LLM)

> The examples below use realistic PII formats to illustrate how FPE preserves structure. In your code examples and test suites, prefer fictional formats (e.g., `X00-00-0000`) so that copy-paste errors don't expose real data to LLM providers before OpenObscure is running.

```
User input                "My SSN is 123-45-6789"
    │
    ▼
Agent formats request     { messages: [{ content: "My SSN is 123-45-6789" }] }
    │
    ▼
L0 Core scans            Regex finds SSN at offset 10..21
    │
    ▼
L0 FPE encrypts           "My SSN is 847-29-3156"   ← same format, different digits
    │
    ▼
LLM Provider              Sees "847-29-3156" — can reason about structure,
                           never sees "123-45-6789"
```

### Inbound (LLM response → user)

```
LLM response              "Your SSN ending in 3156..."
    │
    ▼
L0 FPE decrypts           "Your SSN ending in 6789..."   ← original restored
    │
    ▼
L0 Cognitive firewall     Scans for persuasion techniques
    │                      (urgency, scarcity, authority, etc.)
    ▼
Agent → User               Response delivered with optional warning labels
```

### Tool results (agent tools → transcript)

```
Agent tool executes        file_read("medical_records.csv")
    │
    ▼
Tool produces result       "Name: John Smith, DOB: 1985-03-14, ..."
    │
    ▼
L1 hook fires              tool_result_persist (synchronous)
    │
    ▼
L1 redacts                 "Name: [REDACTED], DOB: [REDACTED], ..."
    │
    ▼
Transcript stored          PII never persisted to disk
```

All analysis is on data flowing through the proxy or passed explicitly by the agent — OpenObscure has no file system access of its own.

Full details → [ARCHITECTURE.md — Security Architecture](../../ARCHITECTURE.md#two-layer-defense-in-depth)

---

## Detection Engines

Full engine tables with latency numbers, activation tiers, and voting logic → [ARCHITECTURE.md — Features](../../ARCHITECTURE.md#features)

---

## Resource Budget

Full tier matrix with NER models, image pipeline variants, and RAM ceilings → [ARCHITECTURE.md — Resource Budget](../../ARCHITECTURE.md#resource-budget)

See [Deployment Tiers](../get-started/deployment-tiers.md) for the feature matrix and override instructions.

---

## Authentication Model

Auth tokens from the host agent pass through to the upstream LLM unchanged. Full details → [ARCHITECTURE.md — Authentication Model](../../ARCHITECTURE.md#authentication-model)

---

## Key Design Decisions

Design decisions (FF1-only, fail-open, per-record tweaks, solid-fill redaction, sequential model loading) are documented with rationale in a dedicated reference.

Full details → [Key Design Decisions](design-decisions.md)

---

## Host Agent Constraints (OpenClaw Reference)

Three OpenClaw-specific constraints that shaped OpenObscure's architecture. Other host agents may have different constraints:

1. **Only `tool_result_persist` is wired** — of OpenClaw's 14 defined hooks, only 3 have invocation sites. `before_tool_call`, `message_sending`, etc. are defined in TypeScript types but never called. This is why L0 (HTTP proxy) exists — it's the only way to intercept data *before* the LLM sees it.

2. **`tool_result_persist` is synchronous** — returning a Promise causes OpenClaw to silently skip the hook. All L1 processing must be synchronous.

3. **OpenClaw updates constantly** — 40+ security patches per release. OpenObscure modules touching internal APIs may break. Pin to known-good OpenClaw versions.

---

## Threat Model

The proxy intercepts data-in-transit to LLM providers but does not protect data at rest, the LLM provider itself, or the agent host. Compromised OS/root access and side-channel attacks are explicitly out of scope.

---

## Further Reading

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
| Current capability matrix and planned features | [Roadmap](../reference/roadmap.md) |
| L2 Encrypted Storage | [openobscure-crypto/ARCHITECTURE.md](../../openobscure-crypto/ARCHITECTURE.md) |
| Export control (cryptography) | [EXPORT_CONTROL_NOTICE.md](../../EXPORT_CONTROL_NOTICE.md) |

# OpenObscure

On-device privacy firewall for AI agents: encrypts PII with FF1 Format-Preserving Encryption before it reaches the LLM, scans every response for manipulation.

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE)
[![CI](https://github.com/openobscure/openobscure/actions/workflows/ci.yml/badge.svg)](https://github.com/openobscure/openobscure/actions/workflows/ci.yml)
![Tests](https://img.shields.io/badge/tests-1%2C801%20passing-brightgreen)
![PII Recall](https://img.shields.io/badge/PII%20recall-99.7%25-brightgreen)

## Before / After

What your agent sends:

```
"Charge $1,200 to card 4111-1111-1111-1111, SSN 123-45-6789"
```

What the LLM receives — FF1 Format-Preserving Encryption, same structure, encrypted values:

```
"Charge $1,200 to card 7823-4621-9034-2817, SSN 847-29-3156"
```

After the LLM responds, values are automatically decrypted before reaching your agent. Your real PII never leaves your device.

## What It Does

- **PII encryption** — detects 15+ PII types (regex + CRF + TinyBERT NER ensemble + multilingual national IDs), encrypts each with FF1 FPE so the LLM sees realistic-looking fake data instead of `[REDACTED]` tokens
- **Image pipeline** — solid-fills faces (SCRFD + BlazeFace), redacts OCR text regions (PaddleOCR v4), and blocks NSFW content (ViT-base) before images leave the device
- **Voice pipeline** — keyword spotting (sherpa-onnx Zipformer KWS) detects PII trigger phrases in audio transcripts
- **Cognitive firewall** — scans every LLM response for persuasion techniques across 7 categories using a 250-phrase dictionary + TinyBERT cascade (EU AI Act Article 5 alignment)
- **Runs anywhere** — Gateway sidecar proxy (macOS/Linux/Windows) or native library embedded in iOS/Android apps via UniFFI Swift/Kotlin bindings, with automatic hardware tier detection (Full/Standard/Lite)

## Try It in a Few Minutes

```bash
# 0. Clone
git clone https://github.com/openobscure/openobscure.git && cd openobscure

# 1. Build
cd openobscure-core && cargo build --release

# 2. Generate an FPE key — stored in your OS keychain (first time only)
#    Your OS will prompt for your login password to store the key securely.
./target/release/openobscure --init-key

# 3. Start the proxy in the background (regex-only mode, no model downloads required)
./target/release/openobscure serve &

# 4. Load the auth token (auto-generated and saved on first start)
TOKEN=$(cat ~/.openobscure/.auth-token)

# 5. Point your agent at the proxy — change one line in your code:
#    base_url = "http://127.0.0.1:18790/openai"   # was: "https://api.openai.com"

# 6. Verify
curl -s -H "X-OpenObscure-Token: $TOKEN" http://127.0.0.1:18790/_openobscure/health | jq .status
```

**Test FPE encryption** — scan text for PII (no upstream required):
```bash
TOKEN=$(cat ~/.openobscure/.auth-token)
curl -s -X POST http://127.0.0.1:18790/_openobscure/ner \
  -H "Content-Type: application/json" \
  -H "X-OpenObscure-Token: $TOKEN" \
  -d '{"text": "Call me at 555-867-5309, my SSN is 123-45-6789"}' | jq .
# [{"start":11,"end":23,"type":"phone","confidence":1.0},
#  {"start":35,"end":46,"type":"ssn","confidence":1.0}]
#
# When the same text flows through the proxy to an LLM, matched values are
# FF1-encrypted before leaving your machine: "123-45-6789" → "847-29-3156"
```

For face redaction, OCR, NSFW filtering, NER, and cognitive firewall (requires model downloads): [Gateway Quick Start](docs/get-started/gateway-quick-start.md).

## How It Works

1. Your agent sends a request containing PII (e.g., `"My SSN is 123-45-6789"`)
2. OpenObscure detects PII using a regex + CRF + NER ensemble
3. Each match is encrypted with FF1 FPE — ciphertext looks realistic (e.g., `847-29-3156`) so the LLM can still reason about the data structure
4. The sanitized request goes to the LLM provider
5. The LLM response is decrypted before returning to your agent
6. The cognitive firewall scans the response for persuasion techniques

## Key Design Decisions

**FPE over redaction.** Most PII tools replace sensitive values with `[REDACTED]`. OpenObscure encrypts them to format-preserving ciphertext — the LLM receives a realistic-looking SSN, not a placeholder. This preserves the LLM's ability to reason about structure and context without exposing real data.

**Fail-open.** If the FPE engine encounters an error, the original text is forwarded and processing continues. Privacy protection never becomes an availability blocker for the AI agent.

**On-device, no telemetry.** The proxy runs entirely on your hardware. No request data, no PII, no model outputs leave the machine. OpenObscure has no API keys of its own and makes no outbound connections except to your configured LLM providers.

**Kerckhoffs principle.** Security depends on key secrecy, not algorithm obscurity. The FPE algorithm (FF1/AES-256), detection logic, and model weights are all open source.

## Supported Providers

| Route prefix | Provider | Upstream |
|---|---|---|
| `/openai` | OpenAI | api.openai.com |
| `/anthropic` | Anthropic | api.anthropic.com |
| `/openrouter` | OpenRouter | openrouter.ai |
| `/ollama` | Ollama | localhost:11434 |
| `/custom` | Any OpenAI-compatible API | configurable |

Add providers in `config/openobscure.toml`. See [Integration Reference](docs/integrate/provider_integration.md).

## Choose Your Path

| I want to... | Start here |
|---|---|
| **Install as an end user** | [Setup](setup/README.md) — install with OpenClaw (Gateway) or as a mobile library (Embedded) |
| **Try it as a developer** | [Gateway Quick Start](docs/get-started/gateway-quick-start.md) — build, run, and verify in 5 minutes |
| **Run as a container** | [Docker Quick Start](docs/get-started/docker-quick-start.md) — pull and run in 2 commands |
| **Understand the architecture** | [Architecture](ARCHITECTURE.md) — system overview, data flow, design decisions |
| **Configure detection or encryption** | [Config Reference](docs/configure/config-reference.md) — every TOML key, FPE, detection engines |
| **Integrate with my app** | [Integration Reference](docs/integrate/provider_integration.md) — LLM providers, third-party embedding |
| **Look up API types** | [API Reference](docs/reference/api-reference.md) — FFI types, PII coverage, fail behavior |
| **Run the test suite** | [test/README.md](test/README.md) — PII corpus, gateway + L1 plugin tests, validation scripts |
| **Contribute** | [Contributing](docs/contribute/contributing.md) — dev setup, conventions, testing |
| **Roadmap** | [Roadmap](docs/get-started/roadmap.md) — current capabilities, v0.2 plans, community-driven items |

## Prerequisites

| Tool | Minimum version | Required for |
|---|---|---|
| Rust | **1.75** | All builds. Install via [rustup.rs](https://rustup.rs). |
| Node.js | **18** | L1 plugin only (`openobscure-plugin/`). Not needed for the proxy or embedded library. |
| Git LFS | any | Model files (NER, NSFW, KWS, RI). Skip for regex-only builds. |
| Xcode | — | iOS/macOS embedded builds. |
| Android NDK + cargo-ndk | — | Android embedded builds (`cargo install cargo-ndk`). |

ONNX Runtime is auto-downloaded at build time by the `ort` crate — no manual installation needed.

**Platform support:** macOS (Apple Silicon, x86_64), Linux (x64, ARM64), Windows (x64), iOS, Android.

## License

Dual-licensed under [MIT or Apache-2.0](LICENSE), at your option.

## Acknowledgements

OpenObscure was developed using [Claude Code](https://claude.ai/code) (Anthropic) as an
AI development assistant. All architecture, security design, cryptographic decisions,
cognitive firewall design, threat modeling, and technical judgment are the author's own.

This project is also a research artifact: building a privacy firewall for AI agents
*using* an AI agent creates a feedback loop that directly informed the design —
particularly the cognitive firewall and data flow boundaries.

# OpenObscure

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE)
[![Security: Kerckhoffs](https://img.shields.io/badge/Security-Kerckhoffs-success)](SECURITY.md)

**The Endpoint Privacy Firewall for AI Agents.**

OpenObscure is an open-source security sidecar that intercepts, sanitizes, and encrypts PII (Personally Identifiable Information) *before* it leaves your device. It is designed to work with any AI agent. Includes first-class [OpenClaw](https://github.com/openclaw/openclaw) integration.

> **Verify, Don't Trust.** OpenObscure runs entirely on `localhost`. No remote servers, no telemetry, no cloud dependencies.

---

## Architecture

OpenObscure is not a monolithic app. It uses a **Sidecar + Plugin** hybrid architecture to provide Defense-in-Depth:

```mermaid
graph TD
    User[User Device]
    subgraph Localhost
        Gateway[AI Agent Gateway] -->|Tool Results| L1[L1 Plugin - TS]
        L1 -->|Redacted Text| Storage[(Local Storage)]
        Gateway -->|HTTP Request| L0[L0 Proxy - Rust]
        L0 -->|FPE Encryption| L0
    end
    L0 -->|Encrypted JSON| Cloud[LLM Provider]
```

| Layer | Language | What it does |
|-------|----------|-------------|
| **L0** — PII Proxy | Rust | Intercepts HTTP traffic, scans JSON for PII, encrypts with FF1 FPE. Processes images (face blur, OCR text blur, EXIF strip). Includes compliance CLI (`openobscure compliance ...`). |
| **L1** — Gateway Plugin | TypeScript | Hooks tool results, redacts PII, blocks sensitive file reads, manages GDPR consent and memory governance. |
| **L2** — Encryption Layer | Rust | AES-256-GCM encryption for session transcripts at rest. |

For the full architecture, see [ARCHITECTURE.md](ARCHITECTURE.md).

---

## Quick Start

### Prerequisites

- **Rust** 1.75+ (for L0 proxy and L2 crypto)
- **Node.js** 20+ (for L1 plugin)
- An AI agent that makes HTTP calls to an LLM provider (OpenClaw, custom agents, etc.)

### 1. Build the proxy

```bash
cd openobscure-proxy
cargo build --release
```

### 2. Generate an FPE key (first time only)

```bash
cargo run --release -- --init-key
```

This stores a 256-bit AES key in your OS keychain. For headless/Docker environments, set `OPENOBSCURE_MASTER_KEY` (64 hex chars) instead.

### 3. Start the proxy

```bash
cargo run --release -- -c config/openobscure.toml
```

The proxy listens on `127.0.0.1:18790` by default.

### 4. Verify

```bash
curl -H "X-OpenObscure-Token: $(cat ~/.openobscure/.auth-token)" \
     http://127.0.0.1:18790/_openobscure/health
```

You should see a JSON response with `"status": "ok"`.

### OpenClaw Integration

Point OpenClaw's LLM traffic through the proxy:

```
LLM_API_BASE=http://127.0.0.1:18790
```

Optionally, copy `openobscure-plugin/` into OpenClaw's `extensions/` directory and enable it in OpenClaw's plugin config for L1 in-process redaction.

### Generic Integration (Any AI Agent)

Any AI agent that sends HTTP requests to an LLM provider can route traffic through the L0 proxy. Set your agent's LLM base URL to the proxy address:

```
http://127.0.0.1:18790
```

The proxy transparently intercepts JSON payloads, scans for PII, applies FF1 Format-Preserving Encryption, and forwards the sanitized request to the upstream LLM provider. Responses are decrypted before being returned to your agent.

For programmatic access to the L1 redaction and file-access guard from TypeScript/JavaScript, import directly from the plugin core:

```typescript
import { redactPii, checkFileAccess } from "openobscure-plugin/core";

// Scan text for PII
const result = redactPii(toolOutput);
if (result.count > 0) toolOutput = result.text;

// Check file safety before reading
const check = checkFileAccess("/path/to/file");
if (!check.allowed) throw new Error(check.reason);
```

This allows any agent — not just OpenClaw — to leverage OpenObscure's PII redaction and file access controls as a library.

---

## Configuration

OpenObscure is configured via `config/openobscure.toml`. Key sections:

```toml
[proxy]
listen_addr = "127.0.0.1:18790"
fail_mode = "open"          # "open" or "closed"

[scanner]
respect_code_fences = true  # Skip PII inside markdown code blocks
nested_json_depth = 2       # Scan PII inside escaped JSON strings

[image]
enabled = true
face_detection = true
ocr_enabled = true
ocr_tier = "detect_and_blur"  # "detect_and_blur" or "full_recognition"
max_dimension = 960

[logging]
json_output = false
pii_scrub = true
```

See `config/openobscure.toml` for all available options.

---

## Running Tests

```bash
# L0 Proxy (306 tests)
cd openobscure-proxy && cargo test

# L2 Crypto (16 tests)
cd openobscure-crypto && cargo test

# L1 Plugin (96 tests)
cd openobscure-plugin && npm test
```

**Total: 418 tests** across all components.

---

## How It Works

OpenObscure uses **Format-Preserving Encryption (FF1)** to replace PII with realistic-looking ciphertext. The LLM sees plausible data, preserving conversational context, while the real values never leave your device.

```
Original:    "My card is 4111-1111-1111-1111"
Encrypted:   "My card is 8714-3927-6051-2483"  ← sent to LLM
LLM reply:   "The card ending in 2483..."
Decrypted:   "The card ending in 1111..."       ← returned to user
```

PII detection uses a hybrid approach:
- **Regex** with post-validation (Luhn for credit cards, range checks for SSNs)
- **NER/CRF** (TinyBERT INT8) for semantic detection (names, addresses, orgs)
- **Keyword dictionary** (~700 terms) for health and child-related terms
- **Image pipeline** (BlazeFace + PaddleOCR ONNX) for visual PII in photos

---

## Security

OpenObscure follows **Kerckhoffs's principle** — security depends on the secrecy of keys, not code. All algorithms (FF1, AES-256-GCM, Argon2id) are public NIST/OWASP standards. Publishing source code does not weaken the system.

Key properties:
- **No telemetry** — zero outbound connections beyond forwarded LLM requests
- **Localhost-only** — proxy binds to `127.0.0.1`, not `0.0.0.0`
- **No default credentials** — FPE key must be explicitly generated
- **Memory-safe** — L0 and L2 are written in Rust

For the full security policy and vulnerability reporting instructions, see [SECURITY.md](SECURITY.md).

For the threat model, see [ARCHITECTURE.md — Threat Model](ARCHITECTURE.md#threat-model).

---

## Export Control

This software contains cryptographic functionality (AES-256-GCM, FF1, Argon2id, TLS) and may be subject to export restrictions. See [EXPORT_CONTROL_NOTICE.md](EXPORT_CONTROL_NOTICE.md) for details.

---

## Project Status

| Phase | Status | Coverage |
|-------|--------|----------|
| Phase 1 — Structured PII (regex + FPE) | Complete | 78% |
| Phase 2 — Semantic PII (NER + GDPR) | Complete | 91% |
| Phase 2.5 — Unified Logging | Complete | 91% |
| Phase 3 — Visual PII (faces + OCR) | Complete | 95% |
| Phase 4 — Compliance CLI + Hardening | Complete | 97% |
| Phase 5 — Key Rotation + Benchmarks | Complete | 97% |
| Phase 6 — Ensemble Recall + Cleanup | Complete | 97% |

---

## Contributing

Contributions are welcome. Please:

1. Read [ARCHITECTURE.md](ARCHITECTURE.md) to understand the system design
2. Run the full test suite before submitting PRs
3. Follow existing code conventions (Rust: `cargo clippy`, TypeScript: strict mode)
4. Report security vulnerabilities via [GitHub private reporting](SECURITY.md), not public issues

---

## License

OpenObscure is dual-licensed under **MIT** or **Apache-2.0**, at your option.

- [LICENSE](LICENSE) (MIT with Apache-2.0 option)
- Each component's dependency licenses are audited in their respective `LICENSE_AUDIT.md` files

See [EXPORT_CONTROL_NOTICE.md](EXPORT_CONTROL_NOTICE.md) for cryptographic export control information.
